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
