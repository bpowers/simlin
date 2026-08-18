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
//! That is what this module is. For every saved step and every seed, a
//! Dijkstra search over the step's active edges recovers cycles through that
//! seed. Two properties make it the right sampler:
//!
//! - **Bounded and interruptible by construction.** One Dijkstra is
//!   `O(E log V)` regardless of how tangled the graph is, so the sweep's cost
//!   is `steps * seeds * E log V` with no cliff, and the deadline can be
//!   honored between searches instead of only between steps.
//! - **Principled about what it drops.** A sampler bounded by refusing node
//!   re-expansions collapses, on a dense graph, into "whatever the first few
//!   thousand expansions happened to reach" -- a fact about the traversal
//!   order rather than about the model, and one that no caller can interpret.
//!   Dijkstra drops cycles too, but what it keeps is stated in terms of the
//!   model: the minimum-weight cycle through a named seed and a named edge.
//!   That is the standing requirement on anything that stands in for the
//!   enumerator here: the set it discards has to be characterizable.
//!
//! The algorithm, per (saved step, seed):
//!
//! 1. Build the step's active adjacency and its reverse, weight every edge by
//!    the configured [`FallbackWeight`], and compute the step's SCCs. A cycle
//!    lives inside one component, so each search is restricted to the seed's.
//! 2. Run a forward Dijkstra from the seed and (when every-edge closures are
//!    on) a reverse Dijkstra to it. Both order by `(weight, hops)`: a large
//!    share of a real graph weighs exactly 0 under `ClampedLogAbs`, and among
//!    equally-weighted routes the shorter one is the loop a modeller means.
//! 3. Close cycles. [`FallbackClosures::SeedInEdges`] closes only the seed's
//!    own in-edges, giving the minimum-weight cycle through the seed.
//!    [`FallbackClosures::EveryEdge`] closes every edge `u -> w` whose source
//!    the forward tree reached and whose target the reverse tree reached,
//!    giving -- for each such edge -- the minimum-weight cycle through both
//!    the seed and that edge. A closure whose two tree paths share a node is
//!    not elementary and is skipped rather than spliced.
//!
//! What it drops, stated: cycles through no seed at all (which is what
//! [`FallbackSeeds`] widens), and, for a given (seed, edge) pair, every cycle
//! but the cheapest.
//!
//! Loop scores are still recomputed exactly from the recorded link-score
//! series afterwards. The configuration only decides WHICH cycles are
//! proposed, never what they are worth, which is why it is selectable and
//! settled by measurement (`examples/ltm_fallback_eval`) rather than by
//! argument.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::time::Instant;

use super::{
    Clock, DEADLINE_CHECK_INTERVAL, IndexedSearch, TarjanScratch, expired, is_active,
    tarjan_scc_ids_into,
};
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
    /// The clamp is what keeps Dijkstra's precondition: a super-unit link is a
    /// NEGATIVE edge in raw `-ln` space, and clamping it to 0 discards its
    /// gain rather than expressing it. So this weight is an UPPER bound on the
    /// true `-ln` cost, not a lower one, and the cheapest cycle it finds is
    /// the cheapest under the clamped weighting rather than the largest-gain
    /// cycle in the model.
    ///
    /// The consequence is a zero-weight plateau: on a model with many
    /// super-unit links -- World3 has 37-91 of its ~190-250 active links per
    /// step above 1 -- a large share of the graph weighs exactly 0, so many
    /// distinct cycles tie at weight 0 and the weight alone cannot rank them.
    /// The hop-count tie-break in the search's ordering is what decides those
    /// ties on something meaningful rather than on node-id order.
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
    /// The baseline the others have to beat in the evaluation harness.
    HopCount,
    /// `w = ln(max active |s| at this step) - ln|s|`: the same `-ln` cost as
    /// [`Self::ClampedLogAbs`], shifted by the step's strongest link instead of
    /// clamped at 0.
    ///
    /// The shift is what makes it non-negative: `max >= |s|` for every active
    /// edge by construction, so no information has to be thrown away to
    /// satisfy Dijkstra. That is the whole difference from the clamped arm,
    /// which collapses EVERY super-unit link to a free hop and so cannot tell
    /// a link of gain 1000 from one of gain 2 -- on a model whose dominant
    /// loops are long chains of super-unit links, that is exactly the
    /// distinction that decides which cycle is strongest.
    ///
    /// Summed over a cycle of length `L`, `Σ w = L·ln(max) - ln(Π|s|)`: it
    /// still charges `ln(max)` per hop, so it prefers short cycles among equal
    /// products, but a longer cycle with a large enough product still wins.
    /// The clamped arm cannot express that trade at all, since every
    /// super-unit hop is free and only the sub-unit links carry cost.
    ///
    /// Infinite links: `ln(inf)` would make every finite edge's weight `inf`
    /// and every infinite edge's `NaN`, so the shift uses the largest FINITE
    /// active |score| and an infinite edge weighs 0. That keeps a divergent
    /// link the cheapest hop available, which is what every other arm does
    /// with one. When every active edge is infinite there is no finite
    /// reference at all and they are all free hops -- the same reading
    /// [`Self::RelativeLinkScore`] gives an inf-against-inf pair.
    ShiftedLogAbs,
}

impl FallbackWeight {
    /// The formulation production uses unless a caller pins another one.
    pub const DEFAULT: FallbackWeight = FallbackWeight::ClampedLogAbs;
}

/// How a search orders two routes of equal weight.
///
/// Not a corner case under [`FallbackWeight::ClampedLogAbs`], which collapses
/// every super-unit link to weight 0: a large share of a real graph then ties,
/// and something has to decide. Which decision is better is a measurement, so
/// it is an axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FallbackTieBreak {
    /// Fewer hops first. Among equally-weighted routes the shorter one is the
    /// loop a modeller means, and it is a statement about the model.
    Hops,
    /// Lower node id first. Deterministic, and a statement about nothing --
    /// the control the hop term is measured against.
    NodeId,
}

/// Which nodes the fallback runs a search from.
///
/// Every SD feedback loop contains a stock, so seeding from the stocks reaches
/// every loop a modeller would draw -- but the runtime graph also carries
/// cycles whose state hides in a module level or in a `PREVIOUS` lag between
/// two auxes, and no stock-seeded search can reach one. Widening the seed set
/// closes that gap at a proportional cost in searches, so which policy is
/// worth its time is a measurement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FallbackSeeds {
    /// The model's stocks. The cheapest policy and the one whose boundary is
    /// easiest to state: a cycle through no stock is invisible to it.
    Stocks,
    /// The model's stocks, plus the lowest-id node of every non-trivial SCC
    /// holding no stock. One extra search per stockless component reaches the
    /// module-level and `PREVIOUS`-lag cycles `Stocks` cannot.
    StocksAndStocklessSccs,
    /// Every node in a non-trivial SCC. No cycle is unreachable for want of a
    /// seed, at a cost proportional to the cyclic core rather than to the
    /// stock count (World3's core is 135 nodes against 15 stocks).
    AllSccNodes,
}

/// Which cycles a completed pair of searches closes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FallbackClosures {
    /// Only the seed's own in-edges: for each `u -> seed` the search reached,
    /// the tree path `seed..u` plus that edge. The cheapest of these is the
    /// minimum-weight elementary cycle through the seed.
    ///
    /// One forward search per (seed, step) and no reverse search, so it is the
    /// cheap policy -- and the narrow one, since a shortest-path tree holds one
    /// route per node and two parallel routes to the same node collapse to the
    /// cheaper.
    SeedInEdges,
    /// Every edge `u -> w` inside the seed's component whose source the
    /// forward tree reached and whose target the reverse tree reached: the
    /// cycle `path(seed..u) + (u -> w) + path(w..seed)`, which is the
    /// minimum-weight cycle through BOTH the seed and that edge.
    ///
    /// The strength-weighted analogue of edge coverage, and a superset of
    /// [`Self::SeedInEdges`] (those are the `w == seed` cases). It costs a
    /// second Dijkstra plus one path check per edge, and it can propose a
    /// non-elementary closure when the two tree paths share a node -- such a
    /// candidate is skipped rather than spliced at the repeat, because a
    /// spliced cycle is no longer the minimum-weight cycle through the edge
    /// and so would not be the thing this policy claims to emit.
    EveryEdge,
}

/// How the fallback searches: the three axes that decide which cycles it
/// proposes.
///
/// Grouped into one value because a caller pinning the fallback -- the
/// evaluation harness, a semantic test -- picks a point in the whole space,
/// not a weight alone.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FallbackConfig {
    /// How an edge's |link score| becomes a search weight.
    pub weight: FallbackWeight,
    /// Which nodes a search starts from.
    pub seeds: FallbackSeeds,
    /// Which cycles a completed search closes.
    pub closures: FallbackClosures,
    /// How two routes of equal weight are ordered.
    pub tie_break: FallbackTieBreak,
}

impl FallbackConfig {
    /// The configuration production uses unless a caller pins another one.
    ///
    /// Measured, not argued: `examples/ltm_fallback_eval` sweeps the strategy
    /// space against the exact enumeration on World3 and C-LEARN, and this is
    /// the best point in it subject to the fallback staying under half the
    /// exact run's time. Closing on every edge is what earns its place --
    /// World3's recall of the exact top-200 goes from 8 to 31 (and its
    /// step-dominant coverage from 34 to 50 of 399 competing (group, step)
    /// pairs) and C-LEARN's from 97 of 153 to 150 (750 of 750 pairs), for
    /// 0.14 s and 0.15 s against exact runs of 0.40 s and (C-LEARN's own
    /// budget) 0.2 s. Seeding every node of the
    /// cyclic core instead recovers a little more and costs 1.03 s on World3,
    /// so it does not; the design plan's "Measured" section holds the table.
    pub const DEFAULT: FallbackConfig = FallbackConfig {
        weight: FallbackWeight::DEFAULT,
        seeds: FallbackSeeds::StocksAndStocklessSccs,
        closures: FallbackClosures::EveryEdge,
        tie_break: FallbackTieBreak::Hops,
    };

    /// [`Self::DEFAULT`] with the weight replaced -- the spelling the weight
    /// comparison in `examples/ltm_fallback_eval` uses.
    pub const fn with_weight(weight: FallbackWeight) -> FallbackConfig {
        FallbackConfig {
            weight,
            ..FallbackConfig::DEFAULT
        }
    }
}

/// The step-scoped normalizers an edge weight may read.
///
/// Both are properties of the loaded step rather than of the edge, and each is
/// read by exactly one arm -- which is why they travel together rather than as
/// positional arguments: a new arm needing a new normalizer adds a field here
/// and `load_step` fills it once, instead of threading another parameter
/// through every call site.
#[derive(Clone, Copy)]
struct EdgeNorms {
    /// Sum of |score| over the TARGET's active in-edges: the denominator of
    /// [`FallbackWeight::RelativeLinkScore`].
    in_sum: f64,
    /// The largest FINITE |score| over every active edge of the step: the
    /// shift of [`FallbackWeight::ShiftedLogAbs`]. Zero when the step has no
    /// finite active edge at all, which only that arm's infinite case reaches.
    step_max_finite: f64,
}

/// The search weight of an edge whose |link score| is `abs_score`, under the
/// step's normalizers.
///
/// `abs_score` is always strictly positive (an inactive edge never reaches
/// here), so `norms.in_sum >= abs_score > 0` and no arm divides by zero.
///
/// The infinite cases are decided rather than avoided, because a divergent
/// link is signal and dropping it would make the reachable cycle set depend on
/// the weight formulation:
///
/// - `ClampedLogAbs`: `ln(inf)` is `inf`, so the clamp makes an infinite link
///   a free hop -- consistent with every other super-unit link.
/// - `ShiftedLogAbs`: an infinite link is a free hop and the shift is taken
///   over the finite links only, so a finite edge's weight stays finite and
///   ordered while the divergent one is preferred.
/// - `RelativeLinkScore` with a finite score against an infinite sibling: the
///   share underflows to 0 and the weight is `+inf`. The edge stays walkable
///   (an infinite distance still orders and still closes a cycle) but is never
///   preferred while the divergent sibling is available.
/// - `RelativeLinkScore` with an infinite score against an infinite sibling:
///   `inf / inf` is NaN, which no ordering can use. Both links are equally
///   divergent, so the pair is treated as free hops.
fn edge_weight(weight: FallbackWeight, abs_score: f64, norms: EdgeNorms) -> f64 {
    match weight {
        FallbackWeight::ClampedLogAbs => (-abs_score.ln()).max(0.0),
        FallbackWeight::ShiftedLogAbs => {
            if abs_score.is_infinite() {
                // The cheapest hop available: a divergent link is the
                // strongest signal there is, and the finite shift below cannot
                // describe it.
                return 0.0;
            }
            // `step_max_finite >= abs_score` for every finite active edge, so
            // the shift makes this non-negative WITHOUT discarding anything --
            // unlike the clamped arm, which is what this formulation exists to
            // avoid. The clamp is only a guard on the floating-point identity:
            // `f64::ln` is not contractually monotone, so two adjacent floats
            // could in principle round the wrong way, and a negative edge
            // breaks Dijkstra's settle-once argument silently rather than
            // loudly.
            (norms.step_max_finite.ln() - abs_score.ln()).max(0.0)
        }
        FallbackWeight::RelativeLinkScore => {
            let w = -(abs_score / norms.in_sum).ln();
            if w.is_nan() {
                0.0
            } else {
                // The share cannot exceed 1 -- a sole determinant sums to
                // exactly its own |score|, so the ratio is exactly 1.0 and the
                // weight is exactly `-ln(1) == 0.0` -- so this clamp is not
                // guarding a negative weight. It normalizes the two spellings
                // of zero: `-0.0` and `0.0` are equal but `total_cmp` orders
                // them apart, and the heap's tie-break must not depend on
                // which arm produced a zero.
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

/// A priority-queue entry ordered as a MIN-heap on `(dist, hops, node)`.
///
/// `BinaryHeap` is a max-heap, so the comparison is reversed. Hop count is the
/// second key because equal distances are not a corner case: under
/// `ClampedLogAbs` every super-unit link weighs exactly 0, so roughly a third
/// of a real graph's active edges are free and a great many routes tie at the
/// same weight. Without the hop term the winner among them is decided by node
/// id -- deterministic but arbitrary -- while the shorter route is the loop a
/// modeller means. The node id remains the last key so ties are still resolved
/// content-purely.
///
/// The lexicographic key keeps Dijkstra exact: weights are non-negative and
/// each edge adds exactly one hop, so extending a path never decreases
/// `(dist, hops)` and a settled node's key is final.
///
/// `total_cmp` gives a total order over f64 without an `unwrap`, and the
/// derived-equality trap is avoided by defining `PartialEq` from `cmp`.
struct HeapEntry {
    dist: f64,
    /// The ORDERING hop component, flattened to a constant when the configured
    /// tie-break does not use hops ([`order_hops`]). Separate from `hops` so
    /// that turning the term off changes the ordering and nothing else -- the
    /// real hop count still has to travel, because the relaxations extend it.
    tie: u32,
    /// The route's real hop count, carried rather than ordered on.
    hops: u32,
    node: u32,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .dist
            .total_cmp(&self.dist)
            .then_with(|| other.tie.cmp(&self.tie))
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
/// step and every seed.
///
/// The sweep runs `steps * seeds` Dijkstras -- about 6,000 on World3, at 401
/// saved steps and 15 stock seeds, and up to nine times that under
/// [`FallbackSeeds::AllSccNodes`] -- so anything allocated per search would
/// dominate the search itself, and anything allocated per visit would be
/// hopeless. Everything here is sized to the node universe once; per-step
/// buffers are cleared rather than reallocated, and per-seed state is
/// invalidated by bumping a generation stamp instead of being rewritten.
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
    /// The largest FINITE |link score| over the loaded step's active edges:
    /// the shift of [`FallbackWeight::ShiftedLogAbs`]. Zero when the step has
    /// no finite active edge.
    step_max_finite: f64,
    /// How the searches order two routes of equal weight. Fixed for the whole
    /// sweep, so it is read rather than passed at every relaxation.
    tie_break: FallbackTieBreak,
    /// Reusable projection of `adj` (targets only) for the per-step Tarjan run.
    scc_adj: Vec<Vec<u32>>,
    /// node -> strongly-connected-component id of the loaded step's active
    /// graph. A cycle lives entirely inside one component, so each Dijkstra is
    /// restricted to its seed's.
    scc_ids: Vec<u32>,
    /// SCC id -> node count, for skipping seeds that are on no cycle.
    scc_sizes: Vec<u32>,
    /// SCC id -> whether any seed stock lies in it, for
    /// [`FallbackSeeds::StocksAndStocklessSccs`].
    scc_has_stock: Vec<bool>,
    /// SCC id -> whether a representative has already been taken, for the same
    /// policy.
    scc_seeded: Vec<bool>,
    /// Working buffers for the per-step Tarjan run, kept across steps: the
    /// pass runs once per saved step, so its six node-sized vectors would
    /// otherwise be allocated and freed 401 times on World3.
    scc_scratch: TarjanScratch,
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
    /// node -> hop count of the shortest known path from the current seed, the
    /// second key of the search order.
    hops: Vec<u32>,
    /// Bumped once per forward Dijkstra so the stamp vectors above need no
    /// clearing.
    generation: u32,
    /// node -> shortest known distance TO the current seed, over the reverse
    /// adjacency. The mirror of `dist`, and meaningful only while
    /// `rev_reached_gen[node] == rev_generation`.
    rev_dist: Vec<f64>,
    /// node -> hop count of that reverse path.
    rev_hops: Vec<u32>,
    /// node -> the NEXT node on its cheapest path to the seed (not a
    /// predecessor: the reverse tree is walked away from the node, toward the
    /// seed).
    rev_parent: Vec<u32>,
    /// node -> generation at which `rev_dist`/`rev_parent` were last written.
    rev_reached_gen: Vec<u32>,
    /// node -> generation at which the node was popped from the reverse search
    /// with its final distance.
    rev_settled_gen: Vec<u32>,
    /// Bumped once per reverse Dijkstra.
    rev_generation: u32,
    /// node -> generation at which the node was last marked as being on the
    /// forward tree path currently under consideration, so the every-edge
    /// closure's simplicity test is O(1) per node rather than a scan.
    path_mark: Vec<u32>,
    /// Bumped once per forward tree path marked.
    path_generation: u32,
    heap: BinaryHeap<HeapEntry>,
}

impl FallbackScratch {
    /// Allocate reusable state sized for `search`'s node universe, with each
    /// node's per-step edge buffers pre-reserved to its static out-degree.
    fn new(search: &IndexedSearch, tie_break: FallbackTieBreak) -> Self {
        let n_nodes = search.node_count();
        FallbackScratch {
            adj: search
                .adj
                .iter()
                .map(|e| Vec::with_capacity(e.len()))
                .collect(),
            rev: vec![Vec::new(); n_nodes],
            in_sum: vec![0.0; n_nodes],
            step_max_finite: 0.0,
            tie_break,
            scc_adj: vec![Vec::new(); n_nodes],
            scc_ids: Vec::new(),
            scc_sizes: Vec::new(),
            scc_has_stock: Vec::new(),
            scc_seeded: Vec::new(),
            scc_scratch: TarjanScratch::default(),
            dist: vec![0.0; n_nodes],
            parent: vec![0; n_nodes],
            reached_gen: vec![0; n_nodes],
            settled_gen: vec![0; n_nodes],
            hops: vec![0; n_nodes],
            generation: 0,
            rev_dist: vec![0.0; n_nodes],
            rev_hops: vec![0; n_nodes],
            rev_parent: vec![0; n_nodes],
            rev_reached_gen: vec![0; n_nodes],
            rev_settled_gen: vec![0; n_nodes],
            rev_generation: 0,
            path_mark: vec![0; n_nodes],
            path_generation: 0,
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
        self.step_max_finite = 0.0;

        // Pass 1: select the active edges, stashing |score| in the weight slot
        // and accumulating each target's determinant mass. The normalization
        // has to see the whole in-edge set before any weight can be computed,
        // which is why this cannot be a single pass.
        //
        // The mass is accumulated for every weight arm, not only the one that
        // reads it. Gating it would make `in_sum` a denominator that is live
        // for some arms and stale zero for others, so any arm added later that
        // consulted it would divide by a zero nothing wrote -- a silent wrong
        // weight rather than a compile error. One indexed add per active edge
        // is not worth that coupling.
        for (from, edges) in search.adj.iter().enumerate() {
            for edge in edges {
                if edge.to as usize == from {
                    continue;
                }
                let value = edge.value_at(results, base);
                if !is_active(value) {
                    continue;
                }
                let abs_score = value.abs();
                self.adj[from].push(WeightedEdge {
                    node: edge.to,
                    weight: abs_score,
                });
                self.in_sum[edge.to as usize] += abs_score;
                // The shift of `ShiftedLogAbs`, over FINITE scores only: an
                // infinite one would make every finite weight infinite and
                // every infinite weight NaN.
                if abs_score.is_finite() && abs_score > self.step_max_finite {
                    self.step_max_finite = abs_score;
                }
            }
        }

        // Pass 2: |score| -> search weight, and the reverse adjacency. Walking
        // sources in ascending id order gives every `rev` row a content-pure
        // order, which the closing-edge tie-break below relies on.
        for from in 0..self.adj.len() {
            for k in 0..self.adj[from].len() {
                let edge = self.adj[from][k];
                let w = edge_weight(
                    weight,
                    edge.weight,
                    EdgeNorms {
                        in_sum: self.in_sum[edge.node as usize],
                        step_max_finite: self.step_max_finite,
                    },
                );
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
        tarjan_scc_ids_into(
            &self.scc_adj,
            &mut self.scc_scratch,
            &mut self.scc_ids,
            &mut self.scc_sizes,
        );
    }

    /// The seeds `policy` selects in the loaded step graph, in a content-pure
    /// order: the stocks in their input order first, then any additional nodes
    /// in ascending id order.
    ///
    /// Every returned seed lies on a cycle at this step, so the caller never
    /// pays for a search that can close nothing.
    fn collect_seeds(&mut self, search: &IndexedSearch, policy: FallbackSeeds, out: &mut Vec<u32>) {
        out.clear();
        if matches!(
            policy,
            FallbackSeeds::Stocks | FallbackSeeds::StocksAndStocklessSccs
        ) {
            out.extend(
                search
                    .stock_ids
                    .iter()
                    .copied()
                    .filter(|&s| self.seed_is_on_a_cycle(s)),
            );
        }
        match policy {
            FallbackSeeds::Stocks => {}
            FallbackSeeds::StocksAndStocklessSccs => {
                self.scc_has_stock.clear();
                self.scc_has_stock.resize(self.scc_sizes.len(), false);
                for &stock in &search.stock_ids {
                    self.scc_has_stock[self.scc_ids[stock as usize] as usize] = true;
                }
                self.scc_seeded.clear();
                self.scc_seeded.resize(self.scc_sizes.len(), false);
                for node in 0..self.scc_ids.len() as u32 {
                    let comp = self.scc_ids[node as usize] as usize;
                    if self.scc_sizes[comp] < 2 || self.scc_has_stock[comp] || self.scc_seeded[comp]
                    {
                        continue;
                    }
                    self.scc_seeded[comp] = true;
                    out.push(node);
                }
            }
            FallbackSeeds::AllSccNodes => {
                out.extend((0..self.scc_ids.len() as u32).filter(|&n| self.seed_is_on_a_cycle(n)));
            }
        }
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
        self.hops[seed as usize] = 0;
        self.parent[seed as usize] = seed;
        self.reached_gen[seed as usize] = generation;
        let tie_break = self.tie_break;
        self.heap.push(HeapEntry {
            dist: 0.0,
            tie: 0,
            hops: 0,
            node: seed,
        });

        let pop_interval = deadline_pop_interval();
        let mut pops: u32 = 0;

        while let Some(HeapEntry {
            dist, hops, node, ..
        }) = self.heap.pop()
        {
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
                let candidate_hops = hops + 1;
                let improves = self.reached_gen[to_idx] != generation
                    || key_is_better(
                        (candidate, order_hops(tie_break, candidate_hops)),
                        (self.dist[to_idx], order_hops(tie_break, self.hops[to_idx])),
                    );
                if improves {
                    self.reached_gen[to_idx] = generation;
                    self.dist[to_idx] = candidate;
                    self.hops[to_idx] = candidate_hops;
                    self.parent[to_idx] = node;
                    self.heap.push(HeapEntry {
                        dist: candidate,
                        tie: order_hops(tie_break, candidate_hops),
                        hops: candidate_hops,
                        node: to,
                    });
                }
            }
        }
        true
    }

    /// Single-target shortest paths INTO `seed`, restricted to `seed`'s SCC:
    /// the mirror of [`Self::dijkstra_from`], relaxing over the reverse
    /// adjacency so `rev_dist[v]` is the cheapest route from `v` to the seed
    /// and `rev_parent[v]` is the next node along it.
    ///
    /// Returns `false` on deadline expiry, with the same guarantee: the
    /// reverse tree is a valid tree of simple paths at every instant, so the
    /// caller may close whatever it already reaches.
    ///
    /// Edges leaving the seed are skipped, mirroring `dijkstra_from` skipping
    /// edges entering it. That is what keeps the seed off every reverse path
    /// except as its endpoint, which is what makes a closure contain the seed
    /// exactly once.
    fn dijkstra_to(&mut self, seed: u32, deadline: Option<Instant>, clock: &mut dyn Clock) -> bool {
        self.rev_generation = self.rev_generation.wrapping_add(1);
        if self.rev_generation == 0 {
            // Wrapped; clear so a stale stamp cannot read as live.
            self.rev_reached_gen.fill(0);
            self.rev_settled_gen.fill(0);
            self.rev_generation = 1;
        }
        let generation = self.rev_generation;
        let seed_scc = self.scc_ids[seed as usize];

        self.heap.clear();
        self.rev_dist[seed as usize] = 0.0;
        self.rev_hops[seed as usize] = 0;
        self.rev_parent[seed as usize] = seed;
        self.rev_reached_gen[seed as usize] = generation;
        let tie_break = self.tie_break;
        self.heap.push(HeapEntry {
            dist: 0.0,
            tie: 0,
            hops: 0,
            node: seed,
        });

        let pop_interval = deadline_pop_interval();
        let mut pops: u32 = 0;

        while let Some(HeapEntry {
            dist, hops, node, ..
        }) = self.heap.pop()
        {
            pops = pops.wrapping_add(1);
            if pops & (pop_interval - 1) == 0 && expired(deadline, clock) {
                return false;
            }
            let node_idx = node as usize;
            if self.rev_settled_gen[node_idx] == generation {
                continue;
            }
            self.rev_settled_gen[node_idx] = generation;

            for k in 0..self.rev[node_idx].len() {
                let edge = self.rev[node_idx][k];
                // `rev[node]` holds the SOURCES of edges into `node`, so this
                // is the edge `from -> node` and the reverse-tree step is
                // "from `from`, go to `node`".
                let from = edge.node;
                if from == seed || self.scc_ids[from as usize] != seed_scc {
                    continue;
                }
                let from_idx = from as usize;
                let candidate = dist + edge.weight;
                let candidate_hops = hops + 1;
                let improves = self.rev_reached_gen[from_idx] != generation
                    || key_is_better(
                        (candidate, order_hops(tie_break, candidate_hops)),
                        (
                            self.rev_dist[from_idx],
                            order_hops(tie_break, self.rev_hops[from_idx]),
                        ),
                    );
                if improves {
                    self.rev_reached_gen[from_idx] = generation;
                    self.rev_dist[from_idx] = candidate;
                    self.rev_hops[from_idx] = candidate_hops;
                    self.rev_parent[from_idx] = node;
                    self.heap.push(HeapEntry {
                        dist: candidate,
                        tie: order_hops(tie_break, candidate_hops),
                        hops: candidate_hops,
                        node: from,
                    });
                }
            }
        }
        true
    }

    /// Every elementary cycle through the seed that the last
    /// [`FallbackScratch::dijkstra_from`] can express, as
    /// `(total weight, hop count, closing source)`, cheapest first.
    ///
    /// The hop count is the ordering tie-break component ([`order_hops`]),
    /// carried here so the sort below can apply it without re-deriving it from
    /// `self.hops`; only the weight and the closing source are semantically
    /// new information, since a caller reconstructs the actual path from the
    /// source via [`FallbackScratch::write_forward_path`].
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
    fn collect_closings(&self, seed: u32, out: &mut Vec<(f64, u32, u32)>) {
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
            out.push((
                self.dist[from as usize] + edge.weight,
                self.hops[from as usize] + 1,
                from,
            ));
        }
        let tie_break = self.tie_break;
        out.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| order_hops(tie_break, a.1).cmp(&order_hops(tie_break, b.1)))
                .then_with(|| a.2.cmp(&b.2))
        });
    }

    /// Emit, for every edge inside the seed's component that both trees reach,
    /// the minimum-weight cycle through the seed and that edge.
    ///
    /// The candidate for `u -> w` is `path(seed..u) + (u -> w) + path(w..seed)`
    /// -- minimum-weight because each half is a shortest path and the edge is
    /// fixed -- but the two halves can share a node, in which case the walk is
    /// not an elementary cycle and is skipped (see
    /// [`FallbackClosures::EveryEdge`] for why it is not spliced instead).
    /// The `w == seed` cases are exactly [`Self::collect_closings`]'s
    /// in-edge closures.
    ///
    /// `cycle` is the caller's scratch buffer: the forward half is written
    /// once per source node and only the reverse tail is rewritten per edge.
    fn collect_every_edge_closures(
        &mut self,
        seed: u32,
        cycle: &mut Vec<u32>,
        dedup: &mut CycleDedup,
        paths: &mut Vec<Vec<u32>>,
    ) {
        let generation = self.generation;
        let rev_generation = self.rev_generation;
        let seed_scc = self.scc_ids[seed as usize];
        for u in 0..self.adj.len() as u32 {
            if self.scc_ids[u as usize] != seed_scc
                || self.reached_gen[u as usize] != generation
                || self.adj[u as usize].is_empty()
            {
                continue;
            }
            self.write_forward_path(seed, u, cycle);
            self.path_generation = self.path_generation.wrapping_add(1);
            if self.path_generation == 0 {
                self.path_mark.fill(0);
                self.path_generation = 1;
            }
            let path_generation = self.path_generation;
            for &node in cycle.iter() {
                self.path_mark[node as usize] = path_generation;
            }
            let forward_len = cycle.len();

            for k in 0..self.adj[u as usize].len() {
                let w = self.adj[u as usize][k].node;
                if self.scc_ids[w as usize] != seed_scc
                    || self.rev_reached_gen[w as usize] != rev_generation
                {
                    continue;
                }
                cycle.truncate(forward_len);
                let mut cursor = w;
                let mut elementary = true;
                while cursor != seed {
                    if self.path_mark[cursor as usize] == path_generation {
                        elementary = false;
                        break;
                    }
                    cycle.push(cursor);
                    if cycle.len() - forward_len > self.rev_parent.len() {
                        // The reverse-tree parent chain of a reached node
                        // terminates at the seed for the same reason the
                        // forward tree's does (`write_forward_path`): a
                        // parent is always a settled node, settled strictly
                        // earlier, so more steps than the node universe
                        // cannot happen without cycling.
                        unreachable!(
                            "fallback reverse-tree parent chain does not terminate at the seed"
                        );
                    }
                    cursor = self.rev_parent[cursor as usize];
                }
                if !elementary {
                    continue;
                }
                dedup.insert_if_new(cycle, paths);
            }
        }
    }

    /// Write the forward tree path `seed -> .. -> node` into `out`, seed first.
    fn write_forward_path(&self, seed: u32, node: u32, out: &mut Vec<u32>) {
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

/// Whether the search key `a` beats `b`, lexicographically on
/// `(weight, hops)`.
///
/// Split out because the forward and reverse relaxations must order
/// identically -- a reverse tree ranked on weight alone beside a forward tree
/// ranked on `(weight, hops)` would make an every-edge closure's two halves
/// answer different questions.
#[inline]
fn key_is_better(a: (f64, u32), b: (f64, u32)) -> bool {
    match a.0.total_cmp(&b.0) {
        Ordering::Less => true,
        Ordering::Equal => a.1 < b.1,
        Ordering::Greater => false,
    }
}

/// The hop component of a search key under `tie_break`.
///
/// [`FallbackTieBreak::NodeId`] flattens it to a constant, which is how the
/// hop term is turned OFF in one place rather than at every ordering site: the
/// heap comparison and `key_is_better` both read this and cannot disagree
/// about whether hops are in the key. The real hop count stays in
/// `FallbackScratch::hops` either way, because `collect_closings` reports it.
#[inline]
fn order_hops(tie_break: FallbackTieBreak, hops: u32) -> u32 {
    match tie_break {
        FallbackTieBreak::Hops => hops,
        FallbackTieBreak::NodeId => 0,
    }
}

/// Maximum distinct elementary cycles one fallback sweep may accumulate across
/// every (step, seed) before it stops early and reports `truncated`.
///
/// Nothing else bounds the sweep's candidate volume: `sweep` has no per-step
/// or per-seed cap, `Model.analyze()` defaults to `timeout=None`, and the
/// every-edge closure family (`FallbackClosures::EveryEdge`, the default)
/// proposes roughly one candidate per active edge per (seed, step) before
/// dedup thins it -- on a sufficiently dense or long-running model that grows
/// without limit.
///
/// This bounds MATERIALIZATION memory, not the candidate list's own footprint
/// (which really is a few tens of MB even at the old 200,000 bound -- a few
/// dozen `u32` node ids per cycle). Every one of `sweep`'s candidates is
/// materialized into a `FoundLoop` carrying a `step_count`-long `Vec<(f64,
/// f64)>` score series BEFORE retention has a chance to drop any of them
/// (`ltm_finding.rs`'s materialization loop runs over `all_paths` in full),
/// so the bound that actually matters is candidates x steps x 16 bytes. The
/// effective cap is therefore the SMALLER of this count and
/// [`MAX_FALLBACK_MATERIALIZATION_BYTES`] divided by the run's per-candidate
/// series size (`max_fallback_paths(step_count)`): at 401 saved steps
/// (World3's shape) 20,000 candidates is ~128 MB and the count binds; at
/// 100,000 saved steps the byte budget binds instead and the cap falls to
/// ~160 candidates -- a bounded, tolerable spike either way, where a fixed
/// count alone would let a long run climb to tens of GB (the same
/// multi-GB-cliff shape AC3.3's enumeration-side `MAX_DISCOVERY_ENUM_EDGE_ROWS`
/// exists to prevent).
///
/// It is still sized with real headroom over what the corpus actually
/// produces -- `examples/ltm_discovery_bench` measured 2,150 deduped
/// candidates on World3-03 (the densest runtime graph in the repo corpus) and
/// 159 on C-LEARN v77, both under [`FallbackConfig::DEFAULT`]'s `EveryEdge`
/// closures, ~9x below this bound (was ~90x below the old 200,000). A trip is
/// therefore still a signal about a genuinely pathological graph, not
/// something the shipped corpus is expected to reach.
///
/// A budget trip is reported the same way a deadline expiry is: `truncated`,
/// with whatever was found so far kept (each candidate is a real, fully
/// traversed cycle). That is the honest signal here too -- the sweep stopped
/// before it could claim to have sampled every seed and step, so the result is
/// a smaller candidate set rather than an invalid one, exactly as a
/// wall-clock-truncated sweep already reads.
const MAX_FALLBACK_PATHS: usize = 20_000;

/// Materialization budget for one fallback sweep's candidates, in bytes of
/// `FoundLoop` score series (`step_count * 16 B` each). See
/// [`MAX_FALLBACK_PATHS`] for why the count alone does not bound memory.
const MAX_FALLBACK_MATERIALIZATION_BYTES: usize = 256 * 1024 * 1024;

/// Bytes one materialized candidate's score series costs: `(f64, f64)` per
/// saved step.
const BYTES_PER_MATERIALIZED_STEP: usize = 16;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`MAX_FALLBACK_PATHS`], scoped by an active
    /// [`MaxFallbackPathsGuard`] -- lets a tiny fixture trip the candidate
    /// budget without materializing anywhere near 200,000 cycles
    /// (docs/dev/rust.md#test-time-budgets).
    static MAX_FALLBACK_PATHS_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The candidate-cycle budget for a sweep over `step_count` saved steps:
/// the smaller of [`MAX_FALLBACK_PATHS`] and the count whose materialized
/// series fit [`MAX_FALLBACK_MATERIALIZATION_BYTES`] (never below 1, so a
/// pathologically long run still proposes something). A test-installed
/// [`MaxFallbackPathsGuard`] override replaces both.
fn max_fallback_paths(step_count: usize) -> usize {
    #[cfg(test)]
    {
        if let Some(n) = MAX_FALLBACK_PATHS_OVERRIDE.with(|c| c.get()) {
            return n;
        }
    }
    let per_candidate = step_count.max(1) * BYTES_PER_MATERIALIZED_STEP;
    MAX_FALLBACK_PATHS.min((MAX_FALLBACK_MATERIALIZATION_BYTES / per_candidate).max(1))
}

/// RAII guard (test-only) overriding [`max_fallback_paths`] for the current
/// thread; restores the previous value on drop so a panicking test does not
/// leak the override to the next test on the same thread.
#[cfg(test)]
struct MaxFallbackPathsGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl MaxFallbackPathsGuard {
    fn new(cap: usize) -> Self {
        let prev = MAX_FALLBACK_PATHS_OVERRIDE.with(|c| c.replace(Some(cap)));
        MaxFallbackPathsGuard { prev }
    }
}

#[cfg(test)]
impl Drop for MaxFallbackPathsGuard {
    fn drop(&mut self) {
        MAX_FALLBACK_PATHS_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// Rotation-independent duplicate detection over the emitted cycles.
///
/// An elementary cycle is determined by its SET of directed edges -- every
/// node on it has exactly one predecessor and one successor there -- so a
/// fingerprint that folds those edges order-independently identifies the cycle
/// however it was rotated. That is what lets a candidate be TESTED for
/// duplication without allocating anything, which matters because after the
/// first few saved steps nearly every candidate is a duplicate: the every-edge
/// closures propose one cycle per active edge per (seed, step), and the sweep
/// keeps only the distinct ones.
///
/// The fingerprint is a filter and not the identity: a bucket hit is resolved
/// by comparing the candidate against the stored cycles under rotation, so the
/// dedup stays exact rather than probabilistic.
///
/// `cap` bounds `paths`' final length (see [`MAX_FALLBACK_PATHS`]): once it is
/// reached, `insert_if_new` refuses every further candidate -- cheaply, before
/// any fingerprinting or allocation -- and the caller (`sweep`) is the one
/// that turns "no more room" into `truncated`. `CycleDedup::default()` sets no
/// cap (`usize::MAX`), which is what the unit tests below use when they are
/// testing dedup semantics rather than the budget.
struct CycleDedup {
    /// fingerprint -> the indices into `paths` of the cycles carrying it.
    buckets: HashMap<u64, Vec<u32>>,
    cap: usize,
}

impl Default for CycleDedup {
    fn default() -> Self {
        CycleDedup::new(usize::MAX)
    }
}

impl CycleDedup {
    fn new(cap: usize) -> Self {
        CycleDedup {
            buckets: HashMap::new(),
            cap,
        }
    }

    /// Append `cycle` to `paths` unless a rotation of it is already there or
    /// `paths` is already at `cap`. Returns whether it was newly inserted.
    fn insert_if_new(&mut self, cycle: &[u32], paths: &mut Vec<Vec<u32>>) -> bool {
        if paths.len() >= self.cap {
            return false;
        }
        let bucket = self.buckets.entry(cycle_fingerprint(cycle)).or_default();
        if bucket
            .iter()
            .any(|&i| is_same_cycle(cycle, &paths[i as usize]))
        {
            return false;
        }
        bucket.push(paths.len() as u32);
        paths.push(cycle.to_vec());
        true
    }
}

/// SplitMix64's finalizer: a full-avalanche 64-bit mix, so adjacent node-id
/// pairs do not land in adjacent fingerprints.
#[inline]
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// An order-independent fold over a cycle's directed edges.
///
/// Wrapping addition is what makes it rotation-independent: the multiset of
/// edges is the same whichever node the walk starts from.
fn cycle_fingerprint(cycle: &[u32]) -> u64 {
    let n = cycle.len();
    let mut acc: u64 = 0;
    for i in 0..n {
        let from = u64::from(cycle[i]);
        let to = u64::from(cycle[(i + 1) % n]);
        acc = acc.wrapping_add(mix64(((from << 32) | to) ^ 0x9e37_79b9_7f4a_7c15));
    }
    acc
}

/// Whether `a` and `b` are the same directed cycle up to rotation.
///
/// Both are elementary, so `a[0]` occurs at most once in `b` and the candidate
/// offset is unique -- which is what makes this a linear check rather than an
/// all-rotations comparison. Sorting the node sets would be wrong here: two
/// distinct directed cycles can share a node set (GH #308) and must stay
/// distinct loops.
fn is_same_cycle(a: &[u32], b: &[u32]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let Some(offset) = b.iter().position(|&n| n == a[0]) else {
        return false;
    };
    (0..a.len()).all(|k| a[k] == b[(offset + k) % b.len()])
}

/// What one fallback sweep produced.
pub(super) struct FallbackOutcome {
    /// Distinct elementary cycles as [`IndexedSearch`] node-id paths, deduped
    /// by rotation, in first-found order: step, then seed, then -- under
    /// [`FallbackClosures::SeedInEdges`] -- cheapest closure first, and under
    /// [`FallbackClosures::EveryEdge`] by the closing edge's source node id.
    /// Nothing downstream reads that order; it is content-pure so that two
    /// runs over the same results produce the same list.
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
/// top of each step, once before each seed's searches, and once per
/// [`deadline_pop_interval`] pops inside a search (one search per seed under
/// [`FallbackClosures::SeedInEdges`], two under
/// [`FallbackClosures::EveryEdge`]). That schedule is what makes "expiry loses
/// at most one step's tail" true, and it is what the budget tests are
/// calibrated against. An unbudgeted sweep reads it nowhere.
///
/// The candidate count is separately bounded at [`MAX_FALLBACK_PATHS`]
/// (checked at every dedup insert, so growth stops the instant the budget is
/// spent rather than after materializing more work). A trip is reported
/// exactly like a deadline expiry -- `truncated`, with everything already
/// found kept -- since both mean the same thing to a caller: the sweep did
/// not get to sample everything it would have, so what came back is a smaller
/// candidate set, not a wrong one.
pub(super) fn sweep(
    search: &IndexedSearch,
    results: &Results,
    config: FallbackConfig,
    deadline: Option<Instant>,
    clock: &mut dyn Clock,
) -> FallbackOutcome {
    let mut scratch = FallbackScratch::new(search, config.tie_break);
    let cap = max_fallback_paths(results.step_count);
    // Dedup on the node-id cycle rather than on names. Within one
    // `IndexedSearch` the id <-> name map is a bijection, so these are the same
    // equivalence classes a name-keyed dedup would give, without allocating a
    // name vector per cycle.
    let mut dedup = CycleDedup::new(cap);
    let mut paths: Vec<Vec<u32>> = Vec::new();
    let mut closings: Vec<(f64, u32, u32)> = Vec::new();
    let mut cycle_buf: Vec<u32> = Vec::new();
    let mut seeds: Vec<u32> = Vec::new();
    let mut steps_processed = 0usize;
    let mut truncated = false;

    'steps: for step in 1..results.step_count {
        if expired(deadline, clock) {
            truncated = true;
            break 'steps;
        }
        scratch.load_step(search, results, step, config.weight);
        scratch.collect_seeds(search, config.seeds, &mut seeds);

        for &seed in &seeds {
            if expired(deadline, clock) {
                truncated = true;
                break 'steps;
            }
            let mut completed = scratch.dijkstra_from(seed, deadline, clock);
            match config.closures {
                FallbackClosures::SeedInEdges => {
                    scratch.collect_closings(seed, &mut closings);
                    for &(_, _, from) in closings.iter() {
                        scratch.write_forward_path(seed, from, &mut cycle_buf);
                        dedup.insert_if_new(&cycle_buf, &mut paths);
                    }
                }
                FallbackClosures::EveryEdge => {
                    // Run the reverse search even when the forward one was cut
                    // short: it stamps a fresh generation and seeds itself
                    // before its first pop, so the closures below always read a
                    // reverse tree belonging to THIS seed rather than the
                    // previous one's.
                    let reverse_completed = scratch.dijkstra_to(seed, deadline, clock);
                    scratch.collect_every_edge_closures(
                        seed,
                        &mut cycle_buf,
                        &mut dedup,
                        &mut paths,
                    );
                    completed = completed && reverse_completed;
                }
            }
            if !completed {
                truncated = true;
                break 'steps;
            }
            if paths.len() >= cap {
                // The candidate budget is spent: `dedup` is already refusing
                // every further insert, so stopping here loses no candidate
                // that a later seed or step would otherwise have contributed
                // -- it only stops paying for searches whose results cannot
                // be kept.
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
