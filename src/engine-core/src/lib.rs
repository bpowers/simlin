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
extern crate float_cmp;

#[macro_use]
mod common;
mod ast;
mod ast_io {
    include!(concat!(env!("OUT_DIR"), "/ast_io.rs"));
}
mod equation {
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/equation.rs"));
}
mod builtin;
mod model;
mod sim;
mod token;
mod variable;
pub mod xmile;
mod stdlib {
    include!(concat!(env!("OUT_DIR"), "/stdlib.rs"));
}

use std::collections::HashMap;
use std::fmt;
use std::io::BufRead;
use std::rc::Rc;

use self::common::Result;

pub use self::sim::Results;
pub use self::sim::Simulation;

pub struct Project {
    pub name: String,
    file: xmile::File,
    pub models: HashMap<String, Rc<model::Model>>,
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
    pub fn from_xmile_reader(reader: &mut dyn BufRead) -> Result<Self> {
        use quick_xml::de;
        let file: xmile::File = match de::from_reader(reader) {
            Ok(file) => file,
            Err(err) => {
                return import_err!(XmlDeserialization, err.to_string());
            }
        };

        use model::Model;

        // first, pull in the models we need from the stdlib
        let mut models_list: Vec<Model> = self::stdlib::MODEL_NAMES
            .iter()
            .map(|name| self::stdlib::get(name).unwrap())
            .map(|x_model| Model::new(&x_model))
            .collect();

        models_list.extend(file.models.iter().map(|m| Model::new(m)));

        let models = models_list
            .into_iter()
            .map(|m| (m.name.to_string(), Rc::new(m)))
            .collect();

        let project = Project {
            name: "test".to_string(),
            file,
            models,
        };

        Ok(project)
    }

    pub fn new_sim(&self, model_name: &str) -> Result<Simulation> {
        if !self.models.contains_key(model_name) {
            return sim_err!(DoesNotExist, model_name.to_string());
        }

        // get reference to model, increasing refcount
        let model = Rc::clone(&self.models[model_name]);

        Simulation::new(self, model)
    }
}

#[test]
fn test_xml_stock_parsing() {
    let input = "<stock name=\"susceptible\">
        <eqn>total_population</eqn>
        <outflow>succumbing</outflow>
        <outflow>succumbing_2</outflow>
        <doc>People who can contract the disease.</doc>
        <units>people</units>
    </stock>";

    let expected = xmile::Stock {
        name: "susceptible".to_string(),
        eqn: Some("total_population".to_string()),
        doc: Some("People who can contract the disease.".to_string()),
        units: Some("people".to_string()),
        inflows: None,
        outflows: Some(vec!["succumbing".to_string(), "succumbing_2".to_string()]),
        non_negative: None,
        dimensions: None,
    };

    use quick_xml::de;
    let stock: xmile::Var = de::from_reader(input.as_bytes()).unwrap();

    if let xmile::Var::Stock(stock) = stock {
        assert_eq!(expected, stock);
    } else {
        assert!(false);
    }
}

#[test]
fn test_sim_specs_parsing() {
    let input = "<sim_specs method=\"Euler\" time_units=\"Time\">
		<start>0</start>
		<stop>100</stop>
		<savestep>1</savestep>
		<dt>0.03125</dt>
	</sim_specs>";

    let expected = xmile::SimSpecs {
        start: 0.0,
        stop: 100.0,
        dt: Some(xmile::Dt {
            value: 0.03125,
            reciprocal: None,
        }),
        save_step: Some(1.0),
        method: Some("Euler".to_string()),
        time_units: Some("Time".to_string()),
    };

    use quick_xml::de;
    let sim_specs: xmile::SimSpecs = de::from_reader(input.as_bytes()).unwrap();

    assert_eq!(expected, sim_specs);
}
