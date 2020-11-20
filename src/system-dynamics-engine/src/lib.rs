// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate float_cmp;

use std::collections::HashMap;
use std::rc::Rc;

#[macro_use]
pub mod common;
mod ast;
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
mod model;
mod sim;
mod token;
mod variable;
pub mod xmile;
mod stdlib {
    include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
}

mod interpreter;

pub use self::common::{canonicalize, Error, Ident, Result};
pub use self::sim::Method;
pub use self::sim::Results;
pub use self::sim::Simulation;
pub use self::sim::Specs as SimSpecs;

#[derive(Clone, PartialEq, Debug)]
pub struct Project {
    pub name: String,
    datamodel: datamodel::Project,
    pub models: HashMap<String, Rc<model::Model>>,
}

impl From<datamodel::Project> for Project {
    fn from(project_datamodel: datamodel::Project) -> Self {
        use model::Model;

        let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = HashMap::new();

        // first, pull in the models we need from the stdlib
        let mut models_list: Vec<Model> = self::stdlib::MODEL_NAMES
            .iter()
            .map(|name| self::stdlib::get(name).unwrap())
            .map(|x_model| Model::new(&models, &x_model))
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
                .map(|m| Model::new(&models, m)),
        );

        let models = models_list
            .into_iter()
            .map(|m| (m.name.to_string(), Rc::new(m)))
            .collect();

        Project {
            name: "test".to_string(),
            datamodel: project_datamodel,
            models,
        }
    }
}
