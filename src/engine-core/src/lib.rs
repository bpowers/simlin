// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![allow(dead_code)]

extern crate core;
#[macro_use]
extern crate lazy_static;
extern crate prost;
extern crate quick_xml;
extern crate regex;
extern crate serde;
extern crate unicode_xid;

#[macro_use]
mod common;
mod ast {
    include!(concat!(env!("OUT_DIR"), "/sd.ast.rs"));
}

mod model;

// mod eqn;
mod sim;
mod tok;
pub mod xmile;

use std::collections::HashMap;
use std::fmt;
use std::io::BufRead;
use std::rc::Rc;

use self::common::Result;

pub use self::sim::Simulation;
use common::SDError;

pub struct Project {
    name: String,
    file: xmile::File,
    models: HashMap<String, Rc<xmile::Model>>,
}

impl fmt::Debug for Project {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Project{{")?;
        writeln!(f, "  name: {}", self.name)?;
        writeln!(f, "  files: {{")?;
        for (name, model) in &self.models {
            writeln!(f, "    {:?}: {:?}", name, model)?;
        }
        writeln!(f, "  }}")?;
        write!(f, "}}")
    }
}

impl Project {
    pub fn from_xmile_reader(reader: &mut dyn BufRead) -> Result<Project> {
        use quick_xml::de;
        let file: xmile::File = match de::from_reader(reader) {
            Ok(file) => file,
            Err(err) => {
                return Err(SDError::new(err.to_string()));
            }
        };

        // writeln!(&mut ::std::io::stderr(), "{:?}\n", file).unwrap();

        //        se::to_writer(::std::io::stderr(), &file).unwrap();
        //        ::std::io::stderr().flush().unwrap();

        let models = HashMap::new();

        // for model in f.get_models() {
        //   models.insert(model.get_name().clone(), model.clone());
        // }

        let project = Project {
            name: "test".to_string(),
            file,
            models,
        };

        Ok(project)
    }

    pub fn new_sim(&self, model_name: &str) -> Result<Simulation> {
        if !self.models.contains_key(model_name) {
            return err!("unknown model");
        }

        // get reference to model, increasing refcount
        let model: Rc<xmile::Model> = self.models.get(model_name).unwrap().clone();

        Simulation::new(self, model)
    }
}
