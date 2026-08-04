// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};
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

/// The numeric value of a literal exponent expression, seeing through unary
/// negation (the lexer takes no leading sign, so `x^-2`'s exponent is
/// `Op1(Negative, Const(2))`). `None` for anything non-literal.
/// Shared with `units_infer`, which applies the same `^` unit semantics.
pub(crate) fn literal_exponent(expr: &Expr2) -> Option<f64> {
    use crate::ast::UnaryOp;
    match expr {
        Expr2::Const(_, lit, _) => Some(lit.value()),
        Expr2::Op1(UnaryOp::Negative, inner, _, _) => literal_exponent(inner).map(|n| -n),
        _ => None,
    }
}

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
                    | BuiltinFn::Tan(a)
                    | BuiltinFn::Size(a)
                    | BuiltinFn::Stddev(a)
                    | BuiltinFn::Sum(a) => self.check(a),
                    // SQRT halves unit exponents (Vensim fn_sqrt:
                    // "SQRT(units*units) --> units"); an argument whose units
                    // are not a perfect square has no representable root.
                    BuiltinFn::Sqrt(a) => match self.check(a)? {
                        Units::Constant => Ok(Units::Constant),
                        Units::Explicit(units) => match crate::units::try_sqrt(&units) {
                            Some(root) => Ok(Units::Explicit(root)),
                            None => Err(ConsistencyError(
                                ErrorCode::UnitMismatch,
                                a.get_loc(),
                                Some(format!(
                                    "the argument to SQRT has units '{units}', which is not a \
                                     perfect square, so its square root has no valid units"
                                )),
                            )),
                        },
                    },
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
                            // A literal argument is unit-polymorphic:
                            // `MAX(0, x)` has x's units.
                            return Ok(a_units.first_explicit(b_units));
                        }
                        Ok(a_units)
                    }
                    BuiltinFn::Quantum(a, _) => self.check(a),
                    // SSHAPE(x, bottom, top) = bottom + (top-bottom)*sigmoid(x)
                    // (vm.rs), so bottom and top carry the result units and
                    // must agree; a literal in either position is
                    // unit-polymorphic. x is visited so errors inside it
                    // surface, but its units are unconstrained (a 0..1 input).
                    BuiltinFn::Sshape(x, bottom, top) => {
                        self.check(x)?;
                        let bottom_units = self.check(bottom)?;
                        let top_units = self.check(top)?;
                        if !bottom_units.equals(&top_units) {
                            return Err(ConsistencyError(
                                ErrorCode::UnitMismatch,
                                bottom.get_loc().union(&top.get_loc()),
                                Some(format!(
                                    "SSHAPE bottom has units '{}' but top has units '{}'",
                                    bottom_units.to_unit_map(),
                                    top_units.to_unit_map()
                                )),
                            ));
                        }
                        Ok(bottom_units.first_explicit(top_units))
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
                            // A constant-classified quotient (both operands
                            // literals) takes the explicit fallback's units:
                            // the same literal-polymorphism rule as MAX/MIN.
                            return Ok(units.first_explicit(c_units));
                        }

                        Ok(units)
                    }
                    BuiltinFn::Rank(a, _) => {
                        // Check the ranked array for internal consistency, but a
                        // RANK result is a dimensionless ordinal position, not
                        // the units of the array being ranked.
                        self.check(a)?;
                        Ok(Units::Explicit(UnitMap::new()))
                    }
                    BuiltinFn::VectorSelect(_, expr_array, _, _, _) => self.check(expr_array),
                    BuiltinFn::VectorElmMap(source, _) => self.check(source),
                    BuiltinFn::VectorSortOrder(_, _) => Ok(Units::Constant),
                    BuiltinFn::AllocateAvailable(req, _, _)
                    | BuiltinFn::AllocateByPriority(req, _, _, _, _) => self.check(req),
                    // Previous(x, init) preserves the units of x and requires
                    // the fallback to be compatible with it. A literal in
                    // either position is unit-polymorphic.
                    BuiltinFn::Previous(a, b) => {
                        let units = self.check(a)?;
                        let fallback_units = self.check(b)?;
                        if !fallback_units.equals(&units) {
                            return Err(ConsistencyError(
                                ErrorCode::UnitMismatch,
                                b.get_loc(),
                                Some(format!(
                                    "PREVIOUS fallback has units '{}' but expected '{}'",
                                    fallback_units.to_unit_map(),
                                    units.to_unit_map()
                                )),
                            ));
                        }
                        Ok(units.first_explicit(fallback_units))
                    }
                    BuiltinFn::Init(a) => self.check(a),
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
                    // `x^n` unit semantics live in `units::power_units`
                    // (shared with `units_infer`): integer-literal exponents
                    // multiply the unit exponents, half-integer literals root
                    // the doubled map, and every other exponent shape --
                    // non-half-integer literals AND non-literal expressions --
                    // degrades to unit-polymorphic rather than (wrongly)
                    // keeping x's units. What Vensim does for those shapes is
                    // unverified, so we deliberately do not warn there.
                    BinaryOp::Exp => match (&lunits, literal_exponent(r)) {
                        (Units::Constant, _) => Ok(Units::Constant),
                        (Units::Explicit(base), Some(n)) => {
                            match crate::units::power_units(base, n) {
                                crate::units::PowerUnits::Explicit(units) => {
                                    Ok(Units::Explicit(units))
                                }
                                crate::units::PowerUnits::NonSquareRoot => Err(ConsistencyError(
                                    ErrorCode::UnitMismatch,
                                    expr.get_loc(),
                                    Some(format!(
                                        "raising units '{base}' to the power {n} requires \
                                             them to be a perfect square"
                                    )),
                                )),
                                crate::units::PowerUnits::Polymorphic => Ok(Units::Constant),
                            }
                        }
                        (Units::Explicit(_), None) => Ok(Units::Constant),
                    },
                    BinaryOp::Mod => Ok(lunits),
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

                // A literal branch (`IF c THEN 0 ELSE flow`) is
                // unit-polymorphic: the result has the other branch's units.
                Ok(lunits.first_explicit(runits))
            }
        }
    }
}

/// The model's time unit as a `UnitMap` (`t`).
///
/// Resolves `sim_specs.time_units` (default `"time"`) through the unit
/// context so an aliased time unit (e.g. `yr`/`year`) normalizes the same way
/// a declared unit does; an undeclared time unit becomes a fresh base unit of
/// that name. Shared by `check` (for the stock/flow `S/t` relationship) and by
/// conveyor unit checking (docs/design/conveyors.md §9.8), which needs `t`,
/// `S/t`, and `1/t` to check a conveyor block's parameters.
pub fn model_time_units(ctx: &Context) -> UnitMap {
    let time_units_name =
        canonicalize(ctx.sim_specs.time_units.as_deref().unwrap_or("time")).into_owned();
    ctx.lookup(&time_units_name)
        .cloned()
        .unwrap_or_else(|| [(time_units_name.clone(), 1)].iter().cloned().collect())
}

/// The synthetic `time` variable the `UnitEvaluator` uses to resolve
/// `time`/`initial_time`/`final_time` references and the TIME/DT/... builtins.
fn time_variable(ctx: &Context) -> Variable {
    Variable::Var {
        ident: Ident::new("time"),
        ast: None,
        init_ast: None,
        eqn: None,
        units: Some(model_time_units(ctx)),
        tables: vec![],
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    }
}

/// Compute the concrete units of a standalone expression evaluated in the
/// context of `model` (its variable references resolve to declared-or-inferred
/// units, exactly as in `check`).
///
/// This exposes the internal `UnitEvaluator` for expressions that are NOT
/// `model.variables` and so are never reached by `check`'s per-variable loop --
/// specifically a conveyor block's parameter expressions (`<len>`,
/// `<capacity>`, `<in_limit>`, leak fractions), which live as datamodel strings
/// on the stock/flow `Compat` (docs/design/conveyors.md §9.8). The caller lowers
/// each string to an `Expr2` in the model's context, then compares the returned
/// `Units` against the unit the block position requires.
///
/// The returned verdict is the raw `UnitEvaluator::check` result: `Explicit(map)`
/// for a determinate unit, `Constant` for a pure literal (compatible with any
/// expected unit), or an error. A `ConsistencyError(DoesNotExist, ..)` means a
/// dependency's units are unknown; callers skip that case, consistent with the
/// rest of unit checking (a reference's units are `declared OR inferred`, and
/// unknown ones are not an error).
pub fn evaluate_expr_units(
    ctx: &Context,
    inferred_units: &HashMap<Ident<Canonical>, UnitMap>,
    model: &ModelStage1,
    expr: &Expr2,
) -> UnitResult<Units> {
    let evaluator = UnitEvaluator {
        ctx,
        model,
        inferred_units,
        time: time_variable(ctx),
    };
    evaluator.check(expr)
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

    let time_units: UnitMap = model_time_units(ctx);
    let one_over_time: UnitMap = combine(UnitOp::Div, Default::default(), time_units.clone());

    let units = UnitEvaluator {
        ctx,
        model,
        inferred_units,
        time: time_variable(ctx),
    };

    // Deterministic emission order (GH #999): `variables` is a HashMap, and
    // its per-instance iteration order used to decide the ORDER of the
    // per-variable consistency errors -- an observable of the diagnostics
    // collection. The GH #595/#633 recipe: sort before iterating.
    let mut sorted_vars: Vec<(&Ident<Canonical>, &Variable)> = model.variables.iter().collect();
    sorted_vars.sort_unstable_by_key(|(id, _)| id.as_str());
    for (ident, var) in sorted_vars {
        if var.table().is_some() {
            // if a variable has a graphical function the equation is fed into
            // that function like `f(eqn)` -- the units are just whatever is
            // specified on the variable (like a constant would be)
            continue;
        }

        // Check that all elements of arrayed expressions have consistent units,
        // even when the array variable has no declared units.
        //
        // The per-element map is a HashMap, and its iteration order used to
        // pick the ANCHOR element ("previous element(s)") -- so a mixed-units
        // variable named DIFFERENT offending elements, and a different NUMBER
        // of rows, run to run (GH #999: two of the ~184 churned corpus lines
        // per run were this shape). Sorting by element key makes the anchor
        // the lexicographically-first element, deterministically.
        if let Some(Ast::Arrayed(_, asts, default_expr, _)) = var.ast() {
            let mut first_units: Option<UnitMap> = None;
            let mut sorted_elements: Vec<_> = asts.iter().collect();
            sorted_elements.sort_unstable_by_key(|(element, _)| element.as_str());
            for (element, expr) in sorted_elements {
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

            if let Some(default_expr) = default_expr {
                match units.check(default_expr) {
                    Ok(Units::Explicit(default_units)) => {
                        if let Some(ref existing) = first_units
                            && *existing != default_units
                        {
                            let loc = default_expr.get_loc();
                            errors.push((
                                ident.clone(),
                                ConsistencyError(
                                    ErrorCode::UnitMismatch,
                                    Loc::new(loc.start.into(), loc.end.into()),
                                    Some(format!(
                                        "array default expression has units '{}' but element(s) have units '{}'",
                                        default_units, existing
                                    )),
                                ),
                            ));
                        }
                    }
                    Ok(Units::Constant) => {}
                    Err(ConsistencyError(ErrorCode::DoesNotExist, _, _)) => {}
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
            // Compare a sub-expression's computed units against the variable's
            // declared (`expected`) units. Returns a mismatch error, or None
            // when the units match, the expression is constant, or a
            // dependency's units are unknown. Unknown units (DoesNotExist) are
            // NOT a dimensional inconsistency -- they arise from module outputs
            // or synthesized helpers that inference left unresolved -- so we
            // skip the check, exactly as the arrayed-element consistency check
            // above does (and as Vensim does: it warns that the dependency
            // lacks units rather than erroring on every reader).
            let check_against_expected = |expr: &Expr2| -> Option<UnitError> {
                match units.check(expr) {
                    Ok(Units::Explicit(actual)) if actual != *expected => {
                        let loc = expr.get_loc();
                        Some(ConsistencyError(
                            ErrorCode::UnitMismatch,
                            Loc::new(loc.start.into(), loc.end.into()),
                            Some(format!(
                                "the equation computes to units '{actual}', but the \
                                 variable's specified units are '{expected}'"
                            )),
                        ))
                    }
                    Ok(_) => None,
                    Err(ConsistencyError(ErrorCode::DoesNotExist, _, _)) => None,
                    Err(err) => Some(err),
                }
            };

            if let Some(ast) = var.ast() {
                match ast {
                    Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => {
                        if let Some(err) = check_against_expected(expr) {
                            errors.push((ident.clone(), err));
                        }
                    }
                    Ast::Arrayed(_, asts, default_expr, _) => {
                        // Sorted (GH #999): the same HashMap-order hazard as
                        // the element-consistency loop above -- the emission
                        // order of per-element declared-units mismatches is
                        // an observable of the diagnostics collection.
                        let mut sorted_elems: Vec<_> = asts.iter().collect();
                        sorted_elems.sort_unstable_by_key(|(element, _)| element.as_str());
                        for (_element, expr) in sorted_elems {
                            if let Some(err) = check_against_expected(expr) {
                                errors.push((ident.clone(), err));
                            }
                        }
                        if let Some(default_expr) = default_expr
                            && let Some(err) = check_against_expected(default_expr)
                        {
                            errors.push((ident.clone(), err));
                        }
                    }
                }
            }
        }
    }

    // An arrayed variable's per-element loops push one error per element, so
    // a mismatch shared by every element repeated identically (C-LEARN's `ph`
    // warned 6x; scirev arrays 50x). Identical (variable, message) rows carry
    // no extra information -- collapse them, preserving first-occurrence
    // order. The key deliberately EXCLUDES the source location: two elements
    // with textually-different equations ('rate' vs 'rate*1') produce the
    // same user-visible message at different offsets, and keying on the
    // Display form (which embeds the Loc) would let those duplicates
    // survive. What the user reads is (variable, code, details); that is
    // what dedups.
    let dedup_key = |err: &UnitError| -> (u32, String) {
        match err {
            ConsistencyError(code, _loc, details) => {
                (*code as u32, details.clone().unwrap_or_default())
            }
            DefinitionError(eq_err, details) => {
                (eq_err.code as u32, details.clone().unwrap_or_default())
            }
            UnitError::InferenceError { code, details, .. } => {
                (*code as u32, details.clone().unwrap_or_default())
            }
        }
    };
    let mut seen: HashSet<(Ident<Canonical>, (u32, String))> = HashSet::new();
    errors.retain(|(ident, err)| seen.insert((ident.clone(), dedup_key(err))));

    // units checking uses the model's equations and variable's
    // unit definitions to calculate the concrete units for each
    // equation.  If these don't match the units as defined, we
    // log an error.
    Ok(Err(errors))
}
