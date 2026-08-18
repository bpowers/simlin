// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Union-graph elementary-circuit enumeration: discovery mode's primary
//! candidate generator (design:
//! docs/design-plans/2026-08-17-ltm-discovery-exact.md).
//!
//! Mounted as a child of [`crate::ltm_finding`] via `#[path]` purely for the
//! per-file line cap; everything here is `pub(super)` implementation detail of
//! `discover_loops_with_graph`.
//!
//! Because discovery runs AFTER the simulation, the set of edges that ever
//! carried signal is observable, and every loop with a nonzero score at some
//! saved step is -- score being a product -- an elementary cycle of that
//! *union graph* all of whose edges are simultaneously active at that step.
//! Enumerating exactly the ever-simultaneously-active cycles (activity-bitset
//! pruning) therefore yields a provably COMPLETE candidate set rather than a
//! sample: cycles active only at disjoint steps are never emitted, and
//! nothing else is missed. The shortest-path fallback
//! (`ltm_finding_fallback.rs`) takes over when the budgets or the deadline
//! trip.
//!
//! A cycle here always spans at least two variables. An elementary cycle never
//! repeats a node, so a self-edge can never be part of one of length >= 2, and
//! a one-variable "loop" is not feedback in the SD sense -- the same contract
//! compile-time exhaustive mode states as `circuit.len() > 1`
//! ([`crate::ltm::indexed`]) and `CausalGraph::order_variable_cycle` states as
//! `vars.len() < 2`. Self-edges are therefore dropped from the union graph at
//! build time rather than traversed and filtered later.
//!
//! Circuits are emitted as **edge-row sequences**, not node paths: a row
//! indexes both the edge's activity bitset and its contiguous per-step score
//! series, so retention scores a circuit without a single `(from, to)` lookup
//! and without striding the results slab. Node paths are derived only where a
//! consumer needs one ([`ActivityGraph::circuit_nodes`]).

use std::collections::HashMap;
use std::time::Instant;

use super::{Clock, DEADLINE_CHECK_INTERVAL, IndexedSearch, MIN_CONTRIBUTION, expired};
use crate::results::Results;

/// Maximum elementary circuits the union-graph enumerator may emit before
/// discovery falls back to the shortest-path sweep.
///
/// This deliberately exceeds compile-time exhaustive mode's
/// [`crate::ltm::MAX_LTM_CIRCUITS`] (100k): that constant is bounded by
/// per-loop `loop_score` synthetic-variable emission (the 65,536 VM
/// result-slot ceiling) and the `build_element_level_loops` materialization
/// cliff, neither of which applies here -- an enumerated circuit is a compact
/// run of `u32` edge rows (World3's ever-simultaneously-active universe of
/// ~150k circuits is ~25 MB of rows plus ~8 MB of activity bitsets; the ~330k
/// figure sometimes quoted is its cycle count WITHOUT the activity constraint,
/// which this enumerator never materializes), and only retention survivors are
/// materialized as `FoundLoop`s. The binding costs are the
/// O(edge-rows x active-steps) retention pass and the circuit storage itself,
/// bounded by this constant together with [`MAX_DISCOVERY_ENUM_EDGE_ROWS`].
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

/// Maximum total edge rows the enumerator may emit.
///
/// The circuit count bounds neither memory nor retention cost on its own: both
/// scale with `circuits x mean circuit length`, and mean length is a property
/// of the graph, not of the budget (World3's is ~42, so its ~150k circuits
/// carry ~6.3M rows). This is the memory bound the design promises -- 20M rows
/// is 80 MB of `u32` -- and it equally bounds the retention pass, whose work is
/// linear in emitted rows.
pub(super) const MAX_DISCOVERY_ENUM_EDGE_ROWS: u64 = 20_000_000;

/// How many recorded values [`ActivityGraph::build`] may copy between
/// wall-clock deadline checks.
///
/// The build's work is `union edges x step_count` slab reads, and either
/// factor alone can be the large one: World3 is ~430 edges over 401 saved
/// steps, while a two-variable goal-seeking model saved at 200k steps is 6
/// edges over 200,001. Counting VALUES rather than edges keeps one check
/// interval the same amount of work in both shapes -- the stride in edges is
/// derived from the step count, so the inner per-step loop stays check-free.
/// The first check happens before the first edge is copied, so an
/// already-expired deadline costs no work at all.
const ACTIVITY_BUILD_DEADLINE_CHECK_VALUES: usize = 1 << 16;

/// How many circuits [`retain_circuits`] scores between wall-clock deadline
/// checks. A circuit's scoring pass is O(active steps), so a few thousand of
/// them is the same order of work as the other phases' intervals; the first
/// check is at circuit 0, so an already-expired deadline is caught before any
/// scoring.
const RETENTION_DEADLINE_CHECK_CIRCUITS: usize = 4096;

/// The three enumeration budgets: circuits, edge-visits, emitted edge rows.
type EnumBudgets = (usize, u64, u64);

#[cfg(test)]
thread_local! {
    /// Test-only overrides, scoped by [`EnumBudgetGuard`] -- tiny fixtures
    /// exercise the budget-trip fallback instead of graphs large enough to
    /// trip the production constants (docs/dev/rust.md#test-time-budgets).
    static ENUM_BUDGET_OVERRIDE: std::cell::Cell<Option<EnumBudgets>> =
        const { std::cell::Cell::new(None) };
}

/// The effective (circuit, visit, edge-row) budgets for enumeration.
pub(super) fn enum_budgets() -> EnumBudgets {
    #[cfg(test)]
    {
        if let Some(b) = ENUM_BUDGET_OVERRIDE.with(|c| c.get()) {
            return b;
        }
    }
    (
        MAX_DISCOVERY_ENUM_CIRCUITS,
        MAX_DISCOVERY_ENUM_VISITS,
        MAX_DISCOVERY_ENUM_EDGE_ROWS,
    )
}

/// RAII guard (test-only) overriding [`enum_budgets`] for the current thread.
/// Restores the previous value on drop so a panicking test does not leak the
/// override to the next test on the same thread.
#[cfg(test)]
pub(crate) struct EnumBudgetGuard {
    prev: Option<EnumBudgets>,
}

#[cfg(test)]
impl EnumBudgetGuard {
    pub(crate) fn new(circuits: usize, visits: u64, edge_rows: u64) -> Self {
        let prev = ENUM_BUDGET_OVERRIDE.with(|c| c.replace(Some((circuits, visits, edge_rows))));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for EnumBudgetGuard {
    fn drop(&mut self) {
        ENUM_BUDGET_OVERRIDE.with(|c| c.set(self.prev));
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only override of [`enum_deadline_visit_interval`], scoped by an
    /// active [`EnumDeadlineVisitIntervalGuard`] -- lets a two-triangle
    /// fixture exercise the in-search deadline check instead of needing a
    /// graph deep enough to reach 8192 edge visits
    /// (docs/dev/rust.md#test-time-budgets).
    static ENUM_DEADLINE_VISIT_INTERVAL_OVERRIDE: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

/// How many edge visits [`enumerate_active_circuits`] may make between
/// wall-clock deadline checks. Must stay a power of two: the test is a single
/// mask.
fn enum_deadline_visit_interval() -> u64 {
    #[cfg(test)]
    {
        if let Some(interval) = ENUM_DEADLINE_VISIT_INTERVAL_OVERRIDE.with(|c| c.get()) {
            debug_assert!(interval.is_power_of_two());
            return interval;
        }
    }
    DEADLINE_CHECK_INTERVAL as u64
}

/// RAII guard (test-only) overriding [`enum_deadline_visit_interval`] for the
/// current thread; restores the previous value on drop so a panicking test
/// does not leak the override to the next test on the same thread.
#[cfg(test)]
pub(crate) struct EnumDeadlineVisitIntervalGuard {
    prev: Option<u64>,
}

#[cfg(test)]
impl EnumDeadlineVisitIntervalGuard {
    pub(crate) fn new(interval: u64) -> Self {
        let prev = ENUM_DEADLINE_VISIT_INTERVAL_OVERRIDE.with(|c| c.replace(Some(interval)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for EnumDeadlineVisitIntervalGuard {
    fn drop(&mut self) {
        ENUM_DEADLINE_VISIT_INTERVAL_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// The union-of-active-edges graph: the discovery edge set restricted to
/// non-self edges whose recorded |score| is nonzero (finite) at >= 1 saved
/// step in `1..step_count`, each edge carrying a word-packed activity bitset
/// and a contiguous copy of its signed per-step score series.
///
/// Node ids are [`IndexedSearch`]'s (the enumerated circuits' node paths index
/// straight into its `idents`); edges here are unique per `(from, to)` because
/// `parse_link_offsets` dedupes and sorts its output, so an edge row is a
/// complete identity for a `(from, to)` pair.
pub(super) struct ActivityGraph {
    /// Per-node outbound edges: `(to, edge_row)`, in `IndexedSearch` order.
    adj: Vec<Vec<(u32, u32)>>,
    /// Per-node inbound neighbours, for the per-root induced-SCC computation.
    radj: Vec<Vec<u32>>,
    /// Flat per-edge activity bitsets: `edge_row * words .. +words`. Bit `t` is
    /// set when the edge's score at saved step `t` is finite nonzero, or
    /// infinite (a real divergent signal the totals keep; only NaN and exact 0
    /// are inactive -- the same test [`Self::build`] applies per step below).
    ///
    /// Bit 0 is carried but never satisfies the enumerator's activity test
    /// (the `head & !1u64` mask in [`enumerate_active_circuits`]): step 0
    /// (`TIME = INITIAL_TIME`) is every link-score equation's own first-step
    /// guard arm, which every generator (`ltm_augment::link_score_guard_form_with_numerator`
    /// and its module-composite twin) emits as the literal constant `0`, so a
    /// cycle active only there is not a scorable loop. In today's production
    /// output bit 0 is therefore never actually SET (every link score is
    /// exactly 0 there), which makes the mask inert rather than load-bearing
    /// -- it exists as defense in depth against a link score that is ever
    /// genuinely nonzero at step 0. A circuit whose activity AND is nonempty
    /// ONLY at step 0 is never emitted at all (masked out at every depth of
    /// the search, not merely windowed out of scoring), so such a circuit's
    /// mass is excluded from the universe -- and hence from the partition
    /// totals -- entirely, matching the "step 0 is not a scorable loop" rule.
    /// Carrying the bit anyway is what keeps [`Self::active_window`] exact for
    /// every circuit that IS emitted (one whose activity AND also has a bit
    /// set at some step >= 1): its window can start at step 0 rather than
    /// silently dropping a genuinely active first step from the totals.
    bits: Vec<u64>,
    /// Words per edge bitset.
    words: usize,
    /// Flat per-edge signed score series: `edge_row * step_count .. +step_count`.
    /// Copied once from the results slab so every scoring pass reads
    /// contiguously instead of striding by `step_size`.
    series: Vec<f64>,
    /// Number of saved steps (the stride of [`Self::series`]).
    step_count: usize,
    /// Per-edge-row source node, so a circuit's node path is O(1) per row.
    edge_from: Vec<u32>,
    /// Per-node strongly-connected-component id of the union graph.
    scc_of: Vec<u32>,
}

impl ActivityGraph {
    /// Scan the results slab once and build the union graph, its activity
    /// bitsets, and its contiguous score rows.
    ///
    /// Returns `None` when `deadline` passes part way: the build is the first
    /// phase of the enumeration path and can itself be the expensive one on a
    /// model saved at many steps, so a caller whose budget is already spent
    /// has to be able to abandon it and go straight to the fallback rather
    /// than copy a slab it will discard. The clock is read at most once per
    /// [`ACTIVITY_BUILD_DEADLINE_CHECK_VALUES`] values copied, and an
    /// unbudgeted build never reads it at all.
    pub(super) fn build(
        search: &IndexedSearch,
        results: &Results,
        deadline: Option<Instant>,
        clock: &mut dyn Clock,
    ) -> Option<ActivityGraph> {
        let n_nodes = search.node_count();
        let step_count = results.step_count;
        let words = step_count.div_ceil(64).max(1);
        let check_stride = (ACTIVITY_BUILD_DEADLINE_CHECK_VALUES / step_count.max(1)).max(1);
        let mut edges_scanned = 0usize;

        let mut adj: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n_nodes];
        let mut radj: Vec<Vec<u32>> = vec![Vec::new(); n_nodes];
        let mut bits: Vec<u64> = Vec::new();
        let mut series: Vec<f64> = Vec::new();
        let mut edge_from: Vec<u32> = Vec::new();

        let mut edge_bits = vec![0u64; words];
        let mut edge_series = vec![0.0f64; step_count];

        for (from, edges) in search.adj.iter().enumerate() {
            for edge in edges {
                if edges_scanned.is_multiple_of(check_stride) && expired(deadline, clock) {
                    return None;
                }
                edges_scanned += 1;
                if edge.to as usize == from {
                    // A self-edge can never be part of an elementary cycle of
                    // length >= 2 (such a cycle never repeats a node), and a
                    // one-variable "loop" is not feedback -- see the module
                    // doc. Dropping it here keeps it out of the traversal, the
                    // SCC structure, and the scoring rows alike.
                    continue;
                }
                edge_bits.iter_mut().for_each(|w| *w = 0);
                let mut any = false;
                for step in 0..step_count {
                    let value = results.data[step * results.step_size + edge.offset];
                    edge_series[step] = value;
                    if value != 0.0 && value.is_finite() || value.is_infinite() {
                        edge_bits[step / 64] |= 1u64 << (step % 64);
                        // Membership in the union graph is decided over
                        // `1..step_count` only, so a step-0-only edge is
                        // excluded -- the same window the fallback sweeps.
                        any |= step >= 1;
                    }
                }
                if any {
                    let row = edge_from.len() as u32;
                    adj[from].push((edge.to, row));
                    radj[edge.to as usize].push(from as u32);
                    bits.extend_from_slice(&edge_bits);
                    series.extend_from_slice(&edge_series);
                    edge_from.push(from as u32);
                }
            }
        }

        let scc_of = tarjan_scc_of(&adj, n_nodes);
        Some(ActivityGraph {
            adj,
            radj,
            bits,
            words,
            series,
            step_count,
            edge_from,
            scc_of,
        })
    }

    #[inline]
    fn edge_bits(&self, row: u32) -> &[u64] {
        let start = row as usize * self.words;
        &self.bits[start..start + self.words]
    }

    /// The edge row for `(from, to)`, or `None` when the pair is not a union
    /// edge. The scan is over one node's out-edges, which stay small on real
    /// models; nothing on the enumerated path needs it (rows are carried), only
    /// the stitched cross-agg sequences, which arrive as node paths.
    fn edge_row(&self, from: u32, to: u32) -> Option<u32> {
        self.adj
            .get(from as usize)?
            .iter()
            .find(|(t, _)| *t == to)
            .map(|(_, row)| *row)
    }

    /// The node path of a circuit given its edge rows: node `i` is the source
    /// of row `i`, and the closing row's target is node 0.
    pub(super) fn circuit_nodes(&self, rows: &[u32]) -> Vec<u32> {
        rows.iter()
            .map(|&row| self.edge_from[row as usize])
            .collect()
    }

    /// The source node of a single edge row, without allocating a node path.
    /// Lets a caller test a per-row predicate (e.g. "is this an agg node?")
    /// over a circuit's edge rows before deciding whether the full
    /// [`Self::circuit_nodes`] path is worth materializing.
    #[inline]
    pub(super) fn edge_source(&self, row: u32) -> u32 {
        self.edge_from[row as usize]
    }

    /// The half-open saved-step range `[lo, hi)` spanning every step at which
    /// the circuit whose activity AND is `and_bits` can be nonzero.
    ///
    /// Outside it at least one link is exactly 0 or NaN, so the circuit's score
    /// is 0 or NaN there: it adds no mass to a partition total and can satisfy
    /// no retention threshold, whichever it is. Restricting scoring to this
    /// window is therefore exact, and on World3 it is a 5x reduction (a mean
    /// window of 79 steps out of 401).
    fn active_window(&self, and_bits: &[u64]) -> (usize, usize) {
        let Some(first) = and_bits.iter().position(|w| *w != 0) else {
            return (0, 0);
        };
        let last = and_bits
            .iter()
            .rposition(|w| *w != 0)
            .expect("a nonzero word exists");
        let lo = first * 64 + and_bits[first].trailing_zeros() as usize;
        let hi = last * 64 + (63 - and_bits[last].leading_zeros() as usize) + 1;
        (lo, hi.min(self.step_count))
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

/// The per-root explorable node set: Johnson's `A_k`, the strongly-connected
/// component containing `root` in the subgraph induced by the union-SCC nodes
/// with id `>= root`.
///
/// Every elementary cycle whose minimum node is `root` lies entirely inside
/// that component, so restricting the search to it is exact -- and it is what
/// stops the min-root search from wandering down branches that provably cannot
/// return to the root (two thirds of World3's 20M descents).
///
/// Membership is stamped with a per-root generation counter rather than
/// cleared, so a root costs only the nodes and edges it actually reaches.
struct RootScc {
    generation: u32,
    /// `reaches_from[v] == generation` iff `v` is reachable from the root.
    reaches_from: Vec<u32>,
    /// `reaches_to[v] == generation` iff `v` both reaches and is reachable from
    /// the root -- i.e. `v` is in the root's induced SCC.
    reaches_to: Vec<u32>,
    stack: Vec<u32>,
}

impl RootScc {
    fn new(n_nodes: usize) -> Self {
        RootScc {
            generation: 0,
            reaches_from: vec![0; n_nodes],
            reaches_to: vec![0; n_nodes],
            stack: Vec::new(),
        }
    }

    /// Recompute the explorable set for `root`. Returns `false` when the
    /// component is trivial (fewer than two nodes), so no cycle is rooted here.
    fn recompute(&mut self, graph: &ActivityGraph, root: u32) -> bool {
        self.generation += 1;
        let generation = self.generation;
        let root_scc = graph.scc_of[root as usize];
        // Only nodes at or above the root, inside the root's union SCC, can be
        // on a cycle through it: min-root canonicalization supplies the first
        // bound, the SCC of the whole union graph the second.
        let admits = |v: u32| v >= root && graph.scc_of[v as usize] == root_scc;

        self.reaches_from[root as usize] = generation;
        self.stack.push(root);
        while let Some(v) = self.stack.pop() {
            for &(to, _) in &graph.adj[v as usize] {
                if admits(to) && self.reaches_from[to as usize] != generation {
                    self.reaches_from[to as usize] = generation;
                    self.stack.push(to);
                }
            }
        }

        // Intersecting "reachable from root" with "reaches root" is exactly the
        // root's strongly-connected component of the induced subgraph.
        let mut size = 0usize;
        self.reaches_to[root as usize] = generation;
        self.stack.push(root);
        while let Some(v) = self.stack.pop() {
            size += 1;
            for &from in &graph.radj[v as usize] {
                if admits(from)
                    && self.reaches_from[from as usize] == generation
                    && self.reaches_to[from as usize] != generation
                {
                    self.reaches_to[from as usize] = generation;
                    self.stack.push(from);
                }
            }
        }
        size >= 2
    }

    #[inline]
    fn contains(&self, node: u32) -> bool {
        self.reaches_to[node as usize] == self.generation
    }
}

/// The outcome of a union-graph enumeration attempt.
///
/// Circuits are stored compressed-row style: one flat `rows` array holding
/// every circuit's edge rows back to back, plus per-circuit bounds. That keeps
/// the emitted set to a handful of amortized-growth allocations instead of one
/// `Vec` per circuit, and makes [`MAX_DISCOVERY_ENUM_EDGE_ROWS`] a direct bound
/// on its memory.
pub(super) struct EnumeratedCandidates {
    /// Every circuit's edge rows, concatenated in emission order.
    rows: Vec<u32>,
    /// Circuit `i` occupies `rows[starts[i]..starts[i + 1]]`; `starts[0] == 0`.
    starts: Vec<usize>,
    /// Flat per-circuit activity AND: circuit `i` occupies
    /// `activity[i * words .. +words]`, the AND of its edges' activity bitsets
    /// as of the emission point -- exactly the steps at which the whole circuit
    /// is simultaneously active.
    activity: Vec<u64>,
    /// Words per activity bitset (matches the graph's).
    words: usize,
    /// `true` iff every branch was explored within the circuit/visit/edge-row
    /// budgets and the deadline -- the emitted set is then provably the complete
    /// ever-simultaneously-active cycle universe of the recorded series.
    pub complete: bool,
}

impl EnumeratedCandidates {
    fn new(words: usize) -> Self {
        EnumeratedCandidates {
            rows: Vec::new(),
            starts: vec![0],
            activity: Vec::new(),
            words,
            complete: true,
        }
    }

    /// Append a circuit given the edge rows of its open path plus the row that
    /// closes it, and the activity AND covering all of them.
    fn push(&mut self, open_rows: &[u32], closing_row: u32, and_bits: &[u64]) {
        debug_assert_eq!(and_bits.len(), self.words);
        self.rows.extend_from_slice(open_rows);
        self.rows.push(closing_row);
        self.starts.push(self.rows.len());
        self.activity.extend_from_slice(and_bits);
    }

    pub(super) fn len(&self) -> usize {
        self.starts.len() - 1
    }

    /// The edge rows of circuit `i`, closing edge included.
    pub(super) fn circuit(&self, i: usize) -> &[u32] {
        &self.rows[self.starts[i]..self.starts[i + 1]]
    }

    /// The steps at which circuit `i` is simultaneously active, word-packed.
    pub(super) fn activity_of(&self, i: usize) -> &[u64] {
        &self.activity[i * self.words..][..self.words]
    }

    /// Total emitted edge rows -- the quantity [`MAX_DISCOVERY_ENUM_EDGE_ROWS`]
    /// bounds.
    fn total_rows(&self) -> usize {
        self.rows.len()
    }
}

/// Enumerate every elementary cycle of the union graph all of whose edges are
/// simultaneously active at >= 1 saved step in `1..step_count`.
///
/// Min-root Tiernan-style search: for each root `s` (ascending node id), walk
/// simple paths inside `s`'s induced-subgraph SCC (see [`RootScc`]),
/// maintaining the running AND of the path's edge-activity bitsets; a branch
/// whose AND empties is pruned (no extension can ever score nonzero), and a
/// cycle is emitted only when the AND including the closing edge is nonempty.
/// Each cycle is emitted exactly once, rooted at its minimum node id.
///
/// Returns `complete == false` (with the partial circuit list, which the
/// caller discards in favor of the fallback) when the circuit budget, the
/// visit budget, the edge-row budget, or `deadline` trips.
pub(super) fn enumerate_active_circuits(
    graph: &ActivityGraph,
    deadline: Option<Instant>,
    clock: &mut dyn Clock,
) -> EnumeratedCandidates {
    let (max_circuits, max_visits, max_edge_rows) = enum_budgets();
    let visit_interval = enum_deadline_visit_interval();
    let n_nodes = graph.adj.len();
    let words = graph.words;

    let mut out = EnumeratedCandidates::new(words);
    let mut visits: u64 = 0;

    // Per-DFS state, reused across roots.
    let mut on_path = vec![false; n_nodes];
    // `frames[d]` is (node at depth d, its next-edge index); `edge_path[d]` is
    // the row of the edge from depth `d` to depth `d + 1`; `and_stack[d]` (a
    // `words`-wide slice) is the AND of the edge bitsets along the path down to
    // depth `d`, with depth 0 carrying the all-ones "empty path" mask so the
    // first edge ANDs against full.
    let mut frames: Vec<(u32, usize)> = Vec::new();
    let mut edge_path: Vec<u32> = Vec::new();
    let mut and_stack: Vec<u64> = Vec::new();

    let mut explorable = RootScc::new(n_nodes);

    for root in 0..n_nodes as u32 {
        if !explorable.recompute(graph, root) {
            continue;
        }

        debug_assert!(frames.is_empty() && edge_path.is_empty() && and_stack.is_empty());
        on_path[root as usize] = true;
        and_stack.resize(words, u64::MAX);
        frames.push((root, 0));

        'dfs: while !frames.is_empty() {
            let depth = frames.len();
            let (v, ei) = frames[depth - 1];
            if let Some(&(to, row)) = graph.adj[v as usize].get(ei) {
                frames[depth - 1].1 += 1;
                visits += 1;
                // `visits == 1` catches an ALREADY-expired deadline on the
                // very first visit regardless of graph size: without it, a
                // deadline that expired before this call even started would
                // go undetected on any graph whose total visit count never
                // reaches a `visit_interval` multiple -- true of almost every
                // real model at the production interval (`DEADLINE_CHECK_INTERVAL`
                // = 8192), so the whole enumeration would run to completion
                // on a budget that was already spent. The periodic
                // `visits & (visit_interval - 1) == 0` arm is what catches a
                // deadline that expires mid-search, after the first visit.
                if (visits == 1 || visits & (visit_interval - 1) == 0) && expired(deadline, clock) {
                    break 'dfs;
                }
                if visits > max_visits {
                    break 'dfs;
                }

                // Nodes outside the root's induced SCC can never close a cycle
                // through it, and a node already on the path would repeat.
                if !explorable.contains(to) || (to != root && on_path[to as usize]) {
                    continue;
                }

                // Running AND of the path's activity with this edge, written
                // straight onto `and_stack` (truncated again unless we descend),
                // so no per-visit allocation is needed at any bitset width.
                let base = and_stack.len() - words;
                let ebits = graph.edge_bits(row);
                and_stack.reserve(words);
                // Step 0 is never a scorable loop (see the `bits` field doc),
                // so mask its bit out of the emptiness test while still
                // carrying it, so windowed scoring stays exact.
                let head = and_stack[base] & ebits[0];
                let mut nonzero = head & !1u64 != 0;
                and_stack.push(head);
                for w in 1..words {
                    let word = and_stack[base + w] & ebits[w];
                    nonzero |= word != 0;
                    and_stack.push(word);
                }
                if !nonzero {
                    // No step at which the whole path-plus-edge is active;
                    // neither this cycle closure nor any extension can score.
                    and_stack.truncate(base + words);
                    continue;
                }

                if to == root {
                    out.push(&edge_path, row, &and_stack[base + words..]);
                    and_stack.truncate(base + words);
                    if out.len() >= max_circuits || out.total_rows() as u64 > max_edge_rows {
                        break 'dfs;
                    }
                } else {
                    on_path[to as usize] = true;
                    edge_path.push(row);
                    frames.push((to, 0));
                }
            } else {
                let (popped, _) = frames.pop().expect("frame stack is non-empty");
                on_path[popped as usize] = false;
                edge_path.pop();
                and_stack.truncate(and_stack.len() - words);
            }
        }

        // A budget/deadline break leaves partial state; report incomplete. The
        // caller discards the partial list (it is root-order-biased and its
        // totals are not the universe), so no unwinding is needed.
        if !frames.is_empty() {
            out.complete = false;
            return out;
        }
    }

    out
}

/// The retention decision over the full enumerated universe.
pub(super) struct RetentionOutcome {
    /// Indices into the enumerated circuit list that survive retention,
    /// ascending.
    pub survivors: Vec<usize>,
    /// Per-engine-internal-partition per-step totals `Sum_j |score_j[t]|`
    /// over ALL enumerated circuits (NaN summands excluded, Inf kept),
    /// for `rank_and_filter`'s external-denominator path.
    pub partition_totals: HashMap<usize, Vec<f64>>,
    /// Per-engine-internal-partition count of circuits in the enumerated
    /// UNIVERSE -- retention non-survivors included. How much company a loop
    /// has in its partition is a fact about the model, not about what cleared
    /// a threshold, so this is the population the competing-vs-solo
    /// classification is entitled to ask about.
    pub partition_circuit_counts: HashMap<usize, usize>,
}

/// Single-pass retention over the enumerated circuits, with a confirm step.
///
/// A circuit is retained iff at some saved step its |score| is at least
/// [`MIN_CONTRIBUTION`] of its partition's total |score| mass at that step
/// (`rank_and_filter`'s rule, applied with full-universe denominators). That
/// needs the FINAL totals, which the pass is still accumulating, so it is
/// answered in two parts:
///
/// 1. **Pass** (every circuit): score it, add its mass into its partition's
///    running total, and take `max_t |s(t)| / running_total(t)` -- the running
///    total including the circuit's own mass, so the ratio is defined even for
///    the first circuit of a partition. Because the running total only grows,
///    that is an UPPER bound of the circuit's true peak share, and a circuit
///    whose bound falls short is dropped without ever being scored again.
/// 2. **Confirm** (only circuits whose bound clears the threshold): recompute
///    the exact ratio against the final totals. The bound is loose for a
///    circuit that arrives early -- the first circuit of a partition bounds at
///    exactly 1.0 -- so this step is what makes the answer exact against the
///    totals THIS pass computes.
///
/// Two classes skip both. A circuit in a Solo group (no stock resolves to a
/// partition) is its own denominator, so its relative score is +/-1 wherever
/// it is active and "ever active" is the whole test -- which the enumerator
/// guarantees, since a circuit is emitted only with a nonempty activity AND;
/// the check that the AND is nonempty is kept as cheap defense in depth. And a
/// module-traversing circuit is kept unconditionally, because its reported
/// score may come from the per-exit-port override series rather than the raw
/// product judged here.
///
/// **Exact against these totals is not exact against the reported totals.**
/// `ltm_finding.rs` corrects the totals THIS pass returns, after
/// materialization: a module-traversing circuit's zero raw contribution is
/// replaced by its reported override series, and a trimmed duplicate
/// representative's raw mass (two enumerated circuits -- a direct reference
/// and its hoisted-reducer twin -- that trim to the same *reported* loop,
/// AC4.3) is subtracted back out. A module-traversing circuit is kept here
/// unconditionally regardless of the threshold, so only the dedup
/// subtraction can move a NON-module circuit's outcome, and it can only
/// LOWER a partition's denominator relative to what this pass saw. So a
/// non-module circuit this pass drops for falling short of
/// [`MIN_CONTRIBUTION`] against the pre-correction total could, against the
/// smaller post-correction one, have cleared it -- and having never been
/// materialized, it can never be reconsidered: `rank_and_filter`'s own
/// (exact, post-correction) retention pass only ever sees survivors of this
/// one. The error this can cause is bounded by the dropped duplicates' share
/// of the partition's final mass, which is nonzero only in a partition
/// containing a hoisted-reducer duplicate pathway; every other partition's
/// pre- and post-correction totals for non-module circuits agree exactly, so
/// this pass's decision there already IS the final one.
///
/// Nothing per-circuit larger than O(1) is retained for non-survivors, so the
/// pass is safe at [`MAX_DISCOVERY_ENUM_CIRCUITS`] scale.
///
/// A step whose product is not a number -- a NaN link, or the `Inf * 0` a
/// finite-overflow-then-zero circuit produces -- follows the engine's
/// loop-score rules: the loop's score there is NaN, excluded from the totals
/// and unable to satisfy retention. Deadline expiry mid-pass returns `None`
/// (caller falls back).
pub(super) fn retain_circuits(
    candidates: &EnumeratedCandidates,
    graph: &ActivityGraph,
    stock_partition_of_node: &[Option<usize>],
    is_module_node: &[bool],
    deadline: Option<Instant>,
    clock: &mut dyn Clock,
) -> Option<RetentionOutcome> {
    let step_count = graph.step_count;

    let mut partition_totals: HashMap<usize, Vec<f64>> = HashMap::new();
    let mut partition_circuit_counts: HashMap<usize, usize> = HashMap::new();
    let mut scratch = vec![0.0f64; step_count];
    let mut nan_mask = vec![false; step_count];

    let mut survivors: Vec<usize> = Vec::new();
    let mut to_confirm: Vec<usize> = Vec::new();

    for ci in 0..candidates.len() {
        if deadline_expired(ci, deadline, clock) {
            return None;
        }
        let rows = candidates.circuit(ci);
        let partition = circuit_partition(rows, graph, stock_partition_of_node);
        if let Some(part) = partition {
            *partition_circuit_counts.entry(part).or_insert(0) += 1;
        }
        if circuit_traverses_module(rows, graph, is_module_node) {
            // Kept, and contributing NO raw mass: what such a loop reports is
            // the per-exit-port override series, and the module composite this
            // product multiplies in max-abs-selects across ALL of the module's
            // output ports, so the two can differ by any factor. Its reported
            // mass joins the denominators after materialization instead.
            survivors.push(ci);
            continue;
        }

        let (lo, hi) = graph.active_window(candidates.activity_of(ci));
        let Some(part) = partition else {
            if lo < hi {
                survivors.push(ci);
            }
            continue;
        };

        score_steps(rows, graph, lo, &mut scratch[lo..hi], &mut nan_mask[lo..hi]);
        let totals = partition_totals
            .entry(part)
            .or_insert_with(|| vec![0.0; step_count]);
        let mut bound = 0.0f64;
        for t in lo..hi {
            if nan_mask[t] {
                continue;
            }
            let mass = scratch[t].abs();
            totals[t] += mass;
            if totals[t] > 0.0 {
                // `max` drops a NaN ratio (`Inf / Inf` at a dominance
                // inflection), which is right: the exact test rejects that step
                // too, so ignoring it costs no survivor.
                bound = bound.max(mass / totals[t]);
            }
        }
        if bound >= MIN_CONTRIBUTION {
            to_confirm.push(ci);
        }
    }

    for (n, &ci) in to_confirm.iter().enumerate() {
        if deadline_expired(n, deadline, clock) {
            return None;
        }
        let rows = candidates.circuit(ci);
        let part = circuit_partition(rows, graph, stock_partition_of_node)
            .expect("only partitioned circuits reach the confirm step");
        let (lo, hi) = graph.active_window(candidates.activity_of(ci));
        score_steps(rows, graph, lo, &mut scratch[lo..hi], &mut nan_mask[lo..hi]);
        let totals = &partition_totals[&part];
        let keep = (lo..hi).any(|t| {
            !nan_mask[t] && totals[t] > 0.0 && scratch[t].abs() / totals[t] >= MIN_CONTRIBUTION
        });
        if keep {
            survivors.push(ci);
        }
    }
    survivors.sort_unstable();

    Some(RetentionOutcome {
        survivors,
        partition_totals,
        partition_circuit_counts,
    })
}

/// The engine-internal cycle partition of a circuit: that of its first stock
/// node in traversal order, or `None` (Solo) when no node resolves to one.
///
/// Every stock of a cycle shares its partition by construction (a cycle
/// partition IS a stock-to-stock SCC), so "first" is "the" partition.
fn circuit_partition(
    rows: &[u32],
    graph: &ActivityGraph,
    stock_partition_of_node: &[Option<usize>],
) -> Option<usize> {
    rows.iter()
        .find_map(|&row| stock_partition_of_node[graph.edge_from[row as usize] as usize])
}

/// Whether a circuit passes through a module-instance node -- in which case its
/// reported score may come from the per-exit-port override series rather than
/// the raw product of its links.
fn circuit_traverses_module(rows: &[u32], graph: &ActivityGraph, is_module_node: &[bool]) -> bool {
    rows.iter()
        .any(|&row| is_module_node[graph.edge_from[row as usize] as usize])
}

/// Add one loop's |score| mass into the external totals (used for stitched
/// cross-agg loops, which are appended after the enumeration passes but must
/// participate in the denominators like every other loop).
///
/// Stitched sequences arrive as node paths, so this is the one place that
/// resolves `(from, to)` pairs back to edge rows.
pub(super) fn accumulate_series_into_totals(
    path: &[u32],
    graph: &ActivityGraph,
    stock_partition_of_node: &[Option<usize>],
    partition_totals: &mut HashMap<usize, Vec<f64>>,
) {
    let Some(part) = path
        .iter()
        .find_map(|&n| stock_partition_of_node[n as usize])
    else {
        return;
    };
    let rows = path_edge_rows(path, graph);
    let step_count = graph.step_count;
    let mut scratch = vec![0.0f64; step_count];
    let mut nan_mask = vec![false; step_count];
    score_series(&rows, graph, &mut scratch, &mut nan_mask);
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
fn deadline_expired(i: usize, deadline: Option<Instant>, clock: &mut dyn Clock) -> bool {
    i.is_multiple_of(RETENTION_DEADLINE_CHECK_CIRCUITS) && expired(deadline, clock)
}

/// Resolve a node path's consecutive-pair (wrapping) edge rows. Every pair is a
/// union-graph edge by construction of the stitcher (stitched sequences
/// concatenate petals whose hops are all real edges).
fn path_edge_rows(path: &[u32], graph: &ActivityGraph) -> Vec<u32> {
    (0..path.len())
        .map(|i| {
            graph
                .edge_row(path[i], path[(i + 1) % path.len()])
                .expect("stitched path edge must exist in the union graph")
        })
        .collect()
}

/// One loop's signed per-step score series over every saved step.
fn score_series(rows: &[u32], graph: &ActivityGraph, out: &mut [f64], nan_mask: &mut [bool]) {
    score_steps(rows, graph, 0, out, nan_mask)
}

/// One loop's signed per-step score series (product of link values) over the
/// saved steps `lo..lo + out.len()`, with `nan_mask[i]` set when the product at
/// that step is not a number.
///
/// The product runs edge-outer / step-inner over the graph's contiguous score
/// rows, so each row is a straight-line multiply the compiler vectorizes. Order
/// within a step is the circuit's traversal order, matching the FoundLoop
/// scoring pipeline link for link, so a survivor's totals and its materialized
/// series agree bit for bit.
///
/// NaN needs no special case: it propagates through multiplication, so the mask
/// is a property of the finished product. That is deliberately stronger than
/// testing the links: an `Inf * 0` product is NaN with no NaN link anywhere, and
/// treating it as a number would poison a whole partition total with NaN.
fn score_steps(
    rows: &[u32],
    graph: &ActivityGraph,
    lo: usize,
    out: &mut [f64],
    nan_mask: &mut [bool],
) {
    debug_assert_eq!(out.len(), nan_mask.len());
    out.fill(1.0);
    for &row in rows {
        let start = row as usize * graph.step_count + lo;
        let row_series = &graph.series[start..start + out.len()];
        for (product, &value) in out.iter_mut().zip(row_series) {
            *product *= value;
        }
    }
    for (mask, product) in nan_mask.iter_mut().zip(out.iter()) {
        *mask = product.is_nan();
    }
}
