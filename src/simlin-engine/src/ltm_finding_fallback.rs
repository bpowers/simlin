// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Shortest-path fallback candidate generation for discovery mode (design:
//! docs/design-plans/2026-08-17-ltm-discovery-exact.md).
//!
//! Mounted as a child of [`crate::ltm_finding`] via `#[path]` purely for the
//! per-file line cap; everything here is implementation detail of
//! `discover_loops_with_candidate_gen`.
//!
//! Union-graph enumeration is discovery's exact generator, but it cannot
//! always finish: a dense runtime graph can exceed the circuit/visit budgets,
//! and a caller with a wall-clock budget can cut it off part way. A partial
//! enumeration is useless as a candidate set -- it is biased by node-id root
//! order and its per-partition totals are not the universe's -- so something
//! else has to produce candidates, and it has to do so under a bound that is
//! known before the work starts.
//!
//! That is what this module is. For every saved step and every seed stock, a
//! Dijkstra search over the step's active edges recovers the cycles through
//! that stock, cheapest first. Two properties make it the right sampler:
//!
//! - **Bounded and interruptible by construction.** One Dijkstra is
//!   `O(E log V)` regardless of how tangled the graph is, so the sweep's cost
//!   is `steps * stocks * E log V` with no cliff, and the deadline can be
//!   honored between searches instead of only between steps.
//! - **Principled about what it drops.** A sampler bounded by refusing node
//!   re-expansions collapses, on a dense graph, into "whatever the first few
//!   thousand expansions happened to reach" -- a fact about the traversal
//!   order rather than about the model, and one that no caller can interpret.
//!   Dijkstra drops cycles too (one tree expresses one path per node), but
//!   what it keeps is exactly the minimum-weight cycle through each seed.
//!   That is the standing requirement on anything that stands in for the
//!   enumerator here: the set it discards has to be characterizable.
//!
//! Loop scores are still recomputed exactly from the recorded link-score
//! series afterwards. The weight function only decides WHICH cycles are
//! proposed, never what they are worth, which is why the formulation is
//! selectable ([`FallbackWeight`]) and settled by measurement rather than by
//! argument.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::time::Instant;

use super::{Clock, DEADLINE_CHECK_INTERVAL, IndexedSearch, expired, tarjan_scc_ids};
use crate::results::Results;

/// Edge weight formulation for the fallback's shortest-path search.
///
/// Every arm must yield non-negative, non-NaN weights: Dijkstra's optimality
/// argument (a settled node's distance is final) needs it, and a link score
/// above 1 is a negative edge in raw `-ln` space. Super-unit links are not a
/// corner case -- the design doc's measurements put 37-91 of World3's ~190-250
/// active links per step above 1 -- and Johnson potentials cannot rescue them,
/// because a negative cycle in that space is just a loop with gain > 1 and no
/// feasible potentials exist. So each arm handles super-unit links its own way.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FallbackWeight {
    /// `w = max(0, -ln|s|)`: sub-unit links cost, super-unit links are free.
    ///
    /// The clamp makes this an admissible optimistic bound on the true `-ln`
    /// cost -- it never overstates a path -- which keeps the search biased
    /// toward high-gain cycles while satisfying Dijkstra's precondition.
    ClampedLogAbs,
    /// `w = -ln(|s| / sum of |s| over the target's active in-edges)`: the LTM
    /// relative link score (reference doc 13.3, the link score normalized
    /// across all determinants of the dependent variable).
    ///
    /// A share is at most 1 by construction, so the weight is non-negative
    /// without any clamping, and the search prefers the link that explains
    /// most of what its target did rather than the link with the largest raw
    /// magnitude.
    RelativeLinkScore,
    /// `w = 1` per hop: shortest-cycle control, ignoring the scores entirely.
    /// The baseline the other two have to beat in the evaluation harness.
    HopCount,
}

impl FallbackWeight {
    /// The formulation production uses unless a caller pins another one.
    pub const DEFAULT: FallbackWeight = FallbackWeight::ClampedLogAbs;
}

/// Whether an edge carries signal at a saved step.
///
/// Identical to [`super::enum_gen::ActivityGraph`]'s rule, and the two must
/// stay identical: a cycle the enumerator considers active and the fallback
/// does not (or the reverse) would make `enumeration_complete` mean different
/// things about the same model. Infinity is a real, divergent signal and stays
/// active; only NaN (no `PREVIOUS` value, or an undefined partial) and an
/// exact zero are inactive, and a loop through a zero link scores exactly zero
/// at this step anyway.
#[inline]
fn is_active(value: f64) -> bool {
    (value != 0.0 && value.is_finite()) || value.is_infinite()
}

/// The search weight of an edge whose |link score| is `abs_score` and whose
/// target's active in-edges sum to `in_sum`.
///
/// `abs_score` is always strictly positive (an inactive edge never reaches
/// here), so `in_sum >= abs_score > 0` and no arm divides by zero.
///
/// The infinite cases are decided rather than avoided, because a divergent
/// link is signal and dropping it would make the reachable cycle set depend on
/// the weight formulation:
///
/// - `ClampedLogAbs`: `ln(inf)` is `inf`, so the clamp makes an infinite link
///   a free hop -- consistent with every other super-unit link.
/// - `RelativeLinkScore` with a finite score against an infinite sibling: the
///   share underflows to 0 and the weight is `+inf`. The edge stays walkable
///   (an infinite distance still orders and still closes a cycle) but is never
///   preferred while the divergent sibling is available.
/// - `RelativeLinkScore` with an infinite score against an infinite sibling:
///   `inf / inf` is NaN, which no ordering can use. Both links are equally
///   divergent, so the pair is treated as free hops.
fn edge_weight(weight: FallbackWeight, abs_score: f64, in_sum: f64) -> f64 {
    match weight {
        FallbackWeight::ClampedLogAbs => (-abs_score.ln()).max(0.0),
        FallbackWeight::RelativeLinkScore => {
            let w = -(abs_score / in_sum).ln();
            if w.is_nan() {
                0.0
            } else {
                // A share can round marginally above 1 when it is the target's
                // sole determinant, which would make the weight a tiny
                // negative number and break Dijkstra's precondition.
                w.max(0.0)
            }
        }
        FallbackWeight::HopCount => 1.0,
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only override of [`deadline_pop_interval`], scoped by an active
    /// [`DeadlinePopIntervalGuard`] -- lets a three-node fixture exercise the
    /// in-search deadline check instead of needing a component big enough for
    /// 8192 pops (docs/dev/rust.md#test-time-budgets).
    static DEADLINE_POP_INTERVAL_OVERRIDE: std::cell::Cell<Option<u32>> =
        const { std::cell::Cell::new(None) };
}

/// How many heap pops one search may make between wall-clock checks.
///
/// Every search is already bracketed by a check, so this interval only has to
/// bound the one pathological shape the brackets miss: a seed whose component
/// is most of the graph, where a single Dijkstra runs long enough to overshoot
/// the caller's budget on its own. Reading the clock on every pop would cost
/// more than the pop does on the small components most seeds sit in, so the
/// check is amortized over discovery's shared interval. The value must stay a
/// power of two -- the test is a single mask.
fn deadline_pop_interval() -> u32 {
    #[cfg(test)]
    {
        if let Some(interval) = DEADLINE_POP_INTERVAL_OVERRIDE.with(|c| c.get()) {
            debug_assert!(interval.is_power_of_two());
            return interval;
        }
    }
    DEADLINE_CHECK_INTERVAL
}

/// RAII guard (test-only) overriding [`deadline_pop_interval`] for the current
/// thread; restores the previous value on drop so a panicking test does not
/// leak the override to the next test on the same thread.
#[cfg(test)]
struct DeadlinePopIntervalGuard {
    prev: Option<u32>,
}

#[cfg(test)]
impl DeadlinePopIntervalGuard {
    fn new(interval: u32) -> Self {
        let prev = DEADLINE_POP_INTERVAL_OVERRIDE.with(|c| c.replace(Some(interval)));
        DeadlinePopIntervalGuard { prev }
    }
}

#[cfg(test)]
impl Drop for DeadlinePopIntervalGuard {
    fn drop(&mut self) {
        DEADLINE_POP_INTERVAL_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// One endpoint of a weighted step edge: the edge's other end plus its search
/// weight. In `adj[u]` the endpoint is the edge's target; in `rev[v]` it is
/// the edge's source.
#[derive(Clone, Copy)]
struct WeightedEdge {
    node: u32,
    weight: f64,
}

/// A priority-queue entry ordered as a MIN-heap on `(dist, node)`.
///
/// `BinaryHeap` is a max-heap, so the comparison is reversed. The node id is
/// part of the key so that equal distances -- which are the common case under
/// `ClampedLogAbs`, where every super-unit link weighs exactly 0 -- resolve
/// the same way on every run, keeping the sweep's output content-pure.
/// `total_cmp` gives a total order over f64 without an `unwrap`, and the
/// derived-equality trap is avoided by defining `PartialEq` from `cmp`.
struct HeapEntry {
    dist: f64,
    node: u32,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .dist
            .total_cmp(&self.dist)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for HeapEntry {}

/// Mutable search state, allocated once per sweep and reused across every
/// step and every seed stock.
///
/// The sweep runs `steps * stocks` Dijkstras -- about 6,000 on World3, at 401
/// saved steps and 15 seed stocks -- so anything allocated per search would
/// dominate the search itself, and anything allocated per visit would be
/// hopeless. Everything
/// here is sized to the node universe once; per-step buffers are cleared
/// rather than reallocated, and per-seed state is invalidated by bumping a
/// generation stamp instead of being rewritten.
struct FallbackScratch {
    /// node -> active outbound edges at the loaded step, weighted. Self-edges
    /// are excluded (see [`FallbackScratch::load_step`]).
    adj: Vec<Vec<WeightedEdge>>,
    /// node -> active inbound edges at the loaded step, weighted, in ascending
    /// source-node order. Only the seed's row is read (to close cycles), but
    /// building all of them costs one pass over the edges per step rather than
    /// one pass per (step, stock).
    rev: Vec<Vec<WeightedEdge>>,
    /// node -> sum of |link score| over its active in-edges at the loaded
    /// step: the denominator of [`FallbackWeight::RelativeLinkScore`].
    in_sum: Vec<f64>,
    /// Reusable projection of `adj` (targets only) for the per-step Tarjan run.
    scc_adj: Vec<Vec<u32>>,
    /// node -> strongly-connected-component id of the loaded step's active
    /// graph. A cycle lives entirely inside one component, so each Dijkstra is
    /// restricted to its seed's.
    scc_ids: Vec<u32>,
    /// SCC id -> node count, for skipping seeds that are on no cycle.
    scc_sizes: Vec<u32>,
    /// node -> shortest known distance from the current seed. Meaningful only
    /// while `reached_gen[node] == generation`.
    dist: Vec<f64>,
    /// node -> predecessor on that shortest path. The chains form a tree
    /// rooted at the seed: a parent is always a settled node, and settled
    /// nodes are never re-parented (non-negative weights make a shorter
    /// relaxation impossible), so no chain can cycle.
    parent: Vec<u32>,
    /// node -> generation at which `dist`/`parent` were last written.
    reached_gen: Vec<u32>,
    /// node -> generation at which the node was popped with its final distance.
    settled_gen: Vec<u32>,
    /// Bumped once per Dijkstra so the two stamp vectors above need no clearing.
    generation: u32,
    heap: BinaryHeap<HeapEntry>,
}

impl FallbackScratch {
    /// Allocate reusable state sized for `search`'s node universe, with each
    /// node's per-step edge buffers pre-reserved to its static out-degree.
    fn new(search: &IndexedSearch) -> Self {
        let n_nodes = search.node_count();
        FallbackScratch {
            adj: search
                .adj
                .iter()
                .map(|e| Vec::with_capacity(e.len()))
                .collect(),
            rev: vec![Vec::new(); n_nodes],
            in_sum: vec![0.0; n_nodes],
            scc_adj: vec![Vec::new(); n_nodes],
            scc_ids: vec![0; n_nodes],
            scc_sizes: Vec::new(),
            dist: vec![0.0; n_nodes],
            parent: vec![0; n_nodes],
            reached_gen: vec![0; n_nodes],
            settled_gen: vec![0; n_nodes],
            generation: 0,
            heap: BinaryHeap::new(),
        }
    }

    /// Rebuild the weighted active graph, its reverse, and its SCCs for `step`.
    ///
    /// Self-edges are dropped outright. An elementary cycle never repeats a
    /// node, so a self-edge can neither extend one nor be one: a one-variable
    /// "loop" is not a feedback loop in the SD sense, and the enumerator's
    /// contract says the same, so both generators agree on what a loop is.
    fn load_step(
        &mut self,
        search: &IndexedSearch,
        results: &Results,
        step: usize,
        weight: FallbackWeight,
    ) {
        let base = step * results.step_size;
        for row in self.adj.iter_mut() {
            row.clear();
        }
        for row in self.rev.iter_mut() {
            row.clear();
        }
        self.in_sum.fill(0.0);

        // Pass 1: select the active edges, stashing |score| in the weight slot
        // and accumulating each target's determinant mass. The normalization
        // has to see the whole in-edge set before any weight can be computed,
        // which is why this cannot be a single pass.
        for (from, edges) in search.adj.iter().enumerate() {
            for edge in edges {
                if edge.to as usize == from {
                    continue;
                }
                let value = results.data[base + edge.offset];
                if !is_active(value) {
                    continue;
                }
                let abs_score = value.abs();
                self.adj[from].push(WeightedEdge {
                    node: edge.to,
                    weight: abs_score,
                });
                self.in_sum[edge.to as usize] += abs_score;
            }
        }

        // Pass 2: |score| -> search weight, and the reverse adjacency. Walking
        // sources in ascending id order gives every `rev` row a content-pure
        // order, which the closing-edge tie-break below relies on.
        for from in 0..self.adj.len() {
            for k in 0..self.adj[from].len() {
                let edge = self.adj[from][k];
                let w = edge_weight(weight, edge.weight, self.in_sum[edge.node as usize]);
                self.adj[from][k].weight = w;
                self.rev[edge.node as usize].push(WeightedEdge {
                    node: from as u32,
                    weight: w,
                });
            }
        }

        for (node, row) in self.adj.iter().enumerate() {
            let proj = &mut self.scc_adj[node];
            proj.clear();
            proj.extend(row.iter().map(|e| e.node));
        }
        let (ids, sizes) = tarjan_scc_ids(&self.scc_adj);
        self.scc_ids = ids;
        self.scc_sizes = sizes;
    }

    /// Whether `seed` lies on any cycle in the loaded step graph.
    ///
    /// Mutual reachability is exactly what an SCC is, so a seed alone in its
    /// component is on no cycle -- self-edges, the one shape that would put a
    /// singleton on a "cycle", are not in the graph.
    fn seed_is_on_a_cycle(&self, seed: u32) -> bool {
        self.scc_sizes[self.scc_ids[seed as usize] as usize] >= 2
    }

    /// Single-source shortest paths from `seed`, restricted to `seed`'s SCC.
    ///
    /// Returns `false` if `deadline` expired part way, in which case the
    /// distances of unsettled nodes are upper bounds rather than minima. The
    /// parent tree is still a valid tree of simple paths from the seed, so the
    /// caller may keep whatever it can already close -- a partial sweep
    /// returns real loops, just not provably the strongest ones.
    ///
    /// Edges back into the seed are skipped: they close cycles rather than
    /// extend paths, and `collect_closings` handles them. That is also what
    /// keeps the seed's parent pointer at its own self-sentinel, and so what
    /// makes every recovered path contain the seed exactly once.
    fn dijkstra_from(
        &mut self,
        seed: u32,
        deadline: Option<Instant>,
        clock: &mut dyn Clock,
    ) -> bool {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // Wrapped; clear so a stale stamp cannot read as live.
            self.reached_gen.fill(0);
            self.settled_gen.fill(0);
            self.generation = 1;
        }
        let generation = self.generation;
        let seed_scc = self.scc_ids[seed as usize];

        self.heap.clear();
        self.dist[seed as usize] = 0.0;
        self.parent[seed as usize] = seed;
        self.reached_gen[seed as usize] = generation;
        self.heap.push(HeapEntry {
            dist: 0.0,
            node: seed,
        });

        let pop_interval = deadline_pop_interval();
        let mut pops: u32 = 0;

        while let Some(HeapEntry { dist, node }) = self.heap.pop() {
            pops = pops.wrapping_add(1);
            if pops & (pop_interval - 1) == 0 && expired(deadline, clock) {
                return false;
            }
            let node_idx = node as usize;
            if self.settled_gen[node_idx] == generation {
                // A stale queue entry: this node was already popped with a
                // shorter distance (we push on improvement rather than
                // decrease-key).
                continue;
            }
            self.settled_gen[node_idx] = generation;

            for k in 0..self.adj[node_idx].len() {
                let edge = self.adj[node_idx][k];
                let to = edge.node;
                if to == seed || self.scc_ids[to as usize] != seed_scc {
                    continue;
                }
                let to_idx = to as usize;
                let candidate = dist + edge.weight;
                if self.reached_gen[to_idx] != generation || candidate < self.dist[to_idx] {
                    self.reached_gen[to_idx] = generation;
                    self.dist[to_idx] = candidate;
                    self.parent[to_idx] = node;
                    self.heap.push(HeapEntry {
                        dist: candidate,
                        node: to,
                    });
                }
            }
        }
        true
    }

    /// Every elementary cycle through the seed that the last
    /// [`FallbackScratch::dijkstra_from`] can express, as
    /// `(total weight, closing source)`, cheapest first.
    ///
    /// An in-edge `u -> seed` whose source the search reached closes the tree
    /// path `seed..u`, and that closure is elementary because the tree path is
    /// simple and never re-enters the seed. Emitting all of them rather than
    /// only the cheapest costs almost nothing (a stock's in-degree is small)
    /// and recovers the sibling loops a single tree cannot rank.
    ///
    /// The cheapest is first because it is the one claim the search actually
    /// proves -- it is the minimum-weight elementary cycle through the seed,
    /// full stop, since any such cycle's prefix costs at least the tree
    /// distance to its last node. Ties break on source id so the order is
    /// content-pure.
    fn collect_closings(&self, seed: u32, out: &mut Vec<(f64, u32)>) {
        out.clear();
        let generation = self.generation;
        let seed_scc = self.scc_ids[seed as usize];
        for edge in &self.rev[seed as usize] {
            let from = edge.node;
            if from == seed || self.scc_ids[from as usize] != seed_scc {
                continue;
            }
            if self.reached_gen[from as usize] != generation {
                continue;
            }
            out.push((self.dist[from as usize] + edge.weight, from));
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    }

    /// Write the tree path `seed -> .. -> node` into `out`, seed first.
    fn reconstruct_path(&self, seed: u32, node: u32, out: &mut Vec<u32>) {
        out.clear();
        let mut cursor = node;
        loop {
            out.push(cursor);
            if cursor == seed {
                break;
            }
            if out.len() > self.parent.len() {
                // The parent chain of a reached node terminates at the seed
                // (parents are settled nodes, settled strictly earlier), so a
                // longer walk than the node universe cannot happen.
                unreachable!("fallback parent chain does not terminate at the seed");
            }
            cursor = self.parent[cursor as usize];
        }
        out.reverse();
    }
}

/// What one fallback sweep produced.
pub(super) struct FallbackOutcome {
    /// Distinct elementary cycles as [`IndexedSearch`] node-id paths, deduped
    /// by canonical rotation, in first-found order: step, then seed-stock,
    /// then cheapest-cycle-first within a seed.
    ///
    /// Opposite-direction cycles over the same node set are kept as distinct
    /// loops (GH #308): they have different polarities and different scores,
    /// and `canonical_rotation` separates them by construction.
    pub paths: Vec<Vec<u32>>,
    /// Saved steps processed to completion. Equal to
    /// `results.step_count.saturating_sub(1)` when the sweep was not
    /// truncated, and the count of whole steps behind it when it was.
    pub steps_processed: usize,
    /// Whether the deadline cut the sweep short. The paths already found are
    /// still returned: each is a real, fully-traversed cycle, so a partial
    /// sweep is a smaller candidate set rather than an invalid one.
    pub truncated: bool,
}

/// Run the fallback over every saved step, returning the candidate cycles.
///
/// Step 0 is skipped: every link score's `TIME = INITIAL_TIME` guard arm
/// emits it as the literal constant `0` (not a genuine score --
/// `ltm_augment::link_score_guard_form_with_numerator`), the same
/// `1..step_count` window the enumerator uses.
///
/// The clock is read at exactly three places, all of them bounded: once at the
/// top of each step, once before each seed stock's search, and once per
/// [`deadline_pop_interval`] pops inside a search. That schedule is what makes
/// "expiry loses at most one step's tail" true, and it is what the budget
/// tests are calibrated against. An unbudgeted sweep reads it nowhere.
pub(super) fn sweep(
    search: &IndexedSearch,
    results: &Results,
    weight: FallbackWeight,
    deadline: Option<Instant>,
    clock: &mut dyn Clock,
) -> FallbackOutcome {
    let mut scratch = FallbackScratch::new(search);
    let mut seen: HashSet<Vec<u32>> = HashSet::new();
    let mut paths: Vec<Vec<u32>> = Vec::new();
    let mut closings: Vec<(f64, u32)> = Vec::new();
    let mut path_buf: Vec<u32> = Vec::new();
    let mut steps_processed = 0usize;
    let mut truncated = false;

    'steps: for step in 1..results.step_count {
        if expired(deadline, clock) {
            truncated = true;
            break 'steps;
        }
        scratch.load_step(search, results, step, weight);

        for &seed in &search.stock_ids {
            if !scratch.seed_is_on_a_cycle(seed) {
                continue;
            }
            if expired(deadline, clock) {
                truncated = true;
                break 'steps;
            }
            let completed = scratch.dijkstra_from(seed, deadline, clock);
            scratch.collect_closings(seed, &mut closings);
            for &(_, from) in closings.iter() {
                scratch.reconstruct_path(seed, from, &mut path_buf);
                // Dedup on the canonical rotation of the node ids. Within one
                // `IndexedSearch` the id <-> name map is a bijection, so these
                // are the same equivalence classes the name-keyed dedup would
                // give, without allocating a name vector per cycle.
                if seen.insert(crate::ltm::canonical_rotation(&path_buf)) {
                    paths.push(path_buf.clone());
                }
            }
            if !completed {
                truncated = true;
                break 'steps;
            }
        }
        steps_processed += 1;
    }

    FallbackOutcome {
        paths,
        steps_processed,
        truncated,
    }
}

#[cfg(test)]
#[path = "ltm_finding_fallback_tests.rs"]
mod tests;
