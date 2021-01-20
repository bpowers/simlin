// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::cmp::Ordering;

use wasm_bindgen::prelude::*;

use js_sys::{Array, Map};
use prost::Message;

use system_dynamics_engine as engine;
use system_dynamics_engine::common::{ErrorCode, ErrorKind};
use system_dynamics_engine::datamodel::{GraphicalFunction, Variable};
use system_dynamics_engine::{canonicalize, datamodel, project_io, prost, serde, Error, VM};

#[wasm_bindgen]
pub struct Engine {
    project: engine::Project,
    sim_vm: Result<engine::VM, engine::Error>,
    results: Option<engine::Results>,
}

#[wasm_bindgen]
impl Engine {
    fn instantiate_sim(&mut self) {
        // TODO: expose the simulation error message here
        let compiler = engine::Simulation::new(&self.project, "main");
        self.sim_vm = compiler
            .and_then(|compiler| compiler.compile())
            .and_then(VM::new);
    }

    #[wasm_bindgen(js_name = serializeToProtobuf)]
    pub fn serialize_to_protobuf(&self) -> Vec<u8> {
        use prost::Message;

        let pb_project = serde::serialize(&self.project.datamodel);
        let mut buf = Vec::with_capacity(pb_project.encoded_len());
        pb_project.encode(&mut buf).unwrap();
        buf
    }

    // time control

    fn update_sim_specs(&mut self, specs: datamodel::SimSpecs) -> Option<Error> {
        self.project.datamodel.sim_specs = specs;
        self.instantiate_sim();

        None
    }

    #[wasm_bindgen(js_name = setSimSpecStart)]
    pub fn set_sim_spec_start(&mut self, value: f64) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.start = value;
        self.update_sim_specs(specs)
    }

    #[wasm_bindgen(js_name = setSimSpecStop)]
    pub fn set_sim_spec_stop(&mut self, value: f64) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.stop = value;
        self.update_sim_specs(specs)
    }

    #[wasm_bindgen(js_name = setSimSpecDt)]
    pub fn set_sim_spec_dt(&mut self, value: f64, is_reciprocal: bool) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.dt = if is_reciprocal {
            datamodel::Dt::Reciprocal(value)
        } else {
            datamodel::Dt::Dt(value)
        };
        self.update_sim_specs(specs)
    }

    #[wasm_bindgen(js_name = setSimSpecSavestep)]
    pub fn set_sim_spec_savestep(&mut self, value: f64, is_reciprocal: bool) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.save_step = Some(if is_reciprocal {
            datamodel::Dt::Reciprocal(value)
        } else {
            datamodel::Dt::Dt(value)
        });
        self.update_sim_specs(specs)
    }

    #[wasm_bindgen(js_name = clearSimSpecSavestep)]
    pub fn clear_sim_spec_savestep(&mut self) {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.save_step = None;
        self.update_sim_specs(specs).unwrap();
    }

    #[wasm_bindgen(js_name = setSimSpecMethod)]
    pub fn set_sim_spec_method(&mut self, method: datamodel::SimMethod) -> Option<Error> {
        let mut specs = self.project.datamodel.sim_specs.clone();
        specs.sim_method = method;
        self.update_sim_specs(specs)
    }

    #[wasm_bindgen(js_name = setSimSpecTimeUnits)]
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

    #[wasm_bindgen(js_name = isSimulatable)]
    pub fn is_simulatable(&self) -> bool {
        self.sim_vm.is_ok()
    }

    #[wasm_bindgen(js_name = getModelVariableErrors, typescript_type = "Map<string, EquationError>")]
    pub fn get_model_variable_errors(&self, model_name: &str) -> JsValue {
        let model = &self.project.models.get(model_name).unwrap();

        let mut result = Map::new();
        for (ident, errors) in model.get_variable_errors() {
            let js_errors: Array = errors.into_iter().map(JsValue::from).collect();
            result = result.set(&JsValue::from(ident.as_str()), &js_errors)
        }

        result.into()
    }

    #[wasm_bindgen(js_name = getModelErrors, typescript_type = "Array<Error> | undefined")]
    pub fn get_model_errors(&self, model_name: &str) -> Array {
        let model = &self.project.models.get(model_name).unwrap();

        if model.errors.is_none() {
            return Array::new();
        }
        model
            .errors
            .as_ref()
            .unwrap()
            .iter()
            .cloned()
            .map(JsValue::from)
            .collect()
    }

    #[wasm_bindgen(js_name = getSimError)]
    pub fn get_sim_error(&self) -> Option<Error> {
        self.sim_vm.as_ref().err().cloned()
    }

    // model control

    #[wasm_bindgen(js_name = addNewVariable)]
    pub fn add_new_variable(&mut self, model_name: &str, kind: &str, name: &str) -> Option<Error> {
        let mut project = self.project.datamodel.clone();
        let model = project.get_model_mut(model_name).unwrap();
        let ident = canonicalize(name);
        if model.get_variable_mut(&ident).is_some() {
            return Some(Error::new(
                ErrorKind::Model,
                ErrorCode::DuplicateVariable,
                None,
            ));
        }

        let var = match kind {
            "aux" => datamodel::Variable::Aux(datamodel::Aux {
                ident,
                equation: datamodel::Equation::Scalar("".to_owned()),
                documentation: "".to_string(),
                units: None,
                gf: None,
            }),
            "flow" => datamodel::Variable::Flow(datamodel::Flow {
                ident,
                equation: datamodel::Equation::Scalar("".to_owned()),
                documentation: "".to_string(),
                units: None,
                gf: None,
                non_negative: false,
            }),
            "stock" => datamodel::Variable::Stock(datamodel::Stock {
                ident,
                equation: datamodel::Equation::Scalar("".to_owned()),
                documentation: "".to_string(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                non_negative: false,
            }),
            _ => return None,
        };

        model.variables.push(var);

        self.project = project.into();
        self.instantiate_sim();

        None
    }

    #[wasm_bindgen(js_name = deleteVariable)]
    pub fn delete_variable(&mut self, model_name: &str, ident: &str) -> Option<Error> {
        let mut project = self.project.datamodel.clone();
        let model = project.get_model_mut(model_name).unwrap();

        // this is O(n), but variables is usually pretty short
        // if the UI borks up, it may try to remove a variable we don't know about.
        // try not to die if that happens.
        let off = model
            .variables
            .iter()
            .position(|v| v.get_ident() == ident)?;
        let removed = model.variables.remove(off);
        if let Variable::Flow(flow) = removed {
            for var in model.variables.iter_mut() {
                if let Variable::Stock(stock) = var {
                    if let Some(off) = stock.inflows.iter().position(|ident| ident == &flow.ident) {
                        let _ = stock.inflows.remove(off);
                    }
                    if let Some(off) = stock.outflows.iter().position(|ident| ident == &flow.ident)
                    {
                        let _ = stock.outflows.remove(off);
                    }
                }
            }
        }

        self.project = project.into();
        self.instantiate_sim();

        None
    }

    #[wasm_bindgen(js_name = addStocksFlow)]
    pub fn add_stocks_flow(
        &mut self,
        model_name: &str,
        stock: &str,
        flow: &str,
        dir: &str,
    ) -> Option<Error> {
        let mut project = self.project.datamodel.clone();
        let model = project.get_model_mut(model_name).unwrap();
        match model.get_variable_mut(stock) {
            Some(datamodel::Variable::Stock(stock)) => match dir {
                "in" => {
                    stock.inflows.push(flow.to_owned());
                    stock.inflows.sort_unstable();
                }
                "out" => {
                    stock.outflows.push(flow.to_owned());
                    stock.outflows.sort_unstable();
                }
                _ => {
                    return None;
                }
            },
            _ => {
                return None;
            }
        }

        self.project = project.into();
        self.instantiate_sim();

        None
    }

    #[wasm_bindgen(js_name = removeStocksFlow)]
    pub fn remove_stocks_flow(
        &mut self,
        model_name: &str,
        stock: &str,
        flow: &str,
        dir: &str,
    ) -> Option<Error> {
        let mut project = self.project.datamodel.clone();
        let model = project.get_model_mut(model_name).unwrap();
        match model.get_variable_mut(stock) {
            Some(datamodel::Variable::Stock(stock)) => match dir {
                "in" => {
                    let off = stock.inflows.iter().position(|f| f == flow).unwrap();
                    stock.inflows.remove(off);
                }
                "out" => {
                    let off = stock.outflows.iter().position(|f| f == flow).unwrap();
                    stock.outflows.remove(off);
                }
                _ => {
                    return None;
                }
            },
            _ => {
                return None;
            }
        }

        self.project = project.into();
        self.instantiate_sim();

        None
    }

    #[wasm_bindgen(js_name = setEquation)]
    pub fn set_equation(
        &mut self,
        model_name: &str,
        ident: &str,
        new_equation: &str,
    ) -> Option<Error> {
        let mut project = self.project.datamodel.clone();
        let model = project.get_model_mut(model_name).unwrap();
        match model.get_variable_mut(ident) {
            Some(var) => var.set_scalar_equation(new_equation),
            _ => {
                return None;
            }
        }

        self.project = project.into();
        self.instantiate_sim();

        None
    }

    fn set_graphical_function_common(
        &mut self,
        model_name: &str,
        ident: &str,
        gf: Option<GraphicalFunction>,
    ) -> Option<Error> {
        let mut project = self.project.datamodel.clone();
        let model = project.get_model_mut(model_name).unwrap();
        match model.get_variable_mut(ident) {
            Some(var) => var.set_graphical_function(gf),
            _ => {
                return None;
            }
        }

        self.project = project.into();
        self.instantiate_sim();

        None
    }

    #[wasm_bindgen(js_name = setGraphicalFunction)]
    pub fn set_graphical_function(
        &mut self,
        model_name: &str,
        ident: &str,
        graphical_function_pb: &[u8],
    ) -> Option<Error> {
        let gf = match project_io::GraphicalFunction::decode(graphical_function_pb) {
            Ok(gf) => serde::deserialize_graphical_function(gf),
            Err(_err) => {
                return None;
            }
        };

        self.set_graphical_function_common(model_name, ident, Some(gf))
    }

    #[wasm_bindgen(js_name = removeGraphicalFunction)]
    pub fn remove_graphical_function(&mut self, model_name: &str, ident: &str) -> Option<Error> {
        self.set_graphical_function_common(model_name, ident, None)
    }

    pub fn rename(&mut self, model_name: &str, old_name: &str, new_name: &str) -> Option<Error> {
        let old_ident = canonicalize(old_name);
        let new_ident = canonicalize(new_name);

        // the rename might result in the same identifier -- which isn't a
        // problem or error.
        if old_ident == new_ident {
            return None;
        }

        let mut project = self.project.datamodel.clone();
        let model = project.get_model_mut(model_name).unwrap();

        // if there is already a variable by that name, its an error
        if model.get_variable_mut(&new_ident).is_some() {
            return Some(Error::new(
                ErrorKind::Model,
                ErrorCode::DuplicateVariable,
                None,
            ));
        }

        let is_flow;
        match model.get_variable_mut(&old_ident) {
            Some(var) => {
                is_flow = matches!(var, Variable::Flow(_));
                var.set_ident(new_ident.clone());
            }
            _ => {
                return None;
            }
        }

        if is_flow {
            for var in model.variables.iter_mut() {
                if let Variable::Stock(stock) = var {
                    for inflow in stock.inflows.iter_mut() {
                        if inflow == &old_ident {
                            *inflow = new_ident.clone();
                        }
                    }
                    for outflow in stock.outflows.iter_mut() {
                        if outflow == &old_ident {
                            *outflow = new_ident.clone();
                        }
                    }
                }
            }
        }

        // TODO: update ASTs of variables that referred to the old equation

        self.project = project.into();
        self.instantiate_sim();

        None
    }

    // view control

    #[wasm_bindgen(js_name = setView)]
    pub fn set_view(&mut self, model_name: &str, view_off: usize, view_pb: &[u8]) -> Option<Error> {
        let project = &mut self.project.datamodel;

        let view = match project_io::View::decode(view_pb) {
            Ok(view) => serde::deserialize_view(view),
            Err(err) => {
                return Some(Error::new(
                    ErrorKind::Model,
                    ErrorCode::ProtobufDecode,
                    Some(format!("{}", err)),
                ));
            }
        };

        project
            .models
            .iter_mut()
            .filter(|m| m.name == model_name)
            .for_each(|model| match view_off.cmp(&model.views.len()) {
                Ordering::Less => model.views[view_off] = view.clone(),
                Ordering::Equal => model.views.push(view.clone()),
                Ordering::Greater => {}
            });

        self.instantiate_sim();

        None
    }

    // simulation control

    #[wasm_bindgen(js_name = simRunToEnd)]
    pub fn sim_run_to_end(&mut self) {
        if self.sim_vm.is_err() {
            return;
        }
        let vm = self.sim_vm.as_ref().unwrap();
        let results = vm.run_to_end();
        self.results = results.ok();
    }

    #[wasm_bindgen(js_name = simVarNames, typescript_type = "Array<string>")]
    pub fn sim_var_names(&self) -> Array {
        if self.results.is_none() {
            let empty: Vec<String> = vec![];
            return empty.into_iter().map(JsValue::from).collect();
        }
        let results = self.results.as_ref().unwrap();
        results
            .offsets
            .keys()
            .into_iter()
            .map(JsValue::from)
            .collect()
    }

    #[wasm_bindgen(js_name = simSeries)]
    pub fn sim_series(&self, ident: &str) -> Vec<f64> {
        if self.results.is_none() {
            return vec![];
        }
        let results = self.results.as_ref().unwrap();
        if !results.offsets.contains_key(ident) {
            return vec![];
        }

        let off = results.offsets[ident];
        results.iter().map(|curr| curr[off]).collect()
    }

    #[wasm_bindgen(js_name = simClose)]
    pub fn sim_close(&mut self) {
        self.results = None
    }
}

#[wasm_bindgen]
pub fn open(project_pb: &[u8]) -> Option<Engine> {
    let project = match project_io::Project::decode(project_pb) {
        Ok(project) => serde::deserialize(project),
        Err(_err) => {
            return None;
        }
    };

    let mut project = Engine {
        project: project.into(),
        sim_vm: Err(Error::new(
            ErrorKind::Simulation,
            ErrorCode::DoesNotExist,
            None,
        )),
        results: None,
    };
    project.instantiate_sim();

    Some(project)
}
