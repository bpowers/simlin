// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};

use salsa::Accumulator;
use salsa::plumbing::AsId;

use crate::canonicalize;
use crate::common::{Canonical, EquationError, Error, Ident, UnitError};
use crate::datamodel;

// The LTM reference-site classification IR (`model_ltm_reference_sites`) and
// the `Expr2` AST-walker helpers it owns, plus the per-project macro-registry
// salsa query, the dt-phase dependency-graph cycle relation, the per-variable
// fragment lowering, and the unit-check pass. Each lives in its own file so
// `db.rs` stays under the per-file line cap (`scripts/lint-project.sh`
// rule 2); they reach each other and the parent via `crate::db::...`.
mod dep_graph;
#[cfg(test)]
mod element_graph_proptest;
mod ltm_ir;
mod macro_registry;
mod units;
mod var_fragment;

mod ltm;
use ltm::*;
pub use ltm::{
    LtmImplicitVarMeta, compile_ltm_var_fragment, link_score_equation_text_shaped,
    model_ltm_implicit_var_info, model_ltm_variables,
};

mod analysis;
pub use analysis::RefShape;
pub use analysis::causal_graph_from_edges;
pub use analysis::causal_graph_from_element_edges;
pub(crate) use analysis::reconstruct_model_variables;
use analysis::*;
// `model_element_loop_circuits` is `#[deprecated]` for LTM consumers (the
// LTM pipeline uses `model_loop_circuits_tiered` instead). The re-export
// itself triggers the deprecation lint, but we need to keep it visible
// for legacy diagnostic / measurement-postscript callers in the test
// suite and the `ltm_full_bench` example. New callers see the
// deprecation warning automatically; existing callers are reviewed
// individually.
#[allow(deprecated)]
pub use analysis::model_element_loop_circuits;
pub use analysis::{
    CausalEdgesResult, CyclePartitionsResult, DetectedLoop, DetectedLoopPolarity,
    DetectedLoopsResult, EdgeShapesResult, ElementCausalEdgesResult, FastPathCircuit,
    LoopCircuitsResult, TieredCircuitsResult, compute_link_polarities, model_causal_edges,
    model_cycle_partitions, model_detected_loops, model_edge_shapes, model_element_causal_edges,
    model_element_cycle_partitions, model_loop_circuits, model_loop_circuits_tiered,
};

mod implicit_deps;
pub use implicit_deps::ImplicitVarDeps;
use implicit_deps::extract_implicit_var_deps;

// ── Database ───────────────────────────────────────────────────────────

#[salsa::db]
pub trait Db: salsa::Database {}

#[salsa::db]
#[derive(Default)]
pub struct SimlinDb {
    storage: salsa::Storage<Self>,
    /// Salsa input handles from the most recent sync. Owned by the db so
    /// callers get incrementality automatically (via `sync`/`sync_staged`)
    /// without threading `prev_state` between calls. A plain non-salsa field
    /// is fine: the `#[salsa::db]` macro locates `storage` by type, and this
    /// field is only ever mutated via `&mut self` during sync (never during
    /// parallel query execution, which uses a shared `&`), so no interior
    /// mutability is required.
    sync_state: Option<PersistentSyncState>,
}

#[salsa::db]
impl salsa::Database for SimlinDb {}

impl SimlinDb {
    /// Sync a datamodel into the db, automatically reusing internal state for
    /// incrementality. Returns the `SourceProject` handle for the synced
    /// project.
    ///
    /// This is the blessed entry point: it threads the db's own `sync_state`
    /// so a no-op re-sync of the same datamodel still hits the salsa caches,
    /// without the caller having to remember to pass the prior state.
    pub fn sync(&mut self, project: &datamodel::Project) -> SourceProject {
        // `take()` is required: `sync_from_datamodel_incremental` borrows
        // `&mut self`, and the `prev` argument cannot simultaneously borrow
        // `self.sync_state`. Move it out to an owned local first, then store
        // the result back.
        let prev = self.sync_state.take();
        let new = sync_from_datamodel_incremental(self, project, prev.as_ref());
        let sp = new.project;
        self.sync_state = Some(new);
        sp
    }

    /// Sync `project` and ALSO return the prior state so the caller can roll
    /// back (re-sync the prior datamodel) on validation failure. Used by the
    /// patch stage/commit/rollback flow.
    ///
    /// The returned `Option<PersistentSyncState>` is the PRE-staging handle
    /// set, required for an exact rollback via `restore`.
    pub fn sync_staged(
        &mut self,
        project: &datamodel::Project,
    ) -> (SourceProject, Option<PersistentSyncState>) {
        let prev = self.sync_state.take();
        let new = sync_from_datamodel_incremental(self, project, prev.as_ref());
        let sp = new.project;
        self.sync_state = Some(new);
        (sp, prev)
    }

    /// Roll a staged sync back: re-sync `project` reusing the explicitly
    /// provided prior state, restoring the inputs' prior field values
    /// (and dropping variables added during staging).
    pub fn restore(&mut self, project: &datamodel::Project, prev: Option<PersistentSyncState>) {
        let restored = sync_from_datamodel_incremental(self, project, prev.as_ref());
        self.sync_state = Some(restored);
    }

    /// The `SourceProject` from the most recent sync, if any.
    pub fn current_source_project(&self) -> Option<SourceProject> {
        self.sync_state.as_ref().map(|s| s.project)
    }
}

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

/// Interned identity for a module instance's input-variable wiring: the
/// sorted, canonical names of the variables a parent supplies to a sub-model
/// instance (the `isModuleInput(...)` set). Replaces the per-query
/// `Vec<String>` module-input key that salsa hashed string-by-string on every
/// lookup, and the `Option::None`/empty-`Vec` "no inputs" sentinel: because
/// salsa interning deduplicates, the empty set (`ModuleInputSet::empty`) is a
/// single id shared across all no-input callers, so the common no-inputs case
/// collapses to one cache entry per query rather than one per caller.
#[salsa::interned(debug)]
pub struct ModuleInputSet<'db> {
    #[returns(ref)]
    pub names: Vec<String>,
}

impl<'db> ModuleInputSet<'db> {
    /// The canonical no-inputs key. Because interning deduplicates, this is the
    /// same id every time, so it shares one cache entry across all callers.
    pub fn empty(db: &'db dyn Db) -> Self {
        ModuleInputSet::new(db, Vec::new())
    }

    /// Build a `ModuleInputSet` from the canonical module-input idents the
    /// dependency/assembly logic consumes. The stored `names` are the sorted
    /// canonical strings, so a round-trip back through `canonical_input_set`
    /// (or `Ident::new`, idempotent on an already-canonical string) reproduces
    /// the original `BTreeSet<Ident<Canonical>>` exactly.
    pub fn from_canonical_set(db: &'db dyn Db, inputs: &BTreeSet<Ident<Canonical>>) -> Self {
        // `BTreeSet` already iterates in sorted order, so the resulting `Vec`
        // is sorted; collecting from it preserves the canonical ordering the
        // interning key relies on for deduplication.
        let names: Vec<String> = inputs.iter().map(|id| id.as_str().to_owned()).collect();
        ModuleInputSet::new(db, names)
    }

    /// Build a `ModuleInputSet` from raw (possibly non-canonical, unsorted)
    /// module-input name strings, canonicalizing and sorting them. This is the
    /// exact inverse of `ModuleInputSet::names` for an interned set built from
    /// canonical idents (canonicalization is idempotent on canonical strings),
    /// and reproduces the old `canonical_module_input_set` derivation so the
    /// dependency classification is byte-identical.
    pub fn from_names(db: &'db dyn Db, names: &[String]) -> Self {
        let canonical = canonical_module_input_set(names);
        ModuleInputSet::from_canonical_set(db, &canonical)
    }

    /// Reconstruct the `BTreeSet<Ident<Canonical>>` the assembly/dependency
    /// logic consumes. The exact inverse of `from_canonical_set`: each stored
    /// name is already canonical, so `Ident::new` is idempotent.
    pub fn canonical_input_set(self, db: &'db dyn Db) -> BTreeSet<Ident<Canonical>> {
        self.names(db)
            .iter()
            .map(|name| Ident::<Canonical>::new(name))
            .collect()
    }
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
    pub sim_specs: datamodel::SimSpecs,
    #[returns(ref)]
    pub dimensions: Vec<datamodel::Dimension>,
    #[returns(ref)]
    pub units: Vec<datamodel::Unit>,
    #[returns(ref)]
    pub model_names: Vec<String>,
    #[returns(ref)]
    pub models: HashMap<String, SourceModel>,
    /// The ordered, pre-dedup macro-declaration list: one entry per
    /// *project*-declared model (NOT stdlib models), in datamodel
    /// declaration order, carrying the model's CANONICAL name and its
    /// `macro_spec.clone()`. This is the minimal raw data
    /// `project_macro_registry` needs to re-derive the AC5.3 duplicate /
    /// collision verdict (Passes 1-2 of `MacroRegistry::build`), which
    /// `models` -- a name-keyed `HashMap` that collapses duplicate /
    /// colliding model names -- cannot supply. Declaration order is
    /// load-bearing: the build error reports the FIRST-detected duplicate /
    /// collision, so the list must preserve the datamodel's model order.
    /// `datamodel::MacroSpec` derives `salsa::Update`, so this field type is
    /// well-formed. See `crate::db::macro_registry`.
    #[returns(ref)]
    pub macro_declarations: Vec<(String, Option<datamodel::MacroSpec>)>,
    /// Whether LTM (Loops That Matter) synthetic variable compilation is
    /// enabled. When true, `compute_layout` allocates slots and
    /// `assemble_module` compiles fragments for LTM variables.
    pub ltm_enabled: bool,
    /// When true, use discovery mode (`model_ltm_variables` with all links)
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
    pub model_sim_specs: Option<datamodel::SimSpecs>,
    /// `Some` iff this model is a callable macro template. On the salsa
    /// input so `project_macro_registry` is keyed on the macro-marked
    /// models (editing a non-macro variable does not invalidate it).
    #[returns(ref)]
    pub macro_spec: Option<datamodel::MacroSpec>,
}

#[salsa::input]
pub struct SourceVariable {
    #[returns(ref)]
    pub ident: String,
    #[returns(ref)]
    pub equation: datamodel::Equation,
    pub kind: SourceVariableKind,
    #[returns(ref)]
    pub units: Option<String>,
    #[returns(ref)]
    pub gf: Option<datamodel::GraphicalFunction>,
    #[returns(ref)]
    pub inflows: Vec<String>,
    #[returns(ref)]
    pub outflows: Vec<String>,
    #[returns(ref)]
    pub module_refs: Vec<datamodel::ModuleReference>,
    #[returns(ref)]
    pub model_name: String,
    pub non_negative: bool,
    pub can_be_module_input: bool,
    #[returns(ref)]
    pub compat: datamodel::Compat,
}

/// Whether a source variable is a standalone lookup-only table: a
/// graphical-function holder with an empty-or-sentinel equation and no real
/// functional input. Such a variable is a *table indexed by an explicit input*
/// (`y = table(input)`), not a value-bearing variable -- it is excluded from the
/// runlist and produces no saved series (issue #606). This is the salsa-layer
/// twin of `crate::variable::var_is_lookup_only`, evaluated over the
/// `datamodel::Equation` + `datamodel::GraphicalFunction` representation; both
/// delegate to the shared `crate::variable::is_empty_or_sentinel` core (which
/// also accepts the legacy `"0+0"` sentinel for back-compat).
///
/// Salsa-tracked so its `bool` output backdates: callers in tracked contexts
/// (`build_var_info` -> `model_dependency_graph`, `calc_flattened_offsets`)
/// must NOT gain a fine-grained dependency on a variable's equation TEXT, which
/// would invalidate the dependency graph on every unrelated equation edit.
#[salsa::tracked]
pub(crate) fn source_var_is_table_only(db: &dyn Db, var: SourceVariable) -> bool {
    use crate::variable::is_empty_or_sentinel;
    match var.equation(db) {
        // Scalar / A2A: one equation string plus a variable-level gf.
        datamodel::Equation::Scalar(s) | datamodel::Equation::ApplyToAll(_, s) => {
            var.gf(db).is_some() && is_empty_or_sentinel(s)
        }
        // Arrayed: a pure per-element table holder iff it has tables (a
        // variable-level or any per-element gf) and EVERY element equation (and
        // the EXCEPT default, if any) is empty/sentinel. The per-element gf is
        // the 4th tuple field `(subscript, equation, gf_equation, gf)`.
        datamodel::Equation::Arrayed(_, elements, default, _) => {
            let has_tables =
                var.gf(db).is_some() || elements.iter().any(|(_, _, _, gf)| gf.is_some());
            has_tables
                && !elements.is_empty()
                && elements
                    .iter()
                    .all(|(_, eq, _, _)| is_empty_or_sentinel(eq))
                && default.as_deref().map(is_empty_or_sentinel).unwrap_or(true)
        }
    }
}

// ── Reconstruct helpers ────────────────────────────────────────────────

/// Build a `datamodel::Variable` from the per-field `SourceVariable` salsa
/// input for use with the existing parsing pipeline (parse_var,
/// lower_variable). The input stores the datamodel `Equation`/`GraphicalFunction`/
/// `ModuleReference` fields directly, so this is a cheap re-assembly into the
/// kind-tagged enum the parser expects rather than a structural conversion.
///
/// The fields the salsa input does not carry -- `documentation`, `ai_state`,
/// `uid` -- are reconstructed as empty/None: parsing and lowering ignore them,
/// so their absence is semantically identical to the original datamodel value.
/// `compat.non_negative`/`can_be_module_input` are taken from the dedicated
/// scalar input fields (the canonical source for those flags after sync).
pub fn datamodel_variable_from_source(db: &dyn Db, var: SourceVariable) -> datamodel::Variable {
    let ident = var.ident(db).clone();
    let equation = var.equation(db).clone();
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
            gf: var.gf(db).clone(),
            ai_state: None,
            uid: None,
            compat,
        }),
        SourceVariableKind::Aux => datamodel::Variable::Aux(datamodel::Aux {
            ident,
            equation,
            documentation: String::new(),
            units,
            gf: var.gf(db).clone(),
            ai_state: None,
            uid: None,
            compat,
        }),
        SourceVariableKind::Module => datamodel::Variable::Module(datamodel::Module {
            ident,
            model_name: var.model_name(db).clone(),
            documentation: String::new(),
            units,
            references: var.module_refs(db).clone(),
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
/// Subsumes the per-variable Context::new_with_builtins calls.
///
/// Reads the datamodel `Vec<Unit>` and `SimSpecs` directly off the salsa input
/// (the inputs now store the datamodel types, so no per-call conversion is
/// needed).
///
/// Unit definition parsing errors are accumulated as diagnostics so they
/// appear in `collect_all_diagnostics`.
#[salsa::tracked(returns(ref))]
pub fn project_units_context(db: &dyn Db, project: SourceProject) -> crate::units::Context {
    let dm_units = project.units(db);
    let dm_sim_specs = project.sim_specs(db);
    // Construction is partial: keep the context built from the valid unit
    // declarations and surface each conflicting/duplicate declaration as a
    // project-level diagnostic, rather than discarding every unit definition on
    // the first conflict. An empty context would lose all alias normalization
    // project-wide (yr/year, person/people, model-defined equivalences) and
    // re-create a spurious unit-mismatch flood -- the context-layer parallel of
    // the inference partial-results fix (GH #614).
    let (ctx, unit_parse_errors) = crate::units::Context::new_with_builtins(dm_units, dm_sim_specs);
    for (unit_name, eq_errors) in &unit_parse_errors {
        for eq_err in eq_errors {
            CompilationDiagnostic(Diagnostic {
                model: String::new(),
                variable: Some(unit_name.clone()),
                error: DiagnosticError::Unit(crate::common::UnitError::DefinitionError(
                    eq_err.clone(),
                    None,
                )),
                severity: DiagnosticSeverity::Error,
            })
            .accumulate(db);
        }
    }
    ctx
}

/// Cached datamodel dimensions -- computed once per project.
///
/// The dimensions input now stores `Vec<datamodel::Dimension>` directly, so
/// this is a clone of the input field. It is retained as a tracked function
/// so downstream queries (`project_dimensions_context`, `parse_source_variable`)
/// keep their existing `returns(ref)` dependency edge on it.
#[salsa::tracked(returns(ref))]
pub fn project_datamodel_dims(db: &dyn Db, project: SourceProject) -> Vec<datamodel::Dimension> {
    project.dimensions(db).clone()
}

/// Cached project-global dimension context -- computed once per project.
///
/// This is the project's immutable `DimensionsContext`, the same value the
/// per-variable compile sites used to rebuild on every variable via
/// `DimensionsContext::from(project_datamodel_dims(..))`. Building it
/// canonicalizes every dimension element name, so doing it once per project
/// (instead of once per explicit-and-implicit variable compilation) removes a
/// dominant allocation cost on large models. Keyed only on `project` -- and
/// reading `project_datamodel_dims`, which depends solely on the project's
/// dimensions input -- so it recomputes exactly when the dimensions change, the
/// SAME dependency granularity the inline rebuild took. The shared context's
/// only interior mutability is its `relationship_cache` `Mutex`, so it is safe
/// to share across the rayon-parallel variable compilations (and the
/// subdimension-relationship memo is now computed once and reused rather than
/// discarded per variable).
#[salsa::tracked(returns(ref))]
pub fn project_dimensions_context(
    db: &dyn Db,
    project: SourceProject,
) -> crate::dimensions::DimensionsContext {
    crate::dimensions::DimensionsContext::from(project_datamodel_dims(db, project).as_slice())
}

/// Cached project-global converted dimensions -- computed once per project.
///
/// The `Vec<crate::dimensions::Dimension>` form of `project_datamodel_dims`,
/// previously rebuilt per variable via
/// `project_datamodel_dims(..).iter().map(Dimension::from).collect()`. Each
/// `Dimension::from` re-canonicalizes the dimension's element names, so caching
/// it once per project removes that repeated work. Same input dependency
/// (`project_datamodel_dims`) and hence same invalidation granularity as the
/// inline rebuild. The interned-backed `Dimension`s clone cheaply, so a
/// consumer that genuinely needs an owned `Vec` can still `.to_vec()` the slice.
#[salsa::tracked(returns(ref))]
pub fn project_converted_dimensions(
    db: &dyn Db,
    project: SourceProject,
) -> Vec<crate::dimensions::Dimension> {
    project_datamodel_dims(db, project)
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect()
}

fn parse_source_variable_impl(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_idents: Option<&HashSet<Ident<Canonical>>>,
    macro_registry: Option<&crate::module_functions::MacroRegistry>,
) -> ParsedVariableResult {
    let relevant_dim_names = variable_relevant_dimensions(db, var);
    let dims: Vec<datamodel::Dimension> = if relevant_dim_names.is_empty() {
        // Scalar variable: no dim dependency, so dim changes don't invalidate.
        vec![]
    } else {
        let all_source_dims = project.dimensions(db);
        let expanded = expand_maps_to_chains(relevant_dim_names, all_source_dims);
        project_datamodel_dims(db, project)
            .iter()
            .filter(|d| expanded.contains(&d.name))
            .cloned()
            .collect()
    };
    let units_ctx = project_units_context(db, project);
    let dm_var = datamodel_variable_from_source(db, var);
    let mut implicit_vars = Vec::new();
    let variable = crate::variable::parse_var_with_module_context(
        &dims,
        &dm_var,
        &mut implicit_vars,
        units_ctx,
        |mi| Ok(Some(mi.clone())),
        module_idents,
        macro_registry,
        crate::db::macro_registry::enclosing_macro_for_var(db, project, var), // #554
    );

    ParsedVariableResult {
        variable,
        implicit_vars,
    }
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
    // Reaches the BuiltinVisitor so a macro call expands (salsa-cached).
    let macro_registry = &crate::db::macro_registry::project_macro_registry(db, project).registry;
    parse_source_variable_impl(db, var, project, Some(&module_idents), Some(macro_registry))
}

fn module_ident_context_for_model<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    extra_module_idents: &[String],
) -> ModuleIdentContext<'db> {
    let source_vars = model.variables(db);
    let dm_vars: Vec<datamodel::Variable> = source_vars
        .values()
        .map(|source_var| datamodel_variable_from_source(db, *source_var))
        .collect();
    // Pre-classification must recognize macro calls as module calls too
    // (so `PREVIOUS(y)` rewrites correctly when `y = MYMACRO(...)`), the
    // same way it already recognizes `y = SMTH1(...)`.
    let macro_registry = &crate::db::macro_registry::project_macro_registry(db, project).registry;
    let mut module_ident_list: Vec<String> =
        crate::model::collect_module_idents(&dm_vars, macro_registry)
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
pub fn model_module_ident_context<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    extra_module_idents: Vec<String>,
) -> ModuleIdentContext<'db> {
    module_ident_context_for_model(db, model, project, &extra_module_idents)
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
    /// Variables referenced only through INIT(...) in dt AST (pruned only for dt ordering).
    pub dt_init_only_referenced_vars: BTreeSet<String>,
    /// Variables referenced *only* through PREVIOUS(...) in the normal dt AST.
    pub dt_previous_referenced_vars: BTreeSet<String>,
    /// Variables referenced *only* through PREVIOUS(...) in the initial AST.
    pub initial_previous_referenced_vars: BTreeSet<String>,
    /// Standalone lookup tables referenced via `LOOKUP(table, x)`. These are
    /// layout references (codegen needs the table's offset for its reverse-map)
    /// but NOT data-flow dependencies, so they are kept out of `dt_deps` /
    /// `initial_deps` (no runlist-ordering or causal/LTM edge) and reunited with
    /// the dep set only by the fragment compiler's metadata/tables build (#606).
    pub referenced_tables: BTreeSet<String>,
}

fn canonical_module_input_set(module_input_names: &[String]) -> BTreeSet<Ident<Canonical>> {
    module_input_names
        .iter()
        .map(|name| Ident::new(canonicalize(name).as_ref()))
        .collect()
}

fn variable_direct_dependencies_impl(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
    module_ident_context: ModuleIdentContext,
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
                dt_init_only_referenced_vars: BTreeSet::new(),
                dt_previous_referenced_vars: BTreeSet::new(),
                initial_previous_referenced_vars: BTreeSet::new(),
                // A module never references a lookup table via LOOKUP(...).
                referenced_tables: BTreeSet::new(),
            }
        }
        _ => {
            let parsed =
                parse_source_variable_with_module_context(db, var, project, module_ident_context);
            // The datamodel-form dims are still needed for the implicit-var
            // parse below; the canonicalized context + converted dims come from
            // the project-global salsa-cached queries (no per-variable rebuild).
            let dims = project_datamodel_dims(db, project);
            let dim_context = project_dimensions_context(db, project);
            let converted_dims = project_converted_dimensions(db, project);
            let models = HashMap::new();
            let scope = crate::model::ScopeStage0 {
                models: &models,
                dimensions: dim_context,
                model_name: "",
            };
            let lowered = crate::model::lower_variable(&scope, &parsed.variable);

            // Two calls to classify_dependencies replace 7 separate walker calls.
            let dt_classification = match lowered.ast() {
                Some(ast) => {
                    crate::variable::classify_dependencies(ast, converted_dims, module_inputs)
                }
                None => crate::variable::DepClassification::default(),
            };
            let init_classification = match lowered.init_ast() {
                Some(ast) => {
                    crate::variable::classify_dependencies(ast, converted_dims, module_inputs)
                }
                None => crate::variable::DepClassification::default(),
            };

            let implicit_vars =
                extract_implicit_var_deps(parsed, dims, dim_context, converted_dims, module_inputs);

            VariableDeps {
                dt_deps: dt_classification
                    .all
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                initial_deps: init_classification
                    .all
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                implicit_vars,
                init_referenced_vars: dt_classification.init_referenced,
                dt_init_only_referenced_vars: dt_classification.init_only,
                dt_previous_referenced_vars: dt_classification.previous_only,
                initial_previous_referenced_vars: init_classification.previous_only,
                referenced_tables: dt_classification
                    .referenced_tables
                    .into_iter()
                    .chain(init_classification.referenced_tables)
                    .collect(),
            }
        }
    }
}

/// Per-variable direct dependency extraction.
///
/// `module_ident_context` is the caller's module-ident set (the empty
/// `ModuleIdentContext` for callers whose model is unknown), and
/// `module_inputs` is the module instance's input wiring (the empty
/// `ModuleInputSet` for the no-inputs case).
///
/// This collapses what were four separately-keyed tracked variants
/// (`variable_direct_dependencies`, `_with_inputs`, `_with_context`,
/// `_with_context_and_inputs`) into one. The empty `ModuleInputSet` is treated
/// IDENTICALLY to the old `None`-inputs path: `classify_dependencies`
/// special-cases an `isModuleInput(...)` conditional only when given
/// `Some(inputs)` (then walks the matching branch), so `None` and
/// `Some(empty_set)` are NOT equivalent there -- `None` walks all three
/// branches. The old no-inputs variants passed `None`, and the only live
/// inputs caller (`build_var_info`) passed `Some(..)` exclusively for a
/// non-empty set. Mapping the empty `ModuleInputSet` to `None` therefore
/// preserves the old behavior exactly.
#[salsa::tracked(returns(ref))]
pub fn variable_direct_dependencies<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    project: SourceProject,
    module_ident_context: ModuleIdentContext<'db>,
    module_inputs: ModuleInputSet<'db>,
) -> VariableDeps {
    let canonical_inputs = module_inputs.canonical_input_set(db);
    // An empty input set is the old `None` path (no `isModuleInput` branch
    // selection); a non-empty set is the old `Some(&set)` path.
    let module_inputs_opt = if canonical_inputs.is_empty() {
        None
    } else {
        Some(&canonical_inputs)
    };
    variable_direct_dependencies_impl(db, var, project, module_inputs_opt, module_ident_context)
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
    let module_ident_context = model_module_ident_context(db, model, project, vec![]);
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

/// Which dependency phase a resolved recurrence SCC belongs to.
///
/// `model_dependency_graph_impl` runs the cycle gate twice -- once for
/// the dt-phase relation and once for the init-phase relation. A
/// `ResolvedScc` records which run proved its element graph acyclic so
/// the consumer applies the right per-element order. Phase 1 only
/// produces `Dt` (single-variable dt self-recurrence); `Initial` is
/// reserved for the Phase 2 init-cycle resolution.
///
/// Derives the same trait set as `ModelDepGraphResult` (it is reachable
/// from a salsa return value, so it must participate in salsa equality).
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub enum SccPhase {
    Dt,
    Initial,
}

/// A recurrence SCC whose induced element graph the cycle gate proved
/// acyclic. `members` is byte-stable (BTreeSet); `element_order` is the
/// per-element topological evaluation order `(member, element-offset)`.
///
/// Reachable from `ModelDepGraphResult` (a salsa return value), so it
/// derives the identical trait set -- in particular `PartialEq`/`Eq`/
/// `salsa::Update` so a change in the resolved-SCC set invalidates the
/// salsa cache. `Ident<Canonical>` derives `Ord` + `salsa::Update`,
/// which makes the `BTreeSet`/`Vec` field types well-formed here.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ResolvedScc {
    pub members: BTreeSet<Ident<Canonical>>,
    pub element_order: Vec<(Ident<Canonical>, usize)>,
    pub phase: SccPhase,
}

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ModelDepGraphResult {
    /// Interned-ident-keyed dependency maps. `Ident<Canonical>` derives
    /// `salsa::Update`, and salsa's blanket impls cover
    /// `std::collections::HashMap<K,V>` / `BTreeSet<K>` for `K,V: Update`, so
    /// these stay on the std `HashMap` (default hasher) -- only the hot
    /// internal working maps in `model_dependency_graph_impl` use FxHash.
    /// `Ident<Canonical>` keys/values are cheap Arc-refcount clones and the
    /// `BTreeSet`s iterate in the same lexicographic order the former
    /// `BTreeSet<String>` did, so a consumer probing by `&str` (via
    /// `Borrow<str>`) sees byte-identical behavior.
    pub dt_dependencies: HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>,
    pub initial_dependencies: HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>,
    pub runlist_initials: Vec<String>,
    pub runlist_flows: Vec<String>,
    pub runlist_stocks: Vec<String>,
    pub has_cycle: bool,
    /// Recurrence SCCs whose induced element graph the cycle gate proved
    /// acyclic and element-sourceable, so they are resolved rather than
    /// rejected with `CircularDependency`. Empty on the acyclic happy
    /// path (zero extra work) and whenever the conservative loud-safe
    /// fallback fires. Populated by the Phase 1 Subcomponent B
    /// element-cycle refinement; every construction site initializes it
    /// explicitly (`Vec::new()` on the early-return/error paths).
    pub resolved_sccs: Vec<ResolvedScc>,
}

/// Per-model tracked dependency graph, keyed on the module-instance input
/// wiring (`module_inputs`). The empty `ModuleInputSet` is the no-inputs case;
/// because it is a single interned id, every no-input caller shares one cache
/// entry. Models instantiated with different input wiring can have different
/// dependency sets when `isModuleInput(...)` appears in equations.
///
/// The dependency-graph cycle gate itself
/// (`model_dependency_graph_impl`, the SCC-aware back-edge break, the
/// collapsed-node transitive accumulation) lives in `db/dep_graph.rs`
/// alongside the shared cycle relation it consumes
/// (`dt_walk_successors`/`init_walk_successors`/`build_var_info`/
/// `resolve_recurrence_sccs`) -- a `db` submodule, like
/// `ltm_ir`/`macro_registry`, only to keep `db.rs` under the
/// per-file line cap. This thin salsa wrapper stays here because the
/// `ModelDepGraphResult` salsa input/return types do.
#[salsa::tracked(returns(ref))]
pub fn model_dependency_graph<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    module_inputs: ModuleInputSet<'db>,
) -> ModelDepGraphResult {
    let module_input_names = module_inputs.names(db);
    crate::db::dep_graph::model_dependency_graph_impl(db, model, project, module_input_names)
}

// ── Diagnostic collection ──────────────────────────────────────────────

/// Per-model tracked function that triggers diagnostic accumulation from
/// all compilation stages. The salsa accumulator is the sole error source
/// for diagnostic reporting -- this function does not read struct fields.
///
/// Triggers three diagnostic sources:
/// 1. `compile_var_fragment` for each variable -- accumulates parse-level
///    equation errors (EmptyEquation, syntax errors), unit definition
///    syntax errors (bad unit strings), and compilation-level errors
///    (BadTable, MismatchedDimensions, etc.)
/// 2. `check_model_units` -- accumulates unit inference/checking warnings
/// 3. When LTM is enabled, `model_ltm_fragment_diagnostics` -- accumulates
///    LTM assembly diagnostics: the auto-flip warning that surfaces when
///    the element-level largest SCC exceeds `MAX_LTM_SCC_NODES` (emitted
///    by `model_ltm_variables`, which the fragment-diagnostic pass drives
///    internally), and a compile-failure warning for any LTM synthetic
///    variable whose fragment fails to compile. Gated on `ltm_enabled` so
///    we don't run LTM synthesis on projects that never requested it.
#[salsa::tracked]
pub fn model_all_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    let source_vars = model.variables(db);

    // Trigger compile_var_fragment for each variable. This is a superset
    // of parse_source_variable_with_module_context: it first accumulates
    // unit definition syntax errors from the parsed variable, then checks
    // for equation parse errors, then proceeds with compilation which can
    // surface additional errors like BadTable, MismatchedDimensions, etc.
    //
    // We use is_root: true and the empty module-input set for diagnostic
    // purposes. The is_root flag only affects offset layout (whether
    // implicit time/dt vars are included); using true ensures variables
    // referencing TIME or DT don't produce false-positive missing-ref
    // errors. The module inputs are empty because we are not in an
    // assembly context -- this is purely for error detection.
    let empty_inputs = ModuleInputSet::empty(db);
    for (_var_name, source_var) in source_vars.iter() {
        let _fragment = compile_var_fragment(db, *source_var, model, project, true, empty_inputs);
    }

    // Trigger unit checking. This is a separate tracked function so
    // that unit inference results are individually cached and
    // invalidated only when unit-relevant inputs change. It lives in the
    // `db::units` submodule (kept out of `db.rs` for the per-file line
    // cap).
    crate::db::units::check_model_units(db, model, project);

    // When LTM is enabled, also trigger the LTM diagnostic pass so that
    // diagnostics accumulated by the LTM pipeline surface through
    // `collect_all_diagnostics`: the auto-flip-to-discovery warning from
    // `model_ltm_variables` and the synthetic-fragment compile-failure
    // warning from `model_ltm_fragment_diagnostics`.
    // `model_ltm_fragment_diagnostics` drives `model_ltm_variables`
    // internally, so the auto-flip warning rides along. Without this
    // call the warnings are invisible to `simlin-mcp`/`libsimlin`
    // callers even though the LTM pipeline already emitted them. (GH
    // #466: this remains gated on `ltm_enabled`, which the
    // diagnostic-collection FFI paths leave false by default.)
    if project.ltm_enabled(db) {
        model_ltm_fragment_diagnostics(db, model, project);
    }
}

// ── LTM tracked functions ──────────────────────────────────────────────

/// A single LTM synthetic variable definition (name + equation).
///
/// `equation` carries its own dimensionality (`Equation::Scalar`,
/// `Equation::ApplyToAll`, or `Equation::Arrayed`). The redundant
/// `dimensions` field is retained because layout sizing (`compute_layout`)
/// and discovery-time offset parsing (`parse_link_offsets`) key off it;
/// every constructor keeps `equation`'s dimension names in lockstep with
/// `dimensions`. When `dimensions` is non-empty the variable occupies
/// `product(dim_lengths)` layout slots instead of 1.
///
/// `compile_directly` forces `assemble_module`'s LTM pass to compile this
/// var's `equation` verbatim instead of re-deriving it from the
/// `(from, to)`-keyed salsa cache (`compile_ltm_var_fragment` ->
/// `link_score_equation_text`, which always uses `RefShape::Bare`). It is
/// set by `emit_per_shape_link_scores` for a scalar link score whose
/// underlying reference shape is *not* `Bare` -- a `Wildcard`/`DynamicIndex`
/// reference into a scalar target (e.g. `total = arr[idx]`), where the salsa
/// path would wrap the whole subscript in `PREVIOUS()` and zero the
/// ceteris-paribus numerator. (Element-subscripted / `$⁚ltm⁚agg⁚{n}` link
/// scores already route directly via name checks; setting it for them is harmless.)
//
// `equation: datamodel::Equation` blocks deriving `Eq` (the embedded
// `GraphicalFunction` carries `f64` points) and unconditional `Debug`
// (datamodel types only derive `Debug` under `debug-derive`, off in WASM /
// pysimlin). Salsa only needs `PartialEq` for incrementality.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct LtmSyntheticVar {
    pub name: String,
    pub equation: datamodel::Equation,
    pub dimensions: Vec<String>,
    pub compile_directly: bool,
}

/// Result of LTM variable generation for a model.
///
/// `loop_partitions` maps each loop ID (as in `$⁚ltm⁚loop_score⁚{id}`) to
/// its cycle-partition index **per slot**: length 1 for scalar/cross-element/
/// mixed loops, one entry per element (in the runtime's row-major slot order)
/// for A2A loops, matching `ltm_post::build_loop_element_index`'s `n_slots`.
/// Slots sharing a `(partition, slot)` key form the denominator when
/// `ltm_post::compute_rel_loop_scores*` normalizes; an element-wise-uncoupled
/// A2A loop's entries are N distinct partitions (the per-slot fix, GH #487),
/// a coupled one's coincide, a `None` entry is a slot below the parent graph
/// (e.g. a pure module-internal loop).  Populated only in exhaustive LTM
/// mode; discovery mode leaves it empty.
///
/// `agg_recovery_truncated` is `true` when reconstruction of the
/// cross-element-through-aggregate loops (`recover_cross_agg_loops`, GH
/// #515) hit its loop-count budget (`ltm::MAX_CROSS_AGG_LOOPS`) or its
/// per-aggregate petal cap, so the recovered loop list is incomplete (a
/// `CompilationDiagnostic` `Warning` is also emitted then -- the flag is
/// the robust signal, the `Warning`'s reachability being #466's concern).
/// Always `false` in discovery mode and for models with no synthetic aggs.
/// (`Debug`/`Eq` are conditional/absent for the same reasons as
/// `LtmSyntheticVar`.)
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct LtmVariablesResult {
    pub vars: Vec<LtmSyntheticVar>,
    pub loop_partitions: HashMap<String, Vec<Option<usize>>>,
    pub agg_recovery_truncated: bool,
}

/// Compute the link score equation text for a single causal link.
///
/// This is the per-link granularity that enables incremental recomputation:
/// when a variable's equation changes, salsa only re-evaluates link score
/// equations for links whose endpoints are affected. Links involving
/// unmodified variables return their cached equation text.
/// Black-box delta-ratio formula for module links where we cannot do
/// ceteris-paribus analysis. Computes `delta_to / delta_from` --
/// the magnitude captures how much `to` changes per unit change in
/// `from`, and the sign captures the polarity of influence.
pub(super) fn black_box_delta_ratio_equation(from_ident: &str, to_ident: &str) -> String {
    let from_q = crate::ltm_augment::quote_ident(from_ident);
    let to_q = crate::ltm_augment::quote_ident(to_ident);
    format!(
        "if (TIME = INITIAL_TIME) then 0 \
         else if (({to_q} - PREVIOUS({to_q})) = 0) OR \
                 (({from_q} - PREVIOUS({from_q})) = 0) \
              then 0 \
         else (({to_q} - PREVIOUS({to_q})) / \
               ({from_q} - PREVIOUS({from_q})))"
    )
}

/// Find output ports of a specific module variable by examining which
/// variables in the model reference it with `module·internal_var` syntax.
pub(super) fn find_model_output_ports_for_module(
    edges: &CausalEdgesResult,
    module_var_name: &str,
) -> Vec<String> {
    // Look up the module's sub-model name. For stdlib modules the
    // output is always "output" by convention.
    if let Some(model_name) = edges.dynamic_modules.get(module_var_name)
        && model_name.starts_with("stdlib\u{205A}")
    {
        return vec!["output".to_string()];
    }
    // For user-defined modules, we'd need to scan variable deps for
    // module·var references. Since we don't have deps here, fall back
    // to "output" as a convention.
    vec!["output".to_string()]
}

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

    // Module-involved links: three cases depending on which end is a module.
    // 1. input -> module: composite reference to module's internal score
    // 2. module -> downstream: standard ceteris-paribus on downstream equation
    // 3. module -> module: black-box delta-ratio equation
    if from_is_module || to_is_module {
        let is_discovery = project.ltm_discovery_mode(db);
        let equation = if !from_is_module && to_is_module {
            if let crate::variable::Variable::Module { inputs, .. } = &to_var {
                if let Some(input) = inputs.iter().find(|i| i.src == from_ident) {
                    if is_discovery {
                        // In discovery mode, use delta-ratio between the input
                        // variable and the module's output variable. The composite
                        // reference works in exhaustive mode (where only loop
                        // edges are scored) but not in discovery mode because
                        // cross-module LTM variable references don't resolve.
                        //
                        // Find the module's output port by looking at which
                        // variables in the model depend on module·internal_var.
                        let edges = model_causal_edges(db, model, project);
                        let output_ports = find_model_output_ports_for_module(edges, to_name);
                        let output_ref = output_ports
                            .first()
                            .map(|port| format!("{}\u{00B7}{}", to_ident.as_str(), port))
                            .unwrap_or_else(|| format!("{}\u{00B7}output", to_ident.as_str()));
                        black_box_delta_ratio_equation(from_ident.as_str(), &output_ref)
                    } else {
                        // In exhaustive mode, reference the composite score
                        // of the input port inside the sub-model.
                        format!(
                            "\"{module}\u{00B7}$\u{205A}ltm\u{205A}composite\u{205A}{port}\"",
                            module = to_ident.as_str(),
                            port = input.dst.as_str(),
                        )
                    }
                } else {
                    black_box_delta_ratio_equation(from_ident.as_str(), to_ident.as_str())
                }
            } else {
                black_box_delta_ratio_equation(from_ident.as_str(), to_ident.as_str())
            }
        } else if from_is_module && !to_is_module {
            // The dependent's equation references the module's output via
            // "module·output_var" syntax. Find that reference and use the
            // middot-qualified name as the "from" for delta-ratio, since
            // the module node itself is not a readable scalar variable.
            let module_output_ref: Option<String> = to_var
                .ast()
                .map(|ast| crate::variable::identifier_set(ast, &[], None))
                .and_then(|deps| {
                    let prefix = format!("{}\u{00B7}", from_ident.as_str());
                    deps.into_iter()
                        .find(|d| d.as_str().starts_with(&prefix))
                        .map(|d| d.to_string())
                });
            if let Some(output_ref) = module_output_ref {
                black_box_delta_ratio_equation(&output_ref, to_ident.as_str())
            } else {
                black_box_delta_ratio_equation(from_ident.as_str(), to_ident.as_str())
            }
        } else {
            // module -> module: black-box delta-ratio
            black_box_delta_ratio_equation(from_ident.as_str(), to_ident.as_str())
        };

        return Some(LtmSyntheticVar {
            name: var_name,
            equation: datamodel::Equation::Scalar(equation),
            dimensions: vec![],
            compile_directly: false,
        });
    }

    // Standard ceteris-paribus formula for non-module links.
    //
    // `link_score_equation_text` keys by `(from, to)` only -- no per-shape
    // info. The Bare shape, empty `source_dim_elements`, and `None`
    // iterated-dim context reproduce the original pre-Phase-3 behavior (the
    // GH #511 context is `None`-safe here: this legacy path is only reached
    // for scalar-target link scores). Per-shape callers use the `_shaped` fn.
    let mut all_vars = HashMap::new();
    if let Some(ref fv) = from_var {
        all_vars.insert(from_ident.clone(), fv.clone());
    }
    all_vars.insert(to_ident.clone(), to_var.clone());
    let equation = crate::ltm_augment::generate_link_score_equation_for_link(
        &from_ident,
        &to_ident,
        &RefShape::Bare,
        &[],
        &to_var,
        &all_vars,
        None,
    );

    // This legacy entry always emits a scalar link score. If the generator
    // produced an arrayed variant for an arrayed target, collapse it to a
    // scalar equation referencing the array vars directly -- the pre-Phase-3
    // behavior this function reproduces.
    let equation = ltm::scalarize_ltm_equation(equation);

    Some(LtmSyntheticVar {
        name: var_name,
        equation,
        dimensions: vec![],
        compile_directly: false,
    })
}

// `link_score_equation_text_shaped` lives in `db/ltm.rs` (where the
// emission loop calls it) so this file stays under the project's
// per-file line cap; see `ltm::link_score_equation_text_shaped`.

/// Build a causal graph from pre-computed edges and enumerate all pathways
/// from each input port to the specified output ports (or auto-detect them).
/// Used by `model_ltm_variables` in `db/ltm.rs` for pathway and composite
/// score generation.
fn module_input_pathways_from_edges(
    edges_result: &CausalEdgesResult,
    output_ports: &[crate::common::Ident<crate::common::Canonical>],
) -> HashMap<crate::common::Ident<crate::common::Canonical>, Vec<Vec<crate::ltm::Link>>> {
    let graph = causal_graph_from_edges(edges_result);
    graph.enumerate_pathways_to_outputs(output_ports)
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
pub fn collect_all_diagnostics(db: &SimlinDb, project: SourceProject) -> Vec<Diagnostic> {
    let mut all = Vec::new();
    for source_model in project.models(db).values() {
        let diags = collect_model_diagnostics(db, *source_model, project);
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

/// Build the ordered, pre-dedup macro-declaration list for
/// `SourceProject::macro_declarations`: one entry per *project*-declared
/// model (stdlib models are added later and excluded here), in datamodel
/// declaration order, carrying the model's canonical name and its
/// `macro_spec.clone()`.
///
/// Declaration order is load-bearing: `MacroRegistry::build` reports the
/// FIRST-detected duplicate macro name / macro-model collision, and the
/// canonical-name-keyed `models` map collapses the very duplicate / colliding
/// names that validation needs -- so the demand-driven `project_macro_registry`
/// query reconstructs the model list from this ordered raw data.
fn macro_declarations_from_datamodel(
    project: &datamodel::Project,
) -> Vec<(String, Option<datamodel::MacroSpec>)> {
    project
        .models
        .iter()
        .map(|m| (canonicalize(&m.name).into_owned(), m.macro_spec.clone()))
        .collect()
}

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

        let model_sim_specs = dm_model.sim_specs.clone();
        let source_model = SourceModel::new(
            db,
            dm_model.name.clone(),
            variable_names,
            source_var_map,
            model_sim_specs,
            dm_model.macro_spec.clone(),
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
            dm_model.sim_specs.clone(),
            // Stdlib models are not macros (the registry only tracks
            // project macros; stdlib lookup goes through `stdlib_descriptor`).
            None,
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
        project.sim_specs.clone(),
        project.dimensions.clone(),
        project.units.clone(),
        model_names,
        source_model_map,
        macro_declarations_from_datamodel(project),
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
        .cloned()
        .unwrap_or_else(|| datamodel::Equation::Scalar(String::new()));

    let units = var.get_units().cloned();

    let gf = match var {
        datamodel::Variable::Flow(f) => f.gf.clone(),
        datamodel::Variable::Aux(a) => a.gf.clone(),
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
        datamodel::Variable::Module(m) => (m.references.clone(), m.model_name.clone()),
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
        .cloned()
        .unwrap_or_else(|| datamodel::Equation::Scalar(String::new()));
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
        datamodel::Variable::Flow(f) => f.gf.clone(),
        datamodel::Variable::Aux(a) => a.gf.clone(),
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
        datamodel::Variable::Module(m) => (m.references.clone(), m.model_name.clone()),
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

    let new_sim_specs = project.sim_specs.clone();
    if *source_project.sim_specs(&*db) != new_sim_specs {
        source_project.set_sim_specs(db).to(new_sim_specs);
    }

    let new_dims: Vec<datamodel::Dimension> = project.dimensions.clone();
    if *source_project.dimensions(&*db) != new_dims {
        source_project.set_dimensions(db).to(new_dims);
    }

    let new_units: Vec<datamodel::Unit> = project.units.clone();
    if *source_project.units(&*db) != new_units {
        source_project.set_units(db).to(new_units);
    }

    // Re-derive the ordered, pre-dedup macro-declaration list from the
    // datamodel models (duplicates / collisions are invisible once models
    // collapse into the name-keyed map below). The demand-driven
    // `project_macro_registry` query reads this to re-derive the build error.
    let new_macro_declarations = macro_declarations_from_datamodel(project);
    if *source_project.macro_declarations(&*db) != new_macro_declarations {
        source_project
            .set_macro_declarations(db)
            .to(new_macro_declarations);
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

            let new_model_sim_specs = dm_model.sim_specs.clone();
            if *source_model.model_sim_specs(&*db) != new_model_sim_specs {
                source_model.set_model_sim_specs(db).to(new_model_sim_specs);
            }

            if *source_model.macro_spec(&*db) != dm_model.macro_spec {
                source_model
                    .set_macro_spec(db)
                    .to(dm_model.macro_spec.clone());
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

            let model_sim_specs = dm_model.sim_specs.clone();
            let source_model = SourceModel::new(
                &*db,
                dm_model.name.clone(),
                variable_names,
                source_var_map,
                model_sim_specs,
                dm_model.macro_spec.clone(),
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
                dm_model.sim_specs.clone(),
                // Stdlib models are not macros.
                None,
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

/// Expands a set of dimension names to include all dimensions reachable
/// via `maps_to` / `mappings` in either direction.
///
/// Forward: if A maps_to B, {A} → {A, B}.
/// Reverse: if A maps_to B, {B} → {B, A}.
///
/// The reverse direction is necessary when a variable declares its own
/// dimension as e.g. DimB but its equation references DimA via a cross-
/// dimension mapping (DimA → DimB). The per-element implicit variables
/// created for the SMTH/DELAY expansion use elements of DimA, so DimA
/// must be present in the DimensionsContext for the substitution to work.
fn expand_maps_to_chains(
    dim_names: &BTreeSet<String>,
    all_dims: &[datamodel::Dimension],
) -> BTreeSet<String> {
    // Dimension display names (`Dimension.name`, the as-written casing) and
    // mapping targets (`maps_to()` / `mappings[].target`, which the MDL/XMILE
    // importers canonicalize to lowercase) are NOT necessarily the same string,
    // so every reachability comparison and lookup here must be on the canonical
    // form. The returned set is keyed by display name (the caller filters the
    // datamodel dims with `expanded.contains(&d.name)`), so we resolve each
    // canonical target back through `canonical_to_display` before inserting.
    let canonical_to_display: HashMap<String, String> = all_dims
        .iter()
        .map(|d| (canonicalize(&d.name).into_owned(), d.name.clone()))
        .collect();
    let dim_map: HashMap<String, &datamodel::Dimension> = all_dims
        .iter()
        .map(|d| (canonicalize(&d.name).into_owned(), d))
        .collect();

    let mut expanded = dim_names.clone();
    let mut to_visit: Vec<String> = dim_names.iter().cloned().collect();
    while let Some(name) = to_visit.pop() {
        let name_canon = canonicalize(&name).into_owned();

        // `push_target` resolves a canonical mapping target to the display name
        // the caller's `expanded.contains(&d.name)` filter expects, falling back
        // to the canonical string when the target is not itself a declared
        // dimension (a defensive case the old `==` path also tolerated).
        let push_target =
            |expanded: &mut BTreeSet<String>, to_visit: &mut Vec<String>, target_canon: &str| {
                let display = canonical_to_display
                    .get(target_canon)
                    .cloned()
                    .unwrap_or_else(|| target_canon.to_string());
                if expanded.insert(display.clone()) {
                    to_visit.push(display);
                }
            };

        // Forward: follow maps_to and mappings targets from the current dim.
        if let Some(dim) = dim_map.get(&name_canon) {
            if let Some(target) = dim.maps_to() {
                push_target(&mut expanded, &mut to_visit, &canonicalize(target));
            }
            for mapping in &dim.mappings {
                push_target(&mut expanded, &mut to_visit, &canonicalize(&mapping.target));
            }
        }
        // Reverse: find any dimension that maps_to (or has a mapping targeting)
        // our current dim. This ensures that when a variable is subscripted by
        // DimB, the DimensionsContext also contains any DimA that maps to DimB,
        // so cross-dimension subscript substitution works in builtins_visitor.
        for source_dim in all_dims {
            let maps_to_current = source_dim
                .maps_to()
                .is_some_and(|t| canonicalize(t) == name_canon)
                || source_dim
                    .mappings
                    .iter()
                    .any(|m| canonicalize(&m.target) == name_canon);
            if maps_to_current && expanded.insert(source_dim.name.clone()) {
                to_visit.push(source_dim.name.clone());
            }
        }
    }
    expanded
}

/// Extracts the set of dimension names referenced in a variable's equation.
///
/// Reads only `var.equation(db)` -- never the project-level dimension list.
/// This means scalar variables produce an empty set without establishing a
/// dependency on `project.dimensions`, so dimension changes cannot
/// invalidate them. For arrayed variables the returned names come from the
/// equation definition, not from the project dimension list.
#[salsa::tracked(returns(ref))]
pub fn variable_relevant_dimensions(db: &dyn Db, var: SourceVariable) -> BTreeSet<String> {
    match var.equation(db) {
        datamodel::Equation::Scalar(_) => BTreeSet::new(),
        datamodel::Equation::ApplyToAll(dim_names, _) => dim_names.iter().cloned().collect(),
        datamodel::Equation::Arrayed(dim_names, _, _, _) => dim_names.iter().cloned().collect(),
    }
}

#[salsa::tracked(returns(ref))]
pub fn variable_dimensions(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> Vec<crate::dimensions::Dimension> {
    // Module context doesn't affect dimension extraction, so an empty
    // context is correct here.
    let empty_context = ModuleIdentContext::new(db, vec![]);
    let parsed = parse_source_variable_with_module_context(db, var, project, empty_context);
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
    // Scalar LTM vars occupy 1 slot; A2A LTM vars occupy
    // product(dim_lengths) slots, computed from the variable's
    // `dimensions` field. When ltm_enabled is false, this section is
    // skipped entirely (zero overhead). Models without feedback loops
    // (e.g. passthrough modules) get an empty LTM var list from
    // model_ltm_variables (also zero overhead).
    //
    // No salsa dependency cycle: model_ltm_variables calls only analysis
    // functions (model_causal_edges, model_loop_circuits) that don't
    // depend on compute_layout.
    if project.ltm_enabled(db) {
        let ltm_vars = model_ltm_variables(db, model, project);
        let dim_context = project_dimensions_context(db, project);

        let mut sorted_ltm_vars: Vec<&LtmSyntheticVar> = ltm_vars.vars.iter().collect();
        sorted_ltm_vars.sort_unstable_by_key(|v| &v.name);

        for ltm_var in sorted_ltm_vars {
            let size = if ltm_var.dimensions.is_empty() {
                1
            } else {
                ltm_var
                    .dimensions
                    .iter()
                    .map(|dim_name| {
                        let canonical = crate::common::CanonicalDimensionName::from_raw(dim_name);
                        dim_context.get(&canonical).map(|d| d.len()).unwrap_or(1)
                    })
                    .product()
            };
            entries.insert(ltm_var.name.clone(), LayoutEntry { offset, size });
            offset += size;
        }

        // Section 3b: Implicit variables generated by LTM equation
        // parsing. Helper auxes and any expanded module vars need their
        // own slots in the parent model's layout.
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
pub(crate) fn extract_tables_from_source_var(
    db: &dyn Db,
    source_var: &SourceVariable,
    project: SourceProject,
) -> Vec<crate::compiler::Table> {
    let ident = source_var.ident(db);
    let eq = source_var.equation(db);

    // For arrayed equations with per-element graphical functions, build one
    // table per element (matching variable.rs build_tables). Each element's
    // table is laid out at the element's flat declared dimension index (not
    // its `elems` Vec position), because the runtime selects a per-element
    // table by the row-major dimension offset (vm.rs Lookup/LookupArray); see
    // `crate::variable::reorder_arrayed_element_tables`. Elements without a GF
    // get an empty placeholder so that table[element_offset] stays aligned.
    if let datamodel::Equation::Arrayed(_, elements, _, _) = eq {
        // The per-element gf is the 4th tuple field
        // `(subscript, equation, gf_equation, gf)`.
        let has_element_gfs = elements.iter().any(|(_, _, _, gf)| gf.is_some());
        if has_element_gfs {
            // Parse present element tables, keyed by canonical (comma-joined)
            // subscript name.
            let mut present: HashMap<crate::common::CanonicalElementName, crate::compiler::Table> =
                HashMap::new();
            for (subscript, _, _, gf) in elements {
                if let Some(gf) = gf.as_ref()
                    && let Some(var_table) = crate::variable::parse_table(&Some(gf.clone()))
                        .ok()
                        .flatten()
                    && let Ok(table) = crate::compiler::Table::new(ident, &var_table)
                {
                    present.insert(
                        crate::common::CanonicalElementName::from_raw(subscript),
                        table,
                    );
                }
            }

            // Resolve the variable's dimensions so the reorder maps each
            // element name to its row-major declared-order flat offset. If the
            // dimensions cannot be resolved, fall back to the original
            // Vec-positional layout rather than dropping tables.
            let dims = variable_dimensions(db, *source_var, project);
            if dims.is_empty() {
                return elements
                    .iter()
                    .map(|(subscript, _, _, _)| {
                        present
                            .get(&crate::common::CanonicalElementName::from_raw(subscript))
                            .cloned()
                            .unwrap_or(crate::compiler::Table { data: vec![] })
                    })
                    .collect();
            }
            return crate::variable::reorder_arrayed_element_tables(
                dims,
                &present,
                || crate::compiler::Table { data: vec![] },
                |t: &crate::compiler::Table| t.clone(),
            );
        }
    }

    // Scalar or apply-to-all: use the variable-level graphical function.
    let gf = source_var.gf(db);
    match gf {
        Some(gf) => crate::variable::parse_table(&Some(gf.clone()))
            .ok()
            .flatten()
            .and_then(|vt| crate::compiler::Table::new(ident, &vt).ok())
            .into_iter()
            .collect(),
        None => vec![],
    }
}

/// Build module input mappings from raw (src, dst) reference pairs.
///
/// Filters out references where src is an internal module input (starts
/// with the module's own prefix), strips the module prefix from dst,
/// and strips leading middots from src in the "main" model (where parent
/// scope refs are represented as `·var` after canonicalization).
pub(crate) fn build_module_inputs<S1: AsRef<str>, S2: AsRef<str>>(
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
pub(crate) fn build_stub_variable(
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
pub(crate) fn build_submodel_metadata<'arena>(
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

/// `model_name -> (var_name -> (offset, size))`: the per-variable mini-
/// layout offset map `lower_var_fragment` produces and the minimal
/// per-phase `crate::compiler::Module` consumes. Structurally identical to
/// `compiler::VariableOffsetMap` / `var_fragment::VarOffsets` (both
/// private aliases in their modules); named here so the factored
/// `compile_phase_to_per_var_bytecodes` signature is self-documenting
/// rather than an inline nested-`HashMap` (which clippy flags as a very
/// complex type).
pub(crate) type PerVarOffsetMap =
    HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, (usize, usize)>>;

/// Compile one phase's lowered `Vec<Expr>` for a single variable through
/// its own correct mini-context and symbolize the result into a
/// layout-independent `PerVarBytecodes`.
///
/// This is the exact body of `compile_var_fragment`'s former
/// `compile_phase` closure, factored out so the element-cycle SCC graph
/// builder (`crate::db::dep_graph` via `var_phase_symbolic_fragment_prod`)
/// reuses the *exact* production compile+symbolize path rather than a
/// re-derivation. `compile_var_fragment` calls this for each phase; the
/// SCC accessor `var_phase_symbolic_fragment_prod` builds the caller-owned
/// context byte-identically to `compile_var_fragment` and calls this with
/// the phase's production-lowered exprs.
///
/// The caller owns and supplies the lowering-independent context
/// (`offsets`, `rmap`, `tables`, `module_refs`, `mini_offset`,
/// `converted_dims`, `dim_context`, `model_name_ident`, `inputs`) exactly
/// as `compile_var_fragment` constructs it. `var_ident_canonical` is the
/// single-variable runlist-order entry the minimal `Module` is built
/// around. Returns `None` (loud-safe, never panics) when `exprs` is
/// empty, the minimal `Module::compile()` fails, or any symbolization
/// step fails -- exactly the closure's original `None` arms.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_phase_to_per_var_bytecodes(
    exprs: &[crate::compiler::Expr],
    offsets: &PerVarOffsetMap,
    rmap: &crate::compiler::symbolic::ReverseOffsetMap,
    tables: &HashMap<Ident<Canonical>, Vec<crate::compiler::Table>>,
    module_refs: &HashMap<Ident<Canonical>, crate::vm::ModuleKey>,
    mini_offset: usize,
    converted_dims: &[crate::dimensions::Dimension],
    dim_context: &crate::dimensions::DimensionsContext,
    model_name_ident: &Ident<Canonical>,
    var_ident_canonical: &Ident<Canonical>,
    inputs: &BTreeSet<Ident<Canonical>>,
) -> Option<crate::compiler::symbolic::PerVarBytecodes> {
    use crate::compiler::symbolic::PerVarBytecodes;

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
        offsets: offsets.clone(),
        runlist_order: vec![var_ident_canonical.clone()],
        tables: tables.clone(),
        dimensions: converted_dims.to_vec(),
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
                crate::compiler::symbolic::symbolize_bytecode(&compiled.compiled_flows, rmap)
                    .ok()?;

            let ctx = &*compiled.context;
            let sym_views: Vec<_> = ctx
                .static_views
                .iter()
                .map(|sv| crate::compiler::symbolic::symbolize_static_view(sv, rmap))
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            let sym_mods: Vec<_> = ctx
                .modules
                .iter()
                .map(|md| crate::compiler::symbolic::symbolize_module_decl(md, rmap))
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
}

/// A variable's *symbolic* `PerVarBytecodes` for a phase, sourced through
/// the exact production compile+symbolize path (`lower_var_fragment` +
/// `compile_phase_to_per_var_bytecodes`), never a re-derivation.
///
/// This is the cross-member-comparable substrate the element-cycle SCC
/// graph builder consumes: every variable reference in the returned
/// bytecode is a layout-independent
/// `SymVarRef { name, element_offset }`, so a multi-member recurrence
/// SCC's induced element graph can be built across members (the fix for
/// GH #575 -- the prior `Expr::AssignCurr`-mini-slot builder was
/// structurally incapable of cross-member edges). It is the production
/// element-graph source consumed by `symbolic_phase_element_order` and
/// `combine_scc_fragment` (the Phase 2 GH #575 rebuild replaced the prior
/// `Expr`-based accessor entirely).
///
/// This accessor returns the *whole* per-phase symbolic stream verbatim
/// (PREVIOUS/INIT reads included). Which opcodes become element-graph
/// *edges* is the consumer's concern: `symbolic_phase_element_order`'s
/// read-opcode arm inherits `build_var_info`'s exact per-phase
/// PREVIOUS/INIT strip (`SymLoadPrev` -> no edge in either phase;
/// `SymLoadInitial` -> no edge in `Dt`, edge in `Initial`; current-value
/// reads kept), so the element graph MATCHES the engine's actual
/// per-phase data-flow relation rather than over-collecting lagged reads.
/// See that function's rustdoc for the AC4 soundness argument and the
/// exact `db/dep_graph.rs` `build_var_info` line citations. The loud-safe
/// contract documented *here* is a distinct concern -- it is about a
/// node failing to be element-*sourced* (always `None`, never a panic),
/// not about which sourced opcodes are ordering edges.
///
/// The caller-owned, lowering-independent context is built byte-identically
/// to `compile_var_fragment` (same helpers, same order, the default
/// no-module-input wiring `build_var_info(.., &[])` uses):
/// `SccPhase::Dt` selects `per_phase_lowered.noninitial`,
/// `SccPhase::Initial` selects `.initial`.
///
/// A synthetic helper (`$\u{205A}` prefix, absent from `model.variables`)
/// that lands in a recurrence SCC is **parent-sourced**: its symbolic
/// `PerVarBytecodes` is the parent variable's `implicit_vars[index]`
/// compiled+symbolized through the shared per-phase relation
/// `compile_implicit_var_phase_bytecodes` (the same chain
/// `compile_implicit_var_fragment` runs), so the element-graph builder
/// consumes it exactly like a real member (element-cycle Phase 3 Task 2 /
/// AC3.1, pinned by `synthetic_helper_symbolic_fragment_is_parent_sourced`).
///
/// **Loud-safe contract (the load-bearing invariant -- formalized here).**
/// This accessor returns `None` -- *never* panics, `expect`s, or `unwrap`s
/// on a sourcing failure -- on EVERY way a node fails to be
/// element-sourced:
/// - no `SourceVariable` AND not a parent-sourceable synthetic helper
///   (absent from `model_implicit_var_info`, or the shared per-phase
///   compile failed): `None` (the loud-safe signal -- AC3.2);
/// - `LoweredVarFragment::Fatal` (the variable did not lower at all):
///   explicit `return None`;
/// - the requested phase's `Var::new` errored (`phase_var.ok()?`);
/// - any `compile_phase_to_per_var_bytecodes` failure (empty exprs, the
///   minimal `Module::compile()`, or any `symbolize_*` step) -- that
///   function is itself total-and-`None`-on-failure.
///
/// `None` propagates loud-safe and all-or-nothing: any in-SCC node that
/// cannot be element-sourced makes `symbolic_phase_element_order` return
/// `None` (its `?` on this call), so `refine_scc_to_element_verdict`
/// yields `SccVerdict::Unresolved`, `resolve_recurrence_sccs` sets
/// `has_unresolved`, and `model_dependency_graph_impl` keeps `has_cycle`
/// and accumulates the `CircularDependency` diagnostic
/// (`dt_scc_map`/`init_scc_map` stays empty, `resolved_sccs` stays empty).
/// The model is rejected loudly -- no panic, no silent miscompile, and the
/// other SCC members are **not** partially resolved (the SCC is rejected
/// as a unit). This contract is regression-pinned by
/// `unsourceable_in_scc_node_falls_back_to_circular_no_panic` (AC3.2,
/// driven through the production `model_dependency_graph` path via the
/// `#[cfg(test)]` `UnsourceableVarsGuard` override) and
/// `var_phase_symbolic_fragment_prod_none_for_absent_var_no_panic`.
pub(crate) fn var_phase_symbolic_fragment_prod(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_name: &str,
    phase: SccPhase,
) -> Option<crate::compiler::symbolic::PerVarBytecodes> {
    use crate::db::var_fragment::{LoweredVarFragment, lower_var_fragment};

    // `#[cfg(test)]` only: an active `UnsourceableVarsGuard` forces this
    // node to take the loud-safe `None` arm, so the AC3.2 regression test
    // can exercise the genuinely-unsourceable in-SCC path through the
    // PRODUCTION `model_dependency_graph` chain (an organic orphan that is
    // neither in `source_vars` nor resolvable via `model_implicit_var_info`
    // is hard to construct deterministically; this is the reliable
    // trigger). It returns the SAME `None` a real no-`SourceVariable`
    // node returns, so the test observes the real loud-safe behavior, not
    // a shim. No effect in non-test builds.
    #[cfg(test)]
    if crate::db::dep_graph::var_is_forced_unsourceable(var_name) {
        return None;
    }

    let source_vars = model.variables(db);
    // No `SourceVariable` (a synthetic INIT/PREVIOUS/SMOOTH/macro-expansion
    // helper, `$\u{205A}` prefix, absent from `model.variables`): before
    // the loud-safe `None`, attempt parent-`implicit_vars` sourcing
    // (element-cycle Phase 3 Task 2 / AC3.1). A synthetic helper that
    // lands in a recurrence SCC has no `SourceVariable` but DOES resolve
    // in `model_implicit_var_info`; its symbolic `PerVarBytecodes` is the
    // parent variable's `implicit_vars[index]` compiled+symbolized through
    // the SAME shared per-phase relation the production per-variable
    // assembly uses (`compile_implicit_var_phase_bytecodes` -- the exact
    // `parent → parsed.implicit_vars[i] → parse_var → lower_variable →
    // compile → symbolize` chain `compile_implicit_var_fragment` runs), so
    // the element-graph builder consumes it exactly like a real member
    // (same layout-independent `SymVarRef` form). The element-cycle SCC
    // identification uses the default no-module-input root wiring, so
    // source the helper with `is_root = true`, `module_input_names = &[]`
    // (matching the real-var arm's `lower_var_fragment(.., true, &[], ..)`
    // below). Genuinely unsourceable (absent from `model_implicit_var_info`
    // too, or the shared compile failed) ⇒ `None`, the loud-safe signal
    // (see the rustdoc's loud-safe contract): the SCC stays unresolved and
    // `CircularDependency` is kept -- no panic, no silent miscompile
    // (AC3.2).
    let Some(sv) = source_vars.get(var_name) else {
        let canonical_name = canonicalize(var_name).into_owned();
        let info = model_implicit_var_info(db, model, project);
        let meta = info.get(&canonical_name)?;
        let is_initial = matches!(phase, SccPhase::Initial);
        return compile_implicit_var_phase_bytecodes(
            db,
            meta,
            model,
            project,
            true,
            &[],
            is_initial,
        );
    };
    let var_ident_canonical: Ident<Canonical> = Ident::new(var_name);

    // Caller-owned, lowering-independent context, read EXACTLY as
    // `compile_var_fragment` reads it (mirror byte-for-byte): the
    // salsa-cached project-global dimension context and converted dims.
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);
    let model_name_ident = Ident::new(model.name(db));
    let inputs: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let module_models = model_module_map(db, model, project).clone();

    let lowered = lower_var_fragment(
        db,
        *sv,
        model,
        project,
        true,
        &[],
        converted_dims,
        dim_context,
        &model_name_ident,
        &module_models,
        &inputs,
    );

    let (per_phase_lowered, tables, offsets, rmap, mini_offset) = match lowered {
        LoweredVarFragment::Lowered {
            per_phase_lowered,
            tables,
            offsets,
            rmap,
            mini_offset,
            ..
        } => (per_phase_lowered, tables, offsets, rmap, mini_offset),
        // The variable did not lower at all => `None` (loud-safe).
        LoweredVarFragment::Fatal { .. } => return None,
    };

    // The element-cycle SCC identification uses the default no-module-
    // input wiring, so the module-ref reconstruction must match that
    // wiring too (mirrors `compile_var_fragment`'s
    // `build_caller_module_refs(.., &module_input_names)` with empty
    // inputs).
    let module_refs =
        crate::db::var_fragment::build_caller_module_refs(db, *sv, model, project, true, &[]);

    // `SccPhase::Dt` selects the non-initial (dt/flow) lowering;
    // `SccPhase::Initial` selects the initial lowering -- the same
    // selection `compile_var_fragment` makes per phase.
    let phase_var = match phase {
        SccPhase::Dt => per_phase_lowered.noninitial,
        SccPhase::Initial => per_phase_lowered.initial,
    };
    // The phase's `Var::new` errored => cannot source its production
    // lowered exprs => `None` (loud-safe).
    let var = phase_var.ok()?;

    compile_phase_to_per_var_bytecodes(
        &var.ast,
        &offsets,
        &rmap,
        &tables,
        &module_refs,
        mini_offset,
        converted_dims,
        dim_context,
        &model_name_ident,
        &var_ident_canonical,
        &inputs,
    )
}

/// Segment one member's symbolic opcode stream into per-element slices,
/// keyed by `element_offset`.
///
/// A per-element slice for element `e` is the run of opcodes up to and
/// including the **write** opcode whose `var.name == member` and
/// `var.element_offset == e` (`AssignCurr | AssignConstCurr |
/// BinOpAssignCurr`). This is the *exact* segmentation
/// `crate::db::dep_graph::symbolic_phase_element_order` performs to build
/// the SCC element graph (GH #575) -- the verdict and the combined
/// fragment MUST agree on segment boundaries or `element_order` would
/// reference a slice the combiner cannot reproduce, so the two share this
/// definition's contract.
///
/// A trailing `Ret` is stripped first (the combined fragment carries one
/// terminal `Ret`). Any opcodes after the member's final per-element write
/// (before the stripped `Ret`) are appended to the last element's slice so
/// no opcode is silently dropped -- a tail with no write is a malformed
/// fragment (`Err`).
///
/// Loud-safe failures (return `Err`, caller keeps `CircularDependency` --
/// NEVER a panic, NEVER a silently-malformed slice):
/// - a duplicate write for the same element (ambiguous segmentation);
/// - opcodes present but no per-element write at all (not element-
///   sourceable in the simple per-element shape, mirroring
///   `symbolic_phase_element_order`'s `saw_write` guard).
///
/// Consumed by `combine_scc_fragment`, which `assemble_module` invokes
/// for every resolved recurrence SCC (the Subcomponent B Task 6
/// production consumer -- the dt flows runlist and the synthetic-ident
/// init `SymbolicCompiledInitial` path).
fn segment_member_by_element(
    member: &str,
    code: &[crate::compiler::symbolic::SymbolicOpcode],
) -> Result<HashMap<usize, Vec<crate::compiler::symbolic::SymbolicOpcode>>, String> {
    use crate::compiler::symbolic::SymbolicOpcode;

    // Strip a trailing Ret -- the combined fragment appends a single Ret.
    let end = if code.last() == Some(&SymbolicOpcode::Ret) {
        code.len() - 1
    } else {
        code.len()
    };
    let body = &code[..end];

    let mut segments: HashMap<usize, Vec<SymbolicOpcode>> = HashMap::new();
    let mut current: Vec<SymbolicOpcode> = Vec::new();
    let mut last_written_elem: Option<usize> = None;

    for op in body {
        current.push(op.clone());
        let write_elem = match op {
            SymbolicOpcode::AssignCurr { var }
            | SymbolicOpcode::AssignConstCurr { var, .. }
            | SymbolicOpcode::BinOpAssignCurr { var, .. }
                if var.name == member =>
            {
                Some(var.element_offset)
            }
            // A write to a *different* member, or AssignNext/
            // BinOpAssignNext (a stock-update, not a per-element
            // current-value write of THIS member) does not terminate this
            // member's element segment -- exactly the
            // `symbolic_phase_element_order` rule.
            _ => None,
        };
        if let Some(elem) = write_elem {
            if segments.contains_key(&elem) {
                return Err(format!(
                    "SCC member `{member}` has a duplicate per-element \
                     write for element {elem}; combined fragment cannot \
                     be unambiguously segmented"
                ));
            }
            segments.insert(elem, std::mem::take(&mut current));
            last_written_elem = Some(elem);
        }
    }

    // Any trailing opcodes after the last write belong to the last
    // element's segment (dropping them would change semantics). With no
    // write at all this member is not element-sourceable -- loud-safe.
    if !current.is_empty() {
        match last_written_elem {
            Some(elem) => {
                segments
                    .get_mut(&elem)
                    .expect("last_written_elem indexes an inserted segment")
                    .extend(current);
            }
            None => {
                return Err(format!(
                    "SCC member `{member}` has no per-element write \
                     opcode; not element-sourceable for the combined \
                     fragment"
                ));
            }
        }
    }

    Ok(segments)
}

/// Interleave a multi-member recurrence SCC's per-element symbolic
/// segments into ONE combined `PerVarBytecodes`, following the SCC's
/// element-acyclic `element_order`.
///
/// `member_fragments` maps each SCC member's canonical name to its
/// *symbolic* `PerVarBytecodes` for the SCC's phase (obtained by the
/// caller via `var_phase_symbolic_fragment_prod(.., scc.phase)` -- the
/// exact production compile+symbolize path, never a re-derivation). The
/// result is a single fragment whose per-element writes appear in
/// `scc.element_order`, with each write keeping its **original**
/// `SymVarRef { name, element_offset }` (only segment ordering changes).
/// `resolve_module` therefore maps every write to the same model slot it
/// would have without the SCC, so variable layout offsets and the results
/// offset map are unchanged and per-variable result series stay
/// individually addressable (AC2.3).
///
/// **This is the per-element-granular generalization of
/// `concatenate_fragments`.** Resources are MEMBER-scoped, not
/// element-scoped: each member's fragment is absorbed into the shared
/// `FragmentMerger` exactly ONCE (in `element_order`'s member
/// first-encounter order, so the offset assignment is deterministic),
/// yielding that member's resource base offsets and merging its
/// side-channels (literals, GFs, modules, views, temps, dim-lists) the
/// same way `concatenate_fragments` merges a fragment. Every segment of
/// that member is then renumbered by the member's offsets. The two
/// consumers share `FragmentMerger`/`renumber_opcode` so the multi-layer
/// resource accounting cannot drift.
///
/// Loud-safe (`Err`, caller keeps `CircularDependency` -- never a panic,
/// never a malformed fragment):
/// - a member named in `element_order` has no supplied fragment (the Task
///   4 accessor returned `None` -- unsourceable);
/// - a member's fragment cannot be cleanly segmented (missing / duplicate
///   / no-write element segment -- `segment_member_by_element`);
/// - an `(member, element)` entry in `element_order` has no matching
///   segment;
/// - a resource-ID renumber overflows its target ID type.
///
/// `assemble_module` (Subcomponent B Task 6) invokes this for every
/// resolved recurrence SCC: it skips each member's per-variable fragment
/// in the dt-flows and init collection loops and injects this combined
/// fragment at the first member's runlist slot (the dt fragment into
/// `flow_frags`, the init fragment as one synthetic-ident
/// `SymbolicCompiledInitial`).
pub(crate) fn combine_scc_fragment(
    scc: &ResolvedScc,
    member_fragments: &HashMap<Ident<Canonical>, crate::compiler::symbolic::PerVarBytecodes>,
) -> Result<crate::compiler::symbolic::PerVarBytecodes, String> {
    use crate::compiler::symbolic::{
        ContextResourceCounts, FragmentMerger, FragmentResourceOffsets, SymbolicOpcode,
        renumber_opcode,
    };

    // Absorb each member ONCE, in `element_order`'s member first-encounter
    // order, so per-member resource offsets are assigned deterministically
    // (the interleave is a pure reordering => byte-stable output, AC2.3).
    // The combined fragment is itself a fragment re-fed to
    // `concatenate_fragments` at assembly, so it is built in an isolated
    // resource namespace (`ctx_base = default`), exactly as a per-variable
    // fragment is.
    let mut merger = FragmentMerger::new(&ContextResourceCounts::default());
    let mut absorbed: HashMap<Ident<Canonical>, FragmentResourceOffsets> = HashMap::new();
    // Per-member, per-element renumbered segments. Keyed by the same
    // `(member, element)` identity `element_order` carries.
    let mut renumbered_segments: HashMap<(Ident<Canonical>, usize), Vec<SymbolicOpcode>> =
        HashMap::new();

    for (member, _elem) in &scc.element_order {
        if absorbed.contains_key(member) {
            continue;
        }
        let frag = member_fragments.get(member).ok_or_else(|| {
            format!(
                "SCC member `{}` has no supplied symbolic fragment \
                 (unsourceable); keeping CircularDependency",
                member.as_str()
            )
        })?;
        // `absorb` merges this member's side-channels (de-duplicating its
        // GF blocks against the running merge -- #582) and returns its flat
        // resource base offsets plus the per-slot GF remap -- the exact
        // per-fragment prologue `concatenate_fragments` runs.
        let (off, gf_remap) = merger.absorb(frag)?;
        absorbed.insert(member.clone(), off);

        // Segment the member's symbolic code on its per-element write
        // opcodes (identical contract to the Task 4 verdict builder), then
        // renumber every opcode of every segment by THIS member's offsets
        // and GF remap.
        let segments = segment_member_by_element(member.as_str(), &frag.symbolic.code)?;
        for (elem, ops) in segments {
            let mut renumbered = Vec::with_capacity(ops.len());
            for op in &ops {
                renumbered.push(renumber_opcode(
                    op,
                    off.lit_offset,
                    &gf_remap,
                    off.mod_offset,
                    off.view_offset,
                    off.temp_offset,
                    off.dl_offset,
                )?);
            }
            renumbered_segments.insert((member.clone(), elem), renumbered);
        }
    }

    // Emit the renumbered segments in `element_order`. Every entry must
    // map to exactly one segment (a missing one is loud-safe). Each
    // segment is consumed exactly once: a duplicate `(member, element)` in
    // `element_order` (which the Task 4 builder cannot produce -- nodes
    // are unique) would try to reuse a removed segment and fail loud-safe.
    let mut combined_code: Vec<SymbolicOpcode> = Vec::new();
    for (member, elem) in &scc.element_order {
        let seg = renumbered_segments
            .remove(&(member.clone(), *elem))
            .ok_or_else(|| {
                format!(
                    "SCC element_order references `{}`[{}] but no such \
                     per-element segment exists in its fragment; keeping \
                     CircularDependency",
                    member.as_str(),
                    elem
                )
            })?;
        combined_code.extend(seg);
    }

    Ok(merger.into_per_var_bytecodes(combined_code))
}

#[salsa::tracked(returns(ref))]
pub fn compile_var_fragment<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_inputs: ModuleInputSet<'db>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{CompiledVarFragment, PerVarBytecodes};
    use crate::db::var_fragment::{LoweredVarFragment, lower_var_fragment};

    let var_ident = var.ident(db).clone();
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_ident);

    // The interned input set stores the sorted canonical names; the plain
    // lowering helpers (`lower_var_fragment`/`build_caller_module_refs`) still
    // take `&[String]`, so read it back as a slice.
    let module_input_names = module_inputs.names(db);

    // Caller-owned, lowering-independent context (built only from
    // project/variable data, never from the lowered equation). Read the
    // salsa-cached project-global dimension context and converted dims
    // (returns(ref)) rather than rebuilding them on every variable -- this
    // fragment compiler is invoked once per variable, and the context is
    // project-global and immutable. Building it canonicalizes every dimension
    // element name, so caching it removes a dominant per-variable allocation.
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);
    let model_name_ident = Ident::new(model.name(db));
    let inputs = canonical_module_input_set(module_input_names);
    let module_models = model_module_map(db, model, project).clone();

    let lowered = lower_var_fragment(
        db,
        var,
        model,
        project,
        is_root,
        module_input_names,
        converted_dims,
        dim_context,
        &model_name_ident,
        &module_models,
        &inputs,
    );

    let (unit_diags, per_phase_lowered, tables, offsets, rmap, mini_offset) = match lowered {
        LoweredVarFragment::Fatal {
            unit_diags,
            fatal_diags,
        } => {
            // Non-fatal unit diagnostics were recorded before the fatal
            // site; replay them first to preserve emission order, then
            // the fatal diagnostic(s), then bail out (whole-variable None).
            for diag in unit_diags {
                CompilationDiagnostic(diag).accumulate(db);
            }
            for diag in fatal_diags {
                CompilationDiagnostic(diag).accumulate(db);
            }
            return None;
        }
        LoweredVarFragment::Lowered {
            unit_diags,
            per_phase_lowered,
            tables,
            offsets,
            rmap,
            mini_offset,
        } => (
            unit_diags,
            per_phase_lowered,
            tables,
            offsets,
            rmap,
            mini_offset,
        ),
    };

    // Malformed-unit diagnostics are non-fatal: record them and continue.
    for diag in unit_diags {
        CompilationDiagnostic(diag).accumulate(db);
    }

    // Determine which runlists this variable belongs to
    let dep_graph = model_dependency_graph(db, model, project, module_inputs);
    let is_stock = var.kind(db) == SourceVariableKind::Stock;
    let is_module = var.kind(db) == SourceVariableKind::Module;
    let is_module_input = inputs.contains(&var_ident_canonical);

    let module_refs = crate::db::var_fragment::build_caller_module_refs(
        db,
        var,
        model,
        project,
        is_root,
        module_input_names,
    );

    // Compile for each phase and symbolize. The closure now delegates to
    // the factored `compile_phase_to_per_var_bytecodes` so the SCC
    // element-graph builder reuses the EXACT production compile+symbolize
    // path (no re-derivation); the per-variable production behavior is
    // byte-identical to the former inline closure (same minimal `Module`,
    // same temp extraction, same symbolization, same `None` arms).
    let compile_phase = |exprs: &[crate::compiler::Expr]| -> Option<PerVarBytecodes> {
        compile_phase_to_per_var_bytecodes(
            exprs,
            &offsets,
            &rmap,
            &tables,
            &module_refs,
            mini_offset,
            converted_dims,
            dim_context,
            &model_name_ident,
            &var_ident_canonical,
            &inputs,
        )
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
        match &per_phase_lowered.initial {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(err) => {
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
            match &per_phase_lowered.noninitial {
                Ok(var_result) => compile_phase(&var_result.ast),
                Err(err) => {
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
            match &per_phase_lowered.noninitial {
                Ok(var_result) => compile_phase(&var_result.ast),
                Err(err) => {
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

/// The genuinely-shared prefix of synthetic-helper sourcing: resolve a
/// model's implicit variable from its parent's `implicit_vars`, parse it,
/// and lower it to a `crate::variable::Variable`.
///
/// This is the *single shared relation* (DRY -- "never re-derive") for
/// "given an `ImplicitVarMeta`, produce the helper's parsed + lowered
/// form". It is the exact `model_implicit_var_info`-fed chain
/// `parent → parsed.implicit_vars[index] → parse_var → lower_variable`
/// (the non-module branch builds via `lower_variable`; the module branch
/// constructs a `Variable::Module` directly because `lower_variable` with
/// an empty models map fails `resolve_module_input`). It is consumed by
/// both `compile_implicit_var_fragment` (the production per-variable
/// fragment compiler) and `var_phase_symbolic_fragment_prod`'s
/// no-`SourceVariable` arm (element-cycle Phase 3 Task 2 / AC3.1:
/// parent-sourcing a synthetic helper that lands in a recurrence SCC), so
/// the accessor's relation is the engine's relation by construction.
///
/// Returns the helper's canonical name and the lowered variable. The
/// parent's `ParsedVariableResult` is intentionally NOT returned: callers
/// that also need it re-call the salsa-`returns(ref)`-cached
/// `parse_source_variable_with_module_context` (a cache hit -- a borrow,
/// zero clone), exactly as the pre-extraction code did. Loud-safe `None`
/// (never panics): the implicit index is absent, the module branch's
/// datamodel variable is not actually a `Module`, or the implicit var has
/// equation errors. (`lower_variable` itself is total -- any lowering
/// error surfaces as a `LoweredVarFragment::Fatal` / `Var::new` error
/// downstream, not here.)
fn lower_implicit_var<'db>(
    db: &'db dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_ident_context: ModuleIdentContext<'db>,
) -> Option<(String, crate::variable::Variable)> {
    let parsed = parse_source_variable_with_module_context(
        db,
        meta.parent_source_var,
        project,
        module_ident_context,
    );
    let implicit_dm_var = parsed.implicit_vars.get(meta.index_in_parent)?;
    let implicit_name = canonicalize(implicit_dm_var.get_ident()).into_owned();

    let dm_dims = project_datamodel_dims(db, project);
    let dim_context = project_dimensions_context(db, project);

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
            dimensions: dim_context,
            model_name: "",
        };
        let lowered = crate::model::lower_variable(&scope, &parsed_implicit);

        // Loud-safe (GH #580): `lower_variable` is total -- on a lowering error
        // (e.g. an un-translatable cross-dimension subscript surviving into a
        // scalar helper as `DimensionInScalarContext`) it records the error and
        // discards the AST rather than failing. The pre-lowering check above
        // only inspects the *parsed* implicit; a lowering-stage error would
        // otherwise leave a helper with `ast == None` that
        // `compile_implicit_var_phase_bytecodes` -> `Var::new` rejects as
        // `EmptyEquation`. Bail out with `None` so the error rides out via the
        // caller's aggregate `missing_vars` string (GH #466 tracks surfacing
        // assembly-stage errors through the per-variable diagnostic API).
        if lowered.equation_errors().is_some() {
            return None;
        }

        lowered
    };

    Some((implicit_name, lowered))
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
    use crate::compiler::symbolic::CompiledVarFragment;

    // The implicit var's canonical name (the runlist-gate key). Resolve it
    // through the shared prefix so this and the per-phase compile agree on
    // the name by construction. `None` here is the same loud-safe signal
    // the per-phase compile returns (absent implicit index / equation
    // errors).
    let module_ident_context =
        model_module_ident_context(db, model, project, module_input_names.to_vec());
    let (implicit_name, _lowered) =
        lower_implicit_var(db, meta, model, project, module_ident_context)?;
    let var_ident_str = canonicalize(&implicit_name).into_owned();

    // Runlist-gated phase selection (unchanged output behavior): the
    // Initial phase is compiled only for implicit vars in
    // `runlist_initials`; the non-initial phase feeds `flow_bytecodes`
    // (non-stock) or `stock_bytecodes` (stock/module), each gated by the
    // corresponding runlist. The per-phase compile builds its own context;
    // it is invoked at most for the gated phases (≤2), so the only cost
    // vs. the prior single-context build is a bounded extra
    // map-construction on the ≤2-phase implicit-var sub-path -- the
    // duplication-free price for a single shared per-phase relation
    // (`compile_implicit_var_phase_bytecodes`, also consumed by
    // `var_phase_symbolic_fragment_prod`'s no-`SourceVariable` arm).
    let phase = |is_initial: bool| {
        compile_implicit_var_phase_bytecodes(
            db,
            meta,
            model,
            project,
            is_root,
            module_input_names,
            is_initial,
        )
    };

    let initial_bytecodes = if dep_graph.runlist_initials.contains(&var_ident_str) {
        phase(true)
    } else {
        None
    };
    let flow_bytecodes = if !meta.is_stock && dep_graph.runlist_flows.contains(&var_ident_str) {
        phase(false)
    } else {
        None
    };
    let stock_bytecodes =
        if (meta.is_stock || meta.is_module) && dep_graph.runlist_stocks.contains(&var_ident_str) {
            phase(false)
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

/// Build the mini-layout context for one implicit variable and compile a
/// single phase (`is_initial`) to symbolic `PerVarBytecodes`. NOT a tracked
/// function -- the parent variable's parse result already provides salsa
/// caching.
///
/// This is the **single shared per-phase relation** for "produce a
/// synthetic helper's symbolic `PerVarBytecodes`": consumed by
/// `compile_implicit_var_fragment` (the production per-variable assembly,
/// runlist-gated) *and* `var_phase_symbolic_fragment_prod`'s
/// no-`SourceVariable` arm (element-cycle Phase 3 Task 2 / AC3.1 --
/// parent-sourcing a synthetic helper that lands in a recurrence SCC), so
/// the element-graph accessor's bytecode is byte-identical to the
/// production fragment by construction (DRY -- "single shared relation,
/// never re-derive"). The shared `parent → implicit → parse → lower`
/// prefix is `lower_implicit_var`; the shared compile+symbolize tail is
/// `compile_phase_to_per_var_bytecodes` (the exact function the real-var
/// arm of `var_phase_symbolic_fragment_prod` and `compile_var_fragment`
/// use). The mini-layout/metadata/dep-collection glue between them is
/// intrinsic to the implicit-var shape (the `meta.is_module` branch, the
/// `is_root` implicit-time prelude, the dep-stub/sub-model collection) and
/// is not separately extractable without restructuring this function.
///
/// Loud-safe `None` (never panics): the shared prefix failed (absent
/// implicit index / equation errors), a graphical-function table failed to
/// build, the phase's `Var::new` errored, or `Module::compile()` /
/// symbolization failed -- exactly the original closure's `None` arms.
#[allow(clippy::too_many_arguments)]
fn compile_implicit_var_phase_bytecodes(
    db: &dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_input_names: &[String],
    is_initial: bool,
) -> Option<crate::compiler::symbolic::PerVarBytecodes> {
    use crate::compiler::symbolic::{ReverseOffsetMap, VariableLayout};

    let module_ident_context =
        model_module_ident_context(db, model, project, module_input_names.to_vec());

    // Shared parent→implicit→parse→lower prefix (the single relation, also
    // consumed by `compile_implicit_var_fragment`). `ModuleIdentContext`
    // is a `Copy` interned handle, so the one context is threaded into
    // both the shared prefix and the parse below (a single
    // `module_ident_context_for_model` build, matching the pre-extraction
    // monolith).
    let (implicit_name, lowered) =
        lower_implicit_var(db, meta, model, project, module_ident_context)?;
    // The parent's parsed result for the module-refs reconstruction /
    // dep-collection below. `parse_source_variable_with_module_context`
    // is salsa-`returns(ref)`-cached, so this is a cache-hit borrow (zero
    // clone) -- `lower_implicit_var` already populated it.
    let parsed = parse_source_variable_with_module_context(
        db,
        meta.parent_source_var,
        project,
        module_ident_context,
    );
    let implicit_dm_var = parsed.implicit_vars.get(meta.index_in_parent)?;

    // Project-global dimension context + converted dims, read from the
    // salsa-cached queries rather than rebuilt per implicit variable.
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

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
    // branches of `if isModuleInput(...)` may still be compiled. The empty
    // `ModuleInputSet` reproduces the old `None`-inputs path.
    let deps = variable_direct_dependencies(
        db,
        meta.parent_source_var,
        project,
        module_ident_context,
        ModuleInputSet::empty(db),
    );
    let implicit_dep = deps
        .implicit_vars
        .iter()
        .find(|iv| canonicalize(&iv.name) == canonicalize(&implicit_name));

    let all_dep_names: BTreeSet<String> = if let Some(iv_deps) = implicit_dep {
        iv_deps
            .dt_deps
            .iter()
            .chain(iv_deps.initial_deps.iter())
            // Lookup tables referenced by this implicit var are layout
            // references, not data-flow deps -- include them so the fragment's
            // metadata + tables map can resolve `LOOKUP(table, x)` (#606).
            .chain(iv_deps.referenced_tables.iter())
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
            let dep_tables = extract_tables_from_source_var(db, dep_sv, project);
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
        dimensions: converted_dims,
        dimensions_ctx: dim_context,
        model_name: &model_name_ident,
        metadata: &all_metadata,
        module_models: &module_models,
        inputs: &inputs,
    };

    let var = crate::compiler::Var::new(
        &crate::compiler::Context::new(core, &var_ident_canonical, is_initial),
        &lowered,
    )
    .ok()?;

    // Offsets in the per-variable form `compile_phase_to_per_var_bytecodes`
    // expects, built from the mini-layout `all_metadata` exactly as the
    // former inline `compile_phase` closure built them (so the shared
    // compile+symbolize tail is byte-identical to the prior per-implicit
    // behavior -- this replaces the verbatim-duplicate closure with the
    // single shared relation).
    let offsets: PerVarOffsetMap = all_metadata
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                v.iter()
                    .map(|(vk, vm)| (vk.clone(), (vm.offset, vm.size)))
                    .collect(),
            )
        })
        .collect();

    compile_phase_to_per_var_bytecodes(
        &var.ast,
        &offsets,
        &rmap,
        &tables,
        &module_refs,
        mini_offset,
        converted_dims,
        dim_context,
        &model_name_ident,
        &var_ident_canonical,
        &inputs,
    )
}

/// Assemble a complete CompiledModule from per-variable fragments.
///
/// Salsa-tracked: the per-module assembly (fragment concatenation, SCC
/// combined-fragment build, GF dedup, resolve) is memoized so an unchanged
/// module (same `model`/`project`/`is_root`/`module_inputs`) is a pure
/// cache hit -- no re-concatenation, no re-resolve. The success payload rides
/// behind an `Arc` so the tracked-fn return value is `salsa::Update` (its
/// inner `CompiledModule` derives `Update` via the per-field `PartialEq`
/// fallback for the opaque bytecode side-channels) and salsa's clone-out on
/// each cache-hit read is a single refcount bump rather than a deep bytecode
/// clone.
///
/// `module_inputs` is an interned `ModuleInputSet` (the sorted canonical input
/// names). The empty set is the no-inputs case and, being a single interned
/// id, shares one cache entry across all no-input callers.
#[salsa::tracked]
pub fn assemble_module<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_inputs: ModuleInputSet<'db>,
) -> Result<std::sync::Arc<crate::bytecode::CompiledModule>, String> {
    use crate::compiler::symbolic::{
        ContextResourceCounts, SymbolicCompiledInitial, SymbolicCompiledModule,
        concatenate_fragments_with_gf, resolve_module,
    };

    // The interned set stores the sorted canonical names; the plain lowering
    // helpers (`compile_implicit_var_fragment` and friends) still take
    // `&[String]`, so read it back as a slice.
    let module_input_names = module_inputs.names(db);
    // Reconstruct the `BTreeSet<Ident<Canonical>>` the assembly logic (the
    // `is_module_input` predicate, the module-input exclusion in the stocks
    // phase) consumes -- the exact inverse of the input set's key derivation.
    let canonical_inputs = module_inputs.canonical_input_set(db);
    let dep_graph = model_dependency_graph(db, model, project, module_inputs);
    if dep_graph.has_cycle {
        let msg = format!("model '{}' has circular dependencies", model.name(db));
        return Err(msg);
    }
    let layout = compute_layout(db, model, project, is_root);
    let source_vars = model.variables(db);
    let implicit_info = model_implicit_var_info(db, model, project);
    let model_name = model.name(db).clone();

    // Pre-compile all fragments (explicit + implicit) into a combined map
    let mut all_fragments: HashMap<String, VarFragmentResult> = HashMap::new();

    for (name, svar) in source_vars.iter() {
        if let Some(result) =
            compile_var_fragment(db, *svar, model, project, is_root, module_inputs)
        {
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
            module_input_names,
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
    let mut ltm_flow_names: Vec<String> = Vec::new();
    if project.ltm_enabled(db) {
        let ltm_vars = model_ltm_variables(db, model, project);

        for ltm_var in &ltm_vars.vars {
            let ltm_var_canonical = canonicalize(&ltm_var.name).into_owned();

            // Select and compile this LTM var's fragment. The
            // selection logic (salsa-cached `(from, to)` path vs.
            // direct compilation of the prepared equation) lives in
            // `compile_ltm_synthetic_fragment` so the diagnostic pass
            // (`model_ltm_fragment_diagnostics`) detects the exact same
            // compile failures this assembly pass would silently drop.
            let fragment_result = compile_ltm_synthetic_fragment(db, ltm_var, model, project);

            if let Some(result) = fragment_result {
                // Drop LTM fragments whose symbolic variable references can't
                // be resolved in this model's layout.  This happens when
                // sub-model LTM equations reference implicit stdlib module
                // instance names (e.g. "smth1") that only exist in the root
                // model's namespace under qualified names like
                // "$:var_name:0:smth1".  Silently dropping these is correct:
                // the root model generates its own LTM vars using the
                // qualified names, so sub-model LTM vars for the same modules
                // would be duplicates anyway.
                if crate::compiler::symbolic::fragment_vars_in_layout(&result.fragment, layout) {
                    all_fragments.insert(ltm_var_canonical.clone(), result);
                    ltm_flow_names.push(ltm_var_canonical);
                }
            }
        }

        // Also compile the implicit modules (PREVIOUS instances) from LTM
        // equations. These are module-type variables that need initial and
        // stock phase compilation like regular implicit modules.
        let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
        let ltm_module_idents = ltm::ltm_module_idents(db, model, project);
        for ltm_var in &ltm_vars.vars {
            let parsed = ltm::parse_ltm_var_with_ids(db, ltm_var, project, &ltm_module_idents);
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
                        module_input_names,
                    );
                    if let Some(result) = im_fragment {
                        // Same layout check as for main LTM vars above.
                        if crate::compiler::symbolic::fragment_vars_in_layout(
                            &result.fragment,
                            layout,
                        ) {
                            all_fragments.insert(im_name.clone(), result);
                        }
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
        |var_name: &str| -> bool { canonical_inputs.contains(&*canonicalize(var_name)) };

    // ── Combined per-element fragments for resolved recurrence SCCs ─────
    //
    // A multi-member (or single-variable) recurrence SCC whose induced
    // element graph the cycle gate proved acyclic (`dep_graph
    // .resolved_sccs`, populated by the Task 4 symbolic verdict) is
    // lowered as ONE combined `PerVarBytecodes` whose per-element writes
    // follow the SCC's verified `element_order` (Task 5
    // `combine_scc_fragment`), instead of the members' individual
    // one-contiguous-block-per-variable fragments -- the latter cannot
    // express the required cross-member per-element interleaving. Each
    // member's symbolic fragment is sourced via the EXACT production
    // compile+symbolize path (`var_phase_symbolic_fragment_prod`, the
    // Task 4 accessor -- never a re-derivation), so every write keeps its
    // original `SymVarRef { name, element_offset }`; `resolve_module`
    // therefore maps each write to the same model slot the acyclic layout
    // assigns and the results offset map is unchanged (AC2.3).
    //
    // Two combined fragments per SCC are built up-front so they OUTLIVE
    // the `concatenate_fragments` / init-renumber calls below (the
    // `flow_frags`/`initial_frags` vectors hold `&` borrows into these):
    //  * the DT combined fragment (sourced from each member's
    //    `SccPhase::Dt` symbolic fragment), injected into the flows
    //    runlist -- only `phase == Dt` SCCs (an `Initial`-phase SCC is
    //    stock-backed and stocks are not flow variables).
    //  * the INIT combined fragment (sourced from each member's
    //    `SccPhase::Initial` symbolic fragment), injected into the
    //    initials runlist via the Task 1 spike's single synthetic-ident
    //    `SymbolicCompiledInitial` mechanism -- built for EVERY resolved
    //    SCC (both phases), because a `Dt`-phase aux SCC's members carry
    //    the SAME recurrence in their init equations and the initials
    //    runlist groups BOTH phases contiguously (see the
    //    `build_scc_grouping(false)` runlist comment). The SCC's
    //    `element_order` (dt order for a `phase: Dt` SCC) is valid for
    //    the init interleave because a same-equation aux's init and dt
    //    element graphs are structurally identical; if they ever diverge
    //    (a member's init fragment cannot be segmented to match
    //    `element_order`) `combine_scc_fragment` returns a loud-safe
    //    `Err` and assembly fails with an Assembly diagnostic rather than
    //    miscompiling.
    //
    // Loud-safe: an unsourceable member (`var_phase_symbolic_fragment_prod`
    // returned `None`) or a `combine_scc_fragment` error accumulates an
    // Assembly diagnostic and aborts assembly (mirrors the existing
    // missing-fragment / concatenate-error pattern); the combined
    // fragment is NEVER silently dropped or partially injected.
    let resolved_sccs = &dep_graph.resolved_sccs;
    let combine_scc_for_phase = |scc: &ResolvedScc,
                                 phase: SccPhase|
     -> Result<crate::compiler::symbolic::PerVarBytecodes, String> {
        let mut member_fragments: HashMap<
            Ident<Canonical>,
            crate::compiler::symbolic::PerVarBytecodes,
        > = HashMap::with_capacity(scc.members.len());
        for member in &scc.members {
            let frag = var_phase_symbolic_fragment_prod(
                db,
                model,
                project,
                member.as_str(),
                phase.clone(),
            )
            .ok_or_else(|| {
                format!(
                    "resolved recurrence SCC member `{}` has no \
                         sourceable symbolic fragment for its phase; \
                         cannot build the combined per-element fragment",
                    member.as_str()
                )
            })?;
            member_fragments.insert(member.clone(), frag);
        }
        combine_scc_fragment(scc, &member_fragments)
    };

    // DT combined fragments, indexed parallel to `resolved_sccs`
    // (`None` for an `Initial`-phase SCC -- not a flow). INIT combined
    // fragments for every SCC. Both owned here to the end of
    // `assemble_module`.
    let mut dt_combined: Vec<Option<crate::compiler::symbolic::PerVarBytecodes>> =
        Vec::with_capacity(resolved_sccs.len());
    let mut init_combined: Vec<crate::compiler::symbolic::PerVarBytecodes> =
        Vec::with_capacity(resolved_sccs.len());
    for scc in resolved_sccs.iter() {
        let dt = if scc.phase == SccPhase::Dt {
            Some(combine_scc_for_phase(scc, SccPhase::Dt)?)
        } else {
            None
        };
        let init = combine_scc_for_phase(scc, SccPhase::Initial)?;
        dt_combined.push(dt);
        init_combined.push(init);
    }

    // Member-name -> resolved-SCC index. A member is in at most one SCC
    // (the SCCs in `resolved_sccs` are pairwise disjoint -- see
    // `scc_map_from_resolved`), so this is well-defined.
    let scc_of_member: HashMap<&str, usize> = resolved_sccs
        .iter()
        .enumerate()
        .flat_map(|(idx, scc)| scc.members.iter().map(move |m| (m.as_str(), idx)))
        .collect();

    // Collect fragments for each phase, tracking missing variables
    let mut initial_frags: Vec<(String, &crate::compiler::symbolic::PerVarBytecodes)> = Vec::new();
    let mut flow_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();
    let mut stock_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = Vec::new();
    let mut missing_vars: Vec<String> = Vec::new();

    // Track which SCCs have already had their combined fragment injected
    // in each runlist. Task 5b guarantees a resolved SCC's members are a
    // contiguous, byte-stable block at the SCC's topological slot, so
    // "inject at the first member encountered, skip the rest" lands the
    // combined fragment in the correct relative position. The runlist
    // `Vec<String>` itself is salsa-owned and NOT mutated (we skip during
    // collection, never remove).
    let mut injected_init_sccs: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut injected_flow_sccs: std::collections::HashSet<usize> = std::collections::HashSet::new();

    for var_name in &dep_graph.runlist_initials {
        if let Some(&scc_idx) = scc_of_member.get(var_name.as_str()) {
            // A resolved-SCC member: its per-ident init fragment is
            // SUBSUMED by the SCC's single combined init fragment. Inject
            // that combined fragment once, at the first member of this
            // SCC seen in the initials runlist, under a synthetic ident
            // (`$⁚scc⁚init⁚{n}`). The spike verified `resolve_module` /
            // `eval_initials` consume `compiled_initials` positionally
            // (ident-agnostic; offsets re-derived from the bytecode's
            // `AssignCurr` operands), so one `SymbolicCompiledInitial`
            // may write every member's init slots.
            if injected_init_sccs.insert(scc_idx) {
                let synthetic_ident = format!("$\u{205A}scc\u{205A}init\u{205A}{scc_idx}");
                initial_frags.push((synthetic_ident, &init_combined[scc_idx]));
            }
            // Non-first members (and the first, after injection): skip
            // the per-ident push entirely.
            continue;
        }
        if let Some(result) = all_fragments.get(var_name)
            && let Some(ref bc) = result.fragment.initial_bytecodes
        {
            initial_frags.push((var_name.clone(), bc));
        } else if !is_module_input(var_name) {
            missing_vars.push(var_name.clone());
        }
    }

    for var_name in &dep_graph.runlist_flows {
        if let Some(&scc_idx) = scc_of_member.get(var_name.as_str())
            && let Some(ref combined) = dt_combined[scc_idx]
        {
            // A `phase == Dt` resolved-SCC member: its per-variable flow
            // fragment is subsumed by the SCC's combined dt fragment.
            // Push the combined fragment once, at the first member of
            // this SCC encountered in the flows runlist; skip the rest.
            if injected_flow_sccs.insert(scc_idx) {
                flow_frags.push(combined);
            }
            continue;
        }
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

    // Append LTM implicit var fragments to the relevant runlists.
    // Some implicit vars participate in initials and/or stocks even
    // though they are not part of the original model.
    if project.ltm_enabled(db) {
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
        return Err(msg);
    }

    // Compute context resource base offsets for each phase so that flows
    // and stocks reference the same resource namespace as the all-phases
    // merge. The all-phases ordering is: initials, then flows, then stocks.
    let initial_refs: Vec<&crate::compiler::symbolic::PerVarBytecodes> =
        initial_frags.iter().map(|(_, bc)| *bc).collect();
    let initial_counts = ContextResourceCounts::from_fragments(&initial_refs);
    let flow_counts = ContextResourceCounts::from_fragments(&flow_frags);

    // #583: temps are NOT a per-phase-offset resource. The plain-phase
    // concat recycles every fragment's 0-based temps into ONE shared
    // identity pool (matching the monolithic `Module::compile` keyed
    // max-merge over the flattened initials+flows+stocks runlists), so the
    // `ctx_base.temps` is 0 for EVERY phase -- the pool is not partitioned by
    // phase. (Summing per phase, as before, drove the renumbered `temp_id`
    // past `u8::MAX` and diverged `flows_concat` from the all-phases `merged`
    // temp_offsets table the VM consumes.) Modules/views/dim-lists DO stay
    // per-phase summed: each is a distinct resource, laid out disjointly
    // across phases exactly as the all-phases `merged` lays them out.
    let no_base = ContextResourceCounts::default();
    let flow_base = ContextResourceCounts {
        temps: 0,
        ..initial_counts.clone()
    };
    let stock_base = ContextResourceCounts {
        modules: initial_counts.modules + flow_counts.modules,
        views: initial_counts.views + flow_counts.views,
        temps: 0,
        dim_lists: initial_counts.dim_lists + flow_counts.dim_lists,
    };

    // #582: graphical functions are content-de-duplicated across ALL
    // fragments of the model (one block per distinct table, matching the
    // monolithic `Compiler::new`), so -- unlike the flat literal/module/
    // view/temp/dim-list resources -- their `base_gf`s cannot be a per-phase
    // running count. Build the dedup ONCE over the union of every phase's
    // fragments (in the all-phases order initials, flows, stocks) and feed
    // each phase the corresponding per-fragment GF remap. A dependency
    // arrayed GF referenced by hundreds of consumer fragments now lands in
    // `graphical_functions` exactly once instead of once per consumer,
    // which both fixes the `GraphicalFunctionId = u8` overflow and matches
    // the monolithic GF-table layout.
    let all_frags: Vec<&crate::compiler::symbolic::PerVarBytecodes> = initial_frags
        .iter()
        .map(|(_, bc)| *bc)
        .chain(flow_frags.iter().copied())
        .chain(stock_frags.iter().copied())
        .collect();
    let gf_dedup = crate::compiler::symbolic::GfDedup::build(&all_frags)?;
    // Phase offsets into `all_frags` so each phase's fragments map to their
    // remap entry.
    let n_init = initial_frags.len();
    let n_flow = flow_frags.len();

    let flows_concat = concatenate_fragments_with_gf(&flow_frags, &flow_base, &gf_dedup, n_init)?;
    let stocks_concat =
        concatenate_fragments_with_gf(&stock_frags, &stock_base, &gf_dedup, n_init + n_flow)?;

    // Build SymbolicCompiledInitial for each initial variable, renumbered
    // so context resource IDs (GFs, modules, views, temps, dim_lists) match
    // the all-phases merge. Literal IDs are local to each initial's bytecode
    // so they get no base offset. The GF base comes from the shared dedup
    // (initial `i` is `all_frags[i]`); the other resources stay flat.
    let mut compiled_initials: Vec<SymbolicCompiledInitial> = Vec::new();
    let mut init_mod_off: u16 = 0;
    let mut init_view_off: u16 = 0;
    // #583: temps recycle into the shared identity pool (the same pool the
    // `merged` table below builds), so each initial's temp ids stay
    // fragment-local (offset 0) -- they are NOT advanced per initial.
    let init_temp_off: u32 = 0;
    let mut init_dl_off: u16 = 0;
    for (i, (name, bc)) in initial_frags.iter().enumerate() {
        let gf_remap = gf_dedup.remap(i);
        let renumbered_code: Vec<crate::compiler::symbolic::SymbolicOpcode> = bc
            .symbolic
            .code
            .iter()
            .map(|op| {
                crate::compiler::symbolic::renumber_opcode(
                    op,
                    0, // literals are local to each initial's bytecode
                    gf_remap,
                    init_mod_off,
                    init_view_off,
                    init_temp_off,
                    init_dl_off,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        compiled_initials.push(SymbolicCompiledInitial {
            ident: Ident::new(name),
            bytecode: crate::compiler::symbolic::SymbolicByteCode {
                literals: bc.symbolic.literals.clone(),
                code: renumbered_code,
            },
        });
        init_mod_off += bc.module_decls.len() as u16;
        init_view_off += bc.static_views.len() as u16;
        // `init_temp_off` is NOT advanced (#583): temps recycle into the
        // shared identity pool, so every initial's temp ids stay
        // fragment-local and index the same `merged.temp_offsets` table.
        init_dl_off += bc.dim_lists.len() as u16;
    }

    // The all-phases merge for the shared context side-channels (modules,
    // views, temps, dim_lists); its `graphical_functions` is the dedup's
    // single table (set by `concatenate_fragments_with_gf`), shared by all
    // three phases.
    let merged = concatenate_fragments_with_gf(&all_frags, &no_base, &gf_dedup, 0)?;

    // Build dimension metadata from project dimensions (mirrors
    // Compiler::populate_dimension_metadata). Read the project-global converted
    // dims from the salsa-cached query instead of rebuilding them here.
    let converted_dims = project_converted_dimensions(db, project);

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

    for dim in converted_dims {
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

    // Resolve symbolic -> concrete offsets. The CompiledModule stays a pure,
    // symbolizable artifact (the symbolic roundtrip tests symbolize it again,
    // and salsa caches it); the 3-address fusion (R2) is applied later, at
    // Vm::new, to the execution copy of the bytecode. The success payload is
    // wrapped in an `Arc` so this tracked fn's return type is `salsa::Update`
    // and salsa's clone-out is a refcount bump (the inner bytecode is large).
    resolve_module(&sym_module, layout).map(std::sync::Arc::new)
}

/// Assemble a full CompiledSimulation from assembled modules.
///
/// Salsa-tracked: enumerating module instances, assembling each unique
/// `(model, input_set)` module, building the `Specs`, and computing the
/// flattened offset map are all memoized, so a recompile with no input
/// changes is a pure cache hit (zero re-assembly). When one variable
/// changes, only the affected `assemble_module` instances re-execute;
/// unchanged submodules cache-hit. `main_model_name` is an owned `String`
/// (a salsa-compatible by-value key); the success payload rides behind an
/// `Arc` so the return type is `salsa::Update` and clone-out is a refcount
/// bump rather than a deep clone of the modules/offsets maps.
#[salsa::tracked]
pub fn assemble_simulation(
    db: &dyn Db,
    project: SourceProject,
    main_model_name: String,
) -> Result<std::sync::Arc<crate::vm::CompiledSimulation>, String> {
    use crate::common::{Canonical, Ident};
    use crate::vm::CompiledSimulation;

    let project_models = project.models(db);
    let main_model_canonical = canonicalize(&main_model_name);

    if !project_models.contains_key(main_model_canonical.as_ref()) {
        let msg = format!("no model named '{}' to simulate", main_model_name);
        return Err(msg);
    }

    // Enumerate module instances by walking module variables recursively.
    // Each unique (model_name, input_set) pair gets its own CompiledModule.
    let module_instances = enumerate_module_instances(db, project, &main_model_name)?;

    // Sort module names: main first, then all others alphabetically
    let main_ident = Ident::<Canonical>::new(&main_model_name);
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

            let is_root = canonicalize(name.as_str()) == main_model_canonical;
            // The tracked `assemble_module` keys on an interned `ModuleInputSet`
            // (the sorted canonical input names). `inputs` is already a
            // `BTreeSet<Ident<Canonical>>`, so this is the canonical round-trip.
            let module_inputs = ModuleInputSet::from_canonical_set(db, inputs);
            let compiled = assemble_module(db, *source_model, project, is_root, module_inputs)?;
            let module_key: crate::vm::ModuleKey = ((*name).clone(), inputs.clone());
            // Clone the `CompiledModule` out of the salsa-owned `Arc`: the
            // `CompiledSimulation.modules` map stores it by value (its bytecode
            // is itself `Arc`-backed, so this clone is cheap refcount bumps).
            compiled_modules.insert(module_key, (*compiled).clone());
        }
    }

    // Build Specs, preferring model-level sim_specs override when present
    let specs = if let Some(source_model) = project_models.get(main_model_canonical.as_ref())
        && let Some(ref model_specs) = *source_model.model_sim_specs(db)
    {
        crate::vm::Specs::from(model_specs)
    } else {
        crate::vm::Specs::from(project.sim_specs(db))
    };

    // Compute flattened offsets for variable name -> offset mapping
    let offsets = calc_flattened_offsets_incremental(db, project, &main_model_name, true);
    let offsets: HashMap<Ident<Canonical>, usize> =
        offsets.into_iter().map(|(k, (off, _))| (k, off)).collect();

    Ok(std::sync::Arc::new(CompiledSimulation::new(
        compiled_modules,
        specs,
        root_key,
        offsets,
    )))
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
        let module_ident_context = model_module_ident_context(db, *source_model, project, vec![]);
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
    // enabled. Models without feedback loops produce empty lists.
    if project.ltm_enabled(db) {
        let ltm_implicit = ltm::model_ltm_implicit_var_info(db, *source_model, project);
        let ltm_module_idents = ltm::ltm_module_idents(db, *source_model, project);

        let ltm_vars = model_ltm_variables(db, *source_model, project);

        for ltm_var in &ltm_vars.vars {
            let parsed = ltm::parse_ltm_var_with_ids(db, ltm_var, project, &ltm_module_idents);

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

/// Compute flattened offsets for each variable in a model, mapping
/// canonical variable names to (start_offset, size) pairs.
/// Works with SourceModel/SourceVariable from the salsa database.
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
        let ident_canonical = Ident::new(ident.as_str());
        let size = if let Some(svar) = source_vars.get(ident.as_str()) {
            if svar.kind(db) == SourceVariableKind::Module {
                let sub_model_name = svar.model_name(db);
                let sub_offsets =
                    calc_flattened_offsets_incremental(db, project, sub_model_name, false);
                let mut sub_var_names: Vec<&Ident<Canonical>> = sub_offsets.keys().collect();
                sub_var_names.sort_unstable();
                for sub_name in &sub_var_names {
                    let (sub_off, sub_size) = sub_offsets[*sub_name];
                    offsets.insert(
                        Ident::join(
                            &ident_canonical.as_canonical_str(),
                            &sub_name.as_canonical_str(),
                        ),
                        (i + sub_off, sub_size),
                    );
                }
                let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
                sub_size
            } else {
                let var_sz = variable_size(db, *svar, project);
                // A lookup-only table is not a saved output variable: reserve
                // its layout slot (so these offsets stay in lockstep with
                // `compute_layout`, whose map codegen's table-identity
                // reverse-map resolves against) but do NOT expose its name in
                // this VM/Results map -- it produces no series (issue #606).
                if !source_var_is_table_only(db, *svar) {
                    if var_sz > 1 {
                        // Array variable: produce per-element offsets
                        let dims = variable_dimensions(db, *svar, project);
                        if !dims.is_empty() {
                            for (j, subscripts) in
                                crate::dimensions::SubscriptIterator::new(dims).enumerate()
                            {
                                let subscript = subscripts.join(",");
                                let subscripted_ident = Ident::<Canonical>::from_unchecked(
                                    format!("{}[{}]", ident_canonical.as_str(), subscript),
                                );
                                offsets.insert(subscripted_ident, (i + j, 1));
                            }
                        }
                    } else {
                        offsets.insert(ident_canonical.clone(), (i, 1));
                    }
                }
                var_sz
            }
        } else {
            offsets.insert(ident_canonical.clone(), (i, 1));
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
                    offsets.insert(
                        Ident::join(
                            &ident_canonical.as_canonical_str(),
                            &sub_name.as_canonical_str(),
                        ),
                        (i + sub_off, sub_size),
                    );
                }
                let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
                i += sub_size;
            } else {
                offsets.insert(ident_canonical.clone(), (i, info.size));
                i += info.size;
            }
        } else {
            offsets.insert(ident_canonical.clone(), (i, info.size));
            i += info.size;
        }
    }

    // Include LTM variables (loop scores, relative loop scores, and their
    // implicit helper/module vars) when LTM is enabled. Models without
    // feedback loops get empty LTM var lists. These occupy slots after the
    // implicit variables, matching compute_layout's Section 3 ordering.
    if project.ltm_enabled(db) {
        let layout = compute_layout(db, *source_model, project, is_root);

        let ltm_vars = model_ltm_variables(db, *source_model, project);

        let ltm_implicit = ltm::model_ltm_implicit_var_info(db, *source_model, project);
        let ltm_module_idents = ltm::ltm_module_idents(db, *source_model, project);

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

            // Add implicit variables from this LTM equation
            let parsed = ltm::parse_ltm_var_with_ids(db, ltm_var, project, &ltm_module_idents);
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

/// Set the `ltm_discovery_mode` flag on a `SourceProject` salsa input.
///
/// When true, LTM generates link scores for every causal edge rather
/// than only edges participating in detected feedback loops.
pub fn set_project_ltm_discovery_mode(db: &mut SimlinDb, project: SourceProject, enabled: bool) {
    use salsa::Setter;
    if project.ltm_discovery_mode(db) != enabled {
        project.set_ltm_discovery_mode(db).to(enabled);
    }
}

/// Compile a project incrementally using salsa tracked functions.
///
/// This is the production compilation entry point. Returns the assembled
/// `CompiledSimulation` for the named model, or `Err(NotSimulatable)` if
/// compilation fails (e.g., unresolved references, unsupported builtins).
pub fn compile_project_incremental(
    db: &SimlinDb,
    project: SourceProject,
    main_model_name: &str,
) -> crate::Result<crate::vm::CompiledSimulation> {
    // An invalid macro set (AC5.2 cycle / AC5.3 duplicate / collision) fails
    // the project-level compile before per-model processing, uniformly as
    // `NotSimulatable` (the build error's own typed code rides the
    // diagnostic `project_macro_registry` accumulated -- see that module).
    if let Some((_code, msg)) =
        &crate::db::macro_registry::project_macro_registry(db, project).build_error
    {
        return crate::sim_err!(NotSimulatable, msg.clone());
    }
    // `assemble_simulation` is salsa-tracked, returning an `Arc` so its return
    // type is `salsa::Update`; clone the `CompiledSimulation` out of the
    // salsa-owned `Arc` to preserve this entry point's owned return type
    // byte-for-byte. The error half stays a `String` mapped to
    // `NotSimulatable`, identical to the prior plain-function behavior.
    match assemble_simulation(db, project, main_model_name.to_string()) {
        Ok(compiled) => Ok((*compiled).clone()),
        Err(msg) => crate::sim_err!(NotSimulatable, msg.clone()),
    }
}

#[cfg(test)]
mod combined_fragment_tests;
#[cfg(test)]
mod diagnostic_tests;
#[cfg(test)]
mod differential_tests;
#[cfg(test)]
mod dimension_context_cache_tests;
#[cfg(test)]
mod dimension_invalidation_tests;
#[cfg(test)]
mod fragment_cache_tests;
#[cfg(test)]
mod ltm_module_tests;
#[cfg(test)]
mod ltm_unified_tests;
#[cfg(test)]
mod prev_init_tests;
#[cfg(test)]
mod tests;
