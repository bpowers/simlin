// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::rc::Rc;

pub use prost;

mod ast;
pub mod common;
pub mod datamodel;
pub mod project_io {
    include!(concat!(env!("OUT_DIR"), "/project_io.rs"));
}
pub mod serde;
mod equation {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/equation.rs"));
}
mod builtins;
mod builtins_visitor;
mod compiler;
mod model;
mod token;
mod variable;
mod stdlib {
    include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
}

mod bytecode;
mod interpreter;
mod units;
mod vm;

pub use self::common::{canonicalize, quoteize, Error, ErrorCode, Ident, Result};
pub use self::compiler::Simulation;
pub use self::variable::Variable;
pub use self::vm::Method;
pub use self::vm::Results;
pub use self::vm::Specs as SimSpecs;
pub use self::vm::VM;
use crate::common::topo_sort;

#[derive(Clone, PartialEq, Debug)]
pub struct Project {
    pub datamodel: datamodel::Project,
    pub models: HashMap<String, Rc<model::Model>>,
}

impl Project {
    pub fn name(&self) -> &str {
        &self.datamodel.name
    }
}

impl From<datamodel::Project> for Project {
    fn from(project_datamodel: datamodel::Project) -> Self {
        use model::Model;

        let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = HashMap::new();

        // first, pull in the models we need from the stdlib
        let mut models_list: Vec<Model> = self::stdlib::MODEL_NAMES
            .iter()
            .map(|name| self::stdlib::get(name).unwrap())
            .map(|x_model| Model::new(&models, &x_model, &project_datamodel.dimensions, true))
            .collect();

        let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = project_datamodel
            .models
            .iter()
            .map(|m| model::build_xvars_map(m.name.clone(), &m))
            .collect();

        models_list.extend(
            project_datamodel
                .models
                .iter()
                .map(|m| Model::new(&models, m, &project_datamodel.dimensions, false)),
        );

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

        // dependency resolution; we need to do this as a second pass
        // to ensure we have the information available for modules
        {
            let mut models: HashMap<Ident, &Model> = HashMap::new();
            for model in models_list.iter_mut() {
                model.set_dependencies(&models, &project_datamodel.dimensions);
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
        }
    }
}
