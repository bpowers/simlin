// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};

use salsa::Accumulator;
use salsa::plumbing::AsId;

use crate::canonicalize;
use crate::common::{Canonical, EquationError, Error, Ident, UnitError};
use crate::datamodel;

// ── Database ───────────────────────────────────────────────────────────

#[salsa::db]
pub trait Db: salsa::Database {}

#[salsa::db]
#[derive(Default, Clone)]
pub struct SimlinDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for SimlinDb {}

#[salsa::db]
impl Db for SimlinDb {}

// ── Accumulator ───────────────────────────────────────────────────────

#[salsa::accumulator]
pub struct CompilationDiagnostic(pub Diagnostic);

/// A single compilation diagnostic emitted by tracked functions.
/// Carries enough context (model name, optional variable name) for
/// downstream formatting without re-walking the model tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub model: String,
    pub variable: Option<String>,
    pub error: DiagnosticError,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticError {
    Equation(EquationError),
    Model(Error),
    Unit(UnitError),
}

// ── Interned identifiers ───────────────────────────────────────────────

#[salsa::interned(debug)]
pub struct VariableId<'db> {
    #[returns(ref)]
    pub text: String,
}

#[salsa::interned(debug)]
pub struct ModelId<'db> {
    #[returns(ref)]
    pub text: String,
}

// ── Variable kind ──────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum SourceVariableKind {
    Stock,
    Flow,
    Aux,
    Module,
}

impl SourceVariableKind {
    fn from_datamodel_variable(var: &datamodel::Variable) -> Self {
        match var {
            datamodel::Variable::Stock(_) => SourceVariableKind::Stock,
            datamodel::Variable::Flow(_) => SourceVariableKind::Flow,
            datamodel::Variable::Aux(_) => SourceVariableKind::Aux,
            datamodel::Variable::Module(_) => SourceVariableKind::Module,
        }
    }
}

// ── Input types ────────────────────────────────────────────────────────

#[salsa::input]
pub struct SourceProject {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub sim_specs: SourceSimSpecs,
    #[returns(ref)]
    pub dimensions: Vec<SourceDimension>,
    #[returns(ref)]
    pub units: Vec<SourceUnit>,
    #[returns(ref)]
    pub model_names: Vec<String>,
    #[returns(ref)]
    pub models: HashMap<String, SourceModel>,
}

#[salsa::input]
pub struct SourceModel {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub variable_names: Vec<String>,
    #[returns(ref)]
    pub variables: HashMap<String, SourceVariable>,
}

#[salsa::input]
pub struct SourceVariable {
    #[returns(ref)]
    pub ident: String,
    #[returns(ref)]
    pub equation: SourceEquation,
    pub kind: SourceVariableKind,
    #[returns(ref)]
    pub units: Option<String>,
    #[returns(ref)]
    pub gf: Option<SourceGraphicalFunction>,
    #[returns(ref)]
    pub inflows: Vec<String>,
    #[returns(ref)]
    pub outflows: Vec<String>,
    #[returns(ref)]
    pub module_refs: Vec<SourceModuleReference>,
    #[returns(ref)]
    pub model_name: String,
    pub non_negative: bool,
    pub can_be_module_input: bool,
    #[returns(ref)]
    pub compat: datamodel::Compat,
}

// ── Mirror types for salsa compatibility ───────────────────────────────
//
// These types mirror the datamodel types but derive salsa::Update.
// This avoids modifying the datamodel module (which is used for
// serialization and has its own derive constraints).

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceSimSpecs {
    pub start: f64,
    pub stop: f64,
    pub dt: SourceDt,
    pub save_step: Option<SourceDt>,
    pub sim_method: SourceSimMethod,
    pub time_units: Option<String>,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub enum SourceDt {
    Dt(f64),
    Reciprocal(f64),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum SourceSimMethod {
    Euler,
    RungeKutta2,
    RungeKutta4,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceDimension {
    pub name: String,
    pub elements: SourceDimensionElements,
    pub maps_to: Option<String>,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub enum SourceDimensionElements {
    Indexed(u32),
    Named(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub enum SourceEquation {
    Scalar(String),
    ApplyToAll(Vec<String>, String),
    Arrayed(Vec<String>, Vec<SourceArrayedEquationElement>),
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceArrayedEquationElement {
    pub subscript: String,
    pub equation: String,
    pub gf_equation: Option<String>,
    pub gf: Option<SourceGraphicalFunction>,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceGraphicalFunction {
    pub kind: SourceGraphicalFunctionKind,
    pub x_points: Option<Vec<f64>>,
    pub y_points: Vec<f64>,
    pub x_scale: SourceGraphicalFunctionScale,
    pub y_scale: SourceGraphicalFunctionScale,
}

#[derive(Copy, Clone, Debug, PartialEq, salsa::Update)]
pub enum SourceGraphicalFunctionKind {
    Continuous,
    Extrapolate,
    Discrete,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceGraphicalFunctionScale {
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceModuleReference {
    pub src: String,
    pub dst: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub struct SourceUnit {
    pub name: String,
    pub equation: Option<String>,
    pub disabled: bool,
    pub aliases: Vec<String>,
}

// ── Conversion from datamodel types ────────────────────────────────────

impl From<&datamodel::SimSpecs> for SourceSimSpecs {
    fn from(specs: &datamodel::SimSpecs) -> Self {
        SourceSimSpecs {
            start: specs.start,
            stop: specs.stop,
            dt: SourceDt::from(&specs.dt),
            save_step: specs.save_step.as_ref().map(SourceDt::from),
            sim_method: SourceSimMethod::from(specs.sim_method),
            time_units: specs.time_units.clone(),
        }
    }
}

impl From<&datamodel::Dt> for SourceDt {
    fn from(dt: &datamodel::Dt) -> Self {
        match dt {
            datamodel::Dt::Dt(v) => SourceDt::Dt(*v),
            datamodel::Dt::Reciprocal(v) => SourceDt::Reciprocal(*v),
        }
    }
}

impl From<datamodel::SimMethod> for SourceSimMethod {
    fn from(method: datamodel::SimMethod) -> Self {
        match method {
            datamodel::SimMethod::Euler => SourceSimMethod::Euler,
            datamodel::SimMethod::RungeKutta2 => SourceSimMethod::RungeKutta2,
            datamodel::SimMethod::RungeKutta4 => SourceSimMethod::RungeKutta4,
        }
    }
}

impl From<&datamodel::Dimension> for SourceDimension {
    fn from(dim: &datamodel::Dimension) -> Self {
        SourceDimension {
            name: dim.name.clone(),
            elements: SourceDimensionElements::from(&dim.elements),
            maps_to: dim.maps_to.clone(),
        }
    }
}

impl From<&datamodel::DimensionElements> for SourceDimensionElements {
    fn from(elements: &datamodel::DimensionElements) -> Self {
        match elements {
            datamodel::DimensionElements::Indexed(size) => SourceDimensionElements::Indexed(*size),
            datamodel::DimensionElements::Named(names) => {
                SourceDimensionElements::Named(names.clone())
            }
        }
    }
}

impl From<&datamodel::Equation> for SourceEquation {
    fn from(eq: &datamodel::Equation) -> Self {
        match eq {
            datamodel::Equation::Scalar(s) => SourceEquation::Scalar(s.clone()),
            datamodel::Equation::ApplyToAll(dims, s) => {
                SourceEquation::ApplyToAll(dims.clone(), s.clone())
            }
            datamodel::Equation::Arrayed(dims, elements) => SourceEquation::Arrayed(
                dims.clone(),
                elements
                    .iter()
                    .map(|(subscript, eq, gf_eq, gf)| SourceArrayedEquationElement {
                        subscript: subscript.clone(),
                        equation: eq.clone(),
                        gf_equation: gf_eq.clone(),
                        gf: gf.as_ref().map(SourceGraphicalFunction::from),
                    })
                    .collect(),
            ),
        }
    }
}

impl From<&datamodel::GraphicalFunction> for SourceGraphicalFunction {
    fn from(gf: &datamodel::GraphicalFunction) -> Self {
        SourceGraphicalFunction {
            kind: SourceGraphicalFunctionKind::from(gf.kind),
            x_points: gf.x_points.clone(),
            y_points: gf.y_points.clone(),
            x_scale: SourceGraphicalFunctionScale::from(&gf.x_scale),
            y_scale: SourceGraphicalFunctionScale::from(&gf.y_scale),
        }
    }
}

impl From<datamodel::GraphicalFunctionKind> for SourceGraphicalFunctionKind {
    fn from(kind: datamodel::GraphicalFunctionKind) -> Self {
        match kind {
            datamodel::GraphicalFunctionKind::Continuous => SourceGraphicalFunctionKind::Continuous,
            datamodel::GraphicalFunctionKind::Extrapolate => {
                SourceGraphicalFunctionKind::Extrapolate
            }
            datamodel::GraphicalFunctionKind::Discrete => SourceGraphicalFunctionKind::Discrete,
        }
    }
}

impl From<&datamodel::GraphicalFunctionScale> for SourceGraphicalFunctionScale {
    fn from(scale: &datamodel::GraphicalFunctionScale) -> Self {
        SourceGraphicalFunctionScale {
            min: scale.min,
            max: scale.max,
        }
    }
}

impl From<&datamodel::ModuleReference> for SourceModuleReference {
    fn from(mr: &datamodel::ModuleReference) -> Self {
        SourceModuleReference {
            src: mr.src.clone(),
            dst: mr.dst.clone(),
        }
    }
}

impl From<&datamodel::Unit> for SourceUnit {
    fn from(unit: &datamodel::Unit) -> Self {
        SourceUnit {
            name: unit.name.clone(),
            equation: unit.equation.clone(),
            disabled: unit.disabled,
            aliases: unit.aliases.clone(),
        }
    }
}

// ── Reconstruct helpers ────────────────────────────────────────────────
//
// Convert Source* types back to datamodel types for use with the existing
// parsing pipeline (parse_var, lower_variable).

pub fn source_dims_to_datamodel(dims: &[SourceDimension]) -> Vec<datamodel::Dimension> {
    dims.iter()
        .map(|sd| {
            let elements = match &sd.elements {
                SourceDimensionElements::Indexed(size) => {
                    datamodel::DimensionElements::Indexed(*size)
                }
                SourceDimensionElements::Named(names) => {
                    datamodel::DimensionElements::Named(names.clone())
                }
            };
            datamodel::Dimension {
                name: sd.name.clone(),
                elements,
                maps_to: sd.maps_to.clone(),
            }
        })
        .collect()
}

fn source_units_to_datamodel(units: &[SourceUnit]) -> Vec<datamodel::Unit> {
    units
        .iter()
        .map(|su| datamodel::Unit {
            name: su.name.clone(),
            equation: su.equation.clone(),
            disabled: su.disabled,
            aliases: su.aliases.clone(),
        })
        .collect()
}

fn source_sim_specs_to_datamodel(specs: &SourceSimSpecs) -> datamodel::SimSpecs {
    datamodel::SimSpecs {
        start: specs.start,
        stop: specs.stop,
        dt: match &specs.dt {
            SourceDt::Dt(v) => datamodel::Dt::Dt(*v),
            SourceDt::Reciprocal(v) => datamodel::Dt::Reciprocal(*v),
        },
        save_step: specs.save_step.as_ref().map(|dt| match dt {
            SourceDt::Dt(v) => datamodel::Dt::Dt(*v),
            SourceDt::Reciprocal(v) => datamodel::Dt::Reciprocal(*v),
        }),
        sim_method: match specs.sim_method {
            SourceSimMethod::Euler => datamodel::SimMethod::Euler,
            SourceSimMethod::RungeKutta2 => datamodel::SimMethod::RungeKutta2,
            SourceSimMethod::RungeKutta4 => datamodel::SimMethod::RungeKutta4,
        },
        time_units: specs.time_units.clone(),
    }
}

fn source_gf_to_datamodel(gf: &SourceGraphicalFunction) -> datamodel::GraphicalFunction {
    datamodel::GraphicalFunction {
        kind: match gf.kind {
            SourceGraphicalFunctionKind::Continuous => datamodel::GraphicalFunctionKind::Continuous,
            SourceGraphicalFunctionKind::Extrapolate => {
                datamodel::GraphicalFunctionKind::Extrapolate
            }
            SourceGraphicalFunctionKind::Discrete => datamodel::GraphicalFunctionKind::Discrete,
        },
        x_points: gf.x_points.clone(),
        y_points: gf.y_points.clone(),
        x_scale: datamodel::GraphicalFunctionScale {
            min: gf.x_scale.min,
            max: gf.x_scale.max,
        },
        y_scale: datamodel::GraphicalFunctionScale {
            min: gf.y_scale.min,
            max: gf.y_scale.max,
        },
    }
}

fn source_equation_to_datamodel(eq: &SourceEquation) -> datamodel::Equation {
    match eq {
        SourceEquation::Scalar(s) => datamodel::Equation::Scalar(s.clone()),
        SourceEquation::ApplyToAll(dims, s) => {
            datamodel::Equation::ApplyToAll(dims.clone(), s.clone())
        }
        SourceEquation::Arrayed(dims, elements) => datamodel::Equation::Arrayed(
            dims.clone(),
            elements
                .iter()
                .map(|e| {
                    (
                        e.subscript.clone(),
                        e.equation.clone(),
                        e.gf_equation.clone(),
                        e.gf.as_ref().map(source_gf_to_datamodel),
                    )
                })
                .collect(),
        ),
    }
}

/// Reconstruct a `datamodel::Variable` from a `SourceVariable`.
pub fn reconstruct_variable(db: &dyn Db, var: SourceVariable) -> datamodel::Variable {
    let ident = var.ident(db).clone();
    let equation = source_equation_to_datamodel(var.equation(db));
    let units = var.units(db).clone();
    let non_negative = var.non_negative(db);
    let can_be_module_input = var.can_be_module_input(db);
    let compat = var.compat(db).clone();

    match var.kind(db) {
        SourceVariableKind::Stock => datamodel::Variable::Stock(datamodel::Stock {
            ident,
            equation,
            documentation: String::new(),
            units,
            inflows: var.inflows(db).clone(),
            outflows: var.outflows(db).clone(),
            non_negative,
            can_be_module_input,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat,
        }),
        SourceVariableKind::Flow => datamodel::Variable::Flow(datamodel::Flow {
            ident,
            equation,
            documentation: String::new(),
            units,
            gf: var.gf(db).as_ref().map(source_gf_to_datamodel),
            non_negative,
            can_be_module_input,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat,
        }),
        SourceVariableKind::Aux => datamodel::Variable::Aux(datamodel::Aux {
            ident,
            equation,
            documentation: String::new(),
            units,
            gf: var.gf(db).as_ref().map(source_gf_to_datamodel),
            can_be_module_input,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
            compat,
        }),
        SourceVariableKind::Module => datamodel::Variable::Module(datamodel::Module {
            ident,
            model_name: var.model_name(db).clone(),
            documentation: String::new(),
            units,
            references: var
                .module_refs(db)
                .iter()
                .map(|mr| datamodel::ModuleReference {
                    src: mr.src.clone(),
                    dst: mr.dst.clone(),
                })
                .collect(),
            can_be_module_input,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
        }),
    }
}

// ── Tracked functions ──────────────────────────────────────────────────

/// Result of parsing a single variable, including any implicit variables
/// generated by builtin expansion (e.g., DELAY1, SMTH create internal stocks).
#[derive(Clone, PartialEq, salsa::Update)]
pub struct ParsedVariableResult {
    pub variable: crate::model::VariableStage0,
    pub implicit_vars: Vec<datamodel::Variable>,
}

impl std::fmt::Debug for ParsedVariableResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedVariableResult")
            .field("ident", &self.variable.ident())
            .field("implicit_vars_count", &self.implicit_vars.len())
            .finish()
    }
}

/// Per-variable tracked function for parsing. Salsa memoizes the result
/// and only re-executes when the specific SourceVariable fields that were
/// read have changed. This means editing one variable's equation does NOT
/// re-parse other variables.
#[salsa::tracked(returns(ref))]
pub fn parse_source_variable(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> ParsedVariableResult {
    let dims = source_dims_to_datamodel(project.dimensions(db));

    let dm_units = source_units_to_datamodel(project.units(db));
    let dm_sim_specs = source_sim_specs_to_datamodel(project.sim_specs(db));
    let units_ctx =
        crate::units::Context::new_with_builtins(&dm_units, &dm_sim_specs).unwrap_or_default();

    let dm_var = reconstruct_variable(db, var);

    let mut implicit_vars = Vec::new();
    let variable =
        crate::variable::parse_var(&dims, &dm_var, &mut implicit_vars, &units_ctx, |mi| {
            Ok(Some(mi.clone()))
        });

    ParsedVariableResult {
        variable,
        implicit_vars,
    }
}

/// Dependency info for a single implicit variable generated by builtin expansion.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ImplicitVarDeps {
    pub name: String,
    pub is_stock: bool,
    pub dt_deps: BTreeSet<String>,
    pub initial_deps: BTreeSet<String>,
}

/// Direct dependencies extracted from a variable's equation.
/// For non-module variables, these are the identifiers referenced in the AST.
/// For module variables, these are the canonicalized source identifiers from
/// module references.
///
/// Module input filtering (isModuleInput branches) is NOT applied here --
/// that is per-instantiation and handled at the model level.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct VariableDeps {
    /// Dependencies used during normal dt timestep calculations.
    pub dt_deps: BTreeSet<String>,
    /// Dependencies used during initial value calculations.
    pub initial_deps: BTreeSet<String>,
    /// Dependencies for implicit variables generated by builtin expansion
    /// (e.g., SMOOTH, DELAY create internal stocks).
    pub implicit_vars: Vec<ImplicitVarDeps>,
}

/// Per-variable tracked function for dependency extraction. Salsa memoizes the
/// result and only re-executes when the parsed variable changes. This means
/// editing an equation to `a * b` from `a + b` (same deps) produces the same
/// VariableDeps, so downstream dependency graph computation is skipped.
#[salsa::tracked(returns(ref))]
pub fn variable_direct_dependencies(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> VariableDeps {
    match var.kind(db) {
        SourceVariableKind::Module => {
            let refs: BTreeSet<String> = var
                .module_refs(db)
                .iter()
                .map(|mr| canonicalize(&mr.src).into_owned())
                .collect();
            VariableDeps {
                dt_deps: refs.clone(),
                initial_deps: refs,
                implicit_vars: Vec::new(),
            }
        }
        _ => {
            let parsed = parse_source_variable(db, var, project);
            let dims = source_dims_to_datamodel(project.dimensions(db));
            let dim_context = crate::dimensions::DimensionsContext::from(dims.as_slice());

            // Non-module variables don't need the models map for lowering --
            // lower_ast only uses scope.dimensions for constify_dimensions and
            // scope.model_name for array context. get_variable returning None
            // is safe (arrays treated as scalar, which doesn't affect the
            // identifier set).
            let models = HashMap::new();
            let scope = crate::model::ScopeStage0 {
                models: &models,
                dimensions: &dim_context,
                model_name: "",
            };
            let lowered = crate::model::lower_variable(&scope, &parsed.variable);

            let converted_dims: Vec<crate::dimensions::Dimension> = dims
                .iter()
                .map(crate::dimensions::Dimension::from)
                .collect();

            let dt_deps = match lowered.ast() {
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, None)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };

            let initial_deps = match lowered.init_ast() {
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, None)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };

            // Extract deps for implicit variables (SMOOTH, DELAY create internal stocks)
            let implicit_vars = extract_implicit_var_deps(parsed, &dims, &dim_context);

            VariableDeps {
                dt_deps,
                initial_deps,
                implicit_vars,
            }
        }
    }
}

fn extract_implicit_var_deps(
    parsed: &ParsedVariableResult,
    dims: &[datamodel::Dimension],
    dim_context: &crate::dimensions::DimensionsContext,
) -> Vec<ImplicitVarDeps> {
    if parsed.implicit_vars.is_empty() {
        return Vec::new();
    }

    let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap_or_default();
    let converted_dims: Vec<crate::dimensions::Dimension> = dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    parsed
        .implicit_vars
        .iter()
        .map(|implicit_var| {
            let implicit_name = canonicalize(implicit_var.get_ident()).into_owned();
            let mut dummy_implicits = Vec::new();
            let parsed_implicit = crate::variable::parse_var(
                dims,
                implicit_var,
                &mut dummy_implicits,
                &units_ctx,
                |mi| Ok(Some(mi.clone())),
            );

            let models = HashMap::new();
            let scope = crate::model::ScopeStage0 {
                models: &models,
                dimensions: dim_context,
                model_name: "",
            };
            let lowered = crate::model::lower_variable(&scope, &parsed_implicit);

            let dt = match lowered.ast() {
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, None)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };
            let initial = match lowered.init_ast() {
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, None)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };

            ImplicitVarDeps {
                name: implicit_name,
                is_stock: parsed_implicit.is_stock(),
                dt_deps: dt,
                initial_deps: initial,
            }
        })
        .collect()
}

/// Result of computing a model's dependency graph. Contains the transitive
/// dependency maps and topologically sorted runlists for one instantiation
/// (the default, no-module-inputs case).
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ModelDepGraphResult {
    pub dt_dependencies: HashMap<String, BTreeSet<String>>,
    pub initial_dependencies: HashMap<String, BTreeSet<String>>,
    pub runlist_initials: Vec<String>,
    pub runlist_flows: Vec<String>,
    pub runlist_stocks: Vec<String>,
}

/// Per-model tracked function for dependency graph computation. Salsa traces
/// that this function reads each variable's direct dependencies (via
/// `variable_direct_dependencies`). If none of those dep sets change, this
/// function's result is returned from cache.
///
/// This handles models without cross-model module dependencies. Models with
/// module references use `set_dependencies_cached` which combines this
/// function's per-variable caching with the existing cross-model analysis.
#[salsa::tracked(returns(ref))]
pub fn model_dependency_graph(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> ModelDepGraphResult {
    let source_vars = model.variables(db);

    // Collect per-variable info: kind and direct deps
    struct VarInfo {
        is_stock: bool,
        is_module: bool,
        dt_deps: BTreeSet<String>,
        initial_deps: BTreeSet<String>,
    }

    let mut var_info: HashMap<String, VarInfo> = HashMap::new();

    for (name, source_var) in source_vars.iter() {
        let deps = variable_direct_dependencies(db, *source_var, project);
        // Use SourceVariableKind from the salsa input directly -- this avoids
        // creating a dependency on parse_source_variable's result (which
        // changes when equation text changes even if deps don't).
        let kind = source_var.kind(db);

        var_info.insert(
            name.clone(),
            VarInfo {
                is_stock: kind == SourceVariableKind::Stock,
                is_module: kind == SourceVariableKind::Module,
                dt_deps: deps.dt_deps.clone(),
                initial_deps: deps.initial_deps.clone(),
            },
        );

        // Include implicit variables from this variable's deps result.
        // Since we read this from variable_direct_dependencies (not
        // parse_source_variable), salsa's backdating ensures that if the
        // deps + implicit vars haven't changed, this function is cached.
        for implicit in &deps.implicit_vars {
            var_info.insert(
                implicit.name.clone(),
                VarInfo {
                    is_stock: implicit.is_stock,
                    is_module: false,
                    dt_deps: implicit.dt_deps.clone(),
                    initial_deps: implicit.initial_deps.clone(),
                },
            );
        }
    }

    // Compute transitive dependencies (simplified all_deps without cross-model support)
    let compute_transitive =
        |is_initial: bool| -> Result<HashMap<String, BTreeSet<String>>, String> {
            let mut all_deps: HashMap<String, Option<BTreeSet<String>>> =
                var_info.keys().map(|k| (k.clone(), None)).collect();
            let mut processing: BTreeSet<String> = BTreeSet::new();

            fn compute_inner(
                var_info: &HashMap<String, VarInfo>,
                all_deps: &mut HashMap<String, Option<BTreeSet<String>>>,
                processing: &mut BTreeSet<String>,
                name: &str,
                is_initial: bool,
            ) -> Result<(), String> {
                if all_deps.get(name).and_then(|d| d.as_ref()).is_some() {
                    return Ok(());
                }

                let info = match var_info.get(name) {
                    Some(info) => info,
                    None => return Ok(()), // unknown variable handled at model level
                };

                // Stocks break the dependency chain in dt phase
                if info.is_stock && !is_initial {
                    all_deps.insert(name.to_string(), Some(BTreeSet::new()));
                    return Ok(());
                }

                // Skip modules -- cross-model deps handled at the orchestrator level
                if info.is_module {
                    let direct = if is_initial {
                        &info.initial_deps
                    } else {
                        &info.dt_deps
                    };
                    all_deps.insert(name.to_string(), Some(direct.clone()));
                    return Ok(());
                }

                processing.insert(name.to_string());

                let direct = if is_initial {
                    &info.initial_deps
                } else {
                    &info.dt_deps
                };

                let mut transitive = BTreeSet::new();
                for dep in direct.iter() {
                    if !var_info.contains_key(dep.as_str()) {
                        continue; // unknown dep, skip (error reported elsewhere)
                    }

                    let dep_info = &var_info[dep.as_str()];
                    if !is_initial && dep_info.is_stock {
                        continue; // stock breaks chain in dt phase
                    }

                    transitive.insert(dep.clone());

                    if processing.contains(dep.as_str()) {
                        return Err(name.to_string()); // circular dependency
                    }

                    if all_deps
                        .get(dep.as_str())
                        .and_then(|d| d.as_ref())
                        .is_none()
                    {
                        compute_inner(var_info, all_deps, processing, dep, is_initial)?;
                    }

                    if !dep_info.is_module
                        && let Some(Some(dep_deps)) = all_deps.get(dep.as_str())
                    {
                        transitive.extend(dep_deps.iter().cloned());
                    }
                }

                processing.remove(name);
                all_deps.insert(name.to_string(), Some(transitive));
                Ok(())
            }

            let names: Vec<String> = var_info.keys().cloned().collect();
            for name in &names {
                compute_inner(&var_info, &mut all_deps, &mut processing, name, is_initial)?;
            }

            Ok(all_deps
                .into_iter()
                .map(|(k, v)| (k, v.unwrap_or_default()))
                .collect())
        };

    let dt_dependencies = compute_transitive(false).unwrap_or_else(|var_name| {
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var_name),
            error: DiagnosticError::Model(crate::common::Error {
                kind: crate::common::ErrorKind::Model,
                code: crate::common::ErrorCode::CircularDependency,
                details: None,
            }),
        })
        .accumulate(db);
        HashMap::new()
    });
    let initial_dependencies = compute_transitive(true).unwrap_or_else(|var_name| {
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var_name),
            error: DiagnosticError::Model(crate::common::Error {
                kind: crate::common::ErrorKind::Model,
                code: crate::common::ErrorCode::CircularDependency,
                details: None,
            }),
        })
        .accumulate(db);
        HashMap::new()
    });

    // Build runlists via topological sort
    let var_names: Vec<String> = {
        let mut names: Vec<String> = var_info.keys().cloned().collect();
        names.sort_unstable();
        names
    };

    let topo_sort_str =
        |names: Vec<&String>, deps: &HashMap<String, BTreeSet<String>>| -> Vec<String> {
            use std::collections::HashSet;
            let mut result: Vec<String> = Vec::new();
            let mut used: HashSet<String> = HashSet::new();

            fn add(
                deps: &HashMap<String, BTreeSet<String>>,
                result: &mut Vec<String>,
                used: &mut HashSet<String>,
                name: &str,
            ) {
                if used.contains(name) {
                    return;
                }
                used.insert(name.to_string());
                if let Some(d) = deps.get(name) {
                    for dep in d.iter() {
                        add(deps, result, used, dep);
                    }
                }
                result.push(name.to_string());
            }

            for name in names {
                add(deps, &mut result, &mut used, name);
            }
            result
        };

    // Initials runlist: stocks, modules, and their deps
    let runlist_initials = {
        use std::collections::HashSet;
        let needed: HashSet<&String> = var_names
            .iter()
            .filter(|n| {
                var_info
                    .get(n.as_str())
                    .map(|i| i.is_stock || i.is_module)
                    .unwrap_or(false)
            })
            .collect();
        let mut init_set: HashSet<&String> = needed
            .iter()
            .flat_map(|n| {
                initial_dependencies
                    .get(n.as_str())
                    .into_iter()
                    .flat_map(|deps| deps.iter())
            })
            .collect();
        init_set.extend(needed);
        let init_list: Vec<&String> = init_set.into_iter().collect();
        topo_sort_str(init_list, &initial_dependencies)
    };

    // Flows runlist: non-stock variables
    let runlist_flows = {
        let flow_names: Vec<&String> = var_names
            .iter()
            .filter(|n| {
                var_info
                    .get(n.as_str())
                    .map(|i| !i.is_stock)
                    .unwrap_or(false)
            })
            .collect();
        topo_sort_str(flow_names, &dt_dependencies)
    };

    // Stocks runlist: stocks and modules
    let runlist_stocks: Vec<String> = var_names
        .iter()
        .filter(|n| {
            var_info
                .get(n.as_str())
                .map(|i| i.is_stock || i.is_module)
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    ModelDepGraphResult {
        dt_dependencies,
        initial_dependencies,
        runlist_initials,
        runlist_flows,
        runlist_stocks,
    }
}

// ── Diagnostic collection ──────────────────────────────────────────────

/// Per-model tracked function that collects compilation diagnostics from
/// already-cached tracked functions and pushes them to the accumulator.
///
/// This runs PARALLEL to the existing error-field path: tracked functions
/// still populate struct fields for backward compatibility, but this
/// function also pushes the same errors into the salsa accumulator so
/// callers can eventually read diagnostics purely from the DB.
#[salsa::tracked]
pub fn model_all_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    let model_name = model.name(db).clone();
    let source_vars = model.variables(db);

    for (var_name, source_var) in source_vars.iter() {
        let parsed = parse_source_variable(db, *source_var, project);
        let variable = &parsed.variable;

        if let Some(errors) = variable.equation_errors() {
            for err in errors {
                CompilationDiagnostic(Diagnostic {
                    model: model_name.clone(),
                    variable: Some(var_name.clone()),
                    error: DiagnosticError::Equation(err),
                })
                .accumulate(db);
            }
        }

        if let Some(errors) = variable.unit_errors() {
            for err in errors {
                CompilationDiagnostic(Diagnostic {
                    model: model_name.clone(),
                    variable: Some(var_name.clone()),
                    error: DiagnosticError::Unit(err),
                })
                .accumulate(db);
            }
        }
    }
}

// ── LTM tracked functions ──────────────────────────────────────────────

/// Causal edge structure for a model, built from variable dependency sets
/// and structural info (stock inflows/outflows, module refs).
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct CausalEdgesResult {
    /// Adjacency list: from_var -> {to_var1, to_var2, ...}
    pub edges: HashMap<String, BTreeSet<String>>,
    /// Stock variables in the model
    pub stocks: BTreeSet<String>,
    /// Module var_name -> model_name for dynamic modules
    pub dynamic_modules: HashMap<String, String>,
}

/// Deduplicated loop circuits as node name lists.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct LoopCircuitsResult {
    pub circuits: Vec<Vec<String>>,
}

/// Stock-to-stock cycle partitions.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct CyclePartitionsResult {
    pub partitions: Vec<Vec<String>>,
    pub stock_partition: HashMap<String, usize>,
}

/// A single LTM synthetic variable definition (name + equation text).
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct LtmSyntheticVar {
    pub name: String,
    pub equation: String,
}

/// Result of LTM variable generation for a model.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct LtmVariablesResult {
    pub vars: Vec<LtmSyntheticVar>,
}

/// Strip interpunct (middot) module output qualifiers, e.g.
/// `$⁚s⁚0⁚smth1·output` → `$⁚s⁚0⁚smth1`.
fn normalize_module_ref_str(s: &str) -> String {
    if let Some(pos) = s.find('\u{00B7}') {
        s[..pos].to_string()
    } else {
        s.to_string()
    }
}

/// Construct a lightweight CausalGraph from a CausalEdgesResult.
/// Variables and module_graphs are empty -- suitable for graph algorithms
/// (circuit finding, SCC computation) but not for polarity analysis.
fn causal_graph_from_edges(result: &CausalEdgesResult) -> crate::ltm::CausalGraph {
    use crate::common::{Canonical, Ident};
    use std::collections::HashSet;

    let edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = result
        .edges
        .iter()
        .map(|(from, tos)| {
            (
                Ident::new(from),
                tos.iter().map(|t| Ident::new(t)).collect(),
            )
        })
        .collect();
    let stocks: HashSet<Ident<Canonical>> = result.stocks.iter().map(|s| Ident::new(s)).collect();

    crate::ltm::CausalGraph {
        edges,
        stocks,
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    }
}

/// Build the causal edge structure for a model from salsa-tracked
/// dependency sets and structural variable info.
///
/// Reads `variable_direct_dependencies` (establishing salsa dep on dep
/// sets) and `parse_source_variable` (for implicit variable details like
/// module input refs). Salsa backdating ensures that when equation text
/// changes without changing the resulting edge structure, the cached
/// result is reused and downstream graph algorithms are skipped.
#[salsa::tracked(returns(ref))]
pub fn model_causal_edges(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> CausalEdgesResult {
    let source_vars = model.variables(db);
    let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut stocks = BTreeSet::new();
    let mut dynamic_modules = HashMap::new();

    for (name, source_var) in source_vars.iter() {
        let kind = source_var.kind(db);

        match kind {
            SourceVariableKind::Stock => {
                stocks.insert(name.clone());
                for flow in source_var
                    .inflows(db)
                    .iter()
                    .chain(source_var.outflows(db).iter())
                {
                    let canonical_flow = canonicalize(flow).into_owned();
                    edges
                        .entry(canonical_flow)
                        .or_default()
                        .insert(name.clone());
                }
            }
            SourceVariableKind::Module => {
                for mr in source_var.module_refs(db).iter() {
                    let canonical_src = canonicalize(&mr.src).into_owned();
                    edges.entry(canonical_src).or_default().insert(name.clone());
                }
                let model_name = source_var.model_name(db);
                if !model_name.is_empty() {
                    dynamic_modules.insert(name.clone(), model_name.clone());
                }
            }
            _ => {
                let deps = variable_direct_dependencies(db, *source_var, project);
                for dep in &deps.dt_deps {
                    let normalized = normalize_module_ref_str(dep);
                    edges.entry(normalized).or_default().insert(name.clone());
                }
            }
        }

        // Include implicit variables (module instances from SMOOTH/DELAY expansion)
        let parsed = parse_source_variable(db, *source_var, project);
        for implicit_dm_var in &parsed.implicit_vars {
            let imp_name = canonicalize(implicit_dm_var.get_ident()).into_owned();

            match implicit_dm_var {
                datamodel::Variable::Stock(s) => {
                    stocks.insert(imp_name.clone());
                    for flow in s.inflows.iter().chain(s.outflows.iter()) {
                        let canonical_flow = canonicalize(flow).into_owned();
                        edges
                            .entry(canonical_flow)
                            .or_default()
                            .insert(imp_name.clone());
                    }
                }
                datamodel::Variable::Module(m) => {
                    for mr in &m.references {
                        let canonical_src = canonicalize(&mr.src).into_owned();
                        edges
                            .entry(canonical_src)
                            .or_default()
                            .insert(imp_name.clone());
                    }
                    dynamic_modules.insert(imp_name.clone(), m.model_name.clone());
                }
                _ => {
                    // For implicit flows/auxes, get deps from the parent's
                    // variable_direct_dependencies result.
                    let deps = variable_direct_dependencies(db, *source_var, project);
                    if let Some(implicit_dep) =
                        deps.implicit_vars.iter().find(|iv| iv.name == imp_name)
                    {
                        for dep in &implicit_dep.dt_deps {
                            let normalized = normalize_module_ref_str(dep);
                            edges
                                .entry(normalized)
                                .or_default()
                                .insert(imp_name.clone());
                        }
                    }
                }
            }
        }
    }

    CausalEdgesResult {
        edges,
        stocks,
        dynamic_modules,
    }
}

/// Find all elementary loop circuits in a model's causal graph.
///
/// Depends on `model_causal_edges`, so loop detection is cached when
/// the edge structure hasn't changed (even if equation text changed).
#[salsa::tracked(returns(ref))]
pub fn model_loop_circuits(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LoopCircuitsResult {
    let edges_result = model_causal_edges(db, model, project);
    let graph = causal_graph_from_edges(edges_result);
    let circuits = graph.find_circuit_node_lists();
    LoopCircuitsResult {
        circuits: circuits
            .into_iter()
            .map(|c| c.into_iter().map(|n| n.to_string()).collect())
            .collect(),
    }
}

/// Compute stock-to-stock cycle partitions (SCCs) for a model.
///
/// Depends on `model_causal_edges`, so partition computation is cached
/// when the edge structure hasn't changed.
#[salsa::tracked(returns(ref))]
pub fn model_cycle_partitions(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> CyclePartitionsResult {
    let edges_result = model_causal_edges(db, model, project);
    let graph = causal_graph_from_edges(edges_result);
    let cp = graph.compute_cycle_partitions();
    CyclePartitionsResult {
        partitions: cp
            .partitions
            .into_iter()
            .map(|p| p.into_iter().map(|s| s.to_string()).collect())
            .collect(),
        stock_partition: cp
            .stock_partition
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

/// Reconstruct `Variable` objects from salsa-tracked parse results for
/// all variables in a model (including implicit variables).
fn reconstruct_model_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<crate::common::Ident<crate::common::Canonical>, crate::variable::Variable> {
    use crate::common::{Canonical, Ident};

    let source_vars = model.variables(db);
    let dims = source_dims_to_datamodel(project.dimensions(db));
    let dim_context = crate::dimensions::DimensionsContext::from(dims.as_slice());
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_context,
        model_name: "",
    };

    let mut variables: HashMap<Ident<Canonical>, crate::variable::Variable> = HashMap::new();

    for (name, source_var) in source_vars.iter() {
        let parsed = parse_source_variable(db, *source_var, project);
        let lowered = crate::model::lower_variable(&scope, &parsed.variable);
        variables.insert(Ident::new(name), lowered);

        // Add implicit variables (module instances from SMOOTH/DELAY expansion)
        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap_or_default();
        for implicit_dm_var in &parsed.implicit_vars {
            let imp_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
            let mut dummy_implicits = Vec::new();
            let parsed_imp = crate::variable::parse_var(
                &dims,
                implicit_dm_var,
                &mut dummy_implicits,
                &units_ctx,
                |mi| Ok(Some(mi.clone())),
            );
            let lowered_imp = crate::model::lower_variable(&scope, &parsed_imp);
            variables.insert(Ident::new(&imp_name), lowered_imp);
        }
    }

    variables
}

/// Compute stdlib composite ports (cached in a process-wide OnceLock).
/// These are static properties of stdlib models and never change.
///
/// On native targets, uses a separate thread for initialization because
/// `Project::from()` creates its own salsa db, which conflicts if we're
/// inside a tracked function query on the caller's db.
/// On wasm32, threads are unavailable so we initialize directly (safe
/// because WASM is single-threaded).
fn get_stdlib_composite_ports() -> &'static crate::ltm_augment::CompositePortMap {
    use std::sync::OnceLock;
    static PORTS: OnceLock<crate::ltm_augment::CompositePortMap> = OnceLock::new();
    PORTS.get_or_init(|| {
        let compute = || {
            use crate::common::{Canonical, Ident};

            let mut models = Vec::new();
            for name in &[
                "smooth", "delay1", "delay3", "trend", "init", "previous",
            ] {
                if let Some(mut dm_model) = crate::stdlib::get(name) {
                    dm_model.name = format!("stdlib\u{205A}{name}");
                    models.push(dm_model);
                }
            }

            if models.is_empty() {
                return HashMap::<Ident<Canonical>, std::collections::HashSet<Ident<Canonical>>>::new();
            }

            let dm_project = datamodel::Project {
                name: "stdlib_composite".to_string(),
                sim_specs: datamodel::SimSpecs::default(),
                dimensions: vec![],
                units: vec![],
                models,
                source: None,
                ai_information: None,
            };

            let project = crate::project::Project::from(dm_project);
            crate::ltm_augment::compute_composite_ports(&project)
        };

        #[cfg(not(target_arch = "wasm32"))]
        {
            std::thread::spawn(compute)
                .join()
                .expect("stdlib composite ports thread panicked")
        }

        #[cfg(target_arch = "wasm32")]
        {
            compute()
        }
    })
}

/// Generate LTM synthetic variables for a user model (exhaustive mode).
///
/// Reads cached loop circuits and cycle partitions (graph algorithms
/// skipped when deps unchanged), then generates link score, loop score,
/// and relative loop score equations.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_synthetic_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LtmVariablesResult {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{CyclePartitions, Link, Loop, assign_loop_ids};
    use std::collections::HashSet;

    let circuits_result = model_loop_circuits(db, model, project);
    if circuits_result.circuits.is_empty() {
        return LtmVariablesResult { vars: vec![] };
    }

    let partitions_result = model_cycle_partitions(db, model, project);
    let edges_result = model_causal_edges(db, model, project);

    // Reconstruct Variable objects for polarity analysis + equation generation
    let variables = reconstruct_model_variables(db, model, project);

    // Build CausalGraph with variables for polarity analysis
    let edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = edges_result
        .edges
        .iter()
        .map(|(from, tos)| {
            (
                Ident::new(from),
                tos.iter().map(|t| Ident::new(t)).collect(),
            )
        })
        .collect();
    let stocks: HashSet<Ident<Canonical>> =
        edges_result.stocks.iter().map(|s| Ident::new(s)).collect();

    let graph = crate::ltm::CausalGraph {
        edges,
        stocks,
        variables: variables.clone(),
        module_graphs: HashMap::new(),
    };

    // Convert circuits to Loops with polarity
    let mut loops: Vec<Loop> = circuits_result
        .circuits
        .iter()
        .map(|circuit_strs| {
            let circuit: Vec<Ident<Canonical>> =
                circuit_strs.iter().map(|s| Ident::new(s)).collect();
            let links = graph.circuit_to_links(&circuit);
            let parent_stocks = graph.find_stocks_in_loop(&circuit);
            let polarity = graph.calculate_polarity(&links);
            Loop {
                id: String::new(),
                links,
                stocks: parent_stocks,
                polarity,
            }
        })
        .collect();

    assign_loop_ids(&mut loops);

    // Collect all links from loops
    let mut loop_links: HashSet<Link> = HashSet::new();
    for loop_item in &loops {
        for link in &loop_item.links {
            loop_links.insert(link.clone());
        }
    }

    // Reconstruct CyclePartitions from cached result
    let partitions = CyclePartitions {
        partitions: partitions_result
            .partitions
            .iter()
            .map(|p| p.iter().map(|s| Ident::new(s)).collect())
            .collect(),
        stock_partition: partitions_result
            .stock_partition
            .iter()
            .map(|(k, v)| (Ident::new(k), *v))
            .collect(),
    };

    let composite_ports = get_stdlib_composite_ports();
    let link_vars =
        crate::ltm_augment::generate_link_score_variables(&loop_links, &variables, composite_ports);
    let loop_vars = crate::ltm_augment::generate_loop_score_variables(&loops, &partitions);

    let mut vars = Vec::new();
    for (name, var) in link_vars.into_iter().chain(loop_vars.into_iter()) {
        let equation = match var.get_equation() {
            Some(datamodel::Equation::Scalar(eq)) => eq.clone(),
            _ => String::new(),
        };
        vars.push(LtmSyntheticVar {
            name: name.to_string(),
            equation,
        });
    }

    // Sort for deterministic output
    vars.sort_by(|a, b| a.name.cmp(&b.name));

    LtmVariablesResult { vars }
}

/// Generate LTM link score variables for ALL causal links (discovery mode).
///
/// Unlike `model_ltm_synthetic_variables` which only generates variables
/// for links in detected loops, this generates link scores for every
/// causal edge. No loop or relative loop scores are generated.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_all_link_synthetic_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LtmVariablesResult {
    use crate::common::{Canonical, Ident};
    use crate::ltm::Link;
    use std::collections::HashSet;

    let edges_result = model_causal_edges(db, model, project);
    let variables = reconstruct_model_variables(db, model, project);

    // Build CausalGraph for polarity analysis
    let graph_edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = edges_result
        .edges
        .iter()
        .map(|(from, tos)| {
            (
                Ident::new(from),
                tos.iter().map(|t| Ident::new(t)).collect(),
            )
        })
        .collect();
    let stocks: HashSet<Ident<Canonical>> =
        edges_result.stocks.iter().map(|s| Ident::new(s)).collect();

    let graph = crate::ltm::CausalGraph {
        edges: graph_edges,
        stocks,
        variables: variables.clone(),
        module_graphs: HashMap::new(),
    };

    let all_links: HashSet<Link> = graph.all_links().into_iter().collect();

    let composite_ports = get_stdlib_composite_ports();
    let link_vars =
        crate::ltm_augment::generate_link_score_variables(&all_links, &variables, composite_ports);

    let mut vars: Vec<LtmSyntheticVar> = link_vars
        .into_iter()
        .map(|(name, var)| {
            let equation = match var.get_equation() {
                Some(datamodel::Equation::Scalar(eq)) => eq.clone(),
                _ => String::new(),
            };
            LtmSyntheticVar {
                name: name.to_string(),
                equation,
            }
        })
        .collect();

    vars.sort_by(|a, b| a.name.cmp(&b.name));
    LtmVariablesResult { vars }
}

/// Generate internal LTM variables for a stdlib dynamic module.
///
/// Since stdlib models are static, this computes once and caches forever.
#[salsa::tracked(returns(ref))]
pub fn module_ltm_synthetic_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LtmVariablesResult {
    use crate::common::{Canonical, Ident};

    // Reconstruct the module's variables
    let variables = reconstruct_model_variables(db, model, project);

    // Check if this is a dynamic module (has stocks)
    let has_stocks = variables
        .values()
        .any(|v| matches!(v, crate::variable::Variable::Stock { .. }));
    if !has_stocks {
        return LtmVariablesResult { vars: vec![] };
    }

    // Build a ModelStage1-like structure for the module
    let edges_result = model_causal_edges(db, model, project);
    let graph_edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = edges_result
        .edges
        .iter()
        .map(|(from, tos)| {
            (
                Ident::new(from),
                tos.iter().map(|t| Ident::new(t)).collect(),
            )
        })
        .collect();
    let stocks: std::collections::HashSet<Ident<Canonical>> =
        edges_result.stocks.iter().map(|s| Ident::new(s)).collect();

    let graph = crate::ltm::CausalGraph {
        edges: graph_edges,
        stocks,
        variables: variables.clone(),
        module_graphs: HashMap::new(),
    };

    // Generate internal link scores for all causal links in the module
    let mut vars = Vec::new();
    let links: std::collections::HashSet<crate::ltm::Link> =
        graph.all_links().into_iter().collect();

    for link in &links {
        let var_name = format!(
            "$\u{205A}ltm\u{205A}ilink\u{205A}{}\u{2192}{}",
            link.from.as_str(),
            link.to.as_str()
        );
        if let Some(to_var) = variables.get(&link.to) {
            let equation = crate::ltm_augment::generate_link_score_equation_for_link(
                &link.from, &link.to, to_var, &variables,
            );
            vars.push(LtmSyntheticVar {
                name: var_name,
                equation,
            });
        }
    }

    // Enumerate pathways from input ports to output
    let output_ident = Ident::new("output");
    let pathways = graph.enumerate_module_pathways(&output_ident);

    for (input_port, port_pathways) in &pathways {
        let mut pathway_names = Vec::new();
        for (idx, pathway_links) in port_pathways.iter().enumerate() {
            let path_var_name = format!(
                "$\u{205A}ltm\u{205A}path\u{205A}{}\u{205A}{}",
                input_port.as_str(),
                idx
            );

            let link_score_refs: Vec<String> = pathway_links
                .iter()
                .map(|link| {
                    format!(
                        "\"$\u{205A}ltm\u{205A}ilink\u{205A}{}\u{2192}{}\"",
                        link.from.as_str(),
                        link.to.as_str()
                    )
                })
                .collect();

            let equation = if link_score_refs.is_empty() {
                "0".to_string()
            } else {
                link_score_refs.join(" * ")
            };

            pathway_names.push(path_var_name.clone());
            vars.push(LtmSyntheticVar {
                name: path_var_name,
                equation,
            });
        }

        // Generate composite score variable (max-magnitude pathway)
        let composite_name = format!(
            "$\u{205A}ltm\u{205A}composite\u{205A}{}",
            input_port.as_str()
        );
        let equation = generate_max_abs_chain_str(&pathway_names);
        vars.push(LtmSyntheticVar {
            name: composite_name,
            equation,
        });
    }

    vars.sort_by(|a, b| a.name.cmp(&b.name));
    LtmVariablesResult { vars }
}

/// Generate a nested max-abs selection equation from pathway variable names.
fn generate_max_abs_chain_str(pathway_names: &[String]) -> String {
    match pathway_names.len() {
        0 => "0".to_string(),
        1 => format!("\"{}\"", pathway_names[0]),
        2 => {
            let p0 = &pathway_names[0];
            let p1 = &pathway_names[1];
            format!("if ABS(\"{p0}\") >= ABS(\"{p1}\") then \"{p0}\" else \"{p1}\"")
        }
        _ => {
            let last = &pathway_names[pathway_names.len() - 1];
            let rest = generate_max_abs_chain_str(&pathway_names[..pathway_names.len() - 1]);
            format!("if ABS(\"{last}\") >= ABS(({rest})) then \"{last}\" else ({rest})")
        }
    }
}

// ── Diagnostic collection helpers ──────────────────────────────────────

/// Collect all `CompilationDiagnostic`s accumulated during
/// `model_all_diagnostics` for a single model.
pub fn collect_model_diagnostics(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Vec<Diagnostic> {
    model_all_diagnostics::accumulated::<CompilationDiagnostic>(db, model, project)
        .into_iter()
        .map(|cd| cd.0.clone())
        .collect()
}

/// Collect all diagnostics for every model in a synced project.
pub fn collect_all_diagnostics(db: &SimlinDb, sync: &SyncResult<'_>) -> Vec<Diagnostic> {
    let mut all = Vec::new();
    for synced_model in sync.models.values() {
        let diags = collect_model_diagnostics(db, synced_model.source, sync.project);
        all.extend(diags);
    }
    all
}

// ── Sync result ────────────────────────────────────────────────────────

/// Result of syncing a datamodel::Project into the salsa database.
/// Maps names to their salsa input/interned IDs for subsequent lookups.
pub struct SyncResult<'db> {
    pub project: SourceProject,
    pub models: HashMap<String, SyncedModel<'db>>,
}

pub struct SyncedModel<'db> {
    pub id: ModelId<'db>,
    pub source: SourceModel,
    pub variables: HashMap<String, SyncedVariable<'db>>,
}

pub struct SyncedVariable<'db> {
    pub id: VariableId<'db>,
    pub source: SourceVariable,
}

// ── Persistent sync state ──────────────────────────────────────────────
//
// Lifetime-erased versions of SyncResult handles, safe to store across
// salsa revisions within the same database instance.

/// Stores salsa input handles between sync calls so that
/// `sync_from_datamodel_incremental` can reuse them instead of
/// creating fresh inputs (which would invalidate all cached queries).
pub struct PersistentSyncState {
    pub project: SourceProject,
    pub models: HashMap<String, PersistentModelState>,
}

pub struct PersistentModelState {
    /// Lifetime-erased `ModelId<'db>` (interned, carries `'db`)
    pub model_interned_id: salsa::Id,
    pub source_model: SourceModel,
    pub variables: HashMap<String, PersistentVariableState>,
}

pub struct PersistentVariableState {
    /// Lifetime-erased `VariableId<'db>` (interned, carries `'db`)
    pub var_interned_id: salsa::Id,
    pub source_var: SourceVariable,
}

impl PersistentSyncState {
    /// Reconstitute a `SyncResult` from the stored handles.
    ///
    /// The returned `SyncResult` borrows the interned `ModelId`/`VariableId`
    /// handles from the database, so the `'db` lifetime is tied to the
    /// database reference used when interning.
    pub fn to_sync_result(&self) -> SyncResult<'_> {
        use salsa::plumbing::FromId;
        SyncResult {
            project: self.project,
            models: self
                .models
                .iter()
                .map(|(name, pm)| {
                    let variables = pm
                        .variables
                        .iter()
                        .map(|(vname, pv)| {
                            (
                                vname.clone(),
                                SyncedVariable {
                                    id: VariableId::from_id(pv.var_interned_id),
                                    source: pv.source_var,
                                },
                            )
                        })
                        .collect();
                    (
                        name.clone(),
                        SyncedModel {
                            id: ModelId::from_id(pm.model_interned_id),
                            source: pm.source_model,
                            variables,
                        },
                    )
                })
                .collect(),
        }
    }

    fn from_sync_result(sync: &SyncResult<'_>) -> Self {
        PersistentSyncState {
            project: sync.project,
            models: sync
                .models
                .iter()
                .map(|(name, sm)| {
                    let variables = sm
                        .variables
                        .iter()
                        .map(|(vname, sv)| {
                            (
                                vname.clone(),
                                PersistentVariableState {
                                    var_interned_id: sv.id.as_id(),
                                    source_var: sv.source,
                                },
                            )
                        })
                        .collect();
                    (
                        name.clone(),
                        PersistentModelState {
                            model_interned_id: sm.id.as_id(),
                            source_model: sm.source,
                            variables,
                        },
                    )
                })
                .collect(),
        }
    }
}

// ── Sync function ──────────────────────────────────────────────────────

/// Populate salsa inputs from a `datamodel::Project`.
///
/// Creates `SourceProject`, `SourceModel`, and `SourceVariable` inputs in
/// the database, along with interned `ModelId` and `VariableId` identifiers.
pub fn sync_from_datamodel<'db>(
    db: &'db SimlinDb,
    project: &datamodel::Project,
) -> SyncResult<'db> {
    let model_names: Vec<String> = project.models.iter().map(|m| m.name.clone()).collect();

    let mut models = HashMap::new();
    let mut source_model_map: HashMap<String, SourceModel> = HashMap::new();

    for dm_model in &project.models {
        let canonical_model_name = canonicalize(&dm_model.name).into_owned();
        let model_id = ModelId::new(db, canonical_model_name.clone());

        let mut variables = HashMap::new();
        let mut source_var_map = HashMap::new();

        for dm_var in &dm_model.variables {
            let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();
            let var_id = VariableId::new(db, canonical_var_name.clone());

            let source_var = source_variable_from_datamodel(db, dm_var);
            source_var_map.insert(canonical_var_name.clone(), source_var);

            variables.insert(
                canonical_var_name,
                SyncedVariable {
                    id: var_id,
                    source: source_var,
                },
            );
        }

        // variable_names must use canonical names to match source_var_map keys
        let variable_names: Vec<String> = source_var_map.keys().cloned().collect();

        let source_model =
            SourceModel::new(db, dm_model.name.clone(), variable_names, source_var_map);

        source_model_map.insert(canonical_model_name.clone(), source_model);

        models.insert(
            canonical_model_name,
            SyncedModel {
                id: model_id,
                source: source_model,
                variables,
            },
        );
    }

    let source_project = SourceProject::new(
        db,
        project.name.clone(),
        SourceSimSpecs::from(&project.sim_specs),
        project
            .dimensions
            .iter()
            .map(SourceDimension::from)
            .collect(),
        project.units.iter().map(SourceUnit::from).collect(),
        model_names,
        source_model_map,
    );

    SyncResult {
        project: source_project,
        models,
    }
}

fn source_variable_from_datamodel(db: &SimlinDb, var: &datamodel::Variable) -> SourceVariable {
    let ident = var.get_ident().to_string();
    let kind = SourceVariableKind::from_datamodel_variable(var);

    let equation = var
        .get_equation()
        .map(SourceEquation::from)
        .unwrap_or_else(|| SourceEquation::Scalar(String::new()));

    let units = var.get_units().cloned();

    let gf = match var {
        datamodel::Variable::Flow(f) => f.gf.as_ref().map(SourceGraphicalFunction::from),
        datamodel::Variable::Aux(a) => a.gf.as_ref().map(SourceGraphicalFunction::from),
        _ => None,
    };

    let inflows = match var {
        datamodel::Variable::Stock(s) => s.inflows.clone(),
        _ => Vec::new(),
    };

    let outflows = match var {
        datamodel::Variable::Stock(s) => s.outflows.clone(),
        _ => Vec::new(),
    };

    let (module_refs, referenced_model_name) = match var {
        datamodel::Variable::Module(m) => (
            m.references
                .iter()
                .map(SourceModuleReference::from)
                .collect(),
            m.model_name.clone(),
        ),
        _ => (Vec::new(), String::new()),
    };

    let non_negative = match var {
        datamodel::Variable::Stock(s) => s.non_negative,
        datamodel::Variable::Flow(f) => f.non_negative,
        _ => false,
    };

    let can_be_module_input = var.can_be_module_input();

    let compat = match var {
        datamodel::Variable::Stock(s) => s.compat.clone(),
        datamodel::Variable::Flow(f) => f.compat.clone(),
        datamodel::Variable::Aux(a) => a.compat.clone(),
        datamodel::Variable::Module(_) => datamodel::Compat::default(),
    };

    SourceVariable::new(
        db,
        ident,
        equation,
        kind,
        units,
        gf,
        inflows,
        outflows,
        module_refs,
        referenced_model_name,
        non_negative,
        can_be_module_input,
        compat,
    )
}

// ── Incremental sync ───────────────────────────────────────────────────

/// Update a single `SourceVariable`'s fields via salsa setters, only
/// touching fields whose values actually changed.
fn update_source_variable(
    db: &mut SimlinDb,
    source_var: SourceVariable,
    dm_var: &datamodel::Variable,
) {
    use salsa::Setter;

    let new_ident = dm_var.get_ident().to_string();
    if *source_var.ident(&*db) != new_ident {
        source_var.set_ident(db).to(new_ident);
    }

    let new_equation = dm_var
        .get_equation()
        .map(SourceEquation::from)
        .unwrap_or_else(|| SourceEquation::Scalar(String::new()));
    if *source_var.equation(&*db) != new_equation {
        source_var.set_equation(db).to(new_equation);
    }

    let new_kind = SourceVariableKind::from_datamodel_variable(dm_var);
    if source_var.kind(&*db) != new_kind {
        source_var.set_kind(db).to(new_kind);
    }

    let new_units = dm_var.get_units().cloned();
    if *source_var.units(&*db) != new_units {
        source_var.set_units(db).to(new_units);
    }

    let new_gf = match dm_var {
        datamodel::Variable::Flow(f) => f.gf.as_ref().map(SourceGraphicalFunction::from),
        datamodel::Variable::Aux(a) => a.gf.as_ref().map(SourceGraphicalFunction::from),
        _ => None,
    };
    if *source_var.gf(&*db) != new_gf {
        source_var.set_gf(db).to(new_gf);
    }

    let new_inflows = match dm_var {
        datamodel::Variable::Stock(s) => s.inflows.clone(),
        _ => Vec::new(),
    };
    if *source_var.inflows(&*db) != new_inflows {
        source_var.set_inflows(db).to(new_inflows);
    }

    let new_outflows = match dm_var {
        datamodel::Variable::Stock(s) => s.outflows.clone(),
        _ => Vec::new(),
    };
    if *source_var.outflows(&*db) != new_outflows {
        source_var.set_outflows(db).to(new_outflows);
    }

    let (new_module_refs, new_model_name) = match dm_var {
        datamodel::Variable::Module(m) => (
            m.references
                .iter()
                .map(SourceModuleReference::from)
                .collect(),
            m.model_name.clone(),
        ),
        _ => (Vec::new(), String::new()),
    };
    if *source_var.module_refs(&*db) != new_module_refs {
        source_var.set_module_refs(db).to(new_module_refs);
    }
    if *source_var.model_name(&*db) != new_model_name {
        source_var.set_model_name(db).to(new_model_name);
    }

    let new_non_negative = match dm_var {
        datamodel::Variable::Stock(s) => s.non_negative,
        datamodel::Variable::Flow(f) => f.non_negative,
        _ => false,
    };
    if source_var.non_negative(&*db) != new_non_negative {
        source_var.set_non_negative(db).to(new_non_negative);
    }

    let new_can_be_module_input = dm_var.can_be_module_input();
    if source_var.can_be_module_input(&*db) != new_can_be_module_input {
        source_var
            .set_can_be_module_input(db)
            .to(new_can_be_module_input);
    }

    let new_compat = match dm_var {
        datamodel::Variable::Stock(s) => s.compat.clone(),
        datamodel::Variable::Flow(f) => f.compat.clone(),
        datamodel::Variable::Aux(a) => a.compat.clone(),
        datamodel::Variable::Module(_) => datamodel::Compat::default(),
    };
    if *source_var.compat(&*db) != new_compat {
        source_var.set_compat(db).to(new_compat);
    }
}

/// Incrementally sync a `datamodel::Project` into an existing salsa
/// database, reusing previous input handles to preserve cached queries.
///
/// When `prev_state` is `None`, behaves like a fresh sync (creating all
/// inputs from scratch). When `Some`, reconstitutes existing handles
/// and uses salsa setters to update only changed fields, so that
/// downstream tracked functions for unchanged variables stay cached.
pub fn sync_from_datamodel_incremental(
    db: &mut SimlinDb,
    project: &datamodel::Project,
    prev_state: Option<&PersistentSyncState>,
) -> PersistentSyncState {
    use salsa::Setter;

    let prev = match prev_state {
        None => {
            let sync = sync_from_datamodel(db, project);
            return PersistentSyncState::from_sync_result(&sync);
        }
        Some(prev) => prev,
    };

    let source_project = prev.project;

    // Update SourceProject fields
    let new_name = project.name.clone();
    if *source_project.name(&*db) != new_name {
        source_project.set_name(db).to(new_name);
    }

    let new_sim_specs = SourceSimSpecs::from(&project.sim_specs);
    if *source_project.sim_specs(&*db) != new_sim_specs {
        source_project.set_sim_specs(db).to(new_sim_specs);
    }

    let new_dims: Vec<SourceDimension> = project
        .dimensions
        .iter()
        .map(SourceDimension::from)
        .collect();
    if *source_project.dimensions(&*db) != new_dims {
        source_project.set_dimensions(db).to(new_dims);
    }

    let new_units: Vec<SourceUnit> = project.units.iter().map(SourceUnit::from).collect();
    if *source_project.units(&*db) != new_units {
        source_project.set_units(db).to(new_units);
    }

    let new_model_names: Vec<String> = project.models.iter().map(|m| m.name.clone()).collect();
    if *source_project.model_names(&*db) != new_model_names {
        source_project.set_model_names(db).to(new_model_names);
    }

    // Process models
    let mut new_models = HashMap::new();

    for dm_model in &project.models {
        let canonical_model_name = canonicalize(&dm_model.name).into_owned();

        if let Some(prev_model) = prev.models.get(&canonical_model_name) {
            // Existing model: update via setters
            let source_model = prev_model.source_model;

            if *source_model.name(&*db) != dm_model.name {
                source_model.set_name(db).to(dm_model.name.clone());
            }

            // Process variables
            let mut new_vars = HashMap::new();
            let mut source_var_map = HashMap::new();

            for dm_var in &dm_model.variables {
                let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();

                if let Some(prev_var) = prev_model.variables.get(&canonical_var_name) {
                    let source_var = prev_var.source_var;
                    update_source_variable(db, source_var, dm_var);
                    source_var_map.insert(canonical_var_name.clone(), source_var);

                    new_vars.insert(
                        canonical_var_name,
                        PersistentVariableState {
                            var_interned_id: prev_var.var_interned_id,
                            source_var,
                        },
                    );
                } else {
                    // New variable
                    let var_id = VariableId::new(&*db, canonical_var_name.clone());
                    let source_var = source_variable_from_datamodel(&*db, dm_var);
                    source_var_map.insert(canonical_var_name.clone(), source_var);

                    new_vars.insert(
                        canonical_var_name,
                        PersistentVariableState {
                            var_interned_id: var_id.as_id(),
                            source_var,
                        },
                    );
                }
            }

            // variable_names must use canonical names to match source_var_map keys
            let variable_names: Vec<String> = source_var_map.keys().cloned().collect();

            // Update model's variable lists if they changed
            if *source_model.variable_names(&*db) != variable_names {
                source_model.set_variable_names(db).to(variable_names);
            }
            if *source_model.variables(&*db) != source_var_map {
                source_model.set_variables(db).to(source_var_map);
            }

            new_models.insert(
                canonical_model_name,
                PersistentModelState {
                    model_interned_id: prev_model.model_interned_id,
                    source_model,
                    variables: new_vars,
                },
            );
        } else {
            // New model: create fresh
            let model_id = ModelId::new(&*db, canonical_model_name.clone());

            let mut new_vars = HashMap::new();
            let mut source_var_map = HashMap::new();

            for dm_var in &dm_model.variables {
                let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();
                let var_id = VariableId::new(&*db, canonical_var_name.clone());
                let source_var = source_variable_from_datamodel(&*db, dm_var);
                source_var_map.insert(canonical_var_name.clone(), source_var);

                new_vars.insert(
                    canonical_var_name,
                    PersistentVariableState {
                        var_interned_id: var_id.as_id(),
                        source_var,
                    },
                );
            }

            // variable_names must use canonical names to match source_var_map keys
            let variable_names: Vec<String> = source_var_map.keys().cloned().collect();

            let source_model =
                SourceModel::new(&*db, dm_model.name.clone(), variable_names, source_var_map);

            new_models.insert(
                canonical_model_name,
                PersistentModelState {
                    model_interned_id: model_id.as_id(),
                    source_model,
                    variables: new_vars,
                },
            );
        }
    }

    // Update the project's models map
    let new_source_model_map: HashMap<String, SourceModel> = new_models
        .iter()
        .map(|(name, pm)| (name.clone(), pm.source_model))
        .collect();
    if *source_project.models(&*db) != new_source_model_map {
        source_project.set_models(db).to(new_source_model_map);
    }

    PersistentSyncState {
        project: source_project,
        models: new_models,
    }
}

// ── Incremental compilation pipeline ───────────────────────────────────
//
// Steps 3-9: per-variable compilation, layout, assembly, and entry point.

/// Returns the variable's array dimensions (empty for scalars).
/// Depends on the equation FORM (Scalar/ApplyToAll/Arrayed) and project
/// dimensions, but NOT on the equation text content. Salsa backdates when
/// text changes don't affect dimensionality.
#[salsa::tracked(returns(ref))]
pub fn variable_dimensions(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> Vec<crate::dimensions::Dimension> {
    let parsed = parse_source_variable(db, var, project);
    match parsed.variable.get_dimensions() {
        Some(dims) => dims.to_vec(),
        None => Vec::new(),
    }
}

/// Returns the number of memory slots this variable occupies.
/// 1 for scalars, product of dimension sizes for arrays.
#[salsa::tracked]
pub fn variable_size(db: &dyn Db, var: SourceVariable, project: SourceProject) -> usize {
    let dims = variable_dimensions(db, var, project);
    if dims.is_empty() {
        1
    } else {
        dims.iter().map(|d| d.len()).product()
    }
}

/// Compute a VariableLayout for a model: the concrete offset assignment.
///
/// Depends on:
/// - model.variable_names (the set of variables)
/// - variable_size for each variable (slot counts)
/// - Sub-model layouts for module variables
///
/// Does NOT depend on equation text. Equation edits (same dims) do not
/// invalidate this. Variable add/remove DOES invalidate it.
#[salsa::tracked(returns(ref))]
pub fn compute_layout(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
) -> crate::compiler::symbolic::VariableLayout {
    use crate::compiler::symbolic::{LayoutEntry, VariableLayout};

    let source_vars = model.variables(db);
    let var_names = model.variable_names(db);

    let mut sorted_names: Vec<&String> = var_names.iter().collect();
    sorted_names.sort_unstable();

    let mut entries = HashMap::new();
    let mut offset = if is_root {
        // Implicit vars: time, dt, initial_time, final_time
        entries.insert("time".to_string(), LayoutEntry { offset: 0, size: 1 });
        entries.insert("dt".to_string(), LayoutEntry { offset: 1, size: 1 });
        entries.insert(
            "initial_time".to_string(),
            LayoutEntry { offset: 2, size: 1 },
        );
        entries.insert("final_time".to_string(), LayoutEntry { offset: 3, size: 1 });
        crate::vm::IMPLICIT_VAR_COUNT
    } else {
        0
    };

    let project_models = project.models(db);

    for name in &sorted_names {
        let size = if let Some(svar) = source_vars.get(name.as_str()) {
            if svar.kind(db) == SourceVariableKind::Module {
                // Module variables occupy the sub-model's total n_slots
                let sub_model_name = canonicalize(svar.model_name(db));
                if let Some(sub_model) = project_models.get(sub_model_name.as_ref()) {
                    let sub_layout = compute_layout(db, *sub_model, project, false);
                    sub_layout.n_slots
                } else {
                    1
                }
            } else {
                variable_size(db, *svar, project)
            }
        } else {
            1
        };

        entries.insert(name.to_string(), LayoutEntry { offset, size });
        offset += size;
    }

    VariableLayout::new(entries, offset)
}

/// Build a dimension-only stub Variable for use in a minimal compilation
/// context. Only get_dimensions() is called on these by Context.
fn build_stub_variable(
    db: &dyn Db,
    source_var: &SourceVariable,
    ident: &Ident<Canonical>,
    dims: &[crate::dimensions::Dimension],
) -> crate::variable::Variable {
    let dummy_ast = if dims.is_empty() {
        None
    } else {
        Some(crate::ast::Ast::ApplyToAll(
            dims.to_vec(),
            crate::ast::Expr2::Const("0".to_string(), 0.0, crate::ast::Loc::default()),
        ))
    };

    match source_var.kind(db) {
        SourceVariableKind::Stock => crate::variable::Variable::Stock {
            ident: ident.clone(),
            init_ast: dummy_ast,
            eqn: None,
            units: None,
            inflows: vec![],
            outflows: vec![],
            non_negative: false,
            errors: vec![],
            unit_errors: vec![],
        },
        SourceVariableKind::Module => crate::variable::Variable::Module {
            ident: ident.clone(),
            model_name: Ident::new(source_var.model_name(db)),
            units: None,
            inputs: vec![],
            errors: vec![],
            unit_errors: vec![],
        },
        _ => crate::variable::Variable::Var {
            ident: ident.clone(),
            ast: dummy_ast,
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: source_var.kind(db) == SourceVariableKind::Flow,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        },
    }
}

/// Result of per-variable compilation: symbolic bytecodes for each phase.
#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub(crate) struct VarFragmentResult {
    pub fragment: crate::compiler::symbolic::CompiledVarFragment,
}

/// Per-variable tracked compilation function. Compiles a SINGLE variable
/// to symbolic (layout-independent) bytecodes.
///
/// Builds a minimal context containing only the compiled variable and its
/// direct references, then compiles through the existing Module/Compiler
/// pipeline, and symbolizes the result.
///
/// Dependencies (salsa tracks these):
/// - parse_source_variable(var) -- this variable's equation
/// - variable_direct_dependencies(var) -- this variable's dep set
/// - variable_dimensions(Y) -- each referenced var's dimensions
/// - variable_size(Y) -- each referenced var's size
/// - project.dimensions -- for array compilation
///
/// Does NOT depend on model.variable_names or any model-wide data.
#[salsa::tracked(returns(ref))]
pub fn compile_var_fragment(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{
        CompiledVarFragment, PerVarBytecodes, ReverseOffsetMap, VariableLayout,
    };

    let var_ident = var.ident(db).clone();
    let parsed = parse_source_variable(db, var, project);

    // Check for parse errors
    if parsed
        .variable
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }

    let deps = variable_direct_dependencies(db, var, project);

    // Get project dimensions and build dimension context
    let dm_dims = source_dims_to_datamodel(project.dimensions(db));
    let dim_context = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());
    let converted_dims: Vec<crate::dimensions::Dimension> = dm_dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    // Lower the variable for compilation
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_context,
        model_name: "",
    };
    let lowered = crate::model::lower_variable(&scope, &parsed.variable);

    // Build minimal metadata: only {self} + deps
    let model_name_ident = Ident::new(model.name(db));
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_ident);
    let var_size = variable_size(db, var, project);

    // Assign sequential offsets for the minimal context
    let mut mini_metadata: HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>> =
        HashMap::new();
    let mut mini_offset = if is_root {
        crate::vm::IMPLICIT_VAR_COUNT
    } else {
        0
    };

    // Add implicit vars if root
    if is_root {
        use std::sync::LazyLock;
        static IMPLICIT_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_DT: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("dt"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_INITIAL_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("initial_time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        static IMPLICIT_FINAL_TIME: LazyLock<crate::variable::Variable> =
            LazyLock::new(|| crate::variable::Variable::Var {
                ident: Ident::new("final_time"),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            });
        mini_metadata.insert(
            Ident::new("time"),
            crate::compiler::VariableMetadata {
                offset: 0,
                size: 1,
                var: &IMPLICIT_TIME,
            },
        );
        mini_metadata.insert(
            Ident::new("dt"),
            crate::compiler::VariableMetadata {
                offset: 1,
                size: 1,
                var: &IMPLICIT_DT,
            },
        );
        mini_metadata.insert(
            Ident::new("initial_time"),
            crate::compiler::VariableMetadata {
                offset: 2,
                size: 1,
                var: &IMPLICIT_INITIAL_TIME,
            },
        );
        mini_metadata.insert(
            Ident::new("final_time"),
            crate::compiler::VariableMetadata {
                offset: 3,
                size: 1,
                var: &IMPLICIT_FINAL_TIME,
            },
        );
    }

    // Add self
    mini_metadata.insert(
        var_ident_canonical.clone(),
        crate::compiler::VariableMetadata {
            offset: mini_offset,
            size: var_size,
            var: &lowered,
        },
    );
    mini_offset += var_size;

    // Collect all dep names from both dt and initial deps
    let all_dep_names: BTreeSet<&String> = deps
        .dt_deps
        .iter()
        .chain(deps.initial_deps.iter())
        .collect();

    // For each dep, build a dimension-only Variable for context.
    // We need these to live long enough for the metadata references.
    let source_vars = model.variables(db);
    let mut dep_variables: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> = Vec::new();

    // Also add inflows/outflows for stocks (needed by stock update expressions)
    let mut extra_dep_names: Vec<String> = Vec::new();
    if var.kind(db) == SourceVariableKind::Stock {
        for flow_name in var.inflows(db).iter().chain(var.outflows(db).iter()) {
            let canonical = canonicalize(flow_name).into_owned();
            if !all_dep_names.contains(&canonical) {
                extra_dep_names.push(canonical);
            }
        }
    }

    let all_names: Vec<&String> = all_dep_names
        .iter()
        .copied()
        .chain(extra_dep_names.iter())
        .collect();

    for dep_name in &all_names {
        // Skip self and implicit vars
        if dep_name.as_str() == var_ident.as_str()
            || matches!(
                dep_name.as_str(),
                "time" | "dt" | "initial_time" | "final_time"
            )
        {
            continue;
        }

        let dep_ident = Ident::new(dep_name.as_str());
        if mini_metadata.contains_key(&dep_ident) {
            continue;
        }

        if let Some(dep_source_var) = source_vars.get(dep_name.as_str()) {
            let dep_dims = variable_dimensions(db, *dep_source_var, project);
            let dep_size = variable_size(db, *dep_source_var, project);

            let dep_var = build_stub_variable(db, dep_source_var, &dep_ident, dep_dims);

            dep_variables.push((dep_ident, dep_var, dep_size));
        }
    }

    // Add dep metadata referencing the stored dep_variables
    for (dep_ident, dep_var, dep_size) in &dep_variables {
        if !mini_metadata.contains_key(dep_ident) {
            mini_metadata.insert(
                dep_ident.clone(),
                crate::compiler::VariableMetadata {
                    offset: mini_offset,
                    size: *dep_size,
                    var: dep_var,
                },
            );
            mini_offset += dep_size;
        }
    }

    // Build the all_metadata map (model_name -> var_name -> metadata)
    let mut all_metadata: HashMap<
        Ident<Canonical>,
        HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>>,
    > = HashMap::new();
    all_metadata.insert(model_name_ident.clone(), mini_metadata);

    // Build the mini VariableLayout for symbolization
    let mini_layout =
        crate::compiler::symbolic::layout_from_metadata(&all_metadata, &model_name_ident)
            .unwrap_or_else(|_| VariableLayout::new(HashMap::new(), 0));
    let rmap = ReverseOffsetMap::from_layout(&mini_layout);

    // Build tables for compilation
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    {
        let gf_tables = lowered.tables();
        if !gf_tables.is_empty() {
            let table_results: Vec<crate::compiler::Table> = gf_tables
                .iter()
                .filter_map(|t| crate::compiler::Table::new(&var_ident, t).ok())
                .collect();
            if !table_results.is_empty() {
                tables.insert(var_ident_canonical.clone(), table_results);
            }
        }
    }

    // Build the minimal Module
    let inputs = BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();

    // Determine which runlists this variable belongs to
    let dep_graph = model_dependency_graph(db, model, project);
    let is_stock = var.kind(db) == SourceVariableKind::Stock;
    let is_module = var.kind(db) == SourceVariableKind::Module;

    // We need module_refs for module variables
    let module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = if is_module {
        let input_set: BTreeSet<Ident<Canonical>> = var
            .module_refs(db)
            .iter()
            .map(|mr| Ident::new(canonicalize(&mr.dst).as_ref()))
            .collect();
        let mut refs = HashMap::new();
        refs.insert(
            var_ident_canonical.clone(),
            (Ident::new(var.model_name(db)), input_set),
        );
        refs
    } else {
        HashMap::new()
    };

    // Build Var for each phase this variable participates in
    let core = crate::compiler::ContextCore {
        dimensions: &converted_dims,
        dimensions_ctx: &dim_context,
        model_name: &model_name_ident,
        metadata: &all_metadata,
        module_models: &module_models,
        inputs: &inputs,
    };

    let build_var = |is_initial: bool| {
        crate::compiler::Var::new(
            &crate::compiler::Context::new(core, &var_ident_canonical, is_initial),
            &lowered,
        )
    };

    // Compile for each phase and symbolize
    let compile_phase = |exprs: &[crate::compiler::Expr]| -> Option<PerVarBytecodes> {
        if exprs.is_empty() {
            return None;
        }

        // Build a minimal Module for this phase
        let runlist_initials_by_var = vec![];
        let module = crate::compiler::Module {
            ident: model_name_ident.clone(),
            inputs: HashSet::new(),
            n_slots: mini_offset,
            n_temps: 0,
            temp_sizes: vec![],
            runlist_initials: vec![],
            runlist_initials_by_var,
            runlist_flows: exprs.to_vec(),
            runlist_stocks: vec![],
            offsets: all_metadata
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.iter()
                            .map(|(vk, vm)| (vk.clone(), (vm.offset, vm.size)))
                            .collect(),
                    )
                })
                .collect(),
            runlist_order: vec![var_ident_canonical.clone()],
            tables: tables.clone(),
            dimensions: converted_dims.clone(),
            dimensions_ctx: dim_context.clone(),
            module_refs: module_refs.clone(),
        };

        // Extract temp sizes from expressions
        let mut temp_sizes_map: HashMap<u32, usize> = HashMap::new();
        for expr in exprs {
            crate::compiler::extract_temp_sizes_pub(expr, &mut temp_sizes_map);
        }
        let n_temps = temp_sizes_map.len();
        let mut temp_sizes: Vec<usize> = vec![0; n_temps];
        for (id, size) in &temp_sizes_map {
            if (*id as usize) < temp_sizes.len() {
                temp_sizes[*id as usize] = *size;
            }
        }

        // Update Module with temp info
        let module = crate::compiler::Module {
            n_temps,
            temp_sizes: temp_sizes.clone(),
            ..module
        };

        match module.compile() {
            Ok(compiled) => {
                // Symbolize the flows bytecode (we put everything in flows)
                let sym_bc =
                    crate::compiler::symbolic::symbolize_bytecode(&compiled.compiled_flows, &rmap)
                        .ok()?;

                let ctx = &*compiled.context;
                let sym_views: Vec<_> = ctx
                    .static_views
                    .iter()
                    .filter_map(|sv| {
                        crate::compiler::symbolic::symbolize_static_view(sv, &rmap).ok()
                    })
                    .collect();
                let sym_mods: Vec<_> = ctx
                    .modules
                    .iter()
                    .filter_map(|md| {
                        crate::compiler::symbolic::symbolize_module_decl(md, &rmap).ok()
                    })
                    .collect();

                let temp_sizes_vec: Vec<(u32, usize)> =
                    temp_sizes_map.iter().map(|(&k, &v)| (k, v)).collect();

                let dim_lists: Vec<Vec<u16>> = ctx
                    .dim_lists
                    .iter()
                    .map(|(n, arr)| arr[..(*n as usize)].to_vec())
                    .collect();

                Some(PerVarBytecodes {
                    symbolic: sym_bc,
                    graphical_functions: ctx.graphical_functions.clone(),
                    module_decls: sym_mods,
                    static_views: sym_views,
                    temp_sizes: temp_sizes_vec,
                    dim_lists,
                })
            }
            Err(_) => None,
        }
    };

    // Runlists use canonical names, so compare with the canonical form.
    let var_ident_str = var_ident_canonical.as_str().to_string();

    // Initial phase: stocks and their deps get compiled with is_initial=true
    let initial_bytecodes = if dep_graph.runlist_initials.contains(&var_ident_str) {
        match build_var(true) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(_) => None,
        }
    } else {
        None
    };

    // Flow phase: non-stock vars get compiled with is_initial=false
    let flow_bytecodes = if !is_stock && dep_graph.runlist_flows.contains(&var_ident_str) {
        match build_var(false) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(_) => None,
        }
    } else {
        None
    };

    // Stock phase: stocks get compiled with is_initial=false for updates
    let stock_bytecodes = if is_stock && dep_graph.runlist_stocks.contains(&var_ident_str) {
        match build_var(false) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(_) => None,
        }
    } else {
        None
    };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: var_ident,
            initial_bytecodes,
            flow_bytecodes,
            stock_bytecodes,
        },
    })
}

/// Assemble a complete CompiledModule from per-variable fragments.
///
/// NOT a tracked function -- the caching happens at the per-variable level
/// (compile_var_fragment, compute_layout, model_dependency_graph). This
/// function reads cached results and concatenates them.
pub fn assemble_module(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
) -> Result<crate::bytecode::CompiledModule, String> {
    use crate::compiler::symbolic::{
        SymbolicCompiledInitial, SymbolicCompiledModule, concatenate_fragments, resolve_module,
    };

    let dep_graph = model_dependency_graph(db, model, project);
    let layout = compute_layout(db, model, project, is_root);
    let source_vars = model.variables(db);
    let model_name = model.name(db).clone();

    // Collect fragments for each phase
    let mut initial_frags: Vec<(String, &crate::compiler::symbolic::PerVarBytecodes)> = Vec::new();
    let mut flow_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();
    let mut stock_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();

    // Process variables in runlist order
    for var_name in &dep_graph.runlist_initials {
        if let Some(svar) = source_vars.get(var_name.as_str())
            && let Some(result) = compile_var_fragment(db, *svar, model, project, is_root)
            && let Some(ref bc) = result.fragment.initial_bytecodes
        {
            initial_frags.push((var_name.clone(), bc));
        }
    }

    for var_name in &dep_graph.runlist_flows {
        if let Some(svar) = source_vars.get(var_name.as_str())
            && let Some(result) = compile_var_fragment(db, *svar, model, project, is_root)
            && let Some(ref bc) = result.fragment.flow_bytecodes
        {
            flow_frags.push(bc);
        }
    }

    for var_name in &dep_graph.runlist_stocks {
        if let Some(svar) = source_vars.get(var_name.as_str())
            && let Some(result) = compile_var_fragment(db, *svar, model, project, is_root)
            && let Some(ref bc) = result.fragment.stock_bytecodes
        {
            stock_frags.push(bc);
        }
    }

    // Concatenate and resolve each phase
    let flows_concat = concatenate_fragments(&flow_frags);
    let stocks_concat = concatenate_fragments(&stock_frags);

    // Build per-variable initials (each variable gets its own bytecode)
    let initial_refs: Vec<&crate::compiler::symbolic::PerVarBytecodes> =
        initial_frags.iter().map(|(_, bc)| *bc).collect();
    let _initials_concat = concatenate_fragments(&initial_refs);

    // Build SymbolicCompiledInitial for each initial variable
    let mut compiled_initials: Vec<SymbolicCompiledInitial> = Vec::new();
    for (name, bc) in &initial_frags {
        compiled_initials.push(SymbolicCompiledInitial {
            ident: Ident::new(name),
            bytecode: bc.symbolic.clone(),
        });
    }

    // Merge all context data from all phases
    let all_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = initial_frags
        .iter()
        .map(|(_, bc)| *bc)
        .chain(flow_frags.iter().copied())
        .chain(stock_frags.iter().copied())
        .collect();
    let merged = concatenate_fragments(&all_frags);

    // Build dimension metadata from project dimensions (mirrors Compiler::populate_dimension_metadata)
    let dm_dims = source_dims_to_datamodel(project.dimensions(db));
    let converted_dims: Vec<crate::dimensions::Dimension> = dm_dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    let mut dim_names: Vec<String> = Vec::new();
    let mut dim_infos: Vec<crate::bytecode::DimensionInfo> = Vec::new();

    let intern_name = |names: &mut Vec<String>, name: &str| -> crate::bytecode::NameId {
        if let Some(idx) = names.iter().position(|n| n == name) {
            return idx as crate::bytecode::NameId;
        }
        let id = names.len() as crate::bytecode::NameId;
        names.push(name.to_string());
        id
    };

    for dim in &converted_dims {
        match dim {
            crate::dimensions::Dimension::Indexed(dim_name, size) => {
                let name_id = intern_name(&mut dim_names, dim_name.as_str());
                dim_infos.push(crate::bytecode::DimensionInfo::indexed(
                    name_id,
                    *size as u16,
                ));
            }
            crate::dimensions::Dimension::Named(dim_name, named_dim) => {
                let name_id = intern_name(&mut dim_names, dim_name.as_str());
                let element_name_ids: smallvec::SmallVec<[crate::bytecode::NameId; 8]> = named_dim
                    .elements
                    .iter()
                    .map(|elem| intern_name(&mut dim_names, elem.as_str()))
                    .collect();
                dim_infos.push(crate::bytecode::DimensionInfo::named(
                    name_id,
                    element_name_ids,
                ));
            }
        }
    }

    // Build the symbolic compiled module
    let sym_module = SymbolicCompiledModule {
        ident: Ident::new(&model_name),
        n_slots: layout.n_slots,
        compiled_initials,
        compiled_flows: flows_concat.bytecode,
        compiled_stocks: stocks_concat.bytecode,
        graphical_functions: merged.graphical_functions,
        module_decls: merged.module_decls,
        static_views: merged.static_views,
        arrays: vec![],
        dimensions: dim_infos,
        subdim_relations: vec![],
        names: dim_names,
        temp_offsets: merged.temp_offsets,
        temp_total_size: merged.temp_total_size,
        dim_lists: merged.dim_lists,
    };

    // Resolve symbolic -> concrete offsets
    resolve_module(&sym_module, layout)
}

/// Assemble a full CompiledSimulation from assembled modules.
///
/// NOT a tracked function -- caching is at the per-variable level.
pub fn assemble_simulation(
    db: &dyn Db,
    project: SourceProject,
    main_model_name: &str,
) -> Result<crate::vm::CompiledSimulation, String> {
    use crate::common::{Canonical, Ident};
    use crate::vm::CompiledSimulation;

    let project_models = project.models(db);
    let main_model_canonical = canonicalize(main_model_name);

    if !project_models.contains_key(main_model_canonical.as_ref()) {
        return Err(format!("no model named '{}' to simulate", main_model_name));
    }

    // Enumerate module instances by walking module variables recursively.
    // Each unique (model_name, input_set) pair gets its own CompiledModule.
    let module_instances = enumerate_module_instances(db, project, main_model_name)?;

    // Sort module names: main first, then all others alphabetically
    let main_ident = Ident::<Canonical>::new(main_model_name);
    let mut module_names: Vec<&Ident<Canonical>> = module_instances.keys().collect();
    module_names.sort_unstable();
    let mut sorted_names = vec![&main_ident];
    sorted_names.extend(
        module_names
            .into_iter()
            .filter(|n| n.as_str() != main_model_name),
    );

    let root_input_set: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let root_key: crate::vm::ModuleKey = (main_ident.clone(), root_input_set);

    let mut compiled_modules: HashMap<crate::vm::ModuleKey, crate::bytecode::CompiledModule> =
        HashMap::new();

    for name in &sorted_names {
        let distinct_inputs = &module_instances[*name];
        for inputs in distinct_inputs.iter() {
            let model_name_str = name.as_str();
            let canonical_name = canonicalize(model_name_str);
            let source_model = project_models.get(canonical_name.as_ref()).ok_or_else(|| {
                format!(
                    "model '{}' referenced as module but not found in project",
                    model_name_str,
                )
            })?;

            let is_root = name.as_str() == main_model_name;
            let compiled = assemble_module(db, *source_model, project, is_root)?;
            let module_key: crate::vm::ModuleKey = ((*name).clone(), inputs.clone());
            compiled_modules.insert(module_key, compiled);
        }
    }

    // Build Specs from project sim specs
    let sim_specs_dm = source_sim_specs_to_datamodel(project.sim_specs(db));
    let specs = crate::vm::Specs::from(&sim_specs_dm);

    // Compute flattened offsets for variable name -> offset mapping
    let offsets = calc_flattened_offsets_incremental(db, project, main_model_name);
    let offsets: HashMap<Ident<Canonical>, usize> =
        offsets.into_iter().map(|(k, (off, _))| (k, off)).collect();

    Ok(CompiledSimulation::new(
        compiled_modules,
        specs,
        root_key,
        offsets,
    ))
}

type ModuleInstanceMap = HashMap<Ident<Canonical>, BTreeSet<BTreeSet<Ident<Canonical>>>>;

/// Enumerate all module instances in a project, starting from the main model.
/// Returns a map from model name to the set of distinct input sets that model
/// is instantiated with.
fn enumerate_module_instances(
    db: &dyn Db,
    project: SourceProject,
    main_model_name: &str,
) -> Result<ModuleInstanceMap, String> {
    use crate::common::{Canonical, Ident};

    let main_ident = Ident::<Canonical>::new(main_model_name);

    let mut modules: ModuleInstanceMap = HashMap::new();

    // Main model with no inputs
    let no_inputs = BTreeSet::new();
    modules.insert(main_ident, [no_inputs].into_iter().collect());

    enumerate_module_instances_inner(db, project, main_model_name, &mut modules)?;

    Ok(modules)
}

fn enumerate_module_instances_inner(
    db: &dyn Db,
    project: SourceProject,
    model_name: &str,
    modules: &mut ModuleInstanceMap,
) -> Result<(), String> {
    use crate::common::{Canonical, Ident};

    let project_models = project.models(db);
    let canonical_name = canonicalize(model_name);
    let source_model = project_models
        .get(canonical_name.as_ref())
        .ok_or_else(|| format!("model '{}' not found", model_name))?;

    let source_vars = source_model.variables(db);
    for (_var_name, source_var) in source_vars.iter() {
        if source_var.kind(db) != SourceVariableKind::Module {
            continue;
        }

        let sub_model_name = source_var.model_name(db);
        let sub_canonical = canonicalize(sub_model_name);

        if !project_models.contains_key(sub_canonical.as_ref()) {
            return Err(format!(
                "model '{}' referenced as module but not found",
                sub_model_name,
            ));
        }

        let inputs: BTreeSet<Ident<Canonical>> = source_var
            .module_refs(db)
            .iter()
            .map(|mr| Ident::new(canonicalize(&mr.dst).as_ref()))
            .collect();

        let key = Ident::<Canonical>::new(sub_model_name);
        let is_new = !modules.contains_key(&key);

        modules.entry(key).or_default().insert(inputs);

        if is_new {
            enumerate_module_instances_inner(db, project, sub_model_name, modules)?;
        }
    }

    Ok(())
}

/// Compute flattened offsets for the incremental path.
/// Mirrors calc_flattened_offsets from interpreter.rs but works with
/// SourceModel/SourceVariable from the salsa database.
fn calc_flattened_offsets_incremental(
    db: &dyn Db,
    project: SourceProject,
    model_name: &str,
) -> HashMap<Ident<Canonical>, (usize, usize)> {
    use crate::common::{Canonical, Ident};

    let is_root = model_name == "main";
    let project_models = project.models(db);
    let canonical_name = canonicalize(model_name);

    let source_model = match project_models.get(canonical_name.as_ref()) {
        Some(m) => m,
        None => return HashMap::new(),
    };

    let mut offsets: HashMap<Ident<Canonical>, (usize, usize)> = HashMap::new();
    let mut i = 0;
    if is_root {
        offsets.insert(Ident::new("time"), (0, 1));
        offsets.insert(Ident::new("dt"), (1, 1));
        offsets.insert(Ident::new("initial_time"), (2, 1));
        offsets.insert(Ident::new("final_time"), (3, 1));
        i += crate::vm::IMPLICIT_VAR_COUNT;
    }

    let source_vars = source_model.variables(db);
    let var_names = source_model.variable_names(db);
    let mut sorted_names: Vec<&String> = var_names.iter().collect();
    sorted_names.sort_unstable();

    for ident in &sorted_names {
        let size = if let Some(svar) = source_vars.get(ident.as_str()) {
            if svar.kind(db) == SourceVariableKind::Module {
                let sub_model_name = svar.model_name(db);
                let sub_offsets = calc_flattened_offsets_incremental(db, project, sub_model_name);
                let mut sub_var_names: Vec<&Ident<Canonical>> = sub_offsets.keys().collect();
                sub_var_names.sort_unstable();
                for sub_name in &sub_var_names {
                    let (sub_off, sub_size) = sub_offsets[*sub_name];
                    let ident_canonical = Ident::new(ident.as_str());
                    let sub_canonical = Ident::new(sub_name.as_str());
                    offsets.insert(
                        Ident::<Canonical>::from_unchecked(format!(
                            "{}.{}",
                            ident_canonical.to_source_repr(),
                            sub_canonical.to_source_repr()
                        )),
                        (i + sub_off, sub_size),
                    );
                }
                let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
                sub_size
            } else {
                let var_sz = variable_size(db, *svar, project);
                if var_sz > 1 {
                    // Array variable: produce per-element offsets
                    let dims = variable_dimensions(db, *svar, project);
                    if !dims.is_empty() {
                        for (j, subscripts) in
                            crate::dimensions::SubscriptIterator::new(dims).enumerate()
                        {
                            let subscript = subscripts.join(",");
                            let ident_canonical = Ident::new(ident.as_str());
                            let subscripted_ident = Ident::<Canonical>::from_unchecked(format!(
                                "{}[{}]",
                                ident_canonical.to_source_repr(),
                                subscript
                            ));
                            offsets.insert(subscripted_ident, (i + j, 1));
                        }
                    }
                } else {
                    let ident_canonical = Ident::new(ident.as_str());
                    offsets.insert(
                        Ident::<Canonical>::from_unchecked(ident_canonical.to_source_repr()),
                        (i, 1),
                    );
                }
                var_sz
            }
        } else {
            let ident_canonical = Ident::new(ident.as_str());
            offsets.insert(
                Ident::<Canonical>::from_unchecked(ident_canonical.to_source_repr()),
                (i, 1),
            );
            1
        };
        i += size;
    }

    offsets
}

/// Compile a project incrementally using salsa.
///
/// This is the new entry point that replaces compile_project for the
/// incremental path. Falls back to the monolithic compile_project when
/// the incremental path is not yet supported (e.g., multi-model projects).
pub fn compile_project_incremental(
    db: &SimlinDb,
    project: SourceProject,
    main_model_name: &str,
) -> crate::Result<crate::vm::CompiledSimulation> {
    match assemble_simulation(db, project, main_model_name) {
        Ok(compiled) => Ok(compiled),
        Err(msg) => crate::sim_err!(NotSimulatable, msg),
    }
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod db_tests;
