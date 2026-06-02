// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM (Loops That Matter) variable generation.
//!
//! This is the orchestration root of the LTM subtree. `model_ltm_variables`
//! (the salsa-tracked entry point) drives loop enumeration and link-score
//! emission for a model's synthetic LTM variables; the per-concern work
//! lives in submodules:
//!
//! * `parse` -- parsing + `datamodel::Equation`-shaping helpers.
//! * `compile` -- per-equation compilation to symbolic bytecodes, the
//!   per-shape link-score equation-text query, and the compile-failure
//!   diagnostic pass.
//! * `loops` -- output-port discovery + the tiered loop builders +
//!   cross-element-through-aggregate loop recovery.
//! * `link_scores` -- the per-edge / per-shape / through-aggregate link-score
//!   emitters (lifted verbatim out of `model_ltm_variables`).

use std::collections::{HashMap, HashSet};

use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::datamodel;
use crate::ltm::strip_subscript;

use super::{
    Db, SourceModel, SourceProject, SourceVariableKind, compute_layout, model_implicit_var_info,
    project_datamodel_dims, project_units_context,
};

mod compile;
mod link_scores;
mod loops;
mod parse;
mod pinned;

// Re-export the LTM surface other `db` submodules (and the `db.rs` root's
// `use ltm::*` / `pub use ltm::{...}` blocks) reach. The directory keeps the
// internal helpers `pub(super)` within the `ltm` subtree; only the names that
// escape it are widened here.
// `compile_ltm_var_fragment` / `link_score_equation_text_shaped` keep the `pub`
// surface the `db.rs` root re-exports with `pub use ltm::{...}`.
pub(crate) use compile::{
    compile_ltm_implicit_var_fragment, compile_ltm_synthetic_fragment,
    model_ltm_fragment_diagnostics,
};
pub use compile::{compile_ltm_var_fragment, link_score_equation_text_shaped};
pub(crate) use link_scores::emit_ltm_partial_equation_warning;
#[cfg(test)]
pub(crate) use link_scores::ltm_partial_equation_warning_message;
pub(crate) use loops::build_loops_from_tiered;
pub(crate) use parse::scalarize_ltm_equation;
pub(crate) use pinned::model_pinned_loops;

// Test-only re-exports. These names are consumed solely by the LTM test
// modules -- `ltm_tests` (mounted here, reached via `super::`) and the
// db-root-mounted `ltm_unified_tests` (reached through `db.rs`'s `use ltm::*`
// glob) -- so they would warn as unused in a non-test lib build.
#[cfg(test)]
pub(crate) use compile::compile_ltm_equation_fragment;
#[cfg(test)]
pub(crate) use loops::{
    AggLoopBudgetGuard, MAX_CROSS_AGG_LOOPS, build_element_level_loops, cyclic_orderings,
};

// Bare names used by `model_ltm_variables`'s body (kept call-site-identical
// after lifting the link-score emitters and moving loop/parse helpers out).
use link_scores::{
    emit_agg_to_target_link_scores, emit_link_scores_for_edge, emit_source_to_agg_link_scores,
};
use loops::{cross_agg_loop_budget, find_model_output_ports, recover_agg_hop_polarities};
use parse::parse_ltm_equation;

/// The model's full variable-name set, for the LTM equation parse path.
///
/// Threaded into `parse_ltm_equation` so PREVIOUS/INIT can accept a
/// non-shadowed bare element name as a static subscript index instead of
/// synthesizing a helper aux per call site (GH #654: C-LEARN's generated LTM
/// equations carry ~24k such call sites). Salsa-tracked because the LTM
/// fragment compilers parse equations once per synthetic variable -- tens of
/// thousands of times on large models -- and rebuilding a several-thousand-
/// entry set per parse would be pure waste.
///
/// Every LTM parse site MUST pass the same set: `model_ltm_implicit_var_info`
/// (which decides which helpers exist and get layout slots) and the fragment
/// compilers / `assemble_module` (which compile them) have to agree on
/// whether a given PREVIOUS argument synthesizes a helper.
#[salsa::tracked(returns(ref))]
pub(super) fn ltm_model_var_names(
    db: &dyn Db,
    model: SourceModel,
    _project: SourceProject,
) -> HashSet<Ident<Canonical>> {
    model
        .variables(db)
        .keys()
        .map(|name| Ident::new(name))
        .collect()
}

/// Salsa-tracked: the LTM fragment compilers consult this once per synthetic
/// variable (tens of thousands of times on large models), and rebuilding the
/// set from every source variable per call was a measurable fraction of LTM
/// compile time (GH #655).
#[salsa::tracked(returns(ref))]
pub(super) fn ltm_module_idents(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashSet<Ident<Canonical>> {
    let source_vars = model.variables(db);
    let mut module_idents: HashSet<Ident<Canonical>> = source_vars
        .iter()
        .filter_map(|(name, source_var)| {
            if source_var.kind(db) == SourceVariableKind::Module {
                Some(Ident::new(name))
            } else {
                None
            }
        })
        .collect();

    for (name, meta) in model_implicit_var_info(db, model, project).iter() {
        if meta.is_module {
            module_idents.insert(Ident::new(name));
        }
    }

    module_idents
}

/// The canonical cyclic rotation of a loop's **variable-level** node
/// sequence -- the dedup key for matching a pinned loop against an enumerated
/// one.
///
/// Loop links may carry element-level subscripts (`"pop[nyc]"`) on either end;
/// stripping them collapses to the variable cycle, so a pinned loop named at
/// variable granularity matches the enumerated loop over the same variables
/// regardless of which element the enumerator happened to visit.
/// `canonical_rotation` makes the comparison rotation-invariant (a loop has no
/// distinguished start), so `[a, b, c]` and `[b, c, a]` produce the same key.
fn canonical_variable_rotation(l: &crate::ltm::Loop) -> Vec<String> {
    let seq: Vec<String> = l
        .links
        .iter()
        .map(|link| strip_subscript(link.from.as_str()).to_string())
        .collect();
    crate::ltm::canonical_rotation(&seq)
}

/// Extract the `(from, to)` variable-level endpoints of a link-score var name,
/// or `None` for any other synthetic var.
///
/// A link-score var is named `$⁚ltm⁚link_score⁚{from}→{to}` where `{from}` may
/// carry a `[...]` FixedIndex subscript; both ends collapse to the
/// variable-level edge so the pin emitter can tell which `(from, to)` causal
/// edges already have a link score.
fn link_score_edge_endpoints(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("$\u{205A}ltm\u{205A}link_score\u{205A}")?;
    let (from, to) = rest.split_once('\u{2192}')?;
    Some((
        strip_subscript(from).to_string(),
        strip_subscript(to).to_string(),
    ))
}

/// Metadata about implicit variables generated by LTM equation parsing.
///
/// LTM equations may synthesize helper auxes for intrinsic PREVIOUS/INIT
/// routing and may also expand stdlib module calls such as SMOOTH/DELAY.
/// This structure collects those implicit variables across all LTM
/// equations in a model so that `compute_layout` can allocate slots and
/// `assemble_module` can compile them.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct LtmImplicitVarMeta {
    /// Canonical name of the LTM variable that created this implicit var
    pub ltm_parent_name: String,
    /// Index into the parent's implicit_vars list
    pub index_in_parent: usize,
    /// Whether this implicit var is a stock
    pub is_stock: bool,
    /// Whether this implicit var is a module
    pub is_module: bool,
    /// Sub-model name if is_module is true
    pub model_name: Option<String>,
    /// Size in slots (for scalar vars: 1; for modules: sub-model n_slots)
    pub size: usize,
    /// The implicit variable itself, exactly as LTM equation parsing
    /// synthesized it. Carrying it here means downstream consumers
    /// (`assemble_module`'s LTM-implicit compile loop, the implicit fragment
    /// compiler, module-instance enumeration) read it directly instead of
    /// re-parsing the parent LTM equation -- which previously happened 2-3
    /// times per synthetic variable and was a measurable fraction of LTM
    /// compile time on large models (GH #655).
    pub variable: datamodel::Variable,
}

/// Cached implicit variable info for all LTM synthetic variables.
///
/// Parses each LTM equation to discover implicit helper/module variables,
/// caching the results. Both `compute_layout` and `assemble_module` read
/// this to allocate slots and compile fragments for those implicit vars
/// within LTM equations.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_implicit_var_info(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<String, LtmImplicitVarMeta> {
    if !project.ltm_enabled(db) {
        return HashMap::new();
    }

    let ltm_vars = model_ltm_variables(db, model, project);

    let dims = project_datamodel_dims(db, project);
    let units_ctx = project_units_context(db, project);
    let module_idents = ltm_module_idents(db, model, project);
    let model_var_names = ltm_model_var_names(db, model, project);

    let mut result = HashMap::new();

    for ltm_var in &ltm_vars.vars {
        let parsed = parse_ltm_equation(
            &ltm_var.name,
            &ltm_var.equation,
            dims,
            units_ctx,
            Some(module_idents),
            Some(model_var_names),
        );

        let project_models = project.models(db);

        for (idx, implicit_dm_var) in parsed.implicit_vars.iter().enumerate() {
            let im_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
            let is_module = matches!(implicit_dm_var, datamodel::Variable::Module(_));
            let is_stock = matches!(implicit_dm_var, datamodel::Variable::Stock(_));
            let model_name = if let datamodel::Variable::Module(m) = implicit_dm_var {
                Some(m.model_name.clone())
            } else {
                None
            };
            let size = if is_module {
                model_name
                    .as_deref()
                    .and_then(|mn| {
                        let sub_canonical = canonicalize(mn);
                        project_models
                            .get(sub_canonical.as_ref())
                            .map(|sm| compute_layout(db, *sm, project).n_slots)
                    })
                    .unwrap_or(1)
            } else {
                1
            };

            result.insert(
                im_name,
                LtmImplicitVarMeta {
                    ltm_parent_name: ltm_var.name.clone(),
                    index_in_parent: idx,
                    is_stock,
                    is_module,
                    model_name,
                    size,
                    variable: implicit_dm_var.clone(),
                },
            );
        }
    }

    result
}

/// The module-typed projection of [`model_ltm_implicit_var_info`]: each
/// module-typed LTM implicit variable's canonical name mapped to its
/// sub-model name.
///
/// `compile_ltm_implicit_var_fragment` runs once per LTM implicit variable,
/// and a large arrayed model produces hundreds of thousands of those
/// (C-LEARN v77: ~145k PREVIOUS-helper auxes). Each run merges the
/// module-typed refs into its compilation context so cross-references
/// between module-typed implicit vars resolve -- but scanning the full
/// implicit-var map inside every run made LTM compilation O(K^2) in the
/// implicit-var count (tens of seconds of pure HashMap iteration on
/// C-LEARN). This query computes the projection once.
///
/// In the current architecture LTM equations are generated from
/// post-module-expansion ASTs and never contain module-function calls, so
/// this map is empty in practice; it exists so that if that ever changes,
/// the cross-reference resolution keeps working.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_implicit_module_refs(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<Ident<Canonical>, Ident<Canonical>> {
    let info = model_ltm_implicit_var_info(db, model, project);
    info.iter()
        .filter(|(_, meta)| meta.is_module)
        .filter_map(|(name, meta)| {
            meta.model_name
                .as_ref()
                .map(|mn| (Ident::new(name), Ident::new(mn.as_str())))
        })
        .collect()
}

/// Name -> first-occurrence-index lookup into [`model_ltm_variables`]'s
/// `vars` list, mirroring `vars.iter().find(|v| v.name == name)` semantics.
///
/// Fragment compilation resolves dependencies that may themselves be LTM
/// synthetic variables (an A2A loop score referencing link scores, a
/// composite referencing pathway scores). A linear scan over all LTM vars
/// per dependency lookup is O(N) per lookup and O(N^2) across a model's
/// full LTM compile (C-LEARN: ~145k dependency lookups over 6.7k vars), so
/// the index is built once and salsa-cached.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_var_name_index(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<String, usize> {
    let ltm_vars = model_ltm_variables(db, model, project);
    let mut index: HashMap<String, usize> = HashMap::with_capacity(ltm_vars.vars.len());
    for (i, v) in ltm_vars.vars.iter().enumerate() {
        // First occurrence wins, matching `.find()` on the vars list.
        index.entry(v.name.clone()).or_insert(i);
    }
    index
}

/// Resolve a model's loop-enumeration mode (exhaustive vs. discovery).
///
/// This is the single source of truth for the discovery gate, consulted by
/// both `model_ltm_variables` (which emits LTM synthetic vars + the auto-flip
/// `Warning`s) and `model_detected_loops` (the FFI loop surface). Factoring it
/// out keeps the two query surfaces from disagreeing: before this query,
/// `model_detected_loops` gated solely on the `causal_graph_with_modules` SCC
/// size and ignored both the user-requested `ltm_discovery_mode` flag and the
/// slow-path late-flip, so a small model with `ltm_discovery_mode` forced true
/// + a pinned loop would have `model_ltm_variables` emit only the pin's
/// `loop_score` while `model_detected_loops` ran full enumeration and dropped
/// the pin as a duplicate.
///
/// A model is in discovery mode when ANY of these holds (mirroring the inline
/// resolution in `model_ltm_variables`, in the same short-circuit order so the
/// expensive tiered enumeration is never run on a graph the cheap gates
/// already rejected):
///   1. the caller requested discovery (`project.ltm_discovery_mode`), or
///   2. the variable-level causal graph's largest SCC exceeds
///      `MAX_LTM_SCC_NODES` (the cheap early gate; variable-level Johnson at
///      that scale would enumerate millions of circuits), or
///   3. (only when 1 and 2 are false) the cross-element / mixed slow-path
///      subgraph's largest SCC exceeds `MAX_LTM_SCC_NODES` (the late flip).
///
/// A stock-free model has no feedback loops, so no flip can occur: it is
/// always `Exhaustive`, matching `model_ltm_variables`'s stock-free early
/// return. This query intentionally accumulates NO diagnostics -- the
/// auto-flip `Warning`s stay in `model_ltm_variables` so they are emitted once
/// (a `returns(ref)` tracked query read by multiple callers would otherwise
/// double-accumulate). `model_ltm_variables` sets its returned `mode` from
/// this query, so the two never drift.
#[salsa::tracked]
pub fn model_ltm_mode(db: &dyn Db, model: SourceModel, project: SourceProject) -> super::LtmMode {
    use super::{LtmMode, causal_graph_from_edges, model_causal_edges, model_loop_circuits_tiered};

    let edges_result = model_causal_edges(db, model, project);
    if edges_result.stocks.is_empty() {
        return LtmMode::Exhaustive;
    }

    if project.ltm_discovery_mode(db) {
        return LtmMode::Discovery;
    }

    // Cheap early gate: the variable-level SCC. Running variable-level Johnson
    // (inside `model_loop_circuits_tiered`) on an SCC this large is exactly the
    // explosion the gate avoids, so it must be checked BEFORE the slow-path
    // tier is ever consulted.
    let var_scc_size = causal_graph_from_edges(edges_result).largest_scc_size();
    if var_scc_size > crate::ltm::MAX_LTM_SCC_NODES {
        return LtmMode::Discovery;
    }

    // Late flip: the variable-level SCC cleared the early gate, but the
    // cross-element / mixed slow-path subgraph blew past the threshold. The
    // tiered enumerator exposes that SCC without having run Johnson on it.
    let tiered = model_loop_circuits_tiered(db, model, project);
    if tiered.slow_path_largest_scc > crate::ltm::MAX_LTM_SCC_NODES {
        LtmMode::Discovery
    } else {
        LtmMode::Exhaustive
    }
}

/// Unified LTM variable generation for any model (root or sub-model).
///
/// Auto-detects sub-model behavior by checking for input ports with causal
/// pathways to output. Sub-models and discovery mode generate link scores
/// for ALL edges; exhaustive mode on root models generates link scores only
/// for edges in detected loops, plus loop/relative loop scores.
///
/// Pathway and composite scores are generated for models with input ports.
/// Module-containing loops are no longer filtered out because
/// `link_score_equation_text` now handles module links via composite refs.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> super::LtmVariablesResult {
    use crate::common::Ident;
    use crate::ltm::{CyclePartitions, Loop};
    use salsa::Accumulator;
    use std::collections::HashSet;

    use super::{
        CompilationDiagnostic, Diagnostic, DiagnosticError, DiagnosticSeverity, LtmSyntheticVar,
        LtmVariablesResult, causal_graph_from_edges, causal_graph_with_modules,
        generate_max_abs_selection, model_causal_edges, model_element_cycle_partitions,
        model_loop_circuits_tiered, model_pinned_loops, module_input_pathways_from_edges,
    };

    use super::LtmMode;

    let edges_result = model_causal_edges(db, model, project);
    if edges_result.stocks.is_empty() {
        // A stock-free model has no feedback loops to enumerate, so no
        // mode flip can occur; report the exhaustive default.
        return LtmVariablesResult {
            vars: vec![],
            loop_partitions: HashMap::new(),
            agg_recovery_truncated: false,
            mode: LtmMode::Exhaustive,
        };
    }

    // When the user explicitly requested discovery mode, honor it
    // directly. Otherwise auto-flip if either:
    //   1. The variable-level SCC exceeds `MAX_LTM_SCC_NODES`, or
    //   2. The slow-path (cross-element / mixed) element-level
    //      subgraph's SCC exceeds `MAX_LTM_SCC_NODES` after the tiered
    //      enumerator classifies cycles.
    //
    // Gating on the *variable-level* SCC (instead of the full
    // element-graph SCC) is the structural change that motivates this
    // PR: pure-A2A models with tens of variables over hundreds of
    // elements have a huge element-graph SCC but a small variable
    // SCC, and the new tiered enumerator produces no element-level
    // circuits for them. Today's gate fires anyway on the inflated
    // element-graph SCC, dropping these models into discovery mode
    // unnecessarily.
    //
    // The variable-level SCC check (Tarjan, O(V+E)) runs first so
    // models like WRLD3 (var SCC = 166) auto-flip without paying for
    // variable-level Johnson. Only models that pass that check pay
    // for the tiered enumerator, which is itself bounded: variable
    // Johnson sees at most `MAX_LTM_SCC_NODES` nodes, and slow-path
    // Johnson is skipped internally when the slow-path subgraph SCC
    // exceeds the threshold.
    //
    // Cliff A (~17 GB in legacy `build_element_level_loops` on
    // WRLD3-scale models) was tied to enumerating the full element
    // graph; the tiered path skips that entirely on pure-scalar /
    // pure-A2A models. Cliff B (rel_loop_score equation text) was
    // retired earlier when post-simulation normalization moved out of
    // the VM. See `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`
    // and `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`.
    let is_discovery_user = project.ltm_discovery_mode(db);
    let var_scc_size = if is_discovery_user {
        0
    } else {
        let edges = model_causal_edges(db, model, project);
        causal_graph_from_edges(edges).largest_scc_size()
    };
    let var_auto_flipped = !is_discovery_user && var_scc_size > crate::ltm::MAX_LTM_SCC_NODES;

    if var_auto_flipped {
        let msg = format!(
            "LTM analysis auto-switched from exhaustive to discovery mode: \
             the variable-level causal graph's largest SCC has {} nodes, \
             exceeding MAX_LTM_SCC_NODES = {}.  Variable-level Johnson at \
             this scale would enumerate millions of circuits (see \
             docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md and \
             docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md). \
             Per-loop scores are ranked post-simulation via the \
             strongest-path search; see \
             docs/design/ltm--loops-that-matter.md for the two-tier \
             strategy.",
            var_scc_size,
            crate::ltm::MAX_LTM_SCC_NODES,
        );
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: None,
            error: DiagnosticError::Assembly(msg),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }
    let mut is_discovery = is_discovery_user || var_auto_flipped;

    // Determine output ports for this model. Stdlib models always use
    // the "output" convention. For user-defined models, output ports are
    // determined by which internal variables are referenced from parent
    // models via module·var syntax.
    let model_name_str = model.name(db);
    let output_ports = if model_name_str.starts_with("stdlib\u{205A}") {
        vec![Ident::new("output")]
    } else {
        find_model_output_ports(db, model, project)
    };
    let pathways = if output_ports.is_empty() {
        HashMap::new()
    } else {
        module_input_pathways_from_edges(edges_result, &output_ports)
    };
    let has_input_ports = !pathways.is_empty();

    let mut vars = Vec::new();

    // Fetch source variables and dimension metadata early -- needed by
    // both loop pre-computation (for dimension lookups) and link score
    // classification.
    let source_vars = model.variables(db);
    let dm_dims = project_datamodel_dims(db, project);

    // Phase 5: emit the synthetic aggregate-node auxiliaries
    // (`$⁚ltm⁚agg⁚{n}`). Each is a plain computed aux whose equation is one
    // maximal inlined reducer subexpression; the simulation evaluates it
    // each timestep (so `PREVIOUS(agg)` is available) and the two
    // link-score halves (`source[d] → agg`, `agg → target`) reference it.
    // A whole-RHS reducer (`total_population = SUM(population[*])`) mints
    // *no* synthetic -- the variable already is the aggregate node. The agg
    // vars are pushed before any link-score var so they sort first (the LTM
    // flow fragments are appended to the runlist in `vars` order, and the
    // `agg → target` link score reads the agg's current-step value -- so the
    // agg fragment must execute first in the same timestep). The category
    // function in the final `vars.sort_by` keeps them ahead of link scores.
    let agg_nodes = crate::ltm_agg::enumerate_agg_nodes(db, model, project);
    for agg in &agg_nodes.aggs {
        if !agg.is_synthetic {
            continue;
        }
        // The equation text is the canonical reducer subexpression. A
        // whole-extent or pinned-slice reducer (`SUM(pop[*])`,
        // `SUM(pop[NYC,*])`) has a scalar result; a partial-reduce slice over
        // an iterated dimension (`SUM(matrix[D1,*])` inside an A2A-over-`D1`
        // body) has `result_dims = [D1]` -- in an A2A-over-`D1` body
        // `matrix[D1,*]` is exactly "the `D1`-th row, all of axis 2", so this
        // evaluates correctly as the `Equation::ApplyToAll` body.
        let equation = if agg.result_dims.is_empty() {
            datamodel::Equation::Scalar(agg.equation_text.clone())
        } else {
            datamodel::Equation::ApplyToAll(agg.result_dims.clone(), agg.equation_text.clone())
        };
        vars.push(LtmSyntheticVar {
            name: agg.name.clone(),
            equation,
            dimensions: agg.result_dims.clone(),
            compile_directly: false,
        });
    }

    // Pre-compute loops for exhaustive mode using tiered loop
    // enumeration: variable-level Johnson runs first, and only the
    // cross-element / mixed slice descends into element-level Johnson
    // on the slow-path subgraph. Pure-A2A and pure-scalar cycles emit
    // a single Loop directly (one per variable-level cycle, with
    // `dimensions` set on the A2A case).
    //
    // See `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`
    // for the architecture; the legacy element-graph Johnson path is
    // still available via `model_element_loop_circuits` for callers
    // outside this function.
    //
    // We also need the variable-level graph for polarity analysis
    // (`build_loops_from_tiered` calls `var_graph.calculate_polarity`
    // and `var_graph.find_stocks_in_loop`); the element-level graph
    // produced by `model_element_causal_edges` carries no variable
    // data.
    //
    // `agg_recovery_truncated` rides on the result so a model author
    // knows the cross-agg loop list is incomplete even if the `Warning`
    // never reaches them (#466); set inside the loop-building branch below.
    let mut agg_recovery_truncated = false;
    let loops: Option<Vec<Loop>> = if !is_discovery {
        let tiered = model_loop_circuits_tiered(db, model, project);
        // Late-stage auto-flip: the variable-level SCC was small enough
        // to clear the early gate, but the cross-element / mixed
        // subgraph (the slow path) blew past the threshold. The tiered
        // enumerator already skipped Johnson on the slow path in that
        // case (`slow_path` is empty); we just need to flip the
        // is_discovery flag so link-score generation, etc. follow the
        // discovery path.
        if tiered.slow_path_largest_scc > crate::ltm::MAX_LTM_SCC_NODES {
            let msg = format!(
                "LTM analysis auto-switched from exhaustive to discovery mode: \
                 the cross-element / mixed slow-path subgraph's largest SCC has {} nodes, \
                 exceeding MAX_LTM_SCC_NODES = {}.  Per-loop scores are ranked \
                 post-simulation via the strongest-path search; see \
                 docs/design/ltm--loops-that-matter.md for the two-tier strategy.",
                tiered.slow_path_largest_scc,
                crate::ltm::MAX_LTM_SCC_NODES,
            );
            CompilationDiagnostic(Diagnostic {
                model: model.name(db).clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg),
                severity: DiagnosticSeverity::Warning,
            })
            .accumulate(db);
            is_discovery = true;
            None
        } else if tiered.fast_path.is_empty() && tiered.slow_path.is_empty() {
            if !has_input_ports {
                // No loops and no input ports: nothing to score. `is_discovery`
                // was never flipped on this branch, so report exhaustive.
                return LtmVariablesResult {
                    vars: vec![],
                    loop_partitions: HashMap::new(),
                    agg_recovery_truncated: false,
                    mode: LtmMode::Exhaustive,
                };
            }
            None
        } else {
            // Bind the budget once: `build_loops_from_tiered` enforces it and
            // the `Warning` text below reports it, and a `#[cfg(test)]`
            // override could in principle change between two reads.
            let cross_agg_budget = cross_agg_loop_budget();
            let var_graph = causal_graph_with_modules(db, model, project);
            let (mut detected, truncated_aggs) = build_loops_from_tiered(
                tiered,
                &var_graph,
                source_vars,
                db,
                project,
                dm_dims,
                cross_agg_budget,
            );
            // Surface a truncated cross-element-through-aggregate loop
            // recovery the same way the auto-flip-to-discovery gate surfaces
            // its mode change: a `Warning` (the human channel) plus the
            // robust `agg_recovery_truncated` flag on the result. The loop
            // list is incomplete -- the budget clipped which disjoint-petal
            // combinations were materialized. `truncated_aggs` names exactly
            // the aggregate nodes whose enumeration was clipped (sorted), so
            // the message points the author at the affected reducers rather
            // than every synthetic agg in the model.
            if !truncated_aggs.is_empty() {
                let msg = format!(
                    "LTM cross-element-through-aggregate loop recovery was truncated: a \
                     reducer in a feedback loop produces more disjoint-petal combinations \
                     than the loop budget ({}); the recovered loop list is incomplete. \
                     Affected aggregate node(s): {}.",
                    cross_agg_budget,
                    truncated_aggs.join(", "),
                );
                CompilationDiagnostic(Diagnostic {
                    model: model.name(db).clone(),
                    variable: None,
                    error: DiagnosticError::Assembly(msg),
                    severity: DiagnosticSeverity::Warning,
                })
                .accumulate(db);
            }
            agg_recovery_truncated = !truncated_aggs.is_empty();
            // GH #516: hops into/out of synthetic `$⁚ltm⁚agg⁚{n}` nodes come
            // back Unknown-polarity from the loop builders (the variable-level
            // graph has no agg node); recover the derivable cases here so
            // agg-traversing loops aren't all forced to Undetermined.
            recover_agg_hop_polarities(&mut detected, &var_graph, db, model, project);
            Some(detected)
        }
    } else {
        None
    };

    let mut loop_partitions: HashMap<String, Vec<Option<usize>>> = HashMap::new();

    // Part 1: Link scores.
    // Sub-models and discovery mode need scores for ALL edges (pathways
    // reference arbitrary edges). Exhaustive root models only need
    // scores for edges that participate in loops.
    //
    // For each link score, classify the edge to determine whether the
    // score should be arrayed (A2A). When the target variable has
    // dimensions and either the source is scalar or shares the same
    // dimensions, the link score inherits the target's dimensions so
    // that per-element scores are computed via the A2A expansion.

    if has_input_ports || is_discovery {
        for (from, tos) in &edges_result.edges {
            for to in tos {
                emit_link_scores_for_edge(
                    db,
                    source_vars,
                    agg_nodes,
                    from,
                    to,
                    model,
                    project,
                    dm_dims,
                    /* skip_agg_halves = */ false,
                    &mut vars,
                );
            }
        }
    } else if let Some(ref detected_loops) = loops {
        // Helper: look up the `AggNode` for a synthetic-agg node name.
        let agg_by_name = |name: &str| -> Option<&crate::ltm_agg::AggNode> {
            if crate::ltm_agg::is_synthetic_agg_name(name) {
                agg_nodes.aggs.iter().find(|a| a.name == name)
            } else {
                None
            }
        };
        let mut seen_links: HashSet<(String, String)> = HashSet::new();
        for loop_item in detected_loops {
            for link in &loop_item.links {
                // Loop links can carry element-level subscripts on either
                // end -- `link.from` for a per-source-element FixedIndex /
                // cross-dimensional edge ("pop[nyc]"), an agg-routed source
                // element, or (for cross-element loops) `link.to` for an A2A
                // target visited at a single element. The link-score helpers
                // and `source_vars` / `agg_nodes` lookups all key on the
                // variable-level name (the agg name has no subscript), so
                // strip the subscript from both ends for the dedup key and
                // the helper calls. Each helper emits the *full* link score
                // for the (var_from, var_to) edge -- per-element when the
                // target is arrayed -- so the loop-score equation's `[elem]`
                // subscript picks the slot the loop actually visits.
                let from_var_level = strip_subscript(link.from.as_str());
                let to_var_level = strip_subscript(link.to.as_str());
                let key = (from_var_level.to_string(), to_var_level.to_string());
                if !seen_links.insert(key) {
                    continue;
                }
                // A loop link whose target is a synthetic agg node is the
                // `source[d] → agg` half; one whose source is a synthetic agg
                // is the `agg → target` half. (The two halves of a hoisted
                // reducer reference appear as two consecutive loop links
                // `X → agg`, `agg → Y` -- the original `(X, Y)` causal edge
                // never appears in an element-level loop once routed.)
                if let Some(agg) = agg_by_name(to_var_level) {
                    emit_source_to_agg_link_scores(
                        db,
                        source_vars,
                        from_var_level,
                        agg,
                        model,
                        project,
                        &mut vars,
                    );
                } else if let Some(agg) = agg_by_name(from_var_level) {
                    emit_agg_to_target_link_scores(
                        db,
                        source_vars,
                        agg_nodes,
                        agg,
                        to_var_level,
                        model,
                        project,
                        &mut vars,
                    );
                } else {
                    emit_link_scores_for_edge(
                        db,
                        source_vars,
                        agg_nodes,
                        from_var_level,
                        to_var_level,
                        model,
                        project,
                        dm_dims,
                        /* skip_agg_halves = */ true,
                        &mut vars,
                    );
                }
            }
        }
    }

    // Part 2: Loop scores and relative loop scores (exhaustive mode only).
    // Generated for any model with feedback loops, regardless of whether
    // it also has input ports. A model can be both a reusable sub-model
    // AND have internal loops that need scoring.
    //
    // Uses element-level cycle partitions so that cross-element feedback
    // is detected correctly (e.g., population[NYC] and population[Boston]
    // in the same partition when connected through migration).
    if let Some(ref detected_loops) = loops {
        let partitions_result = model_element_cycle_partitions(db, model, project);
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

        // Capture each loop's per-slot partition vector before consuming
        // `partitions` so post-sim `compute_rel_loop_scores*` can group slots
        // into the same `(partition, slot)` denominator bins.  The vector's
        // length must match the loop_score series' slot count -- 1 for a
        // scalar/cross-element/mixed loop, the dimension-element-space size
        // for an A2A loop -- which is the same `n_slots` that
        // `ltm_post::build_loop_element_index` derives from
        // `LtmSyntheticVar.dimensions` + the project dims; both feed
        // `compute_rel_loop_scores_per_element`, so a length mismatch would
        // desync the per-element normalization.
        for l in detected_loops.iter() {
            let parts = partitions.partition_for_loop(l, dm_dims);
            debug_assert!(
                {
                    let expected = if l.dimensions.is_empty() {
                        Some(1usize)
                    } else {
                        let n =
                            crate::ltm::loop_dimension_element_tuples(&l.dimensions, dm_dims).len();
                        // n == 0 only when `dm_dims` doesn't cover the loop's
                        // declared dimensions (a mid-edit inconsistency);
                        // `partition_for_loop` then falls back to the present
                        // suffixes and the length is not predictable here.
                        if n == 0 { None } else { Some(n) }
                    };
                    expected.is_none_or(|n| parts.len() == n)
                },
                "loop {:?}: per-slot partition vector length {} disagrees with the loop's slot \
                 count; it must equal `build_loop_element_index`'s n_slots (both feed \
                 `compute_rel_loop_scores_per_element`)",
                l.id,
                parts.len(),
            );
            loop_partitions.insert(l.id.clone(), parts);
        }

        // Build the set of link-score variable names emitted so far so
        // generate_loop_score_equation can resolve each loop link to a
        // name that actually exists. Without this, loops traversing
        // edges whose only AST shape is Wildcard or DynamicIndex would
        // reference a never-emitted Bare canonical name and the
        // fragment compiler would silently fall back to a stub dep.
        let emitted_link_score_names: HashSet<String> = vars
            .iter()
            .filter(|v| v.name.contains("\u{205A}link_score\u{205A}"))
            .map(|v| v.name.clone())
            .collect();
        let loop_vars = crate::ltm_augment::generate_loop_score_variables(
            detected_loops,
            &emitted_link_score_names,
            dm_dims,
        );
        for (name, equation) in loop_vars {
            // The equation carries its own dimension shape (Scalar /
            // ApplyToAll / Arrayed); mirror it onto the layout-sizing
            // `dimensions` field.
            let dimensions = parse::ltm_equation_dimensions(&equation).to_vec();
            vars.push(LtmSyntheticVar {
                name,
                equation,
                dimensions,
                // Loop scores aren't link scores; `assemble_module` compiles
                // them directly via the non-link-score branch already.
                compile_directly: false,
            });
        }
    }

    // Pinned loops (the LOOPSCORE escape hatch, LTM ref section 10).
    //
    // A modeler pins a loop by naming its variable set; the engine then ALWAYS
    // emits that loop's `loop_score`, regardless of mode. This is the whole
    // point in discovery mode -- the heuristic search emits NO loop_score var
    // for any loop, so a pinned loop is the only way to score a specific loop
    // there. In exhaustive mode a pin usually duplicates an already-enumerated
    // loop, so we dedup against `loops` (by canonical variable-cycle rotation)
    // and skip re-emitting; the enumerated loop already carries a score under
    // its `r{n}`/`b{n}`/`u{n}` id.
    //
    // A pin's cycle is dimension-classified by `model_pinned_loops` (GH #653):
    // a pure-A2A pin carries `dimensions` and emits an arrayed (per-element)
    // loop score with a per-slot partition vector; a scalar pin emits a scalar
    // score with a single-slot partition. Both ride the same
    // `generate_loop_score_variables` / `partition_for_loop` machinery as
    // enumerated loops.
    let pinned = model_pinned_loops(db, model, project);
    for (name, reason) in &pinned.invalid {
        // Surface invalid pins the same way the auto-flip gate surfaces its
        // mode change: a `Warning`. Without this a typo'd or stale pin would
        // silently score nothing, which is the failure mode #466 warns about.
        let _ = name;
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: None,
            error: DiagnosticError::Assembly(reason.clone()),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }
    if !pinned.loops.is_empty() {
        // The variable-level node set of each already-emitted enumerated loop,
        // keyed by canonical rotation, so a pin that duplicates one is skipped.
        // (Empty in discovery mode -- `loops` is `None` there -- so every pin
        // is emitted, which is exactly the escape-hatch behavior we want.)
        let enumerated_rotations: HashSet<Vec<String>> = loops
            .as_ref()
            .map(|ls| ls.iter().map(canonical_variable_rotation).collect())
            .unwrap_or_default();

        // The element-keyed cycle partitions, so a pin resolves its per-slot
        // partition(s) the same way enumerated loops do: one entry per element
        // slot for a dimensioned (A2A) pin, a singleton for a scalar one.
        let partitions_result = model_element_cycle_partitions(db, model, project);
        let pin_partitions = CyclePartitions {
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

        // Track which `(from, to)` edges already have a link-score var so a
        // pin only emits the link scores its cycle still needs (in exhaustive
        // mode the enumerated loop already emitted them; in discovery mode all
        // edges did). Keyed on the variable-level edge.
        let mut emitted_edges: HashSet<(String, String)> = vars
            .iter()
            .filter_map(|v| link_score_edge_endpoints(&v.name))
            .collect();

        // Helper: look up the `AggNode` for a synthetic-agg node name (a pin
        // whose cycle traverses an inlined reducer has agg hops in its links,
        // exactly like enumerated loops).
        let agg_by_name = |name: &str| -> Option<&crate::ltm_agg::AggNode> {
            if crate::ltm_agg::is_synthetic_agg_name(name) {
                agg_nodes.aggs.iter().find(|a| a.name == name)
            } else {
                None
            }
        };

        for pin in &pinned.loops {
            // Per scored loop: skip any whose variable-level cycle an
            // enumerated loop already scores (exhaustive mode -- the
            // enumerated emission is preferred, whatever shape the enumerator
            // gave it: A2A, per-element scalars, or cross-element). In
            // discovery mode `enumerated_rotations` is empty, so every scored
            // loop is emitted -- the escape-hatch behavior.
            let loops_to_emit: Vec<&Loop> = pin
                .loops
                .iter()
                .filter(|l| !enumerated_rotations.contains(&canonical_variable_rotation(l)))
                .collect();

            for pin_loop in loops_to_emit {
                // Emit any link scores this loop's cycle needs that aren't
                // present, with the same per-link agg-hop dispatch the
                // enumerated path uses (links may carry element subscripts
                // and traverse synthetic agg nodes).
                for link in &pin_loop.links {
                    let from_var_level = strip_subscript(link.from.as_str());
                    let to_var_level = strip_subscript(link.to.as_str());
                    let key = (from_var_level.to_string(), to_var_level.to_string());
                    if !emitted_edges.insert(key) {
                        continue;
                    }
                    if let Some(agg) = agg_by_name(to_var_level) {
                        emit_source_to_agg_link_scores(
                            db,
                            source_vars,
                            from_var_level,
                            agg,
                            model,
                            project,
                            &mut vars,
                        );
                    } else if let Some(agg) = agg_by_name(from_var_level) {
                        emit_agg_to_target_link_scores(
                            db,
                            source_vars,
                            agg_nodes,
                            agg,
                            to_var_level,
                            model,
                            project,
                            &mut vars,
                        );
                    } else {
                        emit_link_scores_for_edge(
                            db,
                            source_vars,
                            agg_nodes,
                            from_var_level,
                            to_var_level,
                            model,
                            project,
                            dm_dims,
                            /* skip_agg_halves = */ true,
                            &mut vars,
                        );
                    }
                }

                // Register this loop's cycle partition(s): a per-slot vector
                // for a dimensioned (A2A) pin, a singleton for a scalar one --
                // the same `partition_for_loop` resolution enumerated loops
                // use.
                loop_partitions.insert(
                    pin_loop.id.clone(),
                    pin_partitions.partition_for_loop(pin_loop, dm_dims),
                );

                // Emit the pinned loop_score var. The equation is the product
                // of the cycle's link scores, resolved against the names
                // emitted so far -- identical machinery to enumerated loops,
                // including the dimension shaping (a dimensioned pin yields an
                // ApplyToAll / per-slot Arrayed equation exactly like an
                // enumerated A2A loop).
                let emitted_link_score_names: HashSet<String> = vars
                    .iter()
                    .filter(|v| v.name.contains("\u{205A}link_score\u{205A}"))
                    .map(|v| v.name.clone())
                    .collect();
                let pin_loop_vars = crate::ltm_augment::generate_loop_score_variables(
                    std::slice::from_ref(pin_loop),
                    &emitted_link_score_names,
                    dm_dims,
                );
                for (lname, equation) in pin_loop_vars {
                    let dimensions = parse::ltm_equation_dimensions(&equation).to_vec();
                    vars.push(LtmSyntheticVar {
                        name: lname,
                        equation,
                        dimensions,
                        compile_directly: false,
                    });
                }
            }
        }
    }

    // Pathway and composite scores for models with input ports.
    //
    // Each pathway is a product of link-score references. Since
    // emit_per_shape_link_scores only emits the names that appear in
    // each target's AST, we resolve each link against the set of
    // already-emitted names with shape priority (Bare > FixedIndex >
    // Wildcard > DynamicIndex). Without this, an input-port model with
    // an edge like `share[r] = SUM(x[*])` would reference the never-
    // emitted Bare canonical name `x→share`, and the fragment
    // compiler's stub-dep fallback would silently drop that pathway's
    // contribution to the composite score.
    let pathway_emitted_names: HashSet<String> = vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}link_score\u{205A}"))
        .map(|v| v.name.clone())
        .collect();
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
                    // Pathway links come from the variable-level causal
                    // graph (`enumerate_pathways_to_outputs`), so `link.to`
                    // never carries an element subscript and there is no
                    // visited-element to pass.
                    let resolved = crate::ltm_augment::resolve_link_score_name_for_loop(
                        link.from.as_str(),
                        link.to.as_str(),
                        &pathway_emitted_names,
                        None,
                    );
                    format!("\"{resolved}\"")
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
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![],
                compile_directly: false,
            });
        }

        let composite_name = format!(
            "$\u{205A}ltm\u{205A}composite\u{205A}{}",
            input_port.as_str()
        );
        // The selection folds through O(1)-sized accumulator helper variables
        // (named under the port's `⁚path⁚` prefix so they sort -- and therefore
        // evaluate -- after the pathway vars they reference and before this
        // composite). Inlining the fold into one expression would double the
        // equation text per pathway; see `generate_max_abs_selection`.
        let (equation, acc_helpers) =
            generate_max_abs_selection(input_port.as_str(), &pathway_names);
        vars.extend(acc_helpers);
        vars.push(LtmSyntheticVar {
            name: composite_name,
            equation: datamodel::Equation::Scalar(equation),
            dimensions: vec![],
            compile_directly: false,
        });
    }

    // Sort by evaluation-order category so the VM's sequential flow
    // evaluation respects the dependency chain: composites reference paths
    // which reference loop scores which reference link scores, and link
    // scores referencing an aggregate node read its current-step value, so
    // the agg fragment must run first. Within each category, sort lexically
    // for determinism. (`compute_layout` section 3 re-sorts LTM vars purely
    // by name -- `$⁚ltm⁚agg⁚{n}` < `$⁚ltm⁚link_score⁚...` lexically, so the
    // agg gets its layout slot before any consumer there too -- but the
    // runlist order is what the same-timestep ordering hazard turns on, and
    // that comes from this sort.)
    vars.sort_by(|a, b| {
        fn category(name: &str) -> u8 {
            // The agg check uses the `$⁚ltm⁚agg⁚` *prefix*, not a substring
            // search: the `agg → target` link score is named
            // `$⁚ltm⁚link_score⁚$⁚ltm⁚agg⁚{n}→{to}` and contains `⁚agg⁚`,
            // but it is a link score (category 1) that must run *after* the
            // agg aux it references.
            if crate::ltm_agg::is_synthetic_agg_name(name) {
                0 // aggregate nodes: before everything that may reference them
            } else if name.contains("\u{205A}composite\u{205A}") {
                4
            } else if name.contains("\u{205A}path\u{205A}") {
                3
            } else if name.contains("\u{205A}loop_score\u{205A}") {
                2
            } else {
                1 // link_score and anything else
            }
        }
        category(&a.name)
            .cmp(&category(&b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    LtmVariablesResult {
        vars,
        loop_partitions,
        agg_recovery_truncated,
        // Read the resolved mode from the shared `model_ltm_mode` query rather
        // than re-deriving it from the local `is_discovery` flag, so this query
        // and `model_detected_loops` can never disagree about the discovery
        // gate. The two computations are identical by construction (the local
        // `is_discovery` drives the auto-flip `Warning`s and link-score
        // branching above; `model_ltm_mode` is the single source of truth for
        // the decision itself).
        mode: model_ltm_mode(db, model, project),
    }
}

#[cfg(test)]
#[path = "../ltm_tests.rs"]
mod ltm_tests;
