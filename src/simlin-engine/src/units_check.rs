// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::result::Result as StdResult;

use crate::ast::{Ast, BinaryOp, Expr2};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{
    Canonical, EquationError, ErrorCode, Ident, Result, UnitError, UnitResult, canonicalize,
};
use crate::datamodel::UnitMap;
use crate::model::ModelStage1;
use crate::units::{Context, UnitOp, Units, combine};
use crate::variable::Variable;

// Type alias to reduce complexity
type UnitErrorList = Vec<(Ident<Canonical>, UnitError)>;

struct UnitEvaluator<'a> {
    #[allow(dead_code)]
    ctx: &'a Context,
    model: &'a ModelStage1,
    inferred_units: &'a HashMap<Ident<Canonical>, UnitMap>,
    // units for module inputs
    time: Variable,
}

impl UnitEvaluator<'_> {
    fn check(&self, expr: &Expr2) -> UnitResult<Units> {
        use UnitError::ConsistencyError;
        match expr {
            Expr2::Const(_, _, _) => Ok(Units::Constant),
            Expr2::Var(ident, _, loc) => {
                let units: &UnitMap = if ident.as_str() == "time"
                    || ident.as_str() == "initial_time"
                    || ident.as_str() == "final_time"
                {
                    // we created this time variable just for unit checking, it is definitely Some
                    self.time.units().unwrap()
                } else {
                    // use the variable's explicitly defined units unless they don't exist.
                    // if they don't exist, try to use any inferred units (this handles modules)
                    self.model
                        .variables
                        .get(ident)
                        .and_then(|var| var.units())
                        .or_else(|| self.inferred_units.get(ident))
                        .ok_or_else(|| {
                            ConsistencyError(
                                ErrorCode::DoesNotExist,
                                *loc,
                                Some(format!("can't find or no units for dependency '{ident}'")),
                            )
                        })?
                };

                Ok(Units::Explicit(units.clone()))
            }
            Expr2::App(builtin, _, _) => {
                match builtin {
                    BuiltinFn::Inf | BuiltinFn::Pi => Ok(Units::Constant),
                    BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => Ok(Units::Explicit(
                        self.time.units().cloned().unwrap_or_default(),
                    )),
                    BuiltinFn::IsModuleInput(_, _) => {
                        // returns a bool, which is unitless
                        Ok(Units::Explicit(UnitMap::new()))
                    }
                    BuiltinFn::Lookup(table_expr, _, loc)
                    | BuiltinFn::LookupForward(table_expr, _, loc)
                    | BuiltinFn::LookupBackward(table_expr, _, loc) => {
                        // lookups have the units specified on the table
                        let table_name = match table_expr.as_ref() {
                            Expr2::Var(name, _, _) => name.clone(),
                            Expr2::Subscript(name, _, _, _) => name.clone(),
                            _ => {
                                return Err(ConsistencyError(
                                    ErrorCode::DoesNotExist,
                                    *loc,
                                    Some(
                                        "lookup table expression must be a variable or subscript"
                                            .to_string(),
                                    ),
                                ));
                            }
                        };
                        if let Some(units) = self
                            .model
                            .variables
                            .get(&table_name)
                            .and_then(|var| var.units())
                            .or_else(|| self.inferred_units.get(&table_name))
                        {
                            Ok(Units::Explicit(units.clone()))
                        } else {
                            Err(ConsistencyError(
                                ErrorCode::DoesNotExist,
                                *loc,
                                Some(format!(
                                    "can't find or no units for dependency '{table_name}'",
                                )),
                            ))
                        }
                    }
                    BuiltinFn::Abs(a)
                    | BuiltinFn::Arccos(a)
                    | BuiltinFn::Arcsin(a)
                    | BuiltinFn::Arctan(a)
                    | BuiltinFn::Cos(a)
                    | BuiltinFn::Exp(a)
                    | BuiltinFn::Int(a)
                    | BuiltinFn::Ln(a)
                    | BuiltinFn::Log10(a)
                    | BuiltinFn::Sign(a)
                    | BuiltinFn::Sin(a)
                    | BuiltinFn::Sqrt(a)
                    | BuiltinFn::Tan(a)
                    | BuiltinFn::Size(a)
                    | BuiltinFn::Stddev(a)
                    | BuiltinFn::Sum(a) => self.check(a),
                    BuiltinFn::Mean(args) => {
                        let args = args
                            .iter()
                            .map(|arg| self.check(arg))
                            .collect::<UnitResult<Vec<_>>>()?;

                        if args.is_empty() {
                            return Ok(Units::Constant);
                        }

                        // find the first non-constant argument
                        let arg0 = args
                            .iter()
                            .find(|arg| matches!(arg, Units::Explicit(_)))
                            .cloned();
                        match arg0 {
                            Some(arg0) => {
                                if args.iter().all(|arg| arg0.equals(arg)) {
                                    Ok(arg0)
                                } else {
                                    let expected = match arg0 {
                                        Units::Explicit(units) => units,
                                        Units::Constant => Default::default(),
                                    };
                                    Err(ConsistencyError(
                                        ErrorCode::UnitDefinitionErrors,
                                        expr.get_loc(),
                                        Some(format!(
                                            "expected all arguments to mean() to have the same units '{expected}'",
                                        )),
                                    ))
                                }
                            }
                            // all args were constants, so we're good
                            None => Ok(Units::Constant),
                        }
                    }
                    BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
                        let a_units = self.check(a)?;
                        if let Some(b) = b {
                            let b_units = self.check(b)?;
                            if !a_units.equals(&b_units) {
                                let a_units = match a_units {
                                    Units::Explicit(units) => units,
                                    Units::Constant => Default::default(),
                                };
                                let b_units = match b_units {
                                    Units::Explicit(units) => units,
                                    Units::Constant => Default::default(),
                                };
                                let loc = a.get_loc().union(&b.get_loc());
                                return Err(ConsistencyError(
                                    ErrorCode::UnitDefinitionErrors,
                                    loc,
                                    Some(format!(
                                        "expected left and right argument units to match, but '{a_units}' and '{b_units}' don't",
                                    )),
                                ));
                            }
                        }
                        Ok(a_units)
                    }
                    BuiltinFn::Pulse(_, _, _)
                    | BuiltinFn::Ramp(_, _, _)
                    | BuiltinFn::Step(_, _) => Ok(Units::Constant),
                    BuiltinFn::SafeDiv(a, b, c) => {
                        let div = Expr2::Op2(
                            BinaryOp::Div,
                            a.clone(),
                            b.clone(),
                            None,
                            a.get_loc().union(&b.get_loc()),
                        );
                        let units = self.check(&div)?;

                        if let Some(c) = c {
                            let c_units = self.check(c)?;
                            if !c_units.equals(&units) {
                                return Err(ConsistencyError(
                                    ErrorCode::UnitMismatch,
                                    c.get_loc(),
                                    Some(format!(
                                        "SAFEDIV fallback has units '{}' but expected '{}'",
                                        c_units.to_unit_map(),
                                        units.to_unit_map()
                                    )),
                                ));
                            }
                        }

                        Ok(units)
                    }
                    BuiltinFn::Rank(a, _rest) => self.check(a),
                }
            }
            Expr2::Subscript(base_name, _, _, loc) => {
                // A subscripted expression has the same units as the base array variable
                if let Some(units) = self
                    .model
                    .variables
                    .get(base_name)
                    .and_then(|var| var.units())
                    .or_else(|| self.inferred_units.get(base_name))
                {
                    Ok(Units::Explicit(units.clone()))
                } else {
                    Err(UnitError::ConsistencyError(
                        ErrorCode::DoesNotExist,
                        *loc,
                        Some(format!(
                            "can't find or no units for subscripted variable '{base_name}'",
                        )),
                    ))
                }
            }
            Expr2::Op1(_, l, _, _) => self.check(l),
            Expr2::Op2(op, l, r, _, _) => {
                let lunits = self.check(l)?;
                let runits = self.check(r)?;

                match op {
                    BinaryOp::Add | BinaryOp::Sub => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Constant, Units::Explicit(units))
                        | (Units::Explicit(units), Units::Constant) => Ok(Units::Explicit(units)),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            if lunits != runits {
                                let details = Some(format!(
                                    "expected left and right argument units to match, but '{lunits}' and '{runits}' don't",
                                ));
                                let loc = l.get_loc().union(&r.get_loc());
                                Err(ConsistencyError(ErrorCode::UnitMismatch, loc, details))
                            } else {
                                Ok(Units::Explicit(lunits))
                            }
                        }
                    },
                    BinaryOp::Exp | BinaryOp::Mod => Ok(lunits),
                    BinaryOp::Mul => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Explicit(units), Units::Constant)
                        | (Units::Constant, Units::Explicit(units)) => Ok(Units::Explicit(units)),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            Ok(Units::Explicit(combine(UnitOp::Mul, lunits, runits)))
                        }
                    },
                    BinaryOp::Div => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Explicit(units), Units::Constant) => Ok(Units::Explicit(units)),
                        (Units::Constant, Units::Explicit(units)) => {
                            Ok(Units::Explicit(combine(UnitOp::Div, UnitMap::new(), units)))
                        }
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            Ok(Units::Explicit(combine(UnitOp::Div, lunits, runits)))
                        }
                    },
                    BinaryOp::Gt
                    | BinaryOp::Lt
                    | BinaryOp::Gte
                    | BinaryOp::Lte
                    | BinaryOp::Eq
                    | BinaryOp::Neq
                    | BinaryOp::And
                    | BinaryOp::Or => {
                        // binary comparisons result in unitless quantities
                        Ok(Units::Explicit(UnitMap::new()))
                    }
                }
            }
            Expr2::If(_, l, r, _, loc) => {
                let lunits = self.check(l)?;
                let runits = self.check(r)?;

                if !lunits.equals(&runits) {
                    return Err(ConsistencyError(
                        ErrorCode::UnitMismatch,
                        *loc,
                        Some(format!(
                            "IF branches have different units: then '{}', else '{}'",
                            lunits.to_unit_map(),
                            runits.to_unit_map()
                        )),
                    ));
                }

                Ok(lunits)
            }
        }
    }
}

// check uses the model's variables' equations and unit definitions to
// calculate the concrete units for each equation.  The outer result
// indicates if we had a problem running the analysis.  The inner result
// returns a list of unit problems, if there was one.
pub fn check(
    ctx: &Context,
    inferred_units: &HashMap<Ident<Canonical>, UnitMap>,
    model: &ModelStage1,
) -> Result<StdResult<(), UnitErrorList>> {
    use UnitError::{ConsistencyError, DefinitionError};
    let mut errors: Vec<(Ident<Canonical>, UnitError)> = vec![];

    // Module stock/flow relationships are validated when each submodel is processed.
    // Cross-module connections (module inputs/outputs) are handled in unit inference,
    // which creates constraints ensuring input variables match across model boundaries.

    // get the main model
    // iterate over the variables
    // for each variable, evaluate the equation given the unit context
    // if the result doesn't match the expected thing, accumulate an error

    let time_units_name =
        canonicalize(ctx.sim_specs.time_units.as_deref().unwrap_or("time")).into_owned();
    let time_units: UnitMap = ctx
        .lookup(&time_units_name)
        .cloned()
        .unwrap_or_else(|| [(time_units_name.clone(), 1)].iter().cloned().collect());
    let one_over_time: UnitMap = combine(UnitOp::Div, Default::default(), time_units.clone());

    let units = UnitEvaluator {
        ctx,
        model,
        inferred_units,
        time: Variable::Var {
            ident: Ident::new("time"),
            ast: None,
            init_ast: None,
            eqn: None,
            units: Some(time_units),
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        },
    };

    for (ident, var) in model.variables.iter() {
        if var.table().is_some() {
            // if a variable has a graphical function the equation is fed into
            // that function like `f(eqn)` -- the units are just whatever is
            // specified on the variable (like a constant would be)
            continue;
        }

        // Check that all elements of arrayed expressions have consistent units,
        // even when the array variable has no declared units
        if let Some(Ast::Arrayed(_, asts)) = var.ast() {
            let mut first_units: Option<UnitMap> = None;
            for (element, expr) in asts.iter() {
                match units.check(expr) {
                    Ok(Units::Explicit(element_units)) => {
                        if let Some(ref existing) = first_units {
                            if *existing != element_units {
                                let loc = expr.get_loc();
                                errors.push((
                                    ident.clone(),
                                    ConsistencyError(
                                        ErrorCode::UnitMismatch,
                                        Loc::new(loc.start.into(), loc.end.into()),
                                        Some(format!(
                                            "array element '{}' has units '{}' but previous element(s) have units '{}'",
                                            element, element_units, existing
                                        )),
                                    ),
                                ));
                            }
                        } else {
                            first_units = Some(element_units);
                        }
                    }
                    Ok(Units::Constant) => {
                        // Constants are compatible with any units
                    }
                    Err(ConsistencyError(ErrorCode::DoesNotExist, _, _)) => {
                        // If we can't determine units for an element (e.g., it uses a module
                        // that doesn't have inferred units yet), skip the consistency check.
                        // Other error types are propagated as actual errors.
                    }
                    Err(err) => {
                        errors.push((ident.clone(), err));
                    }
                }
            }
        }

        if let Some(expected) = var.units() {
            if let Variable::Stock {
                ident,
                inflows,
                outflows,
                ..
            } = var
            {
                let stock_ident = ident;
                let expected_flow_units =
                    combine(UnitOp::Mul, expected.clone(), one_over_time.clone());
                let mut check_flows = |flows: &Vec<Ident<Canonical>>| {
                    for ident in flows.iter() {
                        if let Some(var) = model.variables.get(ident)
                            && let Some(units) = var.units()
                            && expected_flow_units != *units
                        {
                            let details = format!(
                                "expected units '{units}' to match the units expected by the attached stock {stock_ident} ({expected_flow_units})"
                            );
                            errors.push((
                                Ident::new(var.ident()),
                                DefinitionError(
                                    EquationError {
                                        code: ErrorCode::UnitMismatch,
                                        start: 0,
                                        end: 0,
                                    },
                                    Some(details),
                                ),
                            ));
                        }
                    }
                };
                check_flows(inflows);
                check_flows(outflows);
            }
            if let Some(ast) = var.ast() {
                match ast {
                    Ast::Scalar(expr) => match units.check(expr) {
                        Ok(Units::Explicit(actual)) => {
                            if actual != *expected {
                                let details = format!(
                                    "computed units '{}' don't match specified units",
                                    &actual,
                                );
                                let loc = expr.get_loc();
                                errors.push((
                                    ident.clone(),
                                    ConsistencyError(
                                        ErrorCode::UnitMismatch,
                                        Loc::new(loc.start.into(), loc.end.into()),
                                        Some(details),
                                    ),
                                ))
                            }
                        }
                        Ok(Units::Constant) => {
                            // definitionally we're fine
                        }
                        Err(err) => {
                            errors.push((ident.clone(), err));
                        }
                    },
                    Ast::ApplyToAll(_, expr) => match units.check(expr) {
                        Ok(Units::Explicit(actual)) => {
                            if actual != *expected {
                                let details = format!(
                                    "computed units '{}' don't match specified units",
                                    &actual,
                                );
                                let loc = expr.get_loc();
                                errors.push((
                                    ident.clone(),
                                    ConsistencyError(
                                        ErrorCode::UnitMismatch,
                                        Loc::new(loc.start.into(), loc.end.into()),
                                        Some(details),
                                    ),
                                ))
                            }
                        }
                        Ok(Units::Constant) => {
                            // definitionally we're fine
                        }
                        Err(err) => {
                            errors.push((ident.clone(), err));
                        }
                    },
                    Ast::Arrayed(_, asts) => {
                        // Check each element expression in the arrayed variable
                        for (_element, expr) in asts.iter() {
                            match units.check(expr) {
                                Ok(Units::Explicit(actual)) => {
                                    if actual != *expected {
                                        let details = format!(
                                            "computed units '{}' don't match specified units",
                                            &actual,
                                        );
                                        let loc = expr.get_loc();
                                        errors.push((
                                            ident.clone(),
                                            ConsistencyError(
                                                ErrorCode::UnitMismatch,
                                                Loc::new(loc.start.into(), loc.end.into()),
                                                Some(details),
                                            ),
                                        ))
                                    }
                                }
                                Ok(Units::Constant) => {
                                    // definitionally we're fine
                                }
                                Err(err) => {
                                    errors.push((ident.clone(), err));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // units checking uses the model's equations and variable's
    // unit definitions to calculate the concrete units for each
    // equation.  If these don't match the units as defined, we
    // log an error.
    Ok(Err(errors))
}
