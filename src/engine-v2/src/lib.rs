// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::rc::Rc;

use wasm_bindgen::prelude::*;

use system_dynamics_engine as engine;
use system_dynamics_engine::common::Result;
use system_dynamics_engine::{project_io, prost, serde};

#[wasm_bindgen]
pub struct Project {
    #[allow(dead_code)]
    project: Rc<engine::Project>,
    sim: Option<engine::Simulation>,
}

#[wasm_bindgen]
pub enum SimMethod {
    Euler,
    RungeKutta4,
}

impl Project {
    pub fn serialize_to_protobuf(&self) -> Vec<u8> {
        use prost::Message;

        let pb_project = serde::serialize(&self.project.datamodel);
        let mut buf = Vec::with_capacity(pb_project.encoded_len());
        pb_project.encode(&mut buf).unwrap();
        buf
    }

    // time control

    pub fn set_sim_spec_start(&mut self, _value: f64) -> Vec<u8> {
        self.serialize_to_protobuf()
    }
    pub fn set_sim_spec_stop(&mut self, _value: f64) -> Vec<u8> {
        self.serialize_to_protobuf()
    }
    pub fn set_sim_spec_dt(&mut self, _value: f64, _is_reciprocal: bool) -> Vec<u8> {
        self.serialize_to_protobuf()
    }
    pub fn set_sim_spec_savestep(&mut self, _value: f64, _is_reciprocal: bool) -> Vec<u8> {
        self.serialize_to_protobuf()
    }
    pub fn clear_sim_spec_savestep(&mut self) -> Vec<u8> {
        self.serialize_to_protobuf()
    }
    pub fn set_sim_spec_method(&mut self, _method: SimMethod) -> Vec<u8> {
        self.serialize_to_protobuf()
    }
    pub fn set_sim_spec_time_units(&mut self, _value: &str) -> Vec<u8> {
        self.serialize_to_protobuf()
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

    pub fn delete_variables(&mut self, _model_name: &str, _names: &[&str]) -> Result<Vec<u8>> {
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
        _model_name: &str,
        _ident: &str,
        _new_equation: &str,
    ) -> Result<Vec<u8>> {
        Ok(self.serialize_to_protobuf())
    }

    pub fn set_graphical_function(
        &mut self,
        _model_name: &str,
        _ident: &str,
        _gf: Option<&str>,
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
pub fn open(project_pb: &[u8]) -> Project {
    use prost::Message;
    let project = match project_io::Project::decode(project_pb) {
        Ok(project) => serde::deserialize(project),
        Err(err) => panic!("decode failed: {}", err),
    };

    let project = Rc::new(engine::Project::from(project));
    // TODO: expose the simulation error message here
    let sim = engine::Simulation::new(&project, "main").ok();

    Project { project, sim }
}
