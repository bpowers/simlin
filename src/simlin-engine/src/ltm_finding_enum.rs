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
use std::rc::Rc;
use std::time::Instant;

use super::{Clock, DEADLINE_CHECK_INTERVAL, IndexedSearch, MIN_CONTRIBUTION, expired, is_active};
use crate::results::Results;

/// Closure consulted by [`score_steps`] whenever a circuit edge's target is a
/// module-instance node: `(from_node, module_node, next_node) ->
/// Option<(Rc<Vec<f64>>, (usize, usize))>`, where `next_node` is the node the
/// module hands off to along THIS circuit's traversal order -- the source of
/// the row immediately following `module_node` -- the same "exit-port reader"
/// `FoundLoop` materialization resolves. `Some` REPLACES the raw composite
/// row for that one hop with the returned per-exit-port override series for
/// the whole score computation; `None` (an ambiguous entry/exit port, a
/// pathless module, ...) leaves the raw composite row in place.
///
/// The paired `(usize, usize)` is the override series' OWN active window
/// (`[lo, hi)`, per `is_active`) -- outside it the series is 0 or NaN at
/// every step by construction, so [`effective_scoring_window`] can bound a
/// module-traversing circuit's scoring range by THIS window instead of the
/// full saved-step range, without missing any mass.
///
/// Built once per discovery call (`ltm_finding::ModuleOverrideCache::series`,
/// memoized by the subscript-stripped `(from, module, exit)` triple -- the
/// window is computed once per unique entry alongside the series) and shared
/// by retention (this module) and `FoundLoop` materialization
/// (`ltm_finding.rs`), so the two phases can never disagree about which
/// override a module-traversing loop's score resolves to.
pub(super) type ModuleOverrideFn<'a> =
    dyn FnMut(u32, u32, u32) -> Option<(Rc<Vec<f64>>, (usize, usize))> + 'a;

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
/// linear in emitted rows. The budget is charged in ROW-EQUIVALENTS: each
/// emitted circuit also stores its activity AND bitset (`ceil(step_count/64)`
/// words of 8 bytes, i.e. two `u32` rows per word), so on a high-resolution
/// run -- 100k saved steps is 1,563 words, 12.5 KB, per circuit -- the bitsets
/// rather than the rows are what the bound has to see. World3 (401 steps, 7
/// words per circuit) pays 14 row-equivalents per circuit on top of its 42.
pub(super) const MAX_DISCOVERY_ENUM_EDGE_ROWS: u64 = 20_000_000;

/// How many recorded values [`ActivityGraph::build`] may copy between
/// wall-clock deadline checks.
///
/// The build's work is `union edges x step_count` slab reads, and either
/// factor alone can be the large one: World3 is ~430 edges over 401 saved
/// steps, while a two-variable goal-seeking model saved at 200k steps is 6
/// edges over 200,001. Counting VALUES rather than edges is what keeps one
/// check interval the same amount of work in both shapes: a single edge of
/// the second model is more values than this whole interval, so the copy of
/// one edge's series is itself split into blocks and checked at the block
/// boundaries. An edge-granular check on that model would spend a
/// millisecond-scale budget entirely inside the build and hand the fallback a
/// deadline that had already passed. The first check happens before any value
/// is copied, so an already-expired deadline costs no work at all.
pub(super) const ACTIVITY_BUILD_DEADLINE_CHECK_VALUES: usize = 1 << 16;

/// How many circuits [`retain_circuits`] scores between wall-clock deadline
/// checks. A circuit's scoring pass is O(active steps), so a few thousand of
/// them is the same order of work as the other phases' intervals; the first
/// check is at circuit 0, so an already-expired deadline is caught before any
/// scoring.
///
/// This bounds the wrong thing on its own: a circuit's scoring cost is
/// `O(len * window)`, not `O(1)`, so a model with few but LONG,
/// densely-active circuits (a long `PREVIOUS` chain saved at many steps, say)
/// can do far more work between two circuit-count-spaced checks than the
/// interval's name suggests -- unbounded, in principle, since nothing here
/// caps a single circuit's own length or window. [`RETENTION_DEADLINE_CHECK_EDGE_STEPS`]
/// is the second, WORK-based trigger that closes that gap; the two run
/// together (see [`DeadlineWorkTracker`]), so whichever bound the model's
/// circuit shape stresses is the one that fires.
const RETENTION_DEADLINE_CHECK_CIRCUITS: usize = 4096;

/// How many edge-steps (`circuit_len * scored_window`, summed since the last
/// check) [`retain_circuits`] and [`dedup_trimmed_twins`] may score between
/// wall-clock deadline checks, alongside [`RETENTION_DEADLINE_CHECK_CIRCUITS`]'s
/// circuit-count trigger. Must stay a power of two: the test override
/// (mirroring [`enum_deadline_visit_interval`]) is a single `debug_assert`
/// rather than a mask here, since the comparison is a plain `>=` against a
/// running total instead of a bitwise `visits & (interval - 1)` test -- the
/// running total does not wrap the way a per-visit counter does, so there is
/// no mask to keep aligned, but the power-of-two convention is kept anyway
/// for consistency with the enumerator's own interval and to leave room for a
/// masked implementation later without changing the constant's contract.
const RETENTION_DEADLINE_CHECK_EDGE_STEPS: u64 = 1 << 20;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`RETENTION_DEADLINE_CHECK_EDGE_STEPS`], scoped
    /// by an active [`RetentionDeadlineCheckEdgeStepsGuard`] -- lets a
    /// fixture with two long circuits over many steps reach the work-based
    /// check without needing a model that actually scores a million edge-steps
    /// (docs/dev/rust.md#test-time-budgets).
    static RETENTION_DEADLINE_CHECK_EDGE_STEPS_OVERRIDE: std::cell::Cell<Option<u64>> =
        const { std::cell::Cell::new(None) };
}

/// The effective edge-step interval for [`DeadlineWorkTracker`]'s work-based
/// deadline check.
fn retention_deadline_check_edge_steps() -> u64 {
    #[cfg(test)]
    {
        if let Some(interval) = RETENTION_DEADLINE_CHECK_EDGE_STEPS_OVERRIDE.with(|c| c.get()) {
            debug_assert!(interval.is_power_of_two());
            return interval;
        }
    }
    RETENTION_DEADLINE_CHECK_EDGE_STEPS
}

/// RAII guard (test-only) overriding [`retention_deadline_check_edge_steps`]
/// for the current thread; restores the previous value on drop so a
/// panicking test does not leak the override to the next test on the same
/// thread.
#[cfg(test)]
pub(crate) struct RetentionDeadlineCheckEdgeStepsGuard {
    prev: Option<u64>,
}

#[cfg(test)]
impl RetentionDeadlineCheckEdgeStepsGuard {
    pub(crate) fn new(interval: u64) -> Self {
        let prev = RETENTION_DEADLINE_CHECK_EDGE_STEPS_OVERRIDE.with(|c| c.replace(Some(interval)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for RetentionDeadlineCheckEdgeStepsGuard {
    fn drop(&mut self) {
        RETENTION_DEADLINE_CHECK_EDGE_STEPS_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// Accumulates scoring work between deadline checks for [`retain_circuits`]
/// and [`dedup_trimmed_twins`], and decides when the next check is due.
///
/// A check fires whenever EITHER trigger is due: the circuit-count interval
/// (`RETENTION_DEADLINE_CHECK_CIRCUITS`, which also catches circuit 0 --
/// `0.is_multiple_of(_)` is always true, so an already-expired deadline is
/// caught before any scoring) or the work-based interval
/// (`RETENTION_DEADLINE_CHECK_EDGE_STEPS`, accumulated edge-steps recorded
/// since the last check). Whichever fires resets the work accumulator, so the
/// two triggers cannot compound into checking far more often than either
/// alone would. An unbudgeted call (`deadline: None`) never reads the clock:
/// `expired` short-circuits before touching it either way.
///
/// [`Self::check`] runs BEFORE a circuit is scored (so an already-expired
/// deadline is caught before any work happens) and [`Self::record`] runs
/// AFTER, which is why they are two calls rather than one: the work a check
/// can react to is always the PRIOR circuits' work, never the one about to be
/// scored.
struct DeadlineWorkTracker {
    edge_steps_since_check: u64,
}

impl DeadlineWorkTracker {
    fn new() -> Self {
        DeadlineWorkTracker {
            edge_steps_since_check: 0,
        }
    }

    /// Record `len * window` edge-steps just scored for the circuit the
    /// caller's most recent [`Self::check`] cleared.
    fn record(&mut self, edge_steps: u64) {
        self.edge_steps_since_check += edge_steps;
    }

    /// A closing check for work recorded after the last per-circuit check:
    /// the final (or only) circuit can be arbitrarily large, and a deadline
    /// that expired inside its scoring must not let the pass report success.
    /// Reads the clock only when at least a full interval of work is pending.
    fn check_pending(&mut self, deadline: Option<Instant>, clock: &mut dyn Clock) -> bool {
        if self.edge_steps_since_check < retention_deadline_check_edge_steps() {
            return false;
        }
        self.edge_steps_since_check = 0;
        expired(deadline, clock)
    }

    /// Whether a check is due for circuit `ci` (either trigger) and, if so,
    /// whether the deadline has expired. Resets the work accumulator whenever
    /// a check actually runs, regardless of which trigger fired it.
    fn check(&mut self, ci: usize, deadline: Option<Instant>, clock: &mut dyn Clock) -> bool {
        let work_due = self.edge_steps_since_check >= retention_deadline_check_edge_steps();
        let circuit_due = ci.is_multiple_of(RETENTION_DEADLINE_CHECK_CIRCUITS);
        if !(work_due || circuit_due) {
            return false;
        }
        self.edge_steps_since_check = 0;
        expired(deadline, clock)
    }
}

/// Byte budget for the union graph's own storage: the contiguous per-edge
/// score rows plus activity bitsets `ActivityGraph::build` copies out of the
/// results slab, `union_edges x step_count x (8 B + 1 bit)`. The circuit and
/// edge-row budgets bound the ENUMERATION; nothing else bounds this copy,
/// which on a many-edge, many-saved-step model duplicates a large part of the
/// resident results before a single circuit is considered. World3 is 258 x
/// 401 x 8 B ~ 0.8 MB and C-LEARN ~6 MB; a 20,000-edge model saved at
/// 100,000 steps would be 16 GB. Above this the build abandons and discovery
/// takes the fallback, which reads scores straight from the results slab and
/// whose own materialization is bounded by [`super::fallback`]'s
/// byte-derived candidate cap.
pub(super) const MAX_ACTIVITY_GRAPH_BYTES: usize = 512 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`MAX_ACTIVITY_GRAPH_BYTES`], scoped by an active
    /// [`ActivityGraphBytesGuard`] so a two-edge fixture can trip the storage
    /// budget instead of one large enough to trip the production constant.
    static ACTIVITY_GRAPH_BYTES_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The effective union-graph storage budget.
fn max_activity_graph_bytes() -> usize {
    #[cfg(test)]
    {
        if let Some(b) = ACTIVITY_GRAPH_BYTES_OVERRIDE.with(|c| c.get()) {
            return b;
        }
    }
    MAX_ACTIVITY_GRAPH_BYTES
}

/// RAII guard (test-only) overriding [`max_activity_graph_bytes`] for the
/// current thread; restores the previous value on drop.
#[cfg(test)]
pub(crate) struct ActivityGraphBytesGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl ActivityGraphBytesGuard {
    pub(crate) fn new(bytes: usize) -> Self {
        let prev = ACTIVITY_GRAPH_BYTES_OVERRIDE.with(|c| c.replace(Some(bytes)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for ActivityGraphBytesGuard {
    fn drop(&mut self) {
        ACTIVITY_GRAPH_BYTES_OVERRIDE.with(|c| c.set(self.prev));
    }
}

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
    /// set when the edge's score at saved step `t` is active by
    /// [`super::is_active`] -- finite nonzero, or infinite (a real divergent
    /// signal the totals keep; only NaN and exact 0 are inactive). That rule
    /// is shared with the fallback, so both generators agree on which cycles
    /// exist.
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
        if expired(deadline, clock) {
            return None;
        }
        // Values this check interval has left. Decremented by every value
        // copied and refilled at each check, so the interval spans the same
        // work whether the graph is wide (many short edges) or deep (few long
        // ones).
        let mut values_until_check = ACTIVITY_BUILD_DEADLINE_CHECK_VALUES;

        let mut adj: Vec<Vec<(u32, u32)>> = vec![Vec::new(); n_nodes];
        let mut radj: Vec<Vec<u32>> = vec![Vec::new(); n_nodes];
        let mut bits: Vec<u64> = Vec::new();
        let mut series: Vec<f64> = Vec::new();
        // Storage this build has committed to; the per-edge cost is one score
        // row plus one bitset. Checked before each append so the copy never
        // exceeds the budget by more than one edge.
        let bytes_per_edge =
            step_count * std::mem::size_of::<f64>() + words * std::mem::size_of::<u64>();
        let byte_budget = max_activity_graph_bytes();
        let mut bytes_committed = 0usize;
        let mut edge_from: Vec<u32> = Vec::new();

        let mut edge_bits = vec![0u64; words];
        let mut edge_series = vec![0.0f64; step_count];

        for (from, edges) in search.adj.iter().enumerate() {
            for edge in edges {
                if edge.to as usize == from {
                    // A self-edge can never be part of an elementary cycle of
                    // length >= 2 (such a cycle never repeats a node), and a
                    // one-variable "loop" is not feedback -- see the module
                    // doc. Dropping it here keeps it out of the traversal, the
                    // SCC structure, and the scoring rows alike. It copies no
                    // values, so it also consumes no check interval.
                    continue;
                }
                edge_bits.iter_mut().for_each(|w| *w = 0);
                let mut any = false;
                let mut step = 0usize;
                while step < step_count {
                    if values_until_check == 0 {
                        if expired(deadline, clock) {
                            return None;
                        }
                        values_until_check = ACTIVITY_BUILD_DEADLINE_CHECK_VALUES;
                    }
                    let block_end = (step + values_until_check).min(step_count);
                    for s in step..block_end {
                        let base = s * results.step_size;
                        // The score row keeps the edge's RECORDED value (the
                        // module composite for a module-input edge): scoring
                        // must never bank a value materialization cannot
                        // report, and where a per-exit-port override exists
                        // it substitutes for this row anyway. Only ACTIVITY
                        // is read through the composite's NaN shadow, so a
                        // circuit whose override is finite there is
                        // enumerated and then scored honestly -- NaN when
                        // no override resolves.
                        edge_series[s] = results.data[base + edge.offset];
                        if is_active(edge.value_at(results, base)) {
                            edge_bits[s / 64] |= 1u64 << (s % 64);
                            // Membership in the union graph is decided over
                            // `1..step_count` only, so a step-0-only edge is
                            // excluded -- the same window the fallback sweeps.
                            any |= s >= 1;
                        }
                    }
                    values_until_check -= block_end - step;
                    step = block_end;
                }
                if any {
                    bytes_committed += bytes_per_edge;
                    if bytes_committed > byte_budget {
                        // The union graph would outgrow its storage budget;
                        // abandon so discovery takes the fallback, which
                        // reads the results slab in place.
                        return None;
                    }
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
        // The union-wide SCC pass is O(nodes + edges) with no check inside;
        // charge it like any other phase and read the clock once after it
        // when the graph alone is a check interval of work.
        if n_nodes as u64 + edge_from.len() as u64 >= u64::from(DEADLINE_CHECK_INTERVAL)
            && expired(deadline, clock)
        {
            return None;
        }
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
    /// The union-graph edge row for `from -> to`, if that edge is ever active.
    #[cfg(test)]
    pub(super) fn edge_row_of(&self, from: u32, to: u32) -> Option<u32> {
        self.adj[from as usize]
            .iter()
            .find(|(t, _)| *t == to)
            .map(|(_, row)| *row)
    }

    /// The (NaN-shadow-repaired) score of edge `row` at saved step `step`.
    #[cfg(test)]
    pub(super) fn score_at(&self, row: u32, step: usize) -> f64 {
        self.series[row as usize * self.step_count + step]
    }

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
    /// `work` receives the number of adjacency entries the two traversals
    /// scanned, so the caller can charge it against the same visit budget and
    /// deadline schedule as the DFS proper -- on a large union SCC this pass
    /// alone is a full forward and reverse sweep per root.
    fn recompute(&mut self, graph: &ActivityGraph, root: u32, work: &mut u64) -> bool {
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
            *work += graph.adj[v as usize].len() as u64;
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
            *work += graph.radj[v as usize].len() as u64;
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

    /// Append a loop given as a NODE path -- a stitched cross-agg sequence,
    /// whose every consecutive pair (wrapping) is a union-graph edge -- so it
    /// joins the enumerated circuits in retention as one more candidate: same
    /// trimmed-key dedup, same bank/confirm, same universe count. Its activity
    /// AND is the AND of its edges' bitsets, exactly what the enumerator would
    /// have carried had it emitted the sequence itself; a sequence whose
    /// petals are never simultaneously active gets an empty AND and, like any
    /// circuit that banks no mass, is neither a survivor nor a universe member.
    pub(super) fn push_node_path(&mut self, path: &[u32], graph: &ActivityGraph) {
        let rows = path_edge_rows(path, graph);
        let mut and_bits = vec![u64::MAX; self.words];
        for &row in &rows {
            for (acc, bit) in and_bits.iter_mut().zip(graph.edge_bits(row)) {
                *acc &= *bit;
            }
        }
        let (open, closing) = rows.split_at(rows.len() - 1);
        self.push(open, closing[0], &and_bits);
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

    /// Total storage in `u32` row-equivalents -- the quantity
    /// [`MAX_DISCOVERY_ENUM_EDGE_ROWS`] bounds: the emitted edge rows plus each
    /// circuit's activity bitset at two row-equivalents per 8-byte word.
    fn total_rows(&self) -> usize {
        self.rows.len() + self.activity.len() * 2
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
    // Adjacency entries the per-root pruning passes scanned, charged to the
    // visit budget alongside the DFS's own `visits`.
    let mut prune_total: u64 = 0;

    for root in 0..n_nodes as u32 {
        // The per-root pruning pass is charged to the same visit budget and
        // deadline schedule as the DFS: its two traversals over a large union
        // SCC are real work that must not run past the caller's budget
        // unnoticed, and a root whose induced component is trivial would
        // otherwise never reach a check at all.
        let mut prune_work = 0u64;
        let admits_root = explorable.recompute(graph, root, &mut prune_work);
        // Pruning work counts against the visit BUDGET but not against the
        // DFS's clock-read schedule (`visits`, whose first-visit and interval
        // arms the deadline tests are calibrated to): the clock is read for a
        // pruning pass only when that pass alone was a full check interval of
        // work (a large union SCC), so a small root's pass costs no read.
        prune_total += prune_work;
        if visits + prune_total > max_visits
            || (prune_work >= u64::from(DEADLINE_CHECK_INTERVAL) && expired(deadline, clock))
        {
            out.complete = false;
            return out;
        }
        if !admits_root {
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
                if visits + prune_total > max_visits {
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
    /// Mass-bearing Solo circuits that passed retention but are NOT in
    /// `survivors`: retention keeps only the strongest `max_loops()` Solo loops
    /// (the most the report can hold), and a caller's `retained_loops` must
    /// still count these so a capped stockless report is not mistaken for the
    /// whole retained set.
    pub solo_survivors_beyond_cap: usize,
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
    /// How many of the enumerated circuits are DISTINCT loops carrying mass:
    /// non-representative twins the trimmed-key dedup dropped are excluded,
    /// and so is any circuit whose reported product is zero or NaN at every
    /// step (its edges are individually active but it banks nothing, so it is
    /// not one of the loops the denominators sum). `ltm_finding.rs` adds the
    /// (deduped) stitched cross-agg loop count to this to populate
    /// `DiscoveryResult::universe_loops`.
    pub distinct_circuits: usize,
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
/// One class skips both: a circuit in a Solo group (no stock resolves to a
/// partition) is its own denominator, so its relative score is +/-1 wherever
/// it is active and "ever active" is the whole test -- which the enumerator
/// guarantees, since a circuit is emitted only with a nonempty activity AND;
/// the check that the AND is nonempty is kept as cheap defense in depth.
///
/// **Retention is exact against the reported series for every circuit,
/// module-traversing ones included.** A module-traversing circuit's edge row
/// whose target is the module instance is scored through `override_for`
/// (`ltm_finding::ModuleOverrideCache`, the SAME cache `FoundLoop`
/// materialization consults), which substitutes the per-exit-port override
/// series -- the score the loop actually REPORTS -- for the raw composite row
/// whenever a single exit pathway resolves; where it does not (an ambiguous
/// entry/exit port, a pathless module), the raw composite row stands in for
/// both retention and materialization alike, so the two phases can never
/// judge a circuit against different numbers. Because that substitution can
/// make the circuit active at steps its RAW composite window excludes (the
/// composite and the override are different series), a module-traversing
/// circuit is scored over the OVERRIDE'S OWN active window rather than the
/// raw composite's -- exact, and far cheaper than the full saved-step range
/// in practice -- see [`effective_scoring_window`]'s doc.
///
/// Two enumerated circuits can also trim to the same *reported* loop (a
/// direct reference and its hoisted-reducer twin, AC4.3); [`dedup_trimmed_twins`]
/// decides the representative BEFORE either candidate's mass reaches a
/// partition total -- module-traversing circuits included, now that their
/// reported (override) average is computable -- so this pass's own totals
/// already are the totals `rank_and_filter` will use, and a non-representative
/// twin banks no mass, is never confirmed, and is never a survivor. The one
/// remaining boundary is a STITCHED cross-agg loop (a combination of petals
/// assembled after this pass runs, not one of its circuits) colliding with a
/// reported loop's trimmed identity; `ltm_finding.rs`'s post-materialization
/// `by_reported_cycle` dedup and its `subtract_reported_mass_from_totals`
/// safety net exist for exactly that case.
///
/// Nothing per-circuit larger than O(1) is retained for non-survivors, so the
/// pass is safe at [`MAX_DISCOVERY_ENUM_CIRCUITS`] scale.
///
/// A step whose product is not a number -- a NaN link, or the `Inf * 0` a
/// finite-overflow-then-zero circuit produces -- follows the engine's
/// loop-score rules: the loop's score there is NaN, excluded from the totals
/// and unable to satisfy retention. Deadline expiry mid-pass returns `None`
/// (caller falls back).
#[allow(clippy::too_many_arguments)]
pub(super) fn retain_circuits(
    candidates: &EnumeratedCandidates,
    graph: &ActivityGraph,
    stock_partition_of_node: &[Option<usize>],
    is_module_node: &[bool],
    is_agg_node: &[bool],
    override_for: &mut ModuleOverrideFn,
    deadline: Option<Instant>,
    clock: &mut dyn Clock,
) -> Option<RetentionOutcome> {
    let step_count = graph.step_count;
    // Computed ONCE for the whole call (an O(n_nodes) scan, not O(circuits))
    // and threaded through every scoring site below, so a module-free graph
    // -- the overwhelming majority, and the whole of World3 -- never pays a
    // per-circuit `effective_scoring_window` lookup or a per-row branch in
    // `score_steps`; see that function's doc for the measured cost of
    // getting this wrong.
    let has_module_node = is_module_node.contains(&true);

    let mut partition_totals: HashMap<usize, Vec<f64>> = HashMap::new();
    let mut partition_circuit_counts: HashMap<usize, usize> = HashMap::new();
    let mut scratch = vec![0.0f64; step_count];
    let mut nan_mask = vec![false; step_count];

    let mut survivors: Vec<usize> = Vec::new();
    // Mass-bearing Solo circuits as `(mean |reported score|, index)`; only
    // the top `max_loops()` become survivors (see the Solo arm below).
    let mut solo_ranked: Vec<(f64, usize)> = Vec::new();
    let mut to_confirm: Vec<usize> = Vec::new();
    // Shared across `dedup_trimmed_twins` and both loops below, so the
    // work-based deadline trigger sees this whole call's scoring as one
    // continuous budget rather than resetting at each phase boundary.
    let mut work = DeadlineWorkTracker::new();

    let dropped = dedup_trimmed_twins(
        candidates,
        graph,
        has_module_node,
        is_module_node,
        is_agg_node,
        override_for,
        &mut work,
        &mut scratch,
        &mut nan_mask,
        deadline,
        clock,
    )?;
    // Distinct loops whose mass the denominators (or, for a Solo circuit,
    // its own reported series) actually carry: counted below as each
    // circuit banks nonzero mass, so a circuit whose product underflows to
    // zero at every step is neither a universe member nor a survivor.
    let mut distinct_circuits = 0usize;

    for (ci, &is_dropped) in dropped.iter().enumerate() {
        if work.check(ci, deadline, clock) {
            return None;
        }
        if is_dropped {
            // A non-representative twin: no raw mass, no confirm, not a
            // survivor. Its reported representative already covers the
            // same reported loop.
            continue;
        }
        let rows = candidates.circuit(ci);
        let partition = circuit_partition(rows, graph, stock_partition_of_node);
        let (lo, hi) = graph.active_window(candidates.activity_of(ci));
        // A module-traversing circuit's raw activity window is a window over
        // the COMPOSITE score, which can disagree with the override series
        // `score_steps` may substitute for it. The circuit's own window stays
        // exact under substitution and the override's window is exact on its
        // own, so score over their intersection (see
        // `effective_scoring_window`'s doc) -- and only look it up when
        // `has_module_node`, so a module-free graph never pays per circuit.
        let (score_lo, score_hi) = if has_module_node {
            effective_scoring_window(rows, graph, is_module_node, override_for, (lo, hi))
        } else {
            (lo, hi)
        };

        let Some(part) = partition else {
            // Solo: its own reported series is its denominator, so retention
            // reduces to "does it ever carry nonzero mass" -- decided by
            // scoring it (edges individually active at every step still let
            // the product underflow to 0, and an override can zero it), not
            // by the activity window alone. Nothing is banked anywhere.
            //
            // A Solo loop's relative score is +/-1 wherever it is active, so
            // the ranking can only ever separate Solo loops by raw magnitude
            // and reports at most `max_loops()` of them (after every
            // competing loop). Retention therefore keeps exactly the top
            // `max_loops()` Solo circuits by mean |reported score| -- the
            // same statistic materialization computes as `avg_abs_score`,
            // over the FULL saved-step range so the two agree step for step
            // -- and no more: on a stockless component with hundreds of
            // thousands of mass-bearing cycles that is the difference between
            // materializing 200 loops and materializing all of them.
            score_steps(
                rows,
                graph,
                0,
                has_module_node,
                is_module_node,
                override_for,
                &mut scratch[..],
                &mut nan_mask[..],
            );
            work.record(rows.len() as u64 * step_count as u64);
            let any_mass = (0..step_count).any(|t| !nan_mask[t] && scratch[t] != 0.0);
            if any_mass {
                distinct_circuits += 1;
                solo_ranked.push((mean_abs_over_valid(&scratch, &nan_mask), ci));
            }
            continue;
        };

        score_steps(
            rows,
            graph,
            score_lo,
            has_module_node,
            is_module_node,
            override_for,
            &mut scratch[score_lo..score_hi],
            &mut nan_mask[score_lo..score_hi],
        );
        work.record(rows.len() as u64 * (score_hi - score_lo) as u64);
        let totals = partition_totals
            .entry(part)
            .or_insert_with(|| vec![0.0; step_count]);
        let mut bound = 0.0f64;
        // Whether this circuit ever banks NONZERO mass -- an edge can be
        // individually active (nonzero, finite or Inf) at every step of the
        // circuit's activity window while the PRODUCT still underflows to
        // exactly 0 (or, via `Inf * 0`, is NaN) at every one of them. Such a
        // circuit contributes nothing to any total and can satisfy no
        // threshold, so it must not inflate its partition's universe count
        // either: that count means "how many loops' mass is in this total",
        // and a circuit that banked none is not one of them. The SAME rule
        // now decides a module-traversing circuit's membership too -- there
        // is no separate unconditional-keep arm any more.
        let mut banked_mass = false;
        for t in score_lo..score_hi {
            if nan_mask[t] {
                continue;
            }
            let mass = scratch[t].abs();
            if mass != 0.0 {
                banked_mass = true;
            }
            // Saturating: a sum of FINITE masses that would overflow to Inf
            // would make every finite share read as 0 and drop a real loop
            // universe wholesale; capping the total at f64::MAX keeps shares
            // finite (and merely compressed) there. A genuinely infinite mass
            // still makes the total Inf, the dominance-inflection convention.
            let sum = totals[t] + mass;
            totals[t] = if sum.is_infinite() && mass.is_finite() && totals[t].is_finite() {
                f64::MAX
            } else {
                sum
            };
            if totals[t] > 0.0 {
                // `max` drops a NaN ratio (`Inf / Inf` at a dominance
                // inflection), which is right: the exact test rejects that step
                // too, so ignoring it costs no survivor.
                bound = bound.max(mass / totals[t]);
            }
        }
        if banked_mass {
            *partition_circuit_counts.entry(part).or_insert(0) += 1;
            distinct_circuits += 1;
        }
        if bound >= MIN_CONTRIBUTION {
            to_confirm.push(ci);
        }
    }

    for (n, &ci) in to_confirm.iter().enumerate() {
        if work.check(n, deadline, clock) {
            return None;
        }
        let rows = candidates.circuit(ci);
        let part = circuit_partition(rows, graph, stock_partition_of_node)
            .expect("only partitioned circuits reach the confirm step");
        let (lo, hi) = graph.active_window(candidates.activity_of(ci));
        let (score_lo, score_hi) = if has_module_node {
            effective_scoring_window(rows, graph, is_module_node, override_for, (lo, hi))
        } else {
            (lo, hi)
        };
        score_steps(
            rows,
            graph,
            score_lo,
            has_module_node,
            is_module_node,
            override_for,
            &mut scratch[score_lo..score_hi],
            &mut nan_mask[score_lo..score_hi],
        );
        work.record(rows.len() as u64 * (score_hi - score_lo) as u64);
        let totals = &partition_totals[&part];
        let keep = (score_lo..score_hi).any(|t| {
            !nan_mask[t] && totals[t] > 0.0 && scratch[t].abs() / totals[t] >= MIN_CONTRIBUTION
        });
        if keep {
            survivors.push(ci);
        }
    }
    // Strongest Solo loops first, index order among exact ties (the ranking's
    // own content-key tie-break decides presentation later; retention only
    // has to keep every loop the ranking could report).
    if work.check_pending(deadline, clock) {
        return None;
    }
    solo_ranked.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let solo_kept = solo_ranked.len().min(super::max_loops());
    let solo_survivors_beyond_cap = solo_ranked.len() - solo_kept;
    survivors.extend(solo_ranked.iter().take(solo_kept).map(|&(_, ci)| ci));
    survivors.sort_unstable();

    Some(RetentionOutcome {
        survivors,
        solo_survivors_beyond_cap,
        partition_totals,
        partition_circuit_counts,
        distinct_circuits,
    })
}

/// The identity [`retain_circuits`]' trimmed-key dedup groups circuits by:
/// the circuit's node sequence with every synthetic `$⁚ltm⁚agg⁚{n}` node
/// removed, canonically rotated.
///
/// This is the node-id twin of what `ltm_finding.rs`'s
/// `trim_synthetic_aggs_from_loop_links` (merging agg-touching links) plus
/// `crate::ltm::canonical_rotation` (over the trimmed links' `from` idents)
/// produce once a circuit has been materialized into a reported loop's node
/// names -- the two MUST agree, since they are two routes to the same
/// question ("which reported loop does this circuit trim to?") asked at two
/// different points in the pipeline, and `ltm_finding_tests.rs`'s
/// `retention_dedup_key_matches_the_materialization_trim` asserts the
/// equivalence directly rather than trusting the argument. Operating on node
/// ids rather than names avoids a string allocation per circuit; within one
/// `IndexedSearch` the id <-> name map is a bijection, so the equivalence
/// classes are identical either way.
///
/// A circuit with no agg node in its path is already its own trimmed key
/// (filtering removes nothing), which is exactly why only an agg-bearing
/// circuit can ever collide with another distinct circuit here: an edge row
/// is a complete identity for a `(from, to)` pair, so two DISTINCT non-agg
/// circuits can never share a node sequence.
pub(super) fn trimmed_circuit_key(
    rows: &[u32],
    graph: &ActivityGraph,
    is_agg_node: &[bool],
) -> Vec<u32> {
    let nodes: Vec<u32> = rows
        .iter()
        .map(|&row| graph.edge_source(row))
        .filter(|&n| !is_agg_node[n as usize])
        .collect();
    crate::ltm::canonical_rotation(&nodes)
}

/// One circuit's mean `|score|` over the saved steps where its raw product is
/// a number, computed over the FULL saved-step range -- the exact statistic
/// `FoundLoop::avg_abs_score` computes for a materialized loop, so a winner
/// picked here by comparing this value is the same winner `ltm_finding.rs`'s
/// post-materialization `by_reported_cycle` dedup would pick if it ever saw
/// both circuits (it never does, once this drops one). Module-traversing
/// circuits are scored through `override_for` exactly like every other row
/// (see `score_steps`), so their average is now the REPORTED average -- the
/// per-exit-port override series' magnitude, not the raw composite's -- which
/// is what makes them eligible to compete for representative status here at
/// all (AC4.3 for module twins).
///
/// Deliberately NOT windowed to the circuit's `active_window`: outside that
/// window a step is either NaN (excluded from both the sum and the count) or
/// exactly 0 (excluded from the sum but COUNTED), and a windowed computation
/// would silently drop that second class from the denominator, changing the
/// average relative to what materialization will report. Only circuits that
/// actually collide with another circuit's trimmed key pay this full-range
/// cost -- see [`dedup_trimmed_twins`].
fn raw_avg_abs_score(
    rows: &[u32],
    graph: &ActivityGraph,
    has_module_node: bool,
    is_module_node: &[bool],
    override_for: &mut ModuleOverrideFn,
    scratch: &mut [f64],
    nan_mask: &mut [bool],
) -> f64 {
    score_steps(
        rows,
        graph,
        0,
        has_module_node,
        is_module_node,
        override_for,
        scratch,
        nan_mask,
    );
    mean_abs_over_valid(scratch, nan_mask)
}

/// Mean of `|series[t]|` over the steps `nan_mask` does not exclude, as a
/// running (Welford) mean rather than `sum / count`: a sum of large finite
/// values can overflow to Inf while their mean is representable, and this
/// statistic decides a twin's representative and a Solo loop's rank, where an
/// Inf on both sides would hand the decision to circuit index instead of
/// strength. `FoundLoop::avg_abs_score` uses the same formula so the two agree
/// step for step.
pub(super) fn mean_abs_over_valid(series: &[f64], nan_mask: &[bool]) -> f64 {
    let mut mean = 0.0f64;
    let mut n = 0usize;
    let mut any_inf = false;
    for (i, &is_nan) in nan_mask.iter().enumerate() {
        if is_nan {
            continue;
        }
        let v = series[i].abs();
        // An infinite observation makes the mean infinite; folding it into
        // the running update would produce NaN (`Inf - Inf`) and hand the
        // decision to circuit order instead of to the divergent loop.
        if v.is_infinite() {
            any_inf = true;
            continue;
        }
        n += 1;
        mean += (v - mean) / n as f64;
    }
    if any_inf { f64::INFINITY } else { mean }
}

/// Decide, before either candidate's mass reaches a partition total, which
/// enumerated circuits are non-representative twins of another circuit that
/// trims to the identical reported loop (AC4.3 exactness).
///
/// Only a circuit visiting >= 1 synthetic agg node can have a trimmed twin
/// (see [`trimmed_circuit_key`]'s doc), so the work here is bounded by the
/// model's agg-bearing circuit population rather than the whole universe:
///
/// 1. **Cheap short-circuit**: if the graph has no agg node at all (the
///    overwhelming majority of models, including every purely-scalar one),
///    nothing can collide and no circuit is touched.
/// 2. **Group the agg-bearing circuits** by their trimmed key, scoring each
///    with [`raw_avg_abs_score`] -- module-traversing circuits included, now
///    that their REPORTED (override) average is computable and comparable to
///    a non-module twin's. This population is bounded by the model's
///    synthetic agg count, not by the universe size.
/// 3. **Test every non-agg circuit's OWN identity** (its full node sequence
///    IS its trimmed key) against those keys. This still costs one
///    node-sequence materialization per non-agg circuit -- O(total edge
///    rows), the same order `circuit_partition`/`effective_scoring_window`
///    already pay -- but the expensive part, scoring, runs only on an actual
///    match, which is exactly the (rare) direct/agg-twin collision this
///    exists to catch.
///
/// Ties break exactly as `by_reported_cycle` does: strictly greater
/// `raw_avg_abs_score` wins, and among equal scores the SMALLEST circuit
/// index wins (matching `by_reported_cycle`'s left-to-right "only a strict
/// improvement replaces the representative" scan over ascending circuit
/// index) -- computed order-independently here since circuits reach this
/// decision from two separate loops (step 2's agg population, step 3's
/// colliding non-agg one) that do not themselves run in circuit-index order
/// relative to each other.
#[allow(clippy::too_many_arguments)]
fn dedup_trimmed_twins(
    candidates: &EnumeratedCandidates,
    graph: &ActivityGraph,
    has_module_node: bool,
    is_module_node: &[bool],
    is_agg_node: &[bool],
    override_for: &mut ModuleOverrideFn,
    work: &mut DeadlineWorkTracker,
    scratch: &mut [f64],
    nan_mask: &mut [bool],
    deadline: Option<Instant>,
    clock: &mut dyn Clock,
) -> Option<Vec<bool>> {
    let mut dropped = vec![false; candidates.len()];

    if !is_agg_node.contains(&true) {
        return Some(dropped);
    }

    let mut groups: HashMap<Vec<u32>, Vec<(usize, f64)>> = HashMap::new();

    for ci in 0..candidates.len() {
        if work.check(ci, deadline, clock) {
            return None;
        }
        let rows = candidates.circuit(ci);
        let has_agg = rows
            .iter()
            .any(|&row| is_agg_node[graph.edge_source(row) as usize]);
        if !has_agg {
            continue;
        }
        let key = trimmed_circuit_key(rows, graph, is_agg_node);
        let avg = raw_avg_abs_score(
            rows,
            graph,
            has_module_node,
            is_module_node,
            override_for,
            scratch,
            nan_mask,
        );
        // `raw_avg_abs_score` scores the FULL step range regardless of any
        // circuit's own activity window (see its doc), so that is the work
        // it actually did.
        work.record(rows.len() as u64 * graph.step_count as u64);
        groups.entry(key).or_default().push((ci, avg));
    }

    if groups.is_empty() {
        // Agg nodes exist somewhere in the graph, but no enumerated circuit
        // visits one -- nothing to group.
        return Some(dropped);
    }

    for ci in 0..candidates.len() {
        if work.check(ci, deadline, clock) {
            return None;
        }
        let rows = candidates.circuit(ci);
        let has_agg = rows
            .iter()
            .any(|&row| is_agg_node[graph.edge_source(row) as usize]);
        if has_agg {
            continue; // already scored and grouped above
        }
        let key = trimmed_circuit_key(rows, graph, is_agg_node);
        let Some(members) = groups.get_mut(&key) else {
            continue; // no agg-bearing circuit shares this identity
        };
        let avg = raw_avg_abs_score(
            rows,
            graph,
            has_module_node,
            is_module_node,
            override_for,
            scratch,
            nan_mask,
        );
        work.record(rows.len() as u64 * graph.step_count as u64);
        members.push((ci, avg));
    }

    for members in groups.values() {
        if members.len() < 2 {
            // A solitary agg-bearing circuit whose trimmed identity no other
            // circuit shares -- e.g. its would-be direct twin's edges do not
            // exist in the union graph at all -- has nothing to be
            // representative OF.
            continue;
        }
        let mut winner = members[0];
        for &(idx, avg) in &members[1..] {
            if avg > winner.1 || (avg == winner.1 && idx < winner.0) {
                winner = (idx, avg);
            }
        }
        for &(idx, _) in members {
            if idx != winner.0 {
                dropped[idx] = true;
            }
        }
    }

    Some(dropped)
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

/// The scoring window `retain_circuits` should use for `rows`: `raw_window`
/// (the circuit's own enumerated activity window) for a circuit that touches
/// no module node or whose module row's override declines, and the
/// INTERSECTION of `raw_window` with the override's own active window when a
/// row substitutes.
///
/// Both windows are exact bounds on where the substituted product can be
/// nonzero, so their intersection is too:
///
/// - `raw_window` stays valid under substitution. The substituted row carries
///   the module COMPOSITE in the raw rows, and an override series is one
///   pathway of that composite (the composite max-abs-folds every pathway), so
///   the override is nonzero only where the composite is. Every other row is
///   unchanged. Outside `raw_window` some raw row is therefore exactly 0 or
///   NaN at the step: a non-substituted row zeroes/poisons the product
///   regardless of the substitution; the composite row being 0 means the
///   override is 0 there too. The one exception is a composite that is NaN
///   where its override pathway is finite (the NaN-shadowing of the max-abs
///   fold), which is ALREADY the activity graph's own documented boundary --
///   the activity bit is computed from the composite -- so no new loss.
/// - The override's window is valid on its own: outside it the override
///   contributes exactly 0 or NaN by construction (see
///   `ModuleOverrideCache::series`'s doc), so the substituted product is 0 or
///   NaN there whatever the other rows do.
///
/// An elementary circuit repeats no node, so at most one row's target is a
/// module instance and one lookup decides the window (a memoized `HashMap`
/// hit after the first). Circuits touching no module node never look up.
/// The intersection matters for cost, not just tightness: on World3 most
/// circuits pass through a SMOOTH/DELAY module whose pathway is active for
/// nearly the whole run, so the override window alone is ~5x wider than the
/// circuit's own ~79-step activity window; scoring over the override window
/// alone regressed discovery from ~0.4 s to ~1.1 s.
fn effective_scoring_window(
    rows: &[u32],
    graph: &ActivityGraph,
    is_module_node: &[bool],
    override_for: &mut ModuleOverrideFn,
    raw_window: (usize, usize),
) -> (usize, usize) {
    let len = rows.len();
    for i in 0..len {
        let to = graph.edge_source(rows[(i + 1) % len]);
        if is_module_node[to as usize] {
            let from = graph.edge_source(rows[i]);
            let next = graph.edge_source(rows[(i + 2) % len]);
            return match override_for(from, to, next) {
                Some((_series, (olo, ohi))) => {
                    let (rlo, rhi) = raw_window;
                    // Intersection; an empty range means the substituted
                    // product is 0/NaN everywhere, and `score_steps` handles
                    // `lo >= hi` as "no mass".
                    (rlo.max(olo), rhi.min(ohi).max(rlo.max(olo)))
                }
                None => raw_window,
            };
        }
    }
    raw_window
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

/// One loop's signed per-step score series (product of link values) over the
/// saved steps `lo..lo + out.len()`, with `nan_mask[i]` set when the product at
/// that step is not a number.
///
/// Dispatches on `has_module_node` -- true exactly when the graph contains at
/// least one module-instance node ANYWHERE, computed once per retention call
/// rather than re-scanned per circuit or per row. A module-free model (the
/// overwhelming majority of the corpus, and the whole of World3) takes
/// [`score_steps_plain`] -- the exact tight, auto-vectorizable loop this
/// function had before module override substitution existed, with no per-row
/// branch, no `override_for` call, and no `Option<Rc<..>>` construction on the
/// hot path. That distinction is not cosmetic: measured on World3 (150,827
/// circuits, ~6.3M edge rows), routing every circuit through the general
/// per-row-branching path even with the branch never taken regressed
/// enumeration from ~0.4s to ~1.05s -- the extra indirection was enough to
/// defeat the compiler's vectorization of the inner per-step multiply loop.
/// [`score_steps_with_overrides`] is the general path, taken only when the
/// graph actually has a module node somewhere (module-bearing circuits are
/// still the minority even on a model like C-LEARN that uses SMOOTH/DELAY
/// throughout, since most circuits never traverse one).
#[allow(clippy::too_many_arguments)]
fn score_steps(
    rows: &[u32],
    graph: &ActivityGraph,
    lo: usize,
    has_module_node: bool,
    is_module_node: &[bool],
    override_for: &mut ModuleOverrideFn,
    out: &mut [f64],
    nan_mask: &mut [bool],
) {
    if has_module_node {
        score_steps_with_overrides(rows, graph, lo, is_module_node, override_for, out, nan_mask);
    } else {
        score_steps_plain(rows, graph, lo, out, nan_mask);
    }
}

/// The module-free fast path [`score_steps`] dispatches to when the graph has
/// no module-instance node anywhere: a straight-line, edge-outer/step-inner
/// multiply against the graph's contiguous score rows that the compiler
/// auto-vectorizes, byte-identical to `score_steps` before module override
/// substitution existed (see that function's doc for why the distinction is
/// load-bearing for performance, not just style).
///
/// NaN needs no special case: it propagates through multiplication, so the mask
/// is a property of the finished product. That is deliberately stronger than
/// testing the links: an `Inf * 0` product is NaN with no NaN link anywhere, and
/// treating it as a number would poison a whole partition total with NaN.
fn score_steps_plain(
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

/// The general, override-aware path [`score_steps`] dispatches to when the
/// graph has at least one module-instance node: for a row whose target is NOT
/// a module-instance node it is the same straight-line multiply
/// [`score_steps_plain`] always takes; for a row whose target IS a
/// module-instance node (`is_module_node[to]`), `override_for(from, module,
/// next)` is consulted first -- `next` being the node the module hands off to
/// next in THIS circuit's traversal order, the same "exit-port reader"
/// `FoundLoop` materialization resolves -- and `Some` REPLACES the raw
/// composite row with the returned per-exit-port override series for that one
/// hop; `None` (an ambiguous entry/exit port, a pathless module, ...) leaves
/// the raw composite row in place. Retention and materialization share the
/// SAME `override_for` closure (one `ltm_finding::ModuleOverrideCache` per
/// discovery call), so the two can never resolve a given hop to different
/// series.
///
/// Order within a step is the circuit's traversal order, matching the
/// FoundLoop scoring pipeline link for link, so a survivor's totals and its
/// materialized series agree bit for bit.
fn score_steps_with_overrides(
    rows: &[u32],
    graph: &ActivityGraph,
    lo: usize,
    is_module_node: &[bool],
    override_for: &mut ModuleOverrideFn,
    out: &mut [f64],
    nan_mask: &mut [bool],
) {
    debug_assert_eq!(out.len(), nan_mask.len());
    out.fill(1.0);
    let len = rows.len();
    for i in 0..len {
        let row = rows[i];
        // `to` is the target of `row`: by construction of an elementary
        // circuit's edge-row sequence, that is the SOURCE of the next row,
        // wrapping to row 0's source at the closing edge.
        let to = graph.edge_source(rows[(i + 1) % len]);
        let overridden = if is_module_node[to as usize] {
            let from = graph.edge_source(row);
            // The node `to` hands off to next along this circuit: the source
            // of the row after `to`'s own outbound row -- i.e. two rows
            // ahead of `row`, wrapping the same way.
            let next = graph.edge_source(rows[(i + 2) % len]);
            override_for(from, to, next)
        } else {
            None
        };
        let row_series: &[f64] = match &overridden {
            Some((series, _window)) => &series[lo..lo + out.len()],
            None => {
                let start = row as usize * graph.step_count + lo;
                &graph.series[start..start + out.len()]
            }
        };
        for (product, &value) in out.iter_mut().zip(row_series) {
            *product *= value;
        }
    }
    for (mask, product) in nan_mask.iter_mut().zip(out.iter()) {
        *mask = product.is_nan();
    }
}
