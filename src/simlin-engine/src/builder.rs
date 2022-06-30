// Copyright 2022 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::rc::Rc;

use crate::builtins::Loc;
use crate::common::{ErrorCode, UnitError};
use crate::compiler::Simulation;
use crate::datamodel::{Equation, Project as DatamodelProject};
use crate::project::Project;

pub fn build_sim_with_stderrors(project: &DatamodelProject) -> Option<Simulation> {
    let project_datamodel = project.clone();
    let project = Rc::new(Project::from(project.clone()));
    if !project.errors.is_empty() {
        for err in project.errors.iter() {
            eprintln!("project error: {}", err);
        }
    }

    let mut found_model_error = false;
    for (model_name, model) in project.models.iter() {
        let model_datamodel = project_datamodel.get_model(model_name);
        if model_datamodel.is_none() {
            continue;
        }
        let model_datamodel = model_datamodel.unwrap();
        let mut found_var_error = false;
        for (ident, errors) in model.get_variable_errors() {
            assert!(!errors.is_empty());
            let var = model_datamodel.get_variable(&ident).unwrap();
            found_var_error = true;
            for error in errors {
                eprintln!();
                if let Some(Equation::Scalar(eqn, ..)) = var.get_equation() {
                    eprintln!("    {}", eqn);
                    let space = " ".repeat(error.start as usize);
                    let underline = "~".repeat((error.end - error.start) as usize);
                    eprintln!("    {}{}", space, underline);
                }
                eprintln!(
                    "error in model '{}' variable '{}': {}",
                    model_name, ident, error.code
                );
            }
        }
        for (ident, errors) in model.get_unit_errors() {
            assert!(!errors.is_empty());
            let var = model_datamodel.get_variable(&ident).unwrap();
            for error in errors {
                eprintln!();
                let (eqn, loc, details) = match error {
                    UnitError::DefinitionError(error, details) => {
                        let details = if let Some(details) = details {
                            format!("{} -- {}", error.code, details)
                        } else {
                            format!("{}", error.code)
                        };
                        (
                            var.get_units(),
                            Loc::new(error.start.into(), error.end.into()),
                            details,
                        )
                    }
                    UnitError::ConsistencyError(code, loc, details) => {
                        let (eqn, loc, code) =
                            if let Some(Equation::Scalar(eqn, ..)) = var.get_equation() {
                                (Some(eqn), loc, code)
                            } else {
                                (None, loc, code)
                            };
                        let details = match details {
                            Some(details) => format!("{} -- {}", code, details),
                            None => format!("{}", code),
                        };
                        (eqn, loc, details)
                    }
                };
                if let Some(eqn) = eqn {
                    eprintln!("    {}", eqn);
                    let space = " ".repeat(loc.start as usize);
                    let underline = "~".repeat((loc.end - loc.start) as usize);
                    eprintln!("    {}{}", space, underline);
                }
                eprintln!(
                    "units error in model '{}' variable '{}': {}",
                    model_name, ident, details
                );
            }
        }
        if let Some(errors) = &model.errors {
            for error in errors.iter() {
                if error.code == ErrorCode::VariablesHaveErrors && found_var_error {
                    continue;
                }
                eprintln!("error in model {}: {}", model_name, error);
                found_model_error = true;
            }
        }
    }
    let sim = match Simulation::new(&project, "main") {
        Ok(sim) => sim,
        Err(err) => {
            if !(err.code == ErrorCode::NotSimulatable && found_model_error) {
                eprintln!("error: {}", err);
            }
            return None;
        }
    };

    Some(sim)
}
