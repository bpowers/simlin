// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Common test infrastructure for building test projects
//!
//! This module provides a builder-based API for creating test projects
//! that can be used by various test modules.

#[cfg(test)]
use crate::common::ErrorCode;
#[cfg(test)]
use crate::datamodel::{self, Dimension, Equation, Project, SimSpecs, Variable};
#[cfg(test)]
use crate::interpreter::Simulation;
#[cfg(test)]
use crate::project::Project as CompiledProject;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::rc::Rc;

/// Builder for creating test projects with support for arrays, units, and all variable types
#[cfg(test)]
pub struct TestProject {
    pub name: String,
    pub dimensions: Vec<Dimension>,
    pub variables: Vec<Variable>,
    pub units: Vec<datamodel::Unit>,
    pub sim_specs: SimSpecs,
}

#[cfg(test)]
impl TestProject {
    /// Create a new test project builder with default settings
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            dimensions: Vec::new(),
            variables: Vec::new(),
            units: Vec::new(),
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 1.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: Some(datamodel::Dt::Dt(1.0)),
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("Month".to_string()),
            },
        }
    }

    /// Create a new test project builder with custom sim specs
    #[allow(dead_code)]
    pub fn new_with_specs(name: &str, sim_specs: SimSpecs) -> Self {
        Self {
            name: name.to_string(),
            dimensions: Vec::new(),
            variables: Vec::new(),
            units: Vec::new(),
            sim_specs,
        }
    }

    /// Set time units for the simulation
    #[allow(dead_code)]
    pub fn with_time_units(mut self, units: &str) -> Self {
        self.sim_specs.time_units = Some(units.to_string());
        self
    }

    /// Set simulation time parameters
    #[allow(dead_code)]
    pub fn with_sim_time(mut self, start: f64, stop: f64, dt: f64) -> Self {
        self.sim_specs.start = start;
        self.sim_specs.stop = stop;
        self.sim_specs.dt = datamodel::Dt::Dt(dt);
        self
    }

    /// Add a custom unit definition
    pub fn unit(mut self, name: &str, equation: Option<&str>) -> Self {
        self.units.push(datamodel::Unit {
            name: name.to_string(),
            equation: equation.map(|s| s.to_string()),
            disabled: false,
            aliases: vec![],
        });
        self
    }

    /// Add an indexed dimension (e.g., for numeric indices)
    pub fn indexed_dimension(mut self, name: &str, size: u32) -> Self {
        self.dimensions
            .push(Dimension::Indexed(name.to_string(), size));
        self
    }

    /// Add a named dimension with specific elements
    pub fn named_dimension(mut self, name: &str, elements: &[&str]) -> Self {
        self.dimensions.push(Dimension::Named(
            name.to_string(),
            elements.iter().map(|s| s.to_string()).collect(),
        ));
        self
    }

    /// Add an auxiliary variable
    pub fn aux(mut self, name: &str, equation: &str, units: Option<&str>) -> Self {
        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: Equation::Scalar(equation.to_string(), None),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    /// Add a flow variable
    pub fn flow(mut self, name: &str, equation: &str, units: Option<&str>) -> Self {
        self.variables.push(Variable::Flow(datamodel::Flow {
            ident: name.to_string(),
            equation: Equation::Scalar(equation.to_string(), None),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    /// Add a stock variable
    pub fn stock(
        mut self,
        name: &str,
        initial: &str,
        inflows: &[&str],
        outflows: &[&str],
        units: Option<&str>,
    ) -> Self {
        self.variables.push(Variable::Stock(datamodel::Stock {
            ident: name.to_string(),
            equation: Equation::Scalar(initial.to_string(), None),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            inflows: inflows.iter().map(|s| s.to_string()).collect(),
            outflows: outflows.iter().map(|s| s.to_string()).collect(),
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    // Array-specific convenience methods

    /// Add a scalar constant (convenience for aux with constant value)
    pub fn scalar_const(self, name: &str, value: f64) -> Self {
        self.aux(name, &value.to_string(), None)
    }

    /// Add a scalar auxiliary variable (convenience for aux without units)
    pub fn scalar_aux(self, name: &str, equation: &str) -> Self {
        self.aux(name, equation, None)
    }

    /// Add an array constant using "name[dims]" notation
    pub fn array_const(self, name_with_dims: &str, value: f64) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        self.array_aux_direct(&name, dims, &value.to_string(), None)
    }

    /// Add an array auxiliary using "name[dims]" notation
    pub fn array_aux(self, name_with_dims: &str, equation: &str) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        self.array_aux_direct(&name, dims, equation, None)
    }

    /// Add an array with different equations for different subscript ranges using "name[dims]" notation
    pub fn array_with_ranges(
        self,
        name_with_dims: &str,
        equations: Vec<(&str, &str)>, // (element_name, equation)
    ) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        self.array_with_ranges_direct(&name, dims, equations, None)
    }

    // Unit-specific convenience methods

    /// Add an auxiliary variable with units (convenience)
    pub fn aux_with_units(self, name: &str, equation: &str, units: Option<&str>) -> Self {
        self.aux(name, equation, units)
    }

    /// Add a flow variable with units (convenience)
    pub fn flow_with_units(self, name: &str, equation: &str, units: Option<&str>) -> Self {
        self.flow(name, equation, units)
    }

    /// Add a stock variable with units (convenience)
    pub fn stock_with_units(
        self,
        name: &str,
        initial: &str,
        inflows: &[&str],
        outflows: &[&str],
        units: Option<&str>,
    ) -> Self {
        self.stock(name, initial, inflows, outflows, units)
    }

    /// Add an array auxiliary variable with apply-to-all equation
    pub fn array_aux_direct(
        mut self,
        name: &str,
        dims: Vec<String>,
        equation: &str,
        units: Option<&str>,
    ) -> Self {
        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: Equation::ApplyToAll(dims, equation.to_string(), None),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    /// Add an array variable with different equations for different subscript ranges
    pub fn array_with_ranges_direct(
        mut self,
        name: &str,
        dims: Vec<String>,
        equations: Vec<(&str, &str)>, // (element_name, equation)
        units: Option<&str>,
    ) -> Self {
        let arrayed_equations = equations
            .into_iter()
            .map(|(elem, eq)| (elem.to_string(), eq.to_string(), None))
            .collect();

        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: Equation::Arrayed(dims, arrayed_equations),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    /// Build the datamodel Project
    pub fn build_datamodel(&self) -> Project {
        Project {
            name: self.name.clone(),
            sim_specs: self.sim_specs.clone(),
            dimensions: self.dimensions.clone(),
            units: self.units.clone(),
            models: vec![datamodel::Model {
                name: "main".to_string(),
                variables: self.variables.clone(),
                views: vec![],
            }],
            source: Default::default(),
            ai_information: None,
        }
    }

    /// Build and compile the project
    pub fn compile(&self) -> Result<CompiledProject, Vec<(String, ErrorCode)>> {
        let datamodel = self.build_datamodel();
        let compiled = Rc::new(CompiledProject::from(datamodel));

        let mut errors = Vec::new();

        // Check project-level errors
        if !compiled.errors.is_empty() {
            for err in &compiled.errors {
                errors.push(("project".to_string(), err.code));
            }
        }

        // Check model-level errors
        for (model_name, model) in &compiled.models {
            if let Some(model_errors) = &model.errors {
                for err in model_errors {
                    errors.push((model_name.clone(), err.code));
                }
            }

            // Check variable-level errors (including unit errors)
            for (var_name, var_errors) in model.get_variable_errors() {
                for err in var_errors {
                    errors.push((format!("{}.{}", model_name, var_name), err.code));
                }
            }
        }

        if errors.is_empty() {
            Ok(Rc::try_unwrap(compiled).unwrap_or_else(|rc| (*rc).clone()))
        } else {
            Err(errors)
        }
    }

    /// Build a Simulation (requires successful compilation)
    pub fn build_sim(&self) -> Result<Simulation, String> {
        let datamodel = self.build_datamodel();
        let compiled = Rc::new(CompiledProject::from(datamodel));

        // Check for compilation errors first
        let mut has_errors = false;
        if !compiled.errors.is_empty() {
            has_errors = true;
        }

        for (_model_name, model) in &compiled.models {
            if model.errors.is_some() || !model.get_variable_errors().is_empty() {
                has_errors = true;
                break;
            }
        }

        if has_errors {
            return Err("Project has compilation errors".to_string());
        }

        Simulation::new(&compiled, "main")
            .map_err(|e| format!("Failed to create simulation: {:?}", e))
    }

    /// Run the interpreter and get results
    pub fn run_interpreter(&self) -> Result<HashMap<String, Vec<f64>>, String> {
        let sim = self.build_sim()?;

        // Run the simulation using the tree-walking interpreter
        let results = sim
            .run_to_end()
            .map_err(|e| format!("Simulation failed: {:?}", e))?;

        // Extract results
        let mut output = HashMap::new();

        // First collect all individual array elements
        for (name, &offset) in &results.offsets {
            let mut values = Vec::new();
            for step in 0..results.step_count {
                let idx = step * results.step_size + offset;
                values.push(results.data[idx]);
            }
            output.insert(name.clone(), values);
        }

        // Now collect array variables by their base name
        // Array elements are stored as "varname[subscript]", we want to collect them as "varname"
        // We need to preserve the original offset order, not sort alphabetically
        let mut array_results: HashMap<String, Vec<(usize, String, Vec<f64>)>> = HashMap::new();
        for (name, values) in &output {
            if let Some(bracket_pos) = name.find('[') {
                let base_name = &name[..bracket_pos];
                // Get the offset for this element to maintain proper ordering
                let offset = results.offsets.get(name).copied().unwrap_or(usize::MAX);
                let entry = array_results.entry(base_name.to_string()).or_default();
                entry.push((offset, name.clone(), values.clone()));
            }
        }

        // Sort array elements by their offset (not alphabetically!) and flatten into single vector
        for (base_name, mut elements) in array_results {
            // Sort by offset to ensure correct ordering (not alphabetical)
            elements.sort_by_key(|e| e.0);

            // For simplicity, we'll just concatenate all values at each timestep
            // This assumes all elements have the same number of timesteps
            if !elements.is_empty() {
                let n_steps = elements[0].2.len();
                let mut combined = Vec::new();

                // Since we're testing array values, we only want the values at the final timestep
                // (arrays don't change over time in our test cases)
                // Get the last timestep values
                let last_step = n_steps - 1;
                for (_offset, _name, values) in &elements {
                    if last_step < values.len() {
                        combined.push(values[last_step]);
                    }
                }

                // Store with base name (without brackets)
                output.insert(base_name, combined);
            }
        }

        Ok(output)
    }

    /// Test that compilation succeeds
    pub fn assert_compiles(&self) {
        match self.compile() {
            Ok(_) => {}
            Err(errors) => {
                let error_msg = errors
                    .iter()
                    .map(|(loc, code)| format!("{}: {:?}", loc, code))
                    .collect::<Vec<_>>()
                    .join(", ");
                panic!("Compilation failed with errors: {}", error_msg);
            }
        }
    }

    /// Test that compilation fails with specific error
    pub fn assert_compile_error(&self, expected_error: ErrorCode) {
        self.assert_compile_error_impl(expected_error)
    }

    /// Test that compilation fails with unit mismatch (convenience)
    pub fn assert_unit_error(&self) {
        self.assert_compile_error(ErrorCode::UnitMismatch)
    }

    fn assert_compile_error_impl(&self, expected_error: ErrorCode) {
        match self.compile() {
            Ok(_) => panic!(
                "Expected compilation to fail with {:?}, but it succeeded",
                expected_error
            ),
            Err(errors) => {
                let has_expected = errors.iter().any(|(_, code)| *code == expected_error);
                if !has_expected {
                    let error_msg = errors
                        .iter()
                        .map(|(loc, code)| format!("{}: {:?}", loc, code))
                        .collect::<Vec<_>>()
                        .join(", ");
                    panic!(
                        "Expected error {:?}, but got: {}",
                        expected_error, error_msg
                    );
                }
            }
        }
    }

    /// Test that interpreter evaluation succeeds and returns expected values for a scalar variable
    /// (checks only the final timestep value)
    pub fn assert_scalar_result(&self, var_name: &str, expected: f64) {
        let results = self
            .run_interpreter()
            .expect("Interpreter should run successfully");

        let actual = results
            .get(var_name)
            .unwrap_or_else(|| panic!("Variable {} not found in results", var_name));

        let final_value = actual
            .last()
            .copied()
            .unwrap_or_else(|| panic!("Variable {} has no values", var_name));

        assert!(
            (final_value - expected).abs() < 1e-6,
            "Value mismatch for {}: expected {}, got {}",
            var_name,
            expected,
            final_value
        );
    }

    /// Test that interpreter evaluation succeeds and returns expected values
    pub fn assert_interpreter_result(&self, var_name: &str, expected: &[f64]) {
        let results = self
            .run_interpreter()
            .expect("Interpreter should run successfully");

        let actual = results
            .get(var_name)
            .unwrap_or_else(|| panic!("Variable {} not found in results", var_name));

        assert_eq!(
            actual.len(),
            expected.len(),
            "Result length mismatch for {}: expected {}, got {}",
            var_name,
            expected.len(),
            actual.len()
        );

        for (i, (actual_val, expected_val)) in actual.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual_val - expected_val).abs() < 1e-6,
                "Value mismatch for {} at index {}: expected {}, got {}",
                var_name,
                i,
                expected_val,
                actual_val
            );
        }
    }

    /// Test that simulation creation succeeds
    pub fn assert_sim_builds(&self) {
        self.build_sim()
            .expect("Simulation should build successfully");
    }
}

/// Helper to parse array declarations like "name[dim1,dim2]"
#[cfg(test)]
pub fn parse_array_declaration(decl: &str) -> (String, Vec<String>) {
    if let Some(bracket_pos) = decl.find('[') {
        let name = decl[..bracket_pos].to_string();
        let dims_str = &decl[bracket_pos + 1..decl.len() - 1];
        let dims = dims_str.split(',').map(|s| s.trim().to_string()).collect();
        (name, dims)
    } else {
        (decl.to_string(), vec![])
    }
}
