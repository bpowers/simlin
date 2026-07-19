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
mod invariance;
pub(crate) use invariance::model_flows_invariant;
// `pub(crate)` (not private-to-`db`) so the Track-A classifier-agreement gate,
// mounted under `crate::ltm_augment`, can reach the production Expr2 walker
// (`collect_all_reference_sites`) and the reference-site IR entry
// (`model_ltm_reference_sites`) it compares the Expr0 partial builder against.
pub(crate) mod ltm_ir;
mod macro_registry;
mod units;
mod var_fragment;

mod diagnostic;
pub(crate) use diagnostic::model_duplicate_variables;
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
    ImplicitVarMeta, ModuleReferenceGraph, ParsedVariableResult, VariableDeps,
    model_implicit_var_info, model_module_ident_context, model_module_map,
    parse_source_variable_with_module_context, project_converted_dimensions,
    project_datamodel_dims, project_dimensions_context, project_module_graph,
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
    LtmImplicitVarMeta, ShapedLinkScore, compile_ltm_var_fragment, link_score_equation_text_shaped,
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
// Test-only: the forced-fragment-failure override (GH #547), so the
// fragment-diagnostics positive tests can exercise the diagnostic pass
// deterministically instead of depending on a real fragment-compile bug
// existing.
#[cfg(test)]
pub(crate) use ltm::LtmFragmentFailureGuard;
// Test-only: the forced-`PartialEquationError` edge override (GH #780), so
// the unscoreable-edge-recording contract can be exercised end-to-end on
// the SHAPED-QUERY and per-element paths, whose own partial-equation
// terminals are unreachable through any compiling model (recovered by the
// changed-last fallback or pinned to concrete elements upstream). The
// agg-half emitters' terminals are live-reachable (the square-source
// duplicate-dim feeder, GH #743) and are tested with a real fixture
// instead; the override + tiny fixture covers the rest (per
// docs/dev/rust.md#test-time-budgets).
#[cfg(test)]
pub(crate) use ltm::ForcePartialEquationErrorGuard;

mod analysis;
pub use analysis::RefShape;
pub use analysis::causal_graph_from_edges;
pub use analysis::causal_graph_from_element_edges;
pub use analysis::causal_graph_from_element_edges_with_modules;
pub(crate) use analysis::reconstruct_model_variables;
// The same-element diagonal/broadcast/mapped projection the element graph
// emits for a `Bare` A2A reference. Discovery's `expand_a2a_link_offsets`
// consumes it (via `crate::db::expand_same_element`) so its per-element
// from-node spelling stays in lockstep with `model_element_causal_edges`
// rather than re-deriving it with a subtly different rule (GH #754).
pub(crate) use analysis::expand_same_element;
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
    /// Salsa input handles for the CONVEYOR/QUEUE-EXPANDED twin of the synced
    /// project. `None` until the first special-stock build; `Some` from then on,
    /// even if the model is later edited into an ordinary one (see below).
    ///
    /// A conveyor/queue stock is not compiled directly. It is *expanded* -- a
    /// pure `datamodel::Project -> datamodel::Project` rewrite into ordinary
    /// stocks/flows/auxes plus a native per-step VM pass -- and it is that
    /// rewritten project which is compiled. The rewrite creates variables, so it
    /// creates salsa *inputs*, which a salsa tracked function may not do: it has
    /// to run on an `&mut self` sync path. This second slot is where the result
    /// lives, so the expanded compile is incremental exactly like every other
    /// compile instead of starting from a cold, throwaway database.
    ///
    /// Two `SourceProject`s in one db roughly doubles the salsa *input* footprint
    /// for a conveyor/queue model. That is the price of incrementality, and it is
    /// bounded at exactly ONE extra input set per db: `sync_expanded` always
    /// re-syncs onto the PRIOR handles, so the set is created once and reused
    /// forever after. An ordinary model never allocates it at all.
    ///
    /// The slot is deliberately never cleared. Salsa 0.26 has no input reclamation,
    /// so dropping these handles would free nothing -- the `SourceProject`/
    /// `SourceModel`/`SourceVariable` inputs and their memos stay in the arena --
    /// while forcing the next expanded sync down the `prev == None` path, which
    /// mints a *second* input set. A conveyor model edited into an ordinary one and
    /// back (or a dry-run patch that removes the belt, which `apply_patch` performs
    /// on every editor keystroke) would allocate a fresh set each round trip. A
    /// stale slot costs nothing instead: nothing reads it without re-syncing first.
    expanded_state: Option<PersistentSyncState>,
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

    /// Sync the conveyor/queue-EXPANDED twin of the project into the db's second
    /// input slot, returning its `SourceProject` handle.
    ///
    /// Threads the PRIOR expanded handles into `sync_from_datamodel_incremental`,
    /// so an unrelated single-variable edit re-syncs one `SourceVariable` field
    /// and leaves every other expanded fragment's salsa memo intact. Passing
    /// `None` there (which is what the old throwaway-db build did) would defeat
    /// the entire purpose.
    ///
    /// INVARIANT -- the reason no rollback bookkeeping is needed: this handle is
    /// the ONLY way to reach the expanded inputs, and the sole caller
    /// (`queue_compile::build_compiled`) re-syncs the caller's current datamodel
    /// through here immediately before reading a single field. A patch that is
    /// staged (`sync_staged`), expanded, and then rejected (`restore`) therefore
    /// cannot leave a poisoned expanded project behind: the next build overwrites
    /// every input field from the restored datamodel before compiling. Keeping
    /// the handles across the rollback is deliberate -- re-creating them would
    /// throw away every expanded fragment memo, which is the cost this slot
    /// exists to avoid.
    pub(crate) fn sync_expanded(&mut self, expanded: &datamodel::Project) -> SourceProject {
        // `take()` for the same borrow reason `sync` takes: the `prev` argument
        // cannot borrow `self.expanded_state` while `self` is borrowed mutably.
        let prev = self.expanded_state.take();
        let new = sync_from_datamodel_incremental(self, expanded, prev.as_ref());
        let sp = new.project;
        self.expanded_state = Some(new);
        sp
    }

    /// The expanded project's `SourceProject`, if the last build expanded one.
    ///
    /// Test-only: production code reaches the expanded inputs exclusively through
    /// `sync_expanded`'s return value, which is what makes the "never read
    /// without a preceding re-sync" invariant above structural rather than
    /// merely conventional.
    #[cfg(test)]
    pub(crate) fn expanded_source_project(&self) -> Option<SourceProject> {
        self.expanded_state.as_ref().map(|s| s.project)
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
/// It is an `IndexMap` (not a `HashMap`) so iteration order is the loops'
/// **emission order** -- the content-derived order `assign_loop_ids` produces
/// and `model_ltm_variables` inserts in (enumerated loops first, then pinned).
/// The post-sim rel-loop-score denominator (`ltm_post::compute_rel_loop_scores*`)
/// sums `|loop_score|` in this order, so preserving emission order keeps that
/// IEEE-754 (non-associative) sum bit-for-bit identical to the pre-#461
/// compile-time emitter, which accumulated in the same `detected_loops` order
/// (GH #468). Emission order is itself deterministic across salsa cache
/// invalidations and across processes because `assign_loop_ids` is a pure
/// function of loop content (it sorts on the canonical edge-sequence rotation,
/// not on `HashMap` enumeration order -- see `ltm::graph::loop_id_sort_key`),
/// so the IndexMap order never flaps even though `IndexMap`'s own `PartialEq`
/// (used for salsa cache equality) is order-insensitive.
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
    pub loop_partitions: indexmap::IndexMap<String, Vec<Option<usize>>>,
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

/// The first `{module}·port` output composite that `to`'s equation reads, in
/// DOCUMENT order (left-to-right over `to`'s reconstructed AST), or `None`
/// when `to` reads no output of `module`.
///
/// This is the deterministic replacement (GH #971) for
/// [`module_link_score_equation`]'s former pick -- an arbitrary
/// `{module}·`-prefixed dependency out of `identifier_set`'s `HashSet`, whose
/// iteration order is per-process random. When `to` reads MORE THAN ONE
/// output of one module instance the choice decides which output's change the
/// link score attributes (the others are frozen), so the random pick made the
/// emitted score -- and every loop score through it -- flap between processes.
/// The reference-site IR already enumerates the module-output composites as
/// `OccurrenceRef::ModuleOutput` occurrences in the walk's stable
/// left-to-right order, so taking the first that names `module` is a
/// reproducible choice over the SAME set the old scan considered.
fn module_output_ref_in_document_order(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    module: &str,
    to_name: &str,
) -> Option<String> {
    let ref_sites = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
    ref_sites
        .occurrences
        .get(to_name)?
        .iter()
        .find_map(|occ| match &occ.reference {
            crate::db::ltm_ir::OccurrenceRef::ModuleOutput {
                module: occ_module,
                composite,
                ..
            } if occ_module == module => Some(composite.clone()),
            _ => None,
        })
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
        //
        // Which output to hold live (GH #971): the reference-site IR
        // enumerates the module-output composites in DOCUMENT order, so take
        // the first that names `from` -- a deterministic pick across
        // processes, unlike the former `identifier_set().find()` over a
        // per-process-random `HashSet`. The `identifier_set` scan is kept
        // only as a defensive fallback for the (unexpected) case where the IR
        // recorded no matching `ModuleOutput` occurrence for `to`, preserving
        // the historical set of edges that receive a real partial.
        let from_output = module_output_ref_in_document_order(
            db, model, project, from_name, to_name,
        )
        .or_else(|| {
            // GH #971 / Track A stage 3 prep: the deterministic IR pick above
            // should name the live output for every module->variable edge that
            // receives a real partial (the occurrence IR enumerates every
            // module-output composite `to` reads, in document order). The
            // per-process-random `identifier_set` scan survives ONLY as a
            // defensive fallback for the (expected-unreachable) case where the
            // IR recorded no `ModuleOutput` occurrence for `to`. Mark LOUDLY
            // whenever the scan actually RESCUES (IR missed, scan found one):
            // stage 3 needs a fired-or-not signal before it can delete the scan
            // with confidence. The rescued ref itself is unchanged -- this only
            // warns; it never alters the emitted score.
            let rescued = to_var
                .ast()
                .map(|ast| crate::variable::identifier_set(ast, &[], None))
                .and_then(|deps| {
                    let prefix = format!("{from_name}\u{00B7}");
                    deps.into_iter()
                        .find(|d| d.as_str().starts_with(&prefix))
                        .map(|d| d.to_string())
                });
            if rescued.is_some() {
                // In test/debug builds the assertion is the loud marker (the
                // scan is meant to be dead, so a fired assert is real signal a
                // module-output occurrence is missing from the IR). In release
                // builds `debug_assert!` is a no-op, so also emit a warning line
                // -- the repo's `eprintln!` idiom for an unexpected internal
                // condition (cf. `model.rs`, `dimensions.rs`).
                debug_assert!(
                    false,
                    "GH #971: module-output IR pick missed `{from_name}\u{00B7}` \
                     in `{to_name}`; the identifier_set fallback rescued it. The \
                     occurrence IR should enumerate every module-output composite \
                     (Track A stage 3 deletes this fallback once it is proven dead)."
                );
                eprintln!(
                    "warning: LTM module-output IR pick missed `{from_name}\u{00B7}` \
                     in `{to_name}`; used the identifier_set fallback (GH #971)."
                );
            }
            rescued
        });
        match from_output {
            Some(output_ref) => {
                let output_ident = Ident::<Canonical>::new(&output_ref);
                let mut all_vars = HashMap::new();
                all_vars.insert(to_ident.clone(), to_var.clone());
                let dim_ctx = project_dimensions_context(db, project);
                // The target's per-occurrence access-shape IR. The live source
                // here is a `module·port` composite (an `OccurrenceRef::ModuleOutput`),
                // so the ceteris-paribus wrap's GH #517 reducer-freeze arm
                // (`subtree_has_live_shape`) must see it to reproduce the historical
                // recursion when the composite is read bare inside a reducer
                // (`to = SUM(arr[*] * module·port)`) -- an empty stream froze the
                // reducer whole, silently converting a would-be loud degradation
                // into a clean-compiling zero. This branch already read
                // `model_ltm_reference_sites` (via `module_output_ref_in_document_order`
                // above), so threading the stream adds no new salsa dependency.
                let ref_sites = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
                let to_occurrences: &[crate::db::ltm_ir::OccurrenceSite] = ref_sites
                    .occurrences
                    .get(to_name)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                match crate::ltm_augment::generate_link_score_equation_for_link(
                    &output_ident,
                    &to_ident,
                    &RefShape::Bare,
                    &[],
                    to_var,
                    &all_vars,
                    Some(dim_ctx),
                    // Module-link partials are scalar-context; the GH #526
                    // other-dep check keeps its permissive legacy collapse.
                    None,
                    to_occurrences,
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
    // The target's per-occurrence access-shape IR -- the SAME single classifier
    // family the shaped twin (`link_score_equation_text_shaped`) threads. It is
    // NOT optional: the ceteris-paribus wrap's GH #517 reducer-freeze arm
    // (`subtree_has_live_shape`) consults the stream unconditionally, so passing
    // an empty stream here (the pre-fix bug) made the wrap freeze a reducer whole
    // where the shaped query recursed into it -- deriving a DIFFERENT partial for
    // the very same scalar Bare score whenever the live source appears bare inside
    // a reducer of the target's equation. Assembly compiles this legacy fragment
    // while `model_ltm_variables` reports the shaped one, so the divergence made
    // the VM simulate an equation that disagreed with the one reported. Threading
    // the real stream single-sources the two.
    let ref_sites = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
    let to_occurrences: &[crate::db::ltm_ir::OccurrenceSite] = ref_sites
        .occurrences
        .get(to_name)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
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
        // Legacy (from, to)-keyed path: no dims context is threaded at
        // all, so the GH #526 other-dep check keeps the permissive
        // collapse here too. (This path is only consumed for a SCALAR target,
        // whose empty iterated-dim space makes the other-dep verdict
        // `NotIterated` regardless of `dep_dims`, so omitting it is sound.)
        None,
        to_occurrences,
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

/// Scope guard: flip a `SourceProject`'s `ltm_enabled` salsa input to a chosen
/// value on construction and unconditionally restore the prior value on drop.
///
/// LTM-specific diagnostics (the auto-flip-to-discovery advisory, the
/// synthetic-fragment compile-failure warnings) only accumulate through
/// `model_all_diagnostics` -> `model_ltm_variables` when `ltm_enabled` is true.
/// A caller that wants to harvest those diagnostics on a db synced with LTM
/// off must transiently re-enable the flag for the
/// [`collect_all_diagnostics`] pass and then restore it -- the `SourceProject`
/// salsa input is shared across every other consumer of the project (patch
/// validation, the analyze surfaces, subsequent compiles), so leaking
/// `ltm_enabled = true` past the harvest would silently change the next
/// consumer's output. Using an RAII guard (rather than an explicit reset line
/// somewhere down the function) makes the restore structurally unmissable, even
/// on an early return or a panic in the middle of the queries.
///
/// Shared by libsimlin's `simlin_project_get_errors` / from-wasm rel-loop FFIs
/// (GH #466) and `simlin-mcp-core`'s `read_model` / `edit_model` diagnostic
/// passes (GH #662), so the transient-enable behaves identically across every
/// diagnostic-collection surface instead of being re-implemented per consumer.
pub struct LtmEnabledGuard<'a> {
    db: &'a mut SimlinDb,
    project: SourceProject,
    restore_to: bool,
}

impl<'a> LtmEnabledGuard<'a> {
    /// Set `project.ltm_enabled` to `desired`, capturing the prior value so
    /// `drop` can restore it.
    pub fn enable(
        db: &'a mut SimlinDb,
        project: SourceProject,
        desired: bool,
    ) -> LtmEnabledGuard<'a> {
        let restore_to = project.ltm_enabled(db);
        set_project_ltm_enabled(db, project, desired);
        LtmEnabledGuard {
            db,
            project,
            restore_to,
        }
    }

    /// Borrow the guarded db for read-only salsa queries during the scope.
    pub fn db(&self) -> &SimlinDb {
        self.db
    }
}

impl<'a> Drop for LtmEnabledGuard<'a> {
    fn drop(&mut self) {
        // Panic-safe: `set_project_ltm_enabled` only mutates the salsa input when
        // the flag actually changed (its inner `if ltm_enabled(db) != value`
        // guard), so a no-op restore (flag already matched) never touches salsa
        // at all. On a valid db handle the setter does not panic.
        set_project_ltm_enabled(self.db, self.project, self.restore_to);
    }
}

/// `queue_compile::validate_overflow_markers_over` applied to one salsa
/// `SourceModel`: the twin of `queue_compile`'s `datamodel::Model` adapter, sharing
/// the identical validation algorithm so the two representations can never drift.
///
/// Iterates `declared_variable_idents` -- the ordered, pre-dedup AS-WRITTEN ident
/// list -- rather than the canonical-keyed `variables` map, whose iteration order is
/// nondeterministic. Declaration order decides WHICH offender a multi-offender model
/// reports, so it must match the `datamodel::Model` adapter's `model.variables`
/// order. Duplicate canonical idents are rejected by `compile_project_incremental`
/// before this runs, so the lookup is one-to-one and a miss can only mean the raw
/// list and the map disagree -- skipped rather than panicked on.
fn validate_model_overflow_markers(db: &SimlinDb, model: SourceModel) -> crate::Result<()> {
    use crate::queue_compile::MarkerVar;

    let vars = model.variables(db);
    // Overwhelmingly the common case: no `<overflow/>` anywhere. This runs on every
    // compile of every model in the project, the ~9 synced stdlib models included,
    // so short-circuit before cloning `declared_variable_idents` and building the
    // projection. Order-independent, so the `HashMap` walk is safe here.
    if !vars.values().any(|sv| sv.compat(db).overflow) {
        return Ok(());
    }
    let marker_vars: Vec<MarkerVar<'_>> = model
        .declared_variable_idents(db)
        .iter()
        .filter_map(|raw| vars.get(canonicalize(raw).as_ref()))
        .map(|sv| {
            let compat = sv.compat(db);
            let kind = sv.kind(db);
            MarkerVar {
                ident: sv.ident(db),
                queue_outflows: (kind == SourceVariableKind::Stock && compat.queue.is_some())
                    .then(|| sv.outflows(db).as_slice()),
                overflow_flow: kind == SourceVariableKind::Flow && compat.overflow,
            }
        })
        .collect();

    match crate::queue_compile::validate_overflow_markers_over(&marker_vars) {
        Ok(()) => Ok(()),
        Err((code, msg)) => Err(crate::common::Error::new(
            crate::common::ErrorKind::Simulation,
            code,
            Some(msg),
        )),
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
    // Two variables whose names canonicalize to the same ident silently
    // collapse into one on the canonical-keyed sync maps (last-in-document-
    // order wins), so the simulation would quietly run a DIFFERENT model than
    // the user wrote (GH #885). Reject the whole project loudly, mirroring
    // the macro-registry gate above: model integrity is project-level, and
    // the diagnostic twin (`emit_duplicate_variable_diagnostics`) carries the
    // identical message through `collect_all_diagnostics`. Models are scanned
    // in sorted canonical-name order and each model's groups are in
    // declaration order, so the reported error is deterministic. The synced
    // stdlib models are included in the scan but are duplicate-free by
    // construction.
    {
        let models = project.models(db);
        let mut model_names: Vec<&String> = models.keys().collect();
        model_names.sort_unstable();
        for name in model_names {
            let model = models[name];
            if let Some((canonical, spellings)) =
                crate::db::diagnostic::model_duplicate_variables(db, model).first()
            {
                return crate::sim_err!(
                    DuplicateVariable,
                    crate::common::duplicate_variable_message(model.name(db), canonical, spellings)
                );
            }
        }
    }
    // A conveyor/queue stock is simulated only through the unified special-stock
    // build path (`queue_compile::build_compiled`), which -- via
    // `conveyor_compile::expand_conveyors` / `queue_compile::expand_queues` --
    // expands each belt/FIFO into hidden auxes + driven flows plus a native VM
    // pass and CLEARS the marker BEFORE this point. A surviving marker means one of two things, told apart
    // by which model the stock lives in:
    //
    //   * MAIN model: the model reached the ordinary compile path un-expanded --
    //     a genuine internal invariant violation (`queue_compile::compile_sim`'s
    //     marker dispatch should have routed it to the special path). Integrating
    //     it as a plain stock would silently mis-simulate, so reject it with the
    //     internal `ConveyorNotExpanded`/`QueueNotExpanded` guard code.
    //   * NON-MAIN model (a module-referenced sub-model, or a model defined but
    //     never instantiated): conveyor/queue support is deliberately main-model
    //     only for now -- the expansion pass never touches a sub-model, so the
    //     marker legitimately survives. This is a user-facing feature limitation,
    //     NOT an engine bug, so reject it with the clear
    //     `ConveyorInSubmodelUnsupported`/`QueueInSubmodelUnsupported` diagnostic
    //     naming the stock and its model. Neither spec writes the limitation down
    //     yet; GH #940 tracks doing so.
    //
    // The scan covers every synced model of the passed `project`, which on the
    // special path is the db's EXPANDED `SourceProject` (the main model's markers
    // are cleared there, the sub-models' are not). The salsa-synced stdlib models
    // (SMOOTH etc.) are in this set but carry no conveyor/queue markers, so they
    // never trip it; a genuine sub-model special stock is caught on EVERY compile
    // surface, since the special path expands only the main model and then funnels
    // its expanded-project compile through this same entry point.
    //
    // Both loops walk in sorted canonical order because `SourceProject::models` and
    // `SourceModel::variables` are `HashMap`s: a project with marked stocks in two
    // sub-models (or two marked stocks in one model) would otherwise name a
    // different offender on each run of the same build. The `ErrorCode` would be
    // stable but the user-facing message would not.
    let main_canon = canonicalize(main_model_name);
    let models = project.models(db);
    let mut model_names: Vec<&String> = models.keys().collect();
    model_names.sort_unstable();
    for model_name in model_names {
        let source_model = models[model_name];
        let in_main = canonicalize(source_model.name(db)) == main_canon;
        let vars = source_model.variables(db);
        let mut var_names: Vec<&String> = vars.keys().collect();
        var_names.sort_unstable();
        for var_name in var_names {
            let source_var = vars[var_name];
            if source_var.compat(db).conveyor.is_some() {
                if in_main {
                    return crate::sim_err!(
                        ConveyorNotExpanded,
                        format!(
                            "internal error: conveyor stock '{}' reached the ordinary compile \
                             path un-expanded; every backend routes a conveyor model through \
                             the special-stock dispatch (queue_compile::compile_sim), so this \
                             is an engine bug, not a model or backend limitation",
                            source_var.ident(db)
                        )
                    );
                }
                return crate::sim_err!(
                    ConveyorInSubmodelUnsupported,
                    format!(
                        "conveyor stock '{}' is defined in model '{}', but conveyors are \
                         currently supported only in the main model, not in a sub-model or \
                         module; move the conveyor into the main model to simulate it",
                        source_var.ident(db),
                        source_model.name(db)
                    )
                );
            }
            if source_var.compat(db).queue.is_some() {
                if in_main {
                    return crate::sim_err!(
                        QueueNotExpanded,
                        format!(
                            "internal error: queue stock '{}' reached the ordinary compile path \
                             un-expanded; every backend routes a queue model through the \
                             special-stock dispatch (queue_compile::compile_sim), so this is an \
                             engine bug, not a model or backend limitation",
                            source_var.ident(db)
                        )
                    );
                }
                return crate::sim_err!(
                    QueueInSubmodelUnsupported,
                    format!(
                        "queue stock '{}' is defined in model '{}', but queues are currently \
                         supported only in the main model, not in a sub-model or module; move \
                         the queue into the main model to simulate it",
                        source_var.ident(db),
                        source_model.name(db)
                    )
                );
            }
        }
    }
    // `<overflow/>` marker placement (docs/design/queues.md §3.3, §10.7). The check
    // lives here, not in `queue_compile`'s dispatch, because `assemble_simulation`
    // has exactly ONE production caller -- the line below -- so nothing compiles
    // without passing this gate. The dispatch is not such a chokepoint: `wasmgen`
    // reaches `compile_project_incremental` directly, so validating in the dispatch
    // would leave the wasm backend accepting what the VM rejects. And the marker
    // rides on a FLOW, so neither dispatch predicate (both scan for a marked STOCK)
    // can even see it: a model may carry a stray overflow with no queue anywhere and
    // take the ordinary branch.
    //
    // Placing it AFTER the marker guard above selects WHICH of two errors a
    // sub-model queue reports; it is not what makes the check sound. Expansion never
    // touches a sub-model, so a sub-model queue stock still carries `compat.queue`
    // at this point -- but the guard has already claimed that model, so the marker is
    // never examined and a sub-model queue reports `QueueInSubmodelUnsupported`
    // whatever its overflow placement. Run this gate FIRST instead and one case
    // changes: a sub-model queue whose overflow sits on its FIRST outflow would
    // report the less actionable `QueueOverflowNotOnQueue`. Nothing invalid is
    // accepted either way.
    //
    // What DOES reach this validator is therefore a project in which no model holds a
    // queue marker at all: the main model's were either cleared by expansion or would
    // have tripped the guard's main-model arm, and any sub-model's would have tripped
    // its sub-model arm. Every surviving overflow flag thus
    // names a flow that is not -- and, sub-model queues being unsupported, cannot be
    // -- a queue outflow, so rejecting it is right for sub-models too, not a silent
    // skip. The per-model outflow set the validator builds is consequently always
    // empty today; it is computed rather than assumed so that lifting the guard's
    // sub-model arm (a real sub-model-queue feature) makes legal non-first overflows
    // start being accepted here with no change to this code.
    //
    // The expanded project reaches here with its main-model overflow markers
    // already cleared by `expand_queues` (it clears every driven outflow, and its
    // own pre-expansion validation guarantees each overflow flag WAS one), so a
    // valid special-stock model passes. That pre-expansion check is not redundant
    // with this one: it is the only place the "never the FIRST outflow" rule can be
    // enforced, since expansion erases the evidence.
    //
    // Models are scanned in sorted canonical-name order so a multi-model project
    // reports deterministically, mirroring the duplicate-variable gate above.
    {
        let models = project.models(db);
        let mut model_names: Vec<&String> = models.keys().collect();
        model_names.sort_unstable();
        for name in model_names {
            validate_model_overflow_markers(db, models[name])?;
        }
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
mod incremental_compile_tests;
#[cfg(test)]
mod ltm_char_tests;
#[cfg(test)]
mod ltm_module_tests;
#[cfg(test)]
mod ltm_unified_tests;
#[cfg(test)]
mod module_cycle_tests;
#[cfg(test)]
mod module_wiring_tests;
#[cfg(test)]
mod prev_init_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod vm_verification_tests;
