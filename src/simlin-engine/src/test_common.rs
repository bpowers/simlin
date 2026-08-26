// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Common test infrastructure for building test projects
//!
//! This module provides a builder-based API for creating test projects
//! that can be used by various test modules.

use crate::common::{Canonical, ErrorCode, Ident, UnitError};
use crate::datamodel::{self, Dimension, Equation, Project, SimSpecs, Variable};
#[cfg(test)]
use crate::db::sync_from_datamodel;
use crate::db::{
    DiagnosticError, DiagnosticSeverity, SimlinDb, collect_all_diagnostics,
    compile_project_incremental, sync_from_datamodel_incremental,
};
use crate::vm::{CompiledSimulation, Vm};
use std::collections::HashMap;

/// The `(input, expected)` contract table for the ROUND builtin:
/// round-half-to-even (Python `round()` / IEEE roundTiesToEven).
///
/// This is the SINGLE copy of the case rows, consumed by all three backend
/// tests -- the end-to-end VM pipeline (`round_builtin_tests`), the VM's
/// `apply()` unit test (`vm::tests::apply_round_ties_to_even`), and the wasm
/// backend's parity test (`wasmgen::lower_tests::apply_round`) -- so the
/// backends are pinned against identical rows and cannot drift apart. The
/// exact .5 ties are the load-bearing rows: `f64::round` (ties away from
/// zero) agrees with `round_ties_even` on every non-tie input, so a table
/// without them would pass with the wrong function.
///
/// Comparison contract: a NaN expectation means "result is NaN" (payload
/// bits unspecified -- wasm may canonicalize); every other row compares by
/// BIT PATTERN, which is what makes the signed-zero rows meaningful.
pub const ROUND_TIES_TO_EVEN_CASES: &[(f64, f64)] = &[
    // Exact .5 ties -> even neighbor (the rows that distinguish
    // round-half-even from round-half-away-from-zero).
    (0.5, 0.0),
    (1.5, 2.0),
    (2.5, 2.0),
    (3.5, 4.0),
    (4.5, 4.0),
    // roundTiesToEven preserves the sign of zero: round(-0.5) is NEGATIVE
    // zero (Python's float rounding agrees: round(-0.5, 0) == -0.0).
    (-0.5, -0.0),
    (-1.5, -2.0),
    (-2.5, -2.0),
    (-3.5, -4.0),
    // Non-ties round to nearest.
    (2.4, 2.0),
    (2.6, 3.0),
    (-2.4, -2.0),
    (-2.6, -3.0),
    (0.4999, 0.0),
    // The double closest to but below 0.5: rounds to 0, distinguishing
    // binary-value rounding from decimal-spelling rounding.
    (0.499_999_999_999_999_94, 0.0),
    // Integers and zeros are identities (sign of zero preserved).
    (7.0, 7.0),
    (-7.0, -7.0),
    (0.0, 0.0),
    (-0.0, -0.0),
    // At 2^52 every double is already an integer: identity, no precision
    // loss; and the largest representable .5 tie below it rounds to the
    // even 2^52.
    (4_503_599_627_370_496.0, 4_503_599_627_370_496.0),
    (4_503_599_627_370_495.5, 4_503_599_627_370_496.0),
    // Specials pass through.
    (f64::NAN, f64::NAN),
    (f64::INFINITY, f64::INFINITY),
    (f64::NEG_INFINITY, f64::NEG_INFINITY),
];

/// Assert one ROUND backend result against a [`ROUND_TIES_TO_EVEN_CASES`]
/// expectation, applying the table's comparison contract (NaN by class,
/// everything else by bit pattern).
pub fn assert_round_case(input: f64, got: f64, expected: f64, backend: &str) {
    if expected.is_nan() {
        assert!(
            got.is_nan(),
            "{backend}: round({input}) expected NaN, got {got}"
        );
    } else {
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "{backend}: round({input}) got {got}, expected {expected}"
        );
    }
}

/// Builder for creating test projects with support for arrays, units, and all variable types
pub struct TestProject {
    pub name: String,
    pub dimensions: Vec<Dimension>,
    pub variables: Vec<Variable>,
    pub units: Vec<datamodel::Unit>,
    pub sim_specs: SimSpecs,
    /// When `Some`, [`TestProject::build_datamodel`] returns this whole
    /// `datamodel::Project` verbatim instead of synthesizing a single
    /// `"main"` model from `variables`. Used to wrap a `convert_mdl`-produced
    /// multi-model project (e.g. macro-bearing models) so the full
    /// compile/run/diagnostic assertion surface (`assert_vm_result`,
    /// `assert_compile_error_vm`) applies unchanged.
    datamodel_override: Option<Project>,
}

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
            datamodel_override: None,
        }
    }

    /// Wrap an already-built `datamodel::Project` (e.g. from `convert_mdl`)
    /// so the `TestProject` compile/run/diagnostic helpers apply to it
    /// directly. The builder methods (`.aux()`, `.stock()`, ...) are not
    /// meaningful on a wrapped project and are ignored by `build_datamodel`.
    pub fn from_datamodel(project: Project) -> Self {
        Self {
            name: project.name.clone(),
            dimensions: Vec::new(),
            variables: Vec::new(),
            units: Vec::new(),
            sim_specs: project.sim_specs.clone(),
            datamodel_override: Some(project),
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
            datamodel_override: None,
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
        // Default save_step to dt so callers don't have to set it separately.
        // If save_step differs from dt, use with_save_step() after this.
        self.sim_specs.save_step = None;
        self
    }

    /// Set the integration method
    pub fn with_sim_method(mut self, method: datamodel::SimMethod) -> Self {
        self.sim_specs.sim_method = method;
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

    /// Add a unit definition with alias names (a Vensim `22:` equivalence
    /// group). The first name is the primary; the rest are aliases.
    pub fn unit_with_aliases(mut self, name: &str, aliases: &[&str]) -> Self {
        self.units.push(datamodel::Unit {
            name: name.to_string(),
            equation: None,
            disabled: false,
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    /// Add an indexed dimension (e.g., for numeric indices)
    pub fn indexed_dimension(mut self, name: &str, size: u32) -> Self {
        self.dimensions
            .push(Dimension::indexed(name.to_string(), size));
        self
    }

    /// Add an indexed subdimension that is a subset of a parent indexed dimension.
    pub fn indexed_subdimension(mut self, name: &str, size: u32, parent: &str) -> Self {
        let mut dim = Dimension::indexed(name.to_string(), size);
        dim.parent = Some(parent.to_string());
        self.dimensions.push(dim);
        self
    }

    /// Add a named dimension with specific elements
    pub fn named_dimension(mut self, name: &str, elements: &[&str]) -> Self {
        self.dimensions.push(Dimension::named(
            name.to_string(),
            elements.iter().map(|s| s.to_string()).collect(),
        ));
        self
    }

    /// Add an already-built dimension, for a shape the named constructors do
    /// not cover -- a test that varies ONE dimension's mapping across fixtures
    /// while keeping the rest of the model identical builds it directly and
    /// hands it in here.
    pub fn with_dimension(mut self, dim: Dimension) -> Self {
        self.dimensions.push(dim);
        self
    }

    /// Add a named dimension with a dimension mapping (e.g., DimA -> DimB)
    pub fn named_dimension_with_mapping(
        mut self,
        name: &str,
        elements: &[&str],
        maps_to: &str,
    ) -> Self {
        let mut dim = Dimension::named(
            name.to_string(),
            elements.iter().map(|s| s.to_string()).collect(),
        );
        dim.set_maps_to(maps_to.to_string());
        self.dimensions.push(dim);
        self
    }

    /// Add a named dimension with an explicit element-level mapping to a
    /// target dimension. Unlike `named_dimension_with_mapping` (positional,
    /// requires equal cardinality), an element map can be many-to-one
    /// (e.g. `State{s1,s2,s3} -> Region{a,b}` with s1->a, s2->a, s3->b).
    pub fn named_dimension_with_element_mapping(
        mut self,
        name: &str,
        elements: &[&str],
        target: &str,
        element_map: &[(&str, &str)],
    ) -> Self {
        let mut dim = Dimension::named(
            name.to_string(),
            elements.iter().map(|s| s.to_string()).collect(),
        );
        dim.mappings = vec![datamodel::DimensionMapping {
            target: target.to_string(),
            element_map: element_map
                .iter()
                .map(|(s, t)| (s.to_string(), t.to_string()))
                .collect(),
        }];
        self.dimensions.push(dim);
        self
    }

    /// Add a named dimension carrying SEVERAL mappings at once, each an
    /// optional element map (`&[]` means positional correspondence).
    ///
    /// A dimension with two mapping targets is the shape the implicit-axis
    /// allocator's precedence rule is about (GH #996): one target can be
    /// claimed by an earlier dependency axis while a later axis needs the
    /// other. `named_dimension_with_mapping` and
    /// `named_dimension_with_element_mapping` each declare exactly one, so
    /// neither can express it.
    pub fn named_dimension_with_mappings(
        mut self,
        name: &str,
        elements: &[&str],
        mappings: &[(&str, &[(&str, &str)])],
    ) -> Self {
        let mut dim = Dimension::named(
            name.to_string(),
            elements.iter().map(|s| s.to_string()).collect(),
        );
        dim.mappings = mappings
            .iter()
            .map(|(target, element_map)| datamodel::DimensionMapping {
                target: target.to_string(),
                element_map: element_map
                    .iter()
                    .map(|(s, t)| (s.to_string(), t.to_string()))
                    .collect(),
            })
            .collect();
        self.dimensions.push(dim);
        self
    }

    /// Add an auxiliary variable
    pub fn aux(mut self, name: &str, equation: &str, units: Option<&str>) -> Self {
        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        self
    }

    /// Add an auxiliary variable backed by a graphical function. The `equation`
    /// is the lookup input expression; `gf` is the table the value is looked up
    /// in. With a real input expression this lowers to `LOOKUP(self, input)`.
    pub fn aux_with_gf(
        mut self,
        name: &str,
        equation: &str,
        gf: datamodel::GraphicalFunction,
    ) -> Self {
        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: Some(gf),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        self
    }

    /// Add a flow variable
    pub fn flow(mut self, name: &str, equation: &str, units: Option<&str>) -> Self {
        self.variables.push(Variable::Flow(datamodel::Flow {
            ident: name.to_string(),
            equation: Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
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
            equation: Equation::Scalar(initial.to_string()),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            inflows: inflows.iter().map(|s| s.to_string()).collect(),
            outflows: outflows.iter().map(|s| s.to_string()).collect(),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        self
    }

    /// Add a stock variable with all configurable options
    #[allow(clippy::too_many_arguments)]
    pub fn stock_with_options(
        mut self,
        name: &str,
        initial: &str,
        inflows: &[&str],
        outflows: &[&str],
        units: Option<&str>,
        documentation: &str,
        non_negative: bool,
        can_be_module_input: bool,
        visibility: datamodel::Visibility,
        uid: Option<i32>,
    ) -> Self {
        self.variables.push(Variable::Stock(datamodel::Stock {
            ident: name.to_string(),
            equation: Equation::Scalar(initial.to_string()),
            documentation: documentation.to_string(),
            units: units.map(|s| s.to_string()),
            inflows: inflows.iter().map(|s| s.to_string()).collect(),
            outflows: outflows.iter().map(|s| s.to_string()).collect(),
            ai_state: None,
            uid,
            compat: datamodel::Compat {
                non_negative,
                can_be_module_input,
                visibility,
                ..datamodel::Compat::default()
            },
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

    /// Add an array constant with units using "name[dims]" notation
    pub fn array_const_with_units(self, name_with_dims: &str, value: f64, units: &str) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        self.array_aux_direct(&name, dims, &value.to_string(), Some(units))
    }

    /// Add an array auxiliary using "name[dims]" notation
    pub fn array_aux(self, name_with_dims: &str, equation: &str) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        self.array_aux_direct(&name, dims, equation, None)
    }

    /// Add an array stock using "name[dims]" notation (apply-to-all equation)
    pub fn array_stock(
        mut self,
        name_with_dims: &str,
        initial: &str,
        inflows: &[&str],
        outflows: &[&str],
        units: Option<&str>,
    ) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        self.variables.push(Variable::Stock(datamodel::Stock {
            ident: name,
            equation: Equation::ApplyToAll(dims, initial.to_string()),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            inflows: inflows.iter().map(|s| s.to_string()).collect(),
            outflows: outflows.iter().map(|s| s.to_string()).collect(),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        self
    }

    /// Add an array flow using "name[dims]" notation (apply-to-all equation)
    pub fn array_flow(mut self, name_with_dims: &str, equation: &str, units: Option<&str>) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        self.variables.push(Variable::Flow(datamodel::Flow {
            ident: name,
            equation: Equation::ApplyToAll(dims, equation.to_string()),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        self
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
            equation: Equation::ApplyToAll(dims, equation.to_string()),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
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
            .map(|(elem, eq)| (elem.to_string(), eq.to_string(), None, None))
            .collect();

        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: Equation::Arrayed(dims, arrayed_equations, None, false),
            documentation: String::new(),
            units: units.map(|s| s.to_string()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        self
    }

    /// Add an array *flow* with different equations for different subscript
    /// ranges (a per-element `Equation::Arrayed` flow, the shape the MDL
    /// importer produces for Vensim flows -- each element's equation
    /// references other variables by literal element subscripts).
    pub fn array_flow_with_ranges(
        mut self,
        name_with_dims: &str,
        equations: Vec<(&str, &str)>, // (element_name, equation)
    ) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        let arrayed_equations = equations
            .into_iter()
            .map(|(elem, eq)| (elem.to_string(), eq.to_string(), None, None))
            .collect();

        self.variables.push(Variable::Flow(datamodel::Flow {
            ident: name,
            equation: Equation::Arrayed(dims, arrayed_equations, None, false),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        self
    }

    /// Add an array variable with a default equation and per-element overrides (EXCEPT semantics)
    pub fn array_with_default_and_overrides(
        mut self,
        name_with_dims: &str,
        default_equation: &str,
        overrides: Vec<(&str, &str)>,
    ) -> Self {
        let (name, dims) = parse_array_declaration(name_with_dims);
        let arrayed_equations = overrides
            .into_iter()
            .map(|(elem, eq)| (elem.to_string(), eq.to_string(), None, None))
            .collect();
        self.variables.push(Variable::Aux(datamodel::Aux {
            ident: name,
            equation: Equation::Arrayed(
                dims,
                arrayed_equations,
                Some(default_equation.to_string()),
                true,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        self
    }

    /// Build the datamodel Project
    pub fn build_datamodel(&self) -> Project {
        if let Some(project) = &self.datamodel_override {
            return project.clone();
        }
        Project {
            name: self.name.clone(),
            sim_specs: self.sim_specs.clone(),
            dimensions: self.dimensions.clone(),
            units: self.units.clone(),
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: self.variables.clone(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: Default::default(),
            ai_information: None,
        }
    }
}

/// Methods for tests that inspect the compiler's intermediate results rather
/// than simulation values. `#[cfg(test)]` because they reach into crate-private
/// per-variable lowering that the `test-support` feature does not export.
#[cfg(test)]
impl TestProject {
    /// The `Error`-severity salsa diagnostics of this project, as
    /// `(location, code)` pairs in emission order.
    ///
    /// `location` is `model.variable` for a variable-attributed diagnostic, the
    /// model name for a model-level one, and `"project"` for the project-level
    /// ones (the macro-registry build error and the unit definition errors,
    /// which name no model). A test that must pin WHICH variable a refusal lands
    /// on reads this rather than [`TestProject::assert_compile_error_vm`], which
    /// accepts the code anywhere in the project.
    pub fn error_diagnostics(&self) -> Vec<(String, ErrorCode)> {
        self.diagnostics_incremental()
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Error)
            .map(|d| {
                let location = match (d.model.as_str(), d.variable.as_deref()) {
                    ("", _) => "project".to_string(),
                    (model, Some(var)) => format!("{model}.{var}"),
                    (model, None) => model.to_string(),
                };
                let code = match &d.error {
                    DiagnosticError::Equation(eq_err) => eq_err.code,
                    DiagnosticError::Model(err) => err.code,
                    DiagnosticError::Unit(unit_err) => match unit_err {
                        UnitError::DefinitionError(eq_err, _) => eq_err.code,
                        UnitError::ConsistencyError(code, _, _) => *code,
                        UnitError::InferenceError { code, .. } => *code,
                    },
                    DiagnosticError::Assembly(_) => ErrorCode::NotSimulatable,
                };
                (location, code)
            })
            .collect()
    }

    /// The production-lowered flow-phase expressions of the `main` model's
    /// `var_name`: one `Expr` per element of an arrayed equation, preceded by
    /// any temps the lowering hoisted.
    ///
    /// Sourced through `db::var_fragment::lower_var_fragment`, the exact
    /// per-variable lowering `compile_var_fragment` runs, so a structural
    /// assertion over this list constrains what the fragment compiler emits.
    /// Panics when the variable has no `SourceVariable` (an implicit helper) or
    /// does not lower; a test that expects a refusal reads
    /// [`TestProject::error_diagnostics`] instead.
    pub fn flow_exprs(&self, var_name: &str) -> Vec<crate::compiler::Expr> {
        let datamodel = self.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        crate::db::var_noninitial_lowered_exprs(
            &db,
            sync.models["main"].source,
            sync.project,
            &crate::canonicalize(var_name),
        )
    }
}

impl TestProject {
    /// Get VM results for a variable (allows checking for NaN values)
    pub fn vm_result(&self, var_name: &str) -> Vec<f64> {
        let results = self.run_vm().expect("VM should run successfully");

        results
            .get(var_name)
            .unwrap_or_else(|| panic!("Variable {var_name} not found in VM results"))
            .clone()
    }

    /// Compile and run the project, returning every variable's series.
    ///
    /// There is only ONE compile path (`compile_incremental`);
    /// `run_vm_expecting_success` differs only in panicking instead of
    /// returning `Result`, and delegates here so the path cannot fork.
    pub fn run_vm(&self) -> Result<HashMap<String, Vec<f64>>, String> {
        let compiled = self
            .compile_incremental()
            .map_err(|e| format!("VM compilation failed: {e:?}"))?;
        let mut vm = Vm::new(compiled).map_err(|e| format!("VM creation failed: {e:?}"))?;
        vm.run_to_end()
            .map_err(|e| format!("VM run failed: {e:?}"))?;
        let results = vm.into_results();
        Ok(collect_results(&results))
    }

    /// Test that VM evaluation succeeds and returns expected values
    pub fn assert_vm_result(&self, var_name: &str, expected: &[f64]) {
        let results = self.run_vm().expect("VM should run successfully");

        let actual = results
            .get(var_name)
            .unwrap_or_else(|| panic!("Variable {var_name} not found in VM results"));

        assert_eq!(
            actual.len(),
            expected.len(),
            "VM result length mismatch for {var_name}: expected {}, got {}",
            expected.len(),
            actual.len()
        );

        for (i, (actual_val, expected_val)) in actual.iter().zip(expected.iter()).enumerate() {
            assert!(
                (actual_val - expected_val).abs() < 1e-6,
                "VM value mismatch for {var_name} at index {i}: expected {expected_val}, got {actual_val}"
            );
        }
    }

    // ── Incremental compilation methods ────────────────────────────────

    /// Compile the project via the incremental salsa pipeline.
    pub fn compile_incremental(&self) -> crate::Result<CompiledSimulation> {
        let datamodel = self.build_datamodel();
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
        compile_project_incremental(&db, sync.project, "main")
    }

    /// [`TestProject::run_vm`], panicking instead of returning an error.
    pub fn run_vm_expecting_success(&self) -> HashMap<String, Vec<f64>> {
        self.run_vm().expect("VM should run successfully")
    }

    /// Assert that compilation succeeds.
    ///
    /// Returns `&Self` so a caller can chain a second assertion onto the same
    /// builder expression -- notably [`TestProject::assert_no_unit_diagnostics`],
    /// which this one CANNOT stand in for: unit diagnostics are Warning
    /// severity, so compilation succeeds whether or not unit checking works.
    pub fn assert_compiles_incremental(&self) -> &Self {
        if let Err(e) = self.compile_incremental() {
            panic!("Incremental compilation failed: {e:?}");
        }
        self
    }

    // ── Diagnostic helpers (incremental path) ─────────────────────────

    /// The unit-related diagnostics of this project, as
    /// `(variable, human-readable detail)` pairs, in emission order --
    /// including the model-level inference umbrella (variable `None`).
    /// Message-content and dedup tests read this rather than the raw
    /// `Diagnostic` list so they don't restate the error-variant plumbing.
    pub fn unit_diagnostic_details(&self) -> Vec<(Option<String>, String)> {
        self.diagnostics_incremental()
            .into_iter()
            .filter_map(|d| {
                let detail = match &d.error {
                    DiagnosticError::Unit(UnitError::ConsistencyError(_, _, details)) => {
                        details.clone().unwrap_or_default()
                    }
                    DiagnosticError::Unit(UnitError::DefinitionError(err, details)) => {
                        match details {
                            Some(s) => format!("{err} -- {s}"),
                            None => format!("{err}"),
                        }
                    }
                    DiagnosticError::Unit(UnitError::InferenceError { details, .. }) => {
                        details.clone().unwrap_or_default()
                    }
                    DiagnosticError::Model(e) if e.code == ErrorCode::UnitMismatch => {
                        e.details.clone().unwrap_or_default()
                    }
                    _ => return None,
                };
                Some((d.variable.clone(), detail))
            })
            .collect()
    }

    /// Assert the project produces NO unit diagnostics at all (mismatches,
    /// definition errors, or the model-level inference umbrella). Stronger
    /// than `assert_compiles_incremental`, which ignores Warning-severity
    /// unit diagnostics entirely.
    pub fn assert_no_unit_diagnostics(&self) {
        let details = self.unit_diagnostic_details();
        assert!(
            details.is_empty(),
            "expected no unit diagnostics, got: {details:#?}"
        );
    }

    /// Sync the datamodel into a salsa DB and collect all diagnostics.
    pub(crate) fn diagnostics_incremental(&self) -> Vec<crate::db::Diagnostic> {
        let datamodel = self.build_datamodel();
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
        collect_all_diagnostics(&db, sync.project)
    }

    /// Assert that incremental compilation produces the expected error code.
    pub fn assert_compile_error_vm(&self, expected_error: ErrorCode) {
        let diagnostics = self.diagnostics_incremental();

        let has_error = diagnostics.iter().any(|d| {
            d.severity == DiagnosticSeverity::Error
                && match &d.error {
                    DiagnosticError::Equation(eq_err) => eq_err.code == expected_error,
                    DiagnosticError::Model(err) => err.code == expected_error,
                    _ => false,
                }
        });

        if !has_error {
            if diagnostics.is_empty() {
                panic!(
                    "Expected compilation error {expected_error:?}, but no diagnostics were emitted"
                );
            } else {
                let diag_summary: Vec<_> = diagnostics
                    .iter()
                    .map(|d| format!("{}: {:?} ({:?})", d.model, d.error, d.severity))
                    .collect();
                panic!(
                    "Expected compilation error {expected_error:?}, but got:\n{}",
                    diag_summary.join("\n")
                );
            }
        }
    }

    /// Assert that incremental compilation produces a unit mismatch diagnostic.
    pub fn assert_unit_error_vm(&self) {
        let diagnostics = self.diagnostics_incremental();

        let has_unit_mismatch = diagnostics.iter().any(|d| {
            if let DiagnosticError::Unit(unit_err) = &d.error {
                let code = match unit_err {
                    UnitError::DefinitionError(eq_err, _) => eq_err.code,
                    UnitError::ConsistencyError(code, _, _) => *code,
                    UnitError::InferenceError { code, .. } => *code,
                };
                code == ErrorCode::UnitMismatch
            } else {
                false
            }
        });

        if !has_unit_mismatch {
            let unit_diags: Vec<_> = diagnostics
                .iter()
                .filter(|d| matches!(&d.error, DiagnosticError::Unit(_)))
                .map(|d| {
                    format!(
                        "{}.{}: {:?}",
                        d.model,
                        d.variable.as_deref().unwrap_or("?"),
                        d.error
                    )
                })
                .collect();
            if unit_diags.is_empty() {
                panic!("Expected unit mismatch warning, but no unit diagnostics were found");
            } else {
                panic!("Expected UnitMismatch, but got:\n{}", unit_diags.join("\n"));
            }
        }
    }

    /// Assert that a scalar variable's final-timestep value matches the expected value.
    pub fn assert_vm_scalar_result(&self, var_name: &str, expected: f64) {
        let results = self.run_vm().expect("VM should run successfully");

        let series = results
            .get(var_name)
            .unwrap_or_else(|| panic!("variable '{var_name}' not found in VM results"));

        let actual = *series
            .last()
            .unwrap_or_else(|| panic!("variable '{var_name}' has empty timeseries"));

        let diff = (actual - expected).abs();
        assert!(
            diff < 1e-6,
            "variable '{var_name}': expected {expected}, got {actual} (diff: {diff})"
        );
    }
}

/// Extract variable timeseries from simulation results, including
/// aggregated base-name entries for arrayed variables.
pub(crate) fn collect_results(results: &crate::Results) -> HashMap<String, Vec<f64>> {
    let mut output: HashMap<String, Vec<f64>> = HashMap::new();

    for (name, &offset) in &results.offsets {
        let mut values = Vec::new();
        for step in 0..results.step_count {
            let idx = step * results.step_size + offset;
            values.push(results.data[idx]);
        }
        output.insert(name.to_string(), values);
    }

    type ArrayElement = (usize, String, Vec<f64>);
    let mut array_results: HashMap<Ident<Canonical>, Vec<ArrayElement>> = HashMap::new();
    for (name, values) in &output {
        if let Some(bracket_pos) = name.as_str().find('[') {
            let base_name = Ident::<Canonical>::from_str_unchecked(&name.as_str()[..bracket_pos]);
            let offset = results
                .offsets
                .get(&Ident::<Canonical>::from_str_unchecked(name))
                .copied()
                .unwrap_or(usize::MAX);
            let entry = array_results.entry(base_name.clone()).or_default();
            entry.push((offset, name.to_string(), values.clone()));
        }
    }

    for (base_name, mut elements) in array_results {
        elements.sort_by_key(|e| e.0);
        if !elements.is_empty() {
            let n_steps = elements[0].2.len();
            let mut combined = Vec::new();
            let last_step = n_steps - 1;
            for (_offset, _name, values) in &elements {
                if last_step < values.len() {
                    combined.push(values[last_step]);
                }
            }
            output.insert(base_name.to_string(), combined);
        }
    }

    output
}

/// Helper to parse array declarations like "name[dim1,dim2]"
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

/// A `main` model that instantiates ONE arrayed sub-model TWICE, with a
/// different input wired into each instance.
///
/// The sub-model reduces an array three ways -- `SUM(arr[*])`, an array-valued
/// `SUM(PREVIOUS(arr[*]))` and `SUM(INIT(arr[*]))` (GH #995) -- so all three
/// chunk-shaped view regions (`Curr`, `Prev`, `Initial`) are exercised on the
/// same fixture. Every reduction is pushed as a STATIC VIEW, whose `base_off`
/// comes from the sub-model's own layout and is therefore module-relative; a
/// backend that fails to add the executing instance's `module_off` reads the
/// ROOT's slots instead, and both instances then return the same wrong series.
///
/// Three separate slips are distinguishable in the numbers:
///
/// * `arr[D] = in * w[D]` with `w = [1, 2, 4]`, so `SUM(arr[*]) = 7 * in` and a
///   base offset that is wrong WITHIN the instance lands on a different weight.
/// * the two instances' inputs differ by 100x, so cross-instance aliasing shows
///   up as one instance's series appearing in the other.
/// * both inputs vary with TIME, so `PREVIOUS` and `INIT` are distinguishable
///   from `curr` and from each other.
///
/// Note what was NOT broken, so the fixture's shape reads as necessary rather
/// than belt-and-braces: a cross-module read taken FROM THE ROOT was always
/// correct, because the root's `module_off` is 0 and the dropped addend is
/// invisible there (`array_tests::cross_module_array_reference_tests` passed
/// throughout). Only a view pushed while EXECUTING INSIDE an instance was wrong,
/// which is why this drives two instances rather than reading into one. The
/// two-HOP twin ([`nested_instance_arrayed_submodel_project`]) covers the other
/// axis a single hop cannot separate.
///
/// Shared by the VM pin
/// (`array_operand_materialization_tests::an_array_view_inside_a_module_instance_reads_that_instance`)
/// and the wasm pin
/// (`wasmgen::module::tests::compile_simulation_arrayed_submodel_views_address_their_instance`),
/// because the two backends agreeing proves nothing here: the wasm view emitter
/// mirrors the VM opcode for opcode, so it mirrored this defect too. Both assert
/// the absolute series.
pub fn two_instance_arrayed_submodel_project() -> Project {
    let aux = |ident: &str, eqn: &str, compat: datamodel::Compat| {
        Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: Equation::Scalar(eqn.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat,
        })
    };
    let instance = |ident: &str, src: &str| {
        Variable::Module(datamodel::Module {
            references: vec![datamodel::ModuleReference {
                src: src.to_string(),
                dst: format!("{ident}.in"),
            }],
            ident: ident.to_string(),
            model_name: "submodel".to_string(),
            documentation: String::new(),
            units: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        })
    };
    let arrayed = |ident: &str, eqn: Equation| {
        Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: eqn,
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    };

    Project {
        name: "two_instance_arrayed_submodel".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 3.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![Dimension::named(
            "D".to_string(),
            vec!["e1".to_string(), "e2".to_string(), "e3".to_string()],
        )],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    aux("a_in", "10 * (1 + TIME)", datamodel::Compat::default()),
                    aux("b_in", "1000 * (1 + TIME)", datamodel::Compat::default()),
                    instance("sub_a", "a_in"),
                    instance("sub_b", "b_in"),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: vec![
                    aux(
                        "in",
                        "0",
                        datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    ),
                    arrayed(
                        "w",
                        Equation::Arrayed(
                            vec!["D".to_string()],
                            vec![
                                ("e1".to_string(), "1".to_string(), None, None),
                                ("e2".to_string(), "2".to_string(), None, None),
                                ("e3".to_string(), "4".to_string(), None, None),
                            ],
                            None,
                            false,
                        ),
                    ),
                    arrayed(
                        "arr",
                        Equation::ApplyToAll(vec!["D".to_string()], "in * w[D]".to_string()),
                    ),
                    aux("out_curr", "SUM(arr[*])", datamodel::Compat::default()),
                    aux(
                        "out_prev",
                        "SUM(PREVIOUS(arr[*]))",
                        datamodel::Compat::default(),
                    ),
                    aux(
                        "out_init",
                        "SUM(INIT(arr[*]))",
                        datamodel::Compat::default(),
                    ),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: Default::default(),
        ai_information: None,
    }
}

/// The TWO-HOP twin of [`two_instance_arrayed_submodel_project`]: `main`
/// instantiates `mid` twice, and each `mid` instantiates `inner` once.
///
/// A one-hop fixture cannot separate two different addressing rules. The VM
/// reaches a nested instance by ACCUMULATING (`module_off + decl.off` at each
/// `EvalModule`), so a backend that applied only the LAST hop's offset -- or that
/// re-based from the root at each hop -- still gets a one-hop model right and
/// this one wrong. `mid` therefore carries a scalar of its own AHEAD of the
/// module declaration, so `inner`'s block does not start at its parent's base and
/// the two hops' offsets are distinct non-zero numbers that must sum.
///
/// Same arithmetic as the one-hop fixture (`SUM(arr[*]) = 7 * in`, inputs 100x
/// apart, both time-varying), so
/// [`two_instance_arrayed_submodel_expected`]'s reasoning carries over; only the
/// variable prefixes differ.
pub fn nested_instance_arrayed_submodel_project() -> Project {
    let aux = |ident: &str, eqn: &str, compat: datamodel::Compat| {
        Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: Equation::Scalar(eqn.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat,
        })
    };
    let instance = |ident: &str, model: &str, src: &str| {
        Variable::Module(datamodel::Module {
            references: vec![datamodel::ModuleReference {
                src: src.to_string(),
                dst: format!("{ident}.in"),
            }],
            ident: ident.to_string(),
            model_name: model.to_string(),
            documentation: String::new(),
            units: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        })
    };
    let input = || datamodel::Compat {
        can_be_module_input: true,
        ..datamodel::Compat::default()
    };
    let arrayed = |ident: &str, eqn: Equation| {
        Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: eqn,
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    };
    let model = |name: &str, variables: Vec<Variable>| datamodel::Model {
        name: name.to_string(),
        sim_specs: None,
        variables,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    };

    Project {
        name: "nested_instance_arrayed_submodel".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 3.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![Dimension::named(
            "D".to_string(),
            vec!["e1".to_string(), "e2".to_string(), "e3".to_string()],
        )],
        units: vec![],
        models: vec![
            model(
                "main",
                vec![
                    aux("a_in", "10 * (1 + TIME)", datamodel::Compat::default()),
                    aux("b_in", "1000 * (1 + TIME)", datamodel::Compat::default()),
                    instance("m_a", "mid", "a_in"),
                    instance("m_b", "mid", "b_in"),
                ],
            ),
            model(
                "mid",
                vec![
                    aux("in", "0", input()),
                    // Occupies mid's slot 0, so `inr`'s block starts past its
                    // parent's base and the two hops' offsets are both non-zero.
                    aux("pad", "in * 0", datamodel::Compat::default()),
                    instance("inr", "inner", "in"),
                ],
            ),
            model(
                "inner",
                vec![
                    aux("in", "0", input()),
                    arrayed(
                        "w",
                        Equation::Arrayed(
                            vec!["D".to_string()],
                            vec![
                                ("e1".to_string(), "1".to_string(), None, None),
                                ("e2".to_string(), "2".to_string(), None, None),
                                ("e3".to_string(), "4".to_string(), None, None),
                            ],
                            None,
                            false,
                        ),
                    ),
                    arrayed(
                        "arr",
                        Equation::ApplyToAll(vec!["D".to_string()], "in * w[D]".to_string()),
                    ),
                    aux("out_curr", "SUM(arr[*])", datamodel::Compat::default()),
                    aux(
                        "out_prev",
                        "SUM(PREVIOUS(arr[*]))",
                        datamodel::Compat::default(),
                    ),
                    aux(
                        "out_init",
                        "SUM(INIT(arr[*]))",
                        datamodel::Compat::default(),
                    ),
                ],
            ),
        ],
        source: Default::default(),
        ai_information: None,
    }
}

/// The series [`nested_instance_arrayed_submodel_project`] must produce.
pub fn nested_instance_arrayed_submodel_expected() -> Vec<(&'static str, Vec<f64>)> {
    let p = |instance: &str, var: &str| -> &'static str {
        Box::leak(format!("m_{instance}\u{b7}inr\u{b7}{var}").into_boxed_str())
    };
    vec![
        (p("a", "out_curr"), vec![70.0, 140.0, 210.0, 280.0]),
        (p("b", "out_curr"), vec![7000.0, 14000.0, 21000.0, 28000.0]),
        (p("a", "out_prev"), vec![0.0, 70.0, 140.0, 210.0]),
        (p("b", "out_prev"), vec![0.0, 7000.0, 14000.0, 21000.0]),
        (p("a", "out_init"), vec![70.0; 4]),
        (p("b", "out_init"), vec![7000.0; 4]),
    ]
}

/// The series [`two_instance_arrayed_submodel_project`] must produce, as
/// `(variable, values)` pairs. `in` is `10*(1+TIME)` for `sub_a` and 100x that
/// for `sub_b`, and `SUM(arr[*]) = 7 * in`.
pub fn two_instance_arrayed_submodel_expected() -> Vec<(&'static str, Vec<f64>)> {
    vec![
        ("sub_a\u{b7}out_curr", vec![70.0, 140.0, 210.0, 280.0]),
        (
            "sub_b\u{b7}out_curr",
            vec![7000.0, 14000.0, 21000.0, 28000.0],
        ),
        // The first step has no snapshot yet, so an array PREVIOUS reads its
        // only permitted fallback, 0.
        ("sub_a\u{b7}out_prev", vec![0.0, 70.0, 140.0, 210.0]),
        ("sub_b\u{b7}out_prev", vec![0.0, 7000.0, 14000.0, 21000.0]),
        ("sub_a\u{b7}out_init", vec![70.0; 4]),
        ("sub_b\u{b7}out_init", vec![7000.0; 4]),
    ]
}

/// Run `project` twice -- once straight to the end, once resting at each of
/// `rests` via `run_to` -- and assert every variable's saved series is
/// BIT-identical between the two runs. This is the special-stock (conveyor /
/// queue) mid-run-preview soundness gate: a rest runs the side-table pass as
/// a preview on cloned state, so any observable difference means the preview
/// leaked into the real belt/FIFO.
pub fn assert_segmented_run_identical(project: &datamodel::Project, rests: &[f64]) {
    let main = project.models[0].name.clone();
    let mut seg = crate::queue_compile::build_vm(project, &main).expect("build segmented vm");
    for &t in rests {
        seg.run_to(t).expect("segmented run_to");
    }
    seg.run_to_end().expect("segmented run_to_end");
    let mut full = crate::queue_compile::build_vm(project, &main).expect("build full vm");
    full.run_to_end().expect("full run_to_end");

    for name in full.names_as_strs() {
        let ident = Ident::new(&name);
        let a = full.get_series(&ident).expect("full series");
        let b = seg.get_series(&ident).expect("segmented series");
        assert_eq!(a.len(), b.len(), "{name}: series length");
        for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            assert_eq!(
                x.to_bits(),
                y.to_bits(),
                "{name} step {i}: full {x} != segmented {y}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_array_stock_and_flow_build() {
        let project_builder = TestProject::new("test")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_stock("population[Region]", "100", &["births"], &[], None)
            .array_flow("births[Region]", "population * 0.1", None);

        let project = project_builder.build_datamodel();
        let model = &project.models[0];

        // Verify the stock has ApplyToAll equation with correct dimensions
        let stock = model
            .variables
            .iter()
            .find(|v| matches!(v, Variable::Stock(s) if s.ident == "population"))
            .expect("population stock should exist");
        match stock {
            Variable::Stock(s) => match &s.equation {
                Equation::ApplyToAll(dims, eq) => {
                    assert_eq!(dims, &["Region".to_string()]);
                    assert_eq!(eq, "100");
                }
                other => panic!("expected ApplyToAll equation for stock, got {other:?}"),
            },
            _ => unreachable!(),
        }

        // Verify the flow has ApplyToAll equation with correct dimensions
        let flow = model
            .variables
            .iter()
            .find(|v| matches!(v, Variable::Flow(f) if f.ident == "births"))
            .expect("births flow should exist");
        match flow {
            Variable::Flow(f) => match &f.equation {
                Equation::ApplyToAll(dims, eq) => {
                    assert_eq!(dims, &["Region".to_string()]);
                    assert_eq!(eq, "population * 0.1");
                }
                other => panic!("expected ApplyToAll equation for flow, got {other:?}"),
            },
            _ => unreachable!(),
        }

        // Verify the model compiles without errors via the incremental path
        let project_builder2 = TestProject::new("test")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_stock("population[Region]", "100", &["births"], &[], None)
            .array_flow("births[Region]", "population * 0.1", None);
        project_builder2.assert_compiles_incremental();
    }
}
