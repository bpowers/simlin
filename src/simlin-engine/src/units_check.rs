// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::result::Result as StdResult;

use crate::common::{Error, ErrorKind, Result};
use crate::compiler::{Expr, Module, Simulation};
use crate::datamodel::UnitMap;
use crate::project::Project;
use crate::units::Context;
use crate::vm::StepPart;
use crate::ErrorCode;
use std::collections::HashMap;

struct UnitEvaluator<'a> {
    step_part: StepPart,
    ctx: &'a Context,
    // units for module inputs
    module: &'a Module,
    sim: &'a Simulation,
    offsets: HashMap<usize, &'a str>,
}

/// Units is used to distinguish between explicit units (and explicit
/// dimensionless-ness) and dimensionless-ness that comes from computing
/// on constants.
enum Units {
    Explicit(UnitMap),
    Constant,
}

impl<'a> UnitEvaluator<'a> {
    fn get_units(&self, offset: usize) -> Result<UnitMap> {
        let ident = &self.offsets.get(&offset).ok_or_else(|| Error {
            kind: ErrorKind::Model,
            code: ErrorCode::DoesNotExist,
            details: None,
        })?;

        // TODO: we need to get at/pass in the parsed unit definitions off
        //       the Variable at this point.

        // philosophical question: do we use the units that are user defined, or calculated?
        // I think its gotta be user defined.
        // if we are using user-defined units, do we need the stuff Simulation::new does at all?

        Ok(UnitMap::new())
    }

    fn check(&mut self, expr: &Expr) -> Result<Units> {
        // TODO: how do we recognize that a variable that only does
        //       math on constants (and gets a dmnl from this check fn)
        //       can have a unit assigned to it?  I think this fn
        //       needs to not only return a UnitMap, but some enum

        match expr {
            Expr::Const(_, _) => {
                // TODO: we should eventually support dimensioned constants
                // Units::Constant
            }
            Expr::Var(offset, _) => {
                // self.module.
            }
            Expr::Subscript(_, _, _, _) => {}
            Expr::Dt(_) => {}
            Expr::App(_, _) => {}
            Expr::EvalModule(_, _, _) => {}
            Expr::ModuleInput(_, _) => {}
            Expr::Op2(_, _, _, _) => {}
            Expr::Op1(_, _, _) => {}
            Expr::If(_, _, _, _) => {}
            Expr::AssignCurr(_, _) => {}
            Expr::AssignNext(_, _) => {}
        };

        Ok(Units::Explicit(UnitMap::new()))
    }
}

fn check_runlist(
    ctx: &Context,
    step_part: StepPart,
    sim: &Simulation,
    module: &Module,
) -> StdResult<(), Vec<Error>> {
    let runlist = match step_part {
        StepPart::Initials => &module.runlist_initials,
        StepPart::Flows => &module.runlist_flows,
        StepPart::Stocks => &module.runlist_stocks,
    };

    let offsets: HashMap<_, _> = module.offsets[&module.ident]
        .iter()
        .map(|(ident, (off, _))| (*off, ident.as_str()))
        .collect();

    let mut units = UnitEvaluator {
        step_part,
        ctx,
        sim,
        module,
        offsets,
    };

    for expr in runlist.iter() {
        units.check(expr);
    }

    // TODO: pull list of errors off UnitEvaluator
    Err(vec![])
}

#[allow(dead_code)]
// check uses the model's variables' equations and unit definitions to
// calculate the concrete units for each equation.  The outer result
// indicates if we had a problem running the analysis.  The inner result
// returns a list of unit problems, if there was one.
pub fn check(
    project: &Project,
    ctx: Context,
    main_model_name: &str,
) -> Result<StdResult<(), Vec<Error>>> {
    let sim = Simulation::new(project, main_model_name)?;
    let module = sim.modules.get(main_model_name).ok_or_else(|| Error {
        kind: ErrorKind::Model,
        code: ErrorCode::BadModelName,
        details: Some(main_model_name.to_owned()),
    })?;

    let mut errs = vec![];
    match check_runlist(&ctx, StepPart::Initials, &sim, module) {
        Ok(_) => {}
        Err(mut errors) => errs.append(&mut errors),
    };

    // TODO: flows + stocks

    // units checking uses the model's equations and variable's
    // unit definitions to calculate the concrete units for each
    // equation.  If these don't match the units as defined, we
    // log an error.
    Ok(Err(vec![]))
}
