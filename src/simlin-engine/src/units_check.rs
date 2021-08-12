// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::result::Result as StdResult;

use crate::ast::{Ast, Expr};
use crate::common::{Error, ErrorCode, ErrorKind, Ident, Result};
use crate::datamodel::UnitMap;
use crate::model::ModelStage1;
use crate::units::Context;

struct UnitEvaluator<'a> {
    ctx: &'a Context,
    model: &'a ModelStage1,
    // units for module inputs
}

/// Units is used to distinguish between explicit units (and explicit
/// dimensionless-ness) and dimensionless-ness that comes from computing
/// on constants.
enum Units {
    Explicit(UnitMap),
    Constant,
}

impl<'a> UnitEvaluator<'a> {
    fn check(&self, expr: &Expr) -> Result<Units> {
        match expr {
            Expr::Const(_, _, _) => {
                return Ok(Units::Constant);
            }
            Expr::Var(_, _) => {}
            Expr::App(_, _, _) => {}
            Expr::Subscript(_, _, _) => {}
            Expr::Op1(_, _, _) => {}
            Expr::Op2(_, _, _, _) => {}
            Expr::If(_, _, _, _) => {}
        };

        Ok(Units::Explicit(UnitMap::new()))
    }
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

    let units = UnitEvaluator { ctx, model };

    for (ident, var) in model.variables.iter() {
        if let Some(expected) = var.units() {
            if let Some(ast) = var.ast() {
                match ast {
                    Ast::Scalar(expr) => match units.check(expr) {
                        Ok(Units::Explicit(actual)) => {
                            if &actual != expected {
                                errors.push((
                                    ident.clone(),
                                    Error {
                                        kind: ErrorKind::Variable,
                                        code: ErrorCode::UnitMismatch,
                                        details: Some("TODO: pretty print the mismatch".to_owned()),
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
