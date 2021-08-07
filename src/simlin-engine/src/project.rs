// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};

use prost::alloc::rc::Rc;

use crate::common::Error;
use crate::{datamodel, model};

#[derive(Clone, PartialEq, Debug)]
pub struct Project {
    pub datamodel: datamodel::Project,
    pub models: HashMap<String, Rc<model::Model>>,
    pub errors: Vec<Error>,
}

impl Project {
    pub fn name(&self) -> &str {
        &self.datamodel.name
    }
}

impl From<datamodel::Project> for Project {
    fn from(project_datamodel: datamodel::Project) -> Self {
        use crate::common::{topo_sort, ErrorCode, ErrorKind, Ident};
        use crate::model::{enumerate_modules, Model};
        use crate::units::Context;

        // first, build the unit context.

        let mut project_errors = vec![];

        let units_ctx = match Context::new(&project_datamodel.units) {
            Ok(ctx) => ctx,
            Err(errs) => {
                for (unit_name, unit_errs) in errs {
                    for err in unit_errs {
                        project_errors.push(Error {
                            kind: ErrorKind::Model,
                            code: ErrorCode::UnitDefinitionErrors,
                            details: Some(format!("{}: {}", unit_name, err)),
                        });
                    }
                }
                Default::default()
            }
        };

        let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = HashMap::new();

        // first, pull in the models we need from the stdlib
        let mut models_list: Vec<Model> = crate::stdlib::MODEL_NAMES
            .iter()
            .map(|name| crate::stdlib::get(name).unwrap())
            .map(|x_model| {
                Model::new(
                    &models,
                    &x_model,
                    &project_datamodel.dimensions,
                    &units_ctx,
                    true,
                )
            })
            .collect();

        let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = project_datamodel
            .models
            .iter()
            .map(|m| model::build_xvars_map(m.name.clone(), m))
            .collect();

        models_list.extend(
            project_datamodel
                .models
                .iter()
                .map(|m| Model::new(&models, m, &project_datamodel.dimensions, &units_ctx, false)),
        );

        //             let inputs = v.references.iter().map(|mi| {
        //                 crate::model::resolve_module_input(models, model_name, &ident, &mi.src, &mi.dst)
        //             });
        //             let (inputs, errors): (Vec<_>, Vec<_>) = inputs.partition(EquationResult::is_ok);
        //             let inputs: Vec<ModuleInput> = inputs.into_iter().flat_map(|i| i.unwrap()).collect();
        //             let mut errors: Vec<VariableError> = errors
        //                 .into_iter()
        //                 .map(|e| e.unwrap_err())
        //                 .map(VariableError::EquationError)
        //                 .collect();

        let model_order = {
            let model_deps = models_list
                .iter_mut()
                .map(|model| (model.name.clone(), model.model_deps.take().unwrap()))
                .collect::<HashMap<_, _>>();

            let model_runlist = models_list
                .iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<&str>>();
            let model_runlist = topo_sort(model_runlist, &model_deps);
            model_runlist
                .into_iter()
                .enumerate()
                .map(|(i, n)| (n.to_owned(), i))
                .collect::<HashMap<Ident, usize>>()
        };

        // sort our model list so that the dependency resolution below works
        models_list.sort_unstable_by(|a, b| {
            model_order[a.name.as_str()].cmp(&model_order[b.name.as_str()])
        });

        let module_instantiations = {
            let models = models_list.iter().map(|m| (m.name.as_str(), m)).collect();
            // FIXME: ignoring the result here because if we have errors, it doesn't really matter
            enumerate_modules(&models, "main", |model| model.name.clone()).unwrap_or_default()
        };

        // dependency resolution; we need to do this as a second pass
        // to ensure we have the information available for modules
        {
            let no_instantiations = BTreeSet::new();
            let mut models: HashMap<Ident, &Model> = HashMap::new();
            for model in models_list.iter_mut() {
                let instantiations = module_instantiations
                    .get(&model.name)
                    .unwrap_or(&no_instantiations);
                model.set_dependencies(&models, &project_datamodel.dimensions, instantiations);
                models.insert(model.name.clone(), model);
            }
        }

        let models = models_list
            .into_iter()
            .map(|m| (m.name.to_string(), Rc::new(m)))
            .collect();

        Project {
            datamodel: project_datamodel,
            models,
            errors: project_errors,
        }
    }
}
