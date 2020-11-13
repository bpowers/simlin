// Copyright 2019 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![forbid(unsafe_code)]

#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate float_cmp;

use std::collections::HashMap;
use std::io::BufRead;
use std::rc::Rc;

#[macro_use]
mod common;
mod ast;
mod datamodel;
pub mod project_io {
    include!(concat!(env!("OUT_DIR"), "/project_io.rs"));
}
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

pub use self::common::Result;

pub use self::common::{canonicalize, Ident};
pub use self::sim::Method;
pub use self::sim::Results;
pub use self::sim::Simulation;
pub use self::sim::Specs as SimSpecs;

#[derive(Clone, PartialEq, Debug)]
pub struct Project {
    pub name: String,
    file: xmile::File,
    pub models: HashMap<String, Rc<model::Model>>,
}

impl Project {
    pub fn from_xmile_reader(reader: &mut dyn BufRead) -> Result<Rc<Self>> {
        use quick_xml::de;
        let file: xmile::File = match de::from_reader(reader) {
            Ok(file) => file,
            Err(err) => {
                return import_err!(XmlDeserialization, err.to_string());
            }
        };

        use model::Model;

        let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = HashMap::new();

        // first, pull in the models we need from the stdlib
        let mut models_list: Vec<Model> = self::stdlib::MODEL_NAMES
            .iter()
            .map(|name| self::stdlib::get(name).unwrap())
            .map(|x_model| Model::new(&models, &x_model))
            .collect();

        let file_models: Vec<datamodel::Model> =
            file.models.iter().map(xmile::convert_model).collect();

        let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = file_models
            .iter()
            .map(|m| model::build_xvars_map(m.name.clone(), &m))
            .collect();

        models_list.extend(file_models.iter().map(|m| Model::new(&models, m)));

        let models = models_list
            .into_iter()
            .map(|m| (m.name.to_string(), Rc::new(m)))
            .collect();

        let project = Project {
            name: "test".to_string(),
            file,
            models,
        };

        Ok(Rc::new(project))
    }
}

#[test]
fn test_bad_xml() {
    let input = "<stock name=\"susceptible\">
        <eqn>total_population</eqn>
        <outflow>succumbing</outflow>
        <outflow>succumbing_2";

    use quick_xml::de;
    let stock: std::result::Result<xmile::Var, _> = de::from_reader(input.as_bytes());

    assert!(stock.is_err());
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
fn test_xml_gf_parsing() {
    let input = "            <aux name=\"lookup function table\">
                <eqn>0</eqn>
                <gf>
                    <yscale min=\"-1\" max=\"1\"/>
                    <xpts>0,5,10,15,20,25,30,35,40,45</xpts>
                    <ypts>0,0,1,1,0,0,-1,-1,0,0</ypts>
                </gf>
            </aux>";

    let expected = xmile::Aux {
        name: "lookup function table".to_string(),
        eqn: Some("0".to_string()),
        doc: None,
        units: None,
        gf: Some(xmile::GF {
            name: None,
            kind: None,
            x_scale: None,
            y_scale: Some(xmile::GraphicalFunctionScale {
                min: -1.0,
                max: 1.0,
            }),
            x_pts: Some("0,5,10,15,20,25,30,35,40,45".to_string()),
            y_pts: Some("0,0,1,1,0,0,-1,-1,0,0".to_string()),
        }),
        dimensions: None,
    };

    use quick_xml::de;
    let aux: xmile::Var = de::from_reader(input.as_bytes()).unwrap();

    if let xmile::Var::Aux(aux) = aux {
        assert_eq!(expected, aux);
    } else {
        assert!(false);
    }
}

#[test]
fn test_module_parsing() {
    let input = "<module name=\"hares\" isee:label=\"\">
				<connect to=\"hares.area\" from=\".area\"/>
				<connect2 to=\"hares.area\" from=\"area\"/>
				<connect to=\"lynxes.hare_density\" from=\"hares.hare_density\"/>
				<connect2 to=\"lynxes.hare_density\" from=\"hares.hare_density\"/>
				<connect to=\"hares.lynxes\" from=\"lynxes.lynxes\"/>
				<connect2 to=\"hares.lynxes\" from=\"lynxes.lynxes\"/>
			</module>";

    let expected = xmile::Module {
        name: "hares".to_string(),
        model_name: None,
        doc: None,
        units: None,
        refs: vec![
            xmile::Reference::Connect(xmile::Connect {
                src: ".area".to_string(),
                dst: "hares.area".to_string(),
            }),
            xmile::Reference::Connect2(xmile::Connect {
                src: "area".to_string(),
                dst: "hares.area".to_string(),
            }),
            xmile::Reference::Connect(xmile::Connect {
                src: "hares.hare_density".to_string(),
                dst: "lynxes.hare_density".to_string(),
            }),
            xmile::Reference::Connect2(xmile::Connect {
                src: "hares.hare_density".to_string(),
                dst: "lynxes.hare_density".to_string(),
            }),
            xmile::Reference::Connect(xmile::Connect {
                src: "lynxes.lynxes".to_string(),
                dst: "hares.lynxes".to_string(),
            }),
            xmile::Reference::Connect2(xmile::Connect {
                src: "lynxes.lynxes".to_string(),
                dst: "hares.lynxes".to_string(),
            }),
        ],
    };

    use quick_xml::de;
    let actual: xmile::Module = de::from_reader(input.as_bytes()).unwrap();

    assert_eq!(expected, actual);
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
