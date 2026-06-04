// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::datamodel;

// `BTreeSet` is no longer used by the root module's own code after the
// `db/` split, but the root-mounted `#[cfg(test)]` test modules pull it in
// through their `use super::*` glob (preserving the pre-split import
// surface), so keep it in scope for the test build only.
#[cfg(test)]
use std::collections::BTreeSet;

// `db.rs` is the module ROOT; the bulk of the salsa pipeline is split into
// `db/<name>.rs` submodules so each file stays under the per-file line cap
// (`scripts/lint-project.sh` rule 2). They reach each other and the parent
// via `crate::db::...`; the root re-exports each submodule's items below so
// the historical `simlin_engine::db::...` surface (and the `use super::*`
// globs in the `#[cfg(test)]` test modules and `implicit_deps.rs`) keep
// resolving. The split modules and their concerns:
//
// * `input`      -- the `#[salsa::input]` structs + interned key types.
// * `query`      -- demand-driven read queries (parse, dims, deps, module map).
// * `sync`       -- datamodel -> salsa-input sync (fresh + incremental).
// * `diagnostic` -- the `CompilationDiagnostic` accumulator + drain helpers.
// * `layout`     -- the per-model body layout query.
// * `var_fragment` / `fragment_compile` -- the lowering / emission halves of
//   per-variable compilation.
// * `assemble`   -- module/simulation assembly + flattened-offset map.
// * `dep_graph`  -- the dependency-graph cycle gate + its result types.
// * `analysis`   -- causal-graph analysis tracked functions.
// * `ltm` / `ltm_ir` / `macro_registry` / `units` -- LTM (a `ltm/` directory:
//   mod/parse/compile/loops/link_scores), the reference-site IR, the macro
//   registry, and the unit-check pass.
mod dep_graph;
#[cfg(test)]
mod element_graph_proptest;
mod ltm_ir;
mod macro_registry;
mod units;
mod var_fragment;

mod diagnostic;
pub use diagnostic::{
    CompilationDiagnostic, Diagnostic, DiagnosticError, DiagnosticSeverity,
    collect_all_diagnostics, collect_model_diagnostics, model_all_diagnostics,
};

mod input;
pub(crate) use input::source_var_is_table_only;
pub use input::{
    LtmLinkId, ModuleIdentContext, ModuleInputSet, PinnedLoopSpec, SourceModel, SourceProject,
    SourceVariable, SourceVariableKind, datamodel_variable_from_source,
};

mod query;
pub(crate) use query::canonical_module_input_set;
pub use query::{
    ImplicitVarMeta, ParsedVariableResult, VariableDeps, model_implicit_var_info,
    model_module_ident_context, model_module_map, parse_source_variable_with_module_context,
    project_converted_dimensions, project_datamodel_dims, project_dimensions_context,
    project_units_context, variable_dimensions, variable_direct_dependencies,
    variable_relevant_dimensions, variable_size,
};

mod sync;
pub use sync::{
    PersistentModelState, PersistentSyncState, PersistentVariableState, SyncResult, SyncedModel,
    SyncedVariable, sync_from_datamodel, sync_from_datamodel_incremental,
};
pub(crate) use sync::{build_stdlib_models, expand_maps_to_chains};

mod layout;
pub use layout::compute_layout;

mod fragment_compile;
pub use fragment_compile::compile_var_fragment;
pub(crate) use fragment_compile::{
    compile_implicit_var_fragment, compile_implicit_var_phase_bytecodes,
};

mod assemble;
pub(crate) use assemble::{
    PerVarOffsetMap, VarFragmentResult, build_module_inputs, build_stub_variable,
    build_submodel_metadata, compile_phase_to_per_var_bytecodes, extract_tables_from_source_var,
    var_phase_symbolic_fragment_prod,
};
pub use assemble::{assemble_module, assemble_simulation};
// `combine_scc_fragment` and `calc_flattened_offsets_incremental` are
// consumed at runtime only WITHIN `assemble.rs`; the root re-export exists
// solely so the `#[cfg(test)]` test modules
// (`combined_fragment_tests`/`fragment_cache_tests`) can reach them as
// `crate::db::...` / `super::...`.
#[cfg(test)]
pub(crate) use assemble::{calc_flattened_offsets_incremental, combine_scc_fragment};

pub use dep_graph::{ModelDepGraphResult, ResolvedScc, SccPhase, model_dependency_graph};

mod ltm;
use ltm::*;
pub use ltm::{
    LtmImplicitVarMeta, compile_ltm_var_fragment, link_score_equation_text_shaped,
    model_ltm_implicit_module_refs, model_ltm_implicit_var_info, model_ltm_mode,
    model_ltm_var_name_index, model_ltm_variables,
};
// The cross-agg petal-stitching core, shared with `crate::ltm_finding`'s
// discovery-mode recovery (GH #696).
pub(crate) use ltm::{
    StitchPetal, collect_agg_petals, cross_agg_loop_budget, stitch_cross_agg_petals,
    sub_model_output_ports,
};
// Test-only: the cross-agg loop-count budget override, so `ltm_finding`'s
// discovery-mode truncation test can trip the budget with a tiny fixture
// (per docs/dev/rust.md#test-time-budgets) instead of building one large
// enough to hit the production constant.
#[cfg(test)]
pub(crate) use ltm::AggLoopBudgetGuard;

mod analysis;
pub use analysis::RefShape;
pub use analysis::causal_graph_from_edges;
pub use analysis::causal_graph_from_element_edges;
pub use analysis::causal_graph_from_element_edges_with_modules;
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
    reclassify_loops_from_results,
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
    /// The immutable stdlib model inputs (SMOOTH/DELAY/TREND/systems_*),
    /// built EXACTLY ONCE per db session and reused by every sync.
    ///
    /// Stdlib models never change, so re-walking `crate::stdlib::MODEL_NAMES`
    /// (with its per-name `format!`/`canonicalize`/`get`) and re-creating the
    /// `SourceModel`/`SourceVariable` salsa inputs on every sync is pure
    /// overhead on the interactive edit/sync hot path. Building the inputs once
    /// and splicing the cached `PersistentModelState` handles into each synced
    /// project keeps the stdlib salsa input handles IDENTICAL across syncs, so
    /// salsa treats them as unchanged and never invalidates a query that
    /// depends on a stdlib model (e.g. a SMOOTH instantiation's compiled
    /// fragment stays cached across unrelated user edits).
    ///
    /// `OnceLock` (not the `&mut self`-only `sync_state` pattern) is required
    /// because the fresh `sync_from_datamodel` path holds only `&db`; the
    /// salsa inputs are created during the one-time init, which needs only the
    /// same shared `&db` salsa-input creation uses elsewhere. `OnceLock` is
    /// `Sync` (unlike `std::cell::OnceCell`), preserving the `SimlinDb: Sync`
    /// bound salsa's parallel query execution requires.
    stdlib_models: OnceLock<Arc<StdlibModels>>,
}

/// The one-shot stdlib salsa-input cache held by `SimlinDb::stdlib_models`.
///
/// Built once from `crate::stdlib::MODEL_NAMES`; thereafter both sync paths
/// splice these handles in without re-walking `MODEL_NAMES` or re-doing the
/// `format!`/`canonicalize`/`crate::stdlib::get` work. `pub(crate)` because
/// `build_stdlib_models` (in `db::sync`) returns it; its fields stay private
/// (accessible to the `db::sync` descendant module that builds and splices it).
pub(crate) struct StdlibModels {
    /// Canonical name -> the stdlib model's persistent handles
    /// (`source_model`, per-variable handles, `is_stdlib == true`). Cloned
    /// into each synced project's model map.
    by_canonical: HashMap<String, PersistentModelState>,
    /// `(canonical name, display "stdlib\u{205A}{name}")` pairs in
    /// `MODEL_NAMES` order. Splicing iterates this so the stdlib display names
    /// are appended to `model_names` in the same order the old per-sync walk
    /// produced (preserving the byte-identical ordering downstream consumers
    /// see).
    ordered: Vec<(String, String)>,
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

    /// Get the one-shot stdlib model cache, building it the first time it is
    /// needed and reusing it on every subsequent sync.
    ///
    /// The build creates the stdlib `SourceModel`/`SourceVariable` salsa inputs
    /// exactly as the old per-sync walk did, but only once: the returned
    /// `PersistentModelState` handles are stable for the db's lifetime, so
    /// salsa never re-creates (and hence never invalidates) a stdlib input.
    /// Takes `&self` (not `&mut self`) so the fresh `sync_from_datamodel` path,
    /// which holds only `&db`, can build the cache too; salsa-input creation
    /// only needs a shared `&db`.
    fn stdlib_models(&self) -> &Arc<StdlibModels> {
        self.stdlib_models
            .get_or_init(|| Arc::new(build_stdlib_models(self)))
    }
}

#[salsa::db]
impl Db for SimlinDb {}

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

/// The loop-enumeration mode the LTM pipeline resolved for a model.
///
/// `model_ltm_variables` either enumerates every elementary circuit
/// (Johnson, [`Exhaustive`](LtmMode::Exhaustive)) or, for models whose
/// variable-level or cross-element SCC exceeds `ltm::MAX_LTM_SCC_NODES`
/// (or when the caller requested discovery directly), falls back to the
/// per-timestep strongest-path heuristic ([`Discovery`](LtmMode::Discovery)).
/// A user sees empty or different loop results in the two modes with no
/// other signal; this enum is that signal, surfaced through the FFI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, salsa::Update)]
pub enum LtmMode {
    /// Exhaustive Johnson enumeration of every elementary circuit.
    Exhaustive,
    /// Strongest-path discovery heuristic (the model tripped the SCC gate
    /// or the caller explicitly requested discovery).
    Discovery,
}

/// Result of LTM variable generation for a model.
///
/// `mode` records whether loop enumeration ran exhaustively or auto-flipped
/// (or was forced) to the discovery heuristic -- the only signal a caller has
/// for telling the two apart, since the synthetic-variable output otherwise
/// just looks empty or different.
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
///
/// `pathways_truncated` is `true` when internal module-pathway enumeration hit
/// the per-input-port pathway budget (`ltm::MAX_MODULE_PATHWAYS`, GH #649), so
/// at least one input port's composite link score was computed over a
/// deterministic prefix of its pathways rather than the complete set -- the
/// score is degraded, not wrong-by-panic. A `CompilationDiagnostic` `Warning`
/// naming the module + clipped port(s) accompanies it; the flag is the robust
/// signal. Only ever `true` for a model with input ports (a sub-model or a
/// discovery-mode model) whose pathway count exceeds the budget.
/// (`Debug`/`Eq` are conditional/absent for the same reasons as
/// `LtmSyntheticVar`.)
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct LtmVariablesResult {
    pub vars: Vec<LtmSyntheticVar>,
    pub loop_partitions: HashMap<String, Vec<Option<usize>>>,
    pub agg_recovery_truncated: bool,
    pub pathways_truncated: bool,
    pub mode: LtmMode,
}

/// Compute the link score equation text for a single causal link.
///
/// This is the per-link granularity that enables incremental recomputation:
/// when a variable's equation changes, salsa only re-evaluates link score
/// equations for links whose endpoints are affected. Links involving
/// unmodified variables return their cached equation text.
/// Signed unit-transfer formula for genuine black-box module links --
/// the residual case where neither a composite link score (the target
/// module exposes no internal pathway to the read port) nor a
/// ceteris-paribus partial (the endpoint is a module with no
/// parent-visible equation) is available.
///
/// Returns `0` at `INITIAL_TIME`, `0` when either endpoint did not change
/// over the last step (an inactive link, like every other link score),
/// and otherwise `SIGN(Δto) * SIGN(Δfrom)` -- i.e. `+1` when `to` and
/// `from` moved in the same direction and `-1` when they moved opposite.
///
/// Rationale. An LTM *link score* is `|Δ_x(z)/Δ(z)| * sign(Δ_x(z)/Δ(x))`
/// (ref §3.1), not the *gain* `Δz/Δx` (the sensitivity / partial
/// derivative, ref §3.3). The two differ by the `|Δx/Δz|` weighting that
/// makes link scores chain *multiplicatively* into a loop score: an
/// isolated feedback loop's raw loop score is exactly `±1` regardless of
/// the gains around it (Appendix B), an invariant the gain formula breaks
/// (the loop score scales with the product of the gains).
///
/// For a single-input black box `z = F(x)` the true link score *is* the
/// unit transfer: all of `Δz` is attributable to `x`, so `|Δ_x(z)/Δ(z)|`
/// is identically `1` and only the sign remains. For a stateful or
/// multi-input box this is the perfect-mixing-spirit approximation
/// (ref §6 macros): polarity exact, magnitude approximated as `1`. It
/// preserves the isolated-loop `±1` invariant where the gain formula
/// did not. Prefer the composite or ceteris-paribus forms wherever they
/// exist; this is only the fallback when they do not.
pub(super) fn black_box_unit_transfer_equation(from_ref: &str, to_ref: &str) -> String {
    let from_q = crate::ltm_augment::quote_ident(from_ref);
    let to_q = crate::ltm_augment::quote_ident(to_ref);
    format!(
        "if (TIME = INITIAL_TIME) then 0 \
         else if (({to_q} - PREVIOUS({to_q})) = 0) OR \
                 (({from_q} - PREVIOUS({from_q})) = 0) \
              then 0 \
         else (SIGN({to_q} - PREVIOUS({to_q})) * \
               SIGN({from_q} - PREVIOUS({from_q})))"
    )
}

/// Map each module variable in `model` to the sub-model internal variables
/// the rest of the model actually reads through it (the `port` suffixes of
/// `module·port` dependency references), each port list sorted for
/// determinism.
///
/// One cached pass over the model's variable dependency sets (mirroring the
/// scan `db::ltm::loops::find_model_output_ports` does across *parent*
/// models for a sub-model's ports, but scoped to module instances within
/// this model). Implicit-helper deps are included for the same reason as
/// there: SMOOTH/DELAY expansion synthesizes helper auxes whose deps may be
/// the only readers of a module output.
#[salsa::tracked(returns(ref))]
pub fn model_module_output_ports(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<String, Vec<String>> {
    let middot = '\u{00B7}';
    let empty_ctx = ModuleIdentContext::new(db, vec![]);
    let empty_inputs = ModuleInputSet::empty(db);
    let mut ports: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
    let record = |dep: &str, ports: &mut HashMap<String, std::collections::BTreeSet<String>>| {
        if let Some(dot_pos) = dep.find(middot) {
            let module_part = &dep[..dot_pos];
            let internal_var = &dep[dot_pos + middot.len_utf8()..];
            if !module_part.is_empty() && !internal_var.is_empty() {
                ports
                    .entry(module_part.to_string())
                    .or_default()
                    .insert(internal_var.to_string());
            }
        }
    };
    for (_, source_var) in model.variables(db).iter() {
        let deps = variable_direct_dependencies(db, *source_var, project, empty_ctx, empty_inputs);
        for dep in deps.dt_deps.iter().chain(deps.initial_deps.iter()) {
            record(dep, &mut ports);
        }
        for iv_deps in &deps.implicit_vars {
            for dep in &iv_deps.dt_deps {
                record(dep, &mut ports);
            }
        }
    }
    ports
        .into_iter()
        .map(|(module, port_set)| (module, port_set.into_iter().collect()))
        .collect()
}

/// Find output ports of a specific module variable by examining which
/// variables in the model reference it with `module·internal_var` syntax.
///
/// Stdlib modules always use the `output` convention. For user-defined
/// modules the ports come from [`model_module_output_ports`]'s dependency
/// scan -- the pre-scan code hardcoded `output` here ("we don't have deps"),
/// which silently zeroed every discovery-mode link score into a user module
/// whose output port has any other name (the `module·output` reference
/// resolved to nothing and the fragment stubbed to a constant). The
/// `output` fallback remains only for a module none of whose internals are
/// read (no deps to scan -- such a module drives nothing, so the link score
/// is moot either way).
pub(super) fn find_model_output_ports_for_module(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    edges: &CausalEdgesResult,
    module_var_name: &str,
) -> Vec<String> {
    if let Some(model_name) = edges.dynamic_modules.get(module_var_name)
        && model_name.starts_with("stdlib\u{205A}")
    {
        return vec!["output".to_string()];
    }
    model_module_output_ports(db, model, project)
        .get(module_var_name)
        .cloned()
        .unwrap_or_else(|| vec!["output".to_string()])
}

/// For a module variable in `model`, return the set of internal input
/// ports for which the sub-model emits a composite link score
/// (`$⁚ltm⁚composite⁚{port}`).
///
/// The parent's `input -> module` (and `module -> module`) link score can
/// reference the sub-model's composite only when that composite actually
/// exists. Any model with at least one input->output pathway generates
/// pathway/composite vars -- both DynamicModules (with internal stocks) and,
/// since PR #684, passthroughs (stockless, whose internals are a pure aux
/// chain LTM scores exactly). A module exposes NO composite only when its
/// output does not depend on its input at all (no internal pathway).
/// Referencing a non-existent composite var silently resolves to a constant
/// 0 (cross-module reads of an absent LTM var don't fail to compile), which
/// would zero every loop through the module. This is the authoritative
/// discriminator: it reads the sub-model's actual `model_ltm_variables`
/// output rather than guessing from the module's stock count.
///
/// NOTE: the composite max-abs-selects across ALL of the module's pathways,
/// so it is the WRONG link score for a loop that traverses one specific
/// output port of a multi-output module. The per-exit-port pathway selection
/// in `model_ltm_variables` (PR #684) overrides the loop-score reference for
/// such links; the composite remains the discovery-mode per-edge
/// approximation (no loop-score vars exist there to override).
///
/// Salsa-cached; the sub-model's `model_ltm_variables` is computed once
/// per `(sub_model, project)` and reused across every parent edge that
/// touches the module.
fn module_composite_ports(
    db: &dyn Db,
    sub_model: SourceModel,
    project: SourceProject,
) -> std::collections::BTreeSet<String> {
    let prefix = "$\u{205A}ltm\u{205A}composite\u{205A}";
    crate::db::model_ltm_variables(db, sub_model, project)
        .vars
        .iter()
        .filter_map(|v| v.name.strip_prefix(prefix).map(|p| p.to_string()))
        .collect()
}

/// Equation for a module-involved link score (`from` and/or `to` is a
/// module node in the parent causal graph). Shared verbatim by the
/// `(from, to)`-keyed [`link_score_equation_text`] and the per-shape
/// [`crate::db::link_score_equation_text_shaped`] so the two never drift
/// (the shaped twin's `RefShape` does not change a module link's
/// equation: modules are scalar nodes whose composite-reference /
/// ceteris-paribus / unit-transfer formulas don't reach into the target's
/// AST shape).
///
/// Three cases, each preferring a faithful link score and only falling
/// back to the magnitude-1 [`black_box_unit_transfer_equation`] (NOT the
/// gain) when nothing better exists:
///
/// 1. `variable -> module` and `module -> module`: the edge feeds the
///    target module's input port. When the sub-model exposes a composite
///    for that port, the link score IS that composite
///    (`module·$⁚ltm⁚composite⁚port`) -- the module's internal transfer,
///    exactly the macro treatment (ref §6). When it does not (a
///    passthrough), use the unit transfer against the module's *output*
///    ref (a readable scalar `module·port`), never the bare module name.
///    The composite resolves in BOTH exhaustive and discovery mode (since
///    GH #548 the sub-model's composite var is laid out in the parent's
///    flattened offset map whenever `ltm_enabled`), so the two modes share
///    one branch.
///
/// 2. `module -> variable`: the dependent's equation references the
///    module output via `module·port`, so a real ceteris-paribus partial
///    is available -- prefer it (exact link score). Fall back to the unit
///    transfer against the output ref only if the reference can't be
///    located in the target AST.
pub(crate) fn module_link_score_equation(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    from_name: &str,
    to_name: &str,
    from_var: Option<&crate::variable::Variable>,
    to_var: &crate::variable::Variable,
) -> Option<datamodel::Equation> {
    use crate::common::{Canonical, Ident};

    let from_ident = Ident::<Canonical>::new(from_name);
    let to_ident = Ident::<Canonical>::new(to_name);
    let from_is_module = from_var.is_some_and(|v| v.is_module());
    let to_is_module = to_var.is_module();

    // Resolve a module variable's parent-visible output reference
    // (`module·port`) -- a readable scalar, unlike the bare module name.
    //
    // The `ports.first()` (alphabetically-first parent-read output) choice is
    // arbitrary, but reaching this fallback is now a near-unreachable residual:
    // since PR #684 any module with an input->output pathway exposes a
    // composite (used instead, below), so this unit transfer fires only when
    // the module's output does not depend on its input at all -- a pathway-less
    // module whose link score is moot (it transmits no change around the loop).
    // For a multi-output module that DOES have pathways, the loop's per-link
    // score is fixed exactly by `model_ltm_variables`'s per-exit-port pathway
    // selection, not by this port choice.
    let module_output_ref = |module_name: &str| -> String {
        let edges = model_causal_edges(db, model, project);
        let ports = find_model_output_ports_for_module(db, model, project, edges, module_name);
        let port = ports
            .first()
            .cloned()
            .unwrap_or_else(|| "output".to_string());
        format!("{module_name}\u{00B7}{port}")
    };

    // The composite var name a sub-model emits for `port`, if any.
    //
    // This resolves in BOTH exhaustive and discovery mode: since GH #548,
    // `build_submodel_metadata` lays out a sub-model's LTM synthetic vars
    // (composites included) in the parent's flattened offset map whenever
    // `ltm_enabled`, which holds in both modes. (The pre-#675 code gated
    // composites to exhaustive mode on a now-stale "cross-module refs don't
    // resolve in discovery" assumption; an empirical probe showed the SMOOTH
    // composite resolving to a nonzero value in a discovery-mode run.) A
    // passthrough module emits no composite, so this returns `None` for it
    // and the caller falls back to the unit transfer.
    let composite_ref_for_port = |module_name: &str, port: &str| -> Option<String> {
        let project_models = project.models(db);
        // Resolve the sub-model name. Explicit module variables live in
        // `model.variables`; implicit ones (SMOOTH/DELAY expansions) are
        // not source vars but are recorded in the edges' module->model map
        // -- which is also where stdlib instances resolve from. Consult the
        // edge map first so both kinds are covered.
        let edges = model_causal_edges(db, model, project);
        let sub_model_name = edges
            .dynamic_modules
            .get(module_name)
            .cloned()
            .or_else(|| {
                model
                    .variables(db)
                    .get(module_name)
                    .map(|v| v.model_name(db).to_string())
            })?;
        let sub_model_name = canonicalize(&sub_model_name);
        let sub_model = project_models.get(sub_model_name.as_ref())?;
        if module_composite_ports(db, *sub_model, project).contains(port) {
            Some(format!(
                "{module_name}\u{00B7}$\u{205A}ltm\u{205A}composite\u{205A}{port}"
            ))
        } else {
            None
        }
    };

    let equation = if !from_is_module && to_is_module {
        // variable -> module: the edge feeds one of `to`'s input ports.
        let crate::variable::Variable::Module { inputs, .. } = to_var else {
            return Some(datamodel::Equation::Scalar(
                black_box_unit_transfer_equation(from_name, &module_output_ref(to_name)),
            ));
        };
        match inputs.iter().find(|i| i.src == from_ident) {
            Some(input) => match composite_ref_for_port(to_name, input.dst.as_str()) {
                Some(composite) => format!("\"{composite}\""),
                None => black_box_unit_transfer_equation(from_name, &module_output_ref(to_name)),
            },
            None => black_box_unit_transfer_equation(from_name, &module_output_ref(to_name)),
        }
    } else if from_is_module && to_is_module {
        // module -> module: `from`'s output is wired into `to`'s input
        // port. The edge source matches `to`'s input whose `src` is the
        // module-qualified `from·output`, so match against the normalized
        // module node rather than the bare name.
        let from_output = module_output_ref(from_name);
        let crate::variable::Variable::Module { inputs, .. } = to_var else {
            return Some(datamodel::Equation::Scalar(
                black_box_unit_transfer_equation(&from_output, &module_output_ref(to_name)),
            ));
        };
        let matching_input = inputs
            .iter()
            .find(|i| crate::ltm::normalize_module_ref(&i.src) == from_ident);
        match matching_input.and_then(|input| composite_ref_for_port(to_name, input.dst.as_str())) {
            Some(composite) => format!("\"{composite}\""),
            None => black_box_unit_transfer_equation(&from_output, &module_output_ref(to_name)),
        }
    } else {
        // module -> variable: `to` has a real equation referencing the
        // module output via `module·port`. Prefer a ceteris-paribus
        // partial on that equation (the exact link score); fall back to
        // the unit transfer if the reference can't be located.
        let from_output = to_var
            .ast()
            .map(|ast| crate::variable::identifier_set(ast, &[], None))
            .and_then(|deps| {
                let prefix = format!("{from_name}\u{00B7}");
                deps.into_iter()
                    .find(|d| d.as_str().starts_with(&prefix))
                    .map(|d| d.to_string())
            });
        match from_output {
            Some(output_ref) => {
                let output_ident = Ident::<Canonical>::new(&output_ref);
                let mut all_vars = HashMap::new();
                all_vars.insert(to_ident.clone(), to_var.clone());
                let dim_ctx = project_dimensions_context(db, project);
                match crate::ltm_augment::generate_link_score_equation_for_link(
                    &output_ident,
                    &to_ident,
                    &RefShape::Bare,
                    &[],
                    to_var,
                    &all_vars,
                    Some(dim_ctx),
                ) {
                    Ok(eqn) => return Some(ltm::scalarize_ltm_equation(eqn)),
                    // The target's equation couldn't be parsed for the
                    // partial (GH #311): fall back to the unit transfer
                    // rather than emit a silently non-ceteris-paribus
                    // score. The reference is the located output ref.
                    Err(_) => black_box_unit_transfer_equation(&output_ref, to_name),
                }
            }
            None => black_box_unit_transfer_equation(&module_output_ref(from_name), to_name),
        }
    };

    Some(datamodel::Equation::Scalar(equation))
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

    // Module-involved links: composite reference, ceteris-paribus, or the
    // signed unit-transfer fallback, decided by `module_link_score_equation`
    // (shared with the per-shape twin so the two never drift).
    if from_is_module || to_is_module {
        return module_link_score_equation(
            db,
            model,
            project,
            from_name,
            to_name,
            from_var.as_ref(),
            &to_var,
        )
        .map(|equation| LtmSyntheticVar {
            name: var_name,
            equation,
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
    // A `PartialEquationError` here means the target's equation text could
    // not be parsed for the ceteris-paribus partial (GH #311). Skip the
    // link-score variable and surface a `Warning` instead of emitting a
    // silently non-ceteris-paribus score; `model_ltm_fragment_diagnostics`
    // never sees this case because the bad equation would compile cleanly.
    let equation = match crate::ltm_augment::generate_link_score_equation_for_link(
        &from_ident,
        &to_ident,
        &RefShape::Bare,
        &[],
        &to_var,
        &all_vars,
        None,
    ) {
        Ok(eqn) => eqn,
        Err(err) => {
            emit_ltm_partial_equation_warning(db, model, &var_name, &err);
            return None;
        }
    };

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

// `link_score_equation_text_shaped` lives in `db/ltm/compile.rs` (where
// the emission loop calls it) so this file stays under the project's
// per-file line cap; see `ltm::link_score_equation_text_shaped`.

/// Build a causal graph from pre-computed edges and enumerate all pathways
/// from each input port to the specified output ports (or auto-detect them).
/// Used by `model_ltm_variables` in `db/ltm/mod.rs` for pathway and composite
/// score generation.
///
/// Returns `(pathways, truncated_ports)`; `truncated_ports` (sorted) names the
/// input ports whose internal-pathway enumeration hit the per-port pathway
/// budget (GH #649), so the caller can warn and treat those composite scores
/// as degraded.
fn module_input_pathways_from_edges(
    edges_result: &CausalEdgesResult,
    output_ports: &[crate::common::Ident<crate::common::Canonical>],
) -> crate::ltm::ModulePathwaysWithTruncation {
    let graph = causal_graph_from_edges(edges_result);
    graph.enumerate_pathways_to_outputs_with_truncation(output_ports)
}

/// Build the composite "pathway with the largest absolute score" selection for
/// one module input port: the composite's equation text plus any accumulator
/// helper variables it folds through.
///
/// Every emitted equation is O(1) size, so the TOTAL text is linear in the
/// pathway count. The selection is a left fold: each accumulator holds the
/// running winner (the larger-|x| of the previous accumulator and the next
/// pathway), and the composite is the final fold step. Ties keep the earlier
/// pathway.
///
/// Why the fold is materialized as helper variables instead of one nested
/// expression: a selection step needs its operand twice (`if ABS(a) >= ABS(b)
/// then a else b`), so nesting expressions doubles the text per pathway --
/// O(2^n) bytes. A real Vensim macro module with hundreds of input->output
/// pathways (covid19's SSTATS) exhausted all memory building that string.
/// Folding through variables keeps each step O(1) because the previous step is
/// referenced by NAME, not inlined.
///
/// The accumulators are named `{input path prefix}⁚acc⁚{i:06}` so they sort
/// (a) after the numeric pathway variables they reference (digits < 'a'), and
/// (b) in fold order among themselves (zero-padded index) -- the LTM runlist
/// evaluates fragments in sorted-name order within the "path" category, so
/// this naming is what makes each accumulator's inputs already-evaluated when
/// it runs.
fn generate_max_abs_selection(
    input_port: &str,
    pathway_names: &[String],
) -> (String, Vec<LtmSyntheticVar>) {
    /// One selection step: the larger-|x| of `a` and `b`, ties keeping `a`.
    fn select_step(a: &str, b: &str) -> String {
        format!("if ABS(\"{a}\") >= ABS(\"{b}\") then \"{a}\" else \"{b}\"")
    }

    let acc_name =
        |i: usize| format!("$\u{205A}ltm\u{205A}path\u{205A}{input_port}\u{205A}acc\u{205A}{i:06}");

    match pathway_names {
        [] => ("0".to_string(), vec![]),
        [only] => (format!("\"{only}\""), vec![]),
        [p0, p1] => (select_step(p0, p1), vec![]),
        [p0, p1, rest @ .., last] => {
            // Left fold. `selection` is the running winner's equation; before
            // each fold step it is materialized as an accumulator variable so
            // the step can reference it by name instead of inlining it.
            let mut helpers: Vec<LtmSyntheticVar> = Vec::with_capacity(rest.len() + 1);
            let mut selection = select_step(p0, p1);
            for next in rest {
                let acc = acc_name(helpers.len());
                helpers.push(LtmSyntheticVar {
                    name: acc.clone(),
                    equation: datamodel::Equation::Scalar(selection),
                    dimensions: vec![],
                    compile_directly: false,
                });
                selection = select_step(&acc, next);
            }
            // The composite's own equation is the final fold step against the
            // last pathway, referencing the materialized running winner.
            let final_acc = acc_name(helpers.len());
            helpers.push(LtmSyntheticVar {
                name: final_acc.clone(),
                equation: datamodel::Equation::Scalar(selection),
                dimensions: vec![],
                compile_directly: false,
            });
            (select_step(&final_acc, last), helpers)
        }
    }
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
