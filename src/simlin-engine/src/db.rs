// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};

use salsa::Accumulator;
use salsa::plumbing::AsId;

use crate::canonicalize;
use crate::common::{Canonical, EquationError, Error, Ident, UnitError};
use crate::datamodel;

#[path = "db_ltm.rs"]
mod db_ltm;
use db_ltm::*;
pub use db_ltm::{LtmImplicitVarMeta, compile_ltm_var_fragment, model_ltm_implicit_var_info};

#[path = "db_analysis.rs"]
mod db_analysis;
use db_analysis::*;
pub use db_analysis::{
    CausalEdgesResult, CyclePartitionsResult, DetectedLoop, DetectedLoopPolarity,
    DetectedLoopsResult, LoopCircuitsResult, compute_link_polarities, model_causal_edges,
    model_cycle_partitions, model_detected_loops, model_loop_circuits,
};

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
#[derive(Clone, Debug, PartialEq, Eq, Hash, Copy)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub model: String,
    pub variable: Option<String>,
    pub error: DiagnosticError,
    pub severity: DiagnosticSeverity,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticError {
    Equation(EquationError),
    Model(Error),
    Unit(UnitError),
    Assembly(String),
}

// Thread-local flag tracking whether we are inside a salsa tracked
// function context. Used by `try_accumulate_diagnostic` to avoid calling
// `accumulate` outside a tracked context (which would panic). The
// previous `catch_unwind` approach does not work in WASM where
// `panic = "abort"` is set in the release profile.
thread_local! {
    static IN_TRACKED_CONTEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Attempt to push a diagnostic into the salsa accumulator. When called
/// inside a tracked function (indicated by the `IN_TRACKED_CONTEXT`
/// thread-local flag) the diagnostic is recorded normally. When called
/// outside any tracked context (e.g. from `compile_project_incremental`
/// which is a plain function) the push is silently skipped. This lets
/// `assemble_module` / `assemble_simulation` accumulate diagnostics when
/// invoked from tracked code while remaining safe to call from
/// non-tracked entry points (including WASM where `panic = "abort"`
/// makes `catch_unwind` ineffective).
///
/// Currently the flag is never set to `true`, so all calls are no-ops.
/// Assembly errors are returned via `Result::Err` from
/// `compile_project_incremental` and surfaced through
/// `gather_error_details_with_db` in the patch pipeline. The flag
/// exists as scaffolding for a future change where assembly functions
/// may be called from within a tracked context.
fn try_accumulate_diagnostic(db: &dyn Db, diag: Diagnostic) {
    let in_context = IN_TRACKED_CONTEXT.with(|flag| flag.get());
    if in_context {
        CompilationDiagnostic(diag).accumulate(db);
    }
    // Outside a tracked context the diagnostic is silently discarded.
    // The error is still returned as the function's Result::Err value
    // and handled by the caller.
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

/// Interned identity for a causal link between two variables.
/// Used as a key for per-link tracked functions.
#[salsa::interned(debug)]
pub struct LtmLinkId<'db> {
    #[returns(ref)]
    pub link_from: String,
    #[returns(ref)]
    pub link_to: String,
}

#[salsa::interned(debug)]
pub struct ModuleIdentContext<'db> {
    #[returns(ref)]
    pub idents: Vec<String>,
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
    /// Whether LTM (Loops That Matter) synthetic variable compilation is
    /// enabled. When true, `compute_layout` allocates slots and
    /// `assemble_module` compiles fragments for LTM variables.
    pub ltm_enabled: bool,
    /// When true, use discovery mode (`model_ltm_all_link_synthetic_variables`)
    /// which generates scores for every causal edge, not just edges in detected
    /// loops.
    pub ltm_discovery_mode: bool,
}

#[salsa::input]
pub struct SourceModel {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub variable_names: Vec<String>,
    #[returns(ref)]
    pub variables: HashMap<String, SourceVariable>,
    /// Per-model sim_specs override (None means use project-level specs)
    #[returns(ref)]
    pub model_sim_specs: Option<SourceSimSpecs>,
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
pub struct SourceDimensionMapping {
    pub target: String,
    pub element_map: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct SourceDimension {
    pub name: String,
    pub elements: SourceDimensionElements,
    pub maps_to: Option<String>,
    pub mappings: Vec<SourceDimensionMapping>,
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
    Arrayed(
        Vec<String>,
        Vec<SourceArrayedEquationElement>,
        Option<String>,
    ),
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
            maps_to: dim.maps_to().map(|s| s.to_owned()),
            mappings: dim
                .mappings
                .iter()
                .map(|m| SourceDimensionMapping {
                    target: m.target.clone(),
                    element_map: m.element_map.clone(),
                })
                .collect(),
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
            datamodel::Equation::Arrayed(dims, elements, default_eq) => SourceEquation::Arrayed(
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
                default_eq.clone(),
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
            // Prefer the richer mappings field; fall back to maps_to.
            let mappings = if !sd.mappings.is_empty() {
                sd.mappings
                    .iter()
                    .map(|m| datamodel::DimensionMapping {
                        target: m.target.clone(),
                        element_map: m.element_map.clone(),
                    })
                    .collect()
            } else if let Some(target) = sd.maps_to.clone() {
                vec![datamodel::DimensionMapping {
                    target,
                    element_map: vec![],
                }]
            } else {
                vec![]
            };
            datamodel::Dimension {
                name: sd.name.clone(),
                elements,
                mappings,
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
        SourceEquation::Arrayed(dims, elements, default_eq) => datamodel::Equation::Arrayed(
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
            default_eq.clone(),
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
    let mut compat = var.compat(db).clone();
    compat.non_negative = non_negative;
    compat.can_be_module_input = can_be_module_input;

    match var.kind(db) {
        SourceVariableKind::Stock => datamodel::Variable::Stock(datamodel::Stock {
            ident,
            equation,
            documentation: String::new(),
            units,
            inflows: var.inflows(db).clone(),
            outflows: var.outflows(db).clone(),
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
            compat,
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

/// Cached units context -- computed once per project, reused across all variables.
/// Subsumes the per-variable source_units_to_datamodel + source_sim_specs_to_datamodel +
/// Context::new_with_builtins calls.
///
/// Unit definition parsing errors are accumulated as diagnostics so they
/// appear in `collect_all_diagnostics`. Previously these were stored in
/// `engine::Project.errors` and walked by `collect_formatted_issues`.
#[salsa::tracked(returns(ref))]
pub fn project_units_context(db: &dyn Db, project: SourceProject) -> crate::units::Context {
    let dm_units = source_units_to_datamodel(project.units(db));
    let dm_sim_specs = source_sim_specs_to_datamodel(project.sim_specs(db));
    match crate::units::Context::new_with_builtins(&dm_units, &dm_sim_specs) {
        Ok(ctx) => ctx,
        Err(unit_parse_errors) => {
            // Accumulate each unit definition parsing error as a
            // project-level diagnostic (no model / variable).
            for (_unit_name, eq_errors) in &unit_parse_errors {
                for eq_err in eq_errors {
                    CompilationDiagnostic(Diagnostic {
                        model: String::new(),
                        variable: None,
                        error: DiagnosticError::Unit(crate::common::UnitError::DefinitionError(
                            eq_err.clone(),
                            None,
                        )),
                        severity: DiagnosticSeverity::Error,
                    })
                    .accumulate(db);
                }
            }
            Default::default()
        }
    }
}

/// Cached datamodel dimensions -- computed once per project.
#[salsa::tracked(returns(ref))]
pub fn project_datamodel_dims(db: &dyn Db, project: SourceProject) -> Vec<datamodel::Dimension> {
    source_dims_to_datamodel(project.dimensions(db))
}

fn parse_source_variable_impl(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_idents: Option<&HashSet<Ident<Canonical>>>,
) -> ParsedVariableResult {
    let dims = project_datamodel_dims(db, project);
    let units_ctx = project_units_context(db, project);
    let dm_var = reconstruct_variable(db, var);
    let mut implicit_vars = Vec::new();
    let variable = crate::variable::parse_var_with_module_context(
        dims,
        &dm_var,
        &mut implicit_vars,
        units_ctx,
        |mi| Ok(Some(mi.clone())),
        module_idents,
    );

    ParsedVariableResult {
        variable,
        implicit_vars,
    }
}

#[salsa::tracked(returns(ref))]
pub fn parse_source_variable(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> ParsedVariableResult {
    parse_source_variable_impl(db, var, project, None)
}
#[salsa::tracked(returns(ref))]
pub fn parse_source_variable_with_module_context<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_ident_context: ModuleIdentContext<'db>,
) -> ParsedVariableResult {
    let module_idents: HashSet<Ident<Canonical>> = module_ident_context
        .idents(db)
        .iter()
        .map(|ident| Ident::new(ident.as_str()))
        .collect();
    parse_source_variable_impl(db, var, project, Some(&module_idents))
}

fn module_ident_context_for_model<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    extra_module_idents: &[String],
) -> ModuleIdentContext<'db> {
    let source_vars = model.variables(db);
    let dm_vars: Vec<datamodel::Variable> = source_vars
        .values()
        .map(|source_var| reconstruct_variable(db, *source_var))
        .collect();
    let mut module_ident_list: Vec<String> = crate::model::collect_module_idents(&dm_vars)
        .into_iter()
        .map(|ident| ident.as_str().to_owned())
        .collect();
    module_ident_list.extend(
        extra_module_idents
            .iter()
            .map(|ident| canonicalize(ident).into_owned()),
    );
    module_ident_list.sort();
    module_ident_list.dedup();
    ModuleIdentContext::new(db, module_ident_list)
}

#[salsa::tracked]
fn model_module_ident_context<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    extra_module_idents: Vec<String>,
) -> ModuleIdentContext<'db> {
    module_ident_context_for_model(db, model, &extra_module_idents)
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ImplicitVarDeps {
    pub name: String,
    pub is_stock: bool,
    pub is_module: bool,
    pub model_name: Option<String>,
    pub dt_deps: BTreeSet<String>,
    pub initial_deps: BTreeSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct VariableDeps {
    /// Dependencies used during normal dt timestep calculations.
    pub dt_deps: BTreeSet<String>,
    /// Dependencies used during initial value calculations.
    pub initial_deps: BTreeSet<String>,
    /// Dependencies for implicit variables generated by builtin expansion
    /// (e.g., SMOOTH, DELAY create internal stocks).
    pub implicit_vars: Vec<ImplicitVarDeps>,
    /// Variables referenced by BuiltinFn::Init in this variable's equation.
    /// These must be included in the Initials runlist so their values are
    /// captured in the initial_values snapshot.
    pub init_referenced_vars: BTreeSet<String>,
}

fn canonical_module_input_set(module_input_names: &[String]) -> BTreeSet<Ident<Canonical>> {
    module_input_names
        .iter()
        .map(|name| Ident::new(canonicalize(name).as_ref()))
        .collect()
}

/// Collect variable identifiers referenced by BuiltinFn::Init calls in an AST.
/// Used to determine which variables need to be in the Initials runlist
/// so that INIT(x) can read their initial values.
pub(crate) fn init_referenced_idents(ast: &crate::ast::Ast<crate::ast::Expr2>) -> BTreeSet<String> {
    use crate::ast::Expr2;
    use crate::ast::IndexExpr2;
    use crate::builtins::{BuiltinContents, BuiltinFn, walk_builtin_expr};

    let mut result = BTreeSet::new();
    fn walk(expr: &Expr2, result: &mut BTreeSet<String>) {
        match expr {
            Expr2::Const(_, _, _) => {}
            Expr2::Var(_, _, _) => {}
            Expr2::App(builtin, _, _) => {
                // Check if this is Init specifically -- extract the referenced var
                if let BuiltinFn::Init(arg) = builtin {
                    match arg.as_ref() {
                        Expr2::Var(ident, _, _) | Expr2::Subscript(ident, _, _, _) => {
                            result.insert(ident.to_string());
                        }
                        _ => {}
                    }
                }
                // Recurse into all builtin subexpressions (handles nested Init too)
                walk_builtin_expr(builtin, |contents| match contents {
                    BuiltinContents::Ident(_, _) => {}
                    BuiltinContents::Expr(expr) => walk(expr, result),
                });
            }
            Expr2::Subscript(_, args, _, _) => {
                for arg in args {
                    match arg {
                        IndexExpr2::Expr(expr) | IndexExpr2::Range(expr, _, _) => {
                            walk(expr, result);
                        }
                        _ => {}
                    }
                    if let IndexExpr2::Range(_, end, _) = arg {
                        walk(end, result);
                    }
                }
            }
            Expr2::Op2(_, l, r, _, _) => {
                walk(l, result);
                walk(r, result);
            }
            Expr2::Op1(_, l, _, _) => {
                walk(l, result);
            }
            Expr2::If(cond, t, f, _, _) => {
                walk(cond, result);
                walk(t, result);
                walk(f, result);
            }
        }
    }
    match ast {
        crate::ast::Ast::Scalar(expr) => walk(expr, &mut result),
        crate::ast::Ast::ApplyToAll(_, expr) => walk(expr, &mut result),
        crate::ast::Ast::Arrayed(_, map, default_expr, _) => {
            for expr in map.values() {
                walk(expr, &mut result);
            }
            if let Some(expr) = default_expr {
                walk(expr, &mut result);
            }
        }
    }
    result
}

fn variable_direct_dependencies_impl(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
    module_ident_context: Option<ModuleIdentContext>,
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
                init_referenced_vars: BTreeSet::new(),
            }
        }
        _ => {
            let parsed = if let Some(module_ident_context) = module_ident_context {
                parse_source_variable_with_module_context(db, var, project, module_ident_context)
            } else {
                parse_source_variable(db, var, project)
            };
            let dims = source_dims_to_datamodel(project.dimensions(db));
            let dim_context = crate::dimensions::DimensionsContext::from(dims.as_slice());
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
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, module_inputs)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };

            let initial_deps = match lowered.init_ast() {
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, module_inputs)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };
            let implicit_vars =
                extract_implicit_var_deps(parsed, &dims, &dim_context, module_inputs);
            let init_referenced_vars = match lowered.ast() {
                Some(ast) => init_referenced_idents(ast),
                None => BTreeSet::new(),
            };

            VariableDeps {
                dt_deps,
                initial_deps,
                implicit_vars,
                init_referenced_vars,
            }
        }
    }
}

#[salsa::tracked(returns(ref))]
/// Default direct dependency extraction (no module-input specialization).
pub fn variable_direct_dependencies(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> VariableDeps {
    variable_direct_dependencies_impl(db, var, project, None, None)
}

/// Per-variable dependency extraction for a specific module input set.
///
/// This is required for module instances that use `isModuleInput(...)` in
/// their equations (for example stdlib DELAY/SMOOTH variants), where the
/// dependency set changes by instance wiring.
#[salsa::tracked(returns(ref))]
pub fn variable_direct_dependencies_with_inputs(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_input_names: Vec<String>,
) -> VariableDeps {
    let module_inputs = canonical_module_input_set(&module_input_names);
    variable_direct_dependencies_impl(db, var, project, Some(&module_inputs), None)
}

#[salsa::tracked(returns(ref))]
/// Dependency extraction using caller-provided module-ident context.
pub fn variable_direct_dependencies_with_context<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_ident_context: ModuleIdentContext<'db>,
) -> VariableDeps {
    variable_direct_dependencies_impl(db, var, project, None, Some(module_ident_context))
}

#[salsa::tracked(returns(ref))]
pub fn variable_direct_dependencies_with_context_and_inputs<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_ident_context: ModuleIdentContext<'db>,
    module_input_names: Vec<String>,
) -> VariableDeps {
    let module_inputs = canonical_module_input_set(&module_input_names);
    variable_direct_dependencies_impl(
        db,
        var,
        project,
        Some(&module_inputs),
        Some(module_ident_context),
    )
}

fn extract_implicit_var_deps(
    parsed: &ParsedVariableResult,
    dims: &[datamodel::Dimension],
    dim_context: &crate::dimensions::DimensionsContext,
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
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
            let is_module = matches!(implicit_var, datamodel::Variable::Module(_));
            let model_name = match implicit_var {
                datamodel::Variable::Module(m) => Some(m.model_name.clone()),
                _ => None,
            };

            // Module-type implicit vars have no AST -- extract deps from
            // their module reference src fields instead.
            if let datamodel::Variable::Module(m) = implicit_var {
                let refs: BTreeSet<String> = m
                    .references
                    .iter()
                    .map(|mr| canonicalize(&mr.src).into_owned())
                    .collect();
                return ImplicitVarDeps {
                    name: implicit_name,
                    is_stock: false,
                    is_module: true,
                    model_name: Some(m.model_name.clone()),
                    dt_deps: refs.clone(),
                    initial_deps: refs,
                };
            }

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
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, module_inputs)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };
            let initial = match lowered.init_ast() {
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, module_inputs)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };

            ImplicitVarDeps {
                name: implicit_name,
                is_stock: parsed_implicit.is_stock(),
                is_module,
                model_name,
                dt_deps: dt,
                initial_deps: initial,
            }
        })
        .collect()
}

/// Metadata for a single implicit variable generated by builtin expansion.
#[derive(Clone, PartialEq, Eq, salsa::Update)]
pub struct ImplicitVarMeta {
    pub parent_source_var: SourceVariable,
    pub index_in_parent: usize,
    pub is_stock: bool,
    pub is_module: bool,
    pub model_name: Option<String>,
    pub size: usize,
}

impl std::fmt::Debug for ImplicitVarMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImplicitVarMeta")
            .field("index_in_parent", &self.index_in_parent)
            .field("is_stock", &self.is_stock)
            .field("size", &self.size)
            .finish()
    }
}

/// Collect metadata about all implicit variables in a model.
/// The returned map is keyed by the canonical implicit variable name.
#[salsa::tracked(returns(ref))]
pub fn model_implicit_var_info(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<String, ImplicitVarMeta> {
    let source_vars = model.variables(db);
    let module_ident_context = module_ident_context_for_model(db, model, &[]);
    let mut result = HashMap::new();

    for source_var in source_vars.values() {
        let parsed = parse_source_variable_with_module_context(
            db,
            *source_var,
            project,
            module_ident_context,
        );
        for (index, implicit_var) in parsed.implicit_vars.iter().enumerate() {
            let name = canonicalize(implicit_var.get_ident()).into_owned();
            let is_stock = matches!(implicit_var, datamodel::Variable::Stock(_));
            let is_module = matches!(implicit_var, datamodel::Variable::Module(_));
            let model_name = match implicit_var {
                datamodel::Variable::Module(m) => Some(m.model_name.clone()),
                _ => None,
            };
            result.insert(
                name,
                ImplicitVarMeta {
                    parent_source_var: *source_var,
                    index_in_parent: index,
                    is_stock,
                    is_module,
                    model_name,
                    size: 1,
                },
            );
        }
    }

    result
}

#[salsa::tracked(returns(ref))]
pub fn model_module_map(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> {
    let source_vars = model.variables(db);
    let project_models = project.models(db);
    let model_name_ident: Ident<Canonical> = Ident::new(model.name(db));

    let mut all_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let mut current_mapping: HashMap<Ident<Canonical>, Ident<Canonical>> = HashMap::new();

    let mut sorted_names: Vec<&String> = source_vars.keys().collect();
    sorted_names.sort_unstable();

    for name in sorted_names {
        let svar = &source_vars[name];
        if svar.kind(db) == SourceVariableKind::Module {
            let sub_model_name_str = svar.model_name(db);
            let sub_model_ident: Ident<Canonical> = Ident::new(sub_model_name_str);
            let var_ident: Ident<Canonical> = Ident::new(name);
            current_mapping.insert(var_ident, sub_model_ident.clone());

            let sub_canonical = canonicalize(sub_model_name_str);
            if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                let sub_map = model_module_map(db, *sub_model, project);
                all_models.extend(sub_map.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
    }

    let implicit_vars = model_implicit_var_info(db, model, project);
    for (name, meta) in implicit_vars.iter() {
        if meta.is_module
            && let Some(sub_model_name) = &meta.model_name
        {
            let sub_model_ident: Ident<Canonical> = Ident::new(sub_model_name);
            let var_ident: Ident<Canonical> = Ident::new(name);
            current_mapping.insert(var_ident, sub_model_ident.clone());

            let sub_canonical = canonicalize(sub_model_name);
            if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                let sub_map = model_module_map(db, *sub_model, project);
                all_models.extend(sub_map.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
    }

    all_models.insert(model_name_ident, current_mapping);
    all_models
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ModelDepGraphResult {
    pub dt_dependencies: HashMap<String, BTreeSet<String>>,
    pub initial_dependencies: HashMap<String, BTreeSet<String>>,
    pub runlist_initials: Vec<String>,
    pub runlist_flows: Vec<String>,
    pub runlist_stocks: Vec<String>,
    pub has_cycle: bool,
}

fn model_dependency_graph_impl(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
) -> ModelDepGraphResult {
    let source_vars = model.variables(db);
    let module_input_names = module_input_names.to_vec();
    let module_ident_context = model_module_ident_context(db, model, module_input_names.clone());

    struct VarInfo {
        is_stock: bool,
        is_module: bool,
        dt_deps: BTreeSet<String>,
        initial_deps: BTreeSet<String>,
    }

    let mut var_info: HashMap<String, VarInfo> = HashMap::new();
    let mut all_init_referenced: HashSet<String> = HashSet::new();

    let normalize_dep = |dep: &str| -> String {
        let effective = dep.strip_prefix('\u{00B7}').unwrap_or(dep);
        if let Some(dot_pos) = effective.find('\u{00B7}') {
            effective[..dot_pos].to_string()
        } else {
            effective.to_string()
        }
    };
    let normalize_deps = |deps: &BTreeSet<String>| -> BTreeSet<String> {
        deps.iter().map(|d| normalize_dep(d)).collect()
    };

    let project_models = project.models(db);

    for (name, source_var) in source_vars.iter() {
        let deps = if module_input_names.is_empty() {
            variable_direct_dependencies_with_context(
                db,
                *source_var,
                project,
                module_ident_context,
            )
        } else {
            variable_direct_dependencies_with_context_and_inputs(
                db,
                *source_var,
                project,
                module_ident_context,
                module_input_names.clone(),
            )
        };
        let kind = source_var.kind(db);
        let dt_deps = if kind == SourceVariableKind::Module {
            deps.dt_deps
                .iter()
                .filter(|dep| {
                    let effective = dep.strip_prefix('\u{00B7}').unwrap_or(dep);
                    if let Some(dot_pos) = effective.find('\u{00B7}') {
                        let module_name = &effective[..dot_pos];
                        let var_name = &effective[dot_pos + '\u{00B7}'.len_utf8()..];
                        let sub_canonical = canonicalize(module_name);
                        if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                            let sub_vars = sub_model.variables(db);
                            if let Some(sub_var) = sub_vars.get(var_name) {
                                return sub_var.kind(db) != SourceVariableKind::Stock;
                            }
                        }
                        true
                    } else {
                        true
                    }
                })
                .cloned()
                .collect()
        } else {
            deps.dt_deps.clone()
        };

        var_info.insert(
            name.clone(),
            VarInfo {
                is_stock: kind == SourceVariableKind::Stock,
                is_module: kind == SourceVariableKind::Module,
                dt_deps: normalize_deps(&dt_deps),
                initial_deps: normalize_deps(&deps.initial_deps),
            },
        );
        all_init_referenced.extend(deps.init_referenced_vars.iter().cloned());

        // Include implicit variables from this variable's deps result.
        // Since we read this from variable_direct_dependencies (not
        // parse_source_variable), salsa's backdating ensures that if the
        // deps + implicit vars haven't changed, this function is cached.
        for implicit in &deps.implicit_vars {
            var_info.insert(
                implicit.name.clone(),
                VarInfo {
                    is_stock: implicit.is_stock,
                    is_module: implicit.is_module,
                    dt_deps: normalize_deps(&implicit.dt_deps),
                    initial_deps: normalize_deps(&implicit.initial_deps),
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

    let mut has_cycle = false;
    let dt_dependencies = compute_transitive(false).unwrap_or_else(|var_name| {
        has_cycle = true;
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var_name),
            error: DiagnosticError::Model(crate::common::Error {
                kind: crate::common::ErrorKind::Model,
                code: crate::common::ErrorCode::CircularDependency,
                details: None,
            }),
            severity: DiagnosticSeverity::Error,
        })
        .accumulate(db);
        HashMap::new()
    });
    let initial_dependencies = compute_transitive(true).unwrap_or_else(|var_name| {
        has_cycle = true;
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var_name),
            error: DiagnosticError::Model(crate::common::Error {
                kind: crate::common::ErrorKind::Model,
                code: crate::common::ErrorCode::CircularDependency,
                details: None,
            }),
            severity: DiagnosticSeverity::Error,
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
            // Build the allowed set: only variables in the filtered input list
            // should appear in the output. Dependencies are used solely for
            // ordering, not for expanding the set.
            let allowed: HashSet<&str> = names.iter().map(|n| n.as_str()).collect();
            let mut result: Vec<String> = Vec::new();
            let mut used: HashSet<String> = HashSet::new();

            fn add(
                deps: &HashMap<String, BTreeSet<String>>,
                allowed: &HashSet<&str>,
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
                        add(deps, allowed, result, used, dep);
                    }
                }
                // Only include variables that were in the original filtered list
                if allowed.contains(name) {
                    result.push(name.to_string());
                }
            }

            for name in names {
                add(deps, &allowed, &mut result, &mut used, name);
            }
            result
        };

    // Initials runlist: stocks, modules, INIT-referenced vars, and their
    // transitive deps.
    //
    // Module variables have their transitive deps short-circuited in
    // compute_transitive (only direct deps are stored). The deps of those
    // direct deps (e.g. an implicit intermediate variable depending on a
    // regular model variable) ARE fully expanded in initial_dependencies.
    // We must transitively close init_set so that every variable needed
    // during the initials phase is included in the allowed set for
    // topo_sort_str.
    //
    // Variables referenced by INIT() must also be seeded into the needed
    // set. Without this, aux-only models (no stocks/modules) using INIT(x)
    // would have an empty Initials runlist, and initial_values[x_offset]
    // would stay at zero.
    let runlist_initials = {
        use std::collections::HashSet;
        let needed: HashSet<&String> = var_names
            .iter()
            .filter(|n| {
                var_info
                    .get(n.as_str())
                    .map(|i| i.is_stock || i.is_module)
                    .unwrap_or(false)
                    || all_init_referenced.contains(n.as_str())
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
        // Transitively close: each item added to init_set may itself
        // have deps that also need to be in the initials runlist.
        loop {
            let additional: HashSet<&String> = init_set
                .iter()
                .flat_map(|n| {
                    initial_dependencies
                        .get(n.as_str())
                        .into_iter()
                        .flat_map(|deps| deps.iter())
                })
                .filter(|d| !init_set.contains(d))
                .collect();
            if additional.is_empty() {
                break;
            }
            init_set.extend(additional);
        }
        let init_list: Vec<&String> = init_set.into_iter().collect();
        topo_sort_str(init_list, &initial_dependencies)
    };

    // Flows runlist: non-stock variables, modules, AND stock-typed module inputs.
    // The monolithic path uses `instantiation.contains(id) || !var.is_stock()`
    // which includes stock-typed module inputs (e.g., a stock declared with
    // access="input" in XMILE). These need LoadModuleInput -> AssignCurr in
    // the flows phase to propagate the parent-provided value each timestep.
    let module_input_set: BTreeSet<String> = module_input_names
        .iter()
        .map(|s| canonicalize(s).into_owned())
        .collect();
    let runlist_flows = {
        let flow_names: Vec<&String> = var_names
            .iter()
            .filter(|n| {
                let is_input = module_input_set.contains(canonicalize(n).as_ref());
                var_info
                    .get(n.as_str())
                    .map(|i| is_input || !i.is_stock)
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
        has_cycle,
    }
}

/// Per-model tracked dependency graph for a specific module-input set.
///
/// Models instantiated with different input wiring can have different
/// dependency sets when `isModuleInput(...)` appears in equations.
#[salsa::tracked(returns(ref))]
pub fn model_dependency_graph_with_inputs(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    module_input_names: Vec<String>,
) -> ModelDepGraphResult {
    model_dependency_graph_impl(db, model, project, &module_input_names)
}

/// Default per-model tracked dependency graph (no module inputs).
#[salsa::tracked(returns(ref))]
pub fn model_dependency_graph(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> ModelDepGraphResult {
    model_dependency_graph_impl(db, model, project, &[])
}

// ── Diagnostic collection ──────────────────────────────────────────────

/// Per-model tracked function that performs unit inference and checking,
/// accumulating unit warnings/errors through the salsa accumulator.
///
/// Builds temporary ModelStage0/ModelStage1 representations from the
/// salsa-cached parsed variables, then runs the same unit inference and
/// checking pipeline as the old `run_default_model_checks` callback.
/// Unit mismatches are accumulated as DiagnosticSeverity::Warning to
/// match the old-path behavior where unit issues don't block simulation.
///
/// Stdlib (implicit) models are skipped because they are generic
/// templates that only make sense when instantiated with specific inputs.
#[salsa::tracked]
pub fn check_model_units(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use crate::common::{Canonical, ErrorCode, ErrorKind, Ident};
    use crate::dimensions::DimensionsContext;
    use crate::model::{ModelStage0, ModelStage1, ScopeStage0, VariableStage0};

    // Skip stdlib models -- they are generic and unit checking doesn't
    // apply until instantiated with concrete inputs. Stdlib model names
    // start with the "stdlib\u{205A}" prefix (two-dot punctuation separator).
    if model.name(db).starts_with("stdlib\u{205A}") {
        return;
    }

    let model_name = model.name(db).clone();
    let units_ctx = project_units_context(db, project);
    let dm_dims = project_datamodel_dims(db, project);
    let dim_context = DimensionsContext::from(dm_dims.as_slice());

    // Helper: build a ModelStage0 from a SourceModel's parsed variables.
    let build_model_s0 = |src_model: &SourceModel, is_stdlib: bool| -> ModelStage0 {
        let src_vars = src_model.variables(db);
        let mut var_list: Vec<VariableStage0> = Vec::new();
        let mut implicit_dm: Vec<datamodel::Variable> = Vec::new();
        for (_name, svar) in src_vars.iter() {
            let parsed = parse_source_variable(db, *svar, project);
            var_list.push(parsed.variable.clone());
            implicit_dm.extend(parsed.implicit_vars.iter().cloned());
        }
        // Parse implicit vars (SMOOTH/DELAY expansion).
        let mut dummy: Vec<datamodel::Variable> = Vec::new();
        var_list.extend(implicit_dm.into_iter().map(|dm_var| {
            crate::variable::parse_var(dm_dims, &dm_var, &mut dummy, units_ctx, |mi| {
                Ok(Some(mi.clone()))
            })
        }));
        let variables: HashMap<Ident<Canonical>, VariableStage0> = var_list
            .into_iter()
            .map(|v| (Ident::new(v.ident()), v))
            .collect();
        ModelStage0 {
            ident: Ident::new(src_model.name(db)),
            display_name: src_model.name(db).clone(),
            variables,
            errors: None,
            implicit: is_stdlib,
        }
    };

    // Build ModelStage0 for all project models so that cross-module unit
    // inference constraints (module inputs/outputs) can resolve submodel
    // variable types. Stdlib models are included in the map because user
    // models may reference them as modules.
    let project_models = project.models(db);
    let mut all_s0: Vec<ModelStage0> = Vec::new();
    for (name, src_model) in project_models.iter() {
        let is_stdlib = name.starts_with("stdlib\u{205A}");
        all_s0.push(build_model_s0(src_model, is_stdlib));
    }

    let models_s0: HashMap<Ident<Canonical>, &ModelStage0> =
        all_s0.iter().map(|m| (m.ident.clone(), m)).collect();

    // Lower all ModelStage0 -> ModelStage1.
    let all_s1: Vec<ModelStage1> = all_s0
        .iter()
        .map(|ms0| {
            let scope = ScopeStage0 {
                models: &models_s0,
                dimensions: &dim_context,
                model_name: ms0.ident.as_str(),
            };
            ModelStage1::new(&scope, ms0)
        })
        .collect();

    let models_s1: HashMap<Ident<Canonical>, &ModelStage1> =
        all_s1.iter().map(|m| (m.name.clone(), m)).collect();

    // Find the target model in the lowered map.
    let target_ident = Ident::<Canonical>::new(&model_name);
    let target_model = match models_s1.get(&target_ident) {
        Some(m) => *m,
        None => return,
    };

    // Check whether the model declares units on any variable. If not,
    // skip surfacing inference errors (the model wasn't designed with
    // dimensional analysis in mind).
    let has_declared_units = target_model
        .variables
        .values()
        .any(|var| var.units().is_some());

    // Run unit inference.
    let inferred_units = crate::units_infer::infer(&models_s1, units_ctx, target_model)
        .unwrap_or_else(|err| {
            if has_declared_units
                && let crate::common::UnitError::InferenceError { code, .. } = &err
                && *code == ErrorCode::UnitMismatch
            {
                CompilationDiagnostic(Diagnostic {
                    model: model_name.clone(),
                    variable: None,
                    error: DiagnosticError::Model(crate::common::Error {
                        kind: ErrorKind::Model,
                        code: *code,
                        details: Some(format!("{}", err)),
                    }),
                    severity: DiagnosticSeverity::Warning,
                })
                .accumulate(db);
            }
            Default::default()
        });

    // Run unit checking.
    match crate::units_check::check(units_ctx, &inferred_units, target_model) {
        Ok(Ok(())) => {}
        Ok(Err(errors)) => {
            for (ident, err) in errors.into_iter() {
                CompilationDiagnostic(Diagnostic {
                    model: model_name.clone(),
                    variable: Some(ident.to_string()),
                    error: DiagnosticError::Unit(err),
                    severity: DiagnosticSeverity::Warning,
                })
                .accumulate(db);
            }
        }
        Err(err) => {
            CompilationDiagnostic(Diagnostic {
                model: model_name.clone(),
                variable: None,
                error: DiagnosticError::Model(crate::common::Error {
                    kind: ErrorKind::Model,
                    code: ErrorCode::Generic,
                    details: Some(format!("unit checking failed: {}", err)),
                }),
                severity: DiagnosticSeverity::Warning,
            })
            .accumulate(db);
        }
    }
}

/// Per-model tracked function that triggers diagnostic accumulation from
/// all compilation stages. The salsa accumulator is the sole error source
/// for diagnostic reporting -- this function does not read struct fields.
///
/// Triggers two diagnostic sources:
/// 1. `compile_var_fragment` for each variable -- accumulates parse-level
///    equation errors (EmptyEquation, syntax errors), unit definition
///    syntax errors (bad unit strings), and compilation-level errors
///    (BadTable, MismatchedDimensions, etc.)
/// 2. `check_model_units` -- accumulates unit inference/checking warnings
#[salsa::tracked]
pub fn model_all_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    let source_vars = model.variables(db);

    // Trigger compile_var_fragment for each variable. This is a superset
    // of parse_source_variable: it first accumulates unit definition
    // syntax errors from the parsed variable, then checks for equation
    // parse errors, then proceeds with compilation which can surface
    // additional errors like BadTable, MismatchedDimensions, etc.
    //
    // We use is_root: true and empty module_input_names for diagnostic
    // purposes. The is_root flag only affects offset layout (whether
    // implicit time/dt vars are included); using true ensures variables
    // referencing TIME or DT don't produce false-positive missing-ref
    // errors. The module_input_names are empty because we are not in an
    // assembly context -- this is purely for error detection.
    for (_var_name, source_var) in source_vars.iter() {
        let _fragment = compile_var_fragment(db, *source_var, model, project, true, vec![]);
    }

    // Trigger unit checking. This is a separate tracked function so
    // that unit inference results are individually cached and
    // invalidated only when unit-relevant inputs change.
    check_model_units(db, model, project);
}

// ── LTM tracked functions ──────────────────────────────────────────────

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

/// Compute the link score equation text for a single causal link.
///
/// This is the per-link granularity that enables incremental recomputation:
/// when a variable's equation changes, salsa only re-evaluates link score
/// equations for links whose endpoints are affected. Links involving
/// unmodified variables return their cached equation text.
#[salsa::tracked(returns(ref))]
pub fn link_score_equation_text<'db>(
    db: &'db dyn Db,
    link_id: LtmLinkId<'db>,
    model: SourceModel,
    project: SourceProject,
) -> Option<LtmSyntheticVar> {
    use crate::common::{Canonical, Ident};

    let from_name = link_id.link_from(db);
    let to_name = link_id.link_to(db);
    let from_ident = Ident::<Canonical>::new(from_name);
    let to_ident = Ident::<Canonical>::new(to_name);

    let from_var = reconstruct_single_variable(db, model, project, from_name);
    let to_var = reconstruct_single_variable(db, model, project, to_name)?;

    let var_name = format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
        from_name, to_name
    );

    let from_is_module = from_var.as_ref().is_some_and(|v| v.is_module());
    let to_is_module = to_var.is_module();

    let equation = if !from_is_module && to_is_module {
        // input_src -> module: use composite reference if available
        if let crate::variable::Variable::Module {
            model_name, inputs, ..
        } = &to_var
        {
            let composite_ports = get_stdlib_composite_ports();
            let port = inputs.iter().find(|i| i.src == from_ident).map(|i| &i.dst);
            let has_composite = port.is_some()
                && composite_ports
                    .get(model_name)
                    .is_some_and(|ports| ports.contains(port.unwrap()));

            if let (true, Some(port)) = (has_composite, port) {
                crate::ltm_augment::generate_module_input_link_score_eq(&to_ident, port)
            } else {
                crate::ltm_augment::generate_module_link_score_eq(&from_ident, &to_ident)
            }
        } else {
            crate::ltm_augment::generate_module_link_score_eq(&from_ident, &to_ident)
        }
    } else if from_is_module && !to_is_module {
        // module -> downstream: standard ceteris-paribus formula
        let mut all_vars = HashMap::new();
        if let Some(ref fv) = from_var {
            all_vars.insert(from_ident.clone(), fv.clone());
        }
        all_vars.insert(to_ident.clone(), to_var.clone());
        crate::ltm_augment::generate_link_score_equation_for_link(
            &from_ident,
            &to_ident,
            &to_var,
            &all_vars,
        )
    } else if from_is_module && to_is_module {
        // module -> module: no downstream equation to analyze
        crate::ltm_augment::generate_module_link_score_eq(&from_ident, &to_ident)
    } else {
        // Non-module link: standard ceteris-paribus formula
        let mut all_vars = HashMap::new();
        if let Some(ref fv) = from_var {
            all_vars.insert(from_ident.clone(), fv.clone());
        }
        all_vars.insert(to_ident.clone(), to_var.clone());
        crate::ltm_augment::generate_link_score_equation_for_link(
            &from_ident,
            &to_ident,
            &to_var,
            &all_vars,
        )
    };

    Some(LtmSyntheticVar {
        name: var_name,
        equation,
    })
}

/// Compute the internal link score equation text for a single causal link
/// inside a stdlib dynamic module (e.g. SMOOTH, DELAY).
///
/// Uses `$⁚ltm⁚ilink⁚` prefix instead of `$⁚ltm⁚link_score⁚`.
#[salsa::tracked(returns(ref))]
pub fn module_ilink_equation_text<'db>(
    db: &'db dyn Db,
    link_id: LtmLinkId<'db>,
    model: SourceModel,
    project: SourceProject,
) -> Option<LtmSyntheticVar> {
    use crate::common::{Canonical, Ident};

    let from_name = link_id.link_from(db);
    let to_name = link_id.link_to(db);
    let from_ident = Ident::<Canonical>::new(from_name);
    let to_ident = Ident::<Canonical>::new(to_name);

    let variables = reconstruct_model_variables(db, model, project);

    let to_var = variables.get(&to_ident)?;

    let var_name = format!(
        "$\u{205A}ltm\u{205A}ilink\u{205A}{}\u{2192}{}",
        from_name, to_name
    );

    let equation = crate::ltm_augment::generate_link_score_equation_for_link(
        &from_ident,
        &to_ident,
        to_var,
        &variables,
    );

    Some(LtmSyntheticVar {
        name: var_name,
        equation,
    })
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
                "smooth", "delay1", "delay3", "trend", "previous",
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
/// skipped when deps unchanged), then delegates per-link score generation
/// to `link_score_equation_text` so that equation edits only regenerate
/// scores for affected links.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_synthetic_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LtmVariablesResult {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{CyclePartitions, Loop, assign_loop_ids};
    use std::collections::HashSet;

    let circuits_result = model_loop_circuits(db, model, project);
    if circuits_result.circuits.is_empty() {
        return LtmVariablesResult { vars: vec![] };
    }

    let partitions_result = model_cycle_partitions(db, model, project);
    let edges_result = model_causal_edges(db, model, project);

    // Reconstruct Variable objects for polarity analysis
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

    // Collect unique link endpoints from loops, then compute per-link equations
    let mut seen_links: HashSet<(String, String)> = HashSet::new();
    let mut vars = Vec::new();

    for loop_item in &loops {
        for link in &loop_item.links {
            let key = (link.from.to_string(), link.to.to_string());
            if seen_links.insert(key) {
                let link_id = LtmLinkId::new(db, link.from.to_string(), link.to.to_string());
                if let Some(lsv) = link_score_equation_text(db, link_id, model, project) {
                    vars.push(lsv.clone());
                }
            }
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

    // Loop scores and relative loop scores are pure functions of link
    // score variable names (not equations), so they don't benefit from
    // per-link caching and are generated in bulk.
    let loop_vars = crate::ltm_augment::generate_loop_score_variables(&loops, &partitions);
    for (name, var) in loop_vars {
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
/// Delegates per-link computation to `link_score_equation_text`.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_all_link_synthetic_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LtmVariablesResult {
    let edges_result = model_causal_edges(db, model, project);

    // Iterate all causal edges and compute per-link equations
    let mut vars = Vec::new();
    for (from, tos) in &edges_result.edges {
        for to in tos {
            let link_id = LtmLinkId::new(db, from.clone(), to.clone());
            if let Some(lsv) = link_score_equation_text(db, link_id, model, project) {
                vars.push(lsv.clone());
            }
        }
    }

    vars.sort_by(|a, b| a.name.cmp(&b.name));
    LtmVariablesResult { vars }
}

/// Generate internal LTM variables for a stdlib dynamic module.
///
/// Since stdlib models are static, this computes once and caches forever.
/// Delegates per-link ilink computation to `module_ilink_equation_text`.
#[salsa::tracked(returns(ref))]
pub fn module_ltm_synthetic_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LtmVariablesResult {
    use crate::common::{Canonical, Ident};

    // Check if this is a dynamic module (has stocks) via the edge result
    let edges_result = model_causal_edges(db, model, project);
    if edges_result.stocks.is_empty() {
        return LtmVariablesResult { vars: vec![] };
    }

    // Build CausalGraph for pathway enumeration
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

    let variables = reconstruct_model_variables(db, model, project);
    let graph = crate::ltm::CausalGraph {
        edges: graph_edges,
        stocks,
        variables,
        module_graphs: HashMap::new(),
    };

    // Generate internal link scores via per-link tracked function
    let mut vars = Vec::new();
    for (from, tos) in &edges_result.edges {
        for to in tos {
            let link_id = LtmLinkId::new(db, from.clone(), to.clone());
            if let Some(lsv) = module_ilink_equation_text(db, link_id, model, project) {
                vars.push(lsv.clone());
            }
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
    pub is_stdlib: bool,
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
#[derive(Clone)]
pub struct PersistentSyncState {
    pub project: SourceProject,
    pub models: HashMap<String, PersistentModelState>,
}

#[derive(Clone)]
pub struct PersistentModelState {
    /// Lifetime-erased `ModelId<'db>` (interned, carries `'db`)
    pub model_interned_id: salsa::Id,
    pub source_model: SourceModel,
    pub variables: HashMap<String, PersistentVariableState>,
    /// True when this entry came from the stdlib, false for user-defined models.
    pub is_stdlib: bool,
}

#[derive(Clone)]
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
                            is_stdlib: pm.is_stdlib,
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
                            is_stdlib: sm.is_stdlib,
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
        let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
        variable_names.sort();

        let model_sim_specs = dm_model.sim_specs.as_ref().map(SourceSimSpecs::from);
        let source_model = SourceModel::new(
            db,
            dm_model.name.clone(),
            variable_names,
            source_var_map,
            model_sim_specs,
        );

        source_model_map.insert(canonical_model_name.clone(), source_model);

        models.insert(
            canonical_model_name,
            SyncedModel {
                id: model_id,
                source: source_model,
                variables,
                is_stdlib: false,
            },
        );
    }

    // Add stdlib models so incremental compilation can find them
    // when resolving implicit module references (DELAY, SMOOTH, etc.).
    let mut model_names = model_names;
    for stdlib_name in crate::stdlib::MODEL_NAMES {
        let full_name = format!("stdlib\u{205A}{stdlib_name}");
        let canonical = canonicalize(&full_name).into_owned();
        if source_model_map.contains_key(&canonical) {
            continue;
        }
        let dm_model = crate::stdlib::get(stdlib_name).unwrap();
        let model_id = ModelId::new(db, canonical.clone());
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
        let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
        variable_names.sort();
        let source_model = SourceModel::new(
            db,
            full_name.clone(),
            variable_names,
            source_var_map,
            dm_model.sim_specs.as_ref().map(SourceSimSpecs::from),
        );
        source_model_map.insert(canonical.clone(), source_model);
        models.insert(
            canonical,
            SyncedModel {
                id: model_id,
                source: source_model,
                variables,
                is_stdlib: true,
            },
        );
        model_names.push(full_name);
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
        false,
        false,
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
        datamodel::Variable::Stock(s) => s.compat.non_negative,
        datamodel::Variable::Flow(f) => f.compat.non_negative,
        _ => false,
    };

    let can_be_module_input = var.can_be_module_input();

    let compat = match var {
        datamodel::Variable::Stock(s) => s.compat.clone(),
        datamodel::Variable::Flow(f) => f.compat.clone(),
        datamodel::Variable::Aux(a) => a.compat.clone(),
        datamodel::Variable::Module(m) => m.compat.clone(),
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
        datamodel::Variable::Stock(s) => s.compat.non_negative,
        datamodel::Variable::Flow(f) => f.compat.non_negative,
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
        datamodel::Variable::Module(m) => m.compat.clone(),
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

    // model_names updated below after stdlib models are added

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

            let new_model_sim_specs = dm_model.sim_specs.as_ref().map(SourceSimSpecs::from);
            if *source_model.model_sim_specs(&*db) != new_model_sim_specs {
                source_model.set_model_sim_specs(db).to(new_model_sim_specs);
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
            let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
            variable_names.sort();

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
                    is_stdlib: false,
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
            let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
            variable_names.sort();

            let model_sim_specs = dm_model.sim_specs.as_ref().map(SourceSimSpecs::from);
            let source_model = SourceModel::new(
                &*db,
                dm_model.name.clone(),
                variable_names,
                source_var_map,
                model_sim_specs,
            );

            new_models.insert(
                canonical_model_name,
                PersistentModelState {
                    model_interned_id: model_id.as_id(),
                    source_model,
                    variables: new_vars,
                    is_stdlib: false,
                },
            );
        }
    }

    // Add stdlib models, reusing prev_state handles when available so
    // salsa recognizes unchanged stdlib inputs.
    for stdlib_name in crate::stdlib::MODEL_NAMES {
        let full_name = format!("stdlib\u{205A}{stdlib_name}");
        let canonical = canonicalize(&full_name).into_owned();
        if new_models.contains_key(&canonical) {
            continue;
        }
        if let Some(prev_model) = prev.models.get(&canonical).filter(|pm| pm.is_stdlib) {
            new_models.insert(canonical, prev_model.clone());
        } else {
            let dm_model = crate::stdlib::get(stdlib_name).unwrap();
            let model_id = ModelId::new(&*db, canonical.clone());
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
            let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
            variable_names.sort();
            let source_model = SourceModel::new(
                &*db,
                full_name.clone(),
                variable_names,
                source_var_map,
                dm_model.sim_specs.as_ref().map(SourceSimSpecs::from),
            );
            new_models.insert(
                canonical,
                PersistentModelState {
                    model_interned_id: model_id.as_id(),
                    source_model,
                    variables: new_vars,
                    is_stdlib: true,
                },
            );
        }
    }

    // Update model_names to include stdlib
    let mut new_model_names: Vec<String> = project.models.iter().map(|m| m.name.clone()).collect();
    for stdlib_name in crate::stdlib::MODEL_NAMES {
        let full_name = format!("stdlib\u{205A}{stdlib_name}");
        let canonical = canonicalize(&full_name).into_owned();
        if new_models.contains_key(&canonical) {
            new_model_names.push(full_name);
        }
    }
    if *source_project.model_names(&*db) != new_model_names {
        source_project.set_model_names(db).to(new_model_names);
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

#[salsa::tracked]
pub fn variable_size(db: &dyn Db, var: SourceVariable, project: SourceProject) -> usize {
    let dims = variable_dimensions(db, var, project);
    if dims.is_empty() {
        1
    } else {
        dims.iter().map(|d| d.len()).product()
    }
}

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

    // Include implicit variables (generated by SMOOTH, DELAY, TREND builtins)
    // after all explicit variables.
    let implicit_info = model_implicit_var_info(db, model, project);
    let mut implicit_names: Vec<&String> = implicit_info.keys().collect();
    implicit_names.sort_unstable();
    for name in implicit_names {
        let info = &implicit_info[name];
        let size = if info.is_module {
            if let Some(sub_model_name) = &info.model_name {
                let sub_canonical = canonicalize(sub_model_name);
                project_models
                    .get(sub_canonical.as_ref())
                    .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                    .unwrap_or(info.size)
            } else {
                info.size
            }
        } else {
            info.size
        };
        entries.insert(name.clone(), LayoutEntry { offset, size });
        offset += size;
    }

    // Section 3: LTM synthetic variables (only when ltm_enabled).
    // LTM vars are always scalar aux equations occupying 1 slot each.
    // When ltm_enabled is false, this section is skipped entirely (zero
    // overhead). When the model has no feedback loops,
    // model_ltm_synthetic_variables returns an empty list (also zero
    // overhead).
    //
    // LTM variables only exist in the root model. Stdlib sub-models
    // (previous, init, smth1, etc.) have no feedback loops of their own
    // and must not enter LTM resolution, which would cause a salsa
    // dependency cycle (compute_layout -> model_ltm_implicit_var_info
    // -> compute_layout for the stdlib model).
    if is_root && project.ltm_enabled(db) {
        let ltm_vars = if project.ltm_discovery_mode(db) {
            model_ltm_all_link_synthetic_variables(db, model, project)
        } else {
            model_ltm_synthetic_variables(db, model, project)
        };
        let mut ltm_names: Vec<&str> = ltm_vars.vars.iter().map(|v| v.name.as_str()).collect();
        ltm_names.sort_unstable();
        for name in ltm_names {
            entries.insert(name.to_string(), LayoutEntry { offset, size: 1 });
            offset += 1;
        }

        // Section 3b: Implicit variables generated by LTM equation
        // parsing (PREVIOUS module instances). These stdlib modules
        // need their own slots in the parent model's layout.
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let mut ltm_im_names: Vec<&String> = ltm_implicit.keys().collect();
        ltm_im_names.sort_unstable();
        for name in ltm_im_names {
            let meta = &ltm_implicit[name];
            entries.insert(
                name.clone(),
                LayoutEntry {
                    offset,
                    size: meta.size,
                },
            );
            offset += meta.size;
        }
    }

    VariableLayout::new(entries, offset)
}

/// Extract compiler::Table data directly from a SourceVariable's graphical
/// function fields. Used to populate the mini-Module's tables map for
/// dependency variables that define lookup tables.
fn extract_tables_from_source_var(
    db: &dyn Db,
    source_var: &SourceVariable,
) -> Vec<crate::compiler::Table> {
    let ident = source_var.ident(db);
    let eq = source_var.equation(db);

    // For arrayed equations with per-element graphical functions, build one
    // table per element (matching variable.rs build_tables).  Elements without
    // a GF get an empty placeholder so that table[element_offset] stays aligned.
    if let SourceEquation::Arrayed(_, elements, _) = eq {
        let has_element_gfs = elements.iter().any(|e| e.gf.is_some());
        if has_element_gfs {
            return elements
                .iter()
                .map(|e| {
                    e.gf.as_ref()
                        .and_then(|gf| {
                            let dm_gf = source_gf_to_datamodel(gf);
                            let var_table = crate::variable::parse_table(&Some(dm_gf)).ok()??;
                            crate::compiler::Table::new(ident, &var_table).ok()
                        })
                        .unwrap_or(crate::compiler::Table { data: vec![] })
                })
                .collect();
        }
    }

    // Scalar or apply-to-all: use the variable-level graphical function.
    let gf = source_var.gf(db);
    match gf {
        Some(sgf) => {
            let dm_gf = source_gf_to_datamodel(sgf);
            crate::variable::parse_table(&Some(dm_gf))
                .ok()
                .flatten()
                .and_then(|vt| crate::compiler::Table::new(ident, &vt).ok())
                .into_iter()
                .collect()
        }
        None => vec![],
    }
}

/// Build module input mappings from raw (src, dst) reference pairs.
///
/// Filters out references where src is an internal module input (starts
/// with the module's own prefix), strips the module prefix from dst,
/// and strips leading middots from src in the "main" model (where parent
/// scope refs are represented as `·var` after canonicalization).
fn build_module_inputs<S1: AsRef<str>, S2: AsRef<str>>(
    model_name: &str,
    module_var_prefix: &str,
    refs: impl Iterator<Item = (S1, S2)>,
) -> Vec<crate::variable::ModuleInput> {
    refs.filter_map(|(src, dst)| {
        let src = src.as_ref();
        let dst = dst.as_ref();
        // Skip internal module inputs (src within the module's own namespace)
        if src.starts_with(module_var_prefix) {
            return None;
        }
        let dst_stripped = dst.strip_prefix(module_var_prefix)?;
        let src_str = if model_name == "main" && src.starts_with('\u{00B7}') {
            &src['\u{00B7}'.len_utf8()..]
        } else {
            src
        };
        Some(crate::variable::ModuleInput {
            src: Ident::new(src_str),
            dst: Ident::new(dst_stripped),
        })
    })
    .collect()
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

/// Populate sub-model metadata in `all_metadata` for module variable compilation.
/// Mirrors the monolithic `build_metadata` but works with salsa SourceModel/SourceVariable.
/// Recursively populates metadata for nested modules.
fn build_submodel_metadata<'arena>(
    arena: &'arena bumpalo::Bump,
    db: &dyn Db,
    sub_model: SourceModel,
    project: SourceProject,
    all_metadata: &mut HashMap<
        Ident<Canonical>,
        HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'arena>>,
    >,
) {
    let sub_model_name: Ident<Canonical> = Ident::new(sub_model.name(db));

    if all_metadata.contains_key(&sub_model_name) {
        return;
    }

    let layout = compute_layout(db, sub_model, project, false);
    let source_vars = sub_model.variables(db);
    let project_models = project.models(db);

    let mut sub_metadata: HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'arena>> =
        HashMap::new();

    let mut sorted_names: Vec<&String> = source_vars.keys().collect();
    sorted_names.sort_unstable();

    for name in &sorted_names {
        let svar = &source_vars[name.as_str()];
        let var_ident: Ident<Canonical> = Ident::new(name.as_str());
        let entry = layout.get(name.as_str());
        let (offset, size) = entry.map_or((0, 1), |e| (e.offset, e.size));

        // Build a stub variable with correct dimensions for the sub-model context
        let dims = variable_dimensions(db, *svar, project);
        let stub = build_stub_variable(db, svar, &var_ident, dims);
        let stub: &'arena crate::variable::Variable = arena.alloc(stub);

        sub_metadata.insert(
            var_ident.clone(),
            crate::compiler::VariableMetadata {
                offset,
                size,
                var: stub,
            },
        );

        // Recurse into nested module variables
        if svar.kind(db) == SourceVariableKind::Module {
            let nested_model_name = svar.model_name(db);
            let nested_canonical = canonicalize(nested_model_name);
            if let Some(nested_model) = project_models.get(nested_canonical.as_ref()) {
                build_submodel_metadata(arena, db, *nested_model, project, all_metadata);
            }
        }
    }

    all_metadata.insert(sub_model_name, sub_metadata);
}

/// Result of per-variable compilation: symbolic bytecodes for each phase.
#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub(crate) struct VarFragmentResult {
    pub fragment: crate::compiler::symbolic::CompiledVarFragment,
}

#[salsa::tracked(returns(ref))]
pub fn compile_var_fragment(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_input_names: Vec<String>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{
        CompiledVarFragment, PerVarBytecodes, ReverseOffsetMap, VariableLayout,
    };

    let var_ident = var.ident(db).clone();
    let module_ident_context = module_ident_context_for_model(db, model, &module_input_names);
    let parsed = parse_source_variable_with_module_context(db, var, project, module_ident_context);

    // Accumulate unit definition errors from the parsed variable.
    // These are syntax errors in the unit string (e.g., "bad units
    // here!!!") that are stored in the variable's unit_errors field
    // during parsing but not checked during compilation.
    if let Some(unit_errs) = parsed.variable.unit_errors() {
        let model_name = model.name(db).clone();
        for err in unit_errs {
            CompilationDiagnostic(Diagnostic {
                model: model_name.clone(),
                variable: Some(var_ident.clone()),
                error: DiagnosticError::Unit(err),
                severity: DiagnosticSeverity::Error,
            })
            .accumulate(db);
        }
    }

    // Check for parse errors -- accumulate each one before bailing out
    if let Some(errors) = parsed.variable.equation_errors()
        && !errors.is_empty()
    {
        for err in &errors {
            CompilationDiagnostic(Diagnostic {
                model: model.name(db).clone(),
                variable: Some(var.ident(db).clone()),
                error: DiagnosticError::Equation(err.clone()),
                severity: DiagnosticSeverity::Error,
            })
            .accumulate(db);
        }
        return None;
    }

    // Build metadata from the full, input-agnostic dependency set so both
    // branches of `if isModuleInput(...)` remain compilable in the mini-context.
    let deps = variable_direct_dependencies_with_context(db, var, project, module_ident_context);

    // Get project dimensions and build dimension context
    let dm_dims = source_dims_to_datamodel(project.dimensions(db));
    let dim_context = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());
    let converted_dims: Vec<crate::dimensions::Dimension> = dm_dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    let project_models = project.models(db);

    // Lower the variable for compilation. Module-type variables need
    // direct construction because lower_variable's resolve_module_input
    // requires a populated models map.
    let lowered = if var.kind(db) == SourceVariableKind::Module {
        let var_name_canonical = canonicalize(&var_ident);
        let input_prefix = format!("{var_name_canonical}\u{00B7}");
        let module_inputs = build_module_inputs(
            model.name(db),
            &input_prefix,
            var.module_refs(db)
                .iter()
                .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
        );
        crate::variable::Variable::Module {
            ident: Ident::new(&var_ident),
            model_name: Ident::new(var.model_name(db)),
            units: None,
            inputs: module_inputs,
            errors: vec![],
            unit_errors: vec![],
        }
    } else {
        // Build a minimal ModelStage0 so that ArrayContext::get_dimensions
        // can resolve dependency dimensions during Expr2 lowering. Without
        // this, SUM(arr[*] + 1) fails because the Op2's ArrayBounds are
        // never computed (get_dimensions returns None for dependencies).
        let model_name_str = model.name(db);
        let source_vars = model.variables(db);
        let mut stage0_vars: HashMap<Ident<Canonical>, crate::model::VariableStage0> =
            HashMap::new();

        // Add the current variable
        stage0_vars.insert(Ident::new(&var_ident), parsed.variable.clone());

        // Add dependency variables so get_dimensions can resolve them
        let dep_names: BTreeSet<&String> = deps
            .dt_deps
            .iter()
            .chain(deps.initial_deps.iter())
            .collect();
        for dep_name in &dep_names {
            let effective = dep_name
                .as_str()
                .strip_prefix('\u{00B7}')
                .unwrap_or(dep_name.as_str());
            if effective.contains('\u{00B7}') {
                continue;
            }
            if let Some(dep_sv) = source_vars.get(effective) {
                let dep_parsed = parse_source_variable_with_module_context(
                    db,
                    *dep_sv,
                    project,
                    module_ident_context,
                );
                stage0_vars.insert(Ident::new(effective), dep_parsed.variable.clone());
            }
        }

        let mini_model = crate::model::ModelStage0 {
            ident: Ident::new(model_name_str),
            display_name: model_name_str.to_string(),
            variables: stage0_vars,
            errors: None,
            implicit: false,
        };

        let mut models: HashMap<Ident<Canonical>, &crate::model::ModelStage0> = HashMap::new();
        models.insert(Ident::new(model_name_str), &mini_model);

        let scope = crate::model::ScopeStage0 {
            models: &models,
            dimensions: &dim_context,
            model_name: model_name_str,
        };
        crate::model::lower_variable(&scope, &parsed.variable)
    };

    // Check for errors introduced during AST lowering (e.g.,
    // MismatchedDimensions from expr2/expr3 lowering). These are stored
    // in the lowered variable's errors field but not in the parsed
    // variable's errors, so we check them separately.
    if let Some(errors) = lowered.equation_errors()
        && !errors.is_empty()
    {
        for err in &errors {
            CompilationDiagnostic(Diagnostic {
                model: model.name(db).clone(),
                variable: Some(var.ident(db).clone()),
                error: DiagnosticError::Equation(err.clone()),
                severity: DiagnosticSeverity::Error,
            })
            .accumulate(db);
        }
        return None;
    }

    // Build minimal metadata: only {self} + deps
    let model_name_ident = Ident::new(model.name(db));
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_ident);
    let var_size = variable_size(db, var, project);

    // Arena for sub-model stub variables allocated by build_submodel_metadata
    let arena = bumpalo::Bump::new();

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

    // Track module deps that need module_refs and sub-model metadata
    let mut extra_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
    let mut extra_submodels: Vec<(String, SourceModel)> = Vec::new();
    let implicit_var_info = model_implicit_var_info(db, model, project);

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

        // Handle leading middle-dot (parent model reference in XMILE)
        let effective_name = dep_name
            .as_str()
            .strip_prefix('\u{00B7}')
            .unwrap_or(dep_name.as_str());

        // Check for composite module output reference (contains middle dot)
        if let Some(dot_pos) = effective_name.find('\u{00B7}') {
            let module_var_name = &effective_name[..dot_pos];
            let module_ident: Ident<Canonical> = Ident::new(module_var_name);

            if mini_metadata.contains_key(&module_ident) {
                continue;
            }

            // Look up the module variable in source_vars or implicit vars
            if let Some(mod_source_var) = source_vars.get(module_var_name) {
                if mod_source_var.kind(db) == SourceVariableKind::Module {
                    let mod_model_name = mod_source_var.model_name(db);
                    let sub_canonical = canonicalize(mod_model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1);

                    // Build Module variable with resolved inputs
                    let mod_input_prefix = format!("{module_var_name}\u{00B7}");
                    let module_inputs: Vec<crate::variable::ModuleInput> = mod_source_var
                        .module_refs(db)
                        .iter()
                        .filter_map(|mr| {
                            let src = canonicalize(&mr.src);
                            let dst = canonicalize(&mr.dst);
                            if src.starts_with(&mod_input_prefix) {
                                return None;
                            }
                            let dst_stripped = dst.strip_prefix(&mod_input_prefix)?;
                            let src_str = if model.name(db) == "main" && src.starts_with('\u{00B7}')
                            {
                                &src['\u{00B7}'.len_utf8()..]
                            } else {
                                &src
                            };
                            Some(crate::variable::ModuleInput {
                                src: Ident::new(src_str),
                                dst: Ident::new(dst_stripped),
                            })
                        })
                        .collect();

                    let mod_var = crate::variable::Variable::Module {
                        ident: module_ident.clone(),
                        model_name: Ident::new(mod_model_name),
                        units: None,
                        inputs: module_inputs.clone(),
                        errors: vec![],
                        unit_errors: vec![],
                    };
                    dep_variables.push((module_ident.clone(), mod_var, sub_size));

                    // Build module_refs entry
                    let input_set: BTreeSet<Ident<Canonical>> =
                        module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                    extra_module_refs.insert(module_ident, (Ident::new(mod_model_name), input_set));

                    if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                        extra_submodels.push((mod_model_name.to_string(), *sub_model));
                    }
                }
            } else if let Some(meta) = implicit_var_info.get(module_var_name)
                && meta.is_module
            {
                // Implicit module already handled in the implicit_module_vars section below
            }
            continue;
        }

        let dep_ident = Ident::new(effective_name);
        if mini_metadata.contains_key(&dep_ident) {
            continue;
        }

        if let Some(dep_source_var) = source_vars.get(effective_name) {
            let dep_dims = variable_dimensions(db, *dep_source_var, project);
            let dep_size = variable_size(db, *dep_source_var, project);

            let dep_var = build_stub_variable(db, dep_source_var, &dep_ident, dep_dims);

            dep_variables.push((dep_ident, dep_var, dep_size));
        } else if !implicit_var_info.contains_key(effective_name) {
            // Dependency is not a source variable or implicit variable --
            // this is an unknown dependency. Look up the source location
            // from the AST so the error points to the reference site.
            let loc = parsed
                .variable
                .ast()
                .and_then(|ast| ast.get_var_loc(effective_name))
                .unwrap_or_default();
            CompilationDiagnostic(Diagnostic {
                model: model.name(db).clone(),
                variable: Some(var.ident(db).clone()),
                error: DiagnosticError::Equation(crate::common::EquationError {
                    start: loc.start,
                    end: loc.end,
                    code: crate::common::ErrorCode::UnknownDependency,
                }),
                severity: DiagnosticSeverity::Error,
            })
            .accumulate(db);
            return None;
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

    // Add implicit module variables that this variable's AST references.
    // E.g., INIT(x) creates implicit module $⁚x⁚0⁚init and the variable's
    // AST references $⁚x⁚0⁚init·output -- the compiler needs the implicit
    // module in mini_metadata to resolve the sub-model offset.
    let mut implicit_module_vars: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> =
        Vec::new();
    let mut implicit_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
    let mut implicit_submodels: Vec<(String, SourceModel)> = Vec::new();

    for implicit_dm_var in &parsed.implicit_vars {
        if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
            let im_name = canonicalize(dm_module.ident.as_str()).into_owned();
            let im_ident: Ident<Canonical> = Ident::new(&im_name);
            if mini_metadata.contains_key(&im_ident) {
                continue;
            }

            let sub_canonical = canonicalize(&dm_module.model_name);
            let sub_size = project_models
                .get(sub_canonical.as_ref())
                .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                .unwrap_or(1);

            let im_var = crate::variable::Variable::Module {
                ident: im_ident.clone(),
                model_name: Ident::new(&dm_module.model_name),
                units: None,
                inputs: vec![],
                errors: vec![],
                unit_errors: vec![],
            };
            implicit_module_vars.push((im_ident.clone(), im_var, sub_size));

            // Build module_refs entry for the implicit module, stripping
            // the module ident prefix from dst (same as resolve_module_input)
            let im_input_prefix = format!("{im_name}\u{00B7}");
            let input_set: BTreeSet<Ident<Canonical>> = dm_module
                .references
                .iter()
                .filter_map(|mr| {
                    let dst_canonical = canonicalize(&mr.dst);
                    let bare = dst_canonical.strip_prefix(&im_input_prefix)?;
                    Some(Ident::new(bare))
                })
                .collect();
            implicit_module_refs.insert(im_ident, (Ident::new(&dm_module.model_name), input_set));

            if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                implicit_submodels.push((dm_module.model_name.clone(), *sub_model));
            }
        }
    }

    for (im_ident, im_var, im_size) in &implicit_module_vars {
        if !mini_metadata.contains_key(im_ident) {
            mini_metadata.insert(
                im_ident.clone(),
                crate::compiler::VariableMetadata {
                    offset: mini_offset,
                    size: *im_size,
                    var: im_var,
                },
            );
            mini_offset += im_size;
        }
    }

    // Build the all_metadata map (model_name -> var_name -> metadata)
    let mut all_metadata: HashMap<
        Ident<Canonical>,
        HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>>,
    > = HashMap::new();
    all_metadata.insert(model_name_ident.clone(), mini_metadata);

    // Populate sub-model metadata for implicit and explicit module sub-models
    for (_sub_name, sub_model) in implicit_submodels.iter().chain(extra_submodels.iter()) {
        build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
    }

    // Build the mini VariableLayout for symbolization
    let mini_layout =
        crate::compiler::symbolic::layout_from_metadata(&all_metadata, &model_name_ident)
            .unwrap_or_else(|_| VariableLayout::new(HashMap::new(), 0));
    let rmap = ReverseOffsetMap::from_layout(&mini_layout);

    // Build tables for compilation -- propagate errors rather than
    // silently dropping them, which would shift table indices and cause
    // lookups to read the wrong table at runtime.
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    {
        let gf_tables = lowered.tables();
        if !gf_tables.is_empty() {
            let table_results: crate::Result<Vec<crate::compiler::Table>> = gf_tables
                .iter()
                .map(|t| crate::compiler::Table::new(&var_ident, t))
                .collect();
            match table_results {
                Ok(ts) if !ts.is_empty() => {
                    tables.insert(var_ident_canonical.clone(), ts);
                }
                Err(table_err) => {
                    CompilationDiagnostic(Diagnostic {
                        model: model.name(db).clone(),
                        variable: Some(var.ident(db).clone()),
                        error: DiagnosticError::Model(table_err),
                        severity: DiagnosticSeverity::Error,
                    })
                    .accumulate(db);
                    return None;
                }
                _ => {}
            }
        }
    }

    // Also collect tables from dependency variables that have graphical
    // functions. When a variable uses LOOKUP(dep, x), the dep's table
    // data must be in the mini-Module's tables map so the bytecode
    // compiler can emit the correct Lookup opcodes.
    for dep_name in &all_names {
        let effective = dep_name
            .as_str()
            .strip_prefix('\u{00B7}')
            .unwrap_or(dep_name.as_str());
        if effective.contains('\u{00B7}') {
            continue;
        }
        let dep_canonical: Ident<Canonical> = Ident::new(effective);
        if tables.contains_key(&dep_canonical) {
            continue;
        }
        if let Some(dep_sv) = source_vars.get(effective) {
            let dep_tables = extract_tables_from_source_var(db, dep_sv);
            if !dep_tables.is_empty() {
                tables.insert(dep_canonical, dep_tables);
            }
        }
    }

    // Build the minimal Module
    let inputs = canonical_module_input_set(&module_input_names);
    let module_models = model_module_map(db, model, project).clone();

    // Determine which runlists this variable belongs to
    let dep_graph = if module_input_names.is_empty() {
        model_dependency_graph(db, model, project)
    } else {
        model_dependency_graph_with_inputs(db, model, project, module_input_names.clone())
    };
    let is_stock = var.kind(db) == SourceVariableKind::Stock;
    let is_module = var.kind(db) == SourceVariableKind::Module;
    let is_module_input = inputs.contains(&var_ident_canonical);

    // We need module_refs for module variables (explicit or implicit)
    let mut module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = if is_module {
        let ref_prefix = format!("{var_ident}\u{00B7}");
        let input_set: BTreeSet<Ident<Canonical>> = var
            .module_refs(db)
            .iter()
            .filter_map(|mr| {
                let dst_canonical = canonicalize(&mr.dst);
                let bare = dst_canonical.strip_prefix(&ref_prefix)?;
                Some(Ident::new(bare))
            })
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
    module_refs.extend(implicit_module_refs);
    module_refs.extend(extra_module_refs);

    // For module variables, populate sub-model metadata so the compiler
    // can generate correct CallModule bytecodes.
    if is_module {
        let sub_model_name = var.model_name(db);
        let sub_canonical = canonicalize(sub_model_name);
        if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
            build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
        }
    }

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
        let module_inputs: HashSet<Ident<Canonical>> = inputs.iter().cloned().collect();
        let module = crate::compiler::Module {
            ident: model_name_ident.clone(),
            inputs: module_inputs,
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
                    .map(|sv| crate::compiler::symbolic::symbolize_static_view(sv, &rmap))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                let sym_mods: Vec<_> = ctx
                    .modules
                    .iter()
                    .map(|md| crate::compiler::symbolic::symbolize_module_decl(md, &rmap))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;

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

    // Accumulate a diagnostic when per-variable compilation (Var::new)
    // fails. Without this, errors like DoesNotExist (unknown dependency)
    // are silently dropped and never appear in collect_all_diagnostics.
    let accumulate_var_compile_error = |err: &crate::Error| {
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var.ident(db).clone()),
            error: DiagnosticError::Equation(crate::common::EquationError {
                start: 0,
                end: 0,
                code: err.code,
            }),
            severity: DiagnosticSeverity::Error,
        })
        .accumulate(db);
    };

    // Initial phase: stocks and their deps get compiled with is_initial=true
    let initial_bytecodes = if dep_graph.runlist_initials.contains(&var_ident_str) {
        match build_var(true) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(ref err) => {
                accumulate_var_compile_error(err);
                None
            }
        }
    } else {
        None
    };

    // Flow phase: non-stock vars AND stock-typed module inputs get compiled
    // with is_initial=false. Stock-typed module inputs need LoadModuleInput ->
    // AssignCurr in the flows phase to propagate the parent-provided value
    // each timestep (matching the monolithic path's `instantiation.contains(id)
    // || !var.is_stock()` filter).
    let flow_bytecodes =
        if (!is_stock || is_module_input) && dep_graph.runlist_flows.contains(&var_ident_str) {
            match build_var(false) {
                Ok(var_result) => compile_phase(&var_result.ast),
                Err(ref err) => {
                    accumulate_var_compile_error(err);
                    None
                }
            }
        } else {
            None
        };

    // Stock phase: stocks and modules get compiled with is_initial=false
    let stock_bytecodes =
        if (is_stock || is_module) && dep_graph.runlist_stocks.contains(&var_ident_str) {
            match build_var(false) {
                Ok(var_result) => compile_phase(&var_result.ast),
                Err(ref err) => {
                    accumulate_var_compile_error(err);
                    None
                }
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

/// Compile a single implicit variable (generated by SMOOTH/DELAY/TREND builtins)
/// to symbolic bytecodes. Not a tracked function -- the parent variable's
/// parse result already provides salsa caching.
fn compile_implicit_var_fragment(
    db: &dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    dep_graph: &ModelDepGraphResult,
    module_input_names: &[String],
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{
        CompiledVarFragment, PerVarBytecodes, ReverseOffsetMap, VariableLayout,
    };
    let module_ident_context = module_ident_context_for_model(db, model, module_input_names);
    let parsed = parse_source_variable_with_module_context(
        db,
        meta.parent_source_var,
        project,
        module_ident_context,
    );
    let implicit_dm_var = parsed.implicit_vars.get(meta.index_in_parent)?;
    let implicit_name = canonicalize(implicit_dm_var.get_ident()).into_owned();

    let dm_dims = project_datamodel_dims(db, project);
    let dim_context = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());
    let converted_dims: Vec<crate::dimensions::Dimension> = dm_dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    let units_ctx = project_units_context(db, project);

    let mut dummy_implicits = Vec::new();
    let parsed_implicit = crate::variable::parse_var(
        dm_dims,
        implicit_dm_var,
        &mut dummy_implicits,
        units_ctx,
        |mi| Ok(Some(mi.clone())),
    );

    if parsed_implicit
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }

    // Module-type implicit vars need direct Module construction (lower_variable
    // with empty models map causes resolve_module_input to fail).
    let lowered = if meta.is_module {
        if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
            let module_inputs: Vec<crate::variable::ModuleInput> = dm_module
                .references
                .iter()
                .filter_map(|mr| {
                    let ident_prefix = format!("{}·", canonicalize(&implicit_name));
                    let src = canonicalize(&mr.src);
                    let dst = canonicalize(&mr.dst);
                    if src.starts_with(&ident_prefix) {
                        return None;
                    }
                    let dst_stripped = dst.strip_prefix(&ident_prefix)?;
                    let src_str = if model.name(db) == "main" && src.starts_with('·') {
                        &src['·'.len_utf8()..]
                    } else {
                        &src
                    };
                    Some(crate::variable::ModuleInput {
                        src: Ident::new(src_str),
                        dst: Ident::new(dst_stripped),
                    })
                })
                .collect();
            crate::variable::Variable::Module {
                ident: Ident::new(&implicit_name),
                model_name: Ident::new(&dm_module.model_name),
                units: None,
                inputs: module_inputs,
                errors: vec![],
                unit_errors: vec![],
            }
        } else {
            return None;
        }
    } else {
        let models = HashMap::new();
        let scope = crate::model::ScopeStage0 {
            models: &models,
            dimensions: &dim_context,
            model_name: "",
        };
        crate::model::lower_variable(&scope, &parsed_implicit)
    };

    let model_name_ident = Ident::new(model.name(db));
    let var_ident_canonical: Ident<Canonical> = Ident::new(&implicit_name);

    // Arena for sub-model stub variables allocated by build_submodel_metadata
    let arena = bumpalo::Bump::new();

    let mut mini_metadata: HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>> =
        HashMap::new();
    let mut mini_offset = if is_root {
        crate::vm::IMPLICIT_VAR_COUNT
    } else {
        0
    };

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

    let project_models = project.models(db);
    let self_size = if meta.is_module {
        if let Some(sub_model_name) = &meta.model_name {
            let sub_canonical = canonicalize(sub_model_name);
            project_models
                .get(sub_canonical.as_ref())
                .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                .unwrap_or(1)
        } else {
            1
        }
    } else {
        1
    };
    mini_metadata.insert(
        var_ident_canonical.clone(),
        crate::compiler::VariableMetadata {
            offset: mini_offset,
            size: self_size,
            var: &lowered,
        },
    );
    mini_offset += self_size;

    // Implicit vars' deps are always explicit vars in the same model (or other implicit vars)
    // Keep dependency context conservative for implicit vars as well: both
    // branches of `if isModuleInput(...)` may still be compiled.
    let deps = variable_direct_dependencies(db, meta.parent_source_var, project);
    let implicit_dep = deps
        .implicit_vars
        .iter()
        .find(|iv| canonicalize(&iv.name) == canonicalize(&implicit_name));

    let all_dep_names: BTreeSet<String> = if let Some(iv_deps) = implicit_dep {
        iv_deps
            .dt_deps
            .iter()
            .chain(iv_deps.initial_deps.iter())
            .cloned()
            .collect()
    } else {
        BTreeSet::new()
    };

    let mut extra_dep_names: Vec<String> = Vec::new();
    if meta.is_stock
        && let crate::variable::Variable::Stock {
            inflows, outflows, ..
        } = &lowered
    {
        for flow_name in inflows.iter().chain(outflows.iter()) {
            let canonical = flow_name.as_str().to_string();
            if !all_dep_names.contains(&canonical) {
                extra_dep_names.push(canonical);
            }
        }
    }

    let source_vars = model.variables(db);
    let implicit_info = model_implicit_var_info(db, model, project);
    let all_names: Vec<&String> = all_dep_names.iter().chain(extra_dep_names.iter()).collect();
    let mut dep_variables: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> = Vec::new();
    let mut extra_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
    let mut extra_submodels: HashMap<String, SourceModel> = HashMap::new();

    for dep_name in &all_names {
        let effective_name = dep_name
            .as_str()
            .strip_prefix('\u{00B7}')
            .unwrap_or(dep_name.as_str());

        if effective_name == implicit_name.as_str()
            || matches!(
                effective_name,
                "time" | "dt" | "initial_time" | "final_time"
            )
        {
            continue;
        }

        if let Some(dot_pos) = effective_name.find('\u{00B7}') {
            let module_var_name = &effective_name[..dot_pos];
            let module_ident: Ident<Canonical> = Ident::new(module_var_name);

            if mini_metadata.contains_key(&module_ident) {
                continue;
            }

            if let Some(mod_source_var) = source_vars.get(module_var_name) {
                if mod_source_var.kind(db) == SourceVariableKind::Module {
                    let mod_model_name = mod_source_var.model_name(db);
                    let sub_canonical = canonicalize(mod_model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                        .unwrap_or(1);

                    let mod_input_prefix = format!("{module_var_name}\u{00B7}");
                    let module_inputs = build_module_inputs(
                        model.name(db),
                        &mod_input_prefix,
                        mod_source_var
                            .module_refs(db)
                            .iter()
                            .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                    );

                    let mod_var = crate::variable::Variable::Module {
                        ident: module_ident.clone(),
                        model_name: Ident::new(mod_model_name),
                        units: None,
                        inputs: module_inputs.clone(),
                        errors: vec![],
                        unit_errors: vec![],
                    };
                    dep_variables.push((module_ident.clone(), mod_var, sub_size));

                    let input_set: BTreeSet<Ident<Canonical>> =
                        module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                    extra_module_refs.insert(module_ident, (Ident::new(mod_model_name), input_set));

                    if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                        extra_submodels.insert(mod_model_name.to_string(), *sub_model);
                    }
                }
            } else if let Some(im_meta) = implicit_info.get(module_var_name)
                && im_meta.is_module
                && let Some(im_model_name) = im_meta.model_name.as_deref()
            {
                let sub_canonical = canonicalize(im_model_name);
                let sub_size = project_models
                    .get(sub_canonical.as_ref())
                    .map(|sm| compute_layout(db, *sm, project, false).n_slots)
                    .unwrap_or(1);

                let input_prefix = format!("{module_var_name}\u{00B7}");
                let module_inputs = parsed
                    .implicit_vars
                    .iter()
                    .find_map(|iv| match iv {
                        datamodel::Variable::Module(dm_module)
                            if canonicalize(dm_module.ident.as_str()) == module_var_name =>
                        {
                            Some(build_module_inputs(
                                model.name(db),
                                &input_prefix,
                                dm_module
                                    .references
                                    .iter()
                                    .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                            ))
                        }
                        _ => None,
                    })
                    .unwrap_or_default();

                let mod_var = crate::variable::Variable::Module {
                    ident: module_ident.clone(),
                    model_name: Ident::new(im_model_name),
                    units: None,
                    inputs: module_inputs.clone(),
                    errors: vec![],
                    unit_errors: vec![],
                };
                dep_variables.push((module_ident.clone(), mod_var, sub_size));

                let input_set: BTreeSet<Ident<Canonical>> =
                    module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                extra_module_refs.insert(module_ident, (Ident::new(im_model_name), input_set));

                if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                    extra_submodels.insert(im_model_name.to_string(), *sub_model);
                }
            }
            continue;
        }

        let dep_ident = Ident::new(effective_name);
        if mini_metadata.contains_key(&dep_ident) {
            continue;
        }

        if let Some(dep_source_var) = source_vars.get(effective_name) {
            let dep_dims = variable_dimensions(db, *dep_source_var, project);
            let dep_size = variable_size(db, *dep_source_var, project);
            let dep_var = build_stub_variable(db, dep_source_var, &dep_ident, dep_dims);
            dep_variables.push((dep_ident, dep_var, dep_size));
        } else if let Some(implicit_meta) = implicit_info.get(effective_name) {
            // Dep is another implicit var -- build a scalar stub
            let is_stock = implicit_meta.is_stock;
            let dep_var = if is_stock {
                crate::variable::Variable::Stock {
                    ident: dep_ident.clone(),
                    init_ast: None,
                    eqn: None,
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    non_negative: false,
                    errors: vec![],
                    unit_errors: vec![],
                }
            } else {
                crate::variable::Variable::Var {
                    ident: dep_ident.clone(),
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
                }
            };
            dep_variables.push((dep_ident, dep_var, 1));
        }
    }

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

    let mut all_metadata: HashMap<
        Ident<Canonical>,
        HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>>,
    > = HashMap::new();
    all_metadata.insert(model_name_ident.clone(), mini_metadata);

    for sub_model in extra_submodels.values() {
        build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
    }

    let mini_layout =
        crate::compiler::symbolic::layout_from_metadata(&all_metadata, &model_name_ident)
            .unwrap_or_else(|_| VariableLayout::new(HashMap::new(), 0));
    let rmap = ReverseOffsetMap::from_layout(&mini_layout);

    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    {
        let gf_tables = lowered.tables();
        if !gf_tables.is_empty() {
            let table_results: crate::Result<Vec<crate::compiler::Table>> = gf_tables
                .iter()
                .map(|t| crate::compiler::Table::new(&implicit_name, t))
                .collect();
            match table_results {
                Ok(ts) if !ts.is_empty() => {
                    tables.insert(var_ident_canonical.clone(), ts);
                }
                Err(_) => return None,
                _ => {}
            }
        }
    }

    for dep_name in &all_names {
        let effective = dep_name
            .as_str()
            .strip_prefix('\u{00B7}')
            .unwrap_or(dep_name.as_str());
        if effective.contains('\u{00B7}') {
            continue;
        }
        let dep_canonical: Ident<Canonical> = Ident::new(effective);
        if tables.contains_key(&dep_canonical) {
            continue;
        }
        if let Some(dep_sv) = source_vars.get(effective) {
            let dep_tables = extract_tables_from_source_var(db, dep_sv);
            if !dep_tables.is_empty() {
                tables.insert(dep_canonical, dep_tables);
            }
        }
    }

    let inputs = canonical_module_input_set(module_input_names);
    let (module_models, mut module_refs) = if meta.is_module {
        let mm = model_module_map(db, model, project).clone();

        // Build module_refs from the implicit var's datamodel::Module references,
        // stripping the module ident prefix from dst (matching compile_var_fragment
        // and enumerate_module_instances_inner).
        let mut refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
        if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
            let input_prefix = format!("{implicit_name}\u{00B7}");
            let input_set: BTreeSet<Ident<Canonical>> = dm_module
                .references
                .iter()
                .filter_map(|mr| {
                    let dst_canonical = canonicalize(&mr.dst);
                    let bare = dst_canonical.strip_prefix(&input_prefix)?;
                    Some(Ident::new(bare))
                })
                .collect();
            refs.insert(
                var_ident_canonical.clone(),
                (Ident::new(&dm_module.model_name), input_set),
            );

            // Populate sub-model metadata
            let sub_canonical = canonicalize(&dm_module.model_name);
            if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
            }
        }

        (mm, refs)
    } else {
        (HashMap::new(), HashMap::new())
    };
    module_refs.extend(extra_module_refs);

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

    let compile_phase = |exprs: &[crate::compiler::Expr]| -> Option<PerVarBytecodes> {
        if exprs.is_empty() {
            return None;
        }

        let runlist_initials_by_var = vec![];
        let module_inputs: HashSet<Ident<Canonical>> = inputs.iter().cloned().collect();
        let module = crate::compiler::Module {
            ident: model_name_ident.clone(),
            inputs: module_inputs,
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

        let module = crate::compiler::Module {
            n_temps,
            temp_sizes: temp_sizes.clone(),
            ..module
        };

        match module.compile() {
            Ok(compiled) => {
                let sym_bc =
                    crate::compiler::symbolic::symbolize_bytecode(&compiled.compiled_flows, &rmap)
                        .ok()?;

                let ctx = &*compiled.context;
                let sym_views: Vec<_> = ctx
                    .static_views
                    .iter()
                    .map(|sv| crate::compiler::symbolic::symbolize_static_view(sv, &rmap))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;
                let sym_mods: Vec<_> = ctx
                    .modules
                    .iter()
                    .map(|md| crate::compiler::symbolic::symbolize_module_decl(md, &rmap))
                    .collect::<Result<Vec<_>, _>>()
                    .ok()?;

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

    let var_ident_str = var_ident_canonical.as_str().to_string();

    let initial_bytecodes = if dep_graph.runlist_initials.contains(&var_ident_str) {
        match build_var(true) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(_) => None,
        }
    } else {
        None
    };

    let flow_bytecodes = if !meta.is_stock && dep_graph.runlist_flows.contains(&var_ident_str) {
        match build_var(false) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(_) => None,
        }
    } else {
        None
    };

    let stock_bytecodes =
        if (meta.is_stock || meta.is_module) && dep_graph.runlist_stocks.contains(&var_ident_str) {
            match build_var(false) {
                Ok(var_result) => compile_phase(&var_result.ast),
                Err(_) => None,
            }
        } else {
            None
        };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: implicit_name,
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
    module_inputs: &BTreeSet<Ident<Canonical>>,
) -> Result<crate::bytecode::CompiledModule, String> {
    use crate::compiler::symbolic::{
        ContextResourceCounts, SymbolicCompiledInitial, SymbolicCompiledModule,
        concatenate_fragments, resolve_module,
    };

    let module_input_names: Vec<String> = module_inputs
        .iter()
        .map(|ident| ident.as_str().to_string())
        .collect();
    let dep_graph = if module_input_names.is_empty() {
        model_dependency_graph(db, model, project)
    } else {
        model_dependency_graph_with_inputs(db, model, project, module_input_names.clone())
    };
    if dep_graph.has_cycle {
        let msg = format!("model '{}' has circular dependencies", model.name(db));
        try_accumulate_diagnostic(
            db,
            Diagnostic {
                model: model.name(db).clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg.clone()),
                severity: DiagnosticSeverity::Error,
            },
        );
        return Err(msg);
    }
    let layout = compute_layout(db, model, project, is_root);
    let source_vars = model.variables(db);
    let implicit_info = model_implicit_var_info(db, model, project);
    let model_name = model.name(db).clone();

    // Pre-compile all fragments (explicit + implicit) into a combined map
    let mut all_fragments: HashMap<String, VarFragmentResult> = HashMap::new();

    for (name, svar) in source_vars.iter() {
        if let Some(result) = compile_var_fragment(
            db,
            *svar,
            model,
            project,
            is_root,
            module_input_names.clone(),
        ) {
            all_fragments.insert(name.clone(), result.clone());
        }
    }

    for (name, meta) in implicit_info.iter() {
        if let Some(result) = compile_implicit_var_fragment(
            db,
            meta,
            model,
            project,
            is_root,
            dep_graph,
            &module_input_names,
        ) {
            all_fragments.insert(name.clone(), result);
        }
    }

    // Pass 3: LTM synthetic variables (only when ltm_enabled).
    //
    // LTM link-score, loop-score, and relative-score equations are
    // compiled here and appended to the flows runlist. When ltm_enabled
    // is false this pass is skipped entirely (AC1.5). When the model
    // has no feedback loops the LTM variable list is empty (AC1.4).
    //
    // LTM vars have no dt-phase ordering constraints with regular
    // variables because PREVIOUS reads from the previous timestep's
    // committed values. They can be appended to the end of the flows
    // runlist.
    //
    // LTM variables only exist in the root model -- stdlib sub-models
    // never have LTM instrumentation.
    let mut ltm_flow_names: Vec<String> = Vec::new();
    if is_root && project.ltm_enabled(db) {
        let ltm_vars = if project.ltm_discovery_mode(db) {
            model_ltm_all_link_synthetic_variables(db, model, project)
        } else {
            model_ltm_synthetic_variables(db, model, project)
        };

        for ltm_var in &ltm_vars.vars {
            let ltm_var_canonical = canonicalize(&ltm_var.name).into_owned();

            // Try compile_ltm_var_fragment for link scores (keyed by LtmLinkId)
            let fragment_result = if ltm_var.name.contains("\u{205A}link_score\u{205A}")
                || ltm_var.name.contains("\u{205A}ilink\u{205A}")
            {
                // Extract from/to from the link score name
                // Format: $:ltm:link_score:from->to or $:ltm:ilink:from->to
                let arrow_pos = ltm_var.name.find('\u{2192}');
                if let Some(arrow) = arrow_pos {
                    // Find the last separator before the arrow to locate the
                    // end of the prefix (e.g. "$:ltm:link_score:").  Using
                    // rfind on the full string would match separators inside
                    // the `to` name that appear after the arrow.
                    let prefix_end = ltm_var.name[..arrow]
                        .rfind('\u{205A}')
                        .map(|p| p + '\u{205A}'.len_utf8())
                        .unwrap_or(0);
                    let from_name = &ltm_var.name[prefix_end..arrow];
                    let to_name = &ltm_var.name[arrow + '\u{2192}'.len_utf8()..];
                    let link_id = LtmLinkId::new(db, from_name.to_string(), to_name.to_string());
                    compile_ltm_var_fragment(db, link_id, model, project)
                        .as_ref()
                        .cloned()
                } else {
                    compile_ltm_equation_fragment(
                        db,
                        &ltm_var.name,
                        &ltm_var.equation,
                        model,
                        project,
                    )
                }
            } else {
                // Loop scores and relative loop scores: compile directly
                compile_ltm_equation_fragment(db, &ltm_var.name, &ltm_var.equation, model, project)
            };

            if let Some(result) = fragment_result {
                all_fragments.insert(ltm_var_canonical.clone(), result);
                ltm_flow_names.push(ltm_var_canonical);
            }
        }

        // Also compile the implicit modules (PREVIOUS instances) from LTM
        // equations. These are module-type variables that need initial and
        // stock phase compilation like regular implicit modules.
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let ltm_module_idents = db_ltm::ltm_module_idents(db, model, project);
        for ltm_var in &ltm_vars.vars {
            let parsed = db_ltm::parse_ltm_var_with_ids(db, ltm_var, project, &ltm_module_idents);
            for (idx, implicit_dm_var) in parsed.implicit_vars.iter().enumerate() {
                let im_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
                if all_fragments.contains_key(&im_name) {
                    continue;
                }
                if let Some(meta) = ltm_implicit.get(&im_name) {
                    // Build an ImplicitVarMeta-compatible structure. Since LTM
                    // implicit vars don't have a parent SourceVariable, we
                    // compile them directly using the parsed LTM equation data.
                    let im_fragment = compile_ltm_implicit_var_fragment(
                        db,
                        &parsed,
                        idx,
                        meta,
                        model,
                        project,
                        dep_graph,
                        &module_input_names,
                    );
                    if let Some(result) = im_fragment {
                        all_fragments.insert(im_name.clone(), result);
                        // Implicit modules participate in initials and stocks
                        // runlists. Their names are added to the dependency
                        // graph's runlists below.
                    }
                }
            }
        }
    }

    // Module input variables have their values provided by the parent
    // model via EvalModule/LoadModuleInput. Their compiled bytecodes
    // consist of LoadModuleInput -> AssignCurr, which copies the
    // parent-provided value into the sub-model's local slot. This must
    // happen during initials and flows phases. Only the stocks phase
    // excludes module inputs (matching the monolithic path which uses
    // `!instantiation.contains(id) && (is_stock || is_module)` for stocks).
    let is_module_input =
        |var_name: &str| -> bool { module_inputs.contains(&*canonicalize(var_name)) };

    // Collect fragments for each phase, tracking missing variables
    let mut initial_frags: Vec<(String, &crate::compiler::symbolic::PerVarBytecodes)> = Vec::new();
    let mut flow_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();
    let mut stock_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();
    let mut missing_vars: Vec<String> = Vec::new();

    for var_name in &dep_graph.runlist_initials {
        if let Some(result) = all_fragments.get(var_name)
            && let Some(ref bc) = result.fragment.initial_bytecodes
        {
            initial_frags.push((var_name.clone(), bc));
        } else if !is_module_input(var_name) {
            missing_vars.push(var_name.clone());
        }
    }

    for var_name in &dep_graph.runlist_flows {
        if let Some(result) = all_fragments.get(var_name)
            && let Some(ref bc) = result.fragment.flow_bytecodes
        {
            flow_frags.push(bc);
        } else if !is_module_input(var_name) {
            missing_vars.push(var_name.clone());
        }
    }

    for var_name in &dep_graph.runlist_stocks {
        if is_module_input(var_name) {
            continue;
        }
        if let Some(result) = all_fragments.get(var_name)
            && let Some(ref bc) = result.fragment.stock_bytecodes
        {
            stock_frags.push(bc);
        } else {
            missing_vars.push(var_name.clone());
        }
    }

    // Append LTM flow fragments (link scores, loop scores, relative
    // loop scores). These go at the end of the flows runlist since
    // they have no ordering constraints with regular variables.
    for ltm_name in &ltm_flow_names {
        if let Some(result) = all_fragments.get(ltm_name)
            && let Some(ref bc) = result.fragment.flow_bytecodes
        {
            flow_frags.push(bc);
        }
    }

    // Append LTM implicit module fragments to initials and stocks
    // runlists. PREVIOUS module instances contain stocks that need
    // initialization and stock update phases.
    if is_root && project.ltm_enabled(db) {
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let mut ltm_im_names: Vec<&String> = ltm_implicit.keys().collect();
        ltm_im_names.sort_unstable();
        for im_name in ltm_im_names {
            if let Some(result) = all_fragments.get(im_name) {
                if let Some(ref bc) = result.fragment.initial_bytecodes {
                    initial_frags.push((im_name.clone(), bc));
                }
                if let Some(ref bc) = result.fragment.flow_bytecodes {
                    flow_frags.push(bc);
                }
                if let Some(ref bc) = result.fragment.stock_bytecodes {
                    stock_frags.push(bc);
                }
            }
        }
    }

    if !missing_vars.is_empty() {
        let msg = format!(
            "failed to compile fragments for variables: {}",
            missing_vars.join(", ")
        );
        try_accumulate_diagnostic(
            db,
            Diagnostic {
                model: model_name.clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg.clone()),
                severity: DiagnosticSeverity::Error,
            },
        );
        return Err(msg);
    }

    // Compute context resource base offsets for each phase so that flows
    // and stocks reference the same resource namespace as the all-phases
    // merge. The all-phases ordering is: initials, then flows, then stocks.
    let initial_refs: Vec<&crate::compiler::symbolic::PerVarBytecodes> =
        initial_frags.iter().map(|(_, bc)| *bc).collect();
    let initial_counts = ContextResourceCounts::from_fragments(&initial_refs);
    let flow_counts = ContextResourceCounts::from_fragments(&flow_frags);

    let no_base = ContextResourceCounts::default();
    let flow_base = initial_counts.clone();
    let stock_base = ContextResourceCounts {
        graphical_functions: initial_counts.graphical_functions + flow_counts.graphical_functions,
        modules: initial_counts.modules + flow_counts.modules,
        views: initial_counts.views + flow_counts.views,
        temps: initial_counts.temps + flow_counts.temps,
        dim_lists: initial_counts.dim_lists + flow_counts.dim_lists,
    };

    let flows_concat = concatenate_fragments(&flow_frags, &flow_base).inspect_err(|msg| {
        try_accumulate_diagnostic(
            db,
            Diagnostic {
                model: model_name.clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg.clone()),
                severity: DiagnosticSeverity::Error,
            },
        );
    })?;
    let stocks_concat = concatenate_fragments(&stock_frags, &stock_base).inspect_err(|msg| {
        try_accumulate_diagnostic(
            db,
            Diagnostic {
                model: model_name.clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg.clone()),
                severity: DiagnosticSeverity::Error,
            },
        );
    })?;

    // Build SymbolicCompiledInitial for each initial variable, renumbered
    // so context resource IDs (GFs, modules, views, temps, dim_lists) match
    // the all-phases merge. Literal IDs are local to each initial's bytecode
    // so they get no base offset.
    let mut compiled_initials: Vec<SymbolicCompiledInitial> = Vec::new();
    let mut init_gf_off: u16 = 0;
    let mut init_mod_off: u16 = 0;
    let mut init_view_off: u16 = 0;
    let mut init_temp_off: u32 = 0;
    let mut init_dl_off: u16 = 0;
    for (name, bc) in &initial_frags {
        let renumbered_code: Vec<crate::compiler::symbolic::SymbolicOpcode> = bc
            .symbolic
            .code
            .iter()
            .map(|op| {
                crate::compiler::symbolic::renumber_opcode(
                    op,
                    0, // literals are local to each initial's bytecode
                    init_gf_off,
                    init_mod_off,
                    init_view_off,
                    init_temp_off,
                    init_dl_off,
                )
            })
            .collect::<Result<Vec<_>, _>>()
            .inspect_err(|msg| {
                try_accumulate_diagnostic(
                    db,
                    Diagnostic {
                        model: model_name.clone(),
                        variable: None,
                        error: DiagnosticError::Assembly(msg.clone()),
                        severity: DiagnosticSeverity::Error,
                    },
                );
            })?;
        compiled_initials.push(SymbolicCompiledInitial {
            ident: Ident::new(name),
            bytecode: crate::compiler::symbolic::SymbolicByteCode {
                literals: bc.symbolic.literals.clone(),
                code: renumbered_code,
            },
        });
        init_gf_off += bc.graphical_functions.len() as u16;
        init_mod_off += bc.module_decls.len() as u16;
        init_view_off += bc.static_views.len() as u16;
        let frag_temp_count = bc
            .temp_sizes
            .iter()
            .map(|(id, _)| *id + 1)
            .max()
            .unwrap_or(0);
        init_temp_off += frag_temp_count;
        init_dl_off += bc.dim_lists.len() as u16;
    }

    // Build the all-phases merge for shared context (GFs, modules, views, temps, dim_lists)
    let all_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = initial_frags
        .iter()
        .map(|(_, bc)| *bc)
        .chain(flow_frags.iter().copied())
        .chain(stock_frags.iter().copied())
        .collect();
    let merged = concatenate_fragments(&all_frags, &no_base).inspect_err(|msg| {
        try_accumulate_diagnostic(
            db,
            Diagnostic {
                model: model_name.clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg.clone()),
                severity: DiagnosticSeverity::Error,
            },
        );
    })?;

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
    resolve_module(&sym_module, layout).inspect_err(|msg| {
        try_accumulate_diagnostic(
            db,
            Diagnostic {
                model: model_name.clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg.clone()),
                severity: DiagnosticSeverity::Error,
            },
        );
    })
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
        let msg = format!("no model named '{}' to simulate", main_model_name);
        try_accumulate_diagnostic(
            db,
            Diagnostic {
                model: main_model_name.to_string(),
                variable: None,
                error: DiagnosticError::Assembly(msg.clone()),
                severity: DiagnosticSeverity::Error,
            },
        );
        return Err(msg);
    }

    // Enumerate module instances by walking module variables recursively.
    // Each unique (model_name, input_set) pair gets its own CompiledModule.
    let module_instances =
        enumerate_module_instances(db, project, main_model_name).inspect_err(|msg| {
            try_accumulate_diagnostic(
                db,
                Diagnostic {
                    model: main_model_name.to_string(),
                    variable: None,
                    error: DiagnosticError::Assembly(msg.clone()),
                    severity: DiagnosticSeverity::Error,
                },
            );
        })?;

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
                let msg = format!(
                    "model '{}' referenced as module but not found in project",
                    model_name_str,
                );
                try_accumulate_diagnostic(
                    db,
                    Diagnostic {
                        model: main_model_name.to_string(),
                        variable: None,
                        error: DiagnosticError::Assembly(msg.clone()),
                        severity: DiagnosticSeverity::Error,
                    },
                );
                msg
            })?;

            let is_root = canonicalize(name.as_str()) == main_model_canonical;
            let compiled = assemble_module(db, *source_model, project, is_root, inputs)?;
            let module_key: crate::vm::ModuleKey = ((*name).clone(), inputs.clone());
            compiled_modules.insert(module_key, compiled);
        }
    }

    // Build Specs, preferring model-level sim_specs override when present
    // (mirrors the monolithic path in interpreter.rs compile_project)
    let main_model_canonical = canonicalize(main_model_name);
    let specs = if let Some(source_model) = project_models.get(main_model_canonical.as_ref())
        && let Some(ref model_specs) = *source_model.model_sim_specs(db)
    {
        let sim_specs_dm = source_sim_specs_to_datamodel(model_specs);
        crate::vm::Specs::from(&sim_specs_dm)
    } else {
        let sim_specs_dm = source_sim_specs_to_datamodel(project.sim_specs(db));
        crate::vm::Specs::from(&sim_specs_dm)
    };

    // Compute flattened offsets for variable name -> offset mapping
    let offsets = calc_flattened_offsets_incremental(db, project, main_model_name, true);
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
    for (var_name, source_var) in source_vars.iter() {
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

        // Strip module ident prefix from dst to get bare sub-model variable
        // names, matching how resolve_module_input works in the monolithic path
        let input_prefix = format!("{var_name}\u{00B7}");
        let inputs: BTreeSet<Ident<Canonical>> = source_var
            .module_refs(db)
            .iter()
            .filter_map(|mr| {
                let dst_canonical = canonicalize(&mr.dst);
                let bare = dst_canonical.strip_prefix(&input_prefix)?;
                Some(Ident::new(bare))
            })
            .collect();

        let key = Ident::<Canonical>::new(sub_model_name);
        let is_new = !modules.contains_key(&key);

        modules.entry(key).or_default().insert(inputs);

        if is_new {
            enumerate_module_instances_inner(db, project, sub_model_name, modules)?;
        }
    }

    // Include implicit MODULE variables (e.g. from SMOOTH, DELAY builtins)
    let implicit_info = model_implicit_var_info(db, *source_model, project);
    for (name, meta) in implicit_info.iter() {
        if !meta.is_module {
            continue;
        }
        let sub_model_name = match &meta.model_name {
            Some(n) => n,
            None => continue,
        };
        let sub_canonical = canonicalize(sub_model_name);
        if !project_models.contains_key(sub_canonical.as_ref()) {
            return Err(format!(
                "implicit module '{}' references model '{}' which was not found",
                name, sub_model_name,
            ));
        }
        let module_ident_context = module_ident_context_for_model(db, *source_model, &[]);
        let parsed = parse_source_variable_with_module_context(
            db,
            meta.parent_source_var,
            project,
            module_ident_context,
        );
        let input_prefix = format!("{name}\u{00B7}");
        let inputs: BTreeSet<Ident<Canonical>> =
            if let Some(datamodel::Variable::Module(dm_module)) =
                parsed.implicit_vars.get(meta.index_in_parent)
            {
                dm_module
                    .references
                    .iter()
                    .filter_map(|mr| {
                        let dst_canonical = canonicalize(&mr.dst);
                        let bare = dst_canonical.strip_prefix(&input_prefix)?;
                        Some(Ident::new(bare))
                    })
                    .collect()
            } else {
                BTreeSet::new()
            };

        let key = Ident::<Canonical>::new(sub_model_name);
        let is_new = !modules.contains_key(&key);

        modules.entry(key).or_default().insert(inputs);

        if is_new {
            enumerate_module_instances_inner(db, project, sub_model_name, modules)?;
        }
    }

    // Include LTM implicit MODULE variables (e.g. PREVIOUS instances from
    // feedback loop instrumentation). These are only present when LTM is
    // enabled and exist only in the root model.
    if project.ltm_enabled(db) {
        let ltm_implicit = db_ltm::model_ltm_implicit_var_info(db, *source_model, project);
        let ltm_module_idents = db_ltm::ltm_module_idents(db, *source_model, project);

        let ltm_vars = if project.ltm_discovery_mode(db) {
            model_ltm_all_link_synthetic_variables(db, *source_model, project)
        } else {
            model_ltm_synthetic_variables(db, *source_model, project)
        };

        for ltm_var in &ltm_vars.vars {
            let parsed = db_ltm::parse_ltm_var_with_ids(db, ltm_var, project, &ltm_module_idents);

            for implicit_dm_var in &parsed.implicit_vars {
                let im_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
                if let Some(im_meta) = ltm_implicit.get(&im_name) {
                    if !im_meta.is_module {
                        continue;
                    }
                    let sub_model_name = match &im_meta.model_name {
                        Some(n) => n,
                        None => continue,
                    };
                    let sub_canonical = canonicalize(sub_model_name);
                    if !project_models.contains_key(sub_canonical.as_ref()) {
                        continue;
                    }

                    // Extract input set from the implicit module's references
                    let input_prefix = format!("{im_name}\u{00B7}");
                    let inputs: BTreeSet<Ident<Canonical>> =
                        if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
                            dm_module
                                .references
                                .iter()
                                .filter_map(|mr| {
                                    let dst_canonical = canonicalize(&mr.dst);
                                    let bare = dst_canonical.strip_prefix(&input_prefix)?;
                                    Some(Ident::new(bare))
                                })
                                .collect()
                        } else {
                            BTreeSet::new()
                        };

                    let key = Ident::<Canonical>::new(sub_model_name);
                    let is_new = !modules.contains_key(&key);

                    modules.entry(key).or_default().insert(inputs);

                    if is_new {
                        enumerate_module_instances_inner(db, project, sub_model_name, modules)?;
                    }
                }
            }
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
    is_root: bool,
) -> HashMap<Ident<Canonical>, (usize, usize)> {
    use crate::common::{Canonical, Ident};
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
        let size =
            if let Some(svar) = source_vars.get(ident.as_str()) {
                if svar.kind(db) == SourceVariableKind::Module {
                    let sub_model_name = svar.model_name(db);
                    let sub_offsets =
                        calc_flattened_offsets_incremental(db, project, sub_model_name, false);
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
                                let subscripted_ident = Ident::<Canonical>::from_unchecked(
                                    format!("{}[{}]", ident_canonical.to_source_repr(), subscript),
                                );
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

    // Include implicit variables (SMOOTH, DELAY, TREND builtins) after explicit variables.
    // Implicit MODULE vars (from builtin expansion) occupy their sub-model's full
    // slot count, mirroring compute_layout's handling at the VariableLayout level.
    let implicit_info = model_implicit_var_info(db, *source_model, project);
    let mut implicit_names: Vec<&String> = implicit_info.keys().collect();
    implicit_names.sort_unstable();
    for name in implicit_names {
        let info = &implicit_info[name];
        let ident_canonical = Ident::new(name.as_str());

        if info.is_module {
            if let Some(sub_model_name) = &info.model_name {
                let sub_offsets =
                    calc_flattened_offsets_incremental(db, project, sub_model_name, false);
                let mut sub_var_names: Vec<&Ident<Canonical>> = sub_offsets.keys().collect();
                sub_var_names.sort_unstable();
                for sub_name in &sub_var_names {
                    let (sub_off, sub_size) = sub_offsets[*sub_name];
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
                i += sub_size;
            } else {
                offsets.insert(
                    Ident::<Canonical>::from_unchecked(ident_canonical.to_source_repr()),
                    (i, info.size),
                );
                i += info.size;
            }
        } else {
            offsets.insert(
                Ident::<Canonical>::from_unchecked(ident_canonical.to_source_repr()),
                (i, info.size),
            );
            i += info.size;
        }
    }

    // Include LTM variables (loop scores, relative loop scores, and their
    // implicit PREVIOUS module instances) when LTM is enabled and this is
    // the root model. These occupy slots after the implicit variables,
    // matching compute_layout's Section 3 ordering.
    if is_root && project.ltm_enabled(db) {
        let layout = compute_layout(db, *source_model, project, true);

        // Enumerate all LTM variable names from the synthetic variables list
        // and their implicit PREVIOUS module variables.
        let ltm_vars = if project.ltm_discovery_mode(db) {
            model_ltm_all_link_synthetic_variables(db, *source_model, project)
        } else {
            model_ltm_synthetic_variables(db, *source_model, project)
        };

        let ltm_implicit = db_ltm::model_ltm_implicit_var_info(db, *source_model, project);
        let ltm_module_idents = db_ltm::ltm_module_idents(db, *source_model, project);

        // Add explicit LTM variables (loop scores, relative loop scores)
        for ltm_var in &ltm_vars.vars {
            let canonical_name = canonicalize(&ltm_var.name);
            if let Some(entry) = layout.get(&canonical_name) {
                offsets.insert(
                    Ident::<Canonical>::from_unchecked(
                        Ident::<Canonical>::new(&canonical_name).to_source_repr(),
                    ),
                    (entry.offset, entry.size),
                );
            }

            // Add implicit PREVIOUS module variables from this LTM equation
            let parsed = db_ltm::parse_ltm_var_with_ids(db, ltm_var, project, &ltm_module_idents);
            for implicit_dm_var in &parsed.implicit_vars {
                let im_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
                if let Some(im_meta) = ltm_implicit.get(&im_name)
                    && let Some(entry) = layout.get(&im_name)
                {
                    if im_meta.is_module {
                        // Module-type: include sub-model variable offsets
                        if let Some(sub_model_name) = &im_meta.model_name {
                            let sub_offsets = calc_flattened_offsets_incremental(
                                db,
                                project,
                                sub_model_name,
                                false,
                            );
                            let mut sub_var_names: Vec<&Ident<Canonical>> =
                                sub_offsets.keys().collect();
                            sub_var_names.sort_unstable();
                            let im_ident = Ident::new(im_name.as_str());
                            for sub_name in &sub_var_names {
                                let (sub_off, sub_size) = sub_offsets[*sub_name];
                                let sub_canonical = Ident::new(sub_name.as_str());
                                offsets.insert(
                                    Ident::<Canonical>::from_unchecked(format!(
                                        "{}.{}",
                                        im_ident.to_source_repr(),
                                        sub_canonical.to_source_repr()
                                    )),
                                    (entry.offset + sub_off, sub_size),
                                );
                            }
                        }
                    } else {
                        offsets.insert(
                            Ident::<Canonical>::from_unchecked(
                                Ident::<Canonical>::new(&im_name).to_source_repr(),
                            ),
                            (entry.offset, entry.size),
                        );
                    }
                }
            }
        }
    }

    offsets
}

/// Set the `ltm_enabled` flag on a `SourceProject` salsa input.
///
/// This is a thin wrapper around the salsa-generated setter so that
/// downstream crates (e.g. libsimlin) can toggle LTM without taking
/// a direct dependency on the salsa crate.
pub fn set_project_ltm_enabled(db: &mut SimlinDb, project: SourceProject, enabled: bool) {
    use salsa::Setter;
    if project.ltm_enabled(db) != enabled {
        project.set_ltm_enabled(db).to(enabled);
    }
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
mod conversion_tests {
    use super::*;

    #[test]
    fn source_dimension_preserves_element_level_mappings() {
        let dim = datamodel::Dimension {
            name: "dim_a".to_string(),
            elements: datamodel::DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
            mappings: vec![datamodel::DimensionMapping {
                target: "dim_b".to_string(),
                element_map: vec![
                    ("a1".to_string(), "b2".to_string()),
                    ("a2".to_string(), "b1".to_string()),
                ],
            }],
        };
        let source: SourceDimension = SourceDimension::from(&dim);
        let roundtripped = source_dims_to_datamodel(&[source]);
        assert_eq!(roundtripped.len(), 1);
        assert_eq!(roundtripped[0].mappings.len(), 1);
        assert_eq!(roundtripped[0].mappings[0].target, "dim_b");
        assert_eq!(roundtripped[0].mappings[0].element_map.len(), 2);
    }

    #[test]
    fn source_dimension_preserves_multi_target_positional_mappings() {
        let dim = datamodel::Dimension {
            name: "dim_a".to_string(),
            elements: datamodel::DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
            mappings: vec![
                datamodel::DimensionMapping {
                    target: "dim_b".to_string(),
                    element_map: vec![],
                },
                datamodel::DimensionMapping {
                    target: "dim_c".to_string(),
                    element_map: vec![],
                },
            ],
        };
        let source: SourceDimension = SourceDimension::from(&dim);
        let roundtripped = source_dims_to_datamodel(&[source]);
        assert_eq!(roundtripped.len(), 1);
        assert_eq!(
            roundtripped[0].mappings.len(),
            2,
            "both positional mappings must survive DB round-trip"
        );
    }

    #[test]
    fn source_equation_preserves_default_equation() {
        let eq = datamodel::Equation::Arrayed(
            vec!["DimA".to_string()],
            vec![("A1".to_string(), "5".to_string(), None, None)],
            Some("default_val".to_string()),
        );
        let source = SourceEquation::from(&eq);
        let roundtripped = source_equation_to_datamodel(&source);
        match &roundtripped {
            datamodel::Equation::Arrayed(_, _, default_eq) => {
                assert_eq!(
                    default_eq.as_deref(),
                    Some("default_val"),
                    "default_equation must survive DB round-trip"
                );
            }
            _ => panic!("Expected Arrayed equation"),
        }
    }
}

#[cfg(test)]
#[path = "db_tests.rs"]
mod db_tests;

#[cfg(test)]
#[path = "db_diagnostic_tests.rs"]
mod db_diagnostic_tests;
