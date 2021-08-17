// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::result::Result as StdResult;

use crate::ast::{Ast, BinaryOp, Expr};
use crate::builtins::BuiltinFn;
use crate::common::{Error, ErrorCode, ErrorKind, Ident, Result};
use crate::datamodel::UnitMap;
use crate::model::ModelStage1;
use crate::units::{pretty_print_unit, Context};
use crate::variable::Variable;

#[allow(dead_code)]
struct UnitEvaluator<'a> {
    ctx: &'a Context,
    model: &'a ModelStage1,
    // units for module inputs
    time: Variable,
}

/// Units is used to distinguish between explicit units (and explicit
/// dimensionless-ness) and dimensionless-ness that comes from computing
/// on constants.
#[derive(Debug, PartialEq, Eq, Clone)]
enum Units {
    Explicit(UnitMap),
    Constant,
}

impl Units {
    fn equals(&self, rhs: &Units) -> bool {
        match (self, rhs) {
            (Units::Constant, Units::Constant)
            | (Units::Explicit(_), Units::Constant)
            | (Units::Constant, Units::Explicit(_)) => true,
            (Units::Explicit(lhs), Units::Explicit(rhs)) => *lhs == *rhs,
        }
    }
}

impl<'a> UnitEvaluator<'a> {
    fn check(&self, expr: &Expr) -> Result<Units> {
        match expr {
            Expr::Const(_, _, _) => Ok(Units::Constant),
            Expr::Var(ident, _) => {
                let var: &Variable =
                    if ident == "time" || ident == "initial_time" || ident == "final_time" {
                        &self.time
                    } else {
                        self.model.variables.get(ident).ok_or(Error {
                            kind: ErrorKind::Model,
                            code: ErrorCode::DoesNotExist,
                            details: Some(ident.clone()),
                        })?
                    };

                var.units()
                    .ok_or(Error {
                        kind: ErrorKind::Variable,
                        code: ErrorCode::UnitDefinitionErrors,
                        details: None,
                    })
                    .map(|units| Units::Explicit(units.clone()))
            }
            Expr::App(builtin, _) => match builtin {
                BuiltinFn::Inf | BuiltinFn::Pi => Ok(Units::Constant),
                BuiltinFn::Time
                | BuiltinFn::TimeStep
                | BuiltinFn::StartTime
                | BuiltinFn::FinalTime => Ok(Units::Explicit(
                    self.time.units().cloned().unwrap_or_default(),
                )),
                BuiltinFn::IsModuleInput(_) => {
                    // returns a bool, which is unitless
                    Ok(Units::Explicit(UnitMap::new()))
                }
                BuiltinFn::Lookup(ident, _) => {
                    // lookups have the units specified on the table
                    if let Some(var) = self.model.variables.get(ident) {
                        var.units()
                            .ok_or(Error {
                                kind: ErrorKind::Variable,
                                code: ErrorCode::UnitDefinitionErrors,
                                details: None,
                            })
                            .map(|units| Units::Explicit(units.clone()))
                    } else {
                        Err(Error {
                            kind: ErrorKind::Model,
                            code: ErrorCode::DoesNotExist,
                            details: Some(ident.clone()),
                        })
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
                | BuiltinFn::Sin(a)
                | BuiltinFn::Sqrt(a)
                | BuiltinFn::Tan(a) => self.check(a),
                BuiltinFn::Mean(args) => {
                    let args = args
                        .iter()
                        .map(|arg| self.check(arg))
                        .collect::<Result<Vec<_>>>()?;

                    if args.is_empty() {
                        return Ok(Units::Constant);
                    }

                    // find the first non-constant argument
                    let arg0 = args
                        .iter()
                        .filter(|arg| matches!(arg, Units::Explicit(_)))
                        .cloned()
                        .next();
                    match arg0 {
                        Some(arg0) => {
                            if args.iter().all(|arg| arg0.equals(arg)) {
                                Ok(arg0)
                            } else {
                                let expected = match arg0 {
                                    Units::Explicit(units) => units,
                                    Units::Constant => Default::default(),
                                };
                                Err(Error {
                                    kind: ErrorKind::Model,
                                    code: ErrorCode::UnitDefinitionErrors,
                                    details: Some(format!(
                                        "expected all arguments to mean() to have the units '{}'",
                                        pretty_print_unit(&expected),
                                    )),
                                })
                            }
                        }
                        // all args were constants, so we're good
                        None => Ok(Units::Constant),
                    }
                }
                BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
                    let a_units = self.check(a)?;
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
                        Err(Error {
                            kind: ErrorKind::Model,
                            code: ErrorCode::UnitDefinitionErrors,
                            details: Some(format!(
                                "expected left and right argument units to match, but '{}' and '{}' don't",
                                pretty_print_unit(&a_units),
                                pretty_print_unit(&b_units),
                            )),
                        })
                    } else {
                        Ok(a_units)
                    }
                }
                BuiltinFn::Pulse(_, _, _) | BuiltinFn::Ramp(_, _, _) | BuiltinFn::Step(_, _) => {
                    Ok(Units::Constant)
                }
                BuiltinFn::SafeDiv(a, b, c) => {
                    let div = Expr::Op2(
                        BinaryOp::Div,
                        a.clone(),
                        b.clone(),
                        a.get_loc().union(&b.get_loc()),
                    );
                    let units = self.check(&div)?;

                    if let Some(c) = c {
                        let c_units = self.check(c)?;
                        if c_units != units {
                            // TODO: return an error here
                        }
                    }

                    Ok(units)
                }
            },
            Expr::Subscript(_, _, _) => Ok(Units::Explicit(UnitMap::new())),
            Expr::Op1(_, l, _) => self.check(l),
            Expr::Op2(op, l, r, _) => {
                let lunits = self.check(l)?;
                let runits = self.check(r)?;

                match op {
                    BinaryOp::Add | BinaryOp::Sub => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Constant, Units::Explicit(units))
                        | (Units::Explicit(units), Units::Constant) => Ok(Units::Explicit(units)),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            if lunits != runits {
                                let lunits = pretty_print_unit(&lunits);
                                let runits = pretty_print_unit(&runits);
                                eprintln!(
                                    "TODO: error, left ({}) and right ({}) units don't match",
                                    lunits, runits
                                );
                            }
                            Ok(Units::Explicit(lunits))
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
            Expr::If(_, l, r, _) => {
                let lunits = self.check(l)?;
                let runits = self.check(r)?;

                if !lunits.equals(&runits) {
                    eprintln!("TODO: if error, left and right units don't match");
                }

                Ok(lunits)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UnitOp {
    Mul,
    Div,
}

fn combine(op: UnitOp, l: UnitMap, r: UnitMap) -> UnitMap {
    let mut l = l;

    for (unit, power) in r.into_iter() {
        let lhs = l.get(unit.as_str()).copied().unwrap_or_default();
        let result = {
            match op {
                UnitOp::Mul => lhs + power,
                UnitOp::Div => lhs - power,
            }
        };
        if result == 0 {
            l.remove(&unit);
        } else {
            *l.entry(unit).or_default() = result;
        }
    }

    if l.contains_key("dmnl") {
        l.remove("dmnl");
    }

    l
}

#[allow(dead_code)]
// check uses the model's variables' equations and unit definitions to
// calculate the concrete units for each equation.  The outer result
// indicates if we had a problem running the analysis.  The inner result
// returns a list of unit problems, if there was one.
pub fn check(ctx: &Context, model: &ModelStage1) -> Result<StdResult<(), Vec<(Ident, Error)>>> {
    let mut errors = vec![];

    // TODO: modules

    // get the main model
    // iterate over the variables
    // for each variable, evaluate the equation given the unit context
    // if the result doesn't match the expected thing, accumulate an error

    let time_units = ctx
        .sim_specs
        .time_units
        .as_deref()
        .unwrap_or("time")
        .to_owned();

    let units = UnitEvaluator {
        ctx,
        model,
        time: Variable::Var {
            ident: "time".to_string(),
            ast: None,
            eqn: None,
            units: Some([(time_units, 1)].iter().cloned().collect()),
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
        },
    };

    for (ident, var) in model.variables.iter() {
        if var.table().is_some() {
            // if a variable has a graphical function the equation is fed into
            // that function like `f(eqn)` -- the units are just whatever is
            // specified on the variable (like a constant would be)
            continue;
        }
        if let Some(expected) = var.units() {
            if let Some(ast) = var.ast() {
                match ast {
                    Ast::Scalar(expr) => match units.check(expr) {
                        Ok(Units::Explicit(actual)) => {
                            if actual != *expected {
                                let details = format!(
                                    "expected '{}', found units '{}'",
                                    pretty_print_unit(expected),
                                    pretty_print_unit(&actual)
                                );
                                errors.push((
                                    ident.clone(),
                                    Error {
                                        kind: ErrorKind::Variable,
                                        code: ErrorCode::UnitMismatch,
                                        details: Some(details),
                                    },
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
                    Ast::ApplyToAll(_, _) => {}
                    Ast::Arrayed(_, _) => {}
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
