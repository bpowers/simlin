// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compact integer-indexed graph view plus the Johnson 1975 elementary-circuit
//! enumerator and Tarjan SCC decomposition that drive LTM loop detection.
//!
//! The Tiernan 1970 enumerator survives under `cfg(test)` as the oracle for
//! the Johnson-vs-Tiernan equivalence tests in `tests.rs`.

use std::collections::{HashMap, HashSet};

use crate::common::{Canonical, Ident};

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
pub(super) struct IndexedGraph {
    /// Sorted node identities; index into this vec is a `NodeIdx` (u32).
    pub(super) nodes: Vec<Ident<Canonical>>,
    /// Successor indices per node, each inner Vec sorted ascending.
    pub(super) succ: Vec<Vec<u32>>,
    /// Reverse map for translating external `Ident<Canonical>`s to indices.
    /// Retained on the struct so callers that hold an IndexedGraph can map
    /// caller-supplied idents to indices without rebuilding the map; in the
    /// current call sites it is only exercised via tests.
    #[allow(dead_code)]
    pub(super) node_to_idx: HashMap<Ident<Canonical>, u32>,
}

/// Marker that DFS exceeded the shared circuit budget.  Used internally by
/// `enumerate_circuits_in_scc` to propagate bail-out without confusion with
/// a successful empty-result enumeration.
#[derive(Debug)]
pub(super) struct TruncatedByBudgetInternal;

/// Result bundle from the shared indexed-circuit enumerator: the
/// IndexedGraph owns the node table (so callers can map indices back to
/// `Ident<Canonical>` on demand), and `circuits` is already deduplicated
/// by sorted-node-set.
pub(super) struct IndexedCircuits {
    pub(super) graph: IndexedGraph,
    pub(super) circuits: Vec<Vec<u32>>,
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
/// ## Indexing scheme
///
/// To keep transient memory proportional to the SCC and not the whole
/// indexed graph, `blocked` and `b_list` are sized to `scc.len()` and
/// indexed by **local** index (0..scc.len()).  The translation from
/// global node id (the `u32` index into [`IndexedGraph::nodes`]) to
/// local position lives in a separate `global_to_local: Vec<i32>` map
/// owned by [`IndexedGraph::enumerate_indexed_circuits`] (sized
/// `nodes.len()`, sentinel `-1` for non-members).  That map is allocated
/// once per top-level enumeration call and reset between SCCs by walking
/// the previous SCC's members, so neither it nor `JohnsonState` ever
/// pays the whole-graph allocation cost more than once.
///
/// `path`, `circuits`, and `hash_scratch` continue to use **global**
/// indices because the public contract is that emitted circuits use
/// global node ids (downstream dedup and `build_element_level_loops`
/// rely on this).  The cycle-fingerprint hash is computed on the
/// already-global `path`, so dedup behaviour is identical to the
/// pre-refactor version.
///
/// `blocked` and `b_list` are reset for every start node within an SCC.
/// SCC membership is checked via the `global_to_local[w] >= 0` test on
/// the hot path, replacing the previous whole-graph `in_scc: Vec<bool>`.
/// `path` and `circuits` are reused across starts to amortize allocation.
struct JohnsonState {
    blocked: Vec<bool>,
    b_list: Vec<Vec<u32>>,
    path: Vec<u32>,
    circuits: Vec<Vec<u32>>,
    /// 64-bit rapidhash fingerprints of already-emitted circuits (by
    /// sorted node-index set).  Lets us reject duplicate node-set
    /// circuits during enumeration so peak memory stays proportional to
    /// unique output, not to the number of raw DFS emissions.
    ///
    /// Shared across all DFS starts within a single SCC.  Safe to
    /// share because Johnson's emits each cycle from its lex-smallest
    /// start (enforced by the `w < start` gate in `johnson_circuit`),
    /// so cycles surfaced from different starts already have disjoint
    /// node sets; the deduplication matters only *within* a single
    /// start, where a multidigraph SCC can emit multiple rotations
    /// over the same node set.  Different SCCs' node sets are disjoint
    /// outright; `JohnsonState` is rebuilt per SCC, so `seen` does not
    /// need manual clearing between SCCs.
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
///     2^64 birthday bound (~5e-8) -- the standard risk profile for any
///     64-bit hash on inputs of that cardinality, and dramatically
///     better than the ~400 MiB transient footprint of keying the
///     HashSet on `Vec<u32>`.  See tech-debt item #22 for the option of
///     promoting to 128-bit fingerprints if a future model regularly
///     enumerates past a few hundred thousand distinct circuits.
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
    pub(super) fn from_edges(edges: &HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>) -> Self {
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
        // Successor lists use first-seen insertion order to dedup
        // duplicate edges that `CausalGraph::from_model` can produce
        // (e.g. a flow that is both an inflow and an outflow of the
        // same stock).  We deliberately do NOT sort: on multidigraph
        // inputs (multiple directed cycles sharing a node set, e.g.
        // K_n with all edges bidirectional) the DFS visit order
        // determines which rotation survives the fingerprint dedup in
        // `johnson_circuit`, and we choose not to pin a canonical
        // "lex-smallest rotation" in production code -- pinning that
        // invariant would help only tests, not correctness or salsa
        // cache stability.  Within a single process HashMap iteration
        // is stable (same hasher seed), so repeat calls on the same
        // `CausalGraph` yield identical output; across processes the
        // multidigraph rotation is free to vary.  Downstream consumers
        // (`circuit_to_links`, polarity analysis, stock enrichment)
        // operate on whichever rotation is emitted and are robust to
        // the choice.
        for (from, tos) in edges {
            let fi = node_to_idx[from] as usize;
            let mut seen: HashSet<u32> = HashSet::new();
            for to in tos {
                let ti = node_to_idx[to];
                if seen.insert(ti) {
                    succ[fi].push(ti);
                }
            }
        }

        IndexedGraph {
            nodes,
            succ,
            node_to_idx,
        }
    }

    #[allow(dead_code)]
    pub(super) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Iterative Tarjan's algorithm on the full graph.  Iterative form keeps
    /// WRLD3-style graphs (>300 nodes, long cycles) off the recursion limit.
    /// SCCs are returned in first-discovery order; each inner `Vec<u32>` is
    /// sorted ascending so downstream iteration is deterministic regardless
    /// of the order nodes were popped from Tarjan's stack.
    pub(super) fn tarjan_scc(&self) -> Vec<Vec<u32>> {
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
                                // SCCs are returned in whatever order
                                // Tarjan's stack popped them; callers
                                // that need a specific iteration order
                                // over SCC members must sort themselves.
                                // `enumerate_circuits_in_scc` does not --
                                // each cycle surfaces from its
                                // index-smallest member by the
                                // `w < start` gate regardless of the
                                // order we try starts in.
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
    pub(super) fn enumerate_circuits_in_scc(
        &self,
        scc: &[u32],
        budget: &mut usize,
        global_to_local: &mut [i32],
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

        // Populate the local-index map for this SCC.  Caller passes
        // `global_to_local` filled with `-1` (or with the previous SCC's
        // entries already cleared); we set member entries to their
        // local index 0..scc.len() for the duration of this call and
        // reset them on the way out.  The `JohnsonState` buffers are
        // sized to `scc.len()` rather than the whole graph, so all
        // hot-path indexing into them goes through the local id.
        for (local, &v) in scc.iter().enumerate() {
            global_to_local[v as usize] = local as i32;
        }

        let mut state = JohnsonState {
            blocked: vec![false; scc.len()],
            b_list: vec![Vec::new(); scc.len()],
            path: Vec::with_capacity(scc.len()),
            circuits: Vec::new(),
            seen: HashSet::new(),
            hash_scratch: Vec::with_capacity(scc.len()),
        };

        for &start in scc {
            // Reset blocked/B[] for this start's search.  Reusing the
            // allocations across starts saves a per-start Vec<bool>/Vec<Vec>
            // construction cost.  O(|SCC|) per start; wrld3's 166-node SCC
            // and 166 starts is 27 K bool writes total -- trivial.  The
            // local-id range is exactly 0..scc.len() by construction
            // (see `JohnsonState` indexing notes), so we walk the
            // per-SCC buffers directly.
            for local in 0..scc.len() {
                state.blocked[local] = false;
                state.b_list[local].clear();
            }

            // Push start onto the DFS stack and block it before entering
            // the recursive CIRCUIT() subroutine -- the subroutine assumes
            // its caller has already done this.  `start` is a global id;
            // `start_local` is its position within the per-SCC buffers.
            let start_local = global_to_local[start as usize] as usize;
            state.blocked[start_local] = true;
            state.path.push(start);

            let result = self.johnson_circuit(start, start, &mut state, budget, global_to_local);

            // Always restore the path stack even on bail-out so nested
            // state is coherent if we ever chose to retry; since we
            // currently abort the full enumeration on budget exhaustion
            // this is defense-in-depth.
            state.path.pop();

            result?;
        }

        // Restore `global_to_local` for the next SCC.  Walking the SCC
        // members (rather than re-zeroing the full vec) keeps the cost
        // proportional to |SCC| not |graph|.
        for &v in scc {
            global_to_local[v as usize] = -1;
        }

        Ok(state.circuits)
    }

    /// Recursive CIRCUIT() of Johnson 1975.  Caller must have already:
    ///   * pushed `v` onto `state.path`
    ///   * set `state.blocked[global_to_local[v]] = true`
    ///
    /// Returns `Ok(true)` if any cycle was discovered through `v` (direct
    /// or via a descendant's recursive call); `Ok(false)` otherwise.
    /// The caller decides whether to unblock `v` based on this return
    /// value -- callers typically only need to concern themselves with
    /// that when `v == start` at the outermost frame, where unblocking
    /// happens inside the function itself.
    ///
    /// All identifier types follow the convention documented on
    /// [`JohnsonState`]: `v`, `start`, `w`, and `state.path` entries are
    /// **global** node ids (indices into [`IndexedGraph::nodes`]);
    /// `state.blocked` and `state.b_list` are indexed by **local** ids
    /// translated through `global_to_local`.  The cycle dedup hash is
    /// computed on `state.path` (global), so circuit fingerprints are
    /// identical to the pre-refactor implementation.
    fn johnson_circuit(
        &self,
        v: u32,
        start: u32,
        state: &mut JohnsonState,
        budget: &mut usize,
        global_to_local: &[i32],
    ) -> std::result::Result<bool, TruncatedByBudgetInternal> {
        let mut found_cycle = false;
        // Caller already established that v is an SCC member, so its
        // local id is non-negative; cache it once for the post-loop
        // unblock / waiter-registration paths to avoid redundant lookups.
        let v_local = global_to_local[v as usize] as usize;

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
            //
            // SCC membership comes from `global_to_local[w] >= 0`;
            // `-1` is the sentinel for non-members.  This replaces the
            // pre-refactor whole-graph `in_scc: Vec<bool>` and folds
            // the membership check into the same array we'd consult to
            // index `blocked` / `b_list` on the recurse path -- so the
            // hot loop performs one fewer array probe per neighbor.
            let w_local = global_to_local[w as usize];
            if w_local < 0 || w < start {
                continue;
            }
            let w_local = w_local as usize;

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
            } else if !state.blocked[w_local] {
                // Recurse into w: caller contract is "block and push
                // before call, pop after call, unblock only via the
                // Johnson unblock machinery".
                state.blocked[w_local] = true;
                state.path.push(w);
                let sub_found = self.johnson_circuit(w, start, state, budget, global_to_local)?;
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
            johnson_unblock(v_local as u32, state);
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
                let w_local = global_to_local[w as usize];
                if w_local < 0 || w < start {
                    continue;
                }
                state.b_list[w_local as usize].push(v_local as u32);
            }
        }

        Ok(found_cycle)
    }

    /// Tiernan 1970 enumeration kept as the test oracle for the
    /// Johnson-vs-Tiernan equivalence test.  Compiled only under
    /// `cfg(test)`; production code uses `enumerate_circuits_in_scc`
    /// (Johnson's).
    #[cfg(test)]
    pub(super) fn enumerate_circuits_in_scc_tiernan(
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
    pub(super) fn has_no_duplicate_node_sets(circuits: &[Vec<u32>]) -> bool {
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
    pub(super) fn circuit_to_idents(&self, circuit: &[u32]) -> Vec<Ident<Canonical>> {
        circuit
            .iter()
            .map(|&i| self.nodes[i as usize].clone())
            .collect()
    }
}

/// Johnson 1975 UNBLOCK(u).  Iterative implementation so deeply nested
/// B[] chains cannot overflow the recursion stack.
///
/// Called from `johnson_circuit` when a cycle has been found through `u`.
/// Sets `blocked[u] = false` and drains `b_list[u]`, cascading to unblock
/// every waiter that is still blocked.  `std::mem::take` empties the B[]
/// list so that if `u` is re-blocked later, its list is ready for a fresh
/// round of waiters.
///
/// `u` is a **local** index into the per-SCC `blocked` / `b_list`
/// buffers (see [`JohnsonState`] for the global/local indexing scheme).
/// Waiters stored in `b_list` are also local ids, so the cascade stays
/// inside the per-SCC arrays without ever touching the whole-graph
/// `global_to_local` map.  We carry locals as `u32` rather than `usize`
/// in the work stack because SCC sizes are bounded by graph node counts
/// (which already fit in `u32` everywhere else in this module), and
/// halving the stack-element width vs. `usize` matches the pre-refactor
/// allocation footprint on 64-bit targets exactly.
fn johnson_unblock(u: u32, state: &mut JohnsonState) {
    let mut stack: Vec<u32> = vec![u];
    while let Some(v) = stack.pop() {
        // A vertex can appear in multiple B[] lists; the first pop
        // transitions it to unblocked and any subsequent pop is a no-op.
        let v_idx = v as usize;
        if !state.blocked[v_idx] {
            continue;
        }
        state.blocked[v_idx] = false;
        let waiters = std::mem::take(&mut state.b_list[v_idx]);
        for w in waiters {
            if state.blocked[w as usize] {
                stack.push(w);
            }
        }
    }
}
