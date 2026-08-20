// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Post-simulation "loops that matter" discovery over recorded link scores.
//!
//! Runs as a post-processing step on simulation results that include link
//! score synthetic variables (generated with `ltm_discovery_mode` enabled).
//! Candidate loops are generated in one of two ways:
//!
//! - **Union-graph circuit enumeration** (the primary path; `ltm_finding_enum.rs`,
//!   design: docs/design-plans/2026-08-17-ltm-discovery-exact.md): because
//!   discovery runs after the simulation, the set of edges that ever carried a
//!   nonzero score is observable, and every scorable loop is an
//!   ever-simultaneously-active elementary cycle of that union graph, spanning
//!   at least two variables. Enumerating exactly those cycles (activity-bitset
//!   pruning) yields a provably complete candidate set whenever the enumeration
//!   budgets hold -- `DiscoveryResult::enumeration_complete` reports it.
//! - **Shortest-path fallback** (`ltm_finding_fallback.rs`): per saved step
//!   and per seed stock, a Dijkstra over that step's active edges recovers the
//!   cycles through the stock cheapest first. It runs when the enumeration
//!   cannot complete -- its budgets trip, or the caller's wall-clock deadline
//!   expires -- and its cost is `steps x stocks x E log V` with no cliff, so
//!   it is bounded before the work starts and interruptible between searches.
//!   A sample, not the universe: `enumeration_complete == false` says so.
//!
//! Either way, each candidate's per-step score is computed exactly (the
//! product of its links' recorded score series) and the same retention /
//! competitive-first ranking pipeline selects what is reported. The generator
//! decides only WHICH cycles are proposed, never what they are worth.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::common::{Canonical, Ident, Result};
use crate::datamodel;
use crate::db::LtmSyntheticVar;
use crate::ltm::{CausalGraph, CyclePartitions, Link, LinkPolarity, Loop, LoopPolarity};
use crate::ltm_post::NormGroup;
use crate::project::Project;
use crate::results::Results;

// Union-graph circuit enumeration: discovery's primary candidate generator
// (docs/design-plans/2026-08-17-ltm-discovery-exact.md). A sibling file
// mounted here purely for the per-file line cap.
#[path = "ltm_finding_enum.rs"]
mod enum_gen;
#[cfg(test)]
pub(crate) use enum_gen::EnumBudgetGuard;
use enum_gen::{ActivityGraph, enumerate_active_circuits, retain_circuits};

// Shortest-path fallback: the candidate generator that runs when the
// enumeration cannot complete within its budgets or the caller's deadline
// (docs/design-plans/2026-08-17-ltm-discovery-exact.md). A sibling file
// mounted here purely for the per-file line cap.
#[path = "ltm_finding_fallback.rs"]
mod fallback;
pub use fallback::{
    FallbackClosures, FallbackConfig, FallbackSeeds, FallbackTieBreak, FallbackWeight,
};

// --- Types ---

/// A parsed link score offset: ((from_variable, to_variable), offset_in_results).
type LinkOffset = ((Ident<Canonical>, Ident<Canonical>), usize);

/// HashMap for O(1) link offset lookup by (from, to) key.
type LinkOffsetMap = HashMap<(Ident<Canonical>, Ident<Canonical>), usize>;

/// Memoized per-exit-port module-input recompute, within one discovery call.
///
/// The key is everything `recompute_module_input_edge_series_for` reads that
/// varies by loop edge -- the module-input source, the module instance, and
/// the reader that identifies the exit port -- each stripped of any element
/// subscript, exactly as the recompute strips them before its own lookups. An
/// arrayed loop through a module therefore hits one entry per element rather
/// than re-enumerating the sub-model's pathway map each time. The value is
/// `Rc`-wrapped so [`ModuleOverrideCache::series`] can hand every caller --
/// retention scores a great many more circuits than materialization ever
/// built loops -- a cheap clone of the same allocation rather than a deep
/// copy, paired with the series' own active window (computed once here
/// alongside it; see `active_window_of`) so retention can bound a
/// module-traversing circuit's scoring range by it instead of the full
/// saved-step range.
type ModuleSeriesCache =
    HashMap<(Ident<Canonical>, Ident<Canonical>, Ident<Canonical>), ModuleOverrideEntry>;

/// One [`ModuleOverrideCache`] lookup's answer: the override series (shared
/// via `Rc`) and its own `[lo, hi)` active window, or `None` when no single
/// exit pathway resolves.
pub(crate) type ModuleOverrideEntry = Option<(Rc<Vec<f64>>, (usize, usize))>;

/// What [`ModuleOverrideCache::series`] answers: a resolved override, a
/// decline (the composite applies), or a memory refusal -- which is NEITHER of
/// the first two and must never be read as a decline: scoring the edge on its
/// composite where the override should apply is a wrong number, so a caller
/// that sees it abandons the phase (retention yields to the fallback; a
/// materialization drops the loop and reports the sample truncated).
pub(crate) enum OverrideLookup {
    Resolved(Rc<Vec<f64>>, (usize, usize)),
    Declined,
    OutOfMemory,
}

impl OverrideLookup {
    fn from_entry(entry: &ModuleOverrideEntry) -> Self {
        match entry {
            Some((series, window)) => OverrideLookup::Resolved(Rc::clone(series), *window),
            None => OverrideLookup::Declined,
        }
    }
}

/// Per-sub-model emitted LTM output-port set, keyed by the sub-model's
/// canonical name. The discovery-mode per-exit-port recompute (GH #698) uses
/// it to enumerate pathway indices against the SAME sorted port set the
/// sub-model emitted its `$⁚ltm⁚path⁚{port}⁚{idx}` vars against -- see
/// `recompute_module_input_edge_series` and `discover_loops_with_graph`.
pub type SubModelOutputPorts = HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>;

/// Per-variable metadata `parse_link_offsets` needs to spell the per-element
/// FROM-node of a Bare A2A link score in lockstep with the element graph.
///
/// A Bare A2A link score (`{from}→{to}`, both names un-subscripted in the
/// score name) is dimensioned over the TARGET's dims (see
/// `db::ltm::link_score_dimensions`). When `from`'s OWN declared dims are
/// FEWER than the score's dims -- a scalar feeder (`scale→growth`, GH #790),
/// a lower-dim arrayed feeder (`boost[Region]→growth[Region,Age]`), or a
/// positionally-mapped pair (`x[Region]→target[State]`, GH #527) --
/// subscripting both endpoints with the score's full element tuple invents a
/// from-node that names no real element-graph node (`scale[a]`,
/// `boost[nyc,young]`, `x[s1]`), so the discovery search graph's edge dangles
/// and every loop through `from` is silently undiscoverable (GH #754).
///
/// `declared_dims` maps each variable's canonical name to its DECLARED
/// dimensions (in declared order); `dim_ctx` carries the dimension-mapping
/// element correspondence. Together they let `expand_a2a_link_offsets` project
/// the score's element tuple onto `from`'s own dims through the SAME
/// `db::expand_same_element` diagonal/broadcast/mapped rule the element graph
/// uses, so the two surfaces spell the from-node identically. The
/// db-less `discover_loops` convenience path (variable-level graph, empty
/// `ltm_vars`) passes [`LinkExpansionContext::default`]: no A2A expansion runs
/// there, so the empty map is never consulted.
#[derive(Default, Clone)]
pub struct LinkExpansionContext {
    /// Canonical variable name -> declared dimensions, in declared order.
    /// Holds BOTH endpoints of every Bare A2A edge (the from-side projection
    /// reads the from-var's dims, the to-side offset map reads the to-var's).
    pub declared_dims: HashMap<Ident<Canonical>, Vec<crate::dimensions::Dimension>>,
    /// Dimension-mapping element correspondence for the mapped (#527) leg.
    pub dim_ctx: crate::dimensions::DimensionsContext,
}

// --- Constants (from the paper) ---

/// Maximum loops to retain after discovery (paper uses 200)
const MAX_LOOPS: usize = 200;

/// Minimum average relative contribution to keep a loop (paper uses 0.1%)
const MIN_CONTRIBUTION: f64 = 0.001;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`MAX_LOOPS`], scoped by an active
    /// [`MaxLoopsGuard`]. Lets a test exercise the global cap (and its
    /// partition-relative truncation order) with a tiny fixture instead of
    /// building 200+ loops to trip the production constant (per
    /// docs/dev/rust.md#test-time-budgets, the same override pattern as
    /// `db::ltm::AggLoopBudgetGuard` for GH #515).
    static MAX_LOOPS_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The loop cap for the current `rank_and_filter` call. Returns [`MAX_LOOPS`]
/// in production builds; in `#[cfg(test)]` builds an active [`MaxLoopsGuard`]
/// override takes precedence.
fn max_loops() -> usize {
    #[cfg(test)]
    {
        if let Some(n) = MAX_LOOPS_OVERRIDE.with(|c| c.get()) {
            return n;
        }
    }
    MAX_LOOPS
}

/// RAII guard (test-only) that overrides [`max_loops`] for the current thread
/// for the guard's lifetime, restoring the previous value on drop -- so a
/// panicking test does not leak the override to the next test reusing the
/// thread.
#[cfg(test)]
struct MaxLoopsGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl MaxLoopsGuard {
    fn new(cap: usize) -> Self {
        let prev = MAX_LOOPS_OVERRIDE.with(|c| c.replace(Some(cap)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for MaxLoopsGuard {
    fn drop(&mut self) {
        MAX_LOOPS_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// Memory discovery may commit across ALL of its phases -- the union graph's
/// rows and bitsets (scratch included), the enumerated circuits, the module
/// override cache's series and shared pathway slots, stitched loops on either
/// path, the fallback's kept paths, and the materialized `FoundLoop` series --
/// charged to one [`MemoryMeter`] so the bound is a single statement rather
/// than one constant per allocation site. A phase that cannot fit yields:
/// the activity build abandons, the enumeration reports incomplete, the
/// fallback truncates, and discovery reports what ran. World3 commits ~65 MB
/// (activity ~1.5 MB, ~150k circuits at ~6.3M rows plus bitsets ~35 MB, the
/// 2,979 survivors' series ~29 MB at 401 saved steps); C-LEARN under 10 MB.
pub(crate) const MAX_DISCOVERY_MEMORY_BYTES: usize = 768 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`MAX_DISCOVERY_MEMORY_BYTES`], scoped by an
    /// active [`MemoryBudgetGuard`], so tiny fixtures can trip every phase's
    /// memory arm.
    static MEMORY_BUDGET_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The effective discovery memory bound.
fn max_discovery_memory_bytes() -> usize {
    #[cfg(test)]
    {
        if let Some(b) = MEMORY_BUDGET_OVERRIDE.with(|c| c.get()) {
            return b;
        }
    }
    MAX_DISCOVERY_MEMORY_BYTES
}

/// RAII guard (test-only) overriding [`max_discovery_memory_bytes`] for the
/// current thread; restores the previous value on drop.
#[cfg(test)]
pub(crate) struct MemoryBudgetGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl MemoryBudgetGuard {
    pub(crate) fn new(bytes: usize) -> Self {
        let prev = MEMORY_BUDGET_OVERRIDE.with(|c| c.replace(Some(bytes)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for MemoryBudgetGuard {
    fn drop(&mut self) {
        MEMORY_BUDGET_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// The one memory bound every discovery phase charges its allocations to.
///
/// Interior mutability (`Cell`) so the meter can be shared by reference among
/// phases that are otherwise borrowing each other mutably (retention borrows
/// the override cache through a closure while the cache charges its own
/// series). `charge` never allocates; a phase asks before it allocates and
/// yields when refused. `release` credits memory a phase has dropped (the
/// enumeration's graph and candidates before the fallback runs), so the
/// fallback is measured against the same bound, not against what the
/// abandoned attempt had used.
pub(crate) struct MemoryMeter {
    used: std::cell::Cell<usize>,
    cap: usize,
}

impl MemoryMeter {
    pub(crate) fn new() -> Self {
        MemoryMeter {
            used: std::cell::Cell::new(0),
            cap: max_discovery_memory_bytes(),
        }
    }

    /// Commit `bytes` if they fit; `false` (and nothing committed) otherwise.
    #[must_use]
    pub(crate) fn charge(&self, bytes: usize) -> bool {
        let used = self.used.get();
        match used.checked_add(bytes) {
            Some(total) if total <= self.cap => {
                self.used.set(total);
                true
            }
            _ => false,
        }
    }

    /// Credit `bytes` a phase has dropped.
    pub(crate) fn release(&self, bytes: usize) {
        self.used.set(self.used.get().saturating_sub(bytes));
    }

    /// Bytes currently committed.
    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        self.used.get()
    }
}

/// Bytes one materialized `FoundLoop` over `node_count` nodes costs for a run
/// of `step_count` saved steps: its `(time, score)` series plus its
/// relative-score series, and per node the structural representations a
/// candidate path becomes on the way to the report -- the `Ident` path
/// materialization builds and the `Link` (two `Ident`s and a polarity) the
/// loop retains per edge. `Ident` is an interned `Arc`, so a clone is one
/// pointer, not a string copy; the per-node term is still what makes a sample
/// of many long cycles cost what it costs. Charged before materialization on
/// both paths so the report's own storage is inside the discovery memory
/// bound, not only the candidates'.
pub(super) fn materialized_loop_bytes(step_count: usize, node_count: usize) -> usize {
    let series = step_count * (std::mem::size_of::<(f64, f64)>() + std::mem::size_of::<f64>());
    let structure = node_count
        * (std::mem::size_of::<Ident<Canonical>>() + std::mem::size_of::<crate::ltm::Link>());
    std::mem::size_of::<FoundLoop>() + series + structure
}

/// Prefix for link score synthetic variables
const LINK_SCORE_PREFIX: &str = "$⁚ltm⁚link_score⁚";

/// Separator between from/to in link score variable names (U+2192 RIGHTWARDS ARROW)
const LTM_LINK_SEP: char = '→';

// --- Internal types ---

// --- Public types ---

/// A loop found by discovery, with its scores over time.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct FoundLoop {
    /// The loop structure (reuses existing Loop type from ltm.rs)
    pub loop_info: Loop,
    /// Loop score at each timestep: (time, signed_score)
    /// The signed score is the product of the signed link scores.
    pub scores: Vec<(f64, f64)>,
    /// Average |score| over the simulation (for ranking/filtering)
    pub avg_abs_score: f64,
    /// Signed *partition-relative* loop score at each timestep:
    /// `score[t] / Σ_{j in same cycle partition} |score_j[t]|`, sign preserved
    /// (the same normalization `ltm_post::compute_rel_loop_scores` applies to
    /// the pinned-loop path).  A value in `[-1, 1]` that, unlike the raw
    /// `scores`, IS comparable across partitions -- so it is the correct
    /// importance/dominance key (GH #543's ranking statistic, surfaced as a
    /// per-timestep series).  Filled by `rank_truncate_and_id` once the
    /// per-partition per-timestep denominators are known; empty until then and
    /// for the no-score-data path.  Length matches `scores` when populated.
    pub rel_scores: Vec<f64>,
    /// RESULT-SCOPED cycle-partition index into [`DiscoveryResult::partitions`],
    /// or `None` for a loop whose stocks resolve to no parent-level partition
    /// (a pure module-internal loop).  Indices are dense and assigned in
    /// first-appearance order over the final ranked loop list -- they identify
    /// partitions *within one discovery result only* and are NOT stable across
    /// runs or model edits (the underlying SCC numbering renumbers when stocks
    /// are added or renamed).  Consumers that need a durable identity should
    /// key on the partition's stock-name set instead.  Filled by
    /// `attach_partition_metadata` at the end of ranking.
    pub partition: Option<usize>,
    /// Polarity-confidence ratio in `[0.0, 1.0]` for [`Self::loop_info`]'s
    /// polarity (GH #495).
    ///
    /// When the loop has runtime score data this is the
    /// `|r - |b|| / (r + |b|)` ratio that
    /// [`crate::ltm::LoopPolarity::from_runtime_scores`] returns alongside the
    /// polarity, so a mixed-sign `MostlyReinforcing`/`MostlyBalancing` loop
    /// reports a value strictly below 1.0 that distinguishes it from a clean
    /// `Reinforcing`/`Balancing` (confidence exactly 1.0).  For a loop with no
    /// valid runtime scores the polarity falls back to the structural
    /// negative-link count, and this confidence mirrors the structural
    /// convention `db::analysis` uses (1.0 when the structural polarity is
    /// determined, 0.0 when it is `Undetermined`) so the two surfaces agree.
    pub polarity_confidence: f64,
}

/// One cycle partition referenced by a discovery result's loops.
///
/// A cycle partition is a group of stocks connected by feedback (a strongly
/// connected component of the stock-to-stock reachability graph; ref section
/// 8).  Relative loop scores are normalized *within* a partition, so a loop's
/// importance is only comparable to its partition-mates' -- this metadata lets
/// callers group, filter, or present loops partition-by-partition (e.g. lead
/// with the model's giant component).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredPartition {
    /// The partition's stock names (element-level for arrayed models, e.g.
    /// `population[nyc]`), sorted lexicographically.
    pub stocks: Vec<String>,
    /// Number of loops in the RETURNED loop list that belong to this
    /// partition (post-filter, post-cap -- the count a caller can verify
    /// against the `loops` it received, not the discovered-but-dropped total).
    pub loop_count: usize,
}

/// The outcome of a loop-discovery run.
///
/// `truncated` is `true` when a caller-supplied time budget elapsed before the
/// fallback sweep finished. The budget is split (`ENUM_BUDGET_FRACTION`), so a
/// deadline that expires during the enumeration or its retention pass hands
/// the remainder to the fallback rather than ending discovery: a `truncated`
/// result therefore always reflects the FALLBACK stopping early. `loops` then
/// covers only the saved steps it processed (a loop dominant only in an
/// unprocessed step will be absent, while the per-step importance series of
/// the loops that *were* found is complete, since each loop's score is
/// recomputed across all steps once its path is known). Discovery on large
/// models can be infeasibly slow (GH #647), so the budget lets callers bound
/// wall-clock time and report partial results rather than hang. See
/// `enumeration_complete` below for the completeness statement of an
/// un-truncated run.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct DiscoveryResult {
    /// Loops discovered (ranked, filtered, and ID-assigned).
    pub loops: Vec<FoundLoop>,
    /// The cycle partitions referenced by `loops`, indexed by each loop's
    /// `FoundLoop::partition`.  Dense, in first-appearance order over the
    /// ranked loop list; result-scoped (see `FoundLoop::partition`).
    pub partitions: Vec<DiscoveredPartition>,
    /// Whether the time budget elapsed before discovery finished.
    pub truncated: bool,
    /// Whether cross-element-through-aggregate loop recovery (GH #696) hit its
    /// loop-count budget (`db::cross_agg_loop_budget`) before stitching every
    /// disjoint-petal subset -- so some cross-agg reducer loops are absent.
    /// The exhaustive-mode analogue is `LtmVariablesResult::agg_recovery_truncated`;
    /// this is the discovery-mode signal that the reported loop list is
    /// *possibly incomplete* for the same structural reason, distinct from the
    /// time-`truncated` flag above.
    pub agg_recovery_truncated: bool,
    /// Whether candidate generation was the union-graph circuit enumeration
    /// AND it ran to completion. When `true`, the candidate set is provably
    /// the full universe of ever-simultaneously-active elementary cycles of
    /// the recorded link-score series (at saved-step resolution), so the
    /// reported loops are exactly the retention/ranking pipeline's selection
    /// from ALL scorable loops -- discovery is exact, not heuristic -- with
    /// one further condition: cross-aggregate reducer loops are recovered by
    /// stitching under their own budget, so the report is exact for that
    /// class only while `agg_recovery_truncated` is also `false`. "Exact"
    /// therefore reads `enumeration_complete && !agg_recovery_truncated`.
    ///
    /// When `false`, the shortest-path fallback generated the candidates: an
    /// explicit SAMPLE of the loop universe (the minimum-weight cycle through
    /// each seed and each edge on it, per saved step). The two `false` causes
    /// -- a budget trip and a deadline expiry -- are deliberately not
    /// distinguished: both leave the report equally a sample.
    pub enumeration_complete: bool,
    /// How many candidate loops passed the `MIN_CONTRIBUTION` retention
    /// filter, BEFORE the `MAX_LOOPS` cap truncated the report.
    ///
    /// Equal to `loops.len()` whenever the cap did not bind; above it when it
    /// did, which is the only way a caller can tell "this model has N loops
    /// worth reporting and you are seeing the top `MAX_LOOPS` of them" from
    /// "this model has `loops.len()` loops worth reporting". Counted on both
    /// generator paths -- on the fallback path the population it filters is
    /// the sample, not the universe.
    pub retained_loops: usize,
    /// On the enumeration path, the size of the candidate universe: the
    /// number of DISTINCT loops whose mass the partition denominators sum --
    /// the enumerated ever-simultaneously-active elementary cycles, minus the
    /// non-representative twins retention's own trimmed-key dedup drops
    /// before they ever bank mass (two enumerated circuits, e.g. a direct
    /// reference and its hoisted-reducer twin, that trim to the identical
    /// *reported* loop, AC4.3), plus the cross-agg loops stitched from
    /// disjoint petals (GH #696, which are combinations of enumerated
    /// circuits rather than circuits of the union graph in their own right).
    /// This is the population every retention denominator and
    /// competing-vs-solo decision is measured against.
    ///
    /// `Some` exactly when `enumeration_complete` -- a fallback sample and an
    /// abandoned (budget- or deadline-tripped) enumeration alike have no
    /// universe to report. An enumeration that never had to run because the
    /// model carries no links reports an empty one (`Some(0)`), and so does
    /// one that ran to completion and simply found no scorable cycle at all
    /// (a model with no stocks and no scorable stockless cycle, among other
    /// shapes).
    pub universe_loops: Option<usize>,
    /// On the fallback path, the number of distinct elementary cycles the
    /// shortest-path sweep proposed (`fallback::FallbackOutcome::paths.len()`),
    /// after dedup but before retention or the cap, itself bounded at
    /// `fallback::MAX_FALLBACK_PATHS`.
    ///
    /// `Some` exactly when the fallback ran -- the mirror of `universe_loops`
    /// being `Some` exactly when the enumeration ran, so a caller never has to
    /// ask `enumeration_complete` twice to know which candidate-count field to
    /// read. Reported by `examples/ltm_discovery_bench` and
    /// `examples/ltm_fallback_eval` alongside the enumeration's `universe_loops`
    /// so the two generators' candidate VOLUMES are directly comparable, not
    /// only their final reported counts (which retention and the cap both
    /// shrink).
    pub fallback_candidates: Option<usize>,
}

/// Parse link score variable names from results offsets, expanding A2A
/// link scores into per-element edges.
///
/// For scalar link scores (size 1), produces one `LinkOffset` per variable.
/// For A2A link scores (size N), produces N `LinkOffset` entries -- one per
/// dimension element -- where each element-level edge maps
/// `from[elem]->to[elem]` to `base_offset + element_index`.
///
/// Naming patterns handled (see `ltm_augment::link_score_var_name`):
/// 1. Bare A2A: `from→to` with non-empty dims → expands to N
///    `(from[d], to[d])` entries (Bare path).
/// 2. Bare scalar: `from→to` with empty dims → single `(from, to)`.
/// 3. FixedIndex A2A: `from[elem]→to` with non-empty dims → expands to
///    N entries `(from[elem], to[d])` over the *target* dimension. The
///    source carries a fixed element subscript; only the target varies.
/// 4. FixedIndex / cross-dimensional / agg-hop scalar: `from[elem]→to`
///    (or `to[elem]`, or an `$⁚ltm⁚agg⁚{n}` on either end) with empty
///    dims → single pass-through entry. The element rides in the name.
///
/// When `ltm_vars` is empty (e.g. in the non-salsa convenience path),
/// all link scores are treated as scalar (no expansion).
///
/// Shape priority rank for collapsing duplicate `(from, to)` keys.
/// Lower rank wins, mirroring the Bare-beats-FixedIndex priority used by
/// `ltm_augment::resolve_link_score_name_for_loop`.
///
/// This resolves the collision: Bare A2A vs. FixedIndex A2A at the
/// *expanded* per-element level: e.g., `pop→share` and `pop[nyc]→share`
/// both expand to `(pop[nyc], share[nyc])`. The FixedIndex source carries
/// its own bracketed element, but when the target is also A2A and the
/// FixedIndex element matches the target element, the Bare A2A diagonal
/// aliases with one FixedIndex broadcast slot. Bare wins.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum ShapeRank {
    Bare = 0,
    FixedIndex = 1,
}

/// One element-level causal edge and the results slot holding its link score:
/// `((from_node, to_node), offset)`, where a node carries its element subscript
/// (`pop[nyc]`) exactly as `model_element_causal_edges` names it.
pub type LinkScoreOffset = ((Ident<Canonical>, Ident<Canonical>), usize);

/// The element-level `(from, to) -> results offset` edge set that discovery
/// searches over.
///
/// This is `parse_link_offsets` -- the expansion that turns emitted link-score
/// VARIABLES into per-element EDGES, including the from-side projection
/// (`expand_a2a_link_offsets`) that keeps the search graph's node names matching
/// `model_element_causal_edges` node-for-node.
///
/// Exposed for analysis and diagnostic tooling that needs to reason about the
/// edges discovery actually consumes -- notably
/// `examples/ltm_edge_coverage.rs`, which reports how many causal edges carry a
/// live score. Such a tool must not re-derive this mapping: an A2A score
/// occupies one slot per element and the projection is subtle enough that a
/// second implementation would drift, and a measuring instrument that disagrees
/// with the thing it measures is worse than no instrument. Build `expansion`
/// with [`crate::analysis::build_link_expansion_context`] so the inputs match
/// the production path too.
pub fn link_score_offsets(
    results: &Results,
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
    expansion: &LinkExpansionContext,
) -> Vec<LinkScoreOffset> {
    parse_link_offsets(results, ltm_vars, dims, expansion)
}

/// The cross-aggregate stitching limits discovery applies: `(max petals per
/// aggregate node, stitched-loop budget)`. Exposed for the audit instrument so
/// an independent re-enumeration stitches under the SAME limits production
/// does instead of re-stating them.
pub fn cross_agg_stitching_limits() -> (usize, usize) {
    (
        crate::db::MAX_AGG_PETALS,
        crate::db::cross_agg_loop_budget(),
    )
}

/// The per-edge series discovery's candidate generators read for ACTIVITY --
/// the recorded link score, NaN-shadow-repaired for a module-input edge by
/// [`IndexedEdge::value_at`] (a module composite that is NaN or 0 where one of
/// its pathway series is active reads as that pathway's max-abs value) -- for
/// every edge in [`link_score_offsets`]' order. Identical to the recorded series
/// except on module-input edges, where an audit that derived activity from the
/// raw composite alone would drop cycles production enumerates. Exposed for the
/// audit instrument (`examples/ltm_search_graph_dump.rs`) for the same reason the
/// edge set is: it must consume what discovery consumes, never re-derive it.
pub fn link_activity_series(
    results: &Results,
    causal_graph: &CausalGraph,
    stocks: &[Ident<Canonical>],
    link_offsets: &[LinkScoreOffset],
    sub_model_output_ports: &SubModelOutputPorts,
) -> Vec<Vec<f64>> {
    let mut search = IndexedSearch::build(link_offsets, stocks);
    let meter = MemoryMeter::new();
    let mut cache = ModuleOverrideCache::new(
        causal_graph,
        results,
        sub_model_output_ports,
        results.step_count,
        &meter,
    );
    // Unbudgeted: the attach pass reads no clock, and a fresh meter at the
    // full bound does not refuse a slot list.
    let attached = search.attach_module_pathways(
        &mut |from, to| cache.pathway_offsets(from, to),
        None,
        &mut SystemClock,
    );
    debug_assert_eq!(attached, AttachOutcome::Complete);
    // `IndexedSearch::build` keeps each node's edges in `link_offsets` order
    // and interns nodes in first-seen order, so walking the offsets and
    // looking each edge up by (from, to) reproduces the input order exactly.
    let id_of: HashMap<&Ident<Canonical>, usize> = search
        .idents
        .iter()
        .enumerate()
        .map(|(i, ident)| (ident, i))
        .collect();
    link_offsets
        .iter()
        .map(|((from, to), offset)| {
            let from_id = id_of[from];
            let to_id = id_of[to];
            let edge = search.adj[from_id]
                .iter()
                .find(|e| e.to as usize == to_id && e.offset == *offset)
                .expect("every offset is an edge");
            (0..results.step_count)
                .map(|s| edge.value_at(results, s * results.step_size))
                .collect()
        })
        .collect()
}

fn parse_link_offsets(
    results: &Results,
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
    expansion: &LinkExpansionContext,
) -> Vec<LinkOffset> {
    // Build a lookup from canonical link score name -> LtmSyntheticVar
    // for quick dimension lookup during expansion.
    let ltm_var_map: HashMap<String, &LtmSyntheticVar> = ltm_vars
        .iter()
        .filter(|v| v.name.contains(LINK_SCORE_PREFIX))
        .map(|v| (crate::common::canonicalize(&v.name).into_owned(), v))
        .collect();

    // Phase 1: parse every variable into one or more `(LinkOffset,
    // ShapeRank)` entries. The rank records whether the offset came from
    // a Bare or a FixedIndex link score so phase 2 can dedupe
    // deterministically when a Bare A2A diagonal aliases with one
    // FixedIndex broadcast slot.
    let mut tagged: Vec<(LinkOffset, ShapeRank)> = Vec::new();

    for (var_name, &offset) in &results.offsets {
        let name_str = var_name.as_str();
        let Some(suffix) = name_str.strip_prefix(LINK_SCORE_PREFIX) else {
            continue;
        };
        let Some((from_str, to_str)) = suffix.split_once(LTM_LINK_SEP) else {
            continue;
        };

        // A bracketed `from` marks a per-source-element FixedIndex (or
        // cross-dimensional) link score; everything else is Bare-ranked
        // (a per-target-element `to[elem]` score still rides its element
        // in the name and dedupes against nothing).
        let rank = if from_str.contains('[') {
            ShapeRank::FixedIndex
        } else {
            ShapeRank::Bare
        };

        // Look up the LtmSyntheticVar for this link score to get its
        // dimensions.
        let var_dims = ltm_var_map
            .get(name_str)
            .map(|v| &v.dimensions[..])
            .unwrap_or(&[]);

        let mut entries: Vec<LinkOffset> = Vec::new();

        // FixedIndex A2A: source carries `[elem]` and the link score
        // has dimensions, so each slot represents the edge for
        // `(from[elem], to[d])` at element `d`. Only the target side
        // expands.
        if from_str.contains('[') && !var_dims.is_empty() {
            expand_fixed_from_a2a_link_offsets(
                from_str,
                to_str,
                offset,
                var_dims,
                dims,
                &mut entries,
            );
        } else if from_str.contains('[') || to_str.contains('[') {
            // Cross-dimensional / FixedIndex scalar pass-through: the
            // name is already element-level on at least one side, and
            // there is no further per-element expansion to do.
            entries.push(((Ident::new(from_str), Ident::new(to_str)), offset));
        } else if var_dims.is_empty() {
            // Scalar link score: one entry at the base offset.
            entries.push(((Ident::new(from_str), Ident::new(to_str)), offset));
        } else {
            // Bare A2A link score: expand to N element-level edges. The
            // TARGET side is always subscripted per element; the SOURCE side
            // is projected onto its OWN declared dims (bare for a scalar
            // feeder, the diagonal/broadcast for a lower-dim or mapped
            // feeder) so the from-node names match the element graph (GH
            // #754).
            expand_a2a_link_offsets(
                from_str,
                to_str,
                offset,
                var_dims,
                dims,
                expansion,
                &mut entries,
            );
        }

        for entry in entries {
            tagged.push((entry, rank));
        }
    }

    // Phase 2: dedupe by (from, to) key. When two emitted variants
    // collapse onto the same expanded per-element key, keep the lowest
    // `(rank, offset)` entry. The one collision case: Bare A2A vs.
    // FixedIndex A2A -- `pop→share` and `pop[nyc]→share` both produce the
    // element key `(pop[nyc], share[nyc])` when the FixedIndex element
    // matches the diagonal target element.
    //
    // Without this the union graph would carry parallel edges and
    // `discover_loops_with_graph::link_offset_map` would pick one
    // nondeterministically (HashMap iteration order over
    // `results.offsets` chooses the survivor). Bare wins, matching the
    // priority used by `ltm_augment::resolve_link_score_name_for_loop` so
    // loop_score, pathway, and discovery all reference the same variant
    // for a given edge.
    //
    // Same-rank ties (e.g., two Bare A2A entries that somehow produce
    // the same expanded key, which shouldn't happen but defends
    // against future emitter changes) are broken by smaller offset.
    let mut by_key: HashMap<(Ident<Canonical>, Ident<Canonical>), (ShapeRank, usize)> =
        HashMap::with_capacity(tagged.len());
    for ((key, offset), rank) in tagged {
        by_key
            .entry(key)
            .and_modify(|existing| {
                if (rank, offset) < *existing {
                    *existing = (rank, offset);
                }
            })
            .or_insert((rank, offset));
    }

    // Sort the result so the output is deterministic across runs (the
    // HashMap iteration above is not). Node ids, per-node adjacency order,
    // and hence both generators' emission order all follow from this order,
    // so it is what makes discovery's output content-pure.
    let mut link_offsets: Vec<LinkOffset> = by_key
        .into_iter()
        .map(|(key, (_rank, offset))| (key, offset))
        .collect();
    link_offsets.sort_by(|a, b| {
        a.0.0
            .as_str()
            .cmp(b.0.0.as_str())
            .then_with(|| a.0.1.as_str().cmp(b.0.1.as_str()))
            .then_with(|| a.1.cmp(&b.1))
    });
    link_offsets
}

/// Expand a Bare A2A link score into per-element `LinkOffset` entries.
///
/// The score's `var_dims` are the TARGET's dims (see
/// `db::ltm::link_score_dimensions`), so each result slot `base + idx` is the
/// score for the edge feeding the `idx`-th target element (row-major). The
/// TARGET node is always that element (`to_var[<tuple>]`); the SOURCE node is
/// the PROJECTION of that target element onto `from`'s OWN declared dims:
///
/// - a SCALAR feeder (`scale → growth`, GH #790) emits the bare `from` node,
///   shared by every target-element offset;
/// - a SAME-DIM feeder (`birth_rate[Region] → births[Region]`) emits the
///   diagonal `from[e] → to[e]` (the original behavior);
/// - a LOWER-DIM arrayed feeder (`boost[Region] → growth[Region,Age]`) emits
///   `boost[r] → growth[r,a]`, the bare-region from-node broadcast over the
///   unshared `Age`;
/// - a positionally-MAPPED feeder (`x[Region] → target[State]`, GH #527)
///   emits the mapping's diagonal `x[mapped(s)] → target[s]`.
///
/// The projection runs through the SAME `db::expand_same_element` rule the
/// element graph emits for the corresponding `Bare` reference, so the
/// discovery search graph's from-node names match `model_element_causal_edges`
/// node-for-node (GH #754). Before this fix the source was subscripted with
/// the score's FULL tuple unconditionally, minting phantom from-nodes
/// (`scale[a]`, `boost[r,a]`, `x[s]`) that named no real element node, so
/// every loop through such a feeder dangled and was silently undiscoverable.
///
/// The MAPPED leg covers every pair whose two reference spellings AGREE, which
/// since GH #997 includes an explicit element map at unequal cardinality
/// (C-LEARN's many-to-one). It cannot cover a pair whose spellings DISAGREE,
/// and does not have to: `expand_same_element` emits the UNION of both
/// diagonals there, which would put two from-nodes on the one slot this
/// function assigns per target element -- so `link_score_dimensions` denies
/// such a pair the arrayed retarget (`db::analysis::mapped_pair_projects_uniquely`)
/// and it takes the GH #758 loud skip instead, leaving no dimensioned score for
/// `parse_link_offsets` to expand. That is the whole reason the gate is
/// STRICTER than the element graph's rule; the lockstep with
/// `expand_same_element` is what makes the from-node names match either way.
fn expand_a2a_link_offsets(
    from_var: &str,
    to_var: &str,
    base_offset: usize,
    var_dims: &[String],
    dims: &[datamodel::Dimension],
    expansion: &LinkExpansionContext,
    link_offsets: &mut Vec<LinkOffset>,
) {
    let Some(tuples) = resolve_dim_element_tuples(var_dims, dims) else {
        // Dimension resolution failed; fall back to a single scalar
        // entry so the link is at least registered (consistent with the
        // pre-Phase-3 behavior on misconfigured dims).
        let from = Ident::new(from_var);
        let to = Ident::new(to_var);
        link_offsets.push(((from, to), base_offset));
        return;
    };

    // The target-element node -> its result offset, by row-major position in
    // the score's (== target's) dims. This is the layout slot the runtime
    // wrote the score into, regardless of how the source projects onto it.
    let mut to_node_offset: HashMap<Ident<Canonical>, usize> = HashMap::with_capacity(tuples.len());
    for (idx, elems) in tuples.iter().enumerate() {
        let to_node = Ident::new(&format!("{to_var}[{}]", subscript_from_elements(elems)));
        to_node_offset.insert(to_node, base_offset + idx);
    }

    let from_ident = Ident::<Canonical>::new(from_var);
    let to_ident = Ident::<Canonical>::new(to_var);
    let from_dims = expansion.declared_dims.get(&from_ident);
    let to_dims = expansion.declared_dims.get(&to_ident);

    match (from_dims, to_dims) {
        // Scalar feeder: the bare `from` node feeds every target-element slot.
        // Mirrors `emit_edges_for_reference`'s `from_is_scalar` short-circuit
        // (a scalar source has no subscript form).
        (Some(fd), _) if fd.is_empty() => {
            for (to_node, offset) in &to_node_offset {
                link_offsets.push(((from_ident.clone(), to_node.clone()), *offset));
            }
        }
        // Arrayed feeder with both endpoints' declared dims known: project the
        // source onto its OWN dims via the shared element-graph rule, then
        // attach each emitted (from_node -> to_node) edge to the target
        // element's offset.
        (Some(fd), Some(td)) => {
            let mut element_edges: HashMap<String, std::collections::BTreeSet<String>> =
                HashMap::new();
            crate::db::expand_same_element(
                from_var,
                to_var,
                fd,
                td,
                &expansion.dim_ctx,
                &mut element_edges,
            );
            for (from_node, to_nodes) in element_edges {
                let from = Ident::<Canonical>::new(&from_node);
                for to_node in to_nodes {
                    let to = Ident::<Canonical>::new(&to_node);
                    if let Some(&offset) = to_node_offset.get(&to) {
                        link_offsets.push(((from.clone(), to.clone()), offset));
                    }
                    // A to_node with no offset can't arise: `expand_same_element`
                    // only emits target nodes over the same (target) dims the
                    // offset map enumerates. Dropping it (rather than minting a
                    // dangling edge) is the conservative choice if it ever does.
                }
            }
        }
        // Declared dims unavailable for an endpoint (no production Bare A2A
        // score has an unknown variable -- the map covers every model
        // variable; this guards the db-less convenience path and mid-edit
        // metadata gaps): preserve the historical both-sides diagonal so the
        // absent-metadata case is byte-identical to the pre-#754 behavior
        // rather than risking a node-name mismatch that would error the
        // discovery offset lookup.
        _ => {
            for (idx, elems) in tuples.iter().enumerate() {
                let subscript = subscript_from_elements(elems);
                let from = Ident::new(&format!("{from_var}[{subscript}]"));
                let to = Ident::new(&format!("{to_var}[{subscript}]"));
                link_offsets.push(((from, to), base_offset + idx));
            }
        }
    }
}

/// Expand a FixedIndex A2A link score into per-element `LinkOffset`
/// entries. Used when the source side is a fixed `from[elem]` reference
/// and the target side is array-valued, so each result slot is the link
/// score for the edge `(from[elem], to[d])` at target element `d`.
///
/// The from-name (`from[elem]`) is reused unchanged for every slot;
/// only the to-name receives the per-element subscript. The slot order
/// follows the same row-major cartesian-product convention used for
/// Bare A2A expansion to stay aligned with how the VM lays out the
/// underlying array.
fn expand_fixed_from_a2a_link_offsets(
    from_with_index: &str,
    to_var: &str,
    base_offset: usize,
    var_dims: &[String],
    dims: &[datamodel::Dimension],
    link_offsets: &mut Vec<LinkOffset>,
) {
    let Some(tuples) = resolve_dim_element_tuples(var_dims, dims) else {
        // Dimension resolution failed; preserve the source-side
        // subscript and emit a single pass-through entry. Without
        // expansion the downstream graph still has the FixedIndex edge
        // available, even if not per-element.
        let from = Ident::new(from_with_index);
        let to = Ident::new(to_var);
        link_offsets.push(((from, to), base_offset));
        return;
    };

    for (idx, elems) in tuples.iter().enumerate() {
        let subscript = subscript_from_elements(elems);
        let from = Ident::new(from_with_index);
        let to = Ident::new(&format!("{to_var}[{subscript}]"));
        link_offsets.push(((from, to), base_offset + idx));
    }
}

/// Resolve a list of dimension names into the cartesian product of
/// their element names (row-major). Returns `None` if any dimension is
/// missing from `dims`; callers fall back to a non-expanded entry in
/// that case.
fn resolve_dim_element_tuples(
    var_dims: &[String],
    dims: &[datamodel::Dimension],
) -> Option<Vec<Vec<String>>> {
    let dim_elements: Vec<Vec<String>> = var_dims
        .iter()
        .filter_map(|dim_name| {
            let canonical_dim_name = crate::common::canonicalize(dim_name);
            dims.iter()
                .find(|d| {
                    crate::common::canonicalize(d.name()).as_ref() == canonical_dim_name.as_ref()
                })
                .map(datamodel_dim_element_names)
        })
        .collect();

    if dim_elements.len() != var_dims.len() {
        return None;
    }

    // Cartesian product, row-major: the first dimension cycles slowest.
    let mut tuples: Vec<Vec<String>> = vec![vec![]];
    for elements in &dim_elements {
        let mut new_tuples = Vec::with_capacity(tuples.len() * elements.len());
        for existing in &tuples {
            for elem in elements {
                let mut extended = existing.clone();
                extended.push(elem.clone());
                new_tuples.push(extended);
            }
        }
        tuples = new_tuples;
    }
    Some(tuples)
}

/// Render a list of element names as a subscript body (no surrounding
/// brackets). Single-dimension subscripts are emitted bare (`nyc`);
/// multi-dimension subscripts are comma-joined (`nyc,q1`).
fn subscript_from_elements(elems: &[String]) -> String {
    if elems.len() == 1 {
        elems[0].clone()
    } else {
        elems.join(",")
    }
}

/// Get element names from a datamodel::Dimension, canonicalized for use
/// in element-level identifiers. Named dimensions return their element
/// names lowercased; indexed dimensions return "1", "2", etc. (1-based,
/// matching the engine's subscript formatting in `dimensions.rs`).
fn datamodel_dim_element_names(dim: &datamodel::Dimension) -> Vec<String> {
    match &dim.elements {
        datamodel::DimensionElements::Named(names) => names
            .iter()
            .map(|n| crate::common::canonicalize(n).into_owned())
            .collect(),
        datamodel::DimensionElements::Indexed(size) => (1..=*size).map(|i| i.to_string()).collect(),
    }
}

/// Look up the main model deterministically by its canonical name "main".
///
/// Returns `None` if no model named "main" exists or if it is implicit.
/// We intentionally avoid falling back to arbitrary HashMap iteration
/// (which is nondeterministic) -- all well-formed projects have a "main" model.
fn find_main_model(project: &Project) -> Option<&std::sync::Arc<crate::model::ModelStage1>> {
    project
        .models
        .get(&*crate::common::canonicalize("main"))
        .filter(|m| !m.implicit)
}

/// Identify stock variables from the project's main model.
fn get_stock_variables(project: &Project) -> Vec<Ident<Canonical>> {
    let mut stocks = Vec::new();

    let main_model = match find_main_model(project) {
        Some(model) => model,
        None => return stocks,
    };

    for (var_name, var) in &main_model.variables {
        if matches!(var, crate::variable::Variable::Stock { .. }) {
            stocks.push(var_name.clone());
        }
    }

    // Sort for deterministic ordering
    stocks.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    stocks
}

/// Run loop discovery on simulation results.
///
/// Reads link score values from `results` (computed during simulation via
/// LTM synthetic variables), then generates and scores loop candidates over
/// the recorded series.
///
/// The simulation must have been compiled with `ltm_discovery_mode` enabled
/// so that link score variables exist for all causal links.
///
/// This convenience function builds the causal graph from the `Project` and
/// does not have access to LTM synthetic variable metadata or project
/// dimensions, so A2A link scores are treated as scalar (no element-level
/// expansion). For full element-level discovery, use
/// `discover_loops_with_graph` with explicit `ltm_vars` and `dims`.
pub fn discover_loops(results: &Results, project: &Project) -> Result<Vec<FoundLoop>> {
    let stocks = get_stock_variables(project);
    let main_model = find_main_model(project).ok_or_else(|| crate::common::Error {
        kind: crate::common::ErrorKind::Model,
        code: crate::common::ErrorCode::NotSimulatable,
        details: Some("No non-implicit model found for loop discovery".to_string()),
    })?;
    let causal_graph = CausalGraph::from_model(main_model, project)?;
    // The per-exit-port recompute (GH #698) needs each sub-model's emitted
    // output-port set. The db-backed `analyze_model` path reads it from the
    // emission query directly; this convenience path has no db, so it
    // reconstructs the set with the SAME project-wide semantics emission uses
    // (union of `{instance}·{port}` reads over ALL project models + the stdlib
    // `output` short-circuit -- see `project_sub_model_output_ports`).
    let sub_model_ports = project_sub_model_output_ports(project);
    // The convenience path is unbudgeted: it builds the graph from a `Project`
    // and is used by small-model callers that never hit the GH #647 slowness.
    // It passes empty `ltm_vars`/`dims`, so no A2A expansion runs and the empty
    // `LinkExpansionContext` is never consulted.
    Ok(discover_loops_with_graph(
        results,
        &causal_graph,
        &stocks,
        &[],
        &[],
        &LinkExpansionContext::default(),
        &sub_model_ports,
        None,
    )?
    .loops)
}

/// Reconstruct each sub-model's emitted LTM output-port set from a compiled
/// `Project`, mirroring the emission-side `db::ltm::find_model_output_ports`
/// project-wide semantics for the db-less `discover_loops` convenience path.
///
/// Emission scans reads across ALL project models (not just the analyzed one)
/// and unions the `{instance}·{port}` ports per sub-model, sorted; a stdlib
/// sub-model short-circuits to exactly `["output"]`. The recompute must use the
/// IDENTICAL set/order to land on the sub-model's emitted `$⁚ltm⁚path` indices,
/// so this reproduces that decision rather than scanning the analyzed model
/// alone (the GH #698 / PR #705 r3353097150 cross-model index-shift bug). The
/// db-backed `analyze_model` path instead reads `db::ltm::sub_model_output_ports`
/// directly -- the one authoritative emission decision; this is its db-less
/// twin, kept in lockstep by the shared "project-wide union + stdlib output"
/// rule.
fn project_sub_model_output_ports(project: &Project) -> SubModelOutputPorts {
    use crate::variable::Variable;

    let mut ports: SubModelOutputPorts = HashMap::new();
    for model in project.models.values() {
        // Instance name -> sub-model name, for instances declared in THIS
        // model (an `instance·port` read only resolves to a same-model
        // instance).
        let instance_sub_model: HashMap<&Ident<Canonical>, &Ident<Canonical>> = model
            .variables
            .iter()
            .filter_map(|(name, var)| match var {
                Variable::Module { model_name, .. } => Some((name, model_name)),
                _ => None,
            })
            .collect();
        if instance_sub_model.is_empty() {
            continue;
        }

        let mut note_read = |dep: &str| {
            let Some((module_part, port)) = dep.split_once('\u{00B7}') else {
                return;
            };
            if port.starts_with('$') {
                return;
            }
            if let Some(sub_model) = instance_sub_model.get(&Ident::<Canonical>::new(module_part)) {
                ports
                    .entry((*sub_model).clone())
                    .or_default()
                    .push(Ident::new(port));
            }
        };

        for var in model.variables.values() {
            // A module reads upstream module outputs through its input wiring
            // (`mod_b`'s `ModuleInput.src == mod_a·pos`); a module has no
            // equation AST, so its reads come from `inputs`. Non-module reads
            // come from the equation AST. This mirrors `find_model_output_ports`
            // scanning `variable_direct_dependencies` (which includes input srcs).
            if let Variable::Module { inputs, .. } = var {
                for inp in inputs {
                    note_read(inp.src.as_str());
                }
                continue;
            }
            let Some(ast) = var.ast() else { continue };
            for dep in crate::variable::identifier_set(ast, &[], None) {
                note_read(dep.as_str());
            }
        }
    }

    // Stdlib sub-models are always read through the `output` convention
    // regardless of which internal ports a parent happens to reference, and a
    // stdlib sub-model emits its pathway vars against exactly `["output"]`.
    // Apply the same short-circuit `db::ltm::sub_model_output_ports` takes, then
    // dedup + sort each set to the emission order.
    for (sub_model, port_list) in ports.iter_mut() {
        if sub_model.as_str().starts_with("stdlib\u{205A}") {
            *port_list = vec![Ident::new("output")];
            continue;
        }
        port_list.sort();
        port_list.dedup();
    }
    ports
}

/// Collapse synthetic aggregate nodes out of a discovered loop's link chain.
///
/// Phase 5 of the cross-element aggregate work reroutes inlined array
/// reducers (`SUM(pop[*])`, `MEAN(...)`) through synthetic auxiliaries
/// named `$⁚ltm⁚agg⁚{n}`. The loop *score* equation still references the
/// un-trimmed per-element path (`pop[d] -> agg`, `agg -> share[e]`), but the
/// loop we *report* should not expose the synthetic node: a chain
/// `[X -> agg, agg -> Y]` collapses to a single edge `[X -> Y]` whose
/// polarity is the product of the two (AC4.2).
///
/// Only nodes whose name carries the synthetic agg prefix are trimmed --
/// whole-RHS-scalar reducers (`total_population = SUM(population[*])`) are
/// real, variable-backed nodes and stay in the reported loop.
///
/// Returns `None` if the loop consists entirely of synthetic agg nodes (a
/// degenerate cycle with nothing left after trimming) -- such a loop should
/// be dropped from the report.
fn trim_synthetic_aggs_from_loop_links(links: &[Link]) -> Option<Vec<Link>> {
    use crate::ltm_agg::is_synthetic_agg_name;

    // Nothing to do if no link touches a synthetic agg node.
    if !links
        .iter()
        .any(|l| is_synthetic_agg_name(l.from.as_str()) || is_synthetic_agg_name(l.to.as_str()))
    {
        return Some(links.to_vec());
    }

    let mut links: Vec<Link> = links.to_vec();
    loop {
        if links.is_empty() {
            return None;
        }
        // If every node in the cycle is a synthetic agg, there is nothing
        // meaningful left to report.
        if links
            .iter()
            .all(|l| is_synthetic_agg_name(l.from.as_str()) && is_synthetic_agg_name(l.to.as_str()))
        {
            return None;
        }
        // Find a link whose target is a synthetic agg node; merge it with the
        // following link (the agg's outgoing edge in this cycle).
        let Some(j) = links
            .iter()
            .position(|l| is_synthetic_agg_name(l.to.as_str()))
        else {
            // No synthetic agg appears as a link target anymore.
            break;
        };
        let n = links.len();
        let next = (j + 1) % n;
        debug_assert_eq!(
            links[j].to, links[next].from,
            "loop links must form a cycle"
        );
        let merged = Link {
            from: links[j].from.clone(),
            to: links[next].to.clone(),
            polarity: links[j].polarity.compose(links[next].polarity),
        };
        if next > j {
            links.splice(j..=next, std::iter::once(merged));
        } else {
            // Wraparound: the agg was the last node in the rotation. Drop the
            // trailing link and replace the first with the merged edge.
            links.pop();
            links[0] = merged;
        }
    }

    Some(links)
}

/// A directed causal link, optionally carrying its per-timestep LTM link-score
/// series, suitable for synthetic-node collapse.
///
/// This is the abstract shape [`collapse_synthetic_links`] operates on so the
/// collapse lives in the engine (and every binding benefits) while the caller
/// owns whatever string/score representation it ultimately serializes.
/// `score` is `None` for a structural-only caller (no simulation results) and
/// `Some(series)` for an LTM run; the collapse preserves the distinction.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct CollapsibleLink {
    pub from: Ident<Canonical>,
    pub to: Ident<Canonical>,
    pub polarity: LinkPolarity,
    /// Per-timestep link-score series. `None` when no LTM results back this
    /// link (the structural-only path), `Some` after an LTM simulation.
    pub score: Option<Vec<f64>>,
}

/// Per-timestep product of two link-score series (the LTM *path score*: the
/// product of the link scores along a path -- ref 6.3 / section 5.1).
///
/// `None` if either operand is absent (a path-score is only defined when every
/// edge in the path has a score series). When both are present they are
/// elementwise multiplied over the common prefix; a `NaN` factor propagates,
/// correctly marking that step's path score undefined.
fn multiply_score_series(a: &Option<Vec<f64>>, b: &Option<Vec<f64>>) -> Option<Vec<f64>> {
    match (a, b) {
        (Some(a), Some(b)) => {
            // Invariant: both operands are per-timestep link-score series that
            // span the same `step_count`, so their lengths always match. The
            // debug_assert fails loudly if a future change ever produces a
            // mismatch (which would silently misalign every later timestep);
            // release builds keep the defensive `min` so a mismatch degrades to
            // a short composite rather than a panic in production (#678).
            debug_assert_eq!(
                a.len(),
                b.len(),
                "multiply_score_series: link-score operands differ in length; both must span step_count"
            );
            let n = a.len().min(b.len());
            Some((0..n).map(|i| a[i] * b[i]).collect())
        }
        _ => None,
    }
}

/// Per-timestep, larger-magnitude selection between two candidate composite
/// series (the LTM *composite link score*: the path score with the largest
/// magnitude at each interval -- ref 6.3). Sign is preserved.
///
/// Mirrors the engine's `generate_max_abs_selection` step
/// (`if ABS(a) >= ABS(b) then a else b`): because `NaN` comparisons are
/// false, a `NaN` candidate loses to a finite one at that step. A present
/// series always beats an absent one (we cannot compare against nothing).
fn max_abs_score_series(a: Option<Vec<f64>>, b: Option<Vec<f64>>) -> Option<Vec<f64>> {
    match (a, b) {
        (Some(a), Some(b)) => {
            // Same invariant as `multiply_score_series`: both candidate series
            // span the same `step_count`. Fail loudly in debug/test on a
            // mismatch (it would silently misalign later timesteps), but keep
            // the defensive `min` in release so production degrades gracefully
            // rather than panicking (#678).
            debug_assert_eq!(
                a.len(),
                b.len(),
                "max_abs_score_series: candidate operands differ in length; both must span step_count"
            );
            let n = a.len().min(b.len());
            Some(
                (0..n)
                    .map(|i| if a[i].abs() >= b[i].abs() { a[i] } else { b[i] })
                    .collect(),
            )
        }
        (Some(s), None) | (None, Some(s)) => Some(s),
        (None, None) => None,
    }
}

/// Collapse synthetic/macro/module-internal nodes out of a causal link set,
/// preserving the loop-score contribution that flows *through* them.
///
/// A synthetic node is any whose canonical name carries the reserved `$⁚`
/// prefix ([`crate::ltm::is_synthetic_node_name`]) -- macro-instantiation
/// internals (`$⁚{var}⁚{n}⁚{func}`) and LTM-internal nodes
/// (`$⁚ltm⁚agg⁚{n}`, etc.). Real model variables never start with `$`.
///
/// This is the link-set generalization of
/// [`trim_synthetic_aggs_from_loop_links`] (which collapses only `$⁚ltm⁚agg⁚{n}`
/// nodes out of a single loop's link *cycle*). Per LTM ref 6.4, trimming a
/// macro/module means **collapse, not delete**: a chain
/// `[X -> $⁚…internal…, … -> Y]` becomes one composite edge `[X -> Y]` whose
/// polarity is the product of the collapsed links and whose score is the
/// **composite link score** -- the largest-magnitude path score through the
/// macro/module (ref 6.3). Deleting the internal links instead would
/// disconnect feedback paths through SMOOTH/DELAY/modules and silently drop
/// their contribution.
///
/// Concretely: every direct real -> real edge passes through unchanged, and
/// for every path `R0 -> s1 -> … -> sk -> R1` (each `si` synthetic, `R0`/`R1`
/// real) a composite edge `R0 -> R1` is emitted. The composite polarity is the
/// product along the path; the composite score is the per-timestep
/// max-magnitude over all such paths between the same endpoints, each path
/// score being the per-timestep product of its constituent link scores. A
/// purely-internal cycle (a synthetic node only reachable from synthetics,
/// like a macro's `$⁚…⁚arg1` helper, or an internal feedback loop) yields no
/// real -> real edge and is dropped -- LTM ref 6.4 "internal loop suppression".
///
/// The traversal never re-enters a real node and visits each synthetic node at
/// most once per path, so a synthetic-internal cycle cannot loop forever.
/// The accumulated composite payload for one collapsed edge: the polarity of
/// the largest-magnitude path found so far and its composite score series
/// (`None` until a scored path contributes).
type CompositePayload = (LinkPolarity, Option<Vec<f64>>);

/// One real endpoint reached by a synthetic chain, with the chain's accumulated
/// polarity and path score, produced by `collapse_synthetic_links`'s walk.
type ReachedEndpoint = (String, LinkPolarity, Option<Vec<f64>>);

pub fn collapse_synthetic_links(links: Vec<CollapsibleLink>) -> Vec<CollapsibleLink> {
    use crate::ltm::is_synthetic_node_name;

    let has_synthetic = links
        .iter()
        .any(|l| is_synthetic_node_name(l.from.as_str()) || is_synthetic_node_name(l.to.as_str()));
    if !has_synthetic {
        return links;
    }

    // Adjacency: from-node -> list of outgoing (to, polarity, score).
    let mut adj: HashMap<&str, Vec<&CollapsibleLink>> = HashMap::new();
    for l in &links {
        adj.entry(l.from.as_str()).or_default().push(l);
    }

    // Accumulated composite edges keyed on (real from, real to). Multiple
    // paths between the same endpoints fold together by per-timestep
    // max-magnitude (the composite link score, ref 6.3). The value is wrapped
    // in `Option` so `None` is an unambiguous "no contribution yet" marker:
    // `(Unknown, None)` is itself a legitimate first contribution (a
    // structural-only edge whose polarity is genuinely Unknown), so it must not
    // double as the uninitialized sentinel -- doing so would drop the first of
    // two disagreeing structural paths instead of folding them to Unknown.
    let mut composite: HashMap<(String, String), Option<CompositePayload>> = HashMap::new();

    // Walk every synthetic chain starting at the synthetic successor of a real
    // node, accumulating polarity and path score, until reaching the next real
    // node. `visited` guards against synthetic-internal cycles. There is no
    // explicit path-count budget: the enumeration is bounded only because the
    // synthetic interior of a macro/module is small (a handful of nodes); a
    // pathological synthetic subgraph with many internal diamonds could
    // enumerate exponentially many paths, but no real construct produces one.
    fn walk(
        adj: &HashMap<&str, Vec<&CollapsibleLink>>,
        node: &str,
        acc_polarity: LinkPolarity,
        acc_score: &Option<Vec<f64>>,
        visited: &mut HashSet<String>,
        out: &mut Vec<ReachedEndpoint>,
    ) {
        let Some(edges) = adj.get(node) else {
            return;
        };
        for edge in edges {
            let to = edge.to.as_str();
            let next_polarity = acc_polarity.compose(edge.polarity);
            let next_score = multiply_score_series(acc_score, &edge.score);
            if crate::ltm::is_synthetic_node_name(to) {
                // Visit each synthetic node at most once per path so an
                // internal cycle terminates.
                if !visited.insert(to.to_string()) {
                    continue;
                }
                walk(adj, to, next_polarity, &next_score, visited, out);
                visited.remove(to);
            } else {
                // Reached a real node: the chain `R0 -> … -> to` is a complete
                // composite path.
                out.push((to.to_string(), next_polarity, next_score));
            }
        }
    }

    for l in &links {
        // Only start a collapse from a real source node. A path that begins at
        // a synthetic node (e.g. a macro's argument helper) has no real
        // origin, so it produces no user-visible edge.
        if is_synthetic_node_name(l.from.as_str()) {
            continue;
        }
        if !is_synthetic_node_name(l.to.as_str()) {
            // Direct real -> real edge: pass through, folding into any
            // composite the same endpoints accumulate.
            let key = (l.from.as_str().to_string(), l.to.as_str().to_string());
            let slot = composite.entry(key).or_insert(None);
            if let Some((pol, sc)) = slot {
                *pol = pick_stronger_polarity(*pol, sc, l.polarity, &l.score);
                *sc = max_abs_score_series(sc.take(), l.score.clone());
            } else {
                // First contribution for this key: take it verbatim.
                *slot = Some((l.polarity, l.score.clone()));
            }
            continue;
        }
        // Synthetic successor: walk every chain through synthetics to the next
        // real node and emit a composite edge per reached endpoint.
        let mut reached = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(l.to.as_str().to_string());
        walk(
            &adj,
            l.to.as_str(),
            l.polarity,
            &l.score,
            &mut visited,
            &mut reached,
        );
        for (to_real, polarity, score) in reached {
            let key = (l.from.as_str().to_string(), to_real);
            let slot = composite.entry(key).or_insert(None);
            if let Some((pol, sc)) = slot {
                *pol = pick_stronger_polarity(*pol, sc, polarity, &score);
                *sc = max_abs_score_series(sc.take(), score);
            } else {
                *slot = Some((polarity, score));
            }
        }
    }

    let mut result: Vec<CollapsibleLink> = composite
        .into_iter()
        .filter_map(|((from, to), payload)| {
            payload.map(|(polarity, score)| CollapsibleLink {
                from: Ident::new(&from),
                to: Ident::new(&to),
                polarity,
                score,
            })
        })
        .collect();
    // Deterministic ordering so callers (and tests) see a stable link set.
    result.sort_by(|a, b| {
        a.from
            .as_str()
            .cmp(b.from.as_str())
            .then_with(|| a.to.as_str().cmp(b.to.as_str()))
    });
    result
}

/// When two candidate composites collapse onto the same `(from, to)` edge,
/// the reported polarity should follow the *stronger* (larger-magnitude)
/// path -- the same path whose score wins the max-abs selection -- so polarity
/// and score stay mutually consistent. When neither carries a comparable score
/// series (both `None`, the structural-only path) we fall back to composing:
/// an `Unknown` in either makes the merged polarity `Unknown`, since we cannot
/// say which path dominates.
fn pick_stronger_polarity(
    a_polarity: LinkPolarity,
    a_score: &Option<Vec<f64>>,
    b_polarity: LinkPolarity,
    b_score: &Option<Vec<f64>>,
) -> LinkPolarity {
    match (a_score, b_score) {
        (Some(a), Some(b)) => {
            // Compare aggregate magnitude (sum of |score| over finite steps);
            // the larger total magnitude is the dominant path overall.
            let mag =
                |s: &[f64]| -> f64 { s.iter().filter(|v| v.is_finite()).map(|v| v.abs()).sum() };
            if mag(a) >= mag(b) {
                a_polarity
            } else {
                b_polarity
            }
        }
        (Some(_), None) => a_polarity,
        (None, Some(_)) => b_polarity,
        (None, None) => {
            // No score to disambiguate: if both paths agree, keep it; else
            // the edge's polarity is genuinely ambiguous.
            if a_polarity == b_polarity {
                a_polarity
            } else {
                LinkPolarity::Unknown
            }
        }
    }
}

/// The integer-indexed runtime graph both candidate generators search.
///
/// The graph *topology* -- which `(from -> to)` edges exist and which result
/// slot each reads its score from -- is identical at every saved timestep;
/// only the per-edge score value changes. So it is built once: every node that
/// appears as a `from` or `to` endpoint (or is a stock) gets a dense `u32` id,
/// and edges are stored as `(to_id, result_offset)` in their `link_offsets`
/// order. Everything downstream is `Vec`-indexed by id, which is what keeps
/// long element-level identifiers (`population[nyc]`) out of every inner loop
/// -- on a C-LEARN-class graph, re-hashing those names per visit dominated
/// everything else discovery did.
///
/// Node ids follow `parse_link_offsets`' sorted output, so they are a function
/// of the model rather than of hash iteration order; every generator's
/// emission order inherits that determinism.
struct IndexedSearch {
    /// node id -> canonical identifier (for reconstructing discovered paths)
    idents: Vec<Ident<Canonical>>,
    /// node id -> outbound edges, in `link_offsets` insertion order. Each
    /// generator resolves its own per-step view of these; the static topology
    /// here never changes.
    adj: Vec<Vec<IndexedEdge>>,
    /// stock node ids, in the input `stocks` order (the fallback's seed order)
    stock_ids: Vec<u32>,
}

/// A static outbound edge: target node id plus the result slot its score is
/// read from each timestep.
/// Resolves the per-pathway result slots behind a `(from, to)` edge -- empty
/// unless `to` is a module instance with a unique entry port fed by `from`
/// (see [`ModuleOverrideCache::pathway_offsets`]).
type ModulePathwayFn<'a> =
    dyn FnMut(&Ident<Canonical>, &Ident<Canonical>) -> Option<Rc<[usize]>> + 'a;

/// How [`IndexedSearch::attach_module_pathways`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AttachOutcome {
    /// Every edge carries its pathway slots.
    Complete,
    /// The deadline expired part way; only a prefix of the edges is attached
    /// and the graph must not be searched.
    DeadlineExpired,
    /// The memory meter refused a pathway slot list; the graph cannot be
    /// completed within the discovery bound and must not be searched.
    MemoryRefused,
}

struct IndexedEdge {
    to: u32,
    offset: usize,
    /// Result slots of the per-pathway series (`m·$⁚ltm⁚path⁚{entry}⁚{idx}`)
    /// behind this edge when it enters a module instance; empty otherwise.
    /// The edge's own `offset` holds the module COMPOSITE, whose max-abs fold
    /// lets a NaN pathway shadow a finite one; the pathways let
    /// [`IndexedEdge::value_at`] see through that shadow so an edge (and any
    /// cycle through it) that a per-exit-port override can score is never
    /// read as inactive. Attached by [`IndexedSearch::attach_module_pathways`].
    extra_offsets: Rc<[usize]>,
}

impl IndexedEdge {
    /// The edge's link score at results row `base` (`step * step_size`),
    /// NaN-shadow-repaired for a module-input edge: the composite when it is
    /// active, otherwise the max-abs active pathway value (which is what the
    /// composite's own max-abs fold would have produced without a NaN
    /// operand), otherwise the composite as recorded (0 or NaN). For an edge
    /// with no pathways this is exactly `results.data[base + offset]`.
    #[inline]
    fn value_at(&self, results: &Results, base: usize) -> f64 {
        let composite = results.data[base + self.offset];
        if self.extra_offsets.is_empty() || is_active(composite) {
            return composite;
        }
        let mut best = composite;
        let mut best_abs = -1.0f64;
        for &off in self.extra_offsets.iter() {
            let v = results.data[base + off];
            if is_active(v) && v.abs() > best_abs {
                best_abs = v.abs();
                best = v;
            }
        }
        best
    }
}

/// How much work a deadline-aware search may do between wall-clock checks.
///
/// Reading `Instant::now()` per unit of work would dominate the unit itself,
/// so the check is amortized: with a power-of-two interval the counter test is
/// a single mask. 8192 units is well under a millisecond of search work, so
/// deadline overshoot stays negligible while clock reads stay under 0.1% of
/// the work. Shared by the enumerator (edge visits) and the fallback (heap
/// pops) so one budget means the same responsiveness in either generator.
const DEADLINE_CHECK_INTERVAL: u32 = 8192;

/// Whether an edge carries signal at a saved step.
///
/// The single definition both candidate generators use. A cycle one generator
/// considers active and the other does not would make `enumeration_complete`
/// mean different things about the same model, so this rule is stated once and
/// called from [`enum_gen::ActivityGraph::build`] and the fallback's per-step
/// adjacency alike. Infinity is a real, divergent signal and stays active;
/// only NaN (no `PREVIOUS` value yet, or an undefined partial) and an exact
/// zero are inactive, and a loop through a zero link scores exactly zero at
/// that step anyway.
#[inline]
fn is_active(value: f64) -> bool {
    (value != 0.0 && value.is_finite()) || value.is_infinite()
}

/// Wall-clock source for discovery's deadline checks.
///
/// Production reads `Instant::now()`. Tests substitute a scripted clock so
/// that expiry is a deterministic fact about a phase's structure rather than
/// a race against a real `Duration` -- a budget test that has to pick a
/// duration small enough to trip and large enough to make progress is flaky
/// on a loaded machine and slow on an idle one. Every deadline-aware phase
/// (`ActivityGraph::build`, `enumerate_active_circuits`, `retain_circuits`,
/// the fallback sweep) reads the clock only through this trait, so "an
/// unbudgeted call never reads the clock" is a testable claim rather than an
/// inspection of the source.
pub(crate) trait Clock {
    fn now(&mut self) -> Instant;
}

/// The production clock.
pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now(&mut self) -> Instant {
        Instant::now()
    }
}

/// `true` when `deadline` is set and has passed. An unbudgeted phase
/// (`deadline == None`) never reads the clock at all.
#[inline]
fn expired(deadline: Option<Instant>, clock: &mut dyn Clock) -> bool {
    match deadline {
        Some(d) => clock.now() >= d,
        None => false,
    }
}

/// A test clock whose expiry is scripted by read index rather than by real
/// time, so a phase's mid-work truncation is deterministic instead of racing
/// a `Duration`. It also counts its reads, which is how the
/// never-reads-the-clock claim is pinned.
#[cfg(test)]
pub(crate) struct ScriptedClock {
    base: Instant,
    /// Reads at index >= this one return a time far past [`Self::deadline`].
    expire_at_read: usize,
    pub(crate) reads: usize,
}

#[cfg(test)]
impl ScriptedClock {
    pub(crate) fn new(expire_at_read: usize) -> Self {
        ScriptedClock {
            base: Instant::now(),
            expire_at_read,
            reads: 0,
        }
    }

    /// A deadline this clock is before until `expire_at_read` reads have gone by.
    pub(crate) fn deadline(&self) -> Instant {
        self.base + Duration::from_secs(1)
    }
}

#[cfg(test)]
impl Clock for ScriptedClock {
    fn now(&mut self) -> Instant {
        let index = self.reads;
        self.reads += 1;
        if index + 1 >= self.expire_at_read {
            self.base + Duration::from_secs(3600)
        } else {
            self.base
        }
    }
}

impl IndexedSearch {
    /// Build the integer-indexed topology from the parsed link offsets and the
    /// stock list. Node ids are assigned in first-seen order over the edge
    /// endpoints (then any stock not yet seen), which is irrelevant to results
    /// since every lookup is id-keyed and the output is reconstructed from
    /// `idents`.
    fn build(link_offsets: &[LinkOffset], stocks: &[Ident<Canonical>]) -> Self {
        let mut id_of: HashMap<Ident<Canonical>, u32> =
            HashMap::with_capacity(link_offsets.len() * 2 + stocks.len());
        let mut idents: Vec<Ident<Canonical>> = Vec::new();

        let intern = |ident: &Ident<Canonical>,
                      id_of: &mut HashMap<Ident<Canonical>, u32>,
                      idents: &mut Vec<Ident<Canonical>>|
         -> u32 {
            if let Some(&id) = id_of.get(ident) {
                id
            } else {
                // Node ids are u32; SD models stay far below this (LTM paths
                // are capped at MAX_LTM_SCC_NODES and real edge counts are in
                // the thousands), but make the invariant explicit.
                debug_assert!(idents.len() <= u32::MAX as usize);
                let id = idents.len() as u32;
                idents.push(ident.clone());
                id_of.insert(ident.clone(), id);
                id
            }
        };

        // First pass: assign ids and collect edges. Edges keep their
        // `link_offsets` insertion order within each `from` node, which is
        // sorted by `(from, to)` -- so every per-node traversal order, and
        // hence every generator's emission order, is a function of the model
        // rather than of hash iteration.
        let mut adj: Vec<Vec<IndexedEdge>> = Vec::new();
        for ((from, to), offset) in link_offsets {
            let from_id = intern(from, &mut id_of, &mut idents);
            let to_id = intern(to, &mut id_of, &mut idents);
            if adj.len() <= from_id as usize {
                adj.resize_with(from_id as usize + 1, Vec::new);
            }
            adj[from_id as usize].push(IndexedEdge {
                to: to_id,
                offset: *offset,
                extra_offsets: Rc::from(Vec::new()),
            });
        }

        // Stocks that never appeared as an edge endpoint still need ids: the
        // fallback seeds a search from every stock, and a stock with no
        // outbound edges simply has an empty adjacency list.
        let stock_ids: Vec<u32> = stocks
            .iter()
            .map(|s| intern(s, &mut id_of, &mut idents))
            .collect();

        // Ensure `adj` is sized to the full node universe so every id is a
        // valid index (nodes that are only edge targets have empty lists).
        if adj.len() < idents.len() {
            adj.resize_with(idents.len(), Vec::new);
        }

        IndexedSearch {
            idents,
            adj,
            stock_ids,
        }
    }

    /// Number of distinct nodes.
    fn node_count(&self) -> usize {
        self.idents.len()
    }

    /// Attach the per-pathway result slots to every edge entering a module
    /// instance, so edge activity is read through the NaN shadow of the
    /// module composite (see [`IndexedEdge::extra_offsets`]). `pathways` is
    /// asked once per `(from, to)` edge and returns the slots for that pair
    /// (empty for a non-module target or an ambiguous entry port); a module
    /// with no resolvable pathways keeps composite-only activity, exactly as
    /// its scoring keeps the composite when the recompute declines.
    ///
    /// Each resolution can run the module's bounded-but-large internal pathway
    /// DFS, so the pass reads `deadline` every `DEADLINE_CHECK_INTERVAL` edges
    /// and stops when it has expired (an unbudgeted call never reads the
    /// clock). `pathways` returning `None` means the memory meter refused the
    /// slot list; the pass stops there too. Either way a partially attached
    /// graph would read some module-input edges as inactive where they are
    /// not, so the caller must skip BOTH candidate generators on anything but
    /// [`AttachOutcome::Complete`].
    fn attach_module_pathways(
        &mut self,
        pathways: &mut ModulePathwayFn<'_>,
        deadline: Option<Instant>,
        clock: &mut dyn Clock,
    ) -> AttachOutcome {
        let mut scanned: u32 = 0;
        for from in 0..self.adj.len() {
            for k in 0..self.adj[from].len() {
                scanned = scanned.wrapping_add(1);
                if scanned & (DEADLINE_CHECK_INTERVAL - 1) == 0 && expired(deadline, clock) {
                    return AttachOutcome::DeadlineExpired;
                }
                let to = self.adj[from][k].to;
                let Some(extra) = pathways(&self.idents[from], &self.idents[to as usize]) else {
                    return AttachOutcome::MemoryRefused;
                };
                self.adj[from][k].extra_offsets = extra;
            }
        }
        AttachOutcome::Complete
    }
}

/// Iterative Tarjan strongly-connected components over a dense
/// integer-indexed adjacency list.
///
/// Returns `(component_id_per_node, component_sizes)`: two nodes share a
/// component id iff they are mutually reachable, and `sizes[id]` is that
/// component's node count. Component ids are dense but otherwise arbitrary.
///
/// Used by discovery to identify a graph's *cyclic core*: a feedback loop can
/// only exist within a strongly-connected component, so any search outside the
/// seed's own component is provably wasted (GH #647). The fallback restricts
/// each Dijkstra this way; `discovery_graph_stats` reports the same structure
/// as a diagnostic.
fn tarjan_scc_ids(adj: &[Vec<u32>]) -> (Vec<u32>, Vec<u32>) {
    let mut scratch = TarjanScratch::default();
    let mut comp_ids = Vec::new();
    let mut comp_sizes = Vec::new();
    tarjan_scc_ids_into(adj, &mut scratch, &mut comp_ids, &mut comp_sizes);
    (comp_ids, comp_sizes)
}

/// Iterative frames mirroring `ltm::indexed::IndexedGraph::tarjan_scc`:
/// Enter pushes a node onto Tarjan's stack; Resume continues iterating its
/// successors and pops the SCC when this node is its own root.
enum TarjanFrame {
    Enter(u32),
    Resume { v: u32, next_child: u32 },
}

/// Reusable working buffers for [`tarjan_scc_ids_into`].
///
/// Tarjan needs six node-sized or stack-sized vectors, and the fallback runs
/// one SCC pass per saved step -- 401 of them on World3 -- so allocating them
/// per call costs more than the traversal does. A caller that runs the pass
/// repeatedly over the same node universe keeps one of these and hands it back
/// each time; [`tarjan_scc_ids`] allocates a throwaway for the one-shot
/// callers.
#[derive(Default)]
struct TarjanScratch {
    indices: Vec<i32>,
    lowlinks: Vec<i32>,
    on_stack: Vec<bool>,
    stack: Vec<u32>,
    frames: Vec<TarjanFrame>,
}

/// [`tarjan_scc_ids`] writing into caller-owned outputs and working buffers.
///
/// `comp_ids` is resized to `adj.len()` and fully overwritten; `comp_sizes` is
/// cleared and refilled. Both are `Vec`s rather than slices so the caller need
/// not know the node count up front, and so a shrinking graph reuses the same
/// allocation.
fn tarjan_scc_ids_into(
    adj: &[Vec<u32>],
    scratch: &mut TarjanScratch,
    comp_ids: &mut Vec<u32>,
    comp_sizes: &mut Vec<u32>,
) {
    const UNVISITED: i32 = -1;
    let n = adj.len();
    let TarjanScratch {
        indices,
        lowlinks,
        on_stack,
        stack,
        frames,
    } = scratch;
    indices.clear();
    indices.resize(n, UNVISITED);
    lowlinks.clear();
    lowlinks.resize(n, 0);
    on_stack.clear();
    on_stack.resize(n, false);
    stack.clear();
    comp_ids.clear();
    comp_ids.resize(n, 0);
    comp_sizes.clear();
    let mut next_index: i32 = 0;

    for start in 0..n as u32 {
        if indices[start as usize] != UNVISITED {
            continue;
        }
        frames.clear();
        frames.push(TarjanFrame::Enter(start));
        while let Some(frame) = frames.pop() {
            match frame {
                TarjanFrame::Enter(v) => {
                    indices[v as usize] = next_index;
                    lowlinks[v as usize] = next_index;
                    next_index += 1;
                    stack.push(v);
                    on_stack[v as usize] = true;
                    frames.push(TarjanFrame::Resume { v, next_child: 0 });
                }
                TarjanFrame::Resume { v, next_child } => {
                    let succs = &adj[v as usize];
                    if (next_child as usize) < succs.len() {
                        let w = succs[next_child as usize];
                        frames.push(TarjanFrame::Resume {
                            v,
                            next_child: next_child + 1,
                        });
                        if indices[w as usize] == UNVISITED {
                            frames.push(TarjanFrame::Enter(w));
                        } else if on_stack[w as usize] && indices[w as usize] < lowlinks[v as usize]
                        {
                            lowlinks[v as usize] = indices[w as usize];
                        }
                    } else {
                        if let Some(TarjanFrame::Resume { v: parent, .. }) = frames.last()
                            && lowlinks[v as usize] < lowlinks[*parent as usize]
                        {
                            lowlinks[*parent as usize] = lowlinks[v as usize];
                        }
                        if lowlinks[v as usize] == indices[v as usize] {
                            let comp_id = comp_sizes.len() as u32;
                            let mut size = 0u32;
                            loop {
                                let w = stack.pop().unwrap();
                                on_stack[w as usize] = false;
                                comp_ids[w as usize] = comp_id;
                                size += 1;
                                if w == v {
                                    break;
                                }
                            }
                            comp_sizes.push(size);
                        }
                    }
                }
            }
        }
    }
}

/// Per-sampled-timestep statistics about the discovery runtime graph.
///
/// See [`DiscoveryGraphStats`].
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct DiscoveryStepStats {
    /// The sampled timestep index.
    pub step: usize,
    /// Edges whose |score| is 0 (or NaN) at this step.
    pub zero_edges: usize,
    /// Edges whose |score| is exactly 1.0 at this step.
    pub unit_edges: usize,
    /// Edges with 0 < |score| < 1 at this step.
    pub sub_unit_edges: usize,
    /// Edges with |score| > 1 at this step -- a link whose target changed by
    /// MORE than its source did. They are why a `-log|score|` cycle weight
    /// cannot be used raw (a gain above 1 is a negative edge, and a loop with
    /// gain above 1 is a negative cycle, so no feasible Johnson potentials
    /// exist); `FallbackWeight` says how each formulation handles them.
    pub super_unit_edges: usize,
    /// Largest finite |score| at this step.
    pub max_abs_score: f64,
    /// Multi-node SCC sizes (descending) of the subgraph restricted to
    /// edges with nonzero scores at this step. Loops with a nonzero score at
    /// this step can only exist within these components.
    pub nonzero_scc_sizes: Vec<usize>,
    /// Number of stocks inside some multi-node nonzero-score SCC.
    pub stocks_in_nonzero_core: usize,
}

/// Structural statistics about the runtime graph discovery searches (GH #647).
///
/// Generator-independent: it describes the SHAPE of the graph the recorded
/// link scores define -- its size, its cyclic core (the SCC structure, the
/// only place loops can live), and how much of it carries signal at sampled
/// steps -- not what any particular candidate generator does with it. That is
/// what makes it usable to explain a discovery cost or a completeness verdict
/// after the fact, and it is the diagnostics surface behind
/// `examples/clearn_discover.rs`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct DiscoveryGraphStats {
    /// Total nodes in the runtime graph (link-score edge endpoints + stocks).
    pub n_nodes: usize,
    /// Total directed edges (parsed link-score columns, post-A2A expansion).
    pub n_edges: usize,
    /// Number of stocks (the fallback's seed nodes).
    pub n_stocks: usize,
    /// Multi-node SCC sizes of the static topology, descending.
    pub topology_scc_sizes: Vec<usize>,
    /// Number of stocks inside some multi-node SCC of the static topology.
    /// Only these stocks can participate in any feedback loop.
    pub stocks_in_cyclic_core: usize,
    /// Per-sampled-timestep stats, in the order requested.
    pub step_stats: Vec<DiscoveryStepStats>,
}

/// Compute [`DiscoveryGraphStats`] for the given simulation results.
///
/// `sample_steps` selects which timesteps get per-step score/SCC analysis
/// (full per-step analysis on every step would itself be a large cost on
/// big models). Steps outside `1..results.step_count` are skipped.
pub fn discovery_graph_stats(
    results: &Results,
    stocks: &[Ident<Canonical>],
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
    expansion: &LinkExpansionContext,
    sample_steps: &[usize],
) -> DiscoveryGraphStats {
    let link_offsets = parse_link_offsets(results, ltm_vars, dims, expansion);
    let search = IndexedSearch::build(&link_offsets, stocks);
    let n_nodes = search.node_count();

    // Static topology SCCs.
    let topo_adj: Vec<Vec<u32>> = search
        .adj
        .iter()
        .map(|edges| edges.iter().map(|e| e.to).collect())
        .collect();
    let (topo_ids, topo_sizes) = tarjan_scc_ids(&topo_adj);
    let mut topology_scc_sizes: Vec<usize> = topo_sizes
        .iter()
        .filter(|&&s| s > 1)
        .map(|&s| s as usize)
        .collect();
    topology_scc_sizes.sort_unstable_by(|a, b| b.cmp(a));
    let stocks_in_cyclic_core = search
        .stock_ids
        .iter()
        .filter(|&&sid| topo_sizes[topo_ids[sid as usize] as usize] > 1)
        .count();

    // Per-sampled-step score distribution + nonzero-subgraph SCCs.
    let mut step_stats = Vec::with_capacity(sample_steps.len());
    for &step in sample_steps {
        if step == 0 || step >= results.step_count {
            continue;
        }
        let base = step * results.step_size;
        let mut zero_edges = 0usize;
        let mut unit_edges = 0usize;
        let mut sub_unit_edges = 0usize;
        let mut super_unit_edges = 0usize;
        let mut max_abs_score = 0.0f64;
        let mut nonzero_adj: Vec<Vec<u32>> = vec![Vec::new(); n_nodes];
        for (node, edges) in search.adj.iter().enumerate() {
            for edge in edges {
                let value = edge.value_at(results, base);
                let score = if value.is_nan() { 0.0 } else { value.abs() };
                if score == 0.0 {
                    zero_edges += 1;
                } else {
                    if score == 1.0 {
                        unit_edges += 1;
                    } else if score < 1.0 {
                        sub_unit_edges += 1;
                    } else {
                        super_unit_edges += 1;
                    }
                    if score.is_finite() && score > max_abs_score {
                        max_abs_score = score;
                    }
                    nonzero_adj[node].push(edge.to);
                }
            }
        }
        let (nz_ids, nz_sizes) = tarjan_scc_ids(&nonzero_adj);
        let mut nonzero_scc_sizes: Vec<usize> = nz_sizes
            .iter()
            .filter(|&&s| s > 1)
            .map(|&s| s as usize)
            .collect();
        nonzero_scc_sizes.sort_unstable_by(|a, b| b.cmp(a));
        let stocks_in_nonzero_core = search
            .stock_ids
            .iter()
            .filter(|&&sid| nz_sizes[nz_ids[sid as usize] as usize] > 1)
            .count();

        step_stats.push(DiscoveryStepStats {
            step,
            zero_edges,
            unit_edges,
            sub_unit_edges,
            super_unit_edges,
            max_abs_score,
            nonzero_scc_sizes,
            stocks_in_nonzero_core,
        });
    }

    DiscoveryGraphStats {
        n_nodes,
        n_edges: link_offsets.len(),
        n_stocks: stocks.len(),
        topology_scc_sizes,
        stocks_in_cyclic_core,
        step_stats,
    }
}

/// Read the output port a (non-module) variable `reader` reads off module
/// instance `module_name` via interpunct notation `m·{port}`, ignoring the
/// module's synthetic LTM internals (`m·$⁚ltm⁚…`). Returns the unique such
/// port, or `None` when the reader reads zero or several (ambiguous).
///
/// This is the post-simulation twin of `db::ltm::module_exit_port_for_reader`
/// (the exhaustive-mode override's exit-port determinator); both must agree so
/// discovery and exhaustive select the same pathway for the same loop edge.
fn discovery_module_exit_port(
    module_name: &Ident<Canonical>,
    reader: &crate::variable::Variable,
) -> Option<Ident<Canonical>> {
    let ast = reader.ast()?;
    let deps = crate::variable::identifier_set(ast, &[], None);
    let prefix = format!("{}\u{00B7}", module_name.as_str());
    let mut found: Option<Ident<Canonical>> = None;
    for dep in deps {
        let Some(port) = dep.as_str().strip_prefix(&prefix) else {
            continue;
        };
        if port.starts_with('$') {
            continue;
        }
        if found.is_some() {
            return None;
        }
        found = Some(Ident::new(port));
    }
    found
}

/// Recompute a module-input loop edge's link-score series from the sub-model's
/// per-pathway scores, selecting the pathway(s) that terminate at the exit port
/// the loop actually traverses (GH #698).
///
/// Discovery emits no loop-score variables, so the `x → m` edge's link score is
/// the module's *composite* (`m·$⁚ltm⁚composite⁚{port}`), which max-abs-selects
/// across ALL output-port pathways. For single-dependency pathways every
/// pathway normalizes to magnitude exactly 1, so the composite's tie-break picks
/// an arbitrary (first-enumerated) port -- possibly one whose sign opposes the
/// pathway the loop reads, flipping the loop's polarity. Exhaustive mode fixes
/// this with a per-exit-port override on the loop-score equation; this is the
/// discovery-mode equivalent applied during post-simulation score recomputation.
///
/// `edge_idx` is the index of the `x → m` link in `links`; the next link
/// `m → y` identifies the exit port. Returns the recomputed signed series, or
/// `None` to leave the base (composite) series in place when:
/// * `m` is not a module instance with a recursively-built sub-graph;
/// * the entry port is ambiguous -- `x` feeds MORE THAN ONE input port of `m`,
///   so the collapsed `x → m` edge has no single entry pathway to recompute
///   against (the base composite, itself a documented first-matched-port
///   approximation, is the honest fallback; mirrors the exit-port helper's and
///   the exhaustive twin's multi-match → ambiguous semantics);
/// * the exit port is ambiguous -- a non-module reader `y` reads two distinct
///   `m·port`s, or a module reader `y` reads two distinct output ports of `m`
///   on different inputs (`m·early → y.p` AND `m·late → y.q` collapse to one
///   `m → y` edge); two of `y`'s inputs naming the SAME `m·port` are NOT
///   ambiguous (a unique distinct port);
/// * the sub-model's pathway map yields no pathway from entry to exit.
///
/// Discovery runs on the ELEMENT-LEVEL graph, so an arrayed loop's non-module
/// nodes carry element subscripts (`s[nyc] → m → growth[nyc]`). Every
/// name-sensitive lookup here (entry-port match against the bare
/// `ModuleInput.src`, exit-reader lookup in the bare-keyed `variables()` map,
/// the module-instance node) `strip_subscript`s its operand first, mirroring
/// the exhaustive twin (db/ltm/mod.rs strips `link.from`/`link.to`/`next.from`/
/// `next.to`). Without it the exact matches fail for every arrayed module loop
/// and the recompute declines, re-introducing the wrong-exit-port composite bug
/// (GH #698 / PR #705 r3353758167). This stripping is LIVE, not latent: an
/// arrayed loop through a multi-output module is discoverable end-to-end since
/// GH #716 was closed. A scalar module output feeding an arrayed reader used to
/// emit one scalar constant-0 link score, which dropped the loop; it is now
/// scored per target element by
/// `db::ltm::link_scores::try_implicit_scalar_to_arrayed_link_scores`, and
/// `analysis::tests::analyze_model_arrayed_module_loop_is_discovered_per_element`
/// pins the end-to-end result.
///
/// The pathway selection mirrors `db::ltm::compute_module_link_overrides`: the
/// pathway indices are recomputed from the sub-model graph via the SAME
/// `enumerate_pathways_to_outputs_with_truncation` machinery the emission uses,
/// over the SAME sorted output-port set, so the indices match the emitted
/// `$⁚ltm⁚path⁚{entry}⁚{idx}` variables index-for-index.
///
/// This is the ENGINE [`ModuleOverrideCache::series`] memoizes: it takes the
/// already-resolved, already-subscript-stripped `(from, module, exit_reader)`
/// triple directly rather than a `links`/`edge_idx` pair, because every step
/// below -- the entry-port match, the exit-port resolution, the pathway
/// recompute -- depends on nothing else. [`recompute_module_input_edge_series`]
/// is the links-slice ADAPTER kept for call sites (and tests) that still hold
/// a `Link` sequence; it strips and extracts the triple and delegates here.
#[allow(clippy::too_many_arguments)]
fn recompute_module_input_edge_series_for(
    causal_graph: &CausalGraph,
    results: &Results,
    from_base: &Ident<Canonical>,
    module_name: &Ident<Canonical>,
    exit_reader: &Ident<Canonical>,
    step_count: usize,
    sub_model_output_ports: &SubModelOutputPorts,
) -> Option<Vec<f64>> {
    use crate::ltm::normalize_module_ref;
    use crate::variable::Variable;

    // `m` must be a module instance with a recursively-built internal graph
    // (a DynamicModule / passthrough exposing pathways). Pathless modules and
    // non-modules keep the base link score.
    let module_graph = causal_graph.module_graph(module_name)?;

    // Entry port: `m`'s ModuleInput whose normalized src is `x` (== from_base).
    // When `x` feeds MORE THAN ONE input port of `m` (`x -> m.a` AND `x -> m.b`)
    // the collapsed `x -> m` edge is genuinely ambiguous: there is no single
    // entry pathway to recompute against. Decline (return `None`) so the loop
    // keeps the base composite link score -- the documented pre-existing
    // approximation -- rather than silently picking the first matching port and
    // recomputing against its (possibly wrong-signed) pathway. This mirrors the
    // multi-match -> ambiguous semantics of `discovery_module_exit_port` and the
    // exhaustive twin `compute_module_link_overrides` (GH #698 / PR #705
    // r3353459409).
    let module_var = causal_graph.variables().get(module_name)?;
    let Variable::Module { inputs, .. } = module_var else {
        return None;
    };
    let mut matching = inputs
        .iter()
        .filter(|inp| normalize_module_ref(&inp.src) == *from_base);
    let entry_port = matching.next()?.dst.clone();
    if matching.next().is_some() {
        // A second input port is also fed by `x`: ambiguous entry, fall back.
        return None;
    }

    // Exit port, resolved off the reader `y` supplied by the caller.
    let y_var = causal_graph.variables().get(exit_reader)?;
    let exit_port = match y_var {
        // `y` is itself a module: m's output feeds y's input port(s). y's
        // ModuleInput src is the qualified `m·{port}`; the exit port is the
        // `{port}` whose normalized ref is `m`. If `y` reads TWO DISTINCT
        // output ports of `m` on different inputs (`m·early -> y.p` AND
        // `m·late -> y.q`), the collapsed `m -> y` edge has no unique exit
        // port -- decline (ambiguous) and fall back to the base composite,
        // mirroring the non-module `discovery_module_exit_port` arm and the
        // exhaustive twin (GH #698 / PR #705 r3353597299). Two inputs naming
        // the SAME `m·port` are NOT ambiguous: a unique distinct port is fine.
        Variable::Module { inputs: y_in, .. } => {
            let mut exit: Option<Ident<Canonical>> = None;
            for inp in y_in {
                if normalize_module_ref(&inp.src) != *module_name {
                    continue;
                }
                let Some((_, port)) = inp.src.as_str().split_once('\u{00B7}') else {
                    continue;
                };
                let port = Ident::<Canonical>::new(port);
                match &exit {
                    Some(prev) if *prev != port => return None, // two distinct ports
                    Some(_) => {}                               // same port repeated: fine
                    None => exit = Some(port),
                }
            }
            exit
        }
        _ => discovery_module_exit_port(module_name, y_var),
    }?;

    // Recompute the sub-model's pathway map over the same sorted output-port
    // set the sub-model emitted against, so pathway indices match index-for-
    // index. The set comes from the emission-derived map (built by
    // `analyze_model` via `db::ltm::sub_model_output_ports`, the SAME decision
    // the sub-model used to emit its `$⁚ltm⁚path⁚{port}⁚{idx}` vars), keyed by
    // the sub-model's canonical name -- NOT a parent-scoped re-derivation,
    // which would shift the indices when ANOTHER project model reads a
    // different output port (GH #698 / PR #705 r3353097150).
    let Variable::Module {
        model_name: sub_model_name,
        ..
    } = module_var
    else {
        return None;
    };
    let output_ports = sub_model_output_ports.get(sub_model_name)?;
    if output_ports.is_empty() {
        return None;
    }
    let (pathways, _truncated) =
        module_graph.enumerate_pathways_to_outputs_with_truncation(output_ports);
    let port_pathways = pathways.get(&entry_port)?;

    // Result offsets of the `m·$⁚ltm⁚path⁚{entry}⁚{idx}` series whose pathway
    // ends at the exit port. The pathway var rides under the module instance
    // namespace (`{instance}·…`).
    let matching_offsets: Vec<usize> = port_pathways
        .iter()
        .enumerate()
        .filter(|(_, path_links)| path_links.last().is_some_and(|l| l.to == exit_port))
        .filter_map(|(idx, _)| {
            let name = format!(
                "{}\u{00B7}$\u{205A}ltm\u{205A}path\u{205A}{}\u{205A}{idx}",
                module_name.as_str(),
                entry_port.as_str()
            );
            results
                .offsets
                .get(&Ident::<Canonical>::new(&name))
                .copied()
        })
        .collect();
    if matching_offsets.is_empty() {
        return None;
    }

    // Per-step max-abs selection over the matching pathway series (mirroring
    // the sub-model composite's selection, but restricted to the exit port),
    // folded IN PLACE into one buffer read straight off the slab -- the
    // footprint `ModuleOverrideCache::series` pre-charges -- with exactly
    // `max_abs_score_series`'s per-step rule and operand order (the rule is
    // not symmetric under NaN: a NaN later operand wins, a NaN earlier one
    // loses), so the fold reads the same as the pairwise one did.
    let mut offsets = matching_offsets.into_iter();
    let first = offsets.next()?;
    let mut series: Vec<f64> = (0..step_count)
        .map(|step| results.data[step * results.step_size + first])
        .collect();
    for off in offsets {
        for (step, slot) in series.iter_mut().enumerate() {
            let candidate = results.data[step * results.step_size + off];
            // `max_abs_score_series` keeps the earlier operand iff
            // `a.abs() >= b.abs()`; spelled as the kept-operand test so a
            // NaN on either side resolves exactly as it does there.
            let keep_earlier = slot.abs() >= candidate.abs();
            if !keep_earlier {
                *slot = candidate;
            }
        }
    }
    Some(series)
}

/// Links-slice adapter over [`recompute_module_input_edge_series_for`], kept
/// for callers (and tests) that hold a `Link` sequence rather than an
/// already-resolved `(from, module, exit_reader)` triple. Production no
/// longer calls this directly for a sequential edge -- see
/// [`ModuleOverrideCache::series`] -- but it stays the exact reference
/// behaviour for a NON-sequential edge (`next.from != links[i].to`, outside
/// what the cache's key spells) and for any test exercising the recompute in
/// isolation.
fn recompute_module_input_edge_series(
    causal_graph: &CausalGraph,
    results: &Results,
    links: &[Link],
    edge_idx: usize,
    step_count: usize,
    sub_model_output_ports: &SubModelOutputPorts,
) -> Option<Vec<f64>> {
    use crate::ltm::strip_subscript;

    let n = links.len();
    let link = &links[edge_idx];

    // Discovery runs on the ELEMENT-LEVEL graph, so an arrayed loop's
    // non-module nodes carry element subscripts (`s[nyc] -> m -> growth[nyc]`).
    // Every name-sensitive lookup in the engine compares against bare names
    // (`ModuleInput.src`, the bare-keyed `variables()` map, the module
    // instance node), so strip the subscript first -- mirroring the exhaustive
    // twin `compute_module_link_overrides`, which `strip_subscript`s
    // `link.from` / `link.to` / `next.from` / `next.to` (db/ltm/mod.rs) before
    // the same matches. Without this the exact comparisons fail for EVERY
    // arrayed module loop, the recompute declines, and the wrong-exit-port
    // composite bug it exists to fix re-occurs (GH #698 / PR #705 r3353758167).
    // A module instance node is itself unsubscripted in the element graph, but
    // stripping is idempotent on a bare name, so it is harmless.
    let from_base = Ident::<Canonical>::new(strip_subscript(link.from.as_str()));
    let module_name = Ident::<Canonical>::new(strip_subscript(link.to.as_str()));

    // Exit port from the next link `m → y`.
    let next = &links[(edge_idx + 1) % n];
    // The loop links are emitted in traversal order, so `next.from == m`; guard
    // against a non-sequential list rather than reading a port off an unrelated
    // edge. Strip the subscript so a subscripted `next.from` still matches the
    // (bare) module node.
    if Ident::<Canonical>::new(strip_subscript(next.from.as_str())) != module_name {
        return None;
    }
    let y = Ident::<Canonical>::new(strip_subscript(next.to.as_str()));

    recompute_module_input_edge_series_for(
        causal_graph,
        results,
        &from_base,
        &module_name,
        &y,
        step_count,
        sub_model_output_ports,
    )
}

/// Memoized per-exit-port module-input override series, shared by RETENTION
/// (`ltm_finding_enum.rs`'s `retain_circuits`/`dedup_trimmed_twins`/
/// `accumulate_series_into_totals`, via the `ModuleOverrideFn` closure
/// [`Self::series`] backs) and `FoundLoop` MATERIALIZATION -- the same one
/// instance, so the two phases can never resolve a module-traversing loop's
/// score to different series.
///
/// Keyed by `(from, module, exit_reader)`, each stripped of its element
/// subscript exactly as [`recompute_module_input_edge_series`] strips its own
/// operands, so an arrayed loop's per-element instances share one cache entry
/// rather than re-enumerating the sub-model's pathway map once per element.
/// One instance per discovery call.
/// A module-input edge `(from, module)` with both endpoints' element subscripts
/// stripped -- the key the pathway-offset cache shares across an arrayed loop's
/// per-element instances.
type ModuleEdgeKey = (Ident<Canonical>, Ident<Canonical>);

pub(crate) struct ModuleOverrideCache<'a> {
    causal_graph: &'a CausalGraph,
    results: &'a Results,
    sub_model_output_ports: &'a SubModelOutputPorts,
    step_count: usize,
    cache: ModuleSeriesCache,
    /// `(from, module)` -> every pathway slot behind that module-input edge,
    /// regardless of exit port (see [`Self::pathway_offsets`]).
    pathway_cache: HashMap<ModuleEdgeKey, Rc<[usize]>>,
    /// The discovery-wide memory bound the retained series and pathway slots
    /// are charged to; a series that does not fit is returned but not
    /// retained (recomputation instead of growth).
    meter: &'a MemoryMeter,
}

impl<'a> ModuleOverrideCache<'a> {
    pub(crate) fn new(
        causal_graph: &'a CausalGraph,
        results: &'a Results,
        sub_model_output_ports: &'a SubModelOutputPorts,
        step_count: usize,
        meter: &'a MemoryMeter,
    ) -> Self {
        ModuleOverrideCache {
            causal_graph,
            results,
            sub_model_output_ports,
            step_count,
            cache: HashMap::new(),
            pathway_cache: HashMap::new(),
            meter,
        }
    }

    /// Every `m·$⁚ltm⁚path⁚{entry}⁚{idx}` result slot behind the edge
    /// `from -> module`, over ALL exit ports -- the series whose max-abs fold
    /// is the composite the edge's own slot records. Used to read edge
    /// ACTIVITY through the composite's NaN shadow (a NaN pathway hides a
    /// finite sibling in the fold): if any pathway is active at a step, some
    /// per-exit-port override through this edge can be, so the edge is. Empty
    /// for a non-module target, an ambiguous entry port (`from` feeds two
    /// input ports -- the same decline as [`Self::series`], whose scoring then
    /// keeps the composite too), or a module with no emitted pathways.
    pub(crate) fn pathway_offsets(
        &mut self,
        from: &Ident<Canonical>,
        module: &Ident<Canonical>,
    ) -> Option<Rc<[usize]>> {
        use crate::ltm::{normalize_module_ref, strip_subscript};
        use crate::variable::Variable;

        let key = (
            Ident::<Canonical>::new(strip_subscript(from.as_str())),
            Ident::<Canonical>::new(strip_subscript(module.as_str())),
        );
        if let Some(cached) = self.pathway_cache.get(&key) {
            return Some(Rc::clone(cached));
        }
        let resolve = || -> Option<Vec<usize>> {
            let module_graph = self.causal_graph.module_graph(&key.1)?;
            let module_var = self.causal_graph.variables().get(&key.1)?;
            let Variable::Module {
                inputs,
                model_name: sub_model_name,
                ..
            } = module_var
            else {
                return None;
            };
            let mut matching = inputs
                .iter()
                .filter(|inp| normalize_module_ref(&inp.src) == key.0);
            let entry_port = matching.next()?.dst.clone();
            if matching.next().is_some() {
                return None;
            }
            let output_ports = self.sub_model_output_ports.get(sub_model_name)?;
            if output_ports.is_empty() {
                return None;
            }
            let (pathways, _truncated) =
                module_graph.enumerate_pathways_to_outputs_with_truncation(output_ports);
            let port_pathways = pathways.get(&entry_port)?;
            let offsets: Vec<usize> = (0..port_pathways.len())
                .filter_map(|idx| {
                    let name = format!(
                        "{}\u{00B7}$\u{205A}ltm\u{205A}path\u{205A}{}\u{205A}{idx}",
                        key.1.as_str(),
                        entry_port.as_str()
                    );
                    self.results
                        .offsets
                        .get(&Ident::<Canonical>::new(&name))
                        .copied()
                })
                .collect();
            Some(offsets)
        };
        let offsets: Rc<[usize]> = Rc::from(resolve().unwrap_or_default());
        // One shared slot list per key: every array-expanded edge of the same
        // (source, module) pair points at it, so the slots cost their size
        // once rather than once per element edge. Charged to the meter BEFORE
        // it is handed out: a refusal returns `None` rather than an uncharged
        // list the caller would retain (and every later edge of the same key
        // would recompute and retain again, outside the bound).
        if !self
            .meter
            .charge(std::mem::size_of_val::<[usize]>(&offsets))
        {
            return None;
        }
        self.pathway_cache.insert(key, Rc::clone(&offsets));
        Some(offsets)
    }

    /// The override series for the edge `from -> module` whose next hop reads
    /// `exit_reader`, paired with that series' OWN active window (the [lo, hi)
    /// range bounding every step where it is active per [`is_active`]) -- or
    /// `None` when no single exit pathway resolves (see
    /// [`recompute_module_input_edge_series_for`]'s doc for every decline
    /// case). Idents need not be pre-stripped -- the lookup strips them itself,
    /// so an arrayed loop's per-element edges share one entry regardless of
    /// what the caller passes.
    ///
    /// The window is computed once here (linear scan over one series) rather
    /// than once per circuit that substitutes this override: outside it the
    /// override contributes exactly 0 or NaN at every step by construction,
    /// so retention can score a module-traversing circuit against THIS
    /// window instead of the full saved-step range and still miss no mass --
    /// see `ltm_finding_enum::score_steps`'s doc for why blindly widening to
    /// the full range was a measured 2.6x regression on World3.
    pub(crate) fn series(
        &mut self,
        from: &Ident<Canonical>,
        module: &Ident<Canonical>,
        exit_reader: &Ident<Canonical>,
    ) -> OverrideLookup {
        use crate::ltm::strip_subscript;

        let key = (
            Ident::<Canonical>::new(strip_subscript(from.as_str())),
            Ident::<Canonical>::new(strip_subscript(module.as_str())),
            Ident::<Canonical>::new(strip_subscript(exit_reader.as_str())),
        );
        if let Some(cached) = self.cache.get(&key) {
            return OverrideLookup::from_entry(cached);
        }
        // The series is charged BEFORE it is allocated: the recompute folds
        // the matching pathway rows into exactly one `step_count`-long buffer
        // read straight off the results slab, so this is its whole footprint.
        // A decline releases the charge; a resolved series keeps it, retained
        // in the cache.
        let bytes = self.step_count * std::mem::size_of::<f64>();
        if !self.meter.charge(bytes) {
            return OverrideLookup::OutOfMemory;
        }
        let entry = recompute_module_input_edge_series_for(
            self.causal_graph,
            self.results,
            &key.0,
            &key.1,
            &key.2,
            self.step_count,
            self.sub_model_output_ports,
        )
        .map(|s| {
            let window = active_window_of(&s);
            (Rc::new(s), window)
        });
        if entry.is_none() {
            self.meter.release(bytes);
        }
        let entry = self.cache.entry(key).or_insert(entry);
        OverrideLookup::from_entry(entry)
    }

    /// The links-slice twin of [`Self::series`] for an edge whose next link is
    /// not spelled sequentially (`links[i+1].from != links[i].to` before
    /// subscript stripping), answered uncached through
    /// [`recompute_module_input_edge_series`] under the SAME pre-charge: the
    /// one allocation is the series, charged before it exists and released
    /// when the recompute declines. Kept on the caller's side as an `Rc`, so
    /// the charge stays until discovery returns, like a cached entry's.
    pub(crate) fn uncached_series(&self, links: &[Link], edge_idx: usize) -> OverrideLookup {
        let bytes = self.step_count * std::mem::size_of::<f64>();
        if !self.meter.charge(bytes) {
            return OverrideLookup::OutOfMemory;
        }
        match recompute_module_input_edge_series(
            self.causal_graph,
            self.results,
            links,
            edge_idx,
            self.step_count,
            self.sub_model_output_ports,
        ) {
            Some(series) => {
                let window = active_window_of(&series);
                OverrideLookup::Resolved(Rc::new(series), window)
            }
            None => {
                self.meter.release(bytes);
                OverrideLookup::Declined
            }
        }
    }
}

/// The `[lo, hi)` range bounding every step where `series` is active (per
/// [`is_active`]) -- the flat-`Vec` twin of `ltm_finding_enum::ActivityGraph::
/// active_window`'s contract, computed directly over values rather than a
/// precomputed bitset. Fine to do with a linear scan here: it runs once per
/// UNIQUE override series (memoized by [`ModuleOverrideCache`]), not once per
/// circuit that substitutes it. Returns `(0, 0)` (an empty range) when the
/// series is never active anywhere.
fn active_window_of(series: &[f64]) -> (usize, usize) {
    let Some(first) = series.iter().position(|&v| is_active(v)) else {
        return (0, 0);
    };
    let last = series
        .iter()
        .rposition(|&v| is_active(v))
        .expect("a first active step exists, so a last one does too");
    (first, last + 1)
}

/// Recover the cross-element-through-aggregate loops (GH #696) hiding in a
/// candidate set, as `IndexedSearch` node-id paths.
///
/// Both generators emit only ELEMENTARY cycles, so a feedback loop that
/// traverses a hoisted reducer's synthetic agg node more than once
/// (`pop[a] -> agg -> pop[b] -> agg -> pop[a]`) is structurally unreachable to
/// either -- the enumerator never repeats a node and a Dijkstra tree path
/// never does either. Exhaustive mode recovers such a loop by stitching the
/// single-agg "petals" together, and this is discovery's call into that SAME
/// combinatorial core (`db::stitch_cross_agg_petals`), which is what makes
/// discovery recover exactly the loops exhaustive does.
///
/// Both generators call this one helper, so what the two report about a
/// reducer model differs only in the petals they found, never in how those
/// petals combine. Returns the stitched sequences plus whether the model-wide
/// loop budget clipped the enumeration.
///
/// A candidate path touching zero or two-plus agg OCCURRENCES is not a petal
/// and `collect_agg_petals` drops it, so callers may pass their whole
/// candidate set; the enumeration path pre-filters only to avoid materializing
/// node paths for a universe-sized set. Occurrences rather than distinct aggs
/// -- the two coincide for an elementary cycle, which never repeats a node,
/// and every candidate either generator emits is elementary.
///
/// Metered: the petal collection keeps a bounded number of petals per agg and
/// charges each to `meter` as it goes (credited back here once they are
/// stitched), and the stitched output is charged at its upper bound BEFORE it
/// is built; that charge is returned as `output_charge` for the caller to
/// release once it has consumed (or re-charged) the sequences. `None` means
/// the meter refused somewhere and no stitching happened.
fn stitch_cross_agg_node_paths<I, P>(
    search: &IndexedSearch,
    candidate_paths: I,
    meter: &MemoryMeter,
) -> Option<StitchedNodePaths>
where
    I: IntoIterator<Item = P>,
    P: AsRef<[u32]>,
{
    // What the kept petals currently hold on the meter; credited back once
    // they are stitched (or the collection is abandoned).
    let petal_bytes = std::cell::Cell::new(0usize);
    let petals_by_agg = crate::db::collect_agg_petals(
        candidate_paths,
        |id: &u32| search.idents[*id as usize].as_str(),
        &mut |bytes| {
            let ok = meter.charge(bytes);
            if ok {
                petal_bytes.set(petal_bytes.get() + bytes);
            }
            ok
        },
        &mut |bytes| {
            meter.release(bytes);
            petal_bytes.set(petal_bytes.get() - bytes);
        },
    );
    let Some(petals_by_agg) = petals_by_agg else {
        meter.release(petal_bytes.get());
        return None;
    };
    // Deterministic agg order by NAME (the exhaustive path's order too), so
    // the loop budget clips the same aggs in both modes.
    let mut sorted: Vec<(u32, Vec<crate::db::StitchPetal<u32>>)> =
        petals_by_agg.into_iter().collect();
    sorted.sort_by(|a, b| {
        search.idents[a.0 as usize]
            .as_str()
            .cmp(search.idents[b.0 as usize].as_str())
    });
    let budget = crate::db::cross_agg_loop_budget();
    let output_charge = crate::db::stitched_output_bound(&sorted, budget);
    if !meter.charge(output_charge) {
        meter.release(petal_bytes.get());
        return None;
    }
    let (paths, truncated_aggs) = crate::db::stitch_cross_agg_petals(sorted, budget);
    meter.release(petal_bytes.get());
    Some(StitchedNodePaths {
        paths,
        truncated: !truncated_aggs.is_empty(),
        output_charge,
    })
}

/// [`stitch_cross_agg_node_paths`]'s answer.
struct StitchedNodePaths {
    /// The stitched loops, as node-id sequences starting at their agg.
    paths: Vec<Vec<u32>>,
    /// The model-wide loop budget or the per-agg petal cap clipped the
    /// enumeration.
    truncated: bool,
    /// Bytes charged to the meter for `paths` (at their upper bound), for the
    /// caller to release once it has consumed them.
    output_charge: usize,
}

/// The `stitched` sequences whose canonical rotation is not already in
/// `existing` -- the same directed-cycle identity both generators dedup on
/// (`crate::ltm::canonical_rotation`, issue #308), so a stitched loop never
/// duplicates a generated one and opposite-direction cycles over the same node
/// set stay distinct.
///
/// Keyed on node ids rather than names: within one `IndexedSearch` the id <->
/// name map is a bijection, so the equivalence classes are identical and no
/// name vector is allocated per cycle.
fn new_paths_by_rotation(existing: &[Vec<u32>], stitched: Vec<Vec<u32>>) -> Vec<Vec<u32>> {
    let mut seen: HashSet<Vec<u32>> = existing
        .iter()
        .map(|path| crate::ltm::canonical_rotation(path))
        .collect();
    stitched
        .into_iter()
        .filter(|seq| seen.insert(crate::ltm::canonical_rotation(seq)))
        .collect()
}

/// The share of a caller's wall-clock budget the enumeration path may spend
/// before it must yield to the fallback.
///
/// The two generators are sequential and only the second one can produce
/// partial results, so an undivided budget is a budget the fallback never
/// sees: the enumeration would spend all of it, be abandoned for being
/// incomplete, and discovery would return nothing at all. Half is the split
/// because both halves have to be worth having -- enough enumeration time to
/// finish on the models that can (World3 enumerates in ~0.4 s), and enough
/// fallback time to cover a useful prefix of the saved steps on the models
/// that cannot.
const ENUM_BUDGET_FRACTION: f64 = 0.5;

/// The two wall-clock deadlines a budgeted discovery run works against.
///
/// Separate rather than one, because the phases are not interchangeable: the
/// enumeration either finishes or is thrown away whole, while the fallback
/// yields real loops for every step it completes. So the enumeration gets a
/// fraction of the budget and the fallback gets the caller's full deadline --
/// what is left of it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Deadlines {
    /// `ActivityGraph::build`, `enumerate_active_circuits` and
    /// `retain_circuits` must all finish by this instant.
    pub(crate) enumeration: Option<Instant>,
    /// The fallback sweep's deadline: the caller's own budget expiry.
    pub(crate) fallback: Option<Instant>,
}

/// Split a caller's wall-clock `limit` into the two phase deadlines, measured
/// from `started`.
///
/// The enumeration path gets [`ENUM_BUDGET_FRACTION`] of the budget and the
/// fallback gets what remains of the whole of it -- so the two deadlines are
/// `started + fraction * limit` and `started + limit`, and the fallback's is
/// the caller's own expiry rather than a second slice.
fn split_budget(started: Instant, limit: Duration) -> Deadlines {
    Deadlines {
        enumeration: Some(started + limit.mul_f64(ENUM_BUDGET_FRACTION)),
        fallback: Some(started + limit),
    }
}

/// Run loop discovery using a pre-built `CausalGraph`.
///
/// This is the implementation shared by `discover_loops` (which builds
/// the graph from a `Project`) and callers that have a salsa-derived
/// `CausalGraph`.
///
/// When `ltm_vars` and `dims` are provided, A2A link scores are expanded
/// into per-element edges so discovery operates on the element-level graph.
/// When they are empty (convenience path), all link scores are treated as
/// scalar.
///
/// `sub_model_output_ports` maps each referenced sub-model's canonical name to
/// the sorted LTM output-port set it EMITTED its `$⁚ltm⁚path⁚{port}⁚{idx}` vars
/// against -- the same decision `db::ltm::sub_model_output_ports` makes on the
/// emission side. The per-exit-port recompute (GH #698) enumerates pathway
/// indices against this set, so the indices match the emitted vars
/// index-for-index regardless of which project model the loop lives in. Pass an
/// empty map to disable the recompute (every module-input edge then keeps its
/// composite base score, the pre-GH-#698 behavior).
///
/// `budget` optionally bounds the wall-clock time spent GENERATING
/// candidates: the activity-graph build, the enumeration, retention, and the
/// fallback sweep. It is split by [`ENUM_BUDGET_FRACTION`] between the
/// enumeration path and the fallback, and every phase of both checks it at a
/// bounded interval -- so even a model whose enumeration would run for hours
/// (GH #647) stops generating within roughly the budget, having spent the
/// remainder on a fallback sweep that yields real loops.
///
/// What follows candidate generation is NOT inside the budget: materializing
/// each candidate into a `FoundLoop` and `rank_and_filter` both run to
/// completion afterwards, so a run can exceed the budget by that tail (115 ms
/// of World3's 409 ms) and still report `truncated == false`. The budget is a
/// bound on the unbounded phases, not a wall-clock guarantee for the call.
/// `DiscoveryResult::truncated` records whether the fallback ran out of time;
/// `enumeration_complete` records which generator produced the candidates. A
/// `None` budget runs to completion and never reads the clock. The caller's
/// compilation and simulation time are outside the budget too.
// Each argument is a distinct backend-independent structural input the
// discovery sweep needs; they are not naturally groupable into one struct
// without obscuring the call sites, so the arity lint is suppressed here.
#[allow(clippy::too_many_arguments)]
pub fn discover_loops_with_graph(
    results: &Results,
    causal_graph: &CausalGraph,
    stocks: &[Ident<Canonical>],
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
    expansion: &LinkExpansionContext,
    sub_model_output_ports: &SubModelOutputPorts,
    budget: Option<Duration>,
) -> Result<DiscoveryResult> {
    discover_loops_with_candidate_gen(
        results,
        causal_graph,
        stocks,
        ltm_vars,
        dims,
        expansion,
        sub_model_output_ports,
        budget,
        CandidateGen::Auto,
    )
}

/// Which candidate-generation strategy `discover_loops_with_candidate_gen`
/// uses to find cycles worth scoring.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CandidateGen {
    /// Union-graph circuit enumeration first (provably complete when it
    /// finishes within its budgets), the shortest-path fallback after it. The
    /// production default.
    Auto,
    /// The shortest-path fallback only, under the named configuration -- how
    /// the evaluation harness measures a strategy's recall against the exact
    /// enumeration, and how a test exercises the fallback's semantics without
    /// having to defeat the enumerator's budgets.
    FallbackOnly(FallbackConfig),
}

/// [`discover_loops_with_graph`] with the candidate generator pinned.
#[allow(clippy::too_many_arguments)]
pub fn discover_loops_with_candidate_gen(
    results: &Results,
    causal_graph: &CausalGraph,
    stocks: &[Ident<Canonical>],
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
    expansion: &LinkExpansionContext,
    sub_model_output_ports: &SubModelOutputPorts,
    budget: Option<Duration>,
    candidate_gen: CandidateGen,
) -> Result<DiscoveryResult> {
    // Captured lazily so an unbudgeted run never reads the clock.
    let deadlines = match budget {
        Some(limit) => split_budget(Instant::now(), limit),
        None => Deadlines {
            enumeration: None,
            fallback: None,
        },
    };
    discover_loops_with_deadlines(
        results,
        causal_graph,
        stocks,
        ltm_vars,
        dims,
        expansion,
        sub_model_output_ports,
        deadlines,
        candidate_gen,
        &mut SystemClock,
    )
}

/// [`discover_loops_with_candidate_gen`] with the two phase deadlines and the
/// clock supplied directly rather than derived from a `Duration`.
///
/// The public entry point takes a budget, splits it, and reads the system
/// clock, which makes "the enumeration expired but the fallback still has
/// time, and then the fallback expired part way through" -- the state AC2.2 is
/// about -- reachable only by racing real time. This is the seam that lets a
/// test state it: pass an already-past `enumeration`, a live `fallback`, and a
/// clock scripted to expire after a known number of reads, and the
/// partial-results contract is a deterministic fact rather than a timing
/// accident.
#[allow(clippy::too_many_arguments)]
pub(crate) fn discover_loops_with_deadlines(
    results: &Results,
    causal_graph: &CausalGraph,
    stocks: &[Ident<Canonical>],
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
    expansion: &LinkExpansionContext,
    sub_model_output_ports: &SubModelOutputPorts,
    deadlines: Deadlines,
    candidate_gen: CandidateGen,
    clock: &mut dyn Clock,
) -> Result<DiscoveryResult> {
    // An empty candidate universe is trivially complete, but only the
    // enumeration path may SAY so: `enumeration_complete` names the generator
    // that ran, and a `FallbackOnly` caller (the evaluation harness, the
    // semantic tests) asserts the enumeration never claims to have run.
    let enumeration_ran = matches!(candidate_gen, CandidateGen::Auto);

    let link_offsets = parse_link_offsets(results, ltm_vars, dims, expansion);
    if link_offsets.is_empty() {
        // No link score was recorded. Discovery mode scores EVERY causal edge,
        // so a model with any edge at all and no recorded score was not
        // instrumented -- a run with LTM disabled, or a conveyor/queue model,
        // whose special-stock build does not participate in LTM -- and its
        // universe is UNKNOWN, not empty: nothing ran and the report says so
        // (`enumeration_complete == false`, no universe, no sweep). Only a
        // model with no causal edge has a universe that is empty by
        // construction, and only the enumeration path may say even that.
        let has_edges = causal_graph.edges.values().any(|tos| !tos.is_empty());
        let certified_empty = enumeration_ran && !has_edges;
        return Ok(DiscoveryResult {
            loops: Vec::new(),
            partitions: Vec::new(),
            truncated: false,
            agg_recovery_truncated: false,
            enumeration_complete: certified_empty,
            retained_loops: 0,
            // `universe_loops.is_some()` equals `enumeration_complete` on
            // every path out of this function.
            universe_loops: certified_empty.then_some(0),
            // Neither generator ran.
            fallback_candidates: None,
        });
    }

    // Build HashMap for O(1) link offset lookups during score computation
    let link_offset_map: LinkOffsetMap = link_offsets
        .iter()
        .map(|((from, to), offset)| ((from.clone(), to.clone()), *offset))
        .collect();

    // A model with no parent-level stocks is NOT an empty universe: the
    // enumerator needs no stock seeds at all (it walks every union-graph
    // root regardless), and the fallback's default seed policy
    // (`StocksAndStocklessSccs`) seeds one representative per non-trivial
    // stockless SCC directly. A 2+-node cycle whose only state is a
    // module-internal level or a `PREVIOUS` lag between auxes is therefore
    // still analyzed -- it resolves to no parent-level partition (every
    // `stock_partition_of_node`/`CyclePartitions::stock_partition` entry is
    // `None` when `causal_graph.stocks` is empty) and is reported in a
    // `NormGroup::Solo` group, ranked after every competing loop (AC1.3).
    // Bailing out here used to declare that universe empty by construction
    // rather than letting the pipeline discover it has nothing to partition.

    let step_count = results.step_count;

    // Hoist the integer-indexed topology build out of the candidate search:
    // the graph's edges and result slots are step-invariant. Both candidate
    // generators run over this one structure.
    let mut search = IndexedSearch::build(&link_offsets, stocks);

    // Cycle partitions serve both the enumeration retention pass
    // (full-universe denominators) and the final ranking; compute once.
    let partitions = causal_graph.compute_cycle_partitions();

    // Candidate cycles as `IndexedSearch` node-id paths, whichever generator
    // produced them; materialized into `Ident` paths once, below.
    let mut node_paths: Vec<Vec<u32>> = Vec::new();
    let mut truncated = false;
    let mut enumeration_complete = false;
    // Set alongside `enumeration_complete` and only there, so the pair is one
    // statement: `Some(n)` iff the enumeration ran to completion and its
    // universe holds `n` circuits.
    let mut universe_loops: Option<usize> = None;
    // Retained loops retention deliberately did not hand over for
    // materialization (Solo loops past the strongest `max_loops()`), so the
    // reported `retained_loops` still counts every loop that passed.
    let mut retained_beyond_materialization: usize = 0;
    let mut agg_recovery_truncated = false;
    // Full-universe per-partition denominators and loop counts from the
    // enumeration path (`None` on the fallback path, where `rank_and_filter`
    // measures against the discovered set instead -- a sample has no universe
    // to offer).
    let mut universe: Option<UniverseStats> = None;
    // Set alongside `enumeration_complete == false`: how many distinct cycles
    // the fallback sweep proposed, for `DiscoveryResult::fallback_candidates`.
    let mut fallback_candidates: Option<usize> = None;

    // Shared by retention (below, via the `override_for` closure the enum
    // module's `ModuleOverrideFn` names) and `FoundLoop` materialization
    // (further down): ONE cache, so the two phases can never resolve a
    // module-traversing loop's score to different series. Built regardless
    // of whether the enumeration runs -- materialization needs it on the
    // fallback path too.
    // The one memory bound every phase below charges its allocations to.
    let meter = MemoryMeter::new();
    let mut module_override_cache = ModuleOverrideCache::new(
        causal_graph,
        results,
        sub_model_output_ports,
        step_count,
        &meter,
    );

    // Let both candidate generators read a module-input edge's activity
    // through its pathways rather than only its composite: the composite's
    // max-abs fold lets a NaN pathway shadow a finite one, and a cycle whose
    // per-exit-port override is finite there must not be dropped as inactive
    // before retention ever scores it (that shadow was this pipeline's one
    // stated blind spot; `IndexedEdge::value_at` closes it). The pass is a
    // prerequisite of BOTH generators, so it runs under the caller's whole
    // deadline (`deadlines.fallback`) rather than the enumeration's share: a
    // partially attached graph reads some module-input edges as inactive
    // where they are not, so neither generator may search one, and stopping
    // the pass at the enumeration's share would deny the fallback a graph it
    // still had time to search. If the pass itself exhausts the deadline (or
    // the memory meter refuses a slot list) there is no graph to search and
    // discovery reports an empty, truncated sample.
    let attach = search.attach_module_pathways(
        &mut |from, to| module_override_cache.pathway_offsets(from, to),
        deadlines.fallback,
        clock,
    );
    let pathways_attached = attach == AttachOutcome::Complete;
    if !pathways_attached {
        truncated = true;
    }

    // --- Primary candidate generation: union-graph circuit enumeration ---
    // (docs/design-plans/2026-08-17-ltm-discovery-exact.md).
    // Every loop with a nonzero score at some saved step has ALL its edges
    // active at that step (score is a product), so the ever-simultaneously-
    // active elementary cycles of the union graph are exactly the scorable
    // loop universe. Enumerating them once gives a provably complete candidate
    // set whenever the enumeration budgets and the deadline hold.
    //
    // The enumerated set is also the population every downstream statistic is
    // measured against: retention judges a circuit's peak share of its
    // partition's whole-universe mass, and `rank_and_filter` normalizes
    // relative scores against the same totals via its `universe` parameter
    // (`UniverseStats::totals`). Those totals are the FULL enumerated
    // universe's raw mass (retention
    // non-survivors included, GH #310) -- NOT the mass reported loops carry --
    // corrected below so that each distinct reported cycle contributes its
    // mass exactly once and by the series it reports: a module-traversing
    // loop's raw product is replaced by its per-exit-port override series
    // (they can differ by any factor), and a duplicate representative the
    // reported-cycle dedup discards has its raw mass subtracted back out.
    // Every other circuit -- retention non-survivors included -- keeps its
    // raw enumerated product in the totals unmodified.
    if enumeration_ran
        && pathways_attached
        && let Some(activity) =
            ActivityGraph::build(&search, results, deadlines.enumeration, clock, &meter)
    {
        let mut candidates =
            enumerate_active_circuits(&activity, deadlines.enumeration, clock, &meter);
        if candidates.complete {
            // Per-node metadata for the streaming retention passes.
            let stock_partition_of_node: Vec<Option<usize>> = search
                .idents
                .iter()
                .map(|ident| partitions.stock_partition.get(ident).copied())
                .collect();
            let is_module_node: Vec<bool> = search
                .idents
                .iter()
                .map(|ident| causal_graph.module_graph(ident).is_some())
                .collect();
            // Which nodes are synthetic `$⁚ltm⁚agg⁚{n}` aggregates -- needed
            // both by retention's own trimmed-key dedup (a circuit can only
            // have a twin that trims to the same reported loop if it visits
            // >= 1 of these) and, below, by the cross-agg petal stitcher.
            // Computed once and shared, so a model with no agg node at all
            // (every scalar model) pays for exactly one `contains(&true)`
            // scan in either consumer rather than two.
            let is_agg_node: Vec<bool> = search
                .idents
                .iter()
                .map(|ident| crate::ltm_agg::is_synthetic_agg_name(ident.as_str()))
                .collect();
            // Adapts the enum module's node-id `ModuleOverrideFn` shape to
            // `module_override_cache`'s ident-keyed lookup. Scoped to this
            // block (rather than captured by `move`) so the mutable borrow it
            // holds on the cache ends here, freeing the cache for
            // materialization to borrow again, unaliased, below.
            // A memory refusal from the override cache is recorded here and
            // read after retention: the circuit it answered was scored on
            // its composite, which is the wrong number, so the whole pass is
            // abandoned exactly as an expired deadline abandons it.
            let override_oom = std::cell::Cell::new(false);
            let mut override_for =
                |from_node: u32, module_node: u32, next_node: u32| match module_override_cache
                    .series(
                        &search.idents[from_node as usize],
                        &search.idents[module_node as usize],
                        &search.idents[next_node as usize],
                    ) {
                    OverrideLookup::Resolved(series, window) => Some((series, window)),
                    OverrideLookup::Declined => None,
                    OverrideLookup::OutOfMemory => {
                        override_oom.set(true);
                        None
                    }
                };
            // Cross-agg stitching over the FULL enumerated set (GH #696), BEFORE
            // retention: a petal can fail retention while a stitched
            // combination passes, and a stitched loop is a candidate like any
            // other -- it joins `candidates` here so retention's trimmed-key
            // dedup, its bank/confirm gate, its universe count and its
            // survivor list all cover it uniformly, with nothing banked or
            // counted after the fact.
            //
            // Only a circuit visiting EXACTLY ONE synthetic agg node can be a
            // petal, and `collect_agg_petals` needs node paths rather than the
            // edge rows the enumerator emits, so the node paths are
            // materialized for those circuits alone -- on a model carrying no
            // agg node at all (every scalar model) that is none of them.
            // Pre-filtering changes nothing `collect_agg_petals` would keep:
            // it drops the same circuits itself, in the same order. The agg
            // count is taken off the edge rows via `edge_source` (O(1), no
            // allocation) so `circuit_nodes` -- which allocates a `Vec` -- is
            // called only for the circuits that pass the count test.
            // The node paths are produced one at a time, by an iterator the
            // collector consumes, so at no point is the universe's worth of
            // them held at once; the collector keeps a bounded few per agg.
            let has_agg_node = is_agg_node.contains(&true);
            let petal_circuits = (0..candidates.len()).filter(|&ci| {
                has_agg_node
                    && candidates
                        .circuit(ci)
                        .iter()
                        .filter(|&&row| is_agg_node[activity.edge_source(row) as usize])
                        .count()
                        == 1
            });
            let stitching_over_budget = match stitch_cross_agg_node_paths(
                &search,
                petal_circuits.map(|ci| activity.circuit_nodes(candidates.circuit(ci))),
                &meter,
            ) {
                Some(stitched) => {
                    agg_recovery_truncated = stitched.truncated;
                    // Stitched loops are charged against the enumeration's
                    // storage like any circuit; an addition that would exceed
                    // it makes the enumeration incomplete (the fallback runs),
                    // never a larger allocation than the bound allows. The
                    // stitcher's own output charge is released once the
                    // sequences have been pushed (or refused).
                    let over = stitched
                        .paths
                        .iter()
                        .any(|seq| !candidates.push_node_path(seq, &activity, &meter));
                    meter.release(stitched.output_charge);
                    over
                }
                // The meter refused the petal collection or the stitched
                // output itself: nothing was stitched, and the enumeration
                // cannot vouch for a complete universe.
                None => true,
            };
            // Petal collection and stitching are bounded by the enumeration's
            // own budgets (the scan is one pass over the emitted rows, the
            // stitch by `cross_agg_loop_budget`), but a reducer-heavy universe
            // can still spend real time here before retention's first check;
            // read the clock once so an expired enumeration deadline yields to
            // the fallback with its share intact rather than after this work.
            let stitching_expired =
                stitching_over_budget || (has_agg_node && expired(deadlines.enumeration, clock));

            if stitching_expired {
                // Fall through to the fallback exactly as an incomplete
                // enumeration does.
            } else if let Some(retention) = retain_circuits(
                &candidates,
                &activity,
                &stock_partition_of_node,
                &is_module_node,
                &is_agg_node,
                &mut override_for,
                deadlines.enumeration,
                clock,
            ) && !override_oom.get()
            {
                let survivors: Vec<Vec<u32>> = retention
                    .survivors
                    .iter()
                    .map(|&ci| activity.circuit_nodes(candidates.circuit(ci)))
                    .collect();
                // `universe_loops` is the number of DISTINCT loops whose mass
                // the partition denominators sum -- enumerated circuits and
                // stitched cross-agg loops alike, minus the twins retention's
                // trimmed-key dedup merged and minus anything that banked no
                // mass; retention counts exactly that as it goes.
                let universe_loop_count = retention.distinct_circuits;
                // The report's own storage is charged before it is built: a
                // survivor set whose materialized series would not fit the
                // discovery memory bound is an enumeration this bound cannot
                // carry to a report, so it yields to the (bounded) fallback and
                // says so through `enumeration_complete`.
                let report_bytes: usize = survivors
                    .iter()
                    .map(|nodes| materialized_loop_bytes(step_count, nodes.len()))
                    .sum();
                if meter.charge(report_bytes) {
                    retained_beyond_materialization = retention.solo_survivors_beyond_cap;
                    node_paths = survivors;
                    universe = Some(UniverseStats {
                        totals: retention.partition_totals,
                        loop_counts: retention.partition_circuit_counts,
                    });
                    enumeration_complete = true;
                    universe_loops = Some(universe_loop_count);
                }
            }
        }
        if !enumeration_complete {
            // The graph and the candidate store are dropped at the end of
            // this block; credit their charge so the fallback is measured
            // against the same bound rather than against the abandoned
            // attempt's footprint.
            meter.release(activity.charged_bytes + candidates.charged_bytes);
        }
        // An incomplete enumeration (circuit budget, visit budget, edge-row
        // budget, memory bound, or deadline) falls through to the fallback
        // with whatever wall-clock remains; the partial circuit list is
        // discarded rather than merged, because it is biased by node-id root
        // order and its per-partition totals are not the universe's.
    }

    if !enumeration_complete && pathways_attached {
        // --- Fallback candidate generation: per (seed, step) shortest cycles
        // (`ltm_finding_fallback.rs`). `CandidateGen::Auto` uses the default
        // configuration; `FallbackOnly` names its own.
        let config = match candidate_gen {
            CandidateGen::Auto => FallbackConfig::DEFAULT,
            CandidateGen::FallbackOnly(config) => config,
        };
        let outcome = fallback::sweep(&search, results, config, deadlines.fallback, clock, &meter);
        truncated = outcome.truncated;
        debug_assert!(
            outcome.truncated || outcome.steps_processed == step_count.saturating_sub(1),
            "an untruncated sweep covers every saved step after step 0"
        );
        // Before stitching: a stitched loop is a COMBINATION of proposed
        // cycles rather than one of them, mirroring `universe_loops` counting
        // only the enumerated circuits and not the stitched additions.
        fallback_candidates = Some(outcome.paths.len());

        // Stitch cross-element-through-aggregate loops (GH #696) through the
        // SAME helper the enumeration path uses. Both generators emit only
        // ELEMENTARY cycles, so a feedback loop that traverses a hoisted
        // reducer's synthetic agg node more than once
        // (`pop[a] -> agg -> pop[b] -> agg -> pop[a]`) is structurally
        // unreachable to either, and exhaustive mode's petal stitching is what
        // recovers it in both.
        let stitched = stitch_cross_agg_node_paths(&search, outcome.paths.iter(), &meter);
        node_paths = outcome.paths;
        match stitched {
            Some(stitched) => {
                agg_recovery_truncated = stitched.truncated;
                let output_charge = stitched.output_charge;
                let fresh = new_paths_by_rotation(&node_paths, stitched.paths);
                // Each stitched loop is charged like a kept path (its node ids
                // plus the series it will materialize); one that does not fit
                // ends the additions, and the cross-aggregate recovery is
                // reported incomplete. The stitcher's own output charge is
                // released once the sequences are re-charged as kept paths.
                for seq in fresh {
                    let bytes = std::mem::size_of_val::<[u32]>(&seq)
                        + materialized_loop_bytes(step_count, seq.len());
                    if !meter.charge(bytes) {
                        agg_recovery_truncated = true;
                        break;
                    }
                    node_paths.push(seq);
                }
                meter.release(output_charge);
            }
            // The meter refused the petal collection or the stitched output:
            // the cross-aggregate recovery did not happen.
            None => agg_recovery_truncated = true,
        }
    }

    let all_paths: Vec<Vec<Ident<Canonical>>> = node_paths
        .iter()
        .map(|path| {
            path.iter()
                .map(|&n| search.idents[n as usize].clone())
                .collect()
        })
        .collect();

    if all_paths.is_empty() {
        return Ok(DiscoveryResult {
            loops: Vec::new(),
            partitions: Vec::new(),
            truncated,
            agg_recovery_truncated,
            enumeration_complete,
            retained_loops: 0,
            universe_loops,
            fallback_candidates,
        });
    }

    // Convert paths to FoundLoop objects with scores.
    let mut found_loops: Vec<FoundLoop> = Vec::new();

    'paths: for path in &all_paths {
        // Convert path to links using CausalGraph. These links carry the
        // un-trimmed per-element path -- they map to the synthetic
        // `$⁚ltm⁚link_score⁚...` variables emitted during compilation, so the
        // loop-score offset lookups below need them as-is. The synthetic
        // aggregate nodes are trimmed only from the *reported* loop (below).
        let links = causal_graph.circuit_to_links(path);
        let loop_stocks = causal_graph.find_stocks_in_loop(path);

        // Precompute the results offset for each link in this loop, avoiding
        // repeated HashMap lookups and Ident clones in the per-timestep inner loop.
        let mut link_result_offsets: Vec<usize> = Vec::with_capacity(links.len());
        for link in &links {
            let offset = link_offset_map
                .get(&(link.from.clone(), link.to.clone()))
                .ok_or_else(|| crate::common::Error {
                    kind: crate::common::ErrorKind::Model,
                    code: crate::common::ErrorCode::NotSimulatable,
                    details: Some(format!(
                        "Link score variable not found for {} -> {}. \
                         The simulation may not have been compiled with ltm_discovery_mode enabled.",
                        link.from.as_str(),
                        link.to.as_str()
                    )),
                })?;
            link_result_offsets.push(*offset);
        }

        // Per-exit-port override series for module-input edges (GH #698). For a
        // loop edge `x → m` whose next edge `m → y` identifies the exit port the
        // loop reads, recompute that edge's link-score series from the
        // sub-model's per-pathway scores selecting only the pathway(s) ending at
        // that port -- mirroring the exhaustive-mode override. The base offset
        // (the module *composite*, which max-abs-selects across ALL ports and so
        // can pick a wrong-signed port for a multi-output module) is used
        // verbatim everywhere this returns `None`. `module_override_cache` is
        // the SAME instance retention consulted above (for the enumeration
        // path), so a survivor's materialized score matches the mass it
        // already banked -- and memoizes across every loop, so an arrayed
        // loop through a module hits one cache entry per element rather than
        // re-enumerating the sub-model's pathway map per link.
        let mut link_override_series: Vec<Option<Rc<Vec<f64>>>> = Vec::with_capacity(links.len());
        for i in 0..links.len() {
            let n = links.len();
            let next = &links[(i + 1) % n];
            // Gate on the module instance before spelling a cache key: on a
            // model with no modules -- most of them -- this is the only work
            // the recompute costs per link.
            let module_name =
                Ident::<Canonical>::new(crate::ltm::strip_subscript(links[i].to.as_str()));
            // No module instance at `links[i].to` at all: the cache's own
            // gate (`module_graph(&module_name)?`, inside the engine it
            // calls) would decline for the identical reason, so answer
            // `None` directly instead of re-stripping/re-interning the same
            // names only to reach the same `?` immediately.
            if causal_graph.module_graph(&module_name).is_none() {
                link_override_series.push(None);
                continue;
            }
            let lookup = if next.from != links[i].to {
                // A link list not spelled sequentially is outside what the
                // cache's key assumes, so it is answered uncached through the
                // links-slice adapter (same pre-charge, same decline rule).
                module_override_cache.uncached_series(&links, i)
            } else {
                // Materialization scores the full saved-step range regardless
                // (it never restricts to a window), so only the series itself
                // is needed here -- the cached active window exists for
                // retention's benefit, not this call site's.
                module_override_cache.series(&links[i].from, &module_name, &next.to)
            };
            match lookup {
                OverrideLookup::Resolved(series, _window) => {
                    link_override_series.push(Some(series))
                }
                OverrideLookup::Declined => link_override_series.push(None),
                OverrideLookup::OutOfMemory => {
                    // The override this loop's score needs does not fit the
                    // bound; reporting it on the composite instead would be a
                    // wrong number, so the loop is dropped and the report says
                    // it is partial. (Unreachable on the enumeration path:
                    // retention resolved -- and cached -- every module edge of
                    // every survivor before materialization began.)
                    debug_assert!(!enumeration_complete);
                    truncated = true;
                    continue 'paths;
                }
            }
        }

        // Compute signed loop score at each timestep.
        // Time is derived from specs assuming evenly-spaced results at save_step intervals.
        let mut scores: Vec<(f64, f64)> = Vec::new();
        // Running (Welford) mean of |score| over the valid steps -- the same
        // formula retention uses (`enum_gen::mean_abs_over_valid`), so a
        // twin's representative and a Solo loop's rank are decided on the
        // statistic the materialized loop reports, and a sum of large finite
        // products cannot overflow where their mean is representable.
        let mut abs_mean = 0.0f64;
        let mut valid_count = 0usize;
        let mut abs_any_inf = false;

        for step in 0..step_count {
            let time = results.specs.start + results.specs.save_step * (step as f64);

            // Compute signed loop score = product of signed link scores
            let mut loop_score = 1.0;
            let mut has_nan = false;

            for (i, &offset) in link_result_offsets.iter().enumerate() {
                let value = match &link_override_series[i] {
                    Some(series) => series[step],
                    None => results.data[step * results.step_size + offset],
                };
                if value.is_nan() {
                    has_nan = true;
                    break;
                }
                loop_score *= value;
            }

            // A NaN PRODUCT with no NaN link (`Inf * 0`) is excluded from the
            // mean exactly as a NaN link is -- the same rule retention's
            // product-derived NaN mask applies.
            if has_nan || loop_score.is_nan() {
                scores.push((time, f64::NAN));
            } else {
                scores.push((time, loop_score));
                if loop_score.is_infinite() {
                    abs_any_inf = true;
                } else {
                    valid_count += 1;
                    abs_mean += (loop_score.abs() - abs_mean) / valid_count as f64;
                }
            }
        }

        let avg_abs_score = if abs_any_inf { f64::INFINITY } else { abs_mean };

        // Trim synthetic aggregate nodes out of the reported loop (AC4.2).
        // The loop scores above were computed from the un-trimmed `links`; the
        // structural polarity is (re-)derived from the trimmed chain so the
        // negative-link count matches what we report. A loop made up entirely
        // of synthetic agg nodes has nothing left to report and is dropped.
        let Some(reported_links) = trim_synthetic_aggs_from_loop_links(&links) else {
            continue;
        };
        // Defense in depth for AC1.1 (no reported loop has a single link): a
        // 2-node `agg -> x -> agg` circuit trims to a single `x -> x`
        // self-link (the wraparound arm of `trim_synthetic_aggs_from_loop_links`
        // merges both edges into one). No compiling model reaches this today
        // -- the enumerator never emits a length-1 circuit and the fallback's
        // cycle recovery is likewise elementary -- but nothing upstream of
        // this call proves a trim can never collapse further than two nodes,
        // so a defensive length check is cheaper than an invariant nothing
        // enforces.
        if reported_links.len() < 2 {
            continue;
        }
        let polarity_structural = causal_graph.calculate_polarity(&reported_links);

        // Determine runtime polarity from scores, capturing the confidence
        // ratio alongside it (GH #495). When the loop has no valid runtime
        // scores we fall back to the structural polarity; the matching
        // confidence mirrors the structural pipeline's convention in
        // `db::analysis` (1.0 when the polarity is determined, 0.0 when it is
        // Undetermined) so the discovery and structural surfaces agree on what
        // a "fully confident" loop reports.
        let runtime_scores: Vec<f64> = scores.iter().map(|(_, s)| *s).collect();
        let (polarity, polarity_confidence) = LoopPolarity::from_runtime_scores(&runtime_scores)
            .unwrap_or_else(|| {
                let confidence = if polarity_structural == LoopPolarity::Undetermined {
                    0.0
                } else {
                    1.0
                };
                (polarity_structural, confidence)
            });

        let loop_info = Loop {
            id: String::new(), // Will be assigned below
            links: reported_links,
            stocks: loop_stocks,
            polarity,
            dimensions: vec![],
            slot_links: vec![],
        };

        found_loops.push(FoundLoop {
            loop_info,
            scores,
            avg_abs_score,
            // Filled in once partition denominators are known (rank_truncate_and_id).
            rel_scores: Vec::new(),
            // Filled in by attach_partition_metadata at the end of ranking.
            partition: None,
            polarity_confidence,
        });
    }

    // Two distinct discovered cycles can trim to the same *reported* loop: a
    // direct `pop[d] -> share[d]` numerator path and the
    // `pop[d] -> $⁚ltm⁚agg⁚n -> share[d]` aggregate path differ only in the
    // synthetic agg node, which the report hides. Keep one representative per
    // reported link cycle -- the strongest (highest average |score|) --
    // matching the composite-link-score rule (LTM ref 6.3): when several
    // pathways collapse onto one reported link, the reported magnitude
    // follows the dominant pathway. The kept loop's score series is that one
    // pathway's product at every step (no per-step path flipping).
    let mut by_reported_cycle: HashMap<Vec<String>, usize> = HashMap::new();
    let mut deduped: Vec<FoundLoop> = Vec::new();
    let mut dropped: Vec<FoundLoop> = Vec::new();
    for entry in found_loops {
        let nodes: Vec<String> = entry
            .loop_info
            .links
            .iter()
            .map(|l| l.from.as_str().to_string())
            .collect();
        let key = crate::ltm::canonical_rotation(&nodes);
        match by_reported_cycle.get(&key) {
            Some(&idx) => {
                if entry.avg_abs_score > deduped[idx].avg_abs_score {
                    dropped.push(std::mem::replace(&mut deduped[idx], entry));
                } else {
                    dropped.push(entry);
                }
            }
            None => {
                by_reported_cycle.insert(key, deduped.len());
                deduped.push(entry);
            }
        }
    }

    // Subtract a dropped reported-cycle duplicate's mass back out of its
    // partition's universe total.
    //
    // On the enumeration path `stats.totals` already banks every retained
    // loop's REPORTED mass up front: `retain_circuits` scores every
    // non-Solo circuit -- module-traversing ones included -- through the
    // SAME `ModuleOverrideCache` materialization just consulted above, and
    // `accumulate_series_into_totals` does the same for stitched cross-agg
    // loops. So a duplicate the dedup above discarded is mass that is
    // definitely still in the totals (nothing upstream of `by_reported_cycle`
    // ever excludes a circuit's own reported series from what it banks), and
    // it needs to come back out: the kept representative reports that cycle,
    // and left counted, a partition whose entire universe is one reported
    // loop and its trimmed twin would read as COMPETING while its
    // denominator is that loop's own mass -- the +/-1 relative score that
    // follows by construction would outrank the loops that genuinely divide
    // a denominator, which is the degeneracy the solo demotion exists to
    // prevent.
    //
    // This is a SAFETY NET, not the primary path: `retain_circuits`' own
    // trimmed-key dedup (`ltm_finding_enum.rs`'s `dedup_trimmed_twins`) now
    // decides the representative among ENUMERATED circuits -- module-
    // traversing ones included -- before any mass reaches a partition total,
    // and an edge row is a complete identity for a `(from, to)` pair, so two
    // DISTINCT non-agg enumerated circuits can never share a node sequence
    // either. So a hit here on the enumeration path should fire only for a
    // STITCHED cross-agg loop (assembled from petals AFTER retention runs,
    // never part of its grouping) colliding with another reported loop's
    // trimmed identity. Kept as a runtime safety net rather than asserted
    // away -- `subtract_reported_mass_from_totals`'s own `debug_assert!`
    // (the partition must already carry a total) is what would catch this
    // reasoning missing a case: if a future change makes it fire for a
    // non-stitched duplicate, that failure is telling you `dedup_trimmed_twins`
    // missed a collision class, not that this function is wrong.
    if let Some(stats) = universe.as_mut() {
        // On the enumeration path every candidate -- enumerated circuits and
        // stitched cross-agg loops alike -- went through retention's trimmed-
        // key dedup before banking mass, so nothing reaching this point can
        // still be a duplicate: a hit here means that dedup missed a collision
        // class, and a debug build says so rather than quietly correcting.
        debug_assert!(
            dropped.is_empty(),
            "a reported-cycle duplicate survived retention's trimmed-key dedup"
        );
        for fl in &dropped {
            subtract_reported_mass_from_totals(fl, &partitions, &mut stats.totals);
            subtract_reported_loop_from_counts(fl, &partitions, &mut stats.loop_counts);
        }
        // A duplicate discarded here is one fewer DISTINCT loop in the
        // universe the denominators now sum, so the count reported to
        // callers moves with the totals it describes.
        if let Some(n) = universe_loops.as_mut() {
            *n = n.saturating_sub(dropped.len());
        }
    }

    let mut found_loops = deduped;

    let ranked = rank_and_filter(&mut found_loops, &partitions, universe.as_ref());

    Ok(DiscoveryResult {
        loops: found_loops,
        partitions: ranked.partitions,
        truncated,
        agg_recovery_truncated,
        enumeration_complete,
        retained_loops: ranked.retained_loops + retained_beyond_materialization,
        universe_loops,
        fallback_candidates,
    })
}

/// The engine-internal cycle partition a discovered loop normalizes against,
/// or `None` for a loop whose stocks resolve to none (a `NormGroup::Solo`
/// loop, which is its own denominator and takes no share of a partition
/// total).
///
/// Discovered loops are always scalar, so `partition_for_loop` returns a
/// length-1 vector; collapse it to slot 0, exactly as `rank_and_filter` does.
fn loop_partition_slot(fl: &FoundLoop, partitions: &CyclePartitions) -> Option<usize> {
    partitions
        .partition_for_loop(&fl.loop_info, &[])
        .first()
        .copied()
        .flatten()
}

/// The enumerated universe's per-partition statistics: the population every
/// discovered loop is measured against.
///
/// The two fields are two views of ONE population, keyed by engine-internal
/// cycle partition, but the correspondence between them is NOT exact:
/// `loop_counts[p]` counts every enumerated circuit that resolves to
/// partition `p`, while `totals[p]` sums only the mass those circuits banked
/// at enumeration time, and a circuit can be counted while banking less than
/// its full share -- or none at all. A module-traversing circuit is kept
/// unconditionally but contributes NO raw mass here (its reported score is
/// the per-exit-port override series, added only after materialization, and
/// not at all if it never produces a reported loop -- e.g. its synthetic-agg
/// nodes trim it to fewer than two links). A circuit whose activity window
/// somehow ends up empty (guarded defensively even though the enumerator is
/// supposed never to emit one) is likewise counted with zero mass. Nothing
/// here removes a circuit from `loop_counts` once it is counted, so the
/// mismatch runs only one way: `loop_counts[p]` can overstate the population
/// `totals[p]` actually reflects, never understate it. That is the
/// conservative direction for the competing-vs-solo classification (see
/// [`rank_and_filter`]) -- it can inflate a partition to COMPETING that a
/// mass-only accounting would call closer to solo, but it can never hide a
/// genuinely competing partition as solo, which is the degeneracy the
/// classification exists to prevent.
///
/// That population is the full enumerated universe (retention non-survivors
/// included -- their mass is in the denominator whether or not they are
/// reported), plus the stitched cross-agg loops that join the candidate set
/// after retention, minus the reported-cycle duplicates whose mass is taken
/// back out.
///
/// Only the enumeration path has a universe to describe. The fallback samples
/// the runtime graph, so it passes `None` and `rank_and_filter` measures
/// against the discovered set instead.
struct UniverseStats {
    /// Per-partition per-step `Sum_j |score_j[t]|` over the whole population
    /// (`NaN` summands excluded, `Inf` kept -- `ltm_post::denom_summand`'s
    /// rule). Not every circuit `loop_counts` counts contributed to this sum
    /// -- see the struct doc's boundary note.
    totals: HashMap<usize, Vec<f64>>,
    /// Per-partition count of circuits in the enumerated universe that
    /// resolve to it. Two or more means the partition is COMPETING: its loops
    /// divide a denominator none of them owns outright (see
    /// [`rank_and_filter`]). Counts every such circuit, whether or not it
    /// banked mass into `totals` -- see the struct doc's boundary note.
    loop_counts: HashMap<usize, usize>,
}

/// Drop a loop from its partition's universe count.
///
/// The count's meaning is "how many loops' mass is in this partition's total",
/// so a loop whose mass is not (or is no longer) there must not be counted.
/// The partition necessarily has an entry: retention counted this circuit when
/// it banked (or deliberately withheld) its mass, through the same
/// `CyclePartitions` this resolves against.
fn subtract_reported_loop_from_counts(
    fl: &FoundLoop,
    partitions: &CyclePartitions,
    loop_counts: &mut HashMap<usize, usize>,
) {
    let Some(part) = loop_partition_slot(fl, partitions) else {
        return;
    };
    match loop_counts.get_mut(&part) {
        Some(count) if *count > 0 => *count -= 1,
        _ => debug_assert!(
            false,
            "a dropped duplicate's partition ({part}) must carry a nonzero \
             loop count: retention counted this circuit when it saw it"
        ),
    }
}

/// Remove a loop's |score| series from its partition's universe total.
///
/// Used for a duplicate representative the reported-cycle dedup discarded: its
/// mass was banked -- by `retain_circuits` (module-traversing circuits
/// included, via the same `ModuleOverrideCache` materialization uses) or by
/// `accumulate_series_into_totals` for a stitched cross-agg loop -- and no
/// loop reports it. The partition necessarily has a total (this loop's own
/// mass built it), and the result cannot go negative, since a float
/// subtraction of a summand from a sum of non-negative terms is bounded below
/// by zero -- except at a step whose running total is already `Inf` (a
/// dominance inflection, kept by convention rather than excluded like a NaN
/// summand), where the subtraction is skipped entirely: `Inf - Inf` is `NaN`
/// whenever the dropped duplicate's own score is ALSO infinite there, and
/// poisoning the total with NaN would be strictly worse than leaving one
/// duplicate's mass in an already-divergent step.
///
/// The partition here is derived via `loop_partition_slot` (`partition_for_loop`
/// over `loop_info.stocks`); the mass being removed was banked via
/// `stock_partition_of_node` over the enumerated circuit's node ids
/// (`retain_circuits`/`accumulate_series_into_totals`). Both read the same
/// `CyclePartitions`, computed once per discovery call
/// (`causal_graph.compute_cycle_partitions()`), so the two routes cannot
/// disagree about which stock resolves to which partition -- the
/// `debug_assert!`s below are what would catch it if they ever did.
fn subtract_reported_mass_from_totals(
    fl: &FoundLoop,
    partitions: &CyclePartitions,
    totals: &mut HashMap<usize, Vec<f64>>,
) {
    let Some(part) = loop_partition_slot(fl, partitions) else {
        return;
    };
    let Some(series) = totals.get_mut(&part) else {
        debug_assert!(
            false,
            "a duplicate representative's partition ({part}) must already \
             carry a total: its own raw mass built it during retention"
        );
        return;
    };
    for (t, &(_, score)) in fl.scores.iter().enumerate() {
        if score.is_nan() {
            continue;
        }
        if series[t].is_infinite() {
            // At a dominance inflection the total is `Inf` by convention (a
            // real divergent signal, kept rather than excluded like a NaN
            // summand). `Inf - finite == Inf`, so leaving it alone would be
            // harmless on its own, but a step where the DROPPED duplicate's
            // own score is ALSO infinite computes `Inf - Inf == NaN`, which
            // would poison the total for every sibling loop at that step for
            // the rest of the run -- exactly the failure mode
            // `an_inf_times_zero_product_is_excluded_from_totals_and_retention`
            // pins for pass 1. Skipping the subtraction here keeps the
            // convention: an infinite total stays `Inf`, never becomes NaN.
            continue;
        }
        series[t] -= score.abs();
        // `!(v < 0.0)` rather than `v >= 0.0`: the two differ only on
        // NaN, and NaN is exactly what this assert must NOT flag -- it is
        // ruled out separately by the `!score.is_nan()` guard above, so a
        // NaN reaching here would be a different bug this assert is not
        // the right place to report.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        let non_negative = !(series[t] < 0.0);
        debug_assert!(
            non_negative,
            "subtracting a duplicate representative's mass must not \
             drive partition {part}'s total negative at step {t}: it \
             removes exactly the bit pattern that was added for this \
             circuit, so the result is zero or positive up to floating \
             rounding"
        );
    }
}

/// Mean magnitude of a loop's relative loop score over the steps where it is
/// active -- the partition-relative importance statistic (GH #543).
///
/// `totals[t]` is the loop's cycle-partition denominator at step `t` (the sum
/// of `|loop_score_j|` over the partition, `NaN` excluded). The loop is *active*
/// at step `t` iff its own `score[t]` is non-`NaN` and `totals[t] > 0`; the mean
/// is taken only over active steps ("delayed averaging", ref 13.3). A loop with
/// no active steps returns `NaN` (it sorts last). `Inf/Inf = NaN` at a
/// dominance inflection is naturally excluded since `NaN` is not active.
fn mean_relative_contribution(fl: &FoundLoop, totals: &[f64]) -> f64 {
    let mut sum = 0.0;
    let mut active = 0usize;
    for (i, &(_, score)) in fl.scores.iter().enumerate() {
        let total = totals[i];
        // Active step = own score defined AND partition has activity. A `total`
        // of 0.0 means no loop in the partition is active (SAFEDIV-0 -> skip);
        // a +Inf total is real activity at a dominance inflection and is kept.
        // `total` never carries NaN -- partition totals exclude NaN summands --
        // so `total > 0.0` cleanly separates "inactive" from "active". The
        // negated form is deliberate (it states the *skip* condition as "not
        // active"); `total <= 0.0` would silently differ if a NaN ever leaked
        // in.
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        let inactive = score.is_nan() || !(total > 0.0);
        if inactive {
            continue;
        }
        let rel = score.abs() / total;
        // rel is in [0, 1] for a finite score (the loop's own |score| is part of
        // total). An Inf score makes total Inf, so rel == Inf/Inf == NaN and the
        // step drops out here; guard against any residual NaN to be safe.
        if rel.is_nan() {
            continue;
        }
        sum += rel;
        active += 1;
    }
    if active == 0 {
        f64::NAN
    } else {
        sum / active as f64
    }
}

/// The partition-relative importance statistic of one discovered loop, used
/// as the ranking and truncation key (GH #543).
///
/// `mean_rel` is the **mean magnitude of the loop's relative loop score** over
/// the steps where it is active -- `mean_t(|score[t]| / partition_total[t])`,
/// the literature's loop-inclusion measure ("average magnitude of the relative
/// loop score across the simulation period"; docs/reference 13.3).
///
/// `competing` is whether the loop shares its cycle partition with at least
/// one other DISCOVERED loop.  A loop that is trivially alone in its partition
/// has relative score exactly `±1` at every active step *by construction*
/// (its own `|score|` is the whole denominator), so its `mean_rel` of `1.0`
/// carries zero discriminative information -- on large real models (C-LEARN)
/// dozens of isolated two-variable stock-decay loops would otherwise pin the
/// top of the ranking above the loops that genuinely compete for dominance.
/// Competing loops therefore rank before solo loops regardless of `mean_rel`.
///
/// The secondary `key` is the loop's content-derived sort key (canonical edge
/// sequence) for deterministic tie-breaking; it never falls back to input
/// order.
struct RelativeImportance {
    mean_rel: f64,
    competing: bool,
    /// The loop's normalization group. Not part of the ORDER -- the ranking
    /// compares loops across groups -- but the coverage-aware cap needs to
    /// know which loops divide one denominator, since being "the dominant loop
    /// at step t" is a statement within a group and nowhere else
    /// ([`select_reported`]).
    group: NormGroup,
    /// The loop's raw `avg_abs_score`, the tie-break BETWEEN equal
    /// `mean_rel` values. Solo loops all carry `mean_rel == 1.0` by
    /// construction, so without this the order among them -- and therefore
    /// WHICH solo loops survive `MAX_LOOPS` cap pressure -- fell through to
    /// the content key, i.e. to their names. Raw magnitude is the only
    /// meaningful comparison the solo class admits (their relative scores
    /// are all identically ±1), and it is inert for competing loops, whose
    /// `mean_rel` values essentially never tie exactly.
    avg_abs: f64,
    key: (String, Vec<String>),
}

/// Order two loops for ranking: active loops before never-active (`NaN`)
/// ones, competing loops before trivially-isolated (solo-partition) ones,
/// then descending mean relative importance, then descending raw magnitude,
/// then content-based tie-breaking.
///
/// The competing-first demotion is deliberate (see [`RelativeImportance`]):
/// a solo loop's `mean_rel` is `1.0` by construction, so comparing it against
/// competing loops' shares is a degenerate cross-partition comparison the
/// papers warn against (ref section 8).  Among competing loops the
/// paper-aligned mean-relative statistic is untouched.  A `NaN` `mean_rel`
/// (a loop never active in a non-degenerate partition) sorts last -- below
/// even solo loops, which at least transmitted something -- so it cannot
/// displace a real loop from the cap.
fn cmp_relative_importance(a: &RelativeImportance, b: &RelativeImportance) -> std::cmp::Ordering {
    // Active (non-NaN) first.
    let by_nan = a.mean_rel.is_nan().cmp(&b.mean_rel.is_nan());
    // Competing (true) before solo (false).
    let by_competing = b.competing.cmp(&a.competing);
    // Descending by mean_rel (both finite or both NaN here).
    let by_score = b
        .mean_rel
        .partial_cmp(&a.mean_rel)
        .unwrap_or(std::cmp::Ordering::Equal);
    // Descending raw magnitude breaks exact mean_rel ties -- load-bearing for
    // the solo class, where every loop's mean_rel is 1.0 (see the field doc).
    let by_magnitude = b
        .avg_abs
        .partial_cmp(&a.avg_abs)
        .unwrap_or(std::cmp::Ordering::Equal);
    by_nan
        .then(by_competing)
        .then(by_score)
        .then(by_magnitude)
        .then_with(|| a.key.cmp(&b.key))
}

/// Rank, filter, truncate, assign IDs, and attach partition metadata to
/// discovered loops.  Returns the result-scoped partition list (see
/// [`DiscoveredPartition`]).
///
/// Pipeline (GH #543, GH #310):
/// 1. Compute per-partition per-timestep totals (the sum of `|loop_score_j|`
///    over the loops sharing a cycle partition, `NaN` excluded -- the same
///    denominator the relative loop score uses, ref 4.4 / `ltm_post.rs`).
/// 2. Compute each loop's partition-relative importance: the mean magnitude of
///    its relative loop score over the steps where it is active.
/// 3. Apply the partition-aware `MIN_CONTRIBUTION` retention filter (peak
///    semantics, unchanged) -- BEFORE any global cap, so a loop dominant in a
///    small partition but globally low-magnitude is no longer dropped by a
///    truncate-before-filter (GH #310).
/// 4. Rank competitive-first: loops that share their partition with at least
///    one other discovered loop come first, ordered by the partition-relative
///    key (descending); loops trivially ALONE in their partition -- whose
///    relative score is `±1` at every active step by construction, carrying
///    zero discriminative information -- come after ALL competing loops (see
///    [`RelativeImportance`]).  Then truncate to `MAX_LOOPS` in that order,
///    so under cap pressure the zero-information solo loops are dropped
///    before any competing loop.  Ranking on the relative key rather than raw
///    `avg_abs_score` fixes the magnitude bias (GH #543): a partition-dominant
///    low-magnitude loop still outranks a non-dominant high-magnitude loop in
///    a busier partition, both in truncation and in the caller-facing
///    ordering -- provided both face competition.
/// 5. Assign deterministic polarity-based IDs (r1, b1, ...) -- the assigner is
///    order-independent (it sorts by a content key internally).
/// 6. Re-sort by the relative key so callers get loops ranked by
///    partition-relative importance.
/// 7. Attach result-scoped partition metadata: each surviving loop's
///    `partition` index (dense, first-appearance order) and the partition
///    list itself (stocks + returned-loop count).
///
/// **Ranking key choice (mean vs peak).** The retention filter (step 3) uses
/// the *peak* per-timestep relative contribution -- "did this loop ever
/// matter?" The *ranking* key (steps 2/4/6) uses the *mean* relative
/// contribution -- "how important is it overall?" The mean is the
/// literature-aligned loop-inclusion measure (docs/reference 13.3). These are
/// deliberately different statistics for two different questions.
///
/// **Active-step (delayed) averaging.** A loop is *active* at step `t` when its
/// own `score[t]` is non-`NaN` and `partition_total[t] > 0`. The mean is taken
/// only over active steps (skip, do not count-as-zero). This is the
/// literature's "delayed averaging: starts from the first instant the loop
/// becomes active" (ref 13.3) generalized to "any inactive step" -- counting an
/// inactive step as a 0 contribution would penalize a loop that is sharply
/// dominant for a brief window (the briefly-dominant loop the retention filter
/// is specifically built to keep), pushing it below a perpetually-mediocre
/// loop. A loop with no active steps gets `NaN`, which sorts last.
///
/// Because the mean is over each loop's *own* active-step set, a loop that
/// dominates a partition that is active for only a brief window ties one
/// that dominates an always-active partition: both have a mean relative
/// contribution near `1.0` over their respective active steps. This
/// cross-partition equivalence is by design -- the relative key measures
/// in-partition dominance, not how long the partition itself stays active.
///
/// **NaN/Inf handling** mirrors `ltm_post.rs::denom_summand` (GH #542) so the
/// two LTM paths agree: a `NaN` `score[t]` contributes nothing to a partition
/// total and that step is skipped in the loop's own mean; an `Inf` `score[t]`
/// stays in the partition total (a real dominance-inflection signal), so the
/// loop's own `Inf/Inf = NaN` step is skipped and dominated siblings see a
/// `finite/Inf = 0` contribution at that step.
///
/// The `partitions` argument can be variable-level or element-level. When the
/// discovery pipeline operates on an element-level graph the partitions are
/// element-level (e.g. `population[nyc]` is a distinct stock node) and loop
/// stocks are element-specific. The logic is partition-naming-agnostic -- it
/// compares each loop's score to the total within its partition regardless of
/// granularity.
///
/// **The universe (the enumeration path).** [`UniverseStats`], when provided,
/// describes the whole enumerated loop population per engine-internal
/// partition, and both of its fields replace a discovered-set statistic:
///
/// - `totals` replaces the internally-computed denominators for every
///   `NormGroup::Partition` group, so relative scores are normalized against
///   the whole universe's mass (retention non-survivors included) -- matching
///   exhaustive-mode semantics, where the enumerated set IS the universe --
///   instead of against only the loops in `found_loops`. Solo groups keep
///   their own-series totals either way.
/// - `loop_counts` decides competing-vs-solo: a partition is competing iff the
///   universe holds at least two loops there, however many survived retention
///   or the cap. That is sound precisely because it counts the loops whose
///   mass is IN the denominator: every enumerated circuit is ever-active by
///   construction, so a "co-member" cannot be a phantom that leaves the
///   survivor's relative score at ±1 while flipping it to competing -- the
///   degeneracy the solo demotion exists to prevent. A survivor sharing its
///   partition with a sub-threshold sibling has a relative score strictly
///   below 1, which is real information, and demoting it below loops whose ±1
///   is pure construction would bury it.
///
/// The fallback path passes `None` and both statistics come from the
/// discovered set: a sample has no universe to describe, and the loops it
/// found are the only population there is.
fn rank_and_filter(
    found_loops: &mut Vec<FoundLoop>,
    partitions: &CyclePartitions,
    universe: Option<&UniverseStats>,
) -> RankOutcome {
    let step_count = found_loops.first().map_or(0, |l| l.scores.len());
    debug_assert!(
        found_loops.iter().all(|l| l.scores.len() == step_count),
        "all loops must have the same number of timesteps"
    );

    // Discovered `FoundLoop`s are always scalar (`loop_info.dimensions` is
    // `vec![]`), so `partition_for_loop` returns a length-1 vector; collapse it
    // to slot 0. The empty `dims` slice is fine -- it's only consulted for A2A
    // loops, which discovery never produces. A loop whose stocks resolve to no
    // parent-level partition (a pure module-internal loop, or a
    // PREVIOUS-lagged stockless loop) maps to its own `NormGroup::Solo`
    // group (GH #750): unrelated unpartitioned loops must not share a
    // denominator or count as each other's "competition" -- pre-#750 they
    // pooled into one default `None` bucket, so an unrelated big loop could
    // push a small module-internal loop below MIN_CONTRIBUTION and censor
    // it entirely.  This matches `ltm_post::compute_rel_loop_scores`' Solo
    // grouping, so the discovery and pinned-loop relative-score surfaces
    // agree.
    let slot0 = |fl: &FoundLoop| -> Option<usize> {
        partitions
            .partition_for_loop(&fl.loop_info, &[])
            .first()
            .copied()
            .flatten()
    };
    let loop_groups: Vec<NormGroup> = found_loops
        .iter()
        .enumerate()
        .map(|(i, fl)| NormGroup::for_loop(slot0(fl), i))
        .collect();

    // Group loops by normalization group over the FULL discovered set
    // (before retention or cap).  Drives the relative-score denominators
    // below, and -- on the fallback path -- the competing-vs-solo
    // classification too.
    let mut partition_groups: HashMap<NormGroup, Vec<usize>> = HashMap::new();
    for (i, &group) in loop_groups.iter().enumerate() {
        partition_groups.entry(group).or_default().push(i);
    }
    // "Competing" means at least two loops divide this group's denominator,
    // so that a loop's relative score is a real share rather than ±1 by
    // construction (see [`RelativeImportance`]). Three arms, one per
    // (group kind, generator) the classification can face:
    let competing: Vec<bool> = loop_groups
        .iter()
        .map(|&group| match (group, universe) {
            // Enumeration path, resolved partition: the universe's count of
            // the loops whose mass is in this partition's denominator. A
            // retention non-survivor still holds mass there, so a survivor
            // alone in the REPORTED set is genuinely sharing.
            (NormGroup::Partition(p), Some(stats)) => {
                let count = stats.loop_counts.get(&p).copied().unwrap_or(0);
                debug_assert!(
                    count >= partition_groups[&group].len(),
                    "partition {p}'s universe count ({count}) cannot be below \
                     the number of discovered loops normalizing against it \
                     ({}): every discovered loop's mass is in that total",
                    partition_groups[&group].len()
                );
                count >= 2
            }
            // Fallback path: a sample has no universe to ask about, so the
            // discovered set is the only population there is.
            (NormGroup::Partition(_), None) => partition_groups[&group].len() >= 2,
            // A Solo group holds exactly one loop by construction (it is keyed
            // by that loop's index), so it is never competing on either path.
            (NormGroup::Solo(_), _) => false,
        })
        .collect();

    // Per-group per-timestep totals: Σ|score_j[t]| over the group's loops,
    // NaN excluded (an undefined score is not signal; matches GH #542's
    // denom_summand). Inf is kept -- a real divergence at a dominance
    // inflection. Computed over ALL discovered loops, before any cap, so the
    // denominator reflects the whole partition (the truncate-before-filter
    // order of GH #310 used to compute totals over only the top-200
    // survivors). A Solo-group total is just that loop's own |score| series.
    // On the enumeration path, a Partition group's totals instead come from
    // `external_totals` -- the full enumerated universe's mass (see the fn
    // doc above).
    let mut partition_totals: HashMap<NormGroup, Vec<f64>> = HashMap::new();
    if step_count > 0 {
        for (&group, indices) in &partition_groups {
            if let (NormGroup::Partition(p), Some(stats)) = (group, universe)
                && let Some(totals) = stats.totals.get(&p)
            {
                debug_assert_eq!(totals.len(), step_count);
                partition_totals.insert(group, totals.clone());
                continue;
            }
            let mut totals = vec![0.0; step_count];
            for &idx in indices {
                for (i, &(_, score)) in found_loops[idx].scores.iter().enumerate() {
                    if !score.is_nan() {
                        // Saturating, as retention's own bank is: a finite
                        // sum overflowing to Inf would zero every finite share.
                        let mass = score.abs();
                        let sum = totals[i] + mass;
                        totals[i] =
                            if sum.is_infinite() && mass.is_finite() && totals[i].is_finite() {
                                f64::MAX
                            } else {
                                sum
                            };
                    }
                }
            }
            partition_totals.insert(group, totals);
        }
    }

    // Partition-aware MIN_CONTRIBUTION retention filter (peak semantics,
    // unchanged): keep a loop if at ANY single timestep its |score| is
    // >= MIN_CONTRIBUTION of its partition's total at that step. Runs BEFORE
    // the cap (GH #310).
    let retained_loops;
    if step_count > 0 {
        let mut keep = vec![false; found_loops.len()];
        for (idx, fl) in found_loops.iter().enumerate() {
            let totals = &partition_totals[&loop_groups[idx]];
            keep[idx] = fl.scores.iter().enumerate().any(|(i, &(_, score))| {
                !score.is_nan() && totals[i] > 0.0 && score.abs() / totals[i] >= MIN_CONTRIBUTION
            });
        }
        // Groups and competing flags of the surviving loops, in the same
        // order `retain` will leave them, so the relative-importance pass
        // below indexes the right group for each loop. The retained `Solo`
        // keys carry their pre-retention indices -- they are only ever used
        // as opaque `partition_totals` keys, never re-derived.
        let retained_groups: Vec<NormGroup> = loop_groups
            .iter()
            .zip(&keep)
            .filter_map(|(&p, &k)| k.then_some(p))
            .collect();
        let retained_competing: Vec<bool> = competing
            .iter()
            .zip(&keep)
            .filter_map(|(&c, &k)| k.then_some(c))
            .collect();

        // retain() visits in index order; drive it off the precomputed mask.
        let mut keep_iter = keep.iter();
        found_loops.retain(|_| *keep_iter.next().unwrap());
        debug_assert_eq!(retained_groups.len(), found_loops.len());
        // Read BEFORE the cap: the whole point of reporting this count is to
        // say how much the cap dropped.
        retained_loops = found_loops.len();

        rank_truncate_and_id(
            found_loops,
            &retained_groups,
            &retained_competing,
            &partition_totals,
        );
    } else {
        // No score data: nothing to rank relative to; just assign IDs over the
        // (cap-respecting) set. `partition_for_loop` still resolves, but with no
        // timesteps the relative key is undefined, so fall back to the content
        // key alone for a stable order. With no scores there is no retention
        // filter either, so every discovered loop trivially survives it.
        retained_loops = found_loops.len();
        found_loops.truncate(max_loops());
        assign_loop_ids(found_loops);
        found_loops.sort_by_cached_key(|fl| loop_sort_key(&fl.loop_info));
    }

    // Attach result-scoped partition metadata over the FINAL loop list (both
    // paths): partition indices are dense, in first-appearance order, so a
    // caller's `loops[0].partition` is always `Some(0)` or `None` and the
    // partition list is exactly the partitions the returned loops live in.
    RankOutcome {
        partitions: attach_partition_metadata(found_loops, partitions),
        retained_loops,
    }
}

/// What [`rank_and_filter`] reports about the list it just ranked, beyond the
/// mutated loop list itself.
struct RankOutcome {
    /// The result-scoped partitions the surviving loops live in
    /// (`DiscoveryResult::partitions`).
    partitions: Vec<DiscoveredPartition>,
    /// How many loops passed the retention filter, before the `MAX_LOOPS` cap
    /// truncated the list (`DiscoveryResult::retained_loops`).
    retained_loops: usize,
}

/// Resolve each surviving loop's cycle partition, remap the engine-internal
/// partition indices to dense result-scoped ones (first-appearance order over
/// the final ranked list), set `FoundLoop::partition`, and build the
/// [`DiscoveredPartition`] list.
///
/// Runs over the final (filtered, capped, ranked) loops, so
/// `DiscoveredPartition::loop_count` counts exactly the loops a caller
/// receives.  Loops with no parent-level partition (pure module-internal
/// loops) keep `partition == None` and contribute no partition entry.
fn attach_partition_metadata(
    found_loops: &mut [FoundLoop],
    partitions: &CyclePartitions,
) -> Vec<DiscoveredPartition> {
    let mut dense_for_internal: HashMap<usize, usize> = HashMap::new();
    let mut meta: Vec<DiscoveredPartition> = Vec::new();
    for fl in found_loops.iter_mut() {
        // Discovered loops are always scalar (see `rank_and_filter`'s slot0
        // note), so the length-1 collapse is exact here too.
        let internal = partitions
            .partition_for_loop(&fl.loop_info, &[])
            .first()
            .copied()
            .flatten();
        fl.partition = internal.map(|internal_idx| {
            let dense = *dense_for_internal.entry(internal_idx).or_insert_with(|| {
                meta.push(DiscoveredPartition {
                    stocks: partitions.partitions[internal_idx]
                        .iter()
                        .map(|s| s.as_str().to_string())
                        .collect(),
                    loop_count: 0,
                });
                meta.len() - 1
            });
            meta[dense].loop_count += 1;
            dense
        });
    }
    meta
}

/// The SIGNED per-timestep partition-relative loop score series for one loop.
///
/// `rel[t] = score[t] / totals[t]`, with `totals[t]` the loop's cycle-partition
/// denominator (`Σ_{j in partition} |score_j[t]|`, NaN summands already
/// excluded by `rank_and_filter`).  SAFEDIV-0 (`totals[t] == 0` -> `0.0`) and a
/// `NaN` numerator propagating to `NaN` both match
/// `ltm_post::compute_rel_loop_scores` exactly, so the discovery and pinned-loop
/// relative-score surfaces agree.  Sign is preserved (a balancing loop reads
/// negative), giving a value in `[-1, 1]` for a finite score.
fn signed_relative_scores(fl: &FoundLoop, totals: &[f64]) -> Vec<f64> {
    fl.scores
        .iter()
        .enumerate()
        .map(|(t, &(_, score))| {
            let total = totals.get(t).copied().unwrap_or(0.0);
            if total == 0.0 { 0.0 } else { score / total }
        })
        .collect()
}

/// The deepest per-step rank a loop can hold and still be anchored (AC5.1).
///
/// `k = 1` is the guarantee: every step's dominant loop in a competing group
/// is reported, so a dominance-over-time reading never names the wrong loop
/// for a step. Raising `k` covers the runners-up -- what a reader needs to see
/// a handover coming -- and is taken only while the whole anchor set still fits
/// within [`ANCHOR_SHARE_OF_CAP`] of the cap, so escalation never crowds out
/// the ordinary mean-relative ranking. The bound exists because the value of
/// the k-th place falls off quickly while its cost in slots does not: a
/// competing group can anchor up to `k` loops at EVERY step, and past the top
/// few the "runner-up" is just another loop.
const MAX_ANCHOR_K: usize = 3;

/// The largest share of the `MAX_LOOPS` cap [`select_reported`] lets the
/// anchor set claim before it stops escalating `k` (AC5.1).
///
/// The `k = 1` guarantee is unconditional and exempt from this bound -- it is
/// what AC5.1 promises, and a model whose k=1 anchors alone exceed the cap is
/// the separate pathological arm below. This bound decides only whether `k`
/// may RISE past 1, and it exists because escalation without a limit degrades
/// into the cap's failure mode from the other direction: on World3, before
/// this bound, `k` escalated to `MAX_ANCHOR_K` and 140 of the 200 reported
/// slots ended up anchors, crowding the mean-relative ranking down to a
/// remainder few readers would call representative. Capping the anchor
/// share at one HALF guarantees the ranking still fills at least half of
/// every capped report, whatever the model.
const ANCHOR_SHARE_OF_CAP: f64 = 0.5;

/// One ranked loop, as the coverage-aware cap sees it.
///
/// `rel` is the loop's signed per-step relative score series
/// ([`signed_relative_scores`]); the selection reads magnitudes and treats a
/// `NaN` as absent (a `NaN` score is an undefined contribution, exactly as in
/// [`mean_relative_contribution`]).
struct SelectionRow<'a> {
    group: NormGroup,
    competing: bool,
    rel: &'a [f64],
}

impl SelectionRow<'_> {
    /// The loop's contribution magnitude at step `t`: `0.0` where it is
    /// undefined, absent, or genuinely zero -- all three mean "this loop is
    /// not what happened here".
    fn weight(&self, t: usize) -> f64 {
        match self.rel.get(t) {
            Some(v) if !v.is_nan() => v.abs(),
            _ => 0.0,
        }
    }
}

/// For each ranked loop, the smallest `k` at which it is one of some step's
/// top-`k` loops within its competing group, or `0` for a loop that never is.
///
/// Only competing groups anchor: a solo loop's relative score is `±1` at every
/// active step by construction, so it is trivially its group's per-step
/// maximum and anchoring it would guarantee a slot to the one class of loop
/// that carries no information.
///
/// A step where no member of the group carries any weight is skipped rather
/// than anchoring an arbitrary zero: the partition's mass at that step, if
/// any, belongs to loops outside the retained set, and no retained loop
/// dominates it. Ties break toward the loop earlier in the ranking, which the
/// ascending member order makes automatic -- an equal value never displaces
/// the row already holding the place.
fn anchor_ranks(rows: &[SelectionRow<'_>]) -> Vec<usize> {
    let mut anchor_k = vec![0usize; rows.len()];
    let mut groups: HashMap<NormGroup, Vec<usize>> = HashMap::new();
    for (r, row) in rows.iter().enumerate() {
        if row.competing {
            groups.entry(row.group).or_default().push(r);
        }
    }
    // Group iteration order is unobservable: every group writes only to its
    // own members' slots, and the groups partition the rows.
    let mut top: Vec<(f64, usize)> = Vec::with_capacity(MAX_ANCHOR_K + 1);
    for members in groups.values() {
        let steps = members
            .iter()
            .map(|&r| rows[r].rel.len())
            .max()
            .unwrap_or(0);
        for t in 0..steps {
            top.clear();
            for &r in members {
                let w = rows[r].weight(t);
                if w <= 0.0 {
                    continue;
                }
                let place = top.iter().position(|&(held, _)| w > held);
                let place = place.unwrap_or(top.len());
                if place < MAX_ANCHOR_K {
                    top.insert(place, (w, r));
                    top.truncate(MAX_ANCHOR_K);
                }
            }
            for (place, &(_, r)) in top.iter().enumerate() {
                let k = place + 1;
                if anchor_k[r] == 0 || anchor_k[r] > k {
                    anchor_k[r] = k;
                }
            }
        }
    }
    anchor_k
}

/// Choose which of the ranked loops are reported, as positions into `rows`
/// (ascending, so the caller's presentation order is unchanged).
///
/// `rows` is the retained set in the final ranking order (position 0 is the
/// highest-ranked loop). Membership, and only membership, is what this
/// decides: the reported list is still ordered competitive-first by mean
/// relative score, and ids are still assigned over whatever it returns.
///
/// Selection (AC5.1):
///
/// 1. No cap pressure -- everything is reported.
/// 2. Otherwise every step's dominant loop within a competing group is an
///    ANCHOR and keeps its slot ([`anchor_ranks`], `k = 1`) -- unconditionally,
///    even if that alone claims more than [`ANCHOR_SHARE_OF_CAP`] of the cap.
///    `k` then rises, bounded by [`MAX_ANCHOR_K`], but only while the ENLARGED
///    anchor set stays at or under [`ANCHOR_SHARE_OF_CAP`] of the cap -- so
///    escalation can grow the guarantee's coverage but can never crowd the
///    ordinary ranking down to a sliver of the report.
/// 3. Remaining slots are filled in the existing ranking order.
/// 4. If even the `k = 1` anchors outnumber the cap -- a report that cannot
///    cover every step whatever it names -- the cap applies to the anchors
///    alone, in ranking order. The coverage claim wins over the ranking claim
///    there, because a loop that dominates no step is a worse answer to "what
///    drove this step" than a loop that dominates a different one.
fn select_reported(rows: &[SelectionRow<'_>], cap: usize) -> Vec<usize> {
    if rows.len() <= cap {
        return (0..rows.len()).collect();
    }
    let anchor_k = anchor_ranks(rows);
    let count_at = |k: usize| -> usize {
        anchor_k
            .iter()
            .filter(|&&depth| depth != 0 && depth <= k)
            .count()
    };
    if count_at(1) > cap {
        // Pathological: anchors alone overflow. `anchor_k` is indexed by
        // ranking position, so filtering it in order already ranks them.
        return anchor_k
            .iter()
            .enumerate()
            .filter(|&(_, &depth)| depth == 1)
            .map(|(r, _)| r)
            .take(cap)
            .collect();
    }
    // `k = 1` is the unconditional guarantee, exempt from the share bound
    // (only escalation PAST it is bounded). The anchor set grows monotonically
    // with k, so the first k whose anchor set would exceed the share is where
    // escalation stops; a boundary count exactly AT the share ("at or under")
    // is still taken.
    let anchor_cap = cap as f64 * ANCHOR_SHARE_OF_CAP;
    let mut depth = 1;
    for k in 2..=MAX_ANCHOR_K {
        if count_at(k) as f64 > anchor_cap {
            break;
        }
        depth = k;
    }
    let mut selected = vec![false; rows.len()];
    let mut chosen = 0usize;
    for (r, &at) in anchor_k.iter().enumerate() {
        if at != 0 && at <= depth {
            selected[r] = true;
            chosen += 1;
        }
    }
    for slot in selected.iter_mut() {
        if chosen >= cap {
            break;
        }
        if !*slot {
            *slot = true;
            chosen += 1;
        }
    }
    selected
        .iter()
        .enumerate()
        .filter(|&(_, &keep)| keep)
        .map(|(r, _)| r)
        .collect()
}

/// Rank the retained loops competitive-first by partition-relative importance
/// (see [`cmp_relative_importance`]), apply the coverage-aware cap
/// ([`select_reported`]), assign IDs, and leave the loops in the ranking order
/// callers consume.
///
/// `loop_groups[i]` is the normalization group of `found_loops[i]` (its
/// cycle partition, or its own Solo group when unresolved -- GH #750),
/// `competing[i]` whether at least two loops divide that group's denominator,
/// and `partition_totals` the per-group per-timestep denominator -- all as
/// built by `rank_and_filter` over the full discovered set.
fn rank_truncate_and_id(
    found_loops: &mut Vec<FoundLoop>,
    loop_groups: &[NormGroup],
    competing: &[bool],
    partition_totals: &HashMap<NormGroup, Vec<f64>>,
) {
    // Pair each loop with its partition-relative importance statistic, then sort
    // and truncate the pair vector so the (non-Copy) FoundLoop move is a single
    // permutation and the key survives ID assignment (no recomputation).
    //
    // While the per-partition denominators are in hand, also attach each loop's
    // SIGNED per-timestep relative score series (`rel_scores`) -- the same
    // `score[t] / partition_total[t]` normalization, SAFEDIV-0, that
    // `ltm_post::compute_rel_loop_scores` applies on the pinned-loop path.  This
    // is the [-1, 1] importance series `analysis::to_loop_summary` /
    // `to_feedback_loop` surface, so dominance/ranking is partition-relative
    // (comparable across partitions) rather than raw-magnitude-biased.
    let mut keyed: Vec<(RelativeImportance, FoundLoop)> = std::mem::take(found_loops)
        .into_iter()
        .enumerate()
        .map(|(idx, mut fl)| {
            let totals = &partition_totals[&loop_groups[idx]];
            let mean_rel = mean_relative_contribution(&fl, totals);
            let key = loop_sort_key(&fl.loop_info);
            fl.rel_scores = signed_relative_scores(&fl, totals);
            (
                RelativeImportance {
                    mean_rel,
                    competing: competing[idx],
                    group: loop_groups[idx],
                    avg_abs: fl.avg_abs_score,
                    key,
                },
                fl,
            )
        })
        .collect();

    keyed.sort_by(|a, b| cmp_relative_importance(&a.0, &b.0));

    // Membership under the cap is coverage-aware (AC5.1): each step's dominant
    // loop within a competing group keeps its slot even when the mean-relative
    // ranking would drop it. Order is untouched -- `select_reported` answers in
    // ascending ranking position and `retain` visits in that order.
    let selected = {
        let rows: Vec<SelectionRow<'_>> = keyed
            .iter()
            .map(|(imp, fl)| SelectionRow {
                group: imp.group,
                competing: imp.competing,
                rel: &fl.rel_scores,
            })
            .collect();
        select_reported(&rows, max_loops())
    };
    if selected.len() < keyed.len() {
        let mut keep = vec![false; keyed.len()];
        for &r in &selected {
            keep[r] = true;
        }
        let mut keep_iter = keep.into_iter();
        keyed.retain(|_| keep_iter.next().unwrap_or(false));
    }

    // Assign deterministic, content-derived IDs WITHOUT disturbing the
    // relative-importance ordering callers consume. `assign_loop_ids` reorders
    // its slice by a content key (order-independent, commit 1539329d) and then
    // walks it assigning r#/b#/u# counters; to get the identical id-to-loop
    // mapping while leaving `keyed` in relative order, we replicate that: visit
    // the loops in content-key order and assign each its counter id. Each
    // RelativeImportance carries the same `loop_sort_key` the assigner sorts on.
    let mut by_content: Vec<usize> = (0..keyed.len()).collect();
    by_content.sort_by(|&i, &j| keyed[i].0.key.cmp(&keyed[j].0.key));
    let mut counters = LoopIdCounters::new();
    for &i in &by_content {
        keyed[i].1.loop_info.id = counters.next_id(&keyed[i].1.loop_info.polarity);
    }

    *found_loops = keyed.into_iter().map(|(_, fl)| fl).collect();
}

/// Sequential `r#`/`b#`/`u#` loop-id counters, advanced one loop at a time in
/// a deterministic (content-key) visitation order.
///
/// The prefix follows the dominant polarity so MostlyReinforcing /
/// MostlyBalancing share counters with their pure counterparts; this mirrors
/// `crate::ltm::assign_loop_ids` for the structural side.
struct LoopIdCounters {
    r: u32,
    b: u32,
    u: u32,
}

impl LoopIdCounters {
    fn new() -> Self {
        LoopIdCounters { r: 1, b: 1, u: 1 }
    }

    fn next_id(&mut self, polarity: &LoopPolarity) -> String {
        match polarity {
            LoopPolarity::Reinforcing | LoopPolarity::MostlyReinforcing => {
                let id = format!("r{}", self.r);
                self.r += 1;
                id
            }
            LoopPolarity::Balancing | LoopPolarity::MostlyBalancing => {
                let id = format!("b{}", self.b);
                self.b += 1;
                id
            }
            LoopPolarity::Undetermined => {
                let id = format!("u{}", self.u);
                self.u += 1;
                id
            }
        }
    }
}

/// Assign deterministic IDs to discovered loops based on polarity and content.
///
/// Reorders `loops` by the content-derived `loop_sort_key` and walks the sorted
/// slice assigning sequential ids, so the id-to-loop mapping is independent of
/// the input order (commit 1539329d). Callers that need a different final
/// ordering re-sort after this returns.
fn assign_loop_ids(loops: &mut [FoundLoop]) {
    // `sort_by_cached_key` computes each loop's (allocating) sort key once
    // rather than per comparison, matching the `crate::ltm::graph`
    // `assign_loop_ids` twin.
    loops.sort_by_cached_key(|fl| loop_sort_key(&fl.loop_info));

    let mut counters = LoopIdCounters::new();
    for found in loops.iter_mut() {
        found.loop_info.id = counters.next_id(&found.loop_info.polarity);
    }
}

/// Content-derived sort key that fully orders discovered loops, including
/// sibling cycles over the same node set (GH #497). Mirrors
/// `crate::ltm::graph::loop_id_sort_key`: the primary component is the deduped
/// sorted variable set (the historical key -- single-direction loops keep
/// their existing numbering), and the secondary component is the canonical
/// cyclic rotation of the directed edge sequence, which differs between two
/// sibling cycles so the stable-sort fallback cannot leak the generator's
/// emission order into the assigned ids.
fn loop_sort_key(loop_info: &Loop) -> (String, Vec<String>) {
    let mut vars: Vec<String> = loop_info
        .links
        .iter()
        .flat_map(|link| vec![link.from.as_str().to_string(), link.to.as_str().to_string()])
        .collect();
    vars.sort();
    vars.dedup();
    let primary = vars.join("_");

    let edge_seq: Vec<String> = loop_info
        .links
        .iter()
        .map(|link| link.from.as_str().to_string())
        .collect();
    let secondary = crate::ltm::canonical_rotation(&edge_seq);

    (primary, secondary)
}

#[cfg(test)]
#[path = "ltm_finding_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ltm_finding_enum_tests.rs"]
mod enum_tests;
