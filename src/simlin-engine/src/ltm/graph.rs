// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Public causal-graph type and the elementary-circuit / partition surface
//! built on top of the `IndexedGraph` Johnson enumerator.
//!
//! `CausalGraph` owns model adjacency, stock identity, variable AST
//! references for polarity analysis, and recursively-built sub-graphs for
//! dynamic modules. Callers interact with this type to find loops, compute
//! cycle partitions, materialize per-link polarities, and traverse module
//! pathways.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::common::{Canonical, Ident, Result};
use crate::model::ModelStage1;
use crate::project::Project;
use crate::variable::{Variable, identifier_set};

use super::indexed::{IndexedCircuits, IndexedGraph, TruncatedByBudgetInternal};
use super::partitions::{CyclePartitions, tarjan_scc};
use super::polarity::{analyze_agg_consumer_polarity, analyze_link_polarity};
use super::types::{
    Link, LinkPolarity, Loop, LoopPolarity, ModuleLtmRole, TruncatedByBudget,
    classify_module_for_ltm, normalize_module_ref,
};

/// Internal module pathways keyed by input port: each port maps to its list of
/// open `input -> ... -> output` link-paths.
pub(crate) type ModulePathways = HashMap<Ident<Canonical>, Vec<Vec<Link>>>;

/// [`ModulePathways`] paired with the (sorted) input ports whose enumeration hit
/// the per-port pathway budget (GH #649) and so hold only a deterministic prefix.
pub(crate) type ModulePathwaysWithTruncation = (ModulePathways, Vec<Ident<Canonical>>);

/// Maximum number of internal simple pathways enumerated per module input
/// port before truncation kicks in (GH #649).
///
/// Each enumerated pathway becomes one `$⁚ltm⁚path⁚{input}⁚{idx}` synthetic
/// variable (and folds into the input's composite link score), so the
/// pathway count is model-controlled and, without a cap, unbounded: a module
/// body shaped as a chain of N diamonds has `2^N` short simple pathways that
/// never trip the depth cap, so an adversarial or accidentally-dense module
/// could mint exponentially many synthetic variables and exhaust memory.
///
/// This is defense-in-depth against that blowup, NOT a tight performance cap.
/// The value is chosen comfortably above the legitimate in-repo corpus
/// maximum: the covid19-us-homer SSTATS macro module legitimately produces
/// 303 pathways through its busiest input port via the production composite
/// path, and the covid19 root model's `enrich_with_module_stocks` pathway
/// enumeration peaks at 6744 simple pathways from one node. 8192 clears both
/// with headroom while still tripping long before a true exponential blowup
/// (a 14-diamond chain already exceeds it). Truncation is deterministic
/// (DFS order over sorted neighbor lists) and surfaces a `Warning` naming the
/// module and input port; the composite score then degrades to the max over
/// the kept pathway set, consistent with the macro composite's heuristic
/// nature.
pub(crate) const MAX_MODULE_PATHWAYS: usize = 8192;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`MAX_MODULE_PATHWAYS`], scoped by an active
    /// [`ModulePathwayBudgetGuard`]. Lets a test trip the pathway budget with
    /// a tiny fixture instead of building a module body large enough to mint
    /// thousands of real pathways (per docs/dev/rust.md#test-time-budgets, the
    /// same override pattern as `db::ltm::AggLoopBudgetGuard` for GH #515).
    static MODULE_PATHWAY_BUDGET_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The per-input-port module-pathway budget for the current enumeration.
/// Returns [`MAX_MODULE_PATHWAYS`] in production builds; in `#[cfg(test)]`
/// builds an active [`ModulePathwayBudgetGuard`] override takes precedence.
pub(crate) fn module_pathway_budget() -> usize {
    #[cfg(test)]
    {
        if let Some(b) = MODULE_PATHWAY_BUDGET_OVERRIDE.with(|c| c.get()) {
            return b;
        }
    }
    MAX_MODULE_PATHWAYS
}

/// RAII guard (test-only) that overrides [`module_pathway_budget`] for the
/// current thread for the guard's lifetime, restoring the previous value on
/// drop -- so a panicking test does not leak the override to the next test
/// reusing the thread.
///
/// Because the production composite path runs inside the salsa-memoized
/// `model_ltm_variables`, a salsa-driven test's guard must outlive every
/// `model_ltm_variables` call whose budget it controls (a later call on the
/// same `db` would otherwise return the memoized tiny-budget result), the same
/// caveat `AggLoopBudgetGuard` documents.
#[cfg(test)]
pub(crate) struct ModulePathwayBudgetGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl ModulePathwayBudgetGuard {
    pub(crate) fn new(budget: usize) -> Self {
        let prev = MODULE_PATHWAY_BUDGET_OVERRIDE.with(|c| c.replace(Some(budget)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for ModulePathwayBudgetGuard {
    fn drop(&mut self) {
        MODULE_PATHWAY_BUDGET_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// Get direct dependencies from a Variable
pub(super) fn get_variable_dependencies(var: &Variable) -> Vec<Ident<Canonical>> {
    match var {
        Variable::Module { inputs, .. } => {
            // For modules, dependencies are the source variables of inputs
            inputs.iter().map(|input| input.src.clone()).collect()
        }
        _ => {
            // Get the main equation AST
            let ast = var.ast();
            match ast {
                Some(ast) => {
                    // We don't have dimensions info here, so pass empty vec
                    // We also don't have module inputs, so pass None
                    identifier_set(ast, &[], None).into_iter().collect()
                }
                None => vec![],
            }
        }
    }
}

/// Graph representation for loop detection
impl std::fmt::Debug for CausalGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CausalGraph {{ edges: {:?}, stocks: {:?} }}",
            self.edges, self.stocks
        )
    }
}

pub struct CausalGraph {
    /// Adjacency list representation
    pub(crate) edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>,
    /// Set of stocks in the model
    pub(crate) stocks: HashSet<Ident<Canonical>>,
    /// Variables in the model for polarity analysis
    pub(crate) variables: HashMap<Ident<Canonical>, Variable>,
    /// Module instances and their internal graphs
    pub(crate) module_graphs: HashMap<Ident<Canonical>, Box<CausalGraph>>,
}

impl CausalGraph {
    /// Read-only access to the adjacency list (for benchmarks / debugging).
    pub fn edges(&self) -> &HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> {
        &self.edges
    }

    /// Read-only access to the stock set (for benchmarks / debugging).
    pub fn stocks(&self) -> &HashSet<Ident<Canonical>> {
        &self.stocks
    }

    /// Return the number of nodes in the largest strongly-connected
    /// component, or 0 when the graph has no edges.  Singletons without
    /// self-loops count as size-1 SCCs, so an acyclic graph's return
    /// value is at most 1 and never triggers [`super::MAX_LTM_SCC_NODES`].
    ///
    /// Used as the auto-flip gate in
    /// [`crate::db::model_ltm_variables`]: if any SCC exceeds
    /// [`super::MAX_LTM_SCC_NODES`], LTM compilation switches to discovery
    /// mode before paying for full circuit enumeration.  Runs in
    /// O(V + E) via the iterative Tarjan implementation that backs
    /// Johnson's enumerator.
    pub fn largest_scc_size(&self) -> usize {
        let indexed = IndexedGraph::from_edges(&self.edges);
        indexed
            .tarjan_scc()
            .into_iter()
            .map(|scc| scc.len())
            .max()
            .unwrap_or(0)
    }

    /// Build a causal graph from a model with project context for modules
    pub fn from_model(model: &ModelStage1, project: &Project) -> Result<Self> {
        let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        let mut stocks = HashSet::new();
        let mut variables = HashMap::new();
        let mut module_graphs = HashMap::new();

        // Build edges from variable dependencies
        for (var_name, var) in &model.variables {
            // Store variable for polarity analysis
            variables.insert(var_name.clone(), var.clone());

            // Record if this is a stock
            if matches!(var, Variable::Stock { .. }) {
                stocks.insert(var_name.clone());
            }

            // Handle modules specially
            if let Variable::Module {
                model_name, inputs, ..
            } = var
            {
                // Build internal graph for this module instance if we have the model
                if let Some(module_model) = project.models.get(model_name)
                    && classify_module_for_ltm(module_model) == ModuleLtmRole::DynamicModule
                {
                    // Recursively build graph for the module
                    let module_graph = CausalGraph::from_model(module_model, project)?;
                    module_graphs.insert(var_name.clone(), Box::new(module_graph));
                }

                // Add edges from input sources to the module
                for input in inputs {
                    edges
                        .entry(input.src.clone())
                        .or_default()
                        .push(var_name.clone());
                }
            } else {
                // For stocks, also add edges from inflows and outflows
                if let Variable::Stock {
                    inflows, outflows, ..
                } = var
                {
                    for flow in inflows.iter().chain(outflows.iter()) {
                        edges
                            .entry(flow.clone())
                            .or_default()
                            .push(var_name.clone());
                    }
                } else {
                    // Get dependencies and create edges for flows + auxes.  We don't want to
                    // do this for stocks because get_variable_dependencies() only looks at the
                    // equation for the stock's initial value
                    let deps = get_variable_dependencies(var);
                    for dep in deps {
                        let normalized_dep = normalize_module_ref(&dep);
                        edges
                            .entry(normalized_dep)
                            .or_default()
                            .push(var_name.clone());
                    }
                }
            }
        }

        Ok(CausalGraph {
            edges,
            stocks,
            variables,
            module_graphs,
        })
    }

    /// Find all elementary circuits (feedback loops).
    ///
    /// The caller supplies the enumeration budget as `max_circuits`.
    /// Production paths pass `usize::MAX` (no truncation); stress tests
    /// and diagnostic harnesses pass smaller budgets and check for the
    /// [`TruncatedByBudget`] signal.  The downstream LTM pipeline is
    /// gated separately by [`super::MAX_LTM_SCC_NODES`] at
    /// [`crate::db::model_ltm_variables`].
    pub fn find_loops_with_limit(
        &self,
        max_circuits: usize,
    ) -> std::result::Result<Vec<Loop>, TruncatedByBudget> {
        // Enumerate indexed circuits via SCC-restricted DFS, then rebuild
        // the polarity-annotated `Loop` structs from the circuits that
        // survive dedup.  Splitting enumeration from materialization keeps
        // the hot DFS loop allocating only `Vec<u32>` and lets circuits
        // that dedup away shed their string storage before we pay for it.
        let indexed = self.enumerate_indexed_circuits(max_circuits)?;
        let mut loops = Vec::with_capacity(indexed.circuits.len());
        for circuit_idx in indexed.circuits {
            // Circuit length is always > 1 by construction in
            // `IndexedGraph::enumerate_circuits_in_scc` (pure self-loops
            // are filtered), so the check below is defensive -- kept to
            // guarantee the contract even if a future refactor loosens
            // that invariant.
            if circuit_idx.len() > 1 {
                let circuit = indexed.graph.circuit_to_idents(&circuit_idx);
                let links = self.circuit_to_links(&circuit);
                let parent_stocks = self.find_stocks_in_loop(&circuit);
                let stocks = self.enrich_with_module_stocks(&circuit, parent_stocks);
                let polarity = self.calculate_polarity(&links);

                loops.push(Loop {
                    id: String::new(),
                    links,
                    stocks,
                    polarity,
                    dimensions: vec![],
                    slot_links: vec![],
                });
            }
        }

        // The per-SCC inline dedup in `johnson_circuit` already rejects
        // duplicate canonical-rotation circuits at the `Vec<u32>`
        // level, so every circuit that reaches this point has a
        // distinct directed edge sequence.  A separate Loop-level
        // dedup would be redundant; debug builds verify the invariant
        // so a future regression trips a test.
        debug_assert!(
            loops_have_unique_canonical_rotations(&loops),
            "circuit enumerator must emit unique canonical rotations; duplicate loops reached find_loops_with_limit"
        );

        // Assign deterministic IDs based on sorted loop content
        self.assign_deterministic_loop_ids(&mut loops);

        Ok(loops)
    }

    /// Shared enumeration core used by both `find_loops_with_limit` and
    /// `find_circuit_node_lists_with_limit`.  Builds a compact indexed view
    /// of the adjacency list, decomposes into strongly-connected components,
    /// and enumerates circuits only inside non-trivial SCCs -- cross-SCC
    /// edges cannot close a cycle and exploring them is wasted work (a
    /// significant fraction of time on dense WRLD3-shaped graphs).
    ///
    /// Returns the circuits as integer-index paths plus the `IndexedGraph`
    /// that owns the canonical node ordering, so callers can convert back
    /// to `Ident<Canonical>` lazily and only for surviving circuits.
    fn enumerate_indexed_circuits(
        &self,
        max_circuits: usize,
    ) -> std::result::Result<IndexedCircuits, TruncatedByBudget> {
        let graph = IndexedGraph::from_edges(&self.edges);
        let sccs = graph.tarjan_scc();
        let mut all_circuits: Vec<Vec<u32>> = Vec::new();
        let mut budget = max_circuits;

        // Cross-SCC scratch: maps a global node id to its position within
        // the currently-processing SCC, with `-1` for non-members.  Sized
        // to the whole graph (one allocation per top-level enumeration
        // call) and reset between SCCs by walking the previous SCC's
        // members.  Letting `enumerate_circuits_in_scc` size its
        // per-SCC `JohnsonState` to `scc.len()` instead of
        // `nodes.len()` is the whole point of threading this map through
        // -- on graphs with many small SCCs in a large node space it
        // cuts the transient allocation from O(|graph|) per SCC to
        // O(|SCC|) per SCC.  See issue #460 for the measurement context.
        let mut global_to_local: Vec<i32> = vec![-1; graph.nodes.len()];

        // SCC iteration order is whatever Tarjan emitted.  Each SCC's
        // contribution to `all_circuits` is independent, and per-cycle
        // uniqueness comes from the per-SCC `seen` fingerprint set, not
        // from iteration order -- so there's no correctness reason to
        // pin the order.  Within a single process HashMap iteration is
        // stable, so repeated calls on the same `CausalGraph` still
        // yield identical output.
        for scc in &sccs {
            // Skip trivial SCCs (single node, no self-loop): they cannot
            // carry any elementary circuit and iterating them was
            // measurable overhead on graphs with many feeder nodes.
            if scc.len() == 1 && !graph.succ[scc[0] as usize].contains(&scc[0]) {
                continue;
            }
            match graph.enumerate_circuits_in_scc(scc, &mut budget, &mut global_to_local) {
                Ok(mut circuits) => all_circuits.append(&mut circuits),
                Err(TruncatedByBudgetInternal) => return Err(TruncatedByBudget),
            }
        }

        // Each SCC's enumerator already deduplicates by canonical
        // edge-sequence rotation, and different SCCs share no nodes --
        // so `all_circuits` has no cross-SCC duplicates.  Debug builds
        // verify the invariant.
        debug_assert!(
            IndexedGraph::has_no_duplicate_canonical_rotations(&all_circuits),
            "enumerate_circuits_in_scc should emit unique canonical rotations per SCC"
        );

        Ok(IndexedCircuits {
            graph,
            circuits: all_circuits,
        })
    }

    /// Find all elementary circuits as deduplicated node lists.
    /// Only needs edges -- does not compute polarity or assign IDs.
    ///
    /// Budget semantics match [`Self::find_loops_with_limit`]: the
    /// caller supplies `max_circuits` and receives
    /// `Err(TruncatedByBudget)` when the DFS would enumerate more than
    /// that many elementary circuits.  Production paths pass
    /// `usize::MAX`; callers that need the bounded variant pass a
    /// smaller budget and interpret the error as "too many loops to
    /// enumerate".
    pub fn find_circuit_node_lists_with_limit(
        &self,
        max_circuits: usize,
    ) -> std::result::Result<Vec<Vec<Ident<Canonical>>>, TruncatedByBudget> {
        let indexed = self.enumerate_indexed_circuits(max_circuits)?;
        Ok(indexed
            .circuits
            .into_iter()
            .map(|c| indexed.graph.circuit_to_idents(&c))
            .collect())
    }

    /// Indexed view of the elementary circuits: a shared `Vec<String>`
    /// name table plus circuits as `Vec<Vec<u32>>` indices into that
    /// table.  Callers that want the named view can reconstruct it on
    /// demand via the indices, but many consumers only need to iterate,
    /// length-check, or group circuits -- all of which work on the
    /// integer form without paying the per-name allocation cost.
    ///
    /// The returned name table is trimmed to **only the nodes that
    /// actually appear in a circuit**: non-cyclic feeder variables are
    /// omitted, and `circuits` use compact indices into the trimmed
    /// table.  This matters for salsa cache stability -- otherwise a
    /// rename of any acyclic variable would change the (full) name
    /// table and invalidate every downstream LTM query even though the
    /// loop structure is unchanged.  `names` is empty when `circuits`
    /// is empty (pure DAG / truncated budget).
    ///
    /// Same budget semantics as [`Self::find_circuit_node_lists_with_limit`];
    /// returns `Err(TruncatedByBudget)` when the enumeration would
    /// exceed `max_circuits`.
    pub fn find_indexed_circuits_with_limit(
        &self,
        max_circuits: usize,
    ) -> std::result::Result<(Vec<String>, Vec<Vec<u32>>), TruncatedByBudget> {
        let indexed = self.enumerate_indexed_circuits(max_circuits)?;
        let mut circuits = indexed.circuits;

        if circuits.is_empty() {
            return Ok((Vec::new(), circuits));
        }

        // Compact the name table to only the indices that appear in a
        // circuit.  Using BTreeSet preserves ascending order so the
        // trimmed `names` stays lex-sorted (the enumerator's canonical
        // ordering, which downstream tests rely on).
        let mut used: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        for c in &circuits {
            for &n in c {
                used.insert(n);
            }
        }
        let used_vec: Vec<u32> = used.into_iter().collect();

        // Sparse mapping old_idx -> new_idx.  used_vec.len() is bounded
        // by the (non-trivial) SCC size -- at most a few hundred even
        // on dense SD models -- so a Vec<i32> keyed by old index is
        // both faster and denser than a HashMap.
        let mut old_to_new: Vec<i32> = vec![-1; indexed.graph.nodes.len()];
        for (new_i, &old_i) in used_vec.iter().enumerate() {
            old_to_new[old_i as usize] = new_i as i32;
        }
        for c in &mut circuits {
            for n in c {
                *n = old_to_new[*n as usize] as u32;
            }
        }

        let names: Vec<String> = used_vec
            .iter()
            .map(|&old| indexed.graph.nodes[old as usize].as_str().to_string())
            .collect();

        // Belt-and-braces: after remap, every circuit index must address
        // a real entry in the trimmed `names` table.  A future refactor
        // that accidentally drops a remapped index, mixes global and
        // compact indices, or emits a circuit whose node is missing
        // from `used_vec` would silently corrupt downstream lookups
        // like `LoopCircuitsResult::circuit_names`.  Release builds
        // compile this check out.
        debug_assert!(
            circuits
                .iter()
                .all(|c| c.iter().all(|&i| (i as usize) < names.len())),
            "every compact index in trimmed circuits must address a valid name-table entry"
        );

        Ok((names, circuits))
    }

    /// Enrich a loop's stock list with stocks from inside any DynamicModule
    /// nodes that appear in the circuit. For each module node, we find the
    /// internal pathway from the relevant input port to the output, and collect
    /// all stocks along that pathway.
    ///
    /// `pub(super)` (not private) so the `ltm::tests` sibling module can drive
    /// the GH #649 truncation-fallback path directly.
    pub(super) fn enrich_with_module_stocks(
        &self,
        circuit: &[Ident<Canonical>],
        mut stocks: Vec<Ident<Canonical>>,
    ) -> Vec<Ident<Canonical>> {
        for (i, node) in circuit.iter().enumerate() {
            let module_graph = match self.module_graphs.get(node) {
                Some(g) => g,
                None => continue,
            };
            let module_var = match self.variables.get(node) {
                Some(Variable::Module { inputs, .. }) => inputs,
                _ => continue,
            };

            // The predecessor in the circuit is the variable that feeds
            // into this module.
            let pred_idx = if i == 0 { circuit.len() - 1 } else { i - 1 };
            let predecessor = &circuit[pred_idx];

            // Find which input port the predecessor maps to.
            let internal_port = module_var
                .iter()
                .find(|inp| &inp.src == predecessor)
                .map(|inp| &inp.dst);

            let (pathways, truncated_ports) =
                module_graph.enumerate_pathways_to_outputs_with_truncation(&[]);

            let internal_stocks: Vec<Ident<Canonical>> = if let Some(port) = internal_port {
                // Collect stocks from all pathways for the matched input port.
                // When that port's pathway enumeration hit the budget (GH #649)
                // the kept paths are a prefix and could miss a stock that lives
                // only on a dropped pathway, so degrade to the conservative
                // "all module-internal stocks" fallback rather than silently
                // dropping stocks from the enriched loop.
                match pathways.get(port) {
                    Some(paths) if !truncated_ports.contains(port) => {
                        collect_stocks_from_pathways(module_graph, paths, node)
                    }
                    // Port has no pathway, or its enumeration was truncated:
                    // fall back to all module-internal stocks.
                    _ => all_module_stocks(module_graph, node),
                }
            } else {
                // Predecessor doesn't match any module input (shouldn't happen
                // with a well-formed graph). Conservative fallback.
                all_module_stocks(module_graph, node)
            };

            for s in internal_stocks {
                if !stocks.contains(&s) {
                    stocks.push(s);
                }
            }
        }
        stocks
    }

    /// Convert a circuit (list of nodes) to a list of links
    pub fn circuit_to_links(&self, circuit: &[Ident<Canonical>]) -> Vec<Link> {
        let mut links = Vec::new();
        for i in 0..circuit.len() {
            let from = &circuit[i];
            let to = &circuit[(i + 1) % circuit.len()];
            let polarity = self.get_link_polarity(from, to);
            links.push(Link {
                from: from.clone(),
                to: to.clone(),
                polarity,
            });
        }
        links
    }

    /// Convert an OPEN path (not a closed circuit) to a list of links.
    /// Unlike `circuit_to_links`, does NOT add a closing link from last to first.
    pub(crate) fn path_to_links(&self, path: &[Ident<Canonical>]) -> Vec<Link> {
        let mut links = Vec::new();
        for i in 0..path.len().saturating_sub(1) {
            let from = &path[i];
            let to = &path[i + 1];
            let polarity = self.get_link_polarity(from, to);
            links.push(Link {
                from: from.clone(),
                to: to.clone(),
                polarity,
            });
        }
        links
    }

    /// Find all simple paths from `from` to `to` using DFS with a visited set.
    ///
    /// Returns `(paths, truncated)`: `paths` is the list of open paths (each a
    /// node-ident sequence) found in deterministic DFS order, and `truncated`
    /// is `true` when enumeration stopped early because it hit the pathway
    /// budget or the per-call work bound (see [`dfs_simple_paths`] for the
    /// bound the DFS actually enforces).
    ///
    /// `budget` caps the number of *completed* paths. Without a cap the simple-
    /// path count can be exponential in the graph size (a chain of N diamond
    /// patterns has `2^N` short paths, well under the depth cap), so an
    /// adversarial or accidentally-dense module body could mint exponentially
    /// many `$⁚ltm⁚path⁚…` synthetic variables and exhaust memory (GH #649).
    pub(crate) fn find_all_simple_paths(
        &self,
        from: &Ident<Canonical>,
        to: &Ident<Canonical>,
        max_depth: usize,
        budget: usize,
    ) -> (Vec<Vec<Ident<Canonical>>>, bool) {
        let mut paths = Vec::new();
        let mut current_path = vec![from.clone()];
        let mut visited = HashSet::new();
        visited.insert(from.clone());
        // Bound the DFS WORK, not just the result count. A cap on completed
        // paths alone leaves the pathological case where the walk explores an
        // exponential dead-end frontier (subgraphs with no path to `to`) before
        // completing `budget` paths. `expansions_left` charges one unit per
        // recursive neighbor descent; once it is exhausted the DFS unwinds and
        // truncation is signalled.
        //
        // The allowance `budget * max_depth` is exactly the worst-case
        // expansion count to complete `budget` paths in a *dead-end-free* graph:
        // the tightest such graph has `budget` prefix-disjoint chains of length
        // `max_depth` from the source to `to`, so finding all `budget` costs
        // `budget * max_depth` fresh descents (prefix-sharing only lowers this,
        // since backtracking re-descends a single sibling edge, not a whole
        // chain). So in a dead-end-free enumeration the *completed-path* cap --
        // not this work cap -- is what stops the walk and legitimate
        // enumeration is never starved (proven by
        // `module_pathways_work_bound_does_not_starve_exact_budget`), while a
        // dead-end frontier still aborts in `O(budget * max_depth)` expansions.
        //
        // This no-starvation guarantee is scoped to dead-end-free graphs. In a
        // *mixed* graph -- real paths plus dead-end subgraphs -- the sorted
        // neighbor order can descend a dead-end frontier ahead of the real
        // paths, exhausting the allowance before any (or all `budget`)
        // legitimate paths complete; the walk then returns fewer paths (down to
        // zero) than a dead-end-free graph would. That is intended safe
        // degradation: the result is correctly flagged `truncated` and the
        // caller surfaces a `Warning`, never a runaway or a silently-incomplete
        // composite presented as complete.
        // `.max(max_depth)` covers the `budget == 0` edge case.
        let mut expansions_left = budget.saturating_mul(max_depth).max(max_depth);
        let mut truncated = false;

        self.dfs_simple_paths(
            from,
            to,
            &mut current_path,
            &mut visited,
            &mut paths,
            max_depth,
            budget,
            &mut expansions_left,
            &mut truncated,
        );

        (paths, truncated)
    }

    /// Depth-first simple-path walk with two independent termination bounds:
    ///
    /// 1. `budget` caps *completed* paths -- once `paths.len()` reaches it the
    ///    walk stops emitting and `truncated` is set. For a single-source
    ///    simple-path DFS the completed paths are emitted incrementally during
    ///    the walk, so stopping here bounds the remaining work to the current
    ///    branch's unwinding plus the dead-end frontier (bound 2).
    /// 2. `expansions_left` caps total recursive neighbor descents, so a
    ///    subgraph with no path to `target` (an exponential dead-end frontier)
    ///    cannot be explored without limit. Each descent decrements it; when it
    ///    hits zero the walk unwinds and `truncated` is set.
    ///
    /// The neighbor lists are sorted (built from `BTreeSet`-backed edges), so
    /// the emitted order -- and therefore the kept set under truncation -- is a
    /// deterministic function of the graph alone, reproducible across runs and
    /// processes.
    #[allow(clippy::too_many_arguments)]
    fn dfs_simple_paths(
        &self,
        current: &Ident<Canonical>,
        target: &Ident<Canonical>,
        path: &mut Vec<Ident<Canonical>>,
        visited: &mut HashSet<Ident<Canonical>>,
        paths: &mut Vec<Vec<Ident<Canonical>>>,
        max_depth: usize,
        budget: usize,
        expansions_left: &mut usize,
        truncated: &mut bool,
    ) {
        if path.len() > max_depth {
            return;
        }

        if let Some(neighbors) = self.edges.get(current) {
            // Walk neighbors in a canonical (sorted) order so the emitted
            // pathway order -- and therefore the kept set under truncation --
            // is a deterministic function of the graph's node identities alone,
            // independent of how the adjacency `Vec` was constructed. Production
            // edges already arrive sorted (built from `BTreeSet`), but sorting
            // here makes the determinism guarantee robust rather than an
            // implicit dependency on the caller's edge-vector ordering.
            let mut neighbors: Vec<&Ident<Canonical>> = neighbors.iter().collect();
            neighbors.sort();
            for neighbor in neighbors {
                if paths.len() >= budget {
                    // Completed-path budget reached: stop emitting and signal
                    // truncation. Returning here unwinds the recursion.
                    *truncated = true;
                    return;
                }
                if neighbor == target {
                    let mut complete_path = path.clone();
                    complete_path.push(neighbor.clone());
                    paths.push(complete_path);
                } else if !visited.contains(neighbor) {
                    if *expansions_left == 0 {
                        // Work budget exhausted on a dead-end frontier.
                        *truncated = true;
                        return;
                    }
                    *expansions_left -= 1;
                    visited.insert(neighbor.clone());
                    path.push(neighbor.clone());
                    self.dfs_simple_paths(
                        neighbor,
                        target,
                        path,
                        visited,
                        paths,
                        max_depth,
                        budget,
                        expansions_left,
                        truncated,
                    );
                    path.pop();
                    visited.remove(neighbor);
                    if *truncated {
                        // A nested call tripped a budget; unwind without
                        // exploring further siblings.
                        return;
                    }
                }
            }
        }
    }

    /// Enumerate internal pathways through a module from each input port to a
    /// specific output. Returns a map from input port name to the list of open
    /// paths. Convenience wrapper that discards the truncation signal; callers
    /// that need to surface a `Warning` use
    /// [`enumerate_module_pathways_with_truncation`](Self::enumerate_module_pathways_with_truncation).
    /// Only the truncation-aware variant has production callers (via
    /// `enumerate_pathways_to_outputs`); this plain form is retained for the
    /// graph-level tests that assert on the bare pathway map.
    #[cfg(test)]
    pub(crate) fn enumerate_module_pathways(
        &self,
        output_name: &Ident<Canonical>,
    ) -> ModulePathways {
        self.enumerate_module_pathways_with_truncation(output_name)
            .0
    }

    /// Enumerate internal pathways through a module from each input port to a
    /// specific output, returning `(pathways, truncated_ports)`.
    ///
    /// `truncated_ports` lists (sorted, for a deterministic diagnostic) the
    /// input ports whose enumeration hit the per-port pathway budget
    /// ([`module_pathway_budget`]) -- those ports' pathway lists are a
    /// deterministic prefix of the full set, not the complete enumeration, so a
    /// caller can surface a `Warning` and treat the resulting composite score as
    /// degraded (GH #649). The pathway-to-index mapping is per input port and
    /// driven by deterministic DFS order, so the kept set is reproducible across
    /// runs and processes for a fixed graph.
    pub(crate) fn enumerate_module_pathways_with_truncation(
        &self,
        output_name: &Ident<Canonical>,
    ) -> ModulePathwaysWithTruncation {
        let mut result: HashMap<Ident<Canonical>, Vec<Vec<Link>>> = HashMap::new();
        let mut truncated_ports: Vec<Ident<Canonical>> = Vec::new();

        // Compute which nodes have incoming edges within the module.
        // True input ports have no incoming edges -- they're fed from outside.
        let mut has_incoming: HashSet<Ident<Canonical>> = HashSet::new();
        for targets in self.edges.values() {
            for target in targets {
                has_incoming.insert(target.clone());
            }
        }

        // The DFS depth cap is the principled upper bound on simple-path
        // length: a simple path can visit each distinct node at most
        // once, so N nodes -> at most N entries on the path stack. The
        // visited set inside `dfs_simple_paths` already enforces simple-
        // path semantics; the cap's only remaining job is recursion-stack
        // safety, which N satisfies. The total node set is the union of
        // the adjacency-list keys (sources) and `has_incoming` (targets),
        // matching the convention in `detect_output_ports`.
        let node_count = self
            .edges
            .keys()
            .filter(|node| !has_incoming.contains(*node))
            .count()
            + has_incoming.len();

        let budget = module_pathway_budget();

        for input_port in self.edges.keys() {
            if input_port == output_name {
                continue;
            }
            // Skip intermediate variables (they have incoming edges within the module)
            if has_incoming.contains(input_port) {
                continue;
            }

            let (raw_paths, truncated) =
                self.find_all_simple_paths(input_port, output_name, node_count, budget);
            if raw_paths.is_empty() {
                continue;
            }
            if truncated {
                truncated_ports.push(input_port.clone());
            }

            let link_paths: Vec<Vec<Link>> =
                raw_paths.iter().map(|p| self.path_to_links(p)).collect();

            result.insert(input_port.clone(), link_paths);
        }

        truncated_ports.sort();
        (result, truncated_ports)
    }

    /// Enumerate internal pathways from each input port to the given output ports.
    ///
    /// Output ports are the variables that the parent model references from
    /// this sub-model (e.g., "output" for stdlib SMOOTH, or any variable
    /// name for user-defined modules). When no output ports are specified,
    /// auto-detects by looking for graph sinks (variables with no outgoing
    /// edges) and falling back to the "output" convention.
    ///
    /// Convenience wrapper that discards the truncation signal; production
    /// callers (the composite-score path and `enrich_with_module_stocks`) use
    /// the truncation-aware variant, so this plain form is retained only for the
    /// graph-level tests that assert on the bare pathway map.
    #[cfg(test)]
    pub(crate) fn enumerate_pathways_to_outputs(
        &self,
        output_ports: &[Ident<Canonical>],
    ) -> ModulePathways {
        self.enumerate_pathways_to_outputs_with_truncation(output_ports)
            .0
    }

    /// Truncation-aware sibling of
    /// [`enumerate_pathways_to_outputs`](Self::enumerate_pathways_to_outputs):
    /// returns `(pathways, truncated_ports)` where `truncated_ports` (sorted,
    /// deduped) names every input port whose enumeration hit the pathway budget
    /// for any output port. The composite-score path in `model_ltm_variables`
    /// surfaces a `Warning` from the signal; `enrich_with_module_stocks`
    /// degrades a truncated port to its "all internal stocks" fallback rather
    /// than collecting stocks from only the kept pathway prefix.
    pub(crate) fn enumerate_pathways_to_outputs_with_truncation(
        &self,
        output_ports: &[Ident<Canonical>],
    ) -> ModulePathwaysWithTruncation {
        let ports = if output_ports.is_empty() {
            self.detect_output_ports()
        } else {
            output_ports.to_vec()
        };

        let mut combined: HashMap<Ident<Canonical>, Vec<Vec<Link>>> = HashMap::new();
        let mut truncated_ports: HashSet<Ident<Canonical>> = HashSet::new();
        for output_port in &ports {
            let (pathways, truncated) = self.enumerate_module_pathways_with_truncation(output_port);
            for (input_port, paths) in pathways {
                combined.entry(input_port).or_default().extend(paths);
            }
            truncated_ports.extend(truncated);
        }
        let mut truncated_ports: Vec<Ident<Canonical>> = truncated_ports.into_iter().collect();
        truncated_ports.sort();
        (combined, truncated_ports)
    }

    /// Auto-detect output ports for a module sub-graph.
    ///
    /// Tries graph sinks (no outgoing edges) first, then falls back to
    /// the stdlib "output" convention.
    fn detect_output_ports(&self) -> Vec<Ident<Canonical>> {
        let all_nodes: HashSet<&Ident<Canonical>> = self
            .edges
            .keys()
            .chain(self.edges.values().flat_map(|tos| tos.iter()))
            .collect();

        let has_outgoing: HashSet<&Ident<Canonical>> = self
            .edges
            .iter()
            .filter(|(_, tos)| !tos.is_empty())
            .map(|(from, _)| from)
            .collect();

        let sinks: Vec<Ident<Canonical>> = all_nodes
            .iter()
            .filter(|n| !has_outgoing.contains(*n))
            .map(|n| (*n).clone())
            .collect();

        if !sinks.is_empty() {
            return sinks;
        }

        // No true sinks: all variables participate in internal feedback.
        // Fall back to the conventional "output" variable name used by
        // stdlib modules (smth1, delay1, etc.).
        let output_ident = Ident::new("output");
        if all_nodes.contains(&output_ident) {
            vec![output_ident]
        } else {
            vec![]
        }
    }

    /// Order an unordered variable set into a single elementary cycle, if one
    /// exists that visits **exactly** those variables.
    ///
    /// A modeler pins a loop by naming its variable *set* (the loop's identity
    /// is its node set, not a particular traversal); this recovers the cyclic
    /// order from the causal graph so the pinned loop can be scored like any
    /// enumerated loop. Returns the ordered node list (e.g. `[a, b, c]`
    /// representing `a -> b -> c -> a`) when the induced subgraph contains a
    /// Hamiltonian cycle over `vars`, or `None` when the named set does not
    /// form a closed cycle (a missing edge, an extra/disconnected variable, or
    /// a degenerate set of fewer than two nodes). A `None` is the signal the
    /// pin is invalid and a diagnostic should be surfaced rather than a bogus
    /// score emitted.
    ///
    /// At least one variable in a feedback loop must be a stock, but that
    /// invariant is checked by the caller (it wants to surface a precise
    /// diagnostic); this routine only verifies the closed-cycle structure.
    ///
    /// The search is a DFS that only steps along edges staying within `vars`
    /// and requires every member to be visited before closing back to the
    /// start. `vars` is bounded by the pinned loop's size (a handful of
    /// nodes in practice), so the exponential worst case of Hamiltonian-cycle
    /// search is not a concern here; a pathological pin naming dozens of
    /// densely-connected variables would still terminate because the visited
    /// set prunes revisits.
    ///
    /// # Tie-break: lex-first cycle when the set admits more than one
    ///
    /// A variable SET can admit more than one *distinct* directed Hamiltonian
    /// cycle: the complete digraph over `{a, b, c}` contains both
    /// `a -> b -> c -> a` and `a -> c -> b -> a`. The pin identifies a loop by
    /// its node set, so the two are genuinely ambiguous traversals of the same
    /// pinned set. This routine resolves the ambiguity **deterministically**:
    /// the DFS starts at the lex-smallest member and, at each step, tries
    /// in-set successors in lexicographic order, so it returns the
    /// **lex-first** Hamiltonian cycle (the one whose ordered node sequence is
    /// lexicographically smallest). This is a DEFINED, stable behavior --
    /// callers can rely on it being reproducible across runs and builds -- but
    /// note the returned ordering is one arbitrary-by-lex choice among the
    /// ambiguous traversals, not "the" canonical loop.
    ///
    /// The search walks only the top-level `self.edges` adjacency; edges that
    /// exist only inside a sub-module's recursively-built graph
    /// (`self.module_graphs`) are not traversed, so a pin whose cycle crosses a
    /// module boundary is not currently supported (it resolves to `None`).
    pub fn order_variable_cycle(
        &self,
        vars: &HashSet<Ident<Canonical>>,
    ) -> Option<Vec<Ident<Canonical>>> {
        if vars.len() < 2 {
            // A self-loop (single variable referencing itself) is not a
            // feedback loop in the SD sense; reject sets smaller than 2.
            return None;
        }

        // Deterministic start: the lex-smallest member. Every node in a valid
        // cycle has exactly one in-set successor that continues the cycle, so
        // the start choice does not change whether a cycle is found, only the
        // rotation -- which `canonical_rotation` normalizes downstream.
        let mut members: Vec<&Ident<Canonical>> = vars.iter().collect();
        members.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        let start = members[0].clone();

        let mut path: Vec<Ident<Canonical>> = vec![start.clone()];
        let mut visited: HashSet<Ident<Canonical>> = HashSet::new();
        visited.insert(start.clone());

        if self.dfs_order_cycle(&start, vars, &mut path, &mut visited) {
            Some(path)
        } else {
            None
        }
    }

    /// DFS helper for [`Self::order_variable_cycle`]. Returns `true` and leaves
    /// `path` holding the ordered cycle when a Hamiltonian cycle over `vars`
    /// is found that closes back to `path[0]`.
    fn dfs_order_cycle(
        &self,
        start: &Ident<Canonical>,
        vars: &HashSet<Ident<Canonical>>,
        path: &mut Vec<Ident<Canonical>>,
        visited: &mut HashSet<Ident<Canonical>>,
    ) -> bool {
        let current = path.last().cloned().expect("path is never empty");
        let Some(neighbors) = self.edges.get(&current) else {
            return false;
        };
        // Iterate successors in a deterministic order so the recovered cycle is
        // stable across runs (the adjacency Vec order is build-dependent).
        let mut sorted: Vec<&Ident<Canonical>> =
            neighbors.iter().filter(|n| vars.contains(*n)).collect();
        sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        sorted.dedup();

        for next in sorted {
            if next == start {
                // Closing edge: only valid once every member is on the path.
                if path.len() == vars.len() {
                    return true;
                }
                continue;
            }
            if visited.contains(next) {
                continue;
            }
            visited.insert(next.clone());
            path.push(next.clone());
            if self.dfs_order_cycle(start, vars, path, visited) {
                return true;
            }
            path.pop();
            visited.remove(next);
        }
        false
    }

    /// Build a fully-annotated [`Loop`] from an ordered cycle, assigning the
    /// supplied stable `id`.
    ///
    /// This mirrors the per-circuit body of [`Self::find_loops_with_limit`]
    /// (links + polarity + stock enrichment) but keeps the caller-chosen id
    /// instead of the `r{n}`/`b{n}`/`u{n}` auto-numbering, so a pinned loop
    /// gets a `pin{n}` id that never collides with an enumerated loop's.
    pub fn build_loop_from_cycle(&self, circuit: &[Ident<Canonical>], id: String) -> Loop {
        let links = self.circuit_to_links(circuit);
        let parent_stocks = self.find_stocks_in_loop(circuit);
        let stocks = self.enrich_with_module_stocks(circuit, parent_stocks);
        let polarity = self.calculate_polarity(&links);
        Loop {
            id,
            links,
            stocks,
            polarity,
            dimensions: vec![],
            slot_links: vec![],
        }
    }

    /// Find stocks in a loop
    pub fn find_stocks_in_loop(&self, circuit: &[Ident<Canonical>]) -> Vec<Ident<Canonical>> {
        circuit
            .iter()
            .filter(|node| self.stocks.contains(*node))
            .cloned()
            .collect()
    }

    /// Compute cycle partitions: groups of stocks connected by feedback paths.
    ///
    /// Uses BFS through the full causal graph to build stock-to-stock reachability,
    /// then Tarjan's SCC algorithm to group mutually-reachable stocks.
    pub fn compute_cycle_partitions(&self) -> CyclePartitions {
        let reachability = self.build_stock_reachability();
        let mut stock_names: Vec<Ident<Canonical>> = self.stocks.iter().cloned().collect();
        stock_names.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        let sccs = tarjan_scc(&stock_names, &reachability);

        let mut partitions: Vec<Vec<Ident<Canonical>>> = sccs
            .into_iter()
            .map(|mut scc| {
                scc.sort_by(|a, b| a.as_str().cmp(b.as_str()));
                scc
            })
            .collect();
        partitions.sort_by(|a, b| a[0].as_str().cmp(b[0].as_str()));

        let mut stock_partition = HashMap::new();
        for (idx, partition) in partitions.iter().enumerate() {
            for stock in partition {
                stock_partition.insert(stock.clone(), idx);
            }
        }

        CyclePartitions {
            partitions,
            stock_partition,
        }
    }

    /// BFS from each stock through the full causal graph to find which other
    /// stocks are reachable. Continues past intermediate stocks so that
    /// transitive reachability is captured.
    fn build_stock_reachability(&self) -> HashMap<Ident<Canonical>, HashSet<Ident<Canonical>>> {
        let mut reachability: HashMap<Ident<Canonical>, HashSet<Ident<Canonical>>> = HashMap::new();

        for stock in &self.stocks {
            let mut visited = HashSet::new();
            let mut queue = VecDeque::new();

            visited.insert(stock.clone());
            queue.push_back(stock.clone());

            while let Some(current) = queue.pop_front() {
                if let Some(neighbors) = self.edges.get(&current) {
                    for neighbor in neighbors {
                        if visited.insert(neighbor.clone()) {
                            queue.push_back(neighbor.clone());
                        }
                    }
                }
            }

            let reachable_stocks: HashSet<Ident<Canonical>> = visited
                .into_iter()
                .filter(|node| self.stocks.contains(node) && node != stock)
                .collect();
            reachability.insert(stock.clone(), reachable_stocks);
        }

        reachability
    }

    /// Return all causal links in the model with their computed polarity.
    ///
    /// This iterates over every edge in the causal graph and builds a `Link`
    /// with polarity determined by static analysis of the equation AST. Used
    /// by discovery mode to generate link score variables for ALL causal
    /// connections, not just those in known loops.
    pub fn all_links(&self) -> Vec<Link> {
        let mut links = Vec::new();
        for (from, targets) in &self.edges {
            for to in targets {
                let polarity = self.get_link_polarity(from, to);
                links.push(Link {
                    from: from.clone(),
                    to: to.clone(),
                    polarity,
                });
            }
        }
        // Sort for deterministic ordering
        links.sort_by(|a, b| {
            a.from
                .as_str()
                .cmp(b.from.as_str())
                .then_with(|| a.to.as_str().cmp(b.to.as_str()))
        });
        links
    }

    /// Get the polarity of a single link
    pub(super) fn get_link_polarity(
        &self,
        from: &Ident<Canonical>,
        to: &Ident<Canonical>,
    ) -> LinkPolarity {
        // Get the 'to' variable
        if let Some(to_var) = self.variables.get(to) {
            // Special case: flow -> stock relationships
            if let Variable::Stock {
                inflows, outflows, ..
            } = to_var
            {
                // Check if 'from' is an inflow (positive) or outflow (negative)
                if inflows.contains(from) {
                    return LinkPolarity::Positive;
                } else if outflows.contains(from) {
                    return LinkPolarity::Negative;
                }
                // If 'from' is not a flow for this stock, fall through to AST analysis
            }

            // When the target is a module, the edge represents an input
            // feeding into the module. Module inputs are direct bindings
            // (positive relationship). If the module has an internal graph,
            // we could trace through it, but for the input->module edge
            // itself the polarity is positive.
            if let Variable::Module { inputs, .. } = to_var
                && inputs.iter().any(|inp| &inp.src == from)
            {
                return LinkPolarity::Positive;
            }

            // General case: analyze the equation AST
            if let Some(ast) = to_var.ast() {
                // Analyze how 'from' appears in the equation
                return analyze_link_polarity(ast, from, &self.variables);
            }
        }
        LinkPolarity::Unknown
    }

    /// Polarity of `consumer`'s equation with respect to a reducer
    /// subexpression `reducer_subexpr_text` -- the polarity of a synthetic
    /// aggregate-node hop `$⁚ltm⁚agg → consumer` (GH #516).
    ///
    /// The aggregate node stands in for an inlined reducer that this
    /// variable-level graph has no node for, so `get_link_polarity` returns
    /// `Unknown` for it. This substitutes the reducer subexpression (matched
    /// by its canonical printed form, exactly the `AggNode::equation_text`
    /// key `enumerate_agg_nodes` records) with a bare `Var(agg_name)` in
    /// `consumer`'s equation and runs the ordinary static polarity analysis.
    /// Returns `Unknown` if `consumer` has no AST or the subexpression isn't
    /// found.
    pub(crate) fn agg_consumer_polarity(
        &self,
        consumer: &Ident<Canonical>,
        reducer_subexpr_text: &str,
        agg_name: &Ident<Canonical>,
    ) -> LinkPolarity {
        let Some(consumer_var) = self.variables.get(consumer) else {
            return LinkPolarity::Unknown;
        };
        let Some(ast) = consumer_var.ast() else {
            return LinkPolarity::Unknown;
        };
        analyze_agg_consumer_polarity(ast, reducer_subexpr_text, agg_name, &self.variables)
    }

    /// Compute per-link polarity for all edges in the causal graph.
    ///
    /// Requires `variables` to be populated for accurate results;
    /// returns `Unknown` for links whose target variable is missing.
    pub fn all_link_polarities(&self) -> HashMap<(String, String), LinkPolarity> {
        let mut result = HashMap::new();
        for (from, tos) in &self.edges {
            for to in tos {
                let polarity = self.get_link_polarity(from, to);
                result.insert((from.to_string(), to.to_string()), polarity);
            }
        }
        result
    }

    /// Calculate loop polarity based on link polarities
    pub fn calculate_polarity(&self, links: &[Link]) -> LoopPolarity {
        // If ANY link has unknown polarity, the loop is Undetermined
        let has_unknown_polarity = links
            .iter()
            .any(|link| link.polarity == LinkPolarity::Unknown);

        if has_unknown_polarity {
            return LoopPolarity::Undetermined;
        }

        // All links have known polarity - count negative links
        let negative_count = links
            .iter()
            .filter(|link| link.polarity == LinkPolarity::Negative)
            .count();

        // Even number of negative links = Reinforcing
        // Odd number of negative links = Balancing
        if negative_count % 2 == 0 {
            LoopPolarity::Reinforcing
        } else {
            LoopPolarity::Balancing
        }
    }

    /// Assign deterministic IDs to loops based on their content
    pub(super) fn assign_deterministic_loop_ids(&self, loops: &mut [Loop]) {
        assign_loop_ids(loops);
    }
}

/// Collect stocks from a set of internal pathways, namespaced with the
/// module instance name (using interpunct separator).
fn collect_stocks_from_pathways(
    module_graph: &CausalGraph,
    paths: &[Vec<Link>],
    module_name: &Ident<Canonical>,
) -> Vec<Ident<Canonical>> {
    let mut internal_stocks = Vec::new();
    for path in paths {
        for link in path {
            for node in [&link.from, &link.to] {
                if module_graph.stocks.contains(node) {
                    let qualified = Ident::new(&format!(
                        "{}\u{00B7}{}",
                        module_name.as_str(),
                        node.as_str()
                    ));
                    if !internal_stocks.contains(&qualified) {
                        internal_stocks.push(qualified);
                    }
                }
            }
        }
    }
    internal_stocks
}

/// Fallback: collect ALL stocks from a module's internal graph, namespaced
/// with the module instance name.
fn all_module_stocks(
    module_graph: &CausalGraph,
    module_name: &Ident<Canonical>,
) -> Vec<Ident<Canonical>> {
    let mut stocks: Vec<_> = module_graph
        .stocks
        .iter()
        .map(|s| Ident::new(&format!("{}\u{00B7}{}", module_name.as_str(), s.as_str())))
        .collect();
    stocks.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    stocks
}

/// Content-derived sort key that fully orders any set of distinct loops,
/// including sibling cycles over the same node set (GH #497).
///
/// The key is a tuple `(variable_set, edge_sequence)`:
///
/// - **Primary -- the deduped, sorted variable set** (`vars.join("_")`).  This
///   is the historical key.  Keeping it as the leading component means a model
///   whose loops all have distinct variable sets (the common single-direction
///   case) keeps its existing `r{n}`/`b{n}`/`u{n}` numbering bit-for-bit: the
///   primary already orders those loops totally, so the secondary key is never
///   consulted and no IDs churn.
///
/// - **Secondary -- the canonical cyclic rotation of the directed edge
///   sequence** (`canonical_rotation` over each `link.from`).  Two sibling
///   cycles over the same node set -- `a -> b -> c -> a` and
///   `a -> c -> b -> a` on a multidigraph -- share the primary key, so before
///   this tiebreaker `sort_by_key`'s stable fallback preserved whatever order
///   Johnson's enumerator emitted them in.  That order depends on `HashMap`
///   iteration in [`crate::ltm::indexed::IndexedGraph::from_edges`], which is
///   deterministic within one process but per-process-randomized across
///   processes (Rust's `DefaultHasher` reseeds per process), so the same
///   model could assign `r3` to the forward cycle in one run and the reverse
///   cycle in the next -- flapping the IDs that persisted UI state, loop
///   pinning (`SetLoopName`/`LoopMetadata`), and deterministic tests key on.
///   The canonical edge-sequence rotation differs between siblings (it is the
///   exact key the dedup in `find_loops_with_limit` guarantees is unique per
///   loop -- raw `link.from`, *not* the stripped variable name, so it also
///   distinguishes per-element cross-element loops on arrayed models), so it
///   fully orders any tied group, making `assign_loop_ids` a pure function of
///   loop content rather than enumeration order.
fn loop_id_sort_key(loop_item: &Loop) -> (String, Vec<String>) {
    let mut vars: Vec<String> = loop_item
        .links
        .iter()
        .flat_map(|link| vec![link.from.as_str().to_string(), link.to.as_str().to_string()])
        .collect();
    vars.sort();
    vars.dedup();
    let primary = vars.join("_");

    let edge_seq: Vec<String> = loop_item
        .links
        .iter()
        .map(|link| link.from.as_str().to_string())
        .collect();
    let secondary = super::canonical_rotation(&edge_seq);

    (primary, secondary)
}

/// Assign deterministic IDs to loops based on their polarity and content.
/// Standalone function for use by tracked functions in db.rs.
pub(crate) fn assign_loop_ids(loops: &mut [Loop]) {
    loops.sort_by_cached_key(loop_id_sort_key);

    let mut r_counter = 1;
    let mut b_counter = 1;
    let mut u_counter = 1;

    for loop_item in loops.iter_mut() {
        // ID prefix is decided by the dominant polarity. MostlyReinforcing
        // and MostlyBalancing share the r/b counters with their pure
        // counterparts because user-facing IDs and downstream consumers
        // (UI legend, persisted loop names) treat them as "Rux"/"Bux"
        // variants of R/B rather than as a distinct namespace.
        loop_item.id = match loop_item.polarity {
            LoopPolarity::Reinforcing | LoopPolarity::MostlyReinforcing => {
                let id = format!("r{r_counter}");
                r_counter += 1;
                id
            }
            LoopPolarity::Balancing | LoopPolarity::MostlyBalancing => {
                let id = format!("b{b_counter}");
                b_counter += 1;
                id
            }
            LoopPolarity::Undetermined => {
                let id = format!("u{u_counter}");
                u_counter += 1;
                id
            }
        };
    }
}

/// Debug-only helper: verify every `Loop` has a distinct canonical
/// edge-sequence rotation.  Used by `debug_assert!` in
/// `find_loops_with_limit` to guard against future regressions of the
/// inline dedup in `johnson_circuit`.  Two loops over the same node
/// set that differ in directed traversal (e.g. forward vs reverse
/// 3-cycle on a multidigraph) have **different** canonical rotations
/// and are correctly retained as distinct loops -- this check fires
/// only on accidental re-emission of the same directed cycle.  Release
/// builds compile the call out via `debug_assert!`'s no-op expansion.
fn loops_have_unique_canonical_rotations(loops: &[Loop]) -> bool {
    let mut seen: HashSet<Vec<&str>> = HashSet::with_capacity(loops.len());
    for loop_item in loops {
        let path: Vec<&str> = loop_item.links.iter().map(|l| l.from.as_str()).collect();
        let key = super::canonical_rotation(&path);
        if !seen.insert(key) {
            return false;
        }
    }
    true
}
