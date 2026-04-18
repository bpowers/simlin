// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loops That Matter (LTM) implementation for loop dominance analysis

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ast::{Ast, BinaryOp, Expr2, IndexExpr2};
use crate::common::{Canonical, Ident, Result};
use crate::model::ModelStage1;
use crate::project::Project;
use crate::variable::{Variable, identifier_set};

/// Safety cap on the number of elementary circuits the causal-graph DFS
/// will enumerate before bailing out.
///
/// After the reduce-ltm-mem branch brought wrld3's uncapped enumeration
/// memory from ~8.5 GiB down to ~490 MiB and cut wall time to ~1.2 s
/// (Johnson's), the cap is no longer needed to keep enumeration itself
/// inside WASM's 4 GiB linear memory.  It still serves a second
/// purpose: preventing the *downstream* LTM variable-generation pipeline
/// (~2 synthetic variables per loop) from blowing up when the circuit
/// count is in the millions.  wrld3 has ~1.86M elementary circuits;
/// generating ~3.7M synthetic variables OOMs the compiler even though
/// enumeration itself is fast and small.
///
/// Removing this cap cleanly would require first capping the synthetic
/// variable count inside `model_ltm_variables`, or auto-falling back
/// to discovery mode for very large models -- neither of which is on
/// this branch's scope.
pub(crate) const MAX_LTM_CIRCUITS: usize = 100_000;

/// Marker returned by circuit-enumeration helpers when the DFS bailed
/// out because it would have exceeded the [`MAX_LTM_CIRCUITS`] budget.
/// The caller should treat this as "LTM analysis skipped for this
/// model" and emit a user-visible diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruncatedByBudget;

/// Polarity of a causal link
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkPolarity {
    Positive, // Increase in 'from' causes increase in 'to'
    Negative, // Increase in 'from' causes decrease in 'to'
    Unknown,  // Cannot determine polarity statically
}

/// Represents a causal link between two variables
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Link {
    pub from: Ident<Canonical>,
    pub to: Ident<Canonical>,
    pub polarity: LinkPolarity,
}

/// Represents a feedback loop.
///
/// For scalar models, `dimensions` is empty and links reference scalar
/// variable names.  For arrayed models, a pure-dimension A2A loop has
/// `dimensions` set to the shared dimension names (e.g., `["Region"]`)
/// and links reference variable-level names (the A2A expansion handles
/// per-element evaluation).  Mixed loops (scalar + arrayed nodes, or
/// cross-element feedback) have empty `dimensions` and use
/// element-specific link names.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Loop {
    pub id: String,
    pub links: Vec<Link>,
    pub stocks: Vec<Ident<Canonical>>,
    pub polarity: LoopPolarity,
    /// Dimension names for A2A loop scores. Empty for scalar or mixed loops.
    pub dimensions: Vec<String>,
}

impl Loop {
    /// Format the loop as a string showing the variable path
    pub fn format_path(&self) -> String {
        if self.links.is_empty() {
            return String::new();
        }

        // Build the path by following links
        let mut path = Vec::new();
        let current = &self.links[0].from;
        path.push(current.as_str());

        for link in &self.links {
            path.push(link.to.as_str());
        }

        path.join(" -> ")
    }
}

/// Loop polarity classification
///
/// The structural polarity is determined by counting negative links:
/// - Even number of negative links → Reinforcing
/// - Odd number of negative links → Balancing
/// - ANY link with unknown polarity → Undetermined
///
/// At runtime, if the loop score changes sign during simulation, the polarity
/// is also classified as Undetermined.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum LoopPolarity {
    /// R loop - amplifies changes (positive loop score)
    /// Structurally: even number of negative links
    Reinforcing,
    /// B loop - counteracts changes (negative loop score)
    /// Structurally: odd number of negative links
    Balancing,
    /// U loop - polarity cannot be determined or changes during simulation
    /// Structurally: any link has unknown polarity
    /// At runtime: loop score has both positive and negative values
    Undetermined,
}

impl LoopPolarity {
    /// Classify loop polarity based on actual runtime loop score values.
    ///
    /// This function examines the loop score values from a simulation run
    /// and determines the appropriate polarity:
    /// - All valid (non-NaN, non-zero) scores positive → Reinforcing
    /// - All valid scores negative → Balancing
    /// - Mix of positive and negative → Undetermined
    /// - No valid scores → returns None
    pub fn from_runtime_scores(scores: &[f64]) -> Option<Self> {
        let valid_scores: Vec<f64> = scores
            .iter()
            .copied()
            .filter(|v| !v.is_nan() && *v != 0.0)
            .collect();

        if valid_scores.is_empty() {
            return None;
        }

        let has_positive = valid_scores.iter().any(|v| *v > 0.0);
        let has_negative = valid_scores.iter().any(|v| *v < 0.0);

        match (has_positive, has_negative) {
            (true, false) => Some(LoopPolarity::Reinforcing),
            (false, true) => Some(LoopPolarity::Balancing),
            (true, true) => Some(LoopPolarity::Undetermined),
            (false, false) => None, // All zeros after filtering
        }
    }

    /// Returns the conventional single-letter abbreviation for this polarity
    pub fn abbreviation(&self) -> &'static str {
        match self {
            LoopPolarity::Reinforcing => "R",
            LoopPolarity::Balancing => "B",
            LoopPolarity::Undetermined => "U",
        }
    }
}

/// Classification of a module's role in LTM analysis.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleLtmRole {
    /// Has internal stocks (SMOOTH, DELAY, TREND, user-defined modules with stocks)
    DynamicModule,
    /// No internal stocks -- pure passthrough
    Passthrough,
}

/// Classify a module model for LTM analysis.
///
/// Dynamic modules contain stocks and need composite link scores.
/// Stateless modules are passthroughs.
pub(crate) fn classify_module_for_ltm(module_model: &ModelStage1) -> ModuleLtmRole {
    if module_model
        .variables
        .values()
        .any(|v| matches!(v, Variable::Stock { .. }))
    {
        ModuleLtmRole::DynamicModule
    } else {
        ModuleLtmRole::Passthrough
    }
}

/// Normalize a module·output reference to just the module node.
/// E.g., "$⁚s⁚0⁚smth1·output" becomes "$⁚s⁚0⁚smth1".
/// Non-module references are returned unchanged.
pub(crate) fn normalize_module_ref(ident: &Ident<Canonical>) -> Ident<Canonical> {
    let s = ident.as_str();
    if let Some(pos) = s.find('\u{00B7}') {
        Ident::new(&s[..pos])
    } else {
        ident.clone()
    }
}

/// Get direct dependencies from a Variable
fn get_variable_dependencies(var: &Variable) -> Vec<Ident<Canonical>> {
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

/// Cycle partitions: groups of stocks connected by feedback loops.
///
/// Two stocks are in the same partition if they are mutually reachable
/// through the causal graph (i.e., they form a strongly connected component
/// in the stock-to-stock reachability graph). Relative loop scores should
/// only be compared within the same partition.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct CyclePartitions {
    /// Partitions as sorted Vecs of stock idents.
    /// Outer Vec is sorted by the lexicographically smallest stock in each partition.
    pub partitions: Vec<Vec<Ident<Canonical>>>,
    /// Stock -> partition index (into `partitions`).
    pub stock_partition: HashMap<Ident<Canonical>, usize>,
}

impl CyclePartitions {
    /// Look up the partition index for a loop using its parent-level stocks.
    /// Returns None for loops with no parent-level stocks.
    ///
    /// Module-internal stocks (namespaced with interpunct, e.g.
    /// `smooth·smoothed`) are implicitly in the same partition as the parent
    /// stocks they coexist with in a loop, but they don't appear in the
    /// partition map since partitions are computed on the parent graph.
    ///
    /// All *parent-level* stocks in a feedback loop are guaranteed to be in the
    /// same SCC: if a loop passes through stocks A and B, there are paths
    /// A->B and B->A through the loop, making them mutually reachable.
    pub fn partition_for_loop(&self, loop_item: &Loop) -> Option<usize> {
        let result = loop_item
            .stocks
            .iter()
            .find_map(|s| self.stock_partition.get(s).copied());
        debug_assert!(
            loop_item
                .stocks
                .iter()
                .filter_map(|s| self.stock_partition.get(s).copied())
                .all(|p| Some(p) == result),
            "all parent-level stocks in a loop must be in the same partition"
        );
        result
    }
}

/// Standard Tarjan's SCC algorithm on a directed graph of stock nodes.
///
/// Takes a deterministically-ordered list of stock nodes and a reachability
/// map, returns strongly connected components.
fn tarjan_scc(
    nodes: &[Ident<Canonical>],
    reachability: &HashMap<Ident<Canonical>, HashSet<Ident<Canonical>>>,
) -> Vec<Vec<Ident<Canonical>>> {
    struct TarjanState {
        index_counter: usize,
        stack: Vec<Ident<Canonical>>,
        on_stack: HashSet<Ident<Canonical>>,
        indices: HashMap<Ident<Canonical>, usize>,
        lowlinks: HashMap<Ident<Canonical>, usize>,
        sccs: Vec<Vec<Ident<Canonical>>>,
    }

    fn strongconnect(
        v: &Ident<Canonical>,
        reachability: &HashMap<Ident<Canonical>, HashSet<Ident<Canonical>>>,
        state: &mut TarjanState,
    ) {
        state.indices.insert(v.clone(), state.index_counter);
        state.lowlinks.insert(v.clone(), state.index_counter);
        state.index_counter += 1;
        state.stack.push(v.clone());
        state.on_stack.insert(v.clone());

        if let Some(neighbors) = reachability.get(v) {
            let mut sorted_neighbors: Vec<_> = neighbors.iter().collect();
            sorted_neighbors.sort_by(|a, b| a.as_str().cmp(b.as_str()));

            for w in sorted_neighbors {
                if !state.indices.contains_key(w) {
                    strongconnect(w, reachability, state);
                    let w_lowlink = state.lowlinks[w];
                    let v_lowlink = state.lowlinks.get_mut(v).unwrap();
                    if w_lowlink < *v_lowlink {
                        *v_lowlink = w_lowlink;
                    }
                } else if state.on_stack.contains(w) {
                    let w_index = state.indices[w];
                    let v_lowlink = state.lowlinks.get_mut(v).unwrap();
                    if w_index < *v_lowlink {
                        *v_lowlink = w_index;
                    }
                }
            }
        }

        if state.lowlinks[v] == state.indices[v] {
            let mut scc = Vec::new();
            loop {
                let w = state.stack.pop().unwrap();
                state.on_stack.remove(&w);
                scc.push(w.clone());
                if &w == v {
                    break;
                }
            }
            state.sccs.push(scc);
        }
    }

    let mut state = TarjanState {
        index_counter: 0,
        stack: Vec::new(),
        on_stack: HashSet::new(),
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        sccs: Vec::new(),
    };

    for node in nodes {
        if !state.indices.contains_key(node) {
            strongconnect(node, reachability, &mut state);
        }
    }

    state.sccs
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

/// Compact integer-indexed view of an adjacency list used to accelerate the
/// core DFS circuit enumeration.  Swapping `Ident<Canonical>` (heap-allocated
/// `String`) for a `u32` index slashes per-visit cost in three places:
///
/// * `visited` becomes a `Vec<bool>` of length `nodes.len()` instead of a
///   `HashSet<Ident<Canonical>>`.
/// * `path` becomes a `Vec<u32>` instead of `Vec<Ident<Canonical>>`, so each
///   push/pop is a trivial `u32` copy rather than a `String` clone/drop.
/// * Circuit dedup keys are `Vec<u32>` sorted, not joined strings.
///
/// Nodes are sorted lexicographically so that "neighbor.as_str() >=
/// start.as_str()" is equivalent to "neighbor_idx >= start_idx", preserving
/// the small-start invariant that makes each elementary circuit emerge
/// exactly once from its lex-smallest node.
struct IndexedGraph {
    /// Sorted node identities; index into this vec is a `NodeIdx` (u32).
    nodes: Vec<Ident<Canonical>>,
    /// Successor indices per node, each inner Vec sorted ascending.
    succ: Vec<Vec<u32>>,
    /// Reverse map for translating external `Ident<Canonical>`s to indices.
    /// Retained on the struct so callers that hold an IndexedGraph can map
    /// caller-supplied idents to indices without rebuilding the map; in the
    /// current call sites it is only exercised via tests.
    #[allow(dead_code)]
    node_to_idx: HashMap<Ident<Canonical>, u32>,
}

/// Marker that DFS exceeded the shared circuit budget.  Used internally by
/// `enumerate_circuits_in_scc` to propagate bail-out without confusion with
/// a successful empty-result enumeration.
#[derive(Debug)]
struct TruncatedByBudgetInternal;

/// Result bundle from the shared indexed-circuit enumerator: the
/// IndexedGraph owns the node table (so callers can map indices back to
/// `Ident<Canonical>` on demand), and `circuits` is already deduplicated
/// by sorted-node-set.
struct IndexedCircuits {
    graph: IndexedGraph,
    circuits: Vec<Vec<u32>>,
}

/// State for the per-SCC Tiernan DFS (kept as a test oracle -- the
/// production path now uses Johnson's, see [`JohnsonState`]).
#[cfg(test)]
struct DfsState {
    visited: Vec<bool>,
    path: Vec<u32>,
    circuits: Vec<Vec<u32>>,
    in_scc: Vec<bool>,
}

/// State for the per-SCC Johnson 1975 circuit enumerator.
///
/// The core difference from Tiernan: nodes are `blocked` when entered on
/// the DFS stack and STAY blocked on backtrack if their subtree didn't
/// close a cycle.  Only when a cycle is later discovered through them --
/// or through something they were waiting on (tracked via `b_list`) --
/// are they unblocked for re-exploration.  This avoids Tiernan's repeated
/// traversal of dead-end subtrees from different parent branches.
///
/// `blocked` and `b_list` are reset for every start node within an SCC,
/// but `in_scc` is fixed for the whole SCC.  `path` and `circuits` are
/// reused across starts to amortize allocation.
struct JohnsonState {
    blocked: Vec<bool>,
    b_list: Vec<Vec<u32>>,
    path: Vec<u32>,
    in_scc: Vec<bool>,
    circuits: Vec<Vec<u32>>,
    /// 64-bit rapidhash fingerprints of already-emitted circuits (by
    /// sorted node-index set).  Lets us reject duplicate node-set
    /// circuits during enumeration so peak memory stays proportional to
    /// unique output, not to the number of raw DFS emissions.  Shared
    /// across all DFS starts within a single SCC (different SCCs have
    /// disjoint node sets, so no cross-SCC collision is possible).
    seen: HashSet<u64>,
    /// Scratch buffer for hashing: reused rather than allocated per-call.
    hash_scratch: Vec<u32>,
}

/// Fixed seed for the LTM circuit fingerprint hash.
///
/// rapidhash mixes this into the state alongside the default secret;
/// the only property we care about is that it's non-zero and stable
/// across runs, because the resulting fingerprint set is stored in a
/// salsa-cached `LoopCircuitsResult`.  Changing this value invalidates
/// every cached circuit-enumeration result (the hashes change even
/// though the underlying circuits don't), so treat it as a schema-level
/// constant and bump it only when that invalidation is desired.
const CIRCUIT_HASH_SEED: u64 = 0xabcdef0123456789;

/// Deterministic 64-bit rapidhash V3 (HashMicro variant) fingerprint
/// over a slice of `u32`s, hashed as their little-endian byte
/// representation.
///
/// rapidhash replaces an earlier u32-wise FNV-1a implementation.  At
/// the LTM hot path's size distribution (mean ~188 bytes, max 320
/// bytes) the `HashMicro` variant sustains ~34 GiB/s on x86-64 vs.
/// FNV-1a's ~10 GiB/s, a 3.5x throughput improvement per-hash.  On the
/// wrld3 `ltm_mem_bench` the end-to-end enumeration speedup is ~4%
/// (hashing is a small but non-negligible share of total time; the
/// rest is Johnson's DFS, Vec/HashSet bookkeeping, and sort_unstable
/// on the scratch buffer).  See `benches/rapidhash_bench.rs`.
///
/// rapidhash is preferred here for correctness as much as speed:
///   * It is deterministic given a fixed seed and secret, which matters
///     because `LoopCircuitsResult` is a salsa cache value -- a
///     randomized hasher could silently reshuffle surviving circuits on
///     a rare collision and invalidate the cache even when the input
///     didn't change.  We pass `CIRCUIT_HASH_SEED` (a fixed non-zero
///     constant) and the default rapidhash secret from the port.
///   * It has strong avalanche behavior: single-bit input flips spread
///     across ~32 output bits, so leading-zero elements and common
///     prefixes do not collapse into the seed the way they do with
///     naive multiplicative hashes (the old FNV-1a callout spelled this
///     out explicitly for the `[0, x, y]` vs `[x, y]` case).
///   * Collision probability over wrld3's 1.86M circuits sits at the
///     2^64 birthday bound (~5e-8) -- identical to the FNV-1a tradeoff
///     the callsite already accepted, and dramatically better than the
///     ~400 MiB transient footprint of keying the HashSet on
///     `Vec<u32>`.
///
/// Callers pass an already-sorted slice (Johnson's algorithm emits
/// rotations of the same cycle; sorting makes those collide).
#[inline]
fn hash_u32_slice(vals: &[u32]) -> u64 {
    crate::rapidhash::hash_u32_slice(vals, CIRCUIT_HASH_SEED)
}

impl IndexedGraph {
    /// Build an IndexedGraph from a `CausalGraph`-style adjacency list.
    ///
    /// Every node referenced as either an edge source or target is assigned
    /// an index.  Successor indices are de-duplicated and sorted so the DFS
    /// can early-exit on the first out-of-range neighbor when needed.
    fn from_edges(edges: &HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>) -> Self {
        let mut node_set: HashSet<&Ident<Canonical>> = HashSet::new();
        for (from, tos) in edges {
            node_set.insert(from);
            for to in tos {
                node_set.insert(to);
            }
        }
        let mut nodes: Vec<Ident<Canonical>> = node_set.into_iter().cloned().collect();
        nodes.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        let mut node_to_idx: HashMap<Ident<Canonical>, u32> = HashMap::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            node_to_idx.insert(n.clone(), i as u32);
        }

        let mut succ: Vec<Vec<u32>> = vec![Vec::new(); nodes.len()];
        for (from, tos) in edges {
            let fi = node_to_idx[from] as usize;
            for to in tos {
                let ti = node_to_idx[to];
                succ[fi].push(ti);
            }
            // Dedup identical successor entries (the input may contain them
            // if an edge was inserted twice during causal-graph construction)
            // and sort so the DFS sees the same deterministic order as the
            // old Ident-based iteration produced via lex sorting.
            succ[fi].sort_unstable();
            succ[fi].dedup();
        }

        IndexedGraph {
            nodes,
            succ,
            node_to_idx,
        }
    }

    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Iterative Tarjan's algorithm on the full graph.  Iterative form keeps
    /// WRLD3-style graphs (>300 nodes, long cycles) off the recursion limit.
    /// SCCs are returned in first-discovery order; each inner `Vec<u32>` is
    /// sorted ascending so downstream iteration is deterministic regardless
    /// of the order nodes were popped from Tarjan's stack.
    fn tarjan_scc(&self) -> Vec<Vec<u32>> {
        const UNVISITED: i32 = -1;
        let n = self.nodes.len();
        let mut indices: Vec<i32> = vec![UNVISITED; n];
        let mut lowlinks: Vec<i32> = vec![0; n];
        let mut on_stack: Vec<bool> = vec![false; n];
        let mut stack: Vec<u32> = Vec::new();
        let mut sccs: Vec<Vec<u32>> = Vec::new();
        let mut next_index: i32 = 0;

        // Iterative frames: Enter pushes a node onto Tarjan's stack; Resume
        // continues iterating its successors (and ultimately pops the SCC
        // if this node is its own root).
        enum Frame {
            Enter(u32),
            Resume { v: u32, next_child: u32 },
        }

        for start in 0..n as u32 {
            if indices[start as usize] != UNVISITED {
                continue;
            }
            let mut frames: Vec<Frame> = vec![Frame::Enter(start)];
            while let Some(frame) = frames.pop() {
                match frame {
                    Frame::Enter(v) => {
                        indices[v as usize] = next_index;
                        lowlinks[v as usize] = next_index;
                        next_index += 1;
                        stack.push(v);
                        on_stack[v as usize] = true;
                        frames.push(Frame::Resume { v, next_child: 0 });
                    }
                    Frame::Resume { v, next_child } => {
                        let succs = &self.succ[v as usize];
                        if (next_child as usize) < succs.len() {
                            let w = succs[next_child as usize];
                            frames.push(Frame::Resume {
                                v,
                                next_child: next_child + 1,
                            });
                            if indices[w as usize] == UNVISITED {
                                frames.push(Frame::Enter(w));
                            } else if on_stack[w as usize]
                                && indices[w as usize] < lowlinks[v as usize]
                            {
                                lowlinks[v as usize] = indices[w as usize];
                            }
                        } else {
                            // All children processed; propagate this node's
                            // lowlink up to its parent frame (if any) before
                            // potentially emitting an SCC rooted at v.
                            if let Some(Frame::Resume {
                                v: parent,
                                next_child: _,
                            }) = frames.last()
                                && lowlinks[v as usize] < lowlinks[*parent as usize]
                            {
                                lowlinks[*parent as usize] = lowlinks[v as usize];
                            }
                            if lowlinks[v as usize] == indices[v as usize] {
                                let mut scc = Vec::new();
                                loop {
                                    let w = stack.pop().unwrap();
                                    on_stack[w as usize] = false;
                                    scc.push(w);
                                    if w == v {
                                        break;
                                    }
                                }
                                scc.sort_unstable();
                                sccs.push(scc);
                            }
                        }
                    }
                }
            }
        }

        sccs
    }

    /// Enumerate elementary circuits inside a single SCC using Johnson 1975.
    ///
    /// Only edges whose target is inside the current SCC with index >=
    /// `start` are followed, preserving the "each cycle emitted from its
    /// lex-smallest node" invariant that downstream code
    /// (`enrich_with_module_stocks`, `assign_loop_ids`, deterministic
    /// salsa caching) depends on.
    ///
    /// The algorithm's efficiency vs. Tiernan-style lexicographic restart
    /// comes from the blocked-set mechanism: once a node is visited on
    /// the DFS stack it is `blocked`, and stays blocked on backtrack
    /// unless a cycle was discovered through it.  Nodes that fail to
    /// close a cycle register themselves as waiters in their successors'
    /// `b_list`s; they're unblocked transitively when any of those
    /// successors later participates in a cycle.  This avoids repeated
    /// exploration of dead-end subtrees that Tiernan can fall into.
    ///
    /// `budget` is the **remaining** circuit budget; it is decremented
    /// each time a circuit is emitted.  Returns
    /// `Err(TruncatedByBudgetInternal)` the moment the budget would go
    /// negative so the caller can stop immediately.
    fn enumerate_circuits_in_scc(
        &self,
        scc: &[u32],
        budget: &mut usize,
    ) -> std::result::Result<Vec<Vec<u32>>, TruncatedByBudgetInternal> {
        if scc.is_empty() {
            return Ok(Vec::new());
        }

        // Size-1 SCC: the only possible circuit is a pure self-loop, and
        // those are excluded by the `path.len() > 1` contract (a circuit
        // is encoded as the list of stack nodes without the closing edge,
        // so a length-1 list represents a self-loop we do not emit).
        if scc.len() == 1 {
            return Ok(Vec::new());
        }

        let n = self.nodes.len();
        let mut state = JohnsonState {
            blocked: vec![false; n],
            b_list: vec![Vec::new(); n],
            path: Vec::with_capacity(scc.len()),
            in_scc: vec![false; n],
            circuits: Vec::new(),
            seen: HashSet::new(),
            hash_scratch: Vec::with_capacity(scc.len()),
        };
        for &v in scc {
            state.in_scc[v as usize] = true;
        }

        for &start in scc {
            // Reset blocked/B[] for this start's search.  Reusing the
            // allocations across starts saves a per-start Vec<bool>/Vec<Vec>
            // construction cost.  O(|SCC|) per start; wrld3's 166-node SCC
            // and 166 starts is 27 K bool writes total -- trivial.
            for &v in scc {
                state.blocked[v as usize] = false;
                state.b_list[v as usize].clear();
            }

            // Push start onto the DFS stack and block it before entering
            // the recursive CIRCUIT() subroutine -- the subroutine assumes
            // its caller has already done this.
            state.blocked[start as usize] = true;
            state.path.push(start);

            let result = self.johnson_circuit(start, start, &mut state, budget);

            // Always restore the path stack even on bail-out so nested
            // state is coherent if we ever chose to retry; since we
            // currently abort the full enumeration on budget exhaustion
            // this is defense-in-depth.
            state.path.pop();

            result?;
        }

        Ok(state.circuits)
    }

    /// Recursive CIRCUIT() of Johnson 1975.  Caller must have already:
    ///   * pushed `v` onto `state.path`
    ///   * set `state.blocked[v] = true`
    ///
    /// Returns `Ok(true)` if any cycle was discovered through `v` (direct
    /// or via a descendant's recursive call); `Ok(false)` otherwise.
    /// The caller decides whether to unblock `v` based on this return
    /// value -- callers typically only need to concern themselves with
    /// that when `v == start` at the outermost frame, where unblocking
    /// happens inside the function itself.
    fn johnson_circuit(
        &self,
        v: u32,
        start: u32,
        state: &mut JohnsonState,
        budget: &mut usize,
    ) -> std::result::Result<bool, TruncatedByBudgetInternal> {
        let mut found_cycle = false;

        // Successor list is already sorted ascending by
        // `IndexedGraph::from_edges`.  Index iteration avoids holding an
        // immutable borrow on `self.succ` across the recursive call.
        let succs_len = self.succ[v as usize].len();
        for i in 0..succs_len {
            let w = self.succ[v as usize][i];

            // Induced-subgraph gate: only traverse targets that are
            // inside the current SCC AND have index >= start.  Targets
            // with index < start would re-find cycles that were already
            // emitted when their lex-smallest node was the start.
            // Targets outside the SCC can never close a cycle back to
            // start.
            if !state.in_scc[w as usize] || w < start {
                continue;
            }

            if w == start && state.path.len() > 1 {
                // Elementary cycle: `state.path` currently holds
                // [start, ..., v] with v != start (because path.len() > 1
                // rules out the self-loop-at-root case).  The closing
                // edge v -> start is implicit -- downstream code
                // (circuit_to_links) wraps from path[last] to path[0].
                //
                // Multidigraph graphs (e.g. arms-race 3-cliques, K_n
                // bidirectional cliques) produce multiple distinct
                // directed cycles over the same node set; LTM semantics
                // fold those into a single loop.  Dedup here rather than
                // post-hoc so peak memory stays proportional to unique
                // output.
                //
                // The budget is charged on every RAW cycle discovery,
                // not just unique emissions: the cap exists to bound
                // DFS work, and on a dense multidigraph the raw cycle
                // count can far exceed the unique-node-set count (K9
                // has 125,664 elementary directed cycles but only 502
                // unique node sets).  `found_cycle = true` fires
                // whether or not the circuit survives dedup so the
                // blocked/B[] unblock machinery behaves correctly.
                if *budget == 0 {
                    return Err(TruncatedByBudgetInternal);
                }
                *budget -= 1;
                state.hash_scratch.clear();
                state.hash_scratch.extend_from_slice(&state.path);
                state.hash_scratch.sort_unstable();
                let fp = hash_u32_slice(&state.hash_scratch);
                if state.seen.insert(fp) {
                    state.circuits.push(state.path.clone());
                }
                found_cycle = true;
            } else if w == start {
                // Pure self-loop at the DFS root (v == start).  Excluded
                // from the public contract (`circuit.len() > 1`).  We
                // deliberately do NOT set `found_cycle = true` here --
                // treating a self-loop as a discovered cycle would be
                // semantically misleading even though in this case
                // unblocking v is harmless (we're about to return from
                // the outermost frame anyway).
            } else if !state.blocked[w as usize] {
                // Recurse into w: caller contract is "block and push
                // before call, pop after call, unblock only via the
                // Johnson unblock machinery".
                state.blocked[w as usize] = true;
                state.path.push(w);
                let sub_found = self.johnson_circuit(w, start, state, budget)?;
                state.path.pop();
                if sub_found {
                    found_cycle = true;
                }
            }
        }

        if found_cycle {
            // Cycle found through v: unblock v and everything waiting
            // on v (transitively).  Next exploration from a different
            // branch might close a cycle through v and those waiters.
            johnson_unblock(v, state);
        } else {
            // No cycle through v on this branch.  Register v as a waiter
            // in each successor w's b_list, so that if w is later
            // unblocked (because another DFS branch found a cycle
            // through w), v will also be unblocked and retried.
            //
            // Classical Johnson's does not guard against duplicate
            // entries in b_list[w]: `johnson_unblock` short-circuits on
            // already-unblocked nodes, so at worst we pay one extra
            // pop+check per duplicate rather than an O(|b_list|) scan
            // per insertion.  Dropping the linear `contains` check is a
            // pure win on dense SCCs.
            for i in 0..succs_len {
                let w = self.succ[v as usize][i];
                if !state.in_scc[w as usize] || w < start {
                    continue;
                }
                state.b_list[w as usize].push(v);
            }
        }

        Ok(found_cycle)
    }

    /// Tiernan 1970 enumeration kept as the test oracle for the
    /// Johnson-vs-Tiernan equivalence test.  Compiled only under
    /// `cfg(test)`; production code uses `enumerate_circuits_in_scc`
    /// (Johnson's).
    #[cfg(test)]
    fn enumerate_circuits_in_scc_tiernan(
        &self,
        scc: &[u32],
        budget: &mut usize,
    ) -> std::result::Result<Vec<Vec<u32>>, TruncatedByBudgetInternal> {
        if scc.is_empty() {
            return Ok(Vec::new());
        }
        if scc.len() == 1 {
            let v = scc[0];
            if !self.succ[v as usize].contains(&v) {
                return Ok(Vec::new());
            }
        }

        let mut in_scc: Vec<bool> = vec![false; self.nodes.len()];
        for &v in scc {
            in_scc[v as usize] = true;
        }

        let mut state = DfsState {
            visited: vec![false; self.nodes.len()],
            path: Vec::with_capacity(scc.len()),
            circuits: Vec::new(),
            in_scc,
        };

        for &start in scc {
            state.visited[start as usize] = true;
            state.path.push(start);
            let bailed = self.dfs_tiernan_indexed(start, start, &mut state, budget);
            state.path.pop();
            state.visited[start as usize] = false;
            if bailed {
                return Err(TruncatedByBudgetInternal);
            }
        }

        Ok(state.circuits)
    }

    /// Tiernan DFS body, retained as oracle for the equivalence test.
    /// See `enumerate_circuits_in_scc_tiernan` for rationale.
    #[cfg(test)]
    fn dfs_tiernan_indexed(
        &self,
        start: u32,
        current: u32,
        state: &mut DfsState,
        budget: &mut usize,
    ) -> bool {
        for &neighbor in &self.succ[current as usize] {
            if !state.in_scc[neighbor as usize] {
                continue;
            }
            if neighbor == start && state.path.len() > 1 {
                if *budget == 0 {
                    return true;
                }
                *budget -= 1;
                state.circuits.push(state.path.clone());
            } else if !state.visited[neighbor as usize] && neighbor >= start {
                state.visited[neighbor as usize] = true;
                state.path.push(neighbor);
                let bailed = self.dfs_tiernan_indexed(start, neighbor, state, budget);
                state.path.pop();
                state.visited[neighbor as usize] = false;
                if bailed {
                    return true;
                }
            }
        }
        false
    }

    /// Debug-only helper: confirm the invariant that
    /// `enumerate_circuits_in_scc` emits each node-set at most once per
    /// SCC.  Used by `debug_assert!` callers; release builds compile
    /// the call out via `debug_assert!`'s no-op expansion.
    fn has_no_duplicate_node_sets(circuits: &[Vec<u32>]) -> bool {
        let mut seen: HashSet<Vec<u32>> = HashSet::with_capacity(circuits.len());
        for c in circuits {
            let mut key = c.clone();
            key.sort_unstable();
            if !seen.insert(key) {
                return false;
            }
        }
        true
    }

    /// Convert a circuit of indices back to the caller-facing
    /// `Vec<Ident<Canonical>>` once enumeration and dedup are complete.
    fn circuit_to_idents(&self, circuit: &[u32]) -> Vec<Ident<Canonical>> {
        circuit
            .iter()
            .map(|&i| self.nodes[i as usize].clone())
            .collect()
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

/// Johnson 1975 UNBLOCK(u).  Iterative implementation so deeply nested
/// B[] chains cannot overflow the recursion stack.
///
/// Called from `johnson_circuit` when a cycle has been found through `u`.
/// Sets `blocked[u] = false` and drains `b_list[u]`, cascading to unblock
/// every waiter that is still blocked.  `std::mem::take` empties the B[]
/// list so that if `u` is re-blocked later, its list is ready for a fresh
/// round of waiters.
fn johnson_unblock(u: u32, state: &mut JohnsonState) {
    let mut stack: Vec<u32> = vec![u];
    while let Some(v) = stack.pop() {
        // A vertex can appear in multiple B[] lists; the first pop
        // transitions it to unblocked and any subsequent pop is a no-op.
        if !state.blocked[v as usize] {
            continue;
        }
        state.blocked[v as usize] = false;
        let waiters = std::mem::take(&mut state.b_list[v as usize]);
        for w in waiters {
            if state.blocked[w as usize] {
                stack.push(w);
            }
        }
    }
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
    /// Bounded by [`MAX_LTM_CIRCUITS`] because the downstream LTM
    /// variable-generation pipeline synthesizes ~2 variables per loop;
    /// at wrld3's ~1.86M circuits that's too many variables for the
    /// compiler to handle even though the enumeration itself now
    /// finishes in ~1.2 s and uses under 500 MiB.  On budget exhaustion
    /// the public API returns an empty vec -- LTM compilation short-
    /// circuits to "no loops detected" and simulation proceeds normally.
    ///
    /// Callers that need an explicit truncation signal should use
    /// [`Self::find_loops_with_limit`].
    pub fn find_loops(&self) -> Vec<Loop> {
        self.find_loops_with_limit(MAX_LTM_CIRCUITS)
            .unwrap_or_default()
    }

    /// Bounded variant of [`Self::find_loops`].  Returns
    /// `Err(TruncatedByBudget)` when the DFS would enumerate more than
    /// `max_circuits` elementary circuits; otherwise `Ok(loops)`.
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

        // Iterate SCCs in a deterministic order: sort by smallest index.
        // The per-SCC enumeration is already deterministic (small-start
        // DFS over a sorted successor list), so this gives a fully
        // reproducible iteration overall.
        let mut scc_order: Vec<usize> = (0..sccs.len()).collect();
        scc_order.sort_by_key(|&i| sccs[i].first().copied().unwrap_or(u32::MAX));

        for i in scc_order {
            let scc = &sccs[i];
            // Skip trivial SCCs (single node, no self-loop): they cannot
            // carry any elementary circuit and iterating them was
            // measurable overhead on graphs with many feeder nodes.
            if scc.len() == 1 && !graph.succ[scc[0] as usize].contains(&scc[0]) {
                continue;
            }
            match graph.enumerate_circuits_in_scc(scc, &mut budget) {
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
    /// Bounded by [`MAX_LTM_CIRCUITS`]; empty result on budget
    /// exhaustion.  Use [`Self::find_circuit_node_lists_with_limit`]
    /// when the caller needs to distinguish "no loops" from "too many
    /// loops to enumerate".
    pub fn find_circuit_node_lists(&self) -> Vec<Vec<Ident<Canonical>>> {
        self.find_circuit_node_lists_with_limit(MAX_LTM_CIRCUITS)
            .unwrap_or_default()
    }

    /// Bounded variant of [`Self::find_circuit_node_lists`] exposing the
    /// budget-exhaustion signal explicitly.
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

        Ok((names, circuits))
    }

    /// Default-budget variant of [`Self::find_indexed_circuits_with_limit`];
    /// bounded by [`MAX_LTM_CIRCUITS`].  Returns `(names, [])` when the
    /// DFS budget is exhausted so callers have a uniform empty response.
    pub fn find_indexed_circuits(&self) -> (Vec<String>, Vec<Vec<u32>>) {
        self.find_indexed_circuits_with_limit(MAX_LTM_CIRCUITS)
            .unwrap_or_else(|_| (Vec::new(), Vec::new()))
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

        for input_port in self.edges.keys() {
            if input_port == output_name {
                continue;
            }
            // Skip intermediate variables (they have incoming edges within the module)
            if has_incoming.contains(input_port) {
                continue;
            }

            let raw_paths = self.find_all_simple_paths(input_port, output_name, 20);
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
    fn get_link_polarity(&self, from: &Ident<Canonical>, to: &Ident<Canonical>) -> LinkPolarity {
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
    fn assign_deterministic_loop_ids(&self, loops: &mut [Loop]) {
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

/// Detect all feedback loops in a single model
pub fn detect_loops(model: &ModelStage1, project: &Project) -> Result<Vec<Loop>> {
    let graph = CausalGraph::from_model(model, project)?;
    Ok(graph.find_loops())
}

/// Analyze the polarity of how a variable appears in an equation
fn analyze_link_polarity(
    ast: &Ast<Expr2>,
    from_var: &Ident<Canonical>,
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> LinkPolarity {
    match ast {
        Ast::Scalar(expr) => analyze_expr_polarity_with_context(
            expr,
            from_var,
            LinkPolarity::Positive,
            Some(variables),
        ),
        Ast::ApplyToAll(_, expr) => analyze_expr_polarity_with_context(
            expr,
            from_var,
            LinkPolarity::Positive,
            Some(variables),
        ),
        Ast::Arrayed(_, elements, default_expr, _) => {
            // For arrayed equations, check all elements
            let mut polarity = LinkPolarity::Unknown;
            for expr in elements.values() {
                let elem_polarity = analyze_expr_polarity_with_context(
                    expr,
                    from_var,
                    LinkPolarity::Positive,
                    Some(variables),
                );
                if polarity == LinkPolarity::Unknown {
                    polarity = elem_polarity;
                } else if polarity != elem_polarity && elem_polarity != LinkPolarity::Unknown {
                    // Mixed polarities
                    return LinkPolarity::Unknown;
                }
            }
            if let Some(default_expr) = default_expr {
                let default_polarity = analyze_expr_polarity_with_context(
                    default_expr,
                    from_var,
                    LinkPolarity::Positive,
                    Some(variables),
                );
                if polarity == LinkPolarity::Unknown {
                    polarity = default_polarity;
                } else if polarity != default_polarity && default_polarity != LinkPolarity::Unknown
                {
                    return LinkPolarity::Unknown;
                }
            }
            polarity
        }
    }
}

/// Recursively analyze expression polarity with optional context for looking up tables
fn analyze_expr_polarity_with_context(
    expr: &Expr2,
    from_var: &Ident<Canonical>,
    current_polarity: LinkPolarity,
    variables: Option<&HashMap<Ident<Canonical>, Variable>>,
) -> LinkPolarity {
    match expr {
        Expr2::Const(_, _, _) => LinkPolarity::Unknown,
        Expr2::Var(ident, _, _) => {
            let normalized = normalize_module_ref(ident);
            if &normalized == from_var || ident == from_var {
                current_polarity
            } else {
                LinkPolarity::Unknown
            }
        }
        Expr2::App(crate::builtins::BuiltinFn::Lookup(table_expr, index_expr, _), _, _) => {
            // Check if the argument contains our from_var
            let arg_polarity = analyze_expr_polarity_with_context(
                index_expr,
                from_var,
                LinkPolarity::Positive,
                variables,
            );

            if arg_polarity == LinkPolarity::Unknown {
                return LinkPolarity::Unknown;
            }

            // Try to find the table and analyze its monotonicity
            // TODO: support Expr2::Subscript for subscripted lookup tables (per-element gf)
            let table_name = match table_expr.as_ref() {
                Expr2::Var(name, _, _) => Some(name.as_str()),
                _ => None,
            };

            if let (Some(vars), Some(table_name)) = (variables, table_name)
                && let Some(Variable::Var { tables, .. }) =
                    vars.get(&*crate::common::canonicalize(table_name))
                && let Some(t) = tables.first()
            {
                let table_polarity = analyze_graphical_function_polarity(t);
                // Combine the polarities
                return match (arg_polarity, table_polarity) {
                    (LinkPolarity::Positive, LinkPolarity::Positive) => LinkPolarity::Positive,
                    (LinkPolarity::Positive, LinkPolarity::Negative) => LinkPolarity::Negative,
                    (LinkPolarity::Negative, LinkPolarity::Positive) => LinkPolarity::Negative,
                    (LinkPolarity::Negative, LinkPolarity::Negative) => LinkPolarity::Positive,
                    _ => LinkPolarity::Unknown,
                };
            }
            LinkPolarity::Unknown
        }
        // Non-decreasing single-arg builtins: propagate inner polarity.
        // Int (floor) is a step function with discontinuities, but is still
        // non-decreasing, which is sufficient for polarity propagation.
        Expr2::App(crate::builtins::BuiltinFn::Exp(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Ln(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Log10(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Sqrt(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Arctan(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Int(inner), _, _) => {
            analyze_expr_polarity_with_context(inner, from_var, current_polarity, variables)
        }
        // Max/Min (scalar two-arg form): non-decreasing in each argument
        Expr2::App(crate::builtins::BuiltinFn::Max(a, Some(b)), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Min(a, Some(b)), _, _) => {
            let pol_a =
                analyze_expr_polarity_with_context(a, from_var, current_polarity, variables);
            let pol_b =
                analyze_expr_polarity_with_context(b, from_var, current_polarity, variables);
            match (pol_a, pol_b) {
                // When one side returns Unknown, we must check whether it actually
                // references from_var. Unknown from an independent expression (e.g.
                // a constant or unrelated variable) means we can use the other side's
                // polarity. Unknown from a dependent expression (e.g. ABS(x)) means
                // the result is truly non-monotonic.
                (LinkPolarity::Unknown, known) => {
                    if expr_references_var(a, from_var) {
                        LinkPolarity::Unknown
                    } else {
                        known
                    }
                }
                (known, LinkPolarity::Unknown) => {
                    if expr_references_var(b, from_var) {
                        LinkPolarity::Unknown
                    } else {
                        known
                    }
                }
                // Both agree: propagate
                (a_pol, b_pol) if a_pol == b_pol => a_pol,
                // Disagree: unknown
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::App(_, _, _) => LinkPolarity::Unknown,
        Expr2::Op2(op, left, right, _, _) => {
            let left_pol =
                analyze_expr_polarity_with_context(left, from_var, current_polarity, variables);
            let right_pol =
                analyze_expr_polarity_with_context(right, from_var, current_polarity, variables);

            match op {
                BinaryOp::Add => match (left_pol, right_pol) {
                    (LinkPolarity::Unknown, pol) => {
                        if expr_references_var(left, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            pol
                        }
                    }
                    (pol, LinkPolarity::Unknown) => {
                        if expr_references_var(right, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            pol
                        }
                    }
                    (a, b) if a == b => a,
                    _ => LinkPolarity::Unknown,
                },
                BinaryOp::Sub => match (left_pol, right_pol) {
                    (LinkPolarity::Unknown, pol) => {
                        if expr_references_var(left, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            flip_polarity(pol)
                        }
                    }
                    (pol, LinkPolarity::Unknown) => {
                        if expr_references_var(right, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            pol
                        }
                    }
                    (a, b) if a == flip_polarity(b) => a,
                    _ => LinkPolarity::Unknown,
                },
                BinaryOp::Mul => {
                    // Multiplication needs the SIGN of the other operand to determine
                    // polarity, not just whether it's independent of from_var.
                    // This is why Mul uses is_positive_constant/is_negative_constant
                    // rather than the expr_references_var pattern used by Add/Sub/Div.
                    // If both have known polarity, combine them
                    if left_pol != LinkPolarity::Unknown && right_pol != LinkPolarity::Unknown {
                        // Positive * Positive = Positive
                        // Positive * Negative = Negative
                        // Negative * Positive = Negative
                        // Negative * Negative = Positive
                        match (left_pol, right_pol) {
                            (LinkPolarity::Positive, LinkPolarity::Positive) => {
                                LinkPolarity::Positive
                            }
                            (LinkPolarity::Positive, LinkPolarity::Negative) => {
                                LinkPolarity::Negative
                            }
                            (LinkPolarity::Negative, LinkPolarity::Positive) => {
                                LinkPolarity::Negative
                            }
                            (LinkPolarity::Negative, LinkPolarity::Negative) => {
                                LinkPolarity::Positive
                            }
                            _ => LinkPolarity::Unknown,
                        }
                    } else if left_pol != LinkPolarity::Unknown {
                        // Only left has polarity, check if right is a constant or constant-valued variable
                        if is_positive_constant(right)
                            || (variables.is_some()
                                && is_positive_variable(right, variables.unwrap()))
                        {
                            left_pol
                        } else if is_negative_constant(right)
                            || (variables.is_some()
                                && is_negative_variable(right, variables.unwrap()))
                        {
                            flip_polarity(left_pol)
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else if right_pol != LinkPolarity::Unknown {
                        // Only right has polarity, check if left is a constant or constant-valued variable
                        if is_positive_constant(left)
                            || (variables.is_some()
                                && is_positive_variable(left, variables.unwrap()))
                        {
                            right_pol
                        } else if is_negative_constant(left)
                            || (variables.is_some()
                                && is_negative_variable(left, variables.unwrap()))
                        {
                            flip_polarity(right_pol)
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                BinaryOp::Div => match (left_pol, right_pol) {
                    (LinkPolarity::Unknown, pol) => {
                        if expr_references_var(left, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            flip_polarity(pol)
                        }
                    }
                    (pol, LinkPolarity::Unknown) => {
                        if expr_references_var(right, from_var) {
                            LinkPolarity::Unknown
                        } else {
                            pol
                        }
                    }
                    (a, b) if a == flip_polarity(b) => a,
                    _ => LinkPolarity::Unknown,
                },
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::Op1(op, operand, _, _) => {
            let operand_pol =
                analyze_expr_polarity_with_context(operand, from_var, current_polarity, variables);
            match op {
                crate::ast::UnaryOp::Not => flip_polarity(operand_pol),
                crate::ast::UnaryOp::Negative => flip_polarity(operand_pol),
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::If(_, true_branch, false_branch, _, _) => {
            // For IF-THEN-ELSE, check both branches
            let true_pol = analyze_expr_polarity_with_context(
                true_branch,
                from_var,
                current_polarity,
                variables,
            );
            let false_pol = analyze_expr_polarity_with_context(
                false_branch,
                from_var,
                current_polarity,
                variables,
            );

            if true_pol == false_pol {
                true_pol
            } else {
                LinkPolarity::Unknown
            }
        }
        _ => LinkPolarity::Unknown,
    }
}

/// Flip the polarity
fn flip_polarity(pol: LinkPolarity) -> LinkPolarity {
    match pol {
        LinkPolarity::Positive => LinkPolarity::Negative,
        LinkPolarity::Negative => LinkPolarity::Positive,
        LinkPolarity::Unknown => LinkPolarity::Unknown,
    }
}

/// Check whether an expression tree contains any reference to a specific variable.
/// Used to distinguish "independent of from_var" (returns Unknown because expression
/// doesn't reference from_var at all) from "non-monotonically dependent" (returns
/// Unknown but DOES reference from_var, e.g. ABS(x)).
fn expr_references_var(expr: &Expr2, var: &Ident<Canonical>) -> bool {
    match expr {
        Expr2::Const(_, _, _) => false,
        Expr2::Var(ident, _, _) => ident == var || &normalize_module_ref(ident) == var,
        Expr2::Subscript(ident, indices, _, _) => {
            ident == var
                || indices.iter().any(|idx| match idx {
                    IndexExpr2::Expr(e) => expr_references_var(e, var),
                    IndexExpr2::Range(lo, hi, _) => {
                        expr_references_var(lo, var) || expr_references_var(hi, var)
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => false,
                })
        }
        Expr2::App(builtin, _, _) => {
            let mut found = false;
            builtin.for_each_expr_ref(|child| {
                if !found {
                    found = expr_references_var(child, var);
                }
            });
            found
        }
        Expr2::Op2(_, left, right, _, _) => {
            expr_references_var(left, var) || expr_references_var(right, var)
        }
        Expr2::Op1(_, operand, _, _) => expr_references_var(operand, var),
        Expr2::If(cond, t, f, _, _) => {
            expr_references_var(cond, var)
                || expr_references_var(t, var)
                || expr_references_var(f, var)
        }
    }
}

/// Check if expression is a positive constant
fn is_positive_constant(expr: &Expr2) -> bool {
    match expr {
        Expr2::Const(_, n, _) => *n > 0.0,
        _ => false,
    }
}

/// Check if expression is a negative constant
fn is_negative_constant(expr: &Expr2) -> bool {
    match expr {
        Expr2::Const(_, n, _) => *n < 0.0,
        _ => false,
    }
}

/// Check if a variable has a positive constant value
fn is_positive_variable(expr: &Expr2, variables: &HashMap<Ident<Canonical>, Variable>) -> bool {
    if let Expr2::Var(ident, _, _) = expr
        && let Some(var) = variables.get(ident)
        && let Some(Ast::Scalar(var_expr)) = var.ast()
    {
        // Recursively check if the variable's equation is a positive constant
        return is_positive_constant(var_expr);
    }
    false
}

/// Check if a variable has a negative constant value
fn is_negative_variable(expr: &Expr2, variables: &HashMap<Ident<Canonical>, Variable>) -> bool {
    if let Expr2::Var(ident, _, _) = expr
        && let Some(var) = variables.get(ident)
        && let Some(Ast::Scalar(var_expr)) = var.ast()
    {
        // Recursively check if the variable's equation is a negative constant
        return is_negative_constant(var_expr);
    }
    false
}

/// Analyze the polarity of a graphical function/lookup table
/// Returns Positive if monotonically increasing, Negative if monotonically decreasing, Unknown otherwise
fn analyze_graphical_function_polarity(table: &crate::variable::Table) -> LinkPolarity {
    // Need at least 2 points to determine monotonicity
    if table.x.len() < 2 || table.y.len() < 2 {
        return LinkPolarity::Unknown;
    }

    let mut all_increasing = true;
    let mut all_decreasing = true;
    let mut all_constant = true;

    // Check consecutive pairs of points
    for i in 1..table.y.len() {
        let dy = table.y[i] - table.y[i - 1];

        // Use a small epsilon for floating point comparison
        const EPSILON: f64 = 1e-10;

        if dy > EPSILON {
            all_decreasing = false;
            all_constant = false;
        } else if dy < -EPSILON {
            all_increasing = false;
            all_constant = false;
        } else {
            // dy is approximately 0 (within epsilon)
            // This doesn't break monotonicity but isn't strictly increasing/decreasing
        }
    }

    // If all changes are zero (constant function), return Unknown
    if all_constant {
        return LinkPolarity::Unknown;
    }

    // Return polarity based on monotonicity
    if all_increasing {
        LinkPolarity::Positive
    } else if all_decreasing {
        LinkPolarity::Negative
    } else {
        LinkPolarity::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        DetectedLoopPolarity, SimlinDb, compute_link_polarities, model_cycle_partitions,
        model_detected_loops, sync_from_datamodel,
    };
    use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_simple_reinforcing_loop() {
        // Create a simple reinforcing loop: population -> births -> population
        let model = x_model(
            "main",
            vec![
                x_stock("population", "100", &["births"], &[], None),
                x_flow("births", "population * birth_rate", None),
                x_aux("birth_rate", "0.02", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);
        let loops = &detected.loops;
        assert_eq!(loops.len(), 1);

        let loop_item = &loops[0];
        assert!(
            loop_item.variables.contains(&"population".to_string()),
            "Loop should contain population"
        );
        assert_eq!(loop_item.id, "r1");
        assert_eq!(loop_item.polarity, DetectedLoopPolarity::Reinforcing);
    }

    #[test]
    fn test_deterministic_loop_naming() {
        // Create a model with multiple loops to test deterministic naming
        let model = x_model(
            "main",
            vec![
                x_stock("population", "100", &["births"], &["deaths"], None),
                x_flow("births", "population * birth_rate", None),
                x_flow("deaths", "population * death_rate", None),
                x_aux("birth_rate", "0.02", None),
                x_aux("death_rate", "0.01", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project1 = x_project(sim_specs.clone(), std::slice::from_ref(&model));
        let db1 = SimlinDb::default();
        let result1 = sync_from_datamodel(&db1, &datamodel_project1);
        let model1 = result1.models["main"].source;
        let detected1 = model_detected_loops(&db1, model1, result1.project);

        let datamodel_project2 = x_project(sim_specs, &[model]);
        let db2 = SimlinDb::default();
        let result2 = sync_from_datamodel(&db2, &datamodel_project2);
        let model2 = result2.models["main"].source;
        let detected2 = model_detected_loops(&db2, model2, result2.project);

        assert_eq!(detected1.loops.len(), detected2.loops.len());

        for (loop1, loop2) in detected1.loops.iter().zip(detected2.loops.iter()) {
            assert_eq!(loop1.id, loop2.id, "Loop IDs should be deterministic");
            assert_eq!(
                loop1.variables, loop2.variables,
                "Loop variables should be identical"
            );
        }
    }

    #[test]
    fn test_no_loops() {
        // Create a model with no loops
        let model = x_model(
            "main",
            vec![
                x_aux("input", "10", None),
                x_aux("output", "input * 2", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);
        assert!(detected.loops.is_empty());
    }

    #[test]
    fn test_balancing_loop() {
        // Create a balancing loop: goal -> gap -> adjustment -> level -> gap
        // gap = goal - level (negative link from level to gap)
        let model = x_model(
            "main",
            vec![
                x_stock("level", "100", &["adjustment"], &[], None),
                x_flow("adjustment", "gap / adjustment_time", None),
                x_aux("gap", "goal - level", None),
                x_aux("goal", "200", None),
                x_aux("adjustment_time", "5", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);

        assert!(!detected.loops.is_empty());

        let has_balancing = detected
            .loops
            .iter()
            .any(|l| l.polarity == DetectedLoopPolarity::Balancing);
        assert!(has_balancing, "Should have detected a balancing loop");
    }

    #[test]
    fn test_module_loops() {
        // Test loop detection with modules
        use crate::testutils::x_module;

        // Create a model that uses a module (like SMOOTH)
        // This simulates a model with a module that might create a feedback loop
        let main_model = x_model(
            "main",
            vec![
                x_stock("inventory", "100", &["production"], &["sales"], None),
                x_flow("production", "desired_production", None),
                x_aux(
                    "desired_production",
                    "smooth_inventory_gap * adjustment_rate",
                    None,
                ),
                x_aux("inventory_gap", "target_inventory - inventory", None),
                x_module(
                    "smooth_inventory_gap",
                    &[("inventory_gap", "smooth_inventory_gap\u{00B7}input")],
                    None,
                ),
                x_aux("target_inventory", "100", None),
                x_aux("adjustment_rate", "0.1", None),
                x_flow("sales", "10", None),
            ],
        );

        // Create the SMOOTH module model (simplified version)
        let smooth_model = x_model(
            "smooth_inventory_gap",
            vec![
                x_aux("input", "0", None),
                x_stock("smoothed", "0", &["change_in_smooth"], &[], None),
                x_flow("change_in_smooth", "(input - smoothed) / smooth_time", None),
                x_aux("smooth_time", "3", None),
                x_aux("output", "smoothed", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[main_model, smooth_model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);

        assert!(
            !detected.loops.is_empty(),
            "Should find at least one loop through the module"
        );
    }

    #[test]
    fn test_multi_module_loops() {
        // Test loop detection across multiple module instances
        use crate::testutils::x_module;

        // Create a model with two module instances that form a loop together
        let main_model = x_model(
            "main",
            vec![
                x_aux("initial_value", "10", None),
                x_module(
                    "processor_a",
                    &[("initial_value", "processor_a\u{00B7}input")],
                    None,
                ),
                x_aux("intermediate", "processor_a", None),
                x_module(
                    "processor_b",
                    &[("intermediate", "processor_b\u{00B7}input")],
                    None,
                ),
                x_aux("feedback", "processor_b * 0.5", None),
                x_aux("combined", "initial_value + feedback", None),
            ],
        );

        // Create simple processor modules
        let processor_a_model = x_model(
            "processor_a",
            vec![
                x_aux("input", "0", None),
                x_aux("output", "input * 2", None),
            ],
        );

        let processor_b_model = x_model(
            "processor_b",
            vec![
                x_aux("input", "0", None),
                x_aux("output", "input + 1", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(
            sim_specs,
            &[main_model, processor_a_model, processor_b_model],
        );
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);

        // This model has no feedback loop (initial_value is a constant, no
        // path from output back to input), so no loops should be found.
        assert!(
            detected.loops.is_empty(),
            "Model without feedback should have no loops"
        );
    }

    #[test]
    fn test_link_polarity_detection() {
        // Test polarity detection in simple expressions
        use crate::ast::{Ast, Expr2};

        // Test positive link: y = x * 2
        let x_var = Ident::new("x");
        let expr = Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
            Box::new(Expr2::Const(
                "2".to_string(),
                2.0,
                crate::ast::Loc::default(),
            )),
            None,
            crate::ast::Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let empty_vars = HashMap::new();
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(polarity, LinkPolarity::Positive);

        // Test negative link: y = -x
        let expr = Expr2::Op2(
            BinaryOp::Sub,
            Box::new(Expr2::Const(
                "0".to_string(),
                0.0,
                crate::ast::Loc::default(),
            )),
            Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
            None,
            crate::ast::Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(polarity, LinkPolarity::Negative);

        // Test negative link via multiplication: y = x * -3
        let expr = Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
            Box::new(Expr2::Const(
                "-3".to_string(),
                -3.0,
                crate::ast::Loc::default(),
            )),
            None,
            crate::ast::Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(polarity, LinkPolarity::Negative);
    }

    #[test]
    fn test_format_path_empty_loop() {
        // Test format_path() with empty links (covers line 44)
        let loop_item = Loop {
            id: "R1".to_string(),
            links: vec![],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
            dimensions: vec![],
        };

        let path = loop_item.format_path();
        assert_eq!(path, "", "Empty loop should return empty string");
        assert!(path.is_empty(), "Path must be empty for loop with no links");
    }

    #[test]
    fn test_get_variable_dependencies_module() {
        // Test get_variable_dependencies for Module type (covers lines 70-72)
        use crate::variable::ModuleInput;

        let input_var = Ident::new("input_signal");
        let module = Variable::Module {
            ident: Ident::new("processor"),
            model_name: Ident::new("process_model"),
            units: None,
            inputs: vec![
                ModuleInput {
                    src: input_var.clone(),
                    dst: Ident::new("input"),
                },
                ModuleInput {
                    src: Ident::new("control"),
                    dst: Ident::new("param"),
                },
            ],
            errors: vec![],
            unit_errors: vec![],
        };

        let deps = get_variable_dependencies(&module);
        assert_eq!(deps.len(), 2, "Module should have 2 dependencies");
        assert!(deps.contains(&input_var), "Should contain input_signal");
        assert!(
            deps.contains(&Ident::new("control")),
            "Should contain control"
        );
    }

    #[test]
    fn test_get_variable_dependencies_no_ast() {
        // Test get_variable_dependencies when AST is None (covers line 83)
        let var = Variable::Var {
            ident: Ident::new("empty_var"),
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
        };

        let deps = get_variable_dependencies(&var);
        assert_eq!(
            deps.len(),
            0,
            "Variable with no AST should have no dependencies"
        );
        assert!(
            deps.is_empty(),
            "Dependencies must be empty for variable without AST"
        );
    }

    #[test]
    fn test_flip_polarity() {
        // Test flip_polarity function (covers lines 1049-1054)
        assert_eq!(
            flip_polarity(LinkPolarity::Positive),
            LinkPolarity::Negative
        );
        assert_eq!(
            flip_polarity(LinkPolarity::Negative),
            LinkPolarity::Positive
        );
        assert_eq!(flip_polarity(LinkPolarity::Unknown), LinkPolarity::Unknown);
    }

    #[test]
    fn test_is_positive_constant() {
        // Test is_positive_constant function (covers lines 1058-1062)
        use crate::ast::{Expr2, Loc};

        let pos_const = Expr2::Const("5".to_string(), 5.0, Loc::default());
        assert!(is_positive_constant(&pos_const), "5.0 should be positive");

        let neg_const = Expr2::Const("-5".to_string(), -5.0, Loc::default());
        assert!(
            !is_positive_constant(&neg_const),
            "-5.0 should not be positive"
        );

        let zero_const = Expr2::Const("0".to_string(), 0.0, Loc::default());
        assert!(
            !is_positive_constant(&zero_const),
            "0.0 should not be positive"
        );

        let var_expr = Expr2::Var(Ident::new("x"), None, Loc::default());
        assert!(
            !is_positive_constant(&var_expr),
            "Variable should not be positive constant"
        );
    }

    #[test]
    fn test_is_negative_constant() {
        // Test is_negative_constant function (covers lines 1066-1070)
        use crate::ast::{Expr2, Loc};

        let neg_const = Expr2::Const("-3".to_string(), -3.0, Loc::default());
        assert!(is_negative_constant(&neg_const), "-3.0 should be negative");

        let pos_const = Expr2::Const("3".to_string(), 3.0, Loc::default());
        assert!(
            !is_negative_constant(&pos_const),
            "3.0 should not be negative"
        );

        let zero_const = Expr2::Const("0".to_string(), 0.0, Loc::default());
        assert!(
            !is_negative_constant(&zero_const),
            "0.0 should not be negative"
        );

        let var_expr = Expr2::Var(Ident::new("y"), None, Loc::default());
        assert!(
            !is_negative_constant(&var_expr),
            "Variable should not be negative constant"
        );
    }

    #[test]
    fn test_analyze_link_polarity_arrayed() {
        // Test analyze_link_polarity with Arrayed AST (covers lines 935-947)
        use crate::ast::{Ast, Expr2, Loc};
        use crate::common::CanonicalElementName;
        use std::collections::HashMap;

        let x_var = Ident::new("x");

        // Create arrayed AST with consistent positive polarity
        let mut elements = HashMap::new();
        elements.insert(
            CanonicalElementName::from_raw("dim1"),
            Expr2::Op2(
                BinaryOp::Mul,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
                None,
                Loc::default(),
            ),
        );
        elements.insert(
            CanonicalElementName::from_raw("dim2"),
            Expr2::Op2(
                BinaryOp::Add,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                Box::new(Expr2::Const("10".to_string(), 10.0, Loc::default())),
                None,
                Loc::default(),
            ),
        );

        let ast = Ast::Arrayed(vec![], elements, None, false);
        let empty_vars = HashMap::new();
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Consistent positive elements should be positive"
        );

        // Test with mixed polarities
        let mut mixed_elements = HashMap::new();
        mixed_elements.insert(
            CanonicalElementName::from_raw("dim1"),
            Expr2::Var(x_var.clone(), None, Loc::default()),
        );
        mixed_elements.insert(
            CanonicalElementName::from_raw("dim2"),
            Expr2::Op1(
                crate::ast::UnaryOp::Negative,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                None,
                Loc::default(),
            ),
        );

        let mixed_ast = Ast::Arrayed(vec![], mixed_elements, None, false);
        let mixed_polarity = analyze_link_polarity(&mixed_ast, &x_var, &empty_vars);
        assert_eq!(
            mixed_polarity,
            LinkPolarity::Unknown,
            "Mixed polarities should be Unknown"
        );
    }

    #[test]
    fn test_analyze_expr_polarity_if_then_else() {
        // Test analyze_expr_polarity with If-Then-Else (covers lines 1033-1042)
        use crate::ast::{Expr2, Loc};

        let x_var = Ident::new("x");

        // If with same polarity in both branches
        let if_expr = Expr2::If(
            Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Op2(
                BinaryOp::Mul,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
                None,
                Loc::default(),
            )),
            None,
            Loc::default(),
        );

        let polarity =
            analyze_expr_polarity_with_context(&if_expr, &x_var, LinkPolarity::Positive, None);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Same polarity branches should return that polarity"
        );

        // If with different polarities in branches
        let mixed_if = Expr2::If(
            Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Op1(
                crate::ast::UnaryOp::Negative,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                None,
                Loc::default(),
            )),
            None,
            Loc::default(),
        );

        let mixed_polarity =
            analyze_expr_polarity_with_context(&mixed_if, &x_var, LinkPolarity::Positive, None);
        assert_eq!(
            mixed_polarity,
            LinkPolarity::Unknown,
            "Different polarity branches should be Unknown"
        );
    }

    #[test]
    fn test_analyze_expr_polarity_unary_not() {
        // Test analyze_expr_polarity with unary NOT operator (covers lines 1026-1031)
        use crate::ast::{Expr2, Loc, UnaryOp};

        let x_var = Ident::new("x");

        let not_expr = Expr2::Op1(
            UnaryOp::Not,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        );

        let polarity =
            analyze_expr_polarity_with_context(&not_expr, &x_var, LinkPolarity::Positive, None);
        assert_eq!(
            polarity,
            LinkPolarity::Negative,
            "NOT should flip polarity from positive to negative"
        );
    }

    #[test]
    fn test_analyze_expr_polarity_division_edge_cases() {
        // Test division polarity analysis edge cases (covers lines 1013-1022)
        use crate::ast::{Expr2, Loc};

        let x_var = Ident::new("x");
        let y_var = Ident::new("y");

        // Division with variable in numerator
        let div_num = Expr2::Op2(
            BinaryOp::Div,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Const("10".to_string(), 10.0, Loc::default())),
            None,
            Loc::default(),
        );

        let pol_num =
            analyze_expr_polarity_with_context(&div_num, &x_var, LinkPolarity::Positive, None);
        assert_eq!(
            pol_num,
            LinkPolarity::Positive,
            "Variable in numerator should keep polarity"
        );

        // Division with different variable in denominator (not the one we're tracking)
        let div_other = Expr2::Op2(
            BinaryOp::Div,
            Box::new(Expr2::Const("100".to_string(), 100.0, Loc::default())),
            Box::new(Expr2::Var(y_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        );

        let pol_other =
            analyze_expr_polarity_with_context(&div_other, &x_var, LinkPolarity::Positive, None);
        assert_eq!(
            pol_other,
            LinkPolarity::Unknown,
            "Unrelated variable should give Unknown"
        );
    }

    #[test]
    fn test_graphical_function_polarity() {
        use crate::variable::Table;

        // Test 1: Monotonically increasing function (positive polarity)
        let increasing_table =
            Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0, 4.0], vec![0.0, 2.0, 4.0, 6.0, 8.0]);
        assert_eq!(
            analyze_graphical_function_polarity(&increasing_table),
            LinkPolarity::Positive,
            "Monotonically increasing function should have positive polarity"
        );

        // Test 2: Monotonically decreasing function (negative polarity)
        let decreasing_table = Table::new_for_test(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![10.0, 8.0, 6.0, 4.0, 2.0],
        );
        assert_eq!(
            analyze_graphical_function_polarity(&decreasing_table),
            LinkPolarity::Negative,
            "Monotonically decreasing function should have negative polarity"
        );

        // Test 3: Non-monotonic function (unknown polarity)
        let non_monotonic_table =
            Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0, 4.0], vec![0.0, 5.0, 3.0, 7.0, 2.0]);
        assert_eq!(
            analyze_graphical_function_polarity(&non_monotonic_table),
            LinkPolarity::Unknown,
            "Non-monotonic function should have unknown polarity"
        );

        // Test 4: Constant function (unknown polarity - no change)
        let constant_table =
            Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0], vec![5.0, 5.0, 5.0, 5.0]);
        assert_eq!(
            analyze_graphical_function_polarity(&constant_table),
            LinkPolarity::Unknown,
            "Constant function should have unknown polarity"
        );

        // Test 5: Single point (edge case)
        let single_point_table = Table::new_for_test(vec![1.0], vec![2.0]);
        assert_eq!(
            analyze_graphical_function_polarity(&single_point_table),
            LinkPolarity::Unknown,
            "Single point should have unknown polarity"
        );

        // Test 6: Nearly constant with small variations (testing tolerance)
        let nearly_constant_table =
            Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0], vec![5.0, 5.0001, 5.0002, 5.0003]);
        assert_eq!(
            analyze_graphical_function_polarity(&nearly_constant_table),
            LinkPolarity::Positive,
            "Nearly constant but increasing should have positive polarity"
        );
    }

    #[test]
    fn test_lookup_table_polarity_in_links() {
        use crate::datamodel;

        // Create a model with a lookup table
        let mut model_vars = vec![
            x_stock("water", "100", &[], &["outflow"], None),
            x_flow("outflow", "water * lookup(lookup, water)", None),
        ];

        // Create the lookup table auxiliary
        let mut lookup_var = x_aux("lookup", "0", None);
        if let datamodel::Variable::Aux(aux) = &mut lookup_var {
            aux.gf = Some(datamodel::GraphicalFunction {
                kind: datamodel::GraphicalFunctionKind::Continuous,
                x_points: Some(vec![0.0, 50.0, 100.0, 150.0]),
                y_points: vec![0.1, 0.2, 0.3, 0.4],
                x_scale: datamodel::GraphicalFunctionScale {
                    min: 0.0,
                    max: 150.0,
                },
                y_scale: datamodel::GraphicalFunctionScale { min: 0.1, max: 0.4 },
            });
        }
        model_vars.push(lookup_var);

        let model = x_model("main", model_vars);
        let sim_specs = sim_specs_with_units("months");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;

        // Check per-link polarity via compute_link_polarities
        let polarities = compute_link_polarities(&db, model, result.project);
        let water_to_outflow_key = ("water".to_string(), "outflow".to_string());
        assert_eq!(
            polarities[&water_to_outflow_key],
            LinkPolarity::Positive,
            "Monotonically increasing lookup table should preserve positive polarity"
        );

        // Verify loop polarity via model_detected_loops
        let detected = model_detected_loops(&db, model, result.project);
        assert_eq!(detected.loops.len(), 1, "Should have one loop");
        // water -> outflow: Positive (increasing lookup), outflow -> water: Negative (outflow)
        assert_eq!(
            detected.loops[0].polarity,
            DetectedLoopPolarity::Balancing,
            "Loop with one negative link should be balancing"
        );
    }

    #[test]
    fn test_fishbanks_loops() {
        use crate::prost::Message;
        use std::fs;

        let proto_bytes = fs::read("../../test/fishbanks.protobin")
            .expect("Failed to read fishbanks.protobin file");
        let project_io = crate::project_io::Project::decode(&proto_bytes[..])
            .expect("Failed to decode fishbanks.protobin");
        let datamodel_project = crate::serde::deserialize(project_io);

        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);

        let model_name = crate::canonicalize(&datamodel_project.models[0].name);
        let model = result.models[model_name.as_ref()].source;
        let detected = model_detected_loops(&db, model, result.project);

        assert_eq!(
            detected.loops.len(),
            3,
            "Fishbanks model should have exactly 3 feedback loops, found: {}",
            detected.loops.len()
        );

        // Find the loop containing harvest_rate and fish_stock
        let harvest_loop = detected
            .loops
            .iter()
            .find(|l| {
                l.variables.contains(&"harvest_rate".to_string())
                    && l.variables.contains(&"fish_stock".to_string())
            })
            .expect("Should find loop containing harvest_rate and fish_stock");

        // The loop containing harvest_rate should be Undetermined because some
        // links have unknown polarity (conservative: if ANY link is unknown,
        // the whole loop is Undetermined)
        assert_eq!(
            harvest_loop.polarity,
            DetectedLoopPolarity::Undetermined,
            "Loop containing harvest_rate should be Undetermined (has unknown-polarity links)"
        );

        // Verify per-link polarity separately: harvest_rate -> fish_stock is
        // negative (outflow decreases stock)
        let polarities = compute_link_polarities(&db, model, result.project);
        let harvest_to_stock = polarities
            .get(&("harvest_rate".to_string(), "fish_stock".to_string()))
            .expect("Should have harvest_rate -> fish_stock link");
        assert_eq!(
            *harvest_to_stock,
            LinkPolarity::Negative,
            "harvest_rate -> fish_stock should have negative polarity (outflow decreases stock)"
        );
    }

    #[test]
    fn test_logistic_growth_loops() {
        use crate::prost::Message;
        use std::fs;

        let proto_bytes = fs::read("../../test/logistic-growth.protobin")
            .expect("Failed to read logistic-growth.protobin file");
        let project_io = crate::project_io::Project::decode(&proto_bytes[..])
            .expect("Failed to decode logistic-growth.protobin");
        let datamodel_project = crate::serde::deserialize(project_io);

        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);

        let model_name = crate::canonicalize(&datamodel_project.models[0].name);
        let model = result.models[model_name.as_ref()].source;
        let detected = model_detected_loops(&db, model, result.project);

        assert_eq!(
            detected.loops.len(),
            2,
            "Logistic growth model should have exactly 2 feedback loops, found: {}",
            detected.loops.len()
        );

        let balancing_count = detected
            .loops
            .iter()
            .filter(|l| l.polarity == DetectedLoopPolarity::Balancing)
            .count();
        let undetermined_count = detected
            .loops
            .iter()
            .filter(|l| l.polarity == DetectedLoopPolarity::Undetermined)
            .count();

        assert_eq!(
            balancing_count, 1,
            "Logistic growth model should have exactly 1 balancing loop, found: {}",
            balancing_count
        );
        assert_eq!(
            undetermined_count, 1,
            "Logistic growth model should have exactly 1 undetermined loop, found: {}",
            undetermined_count
        );

        // The carrying capacity loop involves fractional_growth_rate and
        // fraction_of_carrying_capacity_used; it should be balancing
        let carrying_capacity_loop = detected.loops.iter().find(|l| {
            l.variables
                .contains(&"fraction_of_carrying_capacity_used".to_string())
                && l.variables.contains(&"fractional_growth_rate".to_string())
        });

        if let Some(loop_item) = carrying_capacity_loop {
            assert_eq!(
                loop_item.polarity,
                DetectedLoopPolarity::Balancing,
                "The carrying capacity loop should be balancing"
            );
        } else {
            panic!("Could not find the carrying capacity loop in the model");
        }
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_reinforcing() {
        // All positive scores -> Reinforcing
        let scores = vec![f64::NAN, 1.0, 2.0, 3.0, 0.5];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, Some(LoopPolarity::Reinforcing));
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_balancing() {
        // All negative scores -> Balancing
        let scores = vec![f64::NAN, -1.0, -2.0, -3.0, -0.5];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, Some(LoopPolarity::Balancing));
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_undetermined() {
        // Mix of positive and negative scores -> Undetermined
        let scores = vec![f64::NAN, 1.0, -2.0, 3.0, -0.5];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, Some(LoopPolarity::Undetermined));
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_empty() {
        // Empty scores -> None
        let scores: Vec<f64> = vec![];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, None);
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_all_nan() {
        // All NaN scores -> None
        let scores = vec![f64::NAN, f64::NAN, f64::NAN];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, None);
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_all_zero() {
        // All zero scores (after filtering NaN) -> None
        let scores = vec![f64::NAN, 0.0, 0.0, 0.0];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, None);
    }

    #[test]
    fn test_loop_polarity_abbreviation() {
        assert_eq!(LoopPolarity::Reinforcing.abbreviation(), "R");
        assert_eq!(LoopPolarity::Balancing.abbreviation(), "B");
        assert_eq!(LoopPolarity::Undetermined.abbreviation(), "U");
    }

    #[test]
    fn test_calculate_polarity_all_unknown_links() {
        // When all links have Unknown polarity, the loop should be Undetermined
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        let links = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Unknown,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Unknown,
            },
        ];

        let polarity = graph.calculate_polarity(&links);
        assert_eq!(
            polarity,
            LoopPolarity::Undetermined,
            "Loop with all Unknown link polarities should have Undetermined polarity"
        );
    }

    #[test]
    fn test_calculate_polarity_mixed_unknown_and_known() {
        // Conservative approach: if ANY link is unknown, the loop is Undetermined
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        // One negative link, one unknown -> should be Undetermined
        let links_one_negative = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Negative,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Unknown,
            },
        ];

        let polarity = graph.calculate_polarity(&links_one_negative);
        assert_eq!(
            polarity,
            LoopPolarity::Undetermined,
            "Loop with any unknown link should be Undetermined"
        );

        // Two positive links, one unknown -> should also be Undetermined
        let links_two_positive = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("c"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::new("c"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Unknown,
            },
        ];

        let polarity = graph.calculate_polarity(&links_two_positive);
        assert_eq!(
            polarity,
            LoopPolarity::Undetermined,
            "Loop with any unknown link should be Undetermined"
        );
    }

    #[test]
    fn test_calculate_polarity_all_known_links() {
        // When all links have known polarity, count negative links
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        // All positive links -> Reinforcing (even number of negatives: 0)
        let links_all_positive = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Positive,
            },
        ];

        let polarity = graph.calculate_polarity(&links_all_positive);
        assert_eq!(
            polarity,
            LoopPolarity::Reinforcing,
            "Loop with all positive links should be Reinforcing"
        );

        // One negative, one positive -> Balancing (odd number of negatives: 1)
        let links_one_negative = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Negative,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Positive,
            },
        ];

        let polarity = graph.calculate_polarity(&links_one_negative);
        assert_eq!(
            polarity,
            LoopPolarity::Balancing,
            "Loop with one negative link should be Balancing"
        );

        // Two negative links -> Reinforcing (even number of negatives: 2)
        let links_two_negatives = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Negative,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Negative,
            },
        ];

        let polarity = graph.calculate_polarity(&links_two_negatives);
        assert_eq!(
            polarity,
            LoopPolarity::Reinforcing,
            "Loop with two negative links should be Reinforcing"
        );
    }

    #[test]
    fn test_loop_id_assignment_undetermined_polarity() {
        // Loops with Undetermined structural polarity should get "u" prefix
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        let mut loops = vec![
            Loop {
                id: String::new(),
                links: vec![
                    Link {
                        from: Ident::new("a"),
                        to: Ident::new("b"),
                        polarity: LinkPolarity::Unknown,
                    },
                    Link {
                        from: Ident::new("b"),
                        to: Ident::new("a"),
                        polarity: LinkPolarity::Unknown,
                    },
                ],
                stocks: vec![],
                polarity: LoopPolarity::Undetermined,
                dimensions: vec![],
            },
            Loop {
                id: String::new(),
                links: vec![
                    Link {
                        from: Ident::new("x"),
                        to: Ident::new("y"),
                        polarity: LinkPolarity::Positive,
                    },
                    Link {
                        from: Ident::new("y"),
                        to: Ident::new("x"),
                        polarity: LinkPolarity::Positive,
                    },
                ],
                stocks: vec![],
                polarity: LoopPolarity::Reinforcing,
                dimensions: vec![],
            },
        ];

        graph.assign_deterministic_loop_ids(&mut loops);

        // Find the undetermined loop (contains "a" and "b")
        let undetermined_loop = loops
            .iter()
            .find(|l| l.links.iter().any(|link| link.from.as_str() == "a"))
            .expect("Should find undetermined loop");

        assert!(
            undetermined_loop.id.starts_with("u"),
            "Undetermined polarity loop should have 'u' prefix, got: {}",
            undetermined_loop.id
        );

        // Find the reinforcing loop (contains "x" and "y")
        let reinforcing_loop = loops
            .iter()
            .find(|l| l.links.iter().any(|link| link.from.as_str() == "x"))
            .expect("Should find reinforcing loop");

        assert!(
            reinforcing_loop.id.starts_with("r"),
            "Reinforcing polarity loop should have 'r' prefix, got: {}",
            reinforcing_loop.id
        );
    }

    #[test]
    fn test_builtin_polarity_monotone_increasing() {
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
        let empty_vars = HashMap::new();

        // Exp(x), Ln(x), Log10(x), Sqrt(x), Arctan(x), Int(x) all propagate polarity
        let monotone_fns: Vec<(&str, BuiltinFn<Expr2>)> = vec![
            ("Exp", BuiltinFn::Exp(x_expr())),
            ("Ln", BuiltinFn::Ln(x_expr())),
            ("Log10", BuiltinFn::Log10(x_expr())),
            ("Sqrt", BuiltinFn::Sqrt(x_expr())),
            ("Arctan", BuiltinFn::Arctan(x_expr())),
            ("Int", BuiltinFn::Int(x_expr())),
        ];

        for (name, builtin) in monotone_fns {
            let expr = Expr2::App(builtin, None, Loc::default());
            let ast = Ast::Scalar(expr);
            let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
            assert_eq!(
                polarity,
                LinkPolarity::Positive,
                "{name}(x) should propagate positive polarity"
            );
        }
    }

    #[test]
    fn test_builtin_polarity_monotone_negative_inner() {
        use crate::ast::UnaryOp;
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        // -x has Negative polarity
        let neg_x = || {
            Box::new(Expr2::Op1(
                UnaryOp::Negative,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                None,
                Loc::default(),
            ))
        };
        let empty_vars = HashMap::new();

        let expr = Expr2::App(BuiltinFn::Exp(neg_x()), None, Loc::default());
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Negative,
            "Exp(-x) should have negative polarity"
        );
    }

    #[test]
    fn test_builtin_polarity_non_monotone_returns_unknown() {
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
        let empty_vars = HashMap::new();

        let non_monotone: Vec<(&str, BuiltinFn<Expr2>)> = vec![
            ("Abs", BuiltinFn::Abs(x_expr())),
            ("Sin", BuiltinFn::Sin(x_expr())),
            ("Cos", BuiltinFn::Cos(x_expr())),
            ("Sign", BuiltinFn::Sign(x_expr())),
        ];

        for (name, builtin) in non_monotone {
            let expr = Expr2::App(builtin, None, Loc::default());
            let ast = Ast::Scalar(expr);
            let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
            assert_eq!(
                polarity,
                LinkPolarity::Unknown,
                "{name}(x) should return Unknown polarity"
            );
        }
    }

    #[test]
    fn test_builtin_polarity_max_min_scalar() {
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
        let const_5 = || Box::new(Expr2::Const("5".to_string(), 5.0, Loc::default()));
        let empty_vars = HashMap::new();

        // Max(x, 5): only x depends on from_var -> propagate x's polarity
        let expr = Expr2::App(
            BuiltinFn::Max(x_expr(), Some(const_5())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Max(x, 5) should propagate positive polarity"
        );

        // Min(5, x): only x depends on from_var -> propagate x's polarity
        let expr = Expr2::App(
            BuiltinFn::Min(const_5(), Some(x_expr())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Min(5, x) should propagate positive polarity"
        );
    }

    #[test]
    fn test_expr_references_var() {
        use crate::ast::{Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let y_var = Ident::new("y");
        let x_expr = || Expr2::Var(x_var.clone(), None, Loc::default());
        let const_5 = || Expr2::Const("5".to_string(), 5.0, Loc::default());

        assert!(!expr_references_var(&const_5(), &x_var));
        assert!(expr_references_var(&x_expr(), &x_var));
        assert!(!expr_references_var(&x_expr(), &y_var));

        // ABS(x) references x
        let abs_x = Expr2::App(BuiltinFn::Abs(Box::new(x_expr())), None, Loc::default());
        assert!(expr_references_var(&abs_x, &x_var));
        assert!(!expr_references_var(&abs_x, &y_var));

        // PULSE(1, 5) doesn't reference x
        let pulse = Expr2::App(
            BuiltinFn::Pulse(Box::new(const_5()), Box::new(const_5()), None),
            None,
            Loc::default(),
        );
        assert!(!expr_references_var(&pulse, &x_var));

        // x + 5 references x
        let add = Expr2::Op2(
            BinaryOp::Add,
            Box::new(x_expr()),
            Box::new(const_5()),
            None,
            Loc::default(),
        );
        assert!(expr_references_var(&add, &x_var));
        assert!(!expr_references_var(&add, &y_var));

        // Subscript: array[x] references x through the index expression
        use crate::ast::IndexExpr2;
        let array_var = Ident::new("array");
        let subscript_with_x = Expr2::Subscript(
            array_var.clone(),
            vec![IndexExpr2::Expr(x_expr())],
            None,
            Loc::default(),
        );
        assert!(
            expr_references_var(&subscript_with_x, &x_var),
            "array[x] should reference x through its index"
        );
        assert!(
            expr_references_var(&subscript_with_x, &array_var),
            "array[x] should reference array as the subscripted variable"
        );
        assert!(
            !expr_references_var(&subscript_with_x, &y_var),
            "array[x] should not reference y"
        );

        // Subscript with range: array[x..5] references x
        let subscript_range = Expr2::Subscript(
            array_var.clone(),
            vec![IndexExpr2::Range(x_expr(), const_5(), Loc::default())],
            None,
            Loc::default(),
        );
        assert!(
            expr_references_var(&subscript_range, &x_var),
            "array[x..5] should reference x through range index"
        );

        // Subscript with constant index: array[5] doesn't reference x
        let subscript_const = Expr2::Subscript(
            array_var.clone(),
            vec![IndexExpr2::Expr(const_5())],
            None,
            Loc::default(),
        );
        assert!(
            !expr_references_var(&subscript_const, &x_var),
            "array[5] should not reference x"
        );
    }

    #[test]
    fn test_max_min_polarity_with_non_monotone_arg() {
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
        let abs_x = || Box::new(Expr2::App(BuiltinFn::Abs(x_expr()), None, Loc::default()));
        let const_5 = || Box::new(Expr2::Const("5".to_string(), 5.0, Loc::default()));
        let empty_vars = HashMap::new();

        // MAX(ABS(x), x): ABS(x) is non-monotonically dependent on x,
        // so overall polarity is unknown
        let expr = Expr2::App(
            BuiltinFn::Max(abs_x(), Some(x_expr())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Unknown,
            "MAX(ABS(x), x) should be Unknown (non-monotonic arg)"
        );

        // MIN(ABS(x), x): same reasoning
        let expr = Expr2::App(
            BuiltinFn::Min(abs_x(), Some(x_expr())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Unknown,
            "MIN(ABS(x), x) should be Unknown (non-monotonic arg)"
        );

        // MAX(5, x): constant is independent, propagate x's polarity
        let expr = Expr2::App(
            BuiltinFn::Max(const_5(), Some(x_expr())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Positive,
            "MAX(5, x) should be Positive (constant is independent)"
        );

        // MAX(ABS(x), ABS(x)): both non-monotonic
        let expr = Expr2::App(BuiltinFn::Max(abs_x(), Some(abs_x())), None, Loc::default());
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Unknown,
            "MAX(ABS(x), ABS(x)) should be Unknown"
        );

        // MAX(x, x): both positively dependent → Positive
        let expr = Expr2::App(
            BuiltinFn::Max(x_expr(), Some(x_expr())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Positive,
            "MAX(x, x) should be Positive"
        );

        // MAX(PULSE(1, 5), x): PULSE doesn't depend on x → propagate x's polarity
        let pulse = || {
            Box::new(Expr2::App(
                BuiltinFn::Pulse(const_5(), const_5(), None),
                None,
                Loc::default(),
            ))
        };
        let expr = Expr2::App(
            BuiltinFn::Max(pulse(), Some(x_expr())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Positive,
            "MAX(PULSE(1,5), x) should be Positive (PULSE is independent)"
        );
    }

    #[test]
    fn test_add_sub_div_polarity_with_non_monotone_arg() {
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
        let abs_x = || Box::new(Expr2::App(BuiltinFn::Abs(x_expr()), None, Loc::default()));
        let const_5 = || Box::new(Expr2::Const("5".to_string(), 5.0, Loc::default()));
        let empty_vars = HashMap::new();

        // x + ABS(x): ABS(x) non-monotonically depends on x → Unknown
        let expr = Expr2::Op2(BinaryOp::Add, x_expr(), abs_x(), None, Loc::default());
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Unknown,
            "x + ABS(x) should be Unknown"
        );

        // x + 5: 5 is independent → Positive
        let expr = Expr2::Op2(BinaryOp::Add, x_expr(), const_5(), None, Loc::default());
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Positive,
            "x + 5 should be Positive"
        );

        // ABS(x) - x: ABS(x) non-monotonically depends on x → Unknown
        let expr = Expr2::Op2(BinaryOp::Sub, abs_x(), x_expr(), None, Loc::default());
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Unknown,
            "ABS(x) - x should be Unknown"
        );

        // 5 - x: 5 is independent → flip(Positive) = Negative
        let expr = Expr2::Op2(BinaryOp::Sub, const_5(), x_expr(), None, Loc::default());
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Negative,
            "5 - x should be Negative"
        );

        // x / ABS(x) = sign(x), non-monotonic → Unknown
        let expr = Expr2::Op2(BinaryOp::Div, x_expr(), abs_x(), None, Loc::default());
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Unknown,
            "x / ABS(x) should be Unknown"
        );

        // x / 5: 5 is independent → Positive
        let expr = Expr2::Op2(BinaryOp::Div, x_expr(), const_5(), None, Loc::default());
        let ast = Ast::Scalar(expr);
        assert_eq!(
            analyze_link_polarity(&ast, &x_var, &empty_vars),
            LinkPolarity::Positive,
            "x / 5 should be Positive"
        );
    }

    #[test]
    fn test_all_links() {
        // Create a model with known causal structure:
        // population -> births -> population (reinforcing loop)
        // population -> deaths -> population (balancing loop)
        // birth_rate -> births (external input)
        // death_rate -> deaths (external input)
        let model = x_model(
            "main",
            vec![
                x_stock("population", "100", &["births"], &["deaths"], None),
                x_flow("births", "population * birth_rate", None),
                x_flow("deaths", "population * death_rate", None),
                x_aux("birth_rate", "0.02", None),
                x_aux("death_rate", "0.01", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;

        let polarities = compute_link_polarities(&db, model, result.project);

        // Should have links for:
        // birth_rate -> births
        // births -> population (inflow, positive)
        // death_rate -> deaths
        // deaths -> population (outflow, negative)
        // population -> births (stock to flow)
        // population -> deaths (stock to flow)
        assert_eq!(
            polarities.len(),
            6,
            "Should have exactly 6 causal links, found {}",
            polarities.len()
        );

        // Check specific links exist with correct polarity
        assert_eq!(
            polarities[&("births".to_string(), "population".to_string())],
            LinkPolarity::Positive,
            "Inflow should have positive polarity"
        );
        assert_eq!(
            polarities[&("deaths".to_string(), "population".to_string())],
            LinkPolarity::Negative,
            "Outflow should have negative polarity"
        );

        // Verify deterministic ordering: collect keys, sort, and check sorted
        let mut keys: Vec<_> = polarities.keys().cloned().collect();
        keys.sort();
        for i in 1..keys.len() {
            assert!(
                keys[i - 1] < keys[i],
                "Keys should be sorted: {:?} should come before {:?}",
                keys[i - 1],
                keys[i]
            );
        }
    }

    #[test]
    fn test_normalize_module_ref() {
        // Non-module ref passes through unchanged
        let plain = Ident::new("x");
        assert_eq!(normalize_module_ref(&plain).as_str(), "x");

        // Module·output ref normalized to just the module node
        let module_out = Ident::new("$⁚s⁚0⁚smth1\u{00B7}output");
        assert_eq!(normalize_module_ref(&module_out).as_str(), "$⁚s⁚0⁚smth1");

        // Ident with ⁚ but no · passes through unchanged
        let internal = Ident::new("$⁚ltm⁚link_score⁚x→y");
        assert_eq!(
            normalize_module_ref(&internal).as_str(),
            "$⁚ltm⁚link_score⁚x→y"
        );
    }

    #[test]
    fn test_module_output_dep_normalized() {
        use crate::test_common::TestProject;

        // s = SMTH1(x, 5) creates an implicit module "$⁚s⁚0⁚smth1".
        // s's equation becomes a reference to "$⁚s⁚0⁚smth1·output".
        // After normalization, the edge should go from the module node to s
        // (stripping the ·output suffix).
        let project = TestProject::new("test_mod_norm")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .aux("y", "s * 2", None)
            .compile()
            .expect("should compile");

        let main_ident: Ident<Canonical> = Ident::new("main");
        let model = &project.models[&main_ident];
        let graph = CausalGraph::from_model(model, &project).unwrap();

        let smth1_var = model
            .variables
            .keys()
            .find(|k| k.as_str().contains("smth1"))
            .expect("should have smth1 module variable");

        let s_ident = Ident::new("s");
        let has_module_to_s = graph
            .edges
            .get(smth1_var)
            .map(|targets| targets.contains(&s_ident))
            .unwrap_or(false);

        assert!(
            has_module_to_s,
            "Should have an edge from smth1 module to s (after normalization). \
             Edges: {:?}",
            graph.edges
        );

        // Also verify there's NO phantom "module·output" node in the graph
        let has_phantom = graph.edges.keys().any(|k| k.as_str().contains('\u{00B7}'));
        assert!(
            !has_phantom,
            "Should not have any module·output phantom nodes in edges"
        );
    }

    #[test]
    fn test_module_polarity_through_output_ref() {
        use crate::test_common::TestProject;

        // s = SMTH1(x, 5) creates module ref "$⁚s⁚0⁚smth1·output" in s's AST.
        // The polarity of module -> s should be positive (s = module·output, identity).
        // The polarity of s -> y should be positive (y = s * 2).
        let project = TestProject::new("test_mod_pol")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .aux("y", "s * 2", None)
            .compile()
            .expect("should compile");

        let main_ident: Ident<Canonical> = Ident::new("main");
        let model = &project.models[&main_ident];
        let graph = CausalGraph::from_model(model, &project).unwrap();

        let smth1_var = model
            .variables
            .keys()
            .find(|k| k.as_str().contains("smth1"))
            .expect("should have smth1 module variable");

        let s_ident = Ident::new("s");
        let polarity = graph.get_link_polarity(smth1_var, &s_ident);

        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Polarity from smth1 module to s should be positive (s references module·output), got {:?}",
            polarity
        );
    }

    #[test]
    fn test_regression_causal_graph_after_implicit_instantiation() {
        use crate::test_common::TestProject;

        let project = TestProject::new("test_implicit_inst")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .compile()
            .expect("should compile");

        let main_ident: Ident<Canonical> = Ident::new("main");
        let model = &project.models[&main_ident];

        // After compilation, the parent model should have a Module variable for smth1
        let has_module = model
            .variables
            .values()
            .any(|v| matches!(v, Variable::Module { .. }));

        assert!(
            has_module,
            "Parent model should contain a Module variable for the smth1 instance"
        );
    }

    #[test]
    fn test_classify_smth1_as_dynamic() {
        use crate::test_common::TestProject;

        let project = TestProject::new("test_classify_smth1")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .compile()
            .expect("should compile");

        let smth1_ident = Ident::new("stdlib⁚smth1");
        let smth1_model = project
            .models
            .get(&smth1_ident)
            .expect("should have stdlib⁚smth1 model");

        assert_eq!(
            classify_module_for_ltm(smth1_model),
            ModuleLtmRole::DynamicModule
        );
    }

    // --- Cycle Partition tests ---

    #[test]
    fn test_cycle_partitions_single_stock_self_loop() {
        let model = x_model(
            "main",
            vec![
                x_stock("stock", "100", &["flow"], &[], None),
                x_flow("flow", "stock * 0.1", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;

        let partitions = model_cycle_partitions(&db, model, result.project);
        assert_eq!(partitions.partitions.len(), 1);
        assert_eq!(partitions.partitions[0].len(), 1);
        assert_eq!(partitions.partitions[0][0], "stock");
        assert_eq!(partitions.stock_partition["stock"], 0);
    }

    #[test]
    fn test_cycle_partitions_two_independent_stocks() {
        let model = x_model(
            "main",
            vec![
                x_stock("alpha", "50", &["flow_a"], &[], None),
                x_flow("flow_a", "alpha * 0.1", None),
                x_stock("beta", "10", &["flow_b"], &[], None),
                x_flow("flow_b", "beta * 0.2", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;

        let partitions = model_cycle_partitions(&db, model, result.project);
        assert_eq!(partitions.partitions.len(), 2);
        assert_eq!(partitions.partitions[0], vec!["alpha"]);
        assert_eq!(partitions.partitions[1], vec!["beta"]);
        assert_ne!(
            partitions.stock_partition["alpha"],
            partitions.stock_partition["beta"]
        );
    }

    #[test]
    fn test_cycle_partitions_two_mutually_reachable_stocks() {
        // prey <-> predators through flows
        let model = x_model(
            "main",
            vec![
                x_stock("prey", "100", &["prey_births"], &["prey_deaths"], None),
                x_flow("prey_births", "prey * 0.1", None),
                x_flow("prey_deaths", "prey * predators * 0.01", None),
                x_stock("predators", "10", &["pred_births"], &["pred_deaths"], None),
                x_flow("pred_births", "predators * prey * 0.001", None),
                x_flow("pred_deaths", "predators * 0.05", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;

        let partitions = model_cycle_partitions(&db, model, result.project);
        assert_eq!(
            partitions.partitions.len(),
            1,
            "Mutually-reachable stocks should be in one partition"
        );
        assert_eq!(partitions.partitions[0].len(), 2);
        assert_eq!(partitions.partitions[0][0], "predators");
        assert_eq!(partitions.partitions[0][1], "prey");
        assert_eq!(
            partitions.stock_partition["prey"],
            partitions.stock_partition["predators"]
        );
    }

    #[test]
    fn test_cycle_partitions_three_stocks_two_partitions() {
        // a <-> b (coupled), c independent
        let model = x_model(
            "main",
            vec![
                x_stock("stock_a", "50", &["flow_ab"], &[], None),
                x_flow("flow_ab", "stock_b * 0.1", None),
                x_stock("stock_b", "30", &["flow_ba"], &[], None),
                x_flow("flow_ba", "stock_a * 0.2", None),
                x_stock("stock_c", "10", &["flow_c"], &[], None),
                x_flow("flow_c", "stock_c * 0.05", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;

        let partitions = model_cycle_partitions(&db, model, result.project);
        assert_eq!(partitions.partitions.len(), 2);
        let coupled = &partitions.partitions[0];
        let independent = &partitions.partitions[1];
        assert_eq!(coupled.len(), 2);
        assert_eq!(coupled[0], "stock_a");
        assert_eq!(coupled[1], "stock_b");
        assert_eq!(independent.len(), 1);
        assert_eq!(independent[0], "stock_c");
    }

    #[test]
    fn test_cycle_partitions_three_stock_chain_scc() {
        // A -> B -> C -> A: all three mutually reachable through the chain
        let model = x_model(
            "main",
            vec![
                x_stock("stock_a", "50", &["flow_ab"], &[], None),
                x_flow("flow_ab", "stock_c * 0.1", None), // C -> A
                x_stock("stock_b", "30", &["flow_bc"], &[], None),
                x_flow("flow_bc", "stock_a * 0.2", None), // A -> B
                x_stock("stock_c", "10", &["flow_ca"], &[], None),
                x_flow("flow_ca", "stock_b * 0.05", None), // B -> C
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;

        let partitions = model_cycle_partitions(&db, model, result.project);
        assert_eq!(
            partitions.partitions.len(),
            1,
            "3-stock chain should form one SCC"
        );
        assert_eq!(partitions.partitions[0].len(), 3);
        assert_eq!(partitions.partitions[0][0], "stock_a");
        assert_eq!(partitions.partitions[0][1], "stock_b");
        assert_eq!(partitions.partitions[0][2], "stock_c");
    }

    #[test]
    fn test_cycle_partitions_one_way_path() {
        // a -> b but not b -> a: two separate partitions
        let model = x_model(
            "main",
            vec![
                x_stock("stock_a", "50", &["flow_a"], &[], None),
                x_flow("flow_a", "stock_a * 0.1", None),
                x_stock("stock_b", "30", &["flow_b"], &[], None),
                x_flow("flow_b", "stock_a * 0.2", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;

        let partitions = model_cycle_partitions(&db, model, result.project);
        assert_eq!(
            partitions.partitions.len(),
            2,
            "One-way path should yield two separate partitions"
        );
        assert_ne!(
            partitions.stock_partition["stock_a"],
            partitions.stock_partition["stock_b"]
        );
    }

    #[test]
    fn test_cycle_partitions_determinism() {
        let model = x_model(
            "main",
            vec![
                x_stock("stock_a", "50", &["flow_ab"], &[], None),
                x_flow("flow_ab", "stock_b * 0.1", None),
                x_stock("stock_b", "30", &["flow_ba"], &[], None),
                x_flow("flow_ba", "stock_a * 0.2", None),
                x_stock("stock_c", "10", &["flow_c"], &[], None),
                x_flow("flow_c", "stock_c * 0.05", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs.clone(), std::slice::from_ref(&model));
        let db1 = SimlinDb::default();
        let result1 = sync_from_datamodel(&db1, &datamodel_project);
        let model1 = result1.models["main"].source;
        let p1 = model_cycle_partitions(&db1, model1, result1.project);

        let datamodel_project2 = x_project(sim_specs, std::slice::from_ref(&model));
        let db2 = SimlinDb::default();
        let result2 = sync_from_datamodel(&db2, &datamodel_project2);
        let model2 = result2.models["main"].source;
        let p2 = model_cycle_partitions(&db2, model2, result2.project);

        assert_eq!(p1.partitions.len(), p2.partitions.len());
        for (a, b) in p1.partitions.iter().zip(p2.partitions.iter()) {
            assert_eq!(a, b);
        }
    }

    #[test]
    fn test_cycle_partitions_partition_for_loop() {
        let model = x_model(
            "main",
            vec![
                x_stock("stock_a", "50", &["flow_a"], &[], None),
                x_flow("flow_a", "stock_a * 0.1", None),
                x_stock("stock_b", "10", &["flow_b"], &[], None),
                x_flow("flow_b", "stock_b * 0.2", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let partitions = model_cycle_partitions(&db, model, result.project);

        // stock_a and stock_b should each map to a partition
        assert!(partitions.stock_partition.contains_key("stock_a"));
        assert!(partitions.stock_partition.contains_key("stock_b"));
        assert_ne!(
            partitions.stock_partition["stock_a"],
            partitions.stock_partition["stock_b"]
        );

        // Verify that detected loops reference stocks that map to partitions
        let detected = model_detected_loops(&db, model, result.project);
        assert_eq!(detected.loops.len(), 2);
        for detected_loop in &detected.loops {
            let has_partition = detected_loop
                .variables
                .iter()
                .any(|v| partitions.stock_partition.contains_key(v.as_str()));
            assert!(
                has_partition,
                "Loop {} should have at least one stock in the partition map",
                detected_loop.id
            );
        }
    }

    #[test]
    fn test_loop_through_module_has_internal_stocks() {
        use crate::testutils::x_module;

        // Parent model: inventory -> production -> desired_production ->
        //   smooth_inventory_gap (module) -> inventory_gap -> inventory
        let main_model = x_model(
            "main",
            vec![
                x_stock("inventory", "100", &["production"], &["sales"], None),
                x_flow("production", "desired_production", None),
                x_aux(
                    "desired_production",
                    "smooth_inventory_gap * adjustment_rate",
                    None,
                ),
                x_aux("inventory_gap", "target_inventory - inventory", None),
                x_module(
                    "smooth_inventory_gap",
                    &[("inventory_gap", "smooth_inventory_gap\u{00B7}input")],
                    None,
                ),
                x_aux("target_inventory", "100", None),
                x_aux("adjustment_rate", "0.1", None),
                x_flow("sales", "10", None),
            ],
        );

        // SMOOTH-like module with an internal stock
        let smooth_model = x_model(
            "smooth_inventory_gap",
            vec![
                x_aux("input", "0", None),
                x_stock("smoothed", "0", &["change_in_smooth"], &[], None),
                x_flow("change_in_smooth", "(input - smoothed) / smooth_time", None),
                x_aux("smooth_time", "3", None),
                x_aux("output", "smoothed", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[main_model, smooth_model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);

        assert!(
            !detected.loops.is_empty(),
            "Should detect at least one loop through the module"
        );

        // The salsa path treats modules as black-box nodes in the parent
        // graph, so the loop includes the module node and the parent stock
        // but not the module-internal stock name.
        let has_inventory = detected
            .loops
            .iter()
            .any(|l| l.variables.contains(&"inventory".to_string()));
        assert!(
            has_inventory,
            "Should find a loop containing the parent stock 'inventory'. Found: {:?}",
            detected
                .loops
                .iter()
                .map(|l| &l.variables)
                .collect::<Vec<_>>()
        );

        let has_module_node = detected
            .loops
            .iter()
            .any(|l| l.variables.contains(&"smooth_inventory_gap".to_string()));
        assert!(
            has_module_node,
            "Loop should include the module node 'smooth_inventory_gap'. Found: {:?}",
            detected
                .loops
                .iter()
                .map(|l| &l.variables)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_loop_through_modules_with_intermediate_variables() {
        use crate::testutils::x_module;

        // stock -> flow -> module_a -> aux_x -> aux_y -> module_b -> result -> stock
        let main_model = x_model(
            "main",
            vec![
                x_stock("tank", "100", &["inflow"], &[], None),
                x_flow("inflow", "result", None),
                x_module("module_a", &[("tank", "module_a\u{00B7}input")], None),
                x_aux("aux_x", "module_a * 2", None),
                x_aux("aux_y", "aux_x + 1", None),
                x_module("module_b", &[("aux_y", "module_b\u{00B7}input")], None),
                x_aux("result", "module_b * 0.5", None),
            ],
        );

        let module_a_model = x_model(
            "module_a",
            vec![
                x_aux("input", "0", None),
                x_stock("buffer_a", "0", &["fill_a"], &[], None),
                x_flow("fill_a", "(input - buffer_a) / 2", None),
                x_aux("output", "buffer_a", None),
            ],
        );

        let module_b_model = x_model(
            "module_b",
            vec![
                x_aux("input", "0", None),
                x_stock("buffer_b", "0", &["fill_b"], &[], None),
                x_flow("fill_b", "(input - buffer_b) / 3", None),
                x_aux("output", "buffer_b", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[main_model, module_a_model, module_b_model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);

        assert!(
            !detected.loops.is_empty(),
            "Should detect a loop through two modules with intermediate variables"
        );

        // The salsa path treats modules as black-box nodes: loop variables
        // include the module names and the parent stock, not sub-model internals.
        let all_vars: HashSet<&str> = detected
            .loops
            .iter()
            .flat_map(|l| l.variables.iter().map(|s| s.as_str()))
            .collect();

        assert!(
            all_vars.contains("tank"),
            "Should include parent stock 'tank'. Found: {all_vars:?}"
        );
        assert!(
            all_vars.contains("module_a"),
            "Should include module_a node. Found: {all_vars:?}"
        );
        assert!(
            all_vars.contains("module_b"),
            "Should include module_b node. Found: {all_vars:?}"
        );
    }

    #[test]
    fn test_loop_through_three_modules() {
        use crate::testutils::x_module;

        // stock -> module_a -> aux1 -> module_b -> aux2 -> module_c -> result -> stock
        let main_model = x_model(
            "main",
            vec![
                x_stock("level", "50", &["adjustment"], &[], None),
                x_flow("adjustment", "output_c", None),
                x_module("module_a", &[("level", "module_a\u{00B7}input")], None),
                x_aux("mid1", "module_a", None),
                x_module("module_b", &[("mid1", "module_b\u{00B7}input")], None),
                x_aux("mid2", "module_b", None),
                x_module("module_c", &[("mid2", "module_c\u{00B7}input")], None),
                x_aux("output_c", "module_c * 0.1", None),
            ],
        );

        let make_module = |name: &str, stock_name: &str| {
            x_model(
                name,
                vec![
                    x_aux("input", "0", None),
                    x_stock(stock_name, "0", &["fill"], &[], None),
                    x_flow("fill", &format!("(input - {stock_name}) / 2"), None),
                    x_aux("output", stock_name, None),
                ],
            )
        };

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(
            sim_specs,
            &[
                main_model,
                make_module("module_a", "buf_a"),
                make_module("module_b", "buf_b"),
                make_module("module_c", "buf_c"),
            ],
        );
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);

        assert!(
            !detected.loops.is_empty(),
            "Should detect a loop through three modules"
        );

        // The salsa path treats modules as black-box nodes: loop variables
        // include the module names and the parent stock.
        let all_vars: HashSet<&str> = detected
            .loops
            .iter()
            .flat_map(|l| l.variables.iter().map(|s| s.as_str()))
            .collect();

        assert!(
            all_vars.contains("level"),
            "Should include parent stock. Found: {all_vars:?}"
        );
        assert!(
            all_vars.contains("module_a"),
            "Should include module_a node. Found: {all_vars:?}"
        );
        assert!(
            all_vars.contains("module_b"),
            "Should include module_b node. Found: {all_vars:?}"
        );
        assert!(
            all_vars.contains("module_c"),
            "Should include module_c node. Found: {all_vars:?}"
        );
    }

    #[test]
    fn test_internal_module_loops_not_in_parent() {
        use crate::testutils::x_module;

        // A model with a module that has internal feedback,
        // but no feedback loop in the parent model.
        let main_model = x_model(
            "main",
            vec![
                x_aux("input_signal", "10", None),
                x_module(
                    "smoother",
                    &[("input_signal", "smoother\u{00B7}input")],
                    None,
                ),
                x_aux("result", "smoother", None),
            ],
        );

        // Module with internal feedback: smoothed -> change_in_smooth -> smoothed
        let smooth_model = x_model(
            "smoother",
            vec![
                x_aux("input", "0", None),
                x_stock("smoothed", "0", &["change_in_smooth"], &[], None),
                x_flow("change_in_smooth", "(input - smoothed) / smooth_time", None),
                x_aux("smooth_time", "3", None),
                x_aux("output", "smoothed", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let datamodel_project = x_project(sim_specs, &[main_model, smooth_model]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &datamodel_project);
        let model = result.models["main"].source;
        let detected = model_detected_loops(&db, model, result.project);

        // No feedback loop exists in the parent model (no path from result back
        // to input_signal). The module's INTERNAL feedback loop should NOT be
        // reported at the parent level.
        assert!(
            detected.loops.is_empty(),
            "Internal module loops should not appear in parent. Found: {:?}",
            detected
                .loops
                .iter()
                .map(|l| &l.variables)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_enumerate_pathways_to_outputs_non_standard_output() {
        // Module graph with output named "result" instead of "output"
        let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        edges.insert(Ident::new("input_val"), vec![Ident::new("intermediate")]);
        edges.insert(Ident::new("intermediate"), vec![Ident::new("result")]);

        let graph = CausalGraph {
            edges,
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        // enumerate_module_pathways with hard-coded "output" finds nothing
        let pathways_old = graph.enumerate_module_pathways(&Ident::new("output"));
        assert!(
            pathways_old.is_empty(),
            "Hard-coded 'output' should find no pathways when output is named 'result'"
        );

        // With explicit output ports, pathways are found correctly
        let pathways = graph.enumerate_pathways_to_outputs(&[Ident::new("result")]);
        assert!(
            !pathways.is_empty(),
            "Explicit output port should find pathways to 'result'"
        );
        assert!(
            pathways.contains_key(&Ident::new("input_val")),
            "Should find pathway from input_val to the sink"
        );

        // Auto-detection also works: "result" is a sink (no outgoing edges)
        let pathways_auto = graph.enumerate_pathways_to_outputs(&[]);
        assert!(
            !pathways_auto.is_empty(),
            "Auto-detected sink should find pathways to 'result'"
        );
    }

    #[test]
    fn test_enumerate_pathways_to_outputs_standard_output() {
        // Module graph with output named "output" (standard case)
        let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        edges.insert(Ident::new("input"), vec![Ident::new("output")]);

        let graph = CausalGraph {
            edges,
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        // Auto-detection: "output" node is a sink, so it's found automatically
        let pathways = graph.enumerate_pathways_to_outputs(&[]);
        assert!(
            !pathways.is_empty(),
            "Should find pathways with standard 'output' name"
        );
        assert!(pathways.contains_key(&Ident::new("input")));
    }

    /// Construct a three-node reinforcing cycle (A → B → C → A) for
    /// budget-exhaustion tests.  Tiny by design: the graph has exactly one
    /// elementary circuit, so a budget of zero MUST trip and a budget of
    /// one MUST succeed.
    fn tiny_cycle_graph() -> CausalGraph {
        let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        edges.insert(Ident::new("a"), vec![Ident::new("b")]);
        edges.insert(Ident::new("b"), vec![Ident::new("c")]);
        edges.insert(Ident::new("c"), vec![Ident::new("a")]);
        CausalGraph {
            edges,
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        }
    }

    #[test]
    fn find_circuit_node_lists_bails_out_past_budget() {
        let graph = tiny_cycle_graph();
        // Budget of zero: the very first circuit push should trip the
        // bail-out and return TruncatedByBudget.
        let err = graph
            .find_circuit_node_lists_with_limit(0)
            .expect_err("budget of 0 must bail");
        assert_eq!(err, TruncatedByBudget);
    }

    #[test]
    fn find_circuit_node_lists_succeeds_within_budget() {
        let graph = tiny_cycle_graph();
        let circuits = graph
            .find_circuit_node_lists_with_limit(usize::MAX)
            .expect("one-circuit graph must not exhaust the budget");
        assert_eq!(
            circuits.len(),
            1,
            "a three-node directed cycle has exactly one elementary circuit"
        );
    }

    #[test]
    fn find_loops_bails_out_past_budget() {
        let graph = tiny_cycle_graph();
        // Public `find_loops` hides the bail-out and returns an empty vec
        // so that callers get a "no LTM loops" signal and simulation can
        // still proceed.  Use the `_with_limit` variant to observe the
        // TruncatedByBudget marker directly.
        let err = graph
            .find_loops_with_limit(0)
            .expect_err("budget of 0 must bail");
        assert_eq!(err, TruncatedByBudget);
    }

    // --- IndexedGraph + SCC-restricted enumeration tests ---
    //
    // These validate the refactor from `HashSet<Ident>` to integer-indexed
    // DFS.  The goal is to pin down the invariants the public contract
    // depends on so later optimization passes can't silently break them:
    // (1) the node-ordering round-trip, (2) SCC decomposition behavior,
    // (3) self-loop exclusion at length 1, and (4) budget semantics.

    fn build_causal_graph(edges: &[(&str, &[&str])]) -> CausalGraph {
        let mut map: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        for (from, tos) in edges {
            let from_id = Ident::new(from);
            map.entry(from_id)
                .or_default()
                .extend(tos.iter().map(|t| Ident::new(t)));
        }
        CausalGraph {
            edges: map,
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        }
    }

    fn circuits_as_sorted_name_sets(circuits: &[Vec<Ident<Canonical>>]) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = circuits
            .iter()
            .map(|c| {
                let mut names: Vec<String> = c.iter().map(|n| n.as_str().to_string()).collect();
                names.sort();
                names
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn indexed_graph_empty_round_trip() {
        let edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        let graph = IndexedGraph::from_edges(&edges);
        assert_eq!(graph.len(), 0);
        assert!(graph.nodes.is_empty());
        assert!(graph.succ.is_empty());
        assert!(graph.node_to_idx.is_empty());

        let cg = CausalGraph {
            edges,
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };
        let circuits = cg
            .find_circuit_node_lists_with_limit(usize::MAX)
            .expect("empty graph must not trip the budget");
        assert!(circuits.is_empty(), "empty graph has no circuits");
    }

    #[test]
    fn indexed_graph_two_node_back_edge() {
        // A <-> B: one elementary circuit A -> B -> A.
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
        let graph = IndexedGraph::from_edges(&cg.edges);

        // Nodes must be sorted lex so small-start invariant matches index ordering.
        assert_eq!(
            graph.nodes.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        // Round-trip: every node's index resolves back to the same ident.
        for (i, n) in graph.nodes.iter().enumerate() {
            assert_eq!(graph.node_to_idx[n], i as u32);
        }

        let circuits = cg
            .find_circuit_node_lists_with_limit(usize::MAX)
            .expect("small graph must not exhaust budget");
        assert_eq!(
            circuits_as_sorted_name_sets(&circuits),
            vec![vec!["a".to_string(), "b".to_string()]]
        );
    }

    #[test]
    fn indexed_graph_three_node_cycle() {
        // A -> B -> C -> A: exactly one elementary circuit.
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
        let circuits = cg
            .find_circuit_node_lists_with_limit(usize::MAX)
            .expect("tiny cycle must not exhaust budget");
        assert_eq!(
            circuits_as_sorted_name_sets(&circuits),
            vec![vec!["a".to_string(), "b".to_string(), "c".to_string()]]
        );
    }

    #[test]
    fn indexed_graph_two_disjoint_three_cycles() {
        // Two completely disjoint cycles: {a,b,c} and {x,y,z}.
        // Each forms its own SCC so we must find exactly two circuits.
        let cg = build_causal_graph(&[
            ("a", &["b"]),
            ("b", &["c"]),
            ("c", &["a"]),
            ("x", &["y"]),
            ("y", &["z"]),
            ("z", &["x"]),
        ]);
        let circuits = cg
            .find_circuit_node_lists_with_limit(usize::MAX)
            .expect("small disjoint graphs must not exhaust budget");
        let sorted = circuits_as_sorted_name_sets(&circuits);
        assert_eq!(
            sorted,
            vec![
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec!["x".to_string(), "y".to_string(), "z".to_string()],
            ]
        );
    }

    #[test]
    fn indexed_graph_two_cycle_and_self_loop_node() {
        // A <-> B (one 2-cycle), and separately S -> S (self-loop).
        // Pure self-loops are intentionally excluded (circuit.len() > 1),
        // so only the A<->B cycle is returned.
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"]), ("s", &["s"])]);
        let circuits = cg
            .find_circuit_node_lists_with_limit(usize::MAX)
            .expect("small graph must not exhaust budget");
        assert_eq!(
            circuits_as_sorted_name_sets(&circuits),
            vec![vec!["a".to_string(), "b".to_string()]]
        );
    }

    #[test]
    fn indexed_graph_scc_pure_dag() {
        // Pure DAG: a -> b -> c, no cycles.  Tarjan must return only
        // trivial (size-1, no self-loop) SCCs.
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"])]);
        let graph = IndexedGraph::from_edges(&cg.edges);
        let sccs = graph.tarjan_scc();
        assert_eq!(sccs.len(), 3, "three nodes -> three trivial SCCs");
        for scc in &sccs {
            assert_eq!(scc.len(), 1, "DAG must have only singleton SCCs");
            let v = scc[0];
            assert!(
                !graph.succ[v as usize].contains(&v),
                "no self-loops in this DAG"
            );
        }
        // And `find_circuit_node_lists_with_limit` agrees: zero circuits.
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert!(circuits.is_empty());
    }

    #[test]
    fn indexed_graph_scc_two_disjoint_cycles() {
        // Two disjoint 3-cycles produce two non-trivial SCCs of size 3 each.
        let cg = build_causal_graph(&[
            ("a", &["b"]),
            ("b", &["c"]),
            ("c", &["a"]),
            ("x", &["y"]),
            ("y", &["z"]),
            ("z", &["x"]),
        ]);
        let graph = IndexedGraph::from_edges(&cg.edges);
        let sccs = graph.tarjan_scc();
        let non_trivial: Vec<_> = sccs
            .iter()
            .filter(|s| s.len() > 1 || graph.succ[s[0] as usize].contains(&s[0]))
            .collect();
        assert_eq!(
            non_trivial.len(),
            2,
            "two disjoint 3-cycles -> two non-trivial SCCs"
        );
        assert!(non_trivial.iter().all(|s| s.len() == 3));
    }

    #[test]
    fn indexed_graph_scc_figure_eight_single_scc() {
        // Figure-8: two cycles sharing node `m`.  Cycle 1: a -> m -> b -> a,
        // Cycle 2: c -> m -> d -> c.  All five nodes are mutually reachable
        // so Tarjan must return a single non-trivial SCC of size 5.
        let cg = build_causal_graph(&[
            ("a", &["m"]),
            ("m", &["b", "d"]),
            ("b", &["a"]),
            ("c", &["m"]),
            ("d", &["c"]),
        ]);
        let graph = IndexedGraph::from_edges(&cg.edges);
        let sccs = graph.tarjan_scc();
        let non_trivial: Vec<_> = sccs
            .iter()
            .filter(|s| s.len() > 1 || graph.succ[s[0] as usize].contains(&s[0]))
            .collect();
        assert_eq!(non_trivial.len(), 1, "figure-8 shares a node -> single SCC");
        assert_eq!(non_trivial[0].len(), 5);
    }

    #[test]
    fn indexed_graph_self_loop_only_yields_no_circuit() {
        // Graph with just A -> A must yield zero circuits because pure
        // self-loops (circuit.len() == 1) are intentionally excluded.
        let cg = build_causal_graph(&[("a", &["a"])]);
        let circuits = cg
            .find_circuit_node_lists_with_limit(usize::MAX)
            .expect("tiny graph must not exhaust budget");
        assert!(
            circuits.is_empty(),
            "pure self-loop must NOT produce a circuit"
        );
    }

    #[test]
    fn indexed_graph_zero_budget_nonempty_graph_truncates() {
        // Non-empty cycle + zero budget -> immediate TruncatedByBudget.
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
        let err = cg
            .find_circuit_node_lists_with_limit(0)
            .expect_err("zero budget on a cycle must truncate");
        assert_eq!(err, TruncatedByBudget);
    }

    #[test]
    fn indexed_graph_tiny_in_budget_succeeds() {
        // Matching positive control for the budget test: same cycle but
        // with an ample budget must succeed and return exactly one circuit.
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
        let circuits = cg
            .find_circuit_node_lists_with_limit(usize::MAX)
            .expect("ample budget must succeed");
        assert_eq!(circuits.len(), 1);
    }

    // ------------------------------------------------------------------
    // Johnson 1975 circuit-enumeration tests
    // ------------------------------------------------------------------
    //
    // The tests below exercise the production Johnson's enumerator on
    // targeted graph shapes and cross-check it against the Tiernan oracle
    // retained under cfg(test) in the main IndexedGraph impl.  The
    // invariants we care about match the LTM public contract:
    //
    //   (1) exactly the same set of circuits as Tiernan (after
    //       canonicalizing each circuit's rotation),
    //   (2) each circuit emitted rotated to start at its lex-smallest
    //       node,
    //   (3) pure self-loops excluded (circuit.len() > 1 contract),
    //   (4) budget semantics: TruncatedByBudget on exactly the
    //       max_circuits + 1st circuit,
    //   (5) cross-SCC edges never traversed.

    /// Canonicalize a list of circuits to the set of sorted node-sets.
    /// LTM semantics fold distinct rotations with the same node-set into
    /// one loop (see `deduplicate_loops` and the multidigraph handling
    /// in `johnson_circuit`).  The post-refactor Johnson's deduplicates
    /// inline during emission; the Tiernan oracle still emits every
    /// rotation.  Reducing to sorted + deduped node-sets puts them on
    /// equal footing for equivalence comparisons.
    fn canonicalize_circuits(circuits: Vec<Vec<u32>>) -> Vec<Vec<u32>> {
        let mut keys: Vec<Vec<u32>> = circuits
            .into_iter()
            .map(|mut c| {
                c.sort_unstable();
                c
            })
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Run both Johnson's (production) and Tiernan (oracle) on a graph
    /// and assert their canonicalized node-set coverage is equal.
    fn assert_johnson_matches_tiernan(cg: &CausalGraph) {
        let graph = IndexedGraph::from_edges(cg.edges());
        let sccs = graph.tarjan_scc();

        let mut johnson_circuits: Vec<Vec<u32>> = Vec::new();
        let mut budget_j = usize::MAX;
        for scc in &sccs {
            let mut part = graph.enumerate_circuits_in_scc(scc, &mut budget_j).unwrap();
            johnson_circuits.append(&mut part);
        }

        let mut tiernan_circuits: Vec<Vec<u32>> = Vec::new();
        let mut budget_t = usize::MAX;
        for scc in &sccs {
            let mut part = graph
                .enumerate_circuits_in_scc_tiernan(scc, &mut budget_t)
                .unwrap();
            tiernan_circuits.append(&mut part);
        }

        let johnson_canon = canonicalize_circuits(johnson_circuits);
        let tiernan_canon = canonicalize_circuits(tiernan_circuits);
        assert_eq!(
            johnson_canon,
            tiernan_canon,
            "Johnson's and Tiernan node-set coverage disagrees on edges {:?}",
            cg.edges()
        );
    }

    #[test]
    fn johnson_empty_graph_no_circuits() {
        let cg = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert!(circuits.is_empty(), "empty graph must have zero circuits");
    }

    #[test]
    fn johnson_pure_dag_no_circuits() {
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"]), ("a", &["c"])]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert!(circuits.is_empty(), "pure DAG has no circuits");
    }

    #[test]
    fn johnson_single_self_loop_excluded() {
        // Pure self-loop A -> A: path.len() == 1 is filtered.
        let cg = build_causal_graph(&[("a", &["a"])]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert!(circuits.is_empty(), "pure self-loop must not be emitted");
    }

    #[test]
    fn johnson_two_node_back_edge_single_circuit() {
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert_eq!(circuits.len(), 1);
        assert_eq!(
            circuits[0].iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"],
            "circuit must be rotated to start at lex-smallest node"
        );
    }

    #[test]
    fn johnson_three_node_cycle_single_circuit() {
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert_eq!(circuits.len(), 1);
        assert_eq!(
            circuits[0].iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }

    #[test]
    fn johnson_two_disjoint_cycles_two_circuits() {
        let cg = build_causal_graph(&[
            ("a", &["b"]),
            ("b", &["c"]),
            ("c", &["a"]),
            ("x", &["y"]),
            ("y", &["z"]),
            ("z", &["x"]),
        ]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        let names = circuits_as_sorted_name_sets(&circuits);
        assert_eq!(
            names,
            vec![
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec!["x".to_string(), "y".to_string(), "z".to_string()],
            ]
        );
    }

    #[test]
    fn johnson_figure_8_shared_vertex_two_circuits() {
        // Two cycles sharing node `m`:
        //   cycle 1: a -> m -> b -> a
        //   cycle 2: c -> m -> d -> c
        let cg = build_causal_graph(&[
            ("a", &["m"]),
            ("m", &["b", "d"]),
            ("b", &["a"]),
            ("c", &["m"]),
            ("d", &["c"]),
        ]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        let names = circuits_as_sorted_name_sets(&circuits);
        assert_eq!(
            names,
            vec![
                vec!["a".to_string(), "b".to_string(), "m".to_string()],
                vec!["c".to_string(), "d".to_string(), "m".to_string()],
            ]
        );
    }

    #[test]
    fn johnson_complete_k3_all_directed_cycles() {
        // K3 with all 6 directed edges.  Elementary directed cycles:
        //   3 two-cycles: {a,b}, {a,c}, {b,c}
        //   2 three-cycles: a->b->c->a, a->c->b->a
        // Dedup merges the two 3-cycles (same node set {a,b,c}) so the
        // public API reports 4 circuits.
        let cg = build_causal_graph(&[("a", &["b", "c"]), ("b", &["a", "c"]), ("c", &["a", "b"])]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        let names = circuits_as_sorted_name_sets(&circuits);
        assert_eq!(
            names,
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec!["a".to_string(), "c".to_string()],
                vec!["b".to_string(), "c".to_string()],
            ]
        );
    }

    #[test]
    fn johnson_zero_budget_bails_immediately() {
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"])]);
        let err = cg
            .find_circuit_node_lists_with_limit(0)
            .expect_err("zero budget on a cycle must truncate");
        assert_eq!(err, TruncatedByBudget);
    }

    #[test]
    fn johnson_respects_shared_budget_across_sccs() {
        // Two disjoint 2-cycles and a generous-but-finite budget.  Both
        // cycles fit; budget remains positive at the end.
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["a"]), ("c", &["d"]), ("d", &["c"])]);
        let circuits = cg.find_circuit_node_lists_with_limit(2).unwrap();
        assert_eq!(circuits.len(), 2);

        // Budget of 1 can emit only one of the two cycles before bailing.
        let err = cg
            .find_circuit_node_lists_with_limit(1)
            .expect_err("budget of 1 cannot fit both 2-cycles");
        assert_eq!(err, TruncatedByBudget);
    }

    #[test]
    fn johnson_budget_charges_duplicate_raw_cycles() {
        // Complete directed graph on 4 nodes (K4) with every pair
        // bidirectional.  The DFS discovers many raw elementary cycles
        // that collapse to fewer distinct node-sets under dedup:
        //   3 two-cycles: {a,b}, {a,c}, {a,d}, {b,c}, {b,d}, {c,d}
        //   many three-cycles, all over three-node subsets
        //   many four-cycles, all over {a,b,c,d}
        // The raw DFS work -- not the unique output size -- is what
        // can blow up compile time on dense multidigraphs, so
        // `max_circuits` must bound raw cycle discovery, not post-
        // dedup output.  Budget decrement fires on every raw emission
        // so callers cannot have the DFS run for longer than the cap
        // implies.
        let cg = build_causal_graph(&[
            ("a", &["b", "c", "d"]),
            ("b", &["a", "c", "d"]),
            ("c", &["a", "b", "d"]),
            ("d", &["a", "b", "c"]),
        ]);
        // Unique node-sets: C(4,2) + C(4,3) + C(4,4) = 6 + 4 + 1 = 11.
        let full = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert_eq!(
            full.len(),
            11,
            "K4 has exactly 11 distinct node-set circuits after dedup"
        );

        // Raw (pre-dedup) cycle count for K4 is much larger; a budget
        // that fits unique output but not raw work must still trip.
        // If budget is charged per unique circuit the enumeration runs
        // past the cap -- exactly the regression we're guarding.
        let err = cg
            .find_circuit_node_lists_with_limit(11)
            .expect_err("budget of 11 must trip because raw cycle discovery exceeds 11");
        assert_eq!(err, TruncatedByBudget);
    }

    #[test]
    fn johnson_circuit_emitted_from_lex_smallest_node() {
        // Construct a cycle where the lex-smallest node is NOT the first
        // in the edge declarations so we can confirm rotation handling.
        // Edges listed starting from "c": c -> a -> b -> c.  The cycle's
        // lex-min is "a", so the emitted circuit is [a, b, c].
        let cg = build_causal_graph(&[("c", &["a"]), ("a", &["b"]), ("b", &["c"])]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert_eq!(circuits.len(), 1);
        assert_eq!(
            circuits[0].iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            vec!["a", "b", "c"],
            "circuit must be rotated to start at 'a'"
        );
    }

    #[test]
    fn johnson_unblock_transitive_chain() {
        // Graph designed to exercise the B[] chain unblocking.
        //
        //   a -> b
        //   b -> c, b -> e
        //   c -> a        (closes 1st cycle via b->c)
        //   c -> d
        //   d -> e        (d/e won't close by themselves from start=a)
        //   e -> a        (closes 2nd cycle via b->e)
        //
        // When DFS from start=a descends a->b->c->d->e, e has no cycle
        // back in its initial exploration; e registers as waiter of a in
        // its B[]. Later when b->e is explored and closes via e->a,
        // unblock cascades and correct cycle enumeration is preserved.
        let cg = build_causal_graph(&[
            ("a", &["b"]),
            ("b", &["c", "e"]),
            ("c", &["a", "d"]),
            ("d", &["e"]),
            ("e", &["a"]),
        ]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        let names = circuits_as_sorted_name_sets(&circuits);
        // Elementary directed cycles:
        //   a -> b -> c -> a            -> {a,b,c}
        //   a -> b -> e -> a            -> {a,b,e}
        //   a -> b -> c -> d -> e -> a  -> {a,b,c,d,e}
        assert_eq!(
            names,
            vec![
                vec!["a".to_string(), "b".to_string(), "c".to_string()],
                vec![
                    "a".to_string(),
                    "b".to_string(),
                    "c".to_string(),
                    "d".to_string(),
                    "e".to_string()
                ],
                vec!["a".to_string(), "b".to_string(), "e".to_string()],
            ]
        );
    }

    #[test]
    fn johnson_cross_scc_edges_not_traversed() {
        // Two SCCs connected by a cross-SCC edge.
        //   SCC 1: a <-> b
        //   SCC 2: x <-> y
        //   cross: b -> x (one-way; does not close any cycle)
        // Elementary circuits: {a,b} and {x,y}; the cross edge b->x
        // must NOT be traversed into SCC 2 when enumerating from SCC 1.
        let cg = build_causal_graph(&[
            ("a", &["b"]),
            ("b", &["a", "x"]),
            ("x", &["y"]),
            ("y", &["x"]),
        ]);
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        let names = circuits_as_sorted_name_sets(&circuits);
        assert_eq!(
            names,
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["x".to_string(), "y".to_string()],
            ]
        );
    }

    #[test]
    fn dedup_deterministic_across_calls() {
        // Multidigraph (K3 with all 6 directed edges) yields two
        // distinct directed 3-cycles sharing node set {a,b,c}; LTM
        // semantics fold those into one.  A non-deterministic hasher
        // could pick different representatives between calls on rare
        // collisions, which would silently invalidate the salsa
        // LoopCircuitsResult cache.  The rapidhash fingerprint is
        // content-addressed and deterministic (fixed seed, fixed
        // secret); calling twice on the same graph must produce
        // byte-identical output.
        let cg = build_causal_graph(&[("a", &["b", "c"]), ("b", &["a", "c"]), ("c", &["a", "b"])]);
        let r1 = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        let r2 = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert_eq!(
            r1, r2,
            "repeated enumeration of the same graph must be byte-identical"
        );

        // The indexed form must also be byte-identical, including the
        // trimmed name table and compact indices.
        let i1 = cg.find_indexed_circuits();
        let i2 = cg.find_indexed_circuits();
        assert_eq!(
            i1, i2,
            "repeated indexed enumeration must be byte-identical"
        );
    }

    #[test]
    fn find_indexed_circuits_trims_names_to_cycle_participants() {
        // Graph: non-cyclic feeder -> entry -> [ entry <-> loop_a <-> loop_b ]
        // feeder is acyclic and must NOT appear in the names table.
        // Keeping it would invalidate salsa caching whenever an acyclic
        // variable elsewhere in the project is renamed, even though the
        // loop structure is unchanged.
        let cg = build_causal_graph(&[
            ("feeder", &["entry"]),
            ("entry", &["loop_a"]),
            ("loop_a", &["loop_b"]),
            ("loop_b", &["entry"]),
        ]);
        let (names, circuits) = cg.find_indexed_circuits();
        assert_eq!(circuits.len(), 1, "exactly one elementary cycle");
        assert!(
            !names.iter().any(|n| n == "feeder"),
            "non-cyclic feeder must be excluded from the names table: {names:?}"
        );
        let mut expected = vec![
            "entry".to_string(),
            "loop_a".to_string(),
            "loop_b".to_string(),
        ];
        expected.sort();
        let mut got = names.clone();
        got.sort();
        assert_eq!(
            got, expected,
            "names table must contain exactly the cycle participants"
        );

        // Compact indices must all resolve to valid names-table entries.
        for c in &circuits {
            for &idx in c {
                assert!(
                    (idx as usize) < names.len(),
                    "compact index {idx} out of range"
                );
            }
        }
    }

    #[test]
    fn find_indexed_circuits_empty_on_dag_returns_empty_names() {
        // Pure DAG has no circuits; the (names, circuits) pair must both
        // be empty so salsa sees a stable "no LTM" result across any
        // rename/reshape of the DAG.
        let cg = build_causal_graph(&[("a", &["b"]), ("b", &["c"])]);
        let (names, circuits) = cg.find_indexed_circuits();
        assert!(circuits.is_empty(), "DAG has no circuits");
        assert!(
            names.is_empty(),
            "empty circuits must produce empty names table: {names:?}"
        );
    }

    #[test]
    fn find_loops_and_find_circuit_node_lists_agree_on_count() {
        // Both public APIs share `enumerate_indexed_circuits`, so their
        // circuit counts must remain in lock-step.  This guards against
        // accidental drift if future refactors thread a separate dedup
        // or filter through one path but not the other.
        let cg = build_causal_graph(&[
            ("a", &["b", "c"]),
            ("b", &["a", "c"]),
            ("c", &["a", "b"]),
            ("x", &["y"]),
            ("y", &["x"]),
        ]);
        let loops = cg.find_loops_with_limit(usize::MAX).unwrap();
        let circuits = cg.find_circuit_node_lists_with_limit(usize::MAX).unwrap();
        assert_eq!(
            loops.len(),
            circuits.len(),
            "find_loops and find_circuit_node_lists must produce the same count"
        );
    }

    #[test]
    fn johnson_matches_tiernan_on_fixture_corpus() {
        // Hand-curated corpus of graphs that exercise the Johnson/Tiernan
        // equivalence invariant.  Each entry is a list of (from, [to,...])
        // edge-list fragments, compared after canonicalization.
        let corpus: Vec<Vec<(&str, &[&str])>> = vec![
            // Empty graph
            vec![],
            // Single cycle
            vec![("a", &["b"]), ("b", &["c"]), ("c", &["a"])],
            // Two 2-cycles
            vec![("a", &["b"]), ("b", &["a"]), ("c", &["d"]), ("d", &["c"])],
            // Figure-8
            vec![
                ("a", &["m"]),
                ("m", &["b", "d"]),
                ("b", &["a"]),
                ("c", &["m"]),
                ("d", &["c"]),
            ],
            // K3 with all edges (multi-digraph, dedup exercises)
            vec![("a", &["b", "c"]), ("b", &["a", "c"]), ("c", &["a", "b"])],
            // Bowtie: two triangles sharing a single vertex
            vec![
                ("a", &["b"]),
                ("b", &["c"]),
                ("c", &["a"]),
                ("c", &["d"]),
                ("d", &["e"]),
                ("e", &["c"]),
            ],
            // Self-loop + non-trivial SCC (excluded self-loop)
            vec![("a", &["a"]), ("b", &["c"]), ("c", &["b"])],
            // Long chain with side spurs that don't close
            vec![
                ("a", &["b"]),
                ("b", &["c"]),
                ("c", &["d"]),
                ("d", &["a", "e"]),
                ("e", &["f"]),
                ("f", &["g"]),
            ],
            // Graph with many dead-end branches forcing Johnson's blocking to matter
            vec![
                ("a", &["b"]),
                ("b", &["c", "d", "e"]),
                ("c", &["a"]),
                ("d", &["f"]),
                ("e", &["g"]),
                ("f", &["h"]),
                ("g", &["h"]),
                ("h", &["a"]),
            ],
            // Arms race style: three-clique (every pair bidirectional)
            vec![
                ("alpha", &["beta", "gamma"]),
                ("beta", &["alpha", "gamma"]),
                ("gamma", &["alpha", "beta"]),
            ],
        ];

        for edges in &corpus {
            let cg = build_causal_graph(edges);
            assert_johnson_matches_tiernan(&cg);
        }
    }

    // ------------------------------------------------------------------
    // Property-based equivalence test: random small graphs.
    //
    // For each randomly generated graph, assert Johnson's and Tiernan
    // agree on the canonicalized set of elementary circuits.  The tight
    // bound (nodes <= 8, edges up to 16) keeps enumeration cheap while
    // giving the generator room to produce interesting SCC structures,
    // bidirectional edges, and self-loops.
    // ------------------------------------------------------------------

    use proptest::prelude::*;

    fn build_graph_from_pairs(n: usize, pairs: &[(u8, u8)]) -> CausalGraph {
        // Node names are "v0", "v1", ...  Use two-digit zero-padded
        // names so lex order matches numeric order (v01 < v02 < ... < v10).
        let names: Vec<String> = (0..n).map(|i| format!("v{i:02}")).collect();
        let mut edge_pairs: Vec<(String, String)> = Vec::new();
        for &(from_raw, to_raw) in pairs {
            let from = from_raw as usize % n;
            let to = to_raw as usize % n;
            edge_pairs.push((names[from].clone(), names[to].clone()));
        }
        // Deduplicate: HashMap::entry will overwrite but we need to
        // aggregate the adjacency list instead.
        let mut map: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for (f, t) in edge_pairs {
            if seen.insert((f.clone(), t.clone())) {
                map.entry(Ident::new(&f)).or_default().push(Ident::new(&t));
            }
        }
        CausalGraph {
            edges: map,
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Johnson's must agree with Tiernan on any random small graph.
        /// The generator draws a node count 2..=8 and up to 16 directed
        /// edges (possibly self-loops, possibly duplicates that we
        /// deduplicate).  Canonicalized circuit lists must match exactly.
        #[test]
        fn johnson_matches_tiernan_on_random_small_graphs(
            n in 2usize..=8,
            edges in prop::collection::vec((any::<u8>(), any::<u8>()), 0..=16),
        ) {
            let cg = build_graph_from_pairs(n, &edges);
            // Exercise via the CausalGraph public API in addition to the
            // direct IndexedGraph inspection, to catch divergences at the
            // API boundary (rotation, SCC ordering, empty-SCC handling).
            assert_johnson_matches_tiernan(&cg);
        }
    }
}
