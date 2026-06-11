// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Modeler-pinned feedback loops (the `LOOPSCORE` capability, LTM ref
//! section 10).
//!
//! A practitioner pins a loop by naming its variable *set* (via the
//! `SetLoopName` patch primitive, which writes `LoopMetadata`). The LTM
//! discovery heuristic may not surface a loop the modeler cares about; pinning
//! forces it to ALWAYS be scored, in both exhaustive and discovery mode.
//!
//! `model_pinned_loops` is the single salsa-tracked place a pinned loop's
//! variable set is validated against the causal graph and ordered into a
//! scored [`Loop`]. Both `model_ltm_variables` (which emits the pinned
//! `loop_score` synthetic var) and `model_detected_loops` (the FFI loop
//! surface) read it, so a pinned loop appears identically through both paths.

use std::collections::{HashMap, HashSet};

use crate::common::{Canonical, Ident};
use crate::db::{
    CycleClass, Db, LoopCircuitsResult, ModuleIdentContext, ModuleInputSet, SourceModel,
    SourceProject, SourceVariable, SourceVariableKind, causal_graph_with_modules, classify_cycle,
    model_edge_shapes, model_element_causal_edges, project_datamodel_dims, variable_dimensions,
    variable_direct_dependencies,
};
use crate::ltm::{Loop, strip_subscript};

use super::loops::{
    build_a2a_loop_stocks, build_element_level_loops, cross_agg_loop_budget,
    recover_agg_hop_polarities,
};

/// A pinned loop the LTM pipeline must always score, paired with the user's
/// chosen name so a caller can map the synthetic `pin{n}` id back to a label.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct PinnedLoop {
    /// The fully-annotated scored loop(s) this pin resolves to, with stable
    /// pin-derived ids assigned in declaration order:
    ///
    /// - exactly one `Loop` (id `pin{n}`) for scalar, pure-A2A, and
    ///   diagonal-family pins (the per-element circuits collapse into one
    ///   arrayed loop score);
    /// - one `Loop` per element-level instance (ids `pin{n}` when there is
    ///   one, `pin{n}⁚{j}` when there are several) for cross-element / mixed
    ///   pins whose instances cannot share a single arrayed score.
    ///
    /// The pin-derived ids never collide with the enumerator's
    /// `r{n}`/`b{n}`/`u{n}` namespace.
    pub loops: Vec<Loop>,
    /// The user-supplied loop name. Preserved so a caller can recover the
    /// label behind a `pin{n}` id (the FFI loop surface reports the id +
    /// variable set + this name; callers that prefer can match on the variable
    /// set the way they do for enumerated loops).
    pub name: String,
}

/// One model's resolved pinned loops, plus the names of pins that failed
/// validation so the caller can surface a diagnostic without re-deriving the
/// failure.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub struct PinnedLoopsResult {
    pub loops: Vec<PinnedLoop>,
    /// `(name, reason)` for each pinned loop whose variable set did not form a
    /// scorable feedback loop. The reason is a human-readable explanation for
    /// the surfaced diagnostic.
    pub invalid: Vec<(String, String)>,
}

/// Resolve and validate a model's pinned loops against its causal graph.
///
/// For each non-deleted pin (already projected to canonical variable names at
/// sync time), this recovers the loop's cyclic order from the causal graph,
/// confirms the named set forms a closed cycle containing at least one stock
/// -- where stocks INSIDE a module instance the cycle traverses count (GH
/// #673), via the same `enrich_with_module_stocks` enrichment the enumerator
/// applies, so a loop whose only state lives in a SMOOTH/DELAY instance or
/// stock-carrying user sub-model validates while a purely-instantaneous cycle
/// (including one through a stockless passthrough module) is still rejected
/// -- and assigns a stable `pin{n}` id (n = declaration index, so ids never
/// collide with the enumerator's `r{n}`/`b{n}`/`u{n}` namespace). Pins that
/// fail any check land in `invalid` rather than producing a garbage score.
///
/// The resolved cycle is then dimension-classified exactly the way the
/// exhaustive enumerator classifies cycles ([`classify_cycle`], GH #653):
///
/// - **PureScalar** -> a scalar `Loop` (the pre-#653 behavior, correct for
///   scalar models). A cycle traversing a `ThroughAgg`-routed edge (a
///   scalar feeder of a hoisted reducer) never classifies `PureScalar`
///   (GH #737): it lands in the CrossElementOrMixed arm below, whose
///   element-graph expansion routes it through the synthetic
///   `$⁚ltm⁚agg⁚{n}` node -- the only routing with compilable link scores.
/// - **PureSameElementA2A** -> the `Loop` carries the cycle's shared
///   `dimensions` and element-level stocks, so its loop score is emitted as
///   an arrayed (per-element) variable and its cycle partition resolves per
///   slot.
/// - **CrossElementOrMixed** -> the cycle is expanded on the element-level
///   causal graph ([`expand_pin_on_element_graph`]): the element circuits
///   that instantiate the pinned variable cycle flow through the same
///   grouping machinery the enumerator's slow path uses
///   ([`build_element_level_loops`]), so a diagonal family collapses into
///   one arrayed loop (with per-slot equations) and genuinely cross-element
///   instances each become a scalar loop with element-subscripted links.
///   A pin whose expansion is intractable (oversized SCC) or empty (no
///   element-level instantiation) is invalid, with a clear reason.
///
/// Returned by value (not `salsa::tracked`) because `Loop` does not implement
/// the `PartialEq`/`Update` salsa caching requires; callers invoke it directly
/// off the salsa-tracked `causal_graph_with_modules` / `pinned_loops` inputs,
/// so the underlying graph build is still incrementally cached.
pub(crate) fn model_pinned_loops(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> PinnedLoopsResult {
    let specs = model.pinned_loops(db);
    if specs.is_empty() {
        return PinnedLoopsResult::default();
    }

    let graph = causal_graph_with_modules(db, model, project);
    // Classification inputs: per-edge access shapes and per-variable
    // dimensions, the same data the tiered enumerator classifies cycles with.
    let edge_shapes = model_edge_shapes(db, model, project);
    let source_vars = model.variables(db);
    let dm_dims = project_datamodel_dims(db, project);

    let mut result = PinnedLoopsResult::default();
    for (idx, spec) in specs.iter().enumerate() {
        let id = format!("pin{}", idx + 1);

        if spec.variables.len() < 2 {
            result.invalid.push((
                spec.name.clone(),
                format!(
                    "a pinned loop must name at least two variables that form a feedback loop; \
                     '{}' names {}",
                    spec.name,
                    spec.variables.len()
                ),
            ));
            continue;
        }

        let vars: HashSet<Ident<Canonical>> =
            spec.variables.iter().map(|v| Ident::new(v)).collect();

        let Some(cycle) = graph.order_variable_cycle(&vars) else {
            result.invalid.push((
                spec.name.clone(),
                format!(
                    "the variables named by pinned loop '{}' do not form a closed feedback loop \
                     in the model's causal graph: [{}]",
                    spec.name,
                    spec.variables.join(", ")
                ),
            ));
            continue;
        };

        // A standard feedback loop includes at least one state variable (LTM
        // ref 2.1). State is a stock OR a PREVIOUS-lagged reference (GH #749:
        // `PREVIOUS(x)` is a one-DT memory, listed among the state-retaining
        // builtins in LTM ref section 7) -- a stockless cycle with neither is
        // an instantaneous algebraic loop (a compile-time circular
        // dependency), not a feedback loop, so reject it with a clear
        // message. A stockless cycle broken by PREVIOUS compiles, the
        // enumerator scores it, and so must the pin -- the two surfaces
        // agree.
        //
        // "Stock" here counts state INSIDE any module instance the cycle
        // traverses (GH #673): a SMOOTH/DELAY instance or user sub-model whose
        // internal stock is the loop's only state is a genuine feedback loop.
        // The standalone `detect_loops` surface (`find_loops_with_limit`)
        // attaches module-internal stocks via this same
        // `enrich_with_module_stocks` enrichment instead of filtering, as
        // does the PureScalar pin arm's `build_loop_from_cycle` below.
        // (`model_detected_loops` itself now builds loops with the scored
        // surface's `build_loops_from_tiered` -- GH #746 -- whose fast path
        // uses bare `find_stocks_in_loop` without enrichment; that is
        // behaviorally equivalent here because `DetectedLoop` does not
        // expose stocks and module-internal stocks never resolve in the
        // parent partition map either way.)
        // A cycle through a stockless *passthrough* module enriches to
        // nothing and is still rejected -- it is instantaneous end to end.
        // (Module-INTERNAL lagged state is likewise not counted, mirroring
        // `model_is_stateless`'s parent-level-only lagged leg, so the scored
        // and pinned surfaces agree on that shape too.)
        let parent_stocks = graph.find_stocks_in_loop(&cycle);
        let has_stock = !graph
            .enrich_with_module_stocks(&cycle, parent_stocks)
            .is_empty();
        if !has_stock && !cycle_has_lagged_edge(db, project, source_vars, &cycle) {
            result.invalid.push((
                spec.name.clone(),
                format!(
                    "pinned loop '{}' contains no stock; a feedback loop must pass through at \
                     least one stock (or other state, such as a PREVIOUS-lagged reference)",
                    spec.name
                ),
            ));
            continue;
        }

        // Dimension-classify the cycle (GH #653). Module nodes report empty
        // dimensions, matching the tiered enumerator's treatment of modules
        // as scalar graph nodes.
        let cycle_strs: Vec<String> = cycle.iter().map(|c| c.as_str().to_string()).collect();
        let dim_lookup = |name: &str| -> Vec<crate::dimensions::Dimension> {
            source_vars
                .get(name)
                .map(|sv| variable_dimensions(db, *sv, project).to_vec())
                .unwrap_or_default()
        };
        let loops = match classify_cycle(&cycle_strs, edge_shapes, &dim_lookup) {
            // PureScalar: the pre-#653 scalar construction is correct.
            CycleClass::PureScalar => vec![graph.build_loop_from_cycle(&cycle, id)],
            CycleClass::PureSameElementA2A { dimensions } => {
                vec![build_a2a_pin_loop(&graph, &cycle, id, &dimensions, dm_dims)]
            }
            CycleClass::CrossElementOrMixed => {
                match expand_pin_on_element_graph(
                    db,
                    model,
                    project,
                    &graph,
                    &cycle,
                    source_vars,
                    dm_dims,
                ) {
                    Ok(mut loops) => {
                        // A pin expanded through a hoisted reducer carries
                        // synthetic-agg hops, which come back Unknown-polarity
                        // from the variable-level graph (GH #516, same as the
                        // enumerator's slow path); patch the derivable cases so
                        // the pin's reported polarity isn't degraded to
                        // Undetermined. Must run BEFORE `assign_pin_ids`: the
                        // patcher re-runs the (content-sorting) enumerator id
                        // assignment when it changes anything, and the
                        // pin-derived ids overwrite those positionally.
                        recover_agg_hop_polarities(&mut loops, &graph, db, model, project);
                        assign_pin_ids(&mut loops, &id);
                        loops
                    }
                    Err(reason) => {
                        result.invalid.push((
                            spec.name.clone(),
                            format!("pinned loop '{}' {reason}", spec.name),
                        ));
                        continue;
                    }
                }
            }
        };
        result.loops.push(PinnedLoop {
            loops,
            name: spec.name.clone(),
        });
    }

    result
}

/// Whether any edge `from -> to` of the ordered cycle is a PREVIOUS-lagged
/// reference: `to` references `from` ONLY inside `PREVIOUS(...)` in its dt
/// equation (`dt_previous_referenced_vars`, the `previous_only`
/// classification). Such an edge is the one-DT memory that lets a stockless
/// cycle compile -- and `PREVIOUS` retains state (LTM ref section 7), so the
/// cycle is a genuine feedback loop the pin validation must accept (GH #749).
///
/// `previous_only` (rather than any-PREVIOUS) is the right test: a reference
/// appearing both inside and outside `PREVIOUS` keeps its instantaneous
/// edge, so a cycle through it only compiles when some OTHER edge breaks it
/// -- and that breaking edge is itself previous-only or a stock (which the
/// caller's stock check already accepted).
///
/// Module and stock nodes are skipped as the EDGE TARGET: a stock is state
/// in its own right (the caller's check), and a module's lagged INTERNAL
/// state is deliberately invisible here, mirroring `model_is_stateless`'s
/// parent-level-only lagged leg (GH #773). A module as the edge SOURCE is
/// fine: `reader = PREVIOUS(sub.output, 0)` is a parent-level lag of the
/// module's output, recorded in previous_only as the UN-normalized
/// `sub·output` -- while the cycle node is the module-normalized `sub` --
/// so each entry is collapsed through the same `normalize_module_ref_str`
/// the causal-edge builder applies before comparing. Uses the same empty
/// module-ident context / empty input set as `model_causal_edges`, so the
/// per-variable dependency queries are shared salsa cache hits.
fn cycle_has_lagged_edge(
    db: &dyn Db,
    project: SourceProject,
    source_vars: &HashMap<String, SourceVariable>,
    cycle: &[Ident<Canonical>],
) -> bool {
    let empty_ctx = ModuleIdentContext::new(db, vec![]);
    let empty_inputs = ModuleInputSet::empty(db);
    cycle.iter().enumerate().any(|(i, from)| {
        let to = &cycle[(i + 1) % cycle.len()];
        let Some(sv) = source_vars.get(to.as_str()) else {
            return false;
        };
        if matches!(
            sv.kind(db),
            SourceVariableKind::Stock | SourceVariableKind::Module
        ) {
            return false;
        }
        variable_direct_dependencies(db, *sv, project, empty_ctx, empty_inputs)
            .dt_previous_referenced_vars
            .iter()
            .any(|dep| crate::db::analysis::normalize_module_ref_str(dep) == from.as_str())
    })
}

/// Assign pin-derived ids to a pin's expanded loops: the plain `pin{n}` when
/// the expansion produced exactly one loop, `pin{n}⁚{j}` (1-based) when it
/// produced several. The `j` ordering is deterministic: the
/// [`build_element_level_loops`] emission order, content-re-sorted by
/// `recover_agg_hop_polarities`' internal `assign_loop_ids` when any agg-hop
/// polarity was patched (the caller runs the patch before this function) --
/// both orders are pure functions of loop content. The `⁚` separator is
/// the reserved synthetic-name separator, so multi-instance ids can never
/// collide with user content or the enumerator's id namespace.
fn assign_pin_ids(loops: &mut [Loop], pin_id: &str) {
    if loops.len() == 1 {
        loops[0].id = pin_id.to_string();
    } else {
        for (j, l) in loops.iter_mut().enumerate() {
            l.id = format!("{pin_id}\u{205A}{}", j + 1);
        }
    }
}

/// Expand a CrossElementOrMixed pinned cycle on the element-level causal
/// graph and group its instances into scored loops.
///
/// 1. Project [`model_element_causal_edges`] onto the pin's variables plus
///    synthetic `$⁚ltm⁚agg⁚{n}` nodes (a cycle through an inlined reducer
///    genuinely traverses its aggregate node) -- the same `keep_node` rule
///    the tiered enumerator's slow path uses.
/// 2. Reject the pin when the projected subgraph's largest SCC exceeds
///    [`crate::ltm::MAX_LTM_SCC_NODES`] (element-level Johnson at that scale
///    is the cost cliff the auto-flip gate exists to avoid).
/// 3. Run Johnson on the subgraph and keep only the circuits that
///    *instantiate the pinned cycle*: the circuit's variable set (subscripts
///    stripped, agg nodes ignored) equals the pin's variable set. Sub-cycles
///    over a strict subset of the pin's variables are not instances.
/// 4. Reject the pin when no circuit matches (the variable-level cycle has no
///    element-level instantiation).
/// 5. Group the matching circuits with [`build_element_level_loops`] -- the
///    same machinery the enumerator's slow path uses -- so diagonal families
///    collapse into one arrayed loop (with `slot_links` driving per-slot
///    equations) and cross-element instances become element-subscripted
///    scalar loops.
///
/// On success the returned loops carry the enumerator's `r{n}`/`b{n}`/`u{n}`
/// ids; the caller re-assigns pin-derived ids. On failure the returned string
/// completes the sentence "pinned loop '{name}' ..." in the diagnostic.
#[allow(clippy::too_many_arguments)]
fn expand_pin_on_element_graph(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_graph: &crate::ltm::CausalGraph,
    cycle: &[Ident<Canonical>],
    source_vars: &HashMap<String, SourceVariable>,
    dm_dims: &[crate::datamodel::Dimension],
) -> Result<Vec<Loop>, String> {
    let pin_var_set: HashSet<&str> = cycle.iter().map(|c| c.as_str()).collect();

    // Step 1: project the element graph onto the pin's variables + agg nodes.
    let element_edges = model_element_causal_edges(db, model, project);
    let keep_node = |name: &str| -> bool {
        pin_var_set.contains(strip_subscript(name)) || crate::ltm_agg::is_synthetic_agg_name(name)
    };
    let mut sub_edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
    for (from, tos) in &element_edges.edges {
        if !keep_node(from) {
            continue;
        }
        let filtered: Vec<Ident<Canonical>> = tos
            .iter()
            .filter(|to| keep_node(to))
            .map(|to| Ident::new(to))
            .collect();
        if !filtered.is_empty() {
            sub_edges.insert(Ident::new(from), filtered);
        }
    }
    let sub_stocks: HashSet<Ident<Canonical>> = element_edges
        .stocks
        .iter()
        .filter(|s| pin_var_set.contains(strip_subscript(s.as_str())))
        .map(|s| Ident::new(s))
        .collect();
    let sub_graph = crate::ltm::CausalGraph {
        edges: sub_edges,
        stocks: sub_stocks,
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    };

    // Step 2: SCC guard.
    let scc = sub_graph.largest_scc_size();
    if scc > crate::ltm::MAX_LTM_SCC_NODES {
        return Err(format!(
            "cannot be scored: its element-level expansion forms a strongly-connected component \
             of {scc} nodes, exceeding the tractable limit of {}",
            crate::ltm::MAX_LTM_SCC_NODES
        ));
    }

    // Step 3: Johnson + instance filtering. The SCC guard above bounds node
    // count but not density; a dense element-level expansion can hold more
    // elementary circuits than is tractable to enumerate (see
    // `MAX_LTM_CIRCUITS`), so the budget turns that into an invalid pin
    // instead of an OOM.
    let Ok((names, circuits)) =
        sub_graph.find_indexed_circuits_with_limit(crate::ltm::ltm_circuit_budget())
    else {
        return Err(format!(
            "cannot be scored: its element-level expansion contains more than {} \
             elementary circuits, exceeding the tractable enumeration budget",
            crate::ltm::ltm_circuit_budget()
        ));
    };
    let matching: Vec<Vec<u32>> = circuits
        .into_iter()
        .filter(|circuit| {
            let circuit_vars: HashSet<&str> = circuit
                .iter()
                .map(|&i| strip_subscript(&names[i as usize]))
                .filter(|n| !crate::ltm_agg::is_synthetic_agg_name(n))
                .collect();
            circuit_vars == pin_var_set
        })
        .collect();

    // Step 4: no instantiation -> invalid.
    if matching.is_empty() {
        return Err(
            "cannot be scored: its variable-level cycle has no element-level instantiation \
             (the per-element equations never close a cycle at any element)"
                .to_string(),
        );
    }

    // Step 5: group via the enumerator's machinery.
    let filtered = LoopCircuitsResult {
        names,
        circuits: matching,
        truncated: false,
    };
    let (loops, _truncated_aggs) = build_element_level_loops(
        &filtered,
        var_graph,
        source_vars,
        db,
        model,
        project,
        dm_dims,
        cross_agg_loop_budget(),
    );
    Ok(loops)
}

/// Build the `Loop` for a pinned PureSameElementA2A cycle: variable-level
/// links, the cycle's shared dimensions (mapped to datamodel casing so the
/// loop-score equation parses), and element-level stocks (the `Loop`
/// docstring's granularity invariant, required for per-slot partition
/// resolution).
///
/// Mirrors `build_loops_from_tiered`'s fast-path construction. Module stock
/// enrichment is not needed: a cycle containing a module node classifies as
/// CrossElementOrMixed (modules are scalar graph nodes), so it never reaches
/// this function.
fn build_a2a_pin_loop(
    graph: &crate::ltm::CausalGraph,
    cycle: &[Ident<Canonical>],
    id: String,
    canonical_dims: &[String],
    dm_dims: &[crate::datamodel::Dimension],
) -> Loop {
    let links = graph.circuit_to_links(cycle);
    let var_stocks = graph.find_stocks_in_loop(cycle);
    let polarity = graph.calculate_polarity(&links);
    let dimensions: Vec<String> = canonical_dims
        .iter()
        .map(|canonical| {
            dm_dims
                .iter()
                .find(|dm| crate::common::canonicalize(dm.name()).as_ref() == canonical.as_str())
                .map(|dm| dm.name().to_string())
                .unwrap_or_else(|| canonical.to_string())
        })
        .collect();
    let stocks = build_a2a_loop_stocks(&var_stocks, &dimensions, dm_dims);
    Loop {
        id,
        links,
        stocks,
        polarity,
        dimensions,
        slot_links: vec![],
    }
}
