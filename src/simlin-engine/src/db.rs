// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};

use salsa::Accumulator;
use salsa::plumbing::AsId;

use crate::canonicalize;
use crate::common::{EquationError, Error, UnitError};
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
    );

    let mut models = HashMap::new();

    for dm_model in &project.models {
        let canonical_model_name = canonicalize(&dm_model.name).into_owned();
        let model_id = ModelId::new(db, canonical_model_name.clone());

        let variable_names: Vec<String> = dm_model
            .variables
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect();

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

        let source_model =
            SourceModel::new(db, dm_model.name.clone(), variable_names, source_var_map);

        models.insert(
            canonical_model_name,
            SyncedModel {
                id: model_id,
                source: source_model,
                variables,
            },
        );
    }

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

        let variable_names: Vec<String> = dm_model
            .variables
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect();

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

    PersistentSyncState {
        project: source_project,
        models: new_models,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel;

    fn simple_project() -> datamodel::Project {
        datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: Some("months".to_string()),
            },
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: Some("people".to_string()),
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        }
    }

    #[test]
    fn test_create_db() {
        let _db = SimlinDb::default();
    }

    #[test]
    fn test_intern_variable_id_same_name() {
        let db = SimlinDb::default();
        let id1 = VariableId::new(&db, "population".to_string());
        let id2 = VariableId::new(&db, "population".to_string());
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_intern_variable_id_different_names() {
        let db = SimlinDb::default();
        let id1 = VariableId::new(&db, "population".to_string());
        let id2 = VariableId::new(&db, "birth_rate".to_string());
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_intern_model_id_same_name() {
        let db = SimlinDb::default();
        let id1 = ModelId::new(&db, "main".to_string());
        let id2 = ModelId::new(&db, "main".to_string());
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_intern_model_id_different_names() {
        let db = SimlinDb::default();
        let id1 = ModelId::new(&db, "main".to_string());
        let id2 = ModelId::new(&db, "submodel".to_string());
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_intern_variable_id_text_roundtrip() {
        let db = SimlinDb::default();
        let id = VariableId::new(&db, "birth_rate".to_string());
        assert_eq!(id.text(&db), "birth_rate");
    }

    #[test]
    fn test_intern_model_id_text_roundtrip() {
        let db = SimlinDb::default();
        let id = ModelId::new(&db, "main".to_string());
        assert_eq!(id.text(&db), "main");
    }

    #[test]
    fn test_sync_simple_project() {
        let db = SimlinDb::default();
        let project = simple_project();
        let result = sync_from_datamodel(&db, &project);

        assert_eq!(result.project.name(&db), "test");
        assert_eq!(result.project.model_names(&db).len(), 1);
        assert_eq!(result.project.model_names(&db)[0], "main");

        let sim_specs = result.project.sim_specs(&db);
        assert_eq!(sim_specs.start, 0.0);
        assert_eq!(sim_specs.stop, 10.0);
        assert_eq!(sim_specs.time_units, Some("months".to_string()));

        assert!(result.models.contains_key("main"));
        let main_model = &result.models["main"];
        assert_eq!(main_model.source.name(&db), "main");
        assert_eq!(main_model.source.variable_names(&db).len(), 1);
        assert_eq!(main_model.source.variable_names(&db)[0], "population");

        let pop_var = &main_model.variables["population"];
        assert_eq!(pop_var.id.text(&db), "population");
        assert_eq!(pop_var.source.kind(&db), SourceVariableKind::Aux);
        assert_eq!(pop_var.source.units(&db), &Some("people".to_string()));
        assert_eq!(
            pop_var.source.equation(&db),
            &SourceEquation::Scalar("100".to_string())
        );
        assert!(!pop_var.source.non_negative(&db));
        assert!(!pop_var.source.can_be_module_input(&db));
    }

    #[test]
    fn test_sync_multi_model() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "multi".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![
                datamodel::Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                        ident: "x".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    })],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                },
                datamodel::Model {
                    name: "submodel".to_string(),
                    sim_specs: None,
                    variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                        ident: "y".to_string(),
                        equation: datamodel::Equation::Scalar("2".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    })],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                },
            ],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        assert_eq!(result.models.len(), 2);
        assert!(result.models.contains_key("main"));
        assert!(result.models.contains_key("submodel"));

        // Different model names get different IDs
        let main_id = result.models["main"].id;
        let sub_id = result.models["submodel"].id;
        assert_ne!(main_id, sub_id);
    }

    #[test]
    fn test_sync_all_variable_kinds() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "kinds".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Stock(datamodel::Stock {
                        ident: "stock_var".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["flow_in".to_string()],
                        outflows: vec!["flow_out".to_string()],
                        non_negative: true,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "flow_var".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        non_negative: true,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "aux_var".to_string(),
                        equation: datamodel::Equation::Scalar("5".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "mod_var".to_string(),
                        model_name: "submodel".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "x".to_string(),
                            dst: "y".to_string(),
                        }],
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let main = &result.models["main"];

        let stock = &main.variables["stock_var"];
        assert_eq!(stock.source.kind(&db), SourceVariableKind::Stock);
        assert_eq!(stock.source.inflows(&db), &vec!["flow_in".to_string()]);
        assert_eq!(stock.source.outflows(&db), &vec!["flow_out".to_string()]);
        assert!(stock.source.non_negative(&db));

        let flow = &main.variables["flow_var"];
        assert_eq!(flow.source.kind(&db), SourceVariableKind::Flow);
        assert!(flow.source.non_negative(&db));

        let aux = &main.variables["aux_var"];
        assert_eq!(aux.source.kind(&db), SourceVariableKind::Aux);

        let module = &main.variables["mod_var"];
        assert_eq!(module.source.kind(&db), SourceVariableKind::Module);
        assert_eq!(module.source.module_refs(&db).len(), 1);
        assert_eq!(module.source.module_refs(&db)[0].src, "x");
        assert_eq!(module.source.module_refs(&db)[0].dst, "y");
    }

    #[test]
    fn test_sync_variable_with_gf() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "gf_test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "lookup_var".to_string(),
                    equation: datamodel::Equation::Scalar("lookup_var(time)".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: Some(datamodel::GraphicalFunction {
                        kind: datamodel::GraphicalFunctionKind::Continuous,
                        x_points: Some(vec![0.0, 1.0, 2.0]),
                        y_points: vec![0.0, 5.0, 10.0],
                        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                        y_scale: datamodel::GraphicalFunctionScale {
                            min: 0.0,
                            max: 10.0,
                        },
                    }),
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let var = &result.models["main"].variables["lookup_var"];
        let gf = var.source.gf(&db);
        assert!(gf.is_some());
        let gf = gf.as_ref().unwrap();
        assert_eq!(gf.kind, SourceGraphicalFunctionKind::Continuous);
        assert_eq!(gf.x_points, Some(vec![0.0, 1.0, 2.0]));
        assert_eq!(gf.y_points, vec![0.0, 5.0, 10.0]);
        assert_eq!(gf.x_scale.min, 0.0);
        assert_eq!(gf.x_scale.max, 2.0);
    }

    #[test]
    fn test_sync_dimensions() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "dim_test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![
                datamodel::Dimension::named(
                    "Region".to_string(),
                    vec!["North".to_string(), "South".to_string()],
                ),
                datamodel::Dimension::indexed("Periods".to_string(), 5),
            ],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let dims = result.project.dimensions(&db);
        assert_eq!(dims.len(), 2);

        assert_eq!(dims[0].name, "Region");
        assert_eq!(
            dims[0].elements,
            SourceDimensionElements::Named(vec!["North".to_string(), "South".to_string()])
        );

        assert_eq!(dims[1].name, "Periods");
        assert_eq!(dims[1].elements, SourceDimensionElements::Indexed(5));
    }

    #[test]
    fn test_sync_module_refs() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "mod_test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "my_module".to_string(),
                    model_name: "sub".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![
                        datamodel::ModuleReference {
                            src: "input_a".to_string(),
                            dst: "a".to_string(),
                        },
                        datamodel::ModuleReference {
                            src: "input_b".to_string(),
                            dst: "b".to_string(),
                        },
                    ],
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let module = &result.models["main"].variables["my_module"];
        assert_eq!(module.source.kind(&db), SourceVariableKind::Module);
        let refs = module.source.module_refs(&db);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].src, "input_a");
        assert_eq!(refs[0].dst, "a");
        assert_eq!(refs[1].src, "input_b");
        assert_eq!(refs[1].dst, "b");
    }

    #[test]
    fn test_sync_resync_updates() {
        let db = SimlinDb::default();
        let mut project = simple_project();
        let result1 = sync_from_datamodel(&db, &project);

        let pop1 = &result1.models["main"].variables["population"];
        assert_eq!(
            pop1.source.equation(&db),
            &SourceEquation::Scalar("100".to_string())
        );

        // Modify the equation and re-sync
        project.models[0].variables[0].set_scalar_equation("200");
        let result2 = sync_from_datamodel(&db, &project);

        let pop2 = &result2.models["main"].variables["population"];
        assert_eq!(
            pop2.source.equation(&db),
            &SourceEquation::Scalar("200".to_string())
        );

        // Interned IDs for the same canonical name should be the same
        assert_eq!(pop1.id, pop2.id);
    }

    #[test]
    fn test_sync_empty_model_name_canonicalized() {
        let db = SimlinDb::default();
        let id1 = ModelId::new(&db, "".to_string());
        let id2 = ModelId::new(&db, "main".to_string());
        // Empty and "main" are different canonical strings
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_sync_sim_specs_dt_reciprocal() {
        let db = SimlinDb::default();
        let mut project = simple_project();
        project.sim_specs.dt = datamodel::Dt::Reciprocal(4.0);
        project.sim_specs.save_step = Some(datamodel::Dt::Dt(0.5));
        project.sim_specs.sim_method = datamodel::SimMethod::RungeKutta4;

        let result = sync_from_datamodel(&db, &project);
        let specs = result.project.sim_specs(&db);
        assert_eq!(specs.dt, SourceDt::Reciprocal(4.0));
        assert_eq!(specs.save_step, Some(SourceDt::Dt(0.5)));
        assert_eq!(specs.sim_method, SourceSimMethod::RungeKutta4);
    }

    #[test]
    fn test_parse_source_variable_scalar() {
        use crate::ast::Expr0;
        use crate::variable::Variable;

        let db = SimlinDb::default();
        let project = simple_project();
        let result = sync_from_datamodel(&db, &project);

        let pop_var = result.models["main"].variables["population"].source;
        let parsed = parse_source_variable(&db, pop_var, result.project);

        // Should parse to a Var (aux) with equation "100"
        assert!(matches!(&parsed.variable, Variable::Var { .. }));
        assert_eq!(parsed.variable.ident(), "population");

        // Should have a valid AST with a constant 100.0
        let ast = parsed.variable.ast();
        assert!(ast.is_some());
        if let Some(crate::ast::Ast::Scalar(Expr0::Const(_, val, _))) = ast {
            assert_eq!(*val, 100.0);
        } else {
            panic!("Expected Scalar(Const(100.0)), got {:?}", ast);
        }
    }

    #[test]
    fn test_parse_source_variable_stock() {
        use crate::variable::Variable;

        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Stock(datamodel::Stock {
                        ident: "inventory".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["production".to_string()],
                        outflows: vec!["sales".to_string()],
                        non_negative: true,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "production".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        non_negative: false,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "sales".to_string(),
                        equation: datamodel::Equation::Scalar("5".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        non_negative: false,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);

        // Parse the stock variable
        let stock_var = result.models["main"].variables["inventory"].source;
        let parsed = parse_source_variable(&db, stock_var, result.project);
        assert!(matches!(&parsed.variable, Variable::Stock { .. }));
        assert_eq!(parsed.variable.ident(), "inventory");

        // Parse a flow variable
        let flow_var = result.models["main"].variables["production"].source;
        let parsed = parse_source_variable(&db, flow_var, result.project);
        assert!(matches!(
            &parsed.variable,
            Variable::Var { is_flow: true, .. }
        ));
        assert_eq!(parsed.variable.ident(), "production");
    }

    #[test]
    fn test_parse_source_variable_matches_direct_parse() {
        use crate::variable::parse_var;

        let db = SimlinDb::default();
        let project = simple_project();
        let result = sync_from_datamodel(&db, &project);

        // Parse via tracked function
        let pop_var = result.models["main"].variables["population"].source;
        let tracked_result = parse_source_variable(&db, pop_var, result.project);

        // Parse directly via parse_var for comparison
        let dm_var = &project.models[0].variables[0];
        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let mut implicit_vars = Vec::new();
        let direct_result = parse_var(
            &project.dimensions,
            dm_var,
            &mut implicit_vars,
            &units_ctx,
            |mi| Ok(Some(mi.clone())),
        );

        // The tracked function and direct parse should produce equivalent results
        assert_eq!(tracked_result.variable.ident(), direct_result.ident());
        assert_eq!(
            tracked_result.variable.equation_errors().is_some(),
            direct_result.equation_errors().is_some()
        );
    }

    #[test]
    fn test_incrementality_unchanged_variable_not_reparsed() {
        use salsa::Setter;

        let mut db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "alpha".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "beta".to_string(),
                        equation: datamodel::Equation::Scalar("20".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let (source_project, alpha_src, beta_src) = {
            let result = sync_from_datamodel(&db, &project);
            (
                result.project,
                result.models["main"].variables["alpha"].source,
                result.models["main"].variables["beta"].source,
            )
        };

        // Initial parse of both variables to prime the cache
        let beta_ptr_before = {
            let alpha_result = parse_source_variable(&db, alpha_src, source_project);
            let beta_result = parse_source_variable(&db, beta_src, source_project);
            assert_eq!(alpha_result.variable.ident(), "alpha");
            assert_eq!(beta_result.variable.ident(), "beta");
            beta_result as *const ParsedVariableResult
        };

        // Modify only alpha's equation; beta is unchanged
        alpha_src
            .set_equation(&mut db)
            .to(SourceEquation::Scalar("42".to_string()));

        // Re-parse both: alpha should have new result, beta should be cached
        let alpha_result_2 = parse_source_variable(&db, alpha_src, source_project);
        let beta_result_2 = parse_source_variable(&db, beta_src, source_project);

        // Alpha's parse result should reflect the new equation
        if let Some(crate::ast::Ast::Scalar(crate::ast::Expr0::Const(_, val, _))) =
            alpha_result_2.variable.ast()
        {
            assert_eq!(*val, 42.0);
        } else {
            panic!(
                "Expected alpha to parse as Const(42.0), got {:?}",
                alpha_result_2.variable.ast()
            );
        }

        // Beta should be pointer-equal (same &ParsedVariableResult from cache)
        let beta_ptr_after = beta_result_2 as *const ParsedVariableResult;
        assert_eq!(
            beta_ptr_before, beta_ptr_after,
            "beta should be returned from salsa cache (pointer-equal) since it was not modified"
        );
    }

    #[test]
    fn test_variable_direct_dependencies_constant() {
        let db = SimlinDb::default();
        let project = simple_project();
        let result = sync_from_datamodel(&db, &project);

        let pop_var = result.models["main"].variables["population"].source;
        let deps = variable_direct_dependencies(&db, pop_var, result.project);

        assert!(deps.dt_deps.is_empty(), "constant has no deps");
        assert!(deps.initial_deps.is_empty(), "constant has no initial deps");
    }

    #[test]
    fn test_variable_direct_dependencies_with_refs() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "rate".to_string(),
                        equation: datamodel::Equation::Scalar("0.1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "population".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "births".to_string(),
                        equation: datamodel::Equation::Scalar("population * rate".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        non_negative: false,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);

        let births_var = result.models["main"].variables["births"].source;
        let deps = variable_direct_dependencies(&db, births_var, result.project);

        assert_eq!(
            deps.dt_deps,
            ["population", "rate"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn test_variable_direct_dependencies_stock() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Stock(datamodel::Stock {
                    ident: "inventory".to_string(),
                    equation: datamodel::Equation::Scalar("initial_value".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["production".to_string()],
                    outflows: vec![],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let stock_var = result.models["main"].variables["inventory"].source;
        let deps = variable_direct_dependencies(&db, stock_var, result.project);

        // Stock's init equation references "initial_value"
        assert!(deps.dt_deps.contains("initial_value"));
        assert!(deps.initial_deps.contains("initial_value"));
    }

    #[test]
    fn test_variable_direct_dependencies_module() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "submodel".to_string(),
                    model_name: "sub".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![
                        datamodel::ModuleReference {
                            src: "input_x".to_string(),
                            dst: "x".to_string(),
                        },
                        datamodel::ModuleReference {
                            src: "input_y".to_string(),
                            dst: "y".to_string(),
                        },
                    ],
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let mod_var = result.models["main"].variables["submodel"].source;
        let deps = variable_direct_dependencies(&db, mod_var, result.project);

        assert_eq!(
            deps.dt_deps,
            ["input_x", "input_y"]
                .iter()
                .map(|s| s.to_string())
                .collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn test_incrementality_same_deps_no_recompute() {
        use salsa::Setter;

        let mut db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "alpha".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "beta".to_string(),
                        equation: datamodel::Equation::Scalar("alpha + gamma".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "gamma".to_string(),
                        equation: datamodel::Equation::Scalar("20".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let (source_project, _alpha_src, beta_src, source_model) = {
            let result = sync_from_datamodel(&db, &project);
            (
                result.project,
                result.models["main"].variables["alpha"].source,
                result.models["main"].variables["beta"].source,
                result.models["main"].source,
            )
        };

        // Prime the cache: compute deps and dep graph
        let (beta_dt_before, beta_init_before) = {
            let deps = variable_direct_dependencies(&db, beta_src, source_project);
            assert_eq!(
                deps.dt_deps,
                ["alpha", "gamma"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<BTreeSet<_>>()
            );
            (deps.dt_deps.clone(), deps.initial_deps.clone())
        };

        let graph_before = model_dependency_graph(&db, source_model, source_project);
        let graph_ptr_before = graph_before as *const ModelDepGraphResult;

        // Change beta's equation from "alpha + gamma" to "alpha * gamma"
        // Same deps, different equation
        beta_src
            .set_equation(&mut db)
            .to(SourceEquation::Scalar("alpha * gamma".to_string()));

        // Beta's deps should be the same (alpha, gamma)
        let beta_deps_after = variable_direct_dependencies(&db, beta_src, source_project);
        assert_eq!(beta_dt_before, beta_deps_after.dt_deps);
        assert_eq!(beta_init_before, beta_deps_after.initial_deps);

        // The dep graph should be returned from cache (pointer-equal)
        let graph_after = model_dependency_graph(&db, source_model, source_project);
        let graph_ptr_after = graph_after as *const ModelDepGraphResult;
        assert_eq!(
            graph_ptr_before, graph_ptr_after,
            "model_dependency_graph should be cached when deps don't change"
        );
    }

    #[test]
    fn test_incrementality_different_deps_recompute() {
        use salsa::Setter;

        let mut db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "alpha".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "beta".to_string(),
                        equation: datamodel::Equation::Scalar("alpha".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "gamma".to_string(),
                        equation: datamodel::Equation::Scalar("20".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let (source_project, beta_src, source_model) = {
            let result = sync_from_datamodel(&db, &project);
            (
                result.project,
                result.models["main"].variables["beta"].source,
                result.models["main"].source,
            )
        };

        // Prime the cache
        let graph_before = model_dependency_graph(&db, source_model, source_project);
        let graph_ptr_before = graph_before as *const ModelDepGraphResult;

        // Change beta's equation from "alpha" to "gamma" -- different deps
        beta_src
            .set_equation(&mut db)
            .to(SourceEquation::Scalar("gamma".to_string()));

        // The dep graph should be recomputed (different pointer)
        let graph_after = model_dependency_graph(&db, source_model, source_project);
        let graph_ptr_after = graph_after as *const ModelDepGraphResult;
        assert_ne!(
            graph_ptr_before, graph_ptr_after,
            "model_dependency_graph should recompute when deps change"
        );

        // Verify the new graph has the correct deps
        assert!(
            graph_after.dt_dependencies["beta"].contains("gamma"),
            "beta should now depend on gamma"
        );
        assert!(
            !graph_after.dt_dependencies["beta"].contains("alpha"),
            "beta should no longer depend on alpha"
        );
    }

    #[test]
    fn test_model_dependency_graph_basic() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "rate".to_string(),
                        equation: datamodel::Equation::Scalar("0.1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "growth".to_string(),
                        equation: datamodel::Equation::Scalar("rate * 100".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let graph = model_dependency_graph(&db, result.models["main"].source, result.project);

        // growth depends on rate (transitively)
        assert!(graph.dt_dependencies["growth"].contains("rate"));
        // rate has no deps
        assert!(graph.dt_dependencies["rate"].is_empty());

        // Flows runlist should have rate before growth
        let rate_pos = graph
            .runlist_flows
            .iter()
            .position(|n| n == "rate")
            .unwrap();
        let growth_pos = graph
            .runlist_flows
            .iter()
            .position(|n| n == "growth")
            .unwrap();
        assert!(
            rate_pos < growth_pos,
            "rate should come before growth in runlist"
        );
    }

    #[test]
    fn test_model_dependency_graph_stock_breaks_chain() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Stock(datamodel::Stock {
                        ident: "population".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["births".to_string()],
                        outflows: vec![],
                        non_negative: false,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "births".to_string(),
                        equation: datamodel::Equation::Scalar("population * 0.1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        non_negative: false,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let graph = model_dependency_graph(&db, result.models["main"].source, result.project);

        // In dt phase, stocks have empty deps (chain breaks)
        assert!(
            graph.dt_dependencies["population"].is_empty(),
            "stock should have empty dt deps"
        );

        // births references population but population is a stock, so in dt phase
        // the dep is filtered out
        assert!(
            !graph.dt_dependencies["births"].contains("population"),
            "births should not depend on stock in dt phase"
        );

        // Stock equation is "100" (constant), so initial deps are empty
        assert!(
            graph.initial_dependencies["population"].is_empty(),
            "stock with constant equation should have empty initial deps"
        );
    }

    #[test]
    fn test_model_dependency_graph_circular_emits_diagnostic() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "a".to_string(),
                        equation: datamodel::Equation::Scalar("b".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "b".to_string(),
                        equation: datamodel::Equation::Scalar("a".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let result = sync_from_datamodel(&db, &project);
        let _graph = model_dependency_graph(&db, result.models["main"].source, result.project);

        // Collect diagnostics emitted by model_dependency_graph
        let diags = model_dependency_graph::accumulated::<CompilationDiagnostic>(
            &db,
            result.models["main"].source,
            result.project,
        );
        let has_circular = diags.iter().any(|d| {
            matches!(
                d.0.error,
                DiagnosticError::Model(crate::common::Error {
                    code: crate::common::ErrorCode::CircularDependency,
                    ..
                })
            )
        });
        assert!(
            has_circular,
            "circular dependency between a and b should emit a diagnostic"
        );
    }

    fn feedback_loop_project() -> datamodel::Project {
        datamodel::Project {
            name: "feedback".to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            },
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Stock(datamodel::Stock {
                        ident: "population".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        inflows: vec!["births".to_string()],
                        outflows: vec![],
                        non_negative: false,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "births".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "population * birth_rate".to_string(),
                        ),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        non_negative: false,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "birth_rate".to_string(),
                        equation: datamodel::Equation::Scalar("0.1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        }
    }

    #[test]
    fn test_normalize_module_ref_str() {
        assert_eq!(normalize_module_ref_str("foo\u{00B7}output"), "foo");
        assert_eq!(normalize_module_ref_str("plain_name"), "plain_name");
        assert_eq!(normalize_module_ref_str(""), "");
    }

    #[test]
    fn test_generate_max_abs_chain_str() {
        assert_eq!(generate_max_abs_chain_str(&[]), "0");
        assert_eq!(generate_max_abs_chain_str(&["p0".into()]), "\"p0\"");
        let two = generate_max_abs_chain_str(&["p0".into(), "p1".into()]);
        assert!(two.contains("ABS"));
        assert!(two.contains("p0"));
        assert!(two.contains("p1"));
        let three = generate_max_abs_chain_str(&["p0".into(), "p1".into(), "p2".into()]);
        assert!(three.contains("p2"));
    }

    #[test]
    fn test_model_causal_edges_feedback_loop() {
        let db = SimlinDb::default();
        let project = feedback_loop_project();
        let result = sync_from_datamodel(&db, &project);
        let model = result.models["main"].source;

        let edges = model_causal_edges(&db, model, result.project);

        assert!(edges.stocks.contains("population"));
        // births flows into population, so births -> population edge exists
        assert!(
            edges
                .edges
                .get("births")
                .is_some_and(|t| t.contains("population")),
            "births should have edge to population (stock inflow)"
        );
        // births = population * birth_rate, so population -> births and birth_rate -> births
        assert!(
            edges
                .edges
                .get("population")
                .is_some_and(|t| t.contains("births")),
            "population should have edge to births (dep)"
        );
        assert!(
            edges
                .edges
                .get("birth_rate")
                .is_some_and(|t| t.contains("births")),
            "birth_rate should have edge to births (dep)"
        );
    }

    #[test]
    fn test_model_loop_circuits_finds_feedback() {
        let db = SimlinDb::default();
        let project = feedback_loop_project();
        let result = sync_from_datamodel(&db, &project);
        let model = result.models["main"].source;

        let circuits = model_loop_circuits(&db, model, result.project);

        // population -> births -> population is the single feedback loop
        assert!(
            !circuits.circuits.is_empty(),
            "should find at least one circuit"
        );
        let has_pop_births_loop = circuits
            .circuits
            .iter()
            .any(|c| c.contains(&"population".to_string()) && c.contains(&"births".to_string()));
        assert!(has_pop_births_loop, "should find population-births loop");
    }

    #[test]
    fn test_model_cycle_partitions_single_stock() {
        let db = SimlinDb::default();
        let project = feedback_loop_project();
        let result = sync_from_datamodel(&db, &project);
        let model = result.models["main"].source;

        let partitions = model_cycle_partitions(&db, model, result.project);

        // Single stock should yield one partition
        assert!(
            !partitions.partitions.is_empty(),
            "should have at least one partition"
        );
        assert!(
            partitions.stock_partition.contains_key("population"),
            "population should be in a partition"
        );
    }

    #[test]
    fn test_model_ltm_synthetic_variables_generates_scores() {
        let db = SimlinDb::default();
        let project = feedback_loop_project();
        let result = sync_from_datamodel(&db, &project);
        let model = result.models["main"].source;

        let ltm = model_ltm_synthetic_variables(&db, model, result.project);

        // Should generate link scores and loop scores for the feedback loop
        assert!(!ltm.vars.is_empty(), "should generate LTM variables");

        let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
        assert!(has_link_score, "should have link score variables");

        let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
        assert!(has_loop_score, "should have loop score variables");

        // All vars should have non-empty equations
        for var in &ltm.vars {
            assert!(
                !var.equation.is_empty(),
                "var {} should have non-empty equation",
                var.name
            );
        }
    }

    #[test]
    fn test_model_ltm_all_link_synthetic_variables_discovery_mode() {
        let db = SimlinDb::default();
        let project = feedback_loop_project();
        let result = sync_from_datamodel(&db, &project);
        let model = result.models["main"].source;

        let ltm = model_ltm_all_link_synthetic_variables(&db, model, result.project);

        assert!(!ltm.vars.is_empty(), "should generate link score variables");

        // Discovery mode should NOT generate loop scores
        let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
        assert!(
            !has_loop_score,
            "discovery mode should not have loop scores"
        );

        let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
        assert!(has_link_score, "should have link score variables");
    }

    #[test]
    fn test_model_ltm_no_loops_empty() {
        let db = SimlinDb::default();
        // Simple project has just a constant -- no loops
        let project = simple_project();
        let result = sync_from_datamodel(&db, &project);
        let model = result.models["main"].source;

        let ltm = model_ltm_synthetic_variables(&db, model, result.project);
        assert!(ltm.vars.is_empty(), "no loops should produce no LTM vars");
    }

    #[test]
    fn test_ltm_caching_equation_change_no_dep_change() {
        use salsa::Setter;

        let mut db = SimlinDb::default();
        let project = feedback_loop_project();
        let (source_project, births_src, source_model) = {
            let result = sync_from_datamodel(&db, &project);
            (
                result.project,
                result.models["main"].variables["births"].source,
                result.models["main"].source,
            )
        };

        // Prime the cache
        let circuits_before = model_loop_circuits(&db, source_model, source_project);
        let circuits_ptr_before = circuits_before as *const LoopCircuitsResult;

        // Change births equation from "population * birth_rate" to
        // "birth_rate * population" -- same deps, different equation text
        births_src.set_equation(&mut db).to(SourceEquation::Scalar(
            "birth_rate * population".to_string(),
        ));

        // Loop circuits should be pointer-equal (cached) because the
        // causal edge structure hasn't changed
        let circuits_after = model_loop_circuits(&db, source_model, source_project);
        let circuits_ptr_after = circuits_after as *const LoopCircuitsResult;
        assert_eq!(
            circuits_ptr_before, circuits_ptr_after,
            "loop circuits should be cached when deps don't change"
        );
    }

    #[test]
    fn test_ltm_caching_dep_change_recomputes_circuits() {
        use salsa::Setter;

        let mut db = SimlinDb::default();
        let project = feedback_loop_project();
        let (source_project, births_src, source_model) = {
            let result = sync_from_datamodel(&db, &project);
            (
                result.project,
                result.models["main"].variables["births"].source,
                result.models["main"].source,
            )
        };

        // Prime the cache
        let circuits_before = model_loop_circuits(&db, source_model, source_project);
        assert!(
            !circuits_before.circuits.is_empty(),
            "should have circuits initially"
        );

        // Change births to a constant -- breaks the feedback loop
        births_src
            .set_equation(&mut db)
            .to(SourceEquation::Scalar("10".to_string()));

        let circuits_after = model_loop_circuits(&db, source_model, source_project);
        assert!(
            circuits_after.circuits.is_empty(),
            "should have no circuits after breaking loop"
        );
    }

    // ── Accumulator parity tests ──────────────────────────────────────

    #[test]
    fn test_accumulator_no_errors_for_valid_project() {
        let db = SimlinDb::default();
        let project = simple_project();
        let sync = sync_from_datamodel(&db, &project);

        let diags = collect_all_diagnostics(&db, &sync);
        assert!(
            diags.is_empty(),
            "valid project should produce no diagnostics"
        );
    }

    #[test]
    fn test_accumulator_parse_error_bad_equation() {
        let db = SimlinDb::default();
        // "if then" is a syntax error (missing condition/consequent)
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "broken".to_string(),
                    equation: datamodel::Equation::Scalar("if then".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Private,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let sync = sync_from_datamodel(&db, &project);

        // Verify struct field path also shows an error
        let parsed = parse_source_variable(
            &db,
            sync.models["main"].variables["broken"].source,
            sync.project,
        );
        assert!(
            parsed.variable.equation_errors().is_some(),
            "struct fields should show equation errors for 'if then'"
        );

        let diags = collect_all_diagnostics(&db, &sync);
        assert!(!diags.is_empty(), "bad equation should produce diagnostics");

        let d = &diags[0];
        assert_eq!(d.model, "main");
        assert_eq!(d.variable.as_deref(), Some("broken"));
        assert!(
            matches!(&d.error, DiagnosticError::Equation(_)),
            "expected equation error, got {:?}",
            d.error
        );
    }

    #[test]
    fn test_accumulator_parity_with_struct_fields() {
        use std::collections::HashSet;

        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "parity".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "good".to_string(),
                        equation: datamodel::Equation::Scalar("42".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "bad_syntax".to_string(),
                        equation: datamodel::Equation::Scalar("if then".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "empty".to_string(),
                        equation: datamodel::Equation::Scalar(String::new()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let sync = sync_from_datamodel(&db, &project);

        // Collect from accumulator
        let accum_diags = collect_all_diagnostics(&db, &sync);

        // Collect from struct fields (parse_source_variable results)
        let mut field_equation_errors: HashSet<(String, crate::common::EquationError)> =
            HashSet::new();
        for (var_name, synced_var) in &sync.models["main"].variables {
            let parsed = parse_source_variable(&db, synced_var.source, sync.project);
            if let Some(errors) = parsed.variable.equation_errors() {
                for err in errors {
                    field_equation_errors.insert((var_name.clone(), err));
                }
            }
        }

        // Extract equation errors from accumulator
        let mut accum_equation_errors: HashSet<(String, crate::common::EquationError)> =
            HashSet::new();
        for d in &accum_diags {
            if let DiagnosticError::Equation(err) = &d.error
                && let Some(var) = &d.variable
            {
                accum_equation_errors.insert((var.clone(), err.clone()));
            }
        }

        assert_eq!(
            field_equation_errors, accum_equation_errors,
            "accumulator equation errors must match struct field errors"
        );
    }

    #[test]
    fn test_accumulator_multiple_models() {
        let db = SimlinDb::default();
        let project = datamodel::Project {
            name: "multi_err".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![
                datamodel::Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                        ident: "x".to_string(),
                        equation: datamodel::Equation::Scalar("if then".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    })],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                },
                datamodel::Model {
                    name: "sub".to_string(),
                    sim_specs: None,
                    variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                        ident: "y".to_string(),
                        equation: datamodel::Equation::Scalar("if then".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    })],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                },
            ],
            source: None,
            ai_information: None,
        };

        let sync = sync_from_datamodel(&db, &project);
        let diags = collect_all_diagnostics(&db, &sync);

        let models_with_errors: std::collections::HashSet<&str> =
            diags.iter().map(|d| d.model.as_str()).collect();
        assert!(
            models_with_errors.contains("main"),
            "main model should have errors"
        );
        assert!(
            models_with_errors.contains("sub"),
            "sub model should have errors"
        );
    }

    #[test]
    fn test_accumulator_incrementality() {
        use salsa::Setter;

        let mut db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "alpha".to_string(),
                        equation: datamodel::Equation::Scalar("if then".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "beta".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        // Extract the salsa IDs we need (they are Copy) before dropping sync
        let (alpha_src, source_model, source_project) = {
            let sync = sync_from_datamodel(&db, &project);
            let alpha_src = sync.models["main"].variables["alpha"].source;
            let source_model = sync.models["main"].source;
            let source_project = sync.project;

            // Initially: alpha has errors, beta does not
            let diags1 = collect_all_diagnostics(&db, &sync);
            assert_eq!(
                diags1
                    .iter()
                    .filter(|d| d.variable.as_deref() == Some("alpha"))
                    .count(),
                1,
                "alpha should have 1 error"
            );
            assert_eq!(
                diags1
                    .iter()
                    .filter(|d| d.variable.as_deref() == Some("beta"))
                    .count(),
                0,
                "beta should have no errors"
            );

            (alpha_src, source_model, source_project)
        };

        // Fix alpha's equation (needs &mut db)
        alpha_src
            .set_equation(&mut db)
            .to(SourceEquation::Scalar("42".to_string()));

        let diags2 = collect_model_diagnostics(&db, source_model, source_project);
        assert!(
            diags2.is_empty(),
            "after fixing alpha, no diagnostics expected"
        );
    }

    // ── Incremental sync tests ────────────────────────────────────────

    #[test]
    fn test_incremental_sync_fresh_matches_regular_sync() {
        let db1 = SimlinDb::default();
        let mut db2 = SimlinDb::default();
        let project = simple_project();

        let regular = sync_from_datamodel(&db1, &project);
        let state = sync_from_datamodel_incremental(&mut db2, &project, None);

        assert_eq!(regular.project.name(&db1), state.project.name(&db2));
        assert_eq!(regular.models.len(), state.models.len());
        for (name, regular_model) in &regular.models {
            let persistent_model = &state.models[name];
            assert_eq!(
                regular_model.source.name(&db1),
                persistent_model.source_model.name(&db2)
            );
            assert_eq!(
                regular_model.variables.len(),
                persistent_model.variables.len()
            );
        }
    }

    #[test]
    fn test_incremental_sync_preserves_cache_for_unchanged_variable() {
        let mut db = SimlinDb::default();
        let project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "alpha".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "beta".to_string(),
                        equation: datamodel::Equation::Scalar("20".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: datamodel::Visibility::Private,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            source: None,
            ai_information: None,
        };

        // Initial sync
        let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

        // Prime the cache by parsing both variables
        let alpha_src = state1.models["main"].variables["alpha"].source_var;
        let beta_src = state1.models["main"].variables["beta"].source_var;
        let beta_ptr_before = {
            let _alpha_result = parse_source_variable(&db, alpha_src, state1.project);
            let beta_result = parse_source_variable(&db, beta_src, state1.project);
            beta_result as *const ParsedVariableResult
        };

        // Modify only alpha's equation
        let mut project2 = project.clone();
        project2.models[0].variables[0].set_scalar_equation("42");

        // Incremental sync with previous state
        let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));

        // Same SourceProject handle should be reused
        assert_eq!(
            state1.project.as_id(),
            state2.project.as_id(),
            "SourceProject handle should be stable across incremental syncs"
        );

        // Beta's SourceVariable handle should be the same
        let beta_src2 = state2.models["main"].variables["beta"].source_var;
        assert_eq!(
            beta_src.as_id(),
            beta_src2.as_id(),
            "unchanged variable's handle should be stable"
        );

        // Beta's parse result should be pointer-equal (cached)
        let beta_result_after = parse_source_variable(&db, beta_src2, state2.project);
        let beta_ptr_after = beta_result_after as *const ParsedVariableResult;
        assert_eq!(
            beta_ptr_before, beta_ptr_after,
            "beta's parse result should be cached since it was not modified"
        );

        // Alpha's parse result should reflect the new equation
        let alpha_src2 = state2.models["main"].variables["alpha"].source_var;
        let alpha_result = parse_source_variable(&db, alpha_src2, state2.project);
        if let Some(crate::ast::Ast::Scalar(crate::ast::Expr0::Const(_, val, _))) =
            alpha_result.variable.ast()
        {
            assert_eq!(*val, 42.0);
        } else {
            panic!(
                "Expected alpha to parse as Const(42.0), got {:?}",
                alpha_result.variable.ast()
            );
        }
    }

    #[test]
    fn test_incremental_sync_add_variable() {
        let mut db = SimlinDb::default();
        let project = simple_project();

        let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
        assert_eq!(state1.models["main"].variables.len(), 1);

        // Add a new variable
        let mut project2 = project.clone();
        project2.models[0]
            .variables
            .push(datamodel::Variable::Aux(datamodel::Aux {
                ident: "growth".to_string(),
                equation: datamodel::Equation::Scalar("0.1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }));

        let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
        assert_eq!(state2.models["main"].variables.len(), 2);
        assert!(state2.models["main"].variables.contains_key("growth"));

        // Original variable's handle should be preserved
        let pop1 = &state1.models["main"].variables["population"];
        let pop2 = &state2.models["main"].variables["population"];
        assert_eq!(
            pop1.source_var.as_id(),
            pop2.source_var.as_id(),
            "existing variable handle should be preserved when adding new variables"
        );
    }

    #[test]
    fn test_incremental_sync_remove_variable() {
        let mut db = SimlinDb::default();
        let mut project = simple_project();
        project.models[0]
            .variables
            .push(datamodel::Variable::Aux(datamodel::Aux {
                ident: "extra".to_string(),
                equation: datamodel::Equation::Scalar("99".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Private,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }));

        let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
        assert_eq!(state1.models["main"].variables.len(), 2);

        // Remove the "extra" variable
        project.models[0].variables.pop();
        let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));
        assert_eq!(state2.models["main"].variables.len(), 1);
        assert!(!state2.models["main"].variables.contains_key("extra"));
        assert!(state2.models["main"].variables.contains_key("population"));
    }

    #[test]
    fn test_incremental_sync_persistent_state_roundtrip() {
        let mut db = SimlinDb::default();
        let project = simple_project();

        // Create initial state
        let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

        // Sync again with no changes -- should preserve all handles
        let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));

        assert_eq!(
            state1.project.as_id(),
            state2.project.as_id(),
            "project handle should be stable"
        );
        assert_eq!(
            state1.models["main"].source_model.as_id(),
            state2.models["main"].source_model.as_id(),
            "model handle should be stable"
        );
        assert_eq!(
            state1.models["main"].variables["population"]
                .source_var
                .as_id(),
            state2.models["main"].variables["population"]
                .source_var
                .as_id(),
            "variable handle should be stable"
        );
    }

    #[test]
    fn test_persistent_state_to_sync_result() {
        let mut db = SimlinDb::default();
        let project = simple_project();

        let state = sync_from_datamodel_incremental(&mut db, &project, None);
        let sync = state.to_sync_result();

        assert_eq!(sync.project.name(&db), state.project.name(&db));
        assert_eq!(sync.models.len(), state.models.len());

        let main_model = &sync.models["main"];
        let persistent_main = &state.models["main"];
        assert_eq!(
            main_model.source.as_id(),
            persistent_main.source_model.as_id()
        );

        for (name, sv) in &main_model.variables {
            let pv = &persistent_main.variables[name];
            assert_eq!(sv.source.as_id(), pv.source_var.as_id());
            assert_eq!(sv.id.as_id(), pv.var_interned_id);
        }

        // Verify the reconstituted SyncResult works for diagnostic collection
        let diags = collect_all_diagnostics(&db, &sync);
        assert!(
            diags.is_empty(),
            "simple project should have no diagnostics"
        );
    }

    #[test]
    fn test_incremental_sync_successive_patches() {
        let mut db = SimlinDb::default();
        let mut project = simple_project();

        let state0 = sync_from_datamodel_incremental(&mut db, &project, None);

        // Prime parse cache
        let pop_src = state0.models["main"].variables["population"].source_var;
        let _ = parse_source_variable(&db, pop_src, state0.project);

        // Patch 1: change project name (shouldn't affect variable cache)
        project.name = "renamed".to_string();
        let state1 = sync_from_datamodel_incremental(&mut db, &project, Some(&state0));

        let pop_src1 = state1.models["main"].variables["population"].source_var;
        assert_eq!(
            pop_src.as_id(),
            pop_src1.as_id(),
            "variable handle should survive project name change"
        );

        // Patch 2: change the variable's equation
        project.models[0].variables[0].set_scalar_equation("999");
        let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));

        let pop_src2 = state2.models["main"].variables["population"].source_var;
        assert_eq!(
            pop_src.as_id(),
            pop_src2.as_id(),
            "handle should be stable even when equation changes"
        );

        // Parse should reflect the new equation
        let result = parse_source_variable(&db, pop_src2, state2.project);
        if let Some(crate::ast::Ast::Scalar(crate::ast::Expr0::Const(_, val, _))) =
            result.variable.ast()
        {
            assert_eq!(*val, 999.0);
        } else {
            panic!("Expected Const(999.0), got {:?}", result.variable.ast());
        }
    }
}
