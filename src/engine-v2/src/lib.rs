// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::rc::Rc;

use wasm_bindgen::prelude::*;

use system_dynamics_engine as engine;
use system_dynamics_engine::common::Result;
use system_dynamics_engine::{datamodel, project_io, prost, serde, Error};

#[wasm_bindgen]
pub struct Project {
    #[allow(dead_code)]
    project: Rc<engine::Project>,
    sim: Option<engine::Simulation>,
}

impl Project {
    fn instantiate_sim(&mut self) {
        // TODO: expose the simulation error message here
        self.sim = engine::Simulation::new(&self.project, "main").ok();
    }

    pub fn serialize_to_protobuf(&self) -> Vec<u8> {
        use prost::Message;

        let pb_project = serde::serialize(&self.project.datamodel);
        let mut buf = Vec::with_capacity(pb_project.encoded_len());
        pb_project.encode(&mut buf).unwrap();
        buf
    }

    // time control

    fn update_sim_specs(&mut self, specs: datamodel::SimSpecs) -> Option<Error> {
        let mut project = self.project.as_ref().clone();
        project.datamodel.sim_specs = specs;

        self.project = Rc::new(project);
        self.instantiate_sim();

        None
    }

    pub fn set_sim_spec_start(&mut self, value: f64) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.start = value;
        self.update_sim_specs(specs)
    }

    pub fn set_sim_spec_stop(&mut self, value: f64) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.stop = value;
        self.update_sim_specs(specs)
    }

    pub fn set_sim_spec_dt(&mut self, value: f64, is_reciprocal: bool) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.dt = if is_reciprocal {
            datamodel::Dt::Reciprocal(value)
        } else {
            datamodel::Dt::Dt(value)
        };
        self.update_sim_specs(specs)
    }

    pub fn set_sim_spec_savestep(&mut self, value: f64, is_reciprocal: bool) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.save_step = Some(if is_reciprocal {
            datamodel::Dt::Reciprocal(value)
        } else {
            datamodel::Dt::Dt(value)
        });
        self.update_sim_specs(specs)
    }

    pub fn clear_sim_spec_savestep(&mut self) {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.save_step = None;
        self.update_sim_specs(specs).unwrap();
    }

    pub fn set_sim_spec_method(&mut self, method: datamodel::SimMethod) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.sim_method = method;
        self.update_sim_specs(specs)
    }

    pub fn set_sim_spec_time_units(&mut self, value: &str) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.time_units = if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        };
        self.update_sim_specs(specs)
    }

    // general

    pub fn is_simulatable(&self) -> bool {
        self.sim.is_some()
    }

    // model control

    pub fn add_new_variable(
        &mut self,
        _model_name: &str,
        _kind: &str,
        _name: &str,
    ) -> Result<Vec<u8>> {
        Ok(self.serialize_to_protobuf())
    }

    pub fn delete_variables(&mut self, _model_name: &str, _names: Vec<String>) -> Result<Vec<u8>> {
        // TODO: to convert names to idents

        Ok(self.serialize_to_protobuf())
    }

    pub fn add_stocks_flow(
        &mut self,
        _model_name: &str,
        _flow: &str,
        _dir: &str,
    ) -> Result<Vec<u8>> {
        Ok(self.serialize_to_protobuf())
    }

    pub fn remove_stocks_flow(
        &mut self,
        _model_name: &str,
        _flow: &str,
        _dir: &str,
    ) -> Result<Vec<u8>> {
        Ok(self.serialize_to_protobuf())
    }

    pub fn set_equation(
        &mut self,
        model_name: &str,
        ident: &str,
        new_equation: &str,
    ) -> Option<Error> {
        let mut project = self.project.datamodel.clone();

        for m in project.models.iter_mut().filter(|m| m.name == model_name) {
            for v in m.variables.iter_mut().filter(|v| v.get_ident() == ident) {
                v.set_equation(new_equation);
            }
        }

        self.project = Rc::new(project.into());
        self.instantiate_sim();

        None
    }

    pub fn set_graphical_function(
        &mut self,
        _model_name: &str,
        _ident: &str,
        _gf: Option<String>,
    ) -> Result<Vec<u8>> {
        Ok(self.serialize_to_protobuf())
    }

    pub fn rename(
        &mut self,
        _model_name: &str,
        _old_ident: &str,
        _new_ident: &str,
    ) -> Result<Vec<u8>> {
        Ok(self.serialize_to_protobuf())
    }

    // view control

    // selection_delete
    // set_label_position
    // attach_flow
    // link_attach
    // create_variable
    // selection_move
}

#[wasm_bindgen]
pub fn open(project_pb: &[u8]) -> Option<Project> {
    use prost::Message;
    let project = match project_io::Project::decode(project_pb) {
        Ok(project) => serde::deserialize(project),
        Err(err) => panic!("decode failed: {}", err),
    };

    let project = Rc::new(project.into());
    let mut project = Project { project, sim: None };
    project.instantiate_sim();

    Some(project)
}
