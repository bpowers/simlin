// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Union-graph elementary-circuit enumeration: discovery mode's primary
//! candidate generator (design:
//! docs/design-plans/2026-08-10-ltm-discovery-union-enumeration.md).
//!
//! Mounted as a child of [`crate::ltm_finding`] via `#[path]` purely for the
//! per-file line cap; everything here is `pub(super)` implementation detail of
//! `discover_loops_with_graph`.
//!
//! The per-step strongest-first DFS bounds its work with a per-node expansion
//! cap, which silently degrades to a biased sampler on runtime-dense graphs
//! (World3: the cap saturates on 100% of searches and the report misses the
//! step-dominant loop at 57% of steps). Enumeration inverts the approach:
//! because discovery runs AFTER the simulation, the set of edges that ever
//! carried signal is observable, and every loop with a nonzero score at some
//! saved step is -- score being a product -- an elementary cycle of that
//! *union graph* all of whose edges are simultaneously active at that step.
//! Enumerating exactly the ever-simultaneously-active cycles (activity-bitset
//! pruning) yields a provably complete candidate set; cycles active only at
//! disjoint steps are never emitted, and the per-step DFS remains as the
//! fallback when the budgets or deadline trip.
//!
//! A cycle here always spans at least two variables. An elementary cycle never
//! repeats a node, so a self-edge can never be part of one of length >= 2, and
//! a one-variable "loop" is not feedback in the SD sense -- the same contract
//! compile-time exhaustive mode states as `circuit.len() > 1`
//! ([`crate::ltm::indexed`]) and `CausalGraph::order_variable_cycle` states as
//! `vars.len() < 2`. Self-edges are therefore dropped from the union graph at
//! build time rather than traversed and filtered later.

use std::collections::HashMap;
use std::time::Instant;

use super::{DEADLINE_CHECK_INTERVAL, IndexedSearch, MIN_CONTRIBUTION};
use crate::results::Results;

/// Maximum elementary circuits the union-graph enumerator may emit before
/// discovery falls back to the per-step DFS.
///
/// This deliberately exceeds compile-time exhaustive mode's
/// [`crate::ltm::MAX_LTM_CIRCUITS`] (100k): that constant is bounded by
/// per-loop `loop_score` synthetic-variable emission (the 65,536 VM
/// result-slot ceiling) and the `build_element_level_loops` materialization
/// cliff, neither of which applies here -- an enumerated circuit is a compact
/// `Vec<u32>` node path (World3's ever-simultaneously-active universe of
/// ~150k circuits is ~30 MB; the ~330k figure is its cycle count WITHOUT the
/// activity constraint, which this enumerator never materializes), and only
/// retention survivors are materialized as `FoundLoop`s. The binding
/// costs are the O(circuits x mean-length x steps) retention passes and the
/// circuit storage itself, both linear in this budget.
pub(super) const MAX_DISCOVERY_ENUM_CIRCUITS: usize = 1_000_000;

/// Maximum DFS edge-visits during enumeration.
///
/// The circuit budget alone does not bound work: a graph can force long
/// wandering paths that rarely close into cycles (the enumerator is
/// Tiernan-style -- on-path blocking only, no Johnson unblocking, because the
/// activity-bitset pruning below is path-dependent and breaks Johnson's
/// invariant). Each visit is a bitset AND plus bookkeeping (tens of ns), so
/// this bound caps enumeration at a few seconds of work. World3 -- the
/// densest runtime graph in the repo corpus -- completes its full ~150k-circuit
/// enumeration well under this bound.
pub(super) const MAX_DISCOVERY_ENUM_VISITS: u64 = 100_000_000;

#[cfg(test)]
thread_local! {
    /// Test-only overrides, scoped by [`EnumBudgetGuard`] -- tiny fixtures
    /// exercise the budget-trip fallback instead of graphs large enough to
    /// trip the production constants (docs/dev/rust.md#test-time-budgets).
    static ENUM_BUDGET_OVERRIDE: std::cell::Cell<Option<(usize, u64)>> =
        const { std::cell::Cell::new(None) };
}

/// The effective (circuit, visit) budgets for enumeration.
pub(super) fn enum_budgets() -> (usize, u64) {
    #[cfg(test)]
    {
        if let Some(b) = ENUM_BUDGET_OVERRIDE.with(|c| c.get()) {
            return b;
        }
    }
    (MAX_DISCOVERY_ENUM_CIRCUITS, MAX_DISCOVERY_ENUM_VISITS)
}

/// RAII guard (test-only) overriding [`enum_budgets`] for the current thread.
/// Restores the previous value on drop so a panicking test does not leak the
/// override to the next test on the same thread.
#[cfg(test)]
pub(crate) struct EnumBudgetGuard {
    prev: Option<(usize, u64)>,
}

#[cfg(test)]
impl EnumBudgetGuard {
    pub(crate) fn new(circuits: usize, visits: u64) -> Self {
        let prev = ENUM_BUDGET_OVERRIDE.with(|c| c.replace(Some((circuits, visits))));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for EnumBudgetGuard {
    fn drop(&mut self) {
        ENUM_BUDGET_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// The union-of-active-edges graph: the discovery edge set restricted to
/// non-self edges whose recorded |score| is nonzero (finite) at >= 1 saved
/// step, each edge carrying a word-packed activity bitset over steps
/// `1..step_count`.
///
/// Node ids are [`IndexedSearch`]'s (the enumerated circuits index straight
/// into its `idents`); edges here are unique per `(from, to)` because
/// `parse_link_offsets` dedupes and sorts its output.
pub(super) struct ActivityGraph {
    /// Per-node outbound edges: `(to, edge_row)`, in `IndexedSearch` order.
    pub adj: Vec<Vec<(u32, u32)>>,
    /// Flat per-edge activity bitsets: edge_row * words .. +words. Bit `t-1`
    /// is set when the edge's |score| at saved step `t` is finite nonzero
    /// (step 0 is skipped: scores there are NaN by construction, matching the
    /// per-step DFS's `1..step_count` sweep).
    bits: Vec<u64>,
    /// Words per edge bitset.
    words: usize,
    /// Per-edge-row result-slab offset (for the retention scoring passes).
    pub offsets: Vec<usize>,
    /// Per-node strongly-connected-component id of the union graph.
    pub scc_of: Vec<u32>,
}

impl ActivityGraph {
    /// Scan the results slab once and build the union graph + bitsets.
    pub(super) fn build(search: &IndexedSearch, results: &Results) -> ActivityGraph {
        let n_nodes = search.node_count();
        let step_count = results.step_count;
        let words = step_count.saturating_sub(1).div_ceil(64).max(1);

        let mut adj: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n_nodes];
        let mut bits: Vec<u64> = Vec::new();
        let mut offsets: Vec<usize> = Vec::new();

        for (from, edges) in search.adj.iter().enumerate() {
            for edge in edges {
                if edge.to as usize == from {
                    // A self-edge can never be part of an elementary cycle of
                    // length >= 2 (such a cycle never repeats a node), and a
                    // one-variable "loop" is not feedback -- see the module
                    // doc. Dropping it here keeps it out of the traversal, the
                    // SCC structure, and the scoring rows alike.
                    continue;
                }
                let mut edge_bits = vec![0u64; words];
                let mut any = false;
                for step in 1..step_count {
                    let value = results.data[step * results.step_size + edge.offset];
                    if value != 0.0 && value.is_finite() || value.is_infinite() {
                        // Inf is "active" -- it is a real (divergent) signal
                        // the totals keep; only NaN and exact 0 are inactive,
                        // matching `load_step_scores`' NaN->0-then-drop rule.
                        let t = step - 1;
                        edge_bits[t / 64] |= 1u64 << (t % 64);
                        any = true;
                    }
                }
                if any {
                    let row = offsets.len() as u32;
                    adj[from].push((edge.to, row));
                    bits.extend_from_slice(&edge_bits);
                    offsets.push(edge.offset);
                }
            }
        }

        let scc_of = tarjan_scc_of(&adj, n_nodes);
        ActivityGraph {
            adj,
            bits,
            words,
            offsets,
            scc_of,
        }
    }

    #[inline]
    fn edge_bits(&self, row: u32) -> &[u64] {
        let start = row as usize * self.words;
        &self.bits[start..start + self.words]
    }
}

/// Iterative Tarjan over the union adjacency, returning each node's SCC id.
fn tarjan_scc_of(adj: &[Vec<(u32, u32)>], n_nodes: usize) -> Vec<u32> {
    const UNVISITED: u32 = u32::MAX;
    let mut index = vec![UNVISITED; n_nodes];
    let mut low = vec![0u32; n_nodes];
    let mut on_stack = vec![false; n_nodes];
    let mut scc_of = vec![0u32; n_nodes];
    let mut stack: Vec<u32> = Vec::new();
    let mut counter: u32 = 0;
    let mut scc_counter: u32 = 0;
    // (node, next-edge-index) work stack.
    let mut work: Vec<(u32, usize)> = Vec::new();

    for root in 0..n_nodes as u32 {
        if index[root as usize] != UNVISITED {
            continue;
        }
        index[root as usize] = counter;
        low[root as usize] = counter;
        counter += 1;
        stack.push(root);
        on_stack[root as usize] = true;
        work.push((root, 0));
        while let Some(&mut (v, ref mut ei)) = work.last_mut() {
            let vu = v as usize;
            if *ei < adj[vu].len() {
                let (w, _row) = adj[vu][*ei];
                *ei += 1;
                let wu = w as usize;
                if index[wu] == UNVISITED {
                    index[wu] = counter;
                    low[wu] = counter;
                    counter += 1;
                    stack.push(w);
                    on_stack[wu] = true;
                    work.push((w, 0));
                } else if on_stack[wu] {
                    low[vu] = low[vu].min(index[wu]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    let pu = parent as usize;
                    low[pu] = low[pu].min(low[vu]);
                }
                if low[vu] == index[vu] {
                    loop {
                        let w = stack.pop().expect("tarjan stack underflow");
                        on_stack[w as usize] = false;
                        scc_of[w as usize] = scc_counter;
                        if w == v {
                            break;
                        }
                    }
                    scc_counter += 1;
                }
            }
        }
    }
    scc_of
}

/// The outcome of a union-graph enumeration attempt.
pub(super) struct EnumeratedCandidates {
    /// Elementary circuits as node-id paths, each starting at its minimum
    /// node id (the canonical rotation, by construction of the min-root
    /// search). Every path holds at least two nodes: the union graph carries
    /// no self-edges.
    pub circuits: Vec<Vec<u32>>,
    /// `true` iff every branch was explored within the circuit/visit budgets
    /// and the deadline -- the emitted set is then provably the complete
    /// ever-simultaneously-active cycle universe of the recorded series.
    pub complete: bool,
}

/// Enumerate every elementary cycle of the union graph all of whose edges are
/// simultaneously active at >= 1 saved step.
///
/// Min-root Tiernan-style search: for each root `s` (ascending node id), walk
/// simple paths over nodes `> s` within `s`'s SCC, maintaining the running AND
/// of the path's edge-activity bitsets; a branch whose AND empties is pruned
/// (no extension can ever score nonzero), and a cycle is emitted only when the
/// AND including the closing edge is nonempty. Each cycle is emitted exactly
/// once, rooted at its minimum node id.
///
/// Returns `complete == false` (with the partial circuit list, which the
/// caller discards in favor of the DFS fallback) when the circuit budget, the
/// visit budget, or `deadline` trips.
pub(super) fn enumerate_active_circuits(
    graph: &ActivityGraph,
    deadline: Option<Instant>,
) -> EnumeratedCandidates {
    let (max_circuits, max_visits) = enum_budgets();
    let n_nodes = graph.adj.len();
    let words = graph.words;

    let mut circuits: Vec<Vec<u32>> = Vec::new();
    let mut visits: u64 = 0;

    // Per-DFS state, reused across roots.
    let mut on_path = vec![false; n_nodes];
    // path[d] is the node at depth d; and_stack[d] is the AND of the edge
    // bitsets along path[0..=d] (depth 0 carries the all-ones "empty path"
    // mask so the first edge ANDs against full).
    let mut path: Vec<u32> = Vec::new();
    let mut and_stack: Vec<u64> = Vec::new();
    // Work stack of (node, next-edge-index) frames, paralleling `path`.
    let mut frames: Vec<(u32, usize)> = Vec::new();

    let full_mask = vec![u64::MAX; words];

    for root in 0..n_nodes as u32 {
        let root_scc = graph.scc_of[root as usize];
        // A root with no outbound union edges cannot start a cycle.
        if graph.adj[root as usize].is_empty() {
            continue;
        }

        debug_assert!(path.is_empty() && frames.is_empty() && and_stack.is_empty());
        path.push(root);
        on_path[root as usize] = true;
        and_stack.extend_from_slice(&full_mask);
        frames.push((root, 0));

        'dfs: while !frames.is_empty() {
            let depth = frames.len(); // path.len() == depth
            let (v, ei) = frames[depth - 1];
            let vu = v as usize;
            if let Some(&(to, row)) = graph.adj[vu].get(ei) {
                frames[depth - 1].1 += 1;
                visits += 1;
                if visits & (DEADLINE_CHECK_INTERVAL as u64 - 1) == 0
                    && let Some(d) = deadline
                    && Instant::now() >= d
                {
                    break 'dfs;
                }
                if visits > max_visits {
                    break 'dfs;
                }

                // Only nodes >= root (min-root canonicalization) inside the
                // root's SCC can be on a cycle through the root.
                if to < root || graph.scc_of[to as usize] != root_scc {
                    continue;
                }

                // Running AND of path activity with this edge.
                let base = (depth - 1) * words;
                let ebits = graph.edge_bits(row);
                let mut nonzero = false;
                // Compute into a small stack buffer first; only push on descent.
                let mut new_and = [0u64; 8];
                let use_heap = words > 8;
                let mut heap_and: Vec<u64> = Vec::new();
                if use_heap {
                    heap_and.resize(words, 0);
                }
                for w in 0..words {
                    let v_ = and_stack[base + w] & ebits[w];
                    if use_heap {
                        heap_and[w] = v_;
                    } else {
                        new_and[w] = v_;
                    }
                    nonzero |= v_ != 0;
                }
                if !nonzero {
                    // No step at which the whole path-plus-edge is active;
                    // neither this cycle closure nor any extension can score.
                    continue;
                }

                if to == root {
                    circuits.push(path.clone());
                    if circuits.len() >= max_circuits {
                        break 'dfs;
                    }
                } else if !on_path[to as usize] {
                    path.push(to);
                    on_path[to as usize] = true;
                    if use_heap {
                        and_stack.extend_from_slice(&heap_and);
                    } else {
                        and_stack.extend_from_slice(&new_and[..words]);
                    }
                    frames.push((to, 0));
                }
            } else {
                frames.pop();
                let popped = path.pop().expect("path/frame parity");
                on_path[popped as usize] = false;
                and_stack.truncate(and_stack.len() - words);
            }
        }

        // A budget/deadline break leaves partial state; report incomplete.
        if !frames.is_empty() {
            for &n in &path {
                on_path[n as usize] = false;
            }
            path.clear();
            frames.clear();
            and_stack.clear();
            return EnumeratedCandidates {
                circuits,
                complete: false,
            };
        }
    }

    EnumeratedCandidates {
        circuits,
        complete: true,
    }
}

/// Per-circuit metadata the retention passes derive once.
struct CircuitMeta {
    /// Result-slab offsets of the circuit's edges, in path order.
    edge_offsets: Vec<usize>,
    /// `NormGroup`-determining partition: engine-internal cycle-partition
    /// index of the circuit's stocks, or `None` (Solo) when no stock resolves.
    partition: Option<usize>,
    /// Whether the circuit traverses a module-instance node (its final score
    /// may use the per-exit-port override series, so retention keeps it
    /// unconditionally rather than judging it by the raw product).
    traverses_module: bool,
}

/// The retention decision over the full enumerated universe.
pub(super) struct RetentionOutcome {
    /// Indices into the enumerated circuit list that survive retention.
    pub survivors: Vec<usize>,
    /// Per-engine-internal-partition per-step totals `Sum_j |score_j[t]|`
    /// over ALL enumerated circuits (NaN summands excluded, Inf kept),
    /// for `rank_and_filter`'s external-denominator path.
    pub partition_totals: HashMap<usize, Vec<f64>>,
}

/// Streaming two-pass retention over the enumerated circuits.
///
/// Pass 1 accumulates the per-partition per-step |score| totals; pass 2
/// recomputes each circuit's series and keeps it iff its peak per-step
/// relative contribution reaches [`MIN_CONTRIBUTION`] (`rank_and_filter`'s
/// retention rule, applied with full-universe denominators), or it belongs to
/// a Solo group and is ever active (its relative score is +/-1 by
/// construction), or it traverses a module node (conservative: the override
/// series may change its score). Nothing per-circuit larger than O(1) is
/// retained for non-survivors, so the pass is safe at
/// [`MAX_DISCOVERY_ENUM_CIRCUITS`] scale.
///
/// NaN link values follow the engine's loop-score rules: the loop's score at
/// that step is NaN, excluded from the totals and unable to satisfy retention
/// there. Deadline expiry mid-pass returns `None` (caller falls back to the
/// DFS).
pub(super) fn retain_circuits(
    circuits: &[Vec<u32>],
    graph: &ActivityGraph,
    results: &Results,
    stock_partition_of_node: &[Option<usize>],
    is_module_node: &[bool],
    deadline: Option<Instant>,
) -> Option<RetentionOutcome> {
    let step_count = results.step_count;

    // Edge lookup: (from, to) -> edge row. Built from the union adjacency.
    let mut edge_row: HashMap<(u32, u32), u32> = HashMap::new();
    for (from, edges) in graph.adj.iter().enumerate() {
        for &(to, row) in edges {
            edge_row.insert((from as u32, to), row);
        }
    }

    let metas: Vec<CircuitMeta> = circuits
        .iter()
        .map(|path| {
            let edge_offsets = path_edge_offsets(path, &edge_row, graph);
            let partition = path
                .iter()
                .find_map(|&n| stock_partition_of_node[n as usize]);
            let traverses_module = path.iter().any(|&n| is_module_node[n as usize]);
            CircuitMeta {
                edge_offsets,
                partition,
                traverses_module,
            }
        })
        .collect();

    // Pass 1: per-partition totals (Solo circuits are their own denominator,
    // so they need no shared accumulation).
    let mut partition_totals: HashMap<usize, Vec<f64>> = HashMap::new();
    let mut scratch = vec![0.0f64; step_count];
    let mut nan_mask = vec![false; step_count];
    for (ci, meta) in metas.iter().enumerate() {
        if deadline_expired(ci, deadline) {
            return None;
        }
        let Some(part) = meta.partition else { continue };
        score_series(&meta.edge_offsets, results, &mut scratch, &mut nan_mask);
        let totals = partition_totals
            .entry(part)
            .or_insert_with(|| vec![0.0; step_count]);
        for t in 0..step_count {
            if !nan_mask[t] {
                totals[t] += scratch[t].abs();
            }
        }
    }

    // Pass 2: retention.
    let mut survivors: Vec<usize> = Vec::new();
    for (ci, meta) in metas.iter().enumerate() {
        if deadline_expired(ci, deadline) {
            return None;
        }
        if meta.traverses_module {
            survivors.push(ci);
            continue;
        }
        score_series(&meta.edge_offsets, results, &mut scratch, &mut nan_mask);
        let keep = match meta.partition {
            Some(part) => {
                let totals = &partition_totals[&part];
                (0..step_count).any(|t| {
                    !nan_mask[t]
                        && totals[t] > 0.0
                        && scratch[t].abs() / totals[t] >= MIN_CONTRIBUTION
                })
            }
            // Solo: relative score is +/-1 whenever active, which passes the
            // retention threshold; "ever active" is the whole test.
            None => (0..step_count).any(|t| !nan_mask[t] && scratch[t] != 0.0),
        };
        if keep {
            survivors.push(ci);
        }
    }

    Some(RetentionOutcome {
        survivors,
        partition_totals,
    })
}

/// Add one loop's |score| mass into the external totals (used for stitched
/// cross-agg loops, which are appended after the enumeration passes but must
/// participate in the denominators like every other loop).
pub(super) fn accumulate_series_into_totals(
    path: &[u32],
    graph: &ActivityGraph,
    results: &Results,
    stock_partition_of_node: &[Option<usize>],
    partition_totals: &mut HashMap<usize, Vec<f64>>,
) {
    let Some(part) = path
        .iter()
        .find_map(|&n| stock_partition_of_node[n as usize])
    else {
        return;
    };
    let mut edge_row: HashMap<(u32, u32), u32> = HashMap::new();
    for (from, edges) in graph.adj.iter().enumerate() {
        for &(to, row) in edges {
            edge_row.insert((from as u32, to), row);
        }
    }
    let offsets = path_edge_offsets(path, &edge_row, graph);
    let step_count = results.step_count;
    let mut scratch = vec![0.0f64; step_count];
    let mut nan_mask = vec![false; step_count];
    score_series(&offsets, results, &mut scratch, &mut nan_mask);
    let totals = partition_totals
        .entry(part)
        .or_insert_with(|| vec![0.0; step_count]);
    for t in 0..step_count {
        if !nan_mask[t] {
            totals[t] += scratch[t].abs();
        }
    }
}

#[inline]
fn deadline_expired(i: usize, deadline: Option<Instant>) -> bool {
    i.is_multiple_of(4096) && deadline.is_some_and(|d| Instant::now() >= d)
}

/// Resolve a node path's consecutive-pair (wrapping) edge offsets. Every pair
/// is a union-graph edge by construction of the enumerator and the stitcher
/// (stitched sequences concatenate petals whose hops are all real edges).
fn path_edge_offsets(
    path: &[u32],
    edge_row: &HashMap<(u32, u32), u32>,
    graph: &ActivityGraph,
) -> Vec<usize> {
    (0..path.len())
        .map(|i| {
            let key = (path[i], path[(i + 1) % path.len()]);
            let row = edge_row
                .get(&key)
                .expect("enumerated/stitched path edge must exist in the union graph");
            graph.offsets[*row as usize]
        })
        .collect()
}

/// One loop's signed per-step score series (product of link values), with
/// `nan_mask[t]` set when any link value is NaN at `t` -- mirroring the
/// FoundLoop scoring rules exactly (NaN poisons the step; 0 and Inf multiply
/// through).
fn score_series(edge_offsets: &[usize], results: &Results, out: &mut [f64], nan_mask: &mut [bool]) {
    let step_size = results.step_size;
    for t in 0..out.len() {
        let base = t * step_size;
        let mut prod = 1.0f64;
        let mut has_nan = false;
        for &off in edge_offsets {
            let v = results.data[base + off];
            if v.is_nan() {
                has_nan = true;
                break;
            }
            prod *= v;
        }
        out[t] = if has_nan { f64::NAN } else { prod };
        nan_mask[t] = has_nan;
    }
}
