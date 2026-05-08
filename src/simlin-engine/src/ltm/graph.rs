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
use super::polarity::analyze_link_polarity;
use super::types::{
    Link, LinkPolarity, Loop, LoopPolarity, ModuleLtmRole, TruncatedByBudget,
    classify_module_for_ltm, normalize_module_ref,
};

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
                });
            }
        }

        // The per-SCC inline dedup in `johnson_circuit` already rejects
        // multidigraph duplicates at the circuit (Vec<u32>) level, so
        // every circuit that reaches this point has a unique node-set.
        // A separate Loop-level dedup would be redundant; debug builds
        // verify the invariant so a future regression trips a test.
        debug_assert!(
            loops_have_unique_node_sets(&loops),
            "circuit enumerator must emit unique node-sets; duplicate loops reached find_loops_with_limit"
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

        // Each SCC's enumerator already deduplicates by sorted node-set,
        // and different SCCs share no nodes -- so `all_circuits` has no
        // cross-SCC duplicates.  Debug builds verify the invariant.
        debug_assert!(
            IndexedGraph::has_no_duplicate_node_sets(&all_circuits),
            "enumerate_circuits_in_scc should emit unique node-sets per SCC"
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
    fn enrich_with_module_stocks(
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

            let pathways = module_graph.enumerate_pathways_to_outputs(&[]);

            let internal_stocks: Vec<Ident<Canonical>> = if let Some(port) = internal_port {
                // Collect stocks from all pathways for the matched input port.
                if let Some(paths) = pathways.get(port) {
                    collect_stocks_from_pathways(module_graph, paths, node)
                } else {
                    // Port found in module inputs but no pathway exists for
                    // it -- fall back to all module-internal stocks.
                    all_module_stocks(module_graph, node)
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
    /// Returns sequences of node idents forming open paths.
    pub(crate) fn find_all_simple_paths(
        &self,
        from: &Ident<Canonical>,
        to: &Ident<Canonical>,
        max_depth: usize,
    ) -> Vec<Vec<Ident<Canonical>>> {
        let mut paths = Vec::new();
        let mut current_path = vec![from.clone()];
        let mut visited = HashSet::new();
        visited.insert(from.clone());

        self.dfs_simple_paths(
            from,
            to,
            &mut current_path,
            &mut visited,
            &mut paths,
            max_depth,
        );

        paths
    }

    fn dfs_simple_paths(
        &self,
        current: &Ident<Canonical>,
        target: &Ident<Canonical>,
        path: &mut Vec<Ident<Canonical>>,
        visited: &mut HashSet<Ident<Canonical>>,
        paths: &mut Vec<Vec<Ident<Canonical>>>,
        max_depth: usize,
    ) {
        if path.len() > max_depth {
            return;
        }

        if let Some(neighbors) = self.edges.get(current) {
            for neighbor in neighbors {
                if neighbor == target {
                    let mut complete_path = path.clone();
                    complete_path.push(neighbor.clone());
                    paths.push(complete_path);
                } else if !visited.contains(neighbor) {
                    visited.insert(neighbor.clone());
                    path.push(neighbor.clone());
                    self.dfs_simple_paths(neighbor, target, path, visited, paths, max_depth);
                    path.pop();
                    visited.remove(neighbor);
                }
            }
        }
    }

    /// Enumerate internal pathways through a module from each input port to a
    /// specific output. Returns a map from input port name to the list of open paths.
    pub(crate) fn enumerate_module_pathways(
        &self,
        output_name: &Ident<Canonical>,
    ) -> HashMap<Ident<Canonical>, Vec<Vec<Link>>> {
        let mut result: HashMap<Ident<Canonical>, Vec<Vec<Link>>> = HashMap::new();

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

        for input_port in self.edges.keys() {
            if input_port == output_name {
                continue;
            }
            // Skip intermediate variables (they have incoming edges within the module)
            if has_incoming.contains(input_port) {
                continue;
            }

            let raw_paths = self.find_all_simple_paths(input_port, output_name, node_count);
            if raw_paths.is_empty() {
                continue;
            }

            let link_paths: Vec<Vec<Link>> =
                raw_paths.iter().map(|p| self.path_to_links(p)).collect();

            result.insert(input_port.clone(), link_paths);
        }

        result
    }

    /// Enumerate internal pathways from each input port to the given output ports.
    ///
    /// Output ports are the variables that the parent model references from
    /// this sub-model (e.g., "output" for stdlib SMOOTH, or any variable
    /// name for user-defined modules). When no output ports are specified,
    /// auto-detects by looking for graph sinks (variables with no outgoing
    /// edges) and falling back to the "output" convention.
    pub(crate) fn enumerate_pathways_to_outputs(
        &self,
        output_ports: &[Ident<Canonical>],
    ) -> HashMap<Ident<Canonical>, Vec<Vec<Link>>> {
        let ports = if output_ports.is_empty() {
            self.detect_output_ports()
        } else {
            output_ports.to_vec()
        };

        let mut combined: HashMap<Ident<Canonical>, Vec<Vec<Link>>> = HashMap::new();
        for output_port in &ports {
            let pathways = self.enumerate_module_pathways(output_port);
            for (input_port, paths) in pathways {
                combined.entry(input_port).or_default().extend(paths);
            }
        }
        combined
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

/// Assign deterministic IDs to loops based on their polarity and content.
/// Standalone function for use by tracked functions in db.rs.
pub(crate) fn assign_loop_ids(loops: &mut [Loop]) {
    loops.sort_by_key(|loop_item| {
        let mut vars: Vec<String> = loop_item
            .links
            .iter()
            .flat_map(|link| vec![link.from.as_str().to_string(), link.to.as_str().to_string()])
            .collect();
        vars.sort();
        vars.dedup();
        vars.join("_")
    });

    let mut r_counter = 1;
    let mut b_counter = 1;
    let mut u_counter = 1;

    for loop_item in loops.iter_mut() {
        loop_item.id = match loop_item.polarity {
            LoopPolarity::Reinforcing => {
                let id = format!("r{r_counter}");
                r_counter += 1;
                id
            }
            LoopPolarity::Balancing => {
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

/// Debug-only helper: verify every `Loop` has a distinct node-set.
/// Used by `debug_assert!` in `find_loops_with_limit` to guard against
/// future regressions of the inline dedup in `johnson_circuit`.  Release
/// builds compile the call out via `debug_assert!`'s no-op expansion.
fn loops_have_unique_node_sets(loops: &[Loop]) -> bool {
    let mut seen: HashSet<Vec<&str>> = HashSet::with_capacity(loops.len());
    for loop_item in loops {
        let mut key: Vec<&str> = loop_item.links.iter().map(|l| l.from.as_str()).collect();
        key.sort_unstable();
        if !seen.insert(key) {
            return false;
        }
    }
    true
}
