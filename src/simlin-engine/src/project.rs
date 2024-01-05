// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};

use prost::alloc::rc::Rc;

use crate::common::{Error, Ident};
use crate::dimensions::DimensionsContext;
use crate::model::{ModelStage0, ModelStage1, ScopeStage0};
use crate::units::Context;
use crate::{datamodel, model};

#[derive(Clone, PartialEq, Debug)]
pub struct Project {
    pub datamodel: datamodel::Project,
    // these are Rcs so that multiple Modules created by the compiler can
    // reference the same Model instance
    pub models: HashMap<Ident, Rc<model::ModelStage1>>,
    model_order: Vec<Ident>,
    pub errors: Vec<Error>,
}

impl Project {
    pub fn name(&self) -> &str {
        &self.datamodel.name
    }
}

impl From<datamodel::Project> for Project {
    fn from(project_datamodel: datamodel::Project) -> Self {
        Self::base_from(project_datamodel, |models, units_ctx, model| {
            let inferred_units = crate::units_infer::infer(models, units_ctx, model)
                .unwrap_or_else(|_err| {
                    // XXX: for now, ignore inference errors.  They aren't
                    // understandable for anyone but me - we need to thread
                    // location information through at a minimum.

                    // let mut errors = model.errors.take().unwrap_or_default();
                    // errors.push(err);
                    // model.errors = Some(errors);
                    Default::default()
                });
            model.check_units(units_ctx, &inferred_units)
        })
    }
}

impl Project {
    pub(crate) fn base_from<F>(project_datamodel: datamodel::Project, mut model_cb: F) -> Self
    where
        F: FnMut(&HashMap<Ident, &ModelStage1>, &Context, &mut ModelStage1),
    {
        use crate::common::{topo_sort, ErrorCode, ErrorKind};
        use crate::model::enumerate_modules;

        // first, build the unit context.
        // TODO: there is probably a shared/common core of units we should
        //       pull in

        let mut project_errors = vec![];

        let units_ctx =
            Context::new_with_builtins(&project_datamodel.units, &project_datamodel.sim_specs)
                .unwrap_or_else(|errs| {
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
                });

        // next, pull in all the models from the stdlib
        let mut models_list: Vec<ModelStage0> = crate::stdlib::MODEL_NAMES
            .iter()
            .map(|name| crate::stdlib::get(name).unwrap())
            .map(|x_model| {
                ModelStage0::new(&x_model, &project_datamodel.dimensions, &units_ctx, true)
            })
            .collect();

        // extend the list with the models from the project/XMILE file
        models_list.extend(
            project_datamodel
                .models
                .iter()
                .map(|m| ModelStage0::new(m, &project_datamodel.dimensions, &units_ctx, false)),
        );

        let models: HashMap<Ident, ModelStage0> = models_list
            .iter()
            .cloned()
            .map(|m| (m.ident.clone(), m))
            .collect();

        let dims_ctx = DimensionsContext::from(&project_datamodel.dimensions);
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &dims_ctx,
        };

        let mut models_list: Vec<ModelStage1> = models_list
            .into_iter()
            .map(|model| ModelStage1::new(&scope, &model))
            .collect();

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
            let mut models: HashMap<Ident, &ModelStage1> = HashMap::new();
            for model in models_list.iter_mut() {
                let instantiations = module_instantiations
                    .get(&model.name)
                    .unwrap_or(&no_instantiations);
                model.set_dependencies(&models, &project_datamodel.dimensions, instantiations);
                // things like unit inference happen through this callback
                model_cb(&models, &units_ctx, model);
                models.insert(model.name.clone(), model);
            }
        }

        let ordered_models = models_list
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>();

        let models = models_list
            .into_iter()
            .map(|m| (m.name.to_string(), Rc::new(m)))
            .collect();

        Project {
            datamodel: project_datamodel,
            models,
            model_order: ordered_models,
            errors: project_errors,
        }
    }
}
