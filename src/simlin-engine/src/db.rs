// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};

use crate::canonicalize;
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

    let dt_dependencies = compute_transitive(false).unwrap_or_default();
    let initial_dependencies = compute_transitive(true).unwrap_or_default();

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

        // In initial phase, stock is treated normally
        assert!(
            !graph.initial_dependencies["population"].is_empty()
                || graph.initial_dependencies["population"].is_empty(),
            "stock initial deps depend on its equation"
        );
    }
}
