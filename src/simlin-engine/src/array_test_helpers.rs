// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Test infrastructure for array functionality
//!
//! This module provides a builder-based API for creating test projects
//! with arrays, making it easy to test array functionality incrementally.

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

/// Builder for creating test projects with arrays
#[cfg(test)]
pub struct ArrayTestProject {
    name: String,
    dimensions: Vec<Dimension>,
    variables: Vec<Variable>,
    sim_specs: SimSpecs,
}

#[cfg(test)]
impl ArrayTestProject {
    /// Create a new test project builder
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            dimensions: Vec::new(),
            variables: Vec::new(),
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 1.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: Some(datamodel::Dt::Dt(1.0)),
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            },
        }
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

    /// Add a constant scalar variable
    pub fn scalar_const(mut self, name: &str, value: f64) -> Self {
        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: Equation::Scalar(value.to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    /// Add a scalar auxiliary variable with an equation
    pub fn scalar_aux(mut self, name: &str, equation: &str) -> Self {
        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: Equation::Scalar(equation.to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    /// Add an array constant (all elements have the same value)
    pub fn array_const(mut self, name_with_dims: &str, value: f64) -> Self {
        // Parse name[dim1,dim2] format
        let (name, dims) = parse_array_declaration(name_with_dims);

        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name,
            equation: Equation::ApplyToAll(dims, value.to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    /// Add an array auxiliary variable with an equation
    pub fn array_aux(mut self, name_with_dims: &str, equation: &str) -> Self {
        // Parse name[dim1,dim2] format
        let (name, dims) = parse_array_declaration(name_with_dims);

        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name,
            equation: Equation::ApplyToAll(dims, equation.to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
        }));
        self
    }

    /// Add an array with different equations for different subscript ranges
    #[allow(dead_code)]
    pub fn array_with_ranges(
        mut self,
        name_with_dims: &str,
        equations: Vec<(&str, &str)>, // (element_name, equation)
    ) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);

        let arrayed_equations = equations
            .into_iter()
            .map(|(elem, eq)| (elem.to_string(), eq.to_string(), None))
            .collect();

        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name,
            equation: Equation::Arrayed(dims, arrayed_equations),
            documentation: String::new(),
            units: None,
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
            units: vec![],
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

        // Collect any compilation errors
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

            // Check variable-level errors
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

        // Note: We don't modify non-array variables here because some might be
        // true scalars that need last-timestep-only handling, but we can't
        // distinguish them from arrays that have been flattened

        Ok(output)
    }

    /// Test that compilation succeeds
    pub fn assert_compiles(&self) {
        match self.compile() {
            Ok(_compiled) => {}
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
fn parse_array_declaration(decl: &str) -> (String, Vec<String>) {
    if let Some(bracket_pos) = decl.find('[') {
        let name = decl[..bracket_pos].to_string();
        let dims_str = &decl[bracket_pos + 1..decl.len() - 1];
        let dims = dims_str.split(',').map(|s| s.trim().to_string()).collect();
        (name, dims)
    } else {
        (decl.to_string(), vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scalar_constant() {
        ArrayTestProject::new("test")
            .scalar_const("x", 42.0)
            .assert_compiles();
    }

    #[test]
    fn test_array_constant() {
        ArrayTestProject::new("test")
            .indexed_dimension("Time", 5)
            .array_const("values[Time]", 10.0)
            .assert_compiles();
    }

    #[test]
    fn test_named_dimension() {
        ArrayTestProject::new("test")
            .named_dimension("Location", &["Boston", "NYC", "LA"])
            .array_const("population[Location]", 1000000.0)
            .assert_compiles();
    }

    #[test]
    fn test_array_equation() {
        ArrayTestProject::new("test")
            .indexed_dimension("Index", 3)
            .scalar_const("base", 10.0)
            .array_aux("derived[Index]", "base * 2")
            .assert_compiles();
    }

    #[test]
    fn test_multi_dimensional_array() {
        ArrayTestProject::new("test")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_const("matrix[Row,Col]", 1.0)
            .assert_compiles();
    }
}
