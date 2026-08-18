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
use enum_gen::{
    ActivityGraph, accumulate_series_into_totals, enumerate_active_circuits, retain_circuits,
};

// Shortest-path fallback: the candidate generator that runs when the
// enumeration cannot complete within its budgets or the caller's deadline
// (docs/design-plans/2026-08-17-ltm-discovery-exact.md). A sibling file
// mounted here purely for the per-file line cap.
#[path = "ltm_finding_fallback.rs"]
mod fallback;
pub use fallback::{FallbackClosures, FallbackConfig, FallbackSeeds, FallbackWeight};

// --- Types ---

/// A parsed link score offset: ((from_variable, to_variable), offset_in_results).
type LinkOffset = ((Ident<Canonical>, Ident<Canonical>), usize);

/// HashMap for O(1) link offset lookup by (from, to) key.
type LinkOffsetMap = HashMap<(Ident<Canonical>, Ident<Canonical>), usize>;

/// Memoized per-exit-port module-input recompute, within one discovery call.
///
/// The key is everything `recompute_module_input_edge_series` reads that
/// varies by loop edge -- the module-input source, the module instance, and
/// the reader that identifies the exit port -- each stripped of any element
/// subscript, exactly as the recompute strips them before its own lookups. An
/// arrayed loop through a module therefore hits one entry per element rather
/// than re-enumerating the sub-model's pathway map each time.
type ModuleSeriesCache =
    HashMap<(Ident<Canonical>, Ident<Canonical>, Ident<Canonical>), Option<Vec<f64>>>;

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
    /// from ALL scorable loops -- discovery is exact, not heuristic.
    ///
    /// When `false`, the shortest-path fallback generated the candidates: an
    /// explicit SAMPLE of the loop universe, holding the minimum-weight cycle
    /// through each (stock, saved step) plus whatever its closing in-edges
    /// recover. This is the only completeness signal a caller gets, and the
    /// two `false` causes are deliberately not distinguished -- a budget trip
    /// and a deadline expiry leave the report equally a sample.
    pub enumeration_complete: bool,
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
struct IndexedEdge {
    to: u32,
    offset: usize,
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
                let value = results.data[base + edge.offset];
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
fn recompute_module_input_edge_series(
    causal_graph: &CausalGraph,
    results: &Results,
    links: &[Link],
    edge_idx: usize,
    step_count: usize,
    sub_model_output_ports: &SubModelOutputPorts,
) -> Option<Vec<f64>> {
    use crate::ltm::{normalize_module_ref, strip_subscript};
    use crate::variable::Variable;

    let n = links.len();
    let link = &links[edge_idx];

    // Discovery runs on the ELEMENT-LEVEL graph, so an arrayed loop's
    // non-module nodes carry element subscripts (`s[nyc] -> m -> growth[nyc]`).
    // Every name-sensitive lookup below compares against bare names
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

    // `m` must be a module instance with a recursively-built internal graph
    // (a DynamicModule / passthrough exposing pathways). Pathless modules and
    // non-modules keep the base link score.
    let module_graph = causal_graph.module_graph(&module_name)?;

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
    let module_var = causal_graph.variables().get(&module_name)?;
    let Variable::Module { inputs, .. } = module_var else {
        return None;
    };
    let mut matching = inputs
        .iter()
        .filter(|inp| normalize_module_ref(&inp.src) == from_base);
    let entry_port = matching.next()?.dst.clone();
    if matching.next().is_some() {
        // A second input port is also fed by `x`: ambiguous entry, fall back.
        return None;
    }

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
    let y_var = causal_graph.variables().get(&y)?;
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
                if normalize_module_ref(&inp.src) != module_name {
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
        _ => discovery_module_exit_port(&module_name, y_var),
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
    // the sub-model composite's selection, but restricted to the exit port).
    let mut series: Option<Vec<f64>> = None;
    for off in matching_offsets {
        let candidate: Vec<f64> = (0..step_count)
            .map(|step| results.data[step * results.step_size + off])
            .collect();
        series = max_abs_score_series(series, Some(candidate));
    }
    series
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
fn stitch_cross_agg_node_paths(
    search: &IndexedSearch,
    candidate_paths: &[Vec<u32>],
) -> (Vec<Vec<u32>>, bool) {
    let petals_by_agg = crate::db::collect_agg_petals(candidate_paths, |id: &u32| {
        search.idents[*id as usize].as_str()
    });
    let mut sorted: Vec<(&str, Vec<crate::db::StitchPetal<u32>>)> =
        petals_by_agg.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let (stitched, truncated_aggs) =
        crate::db::stitch_cross_agg_petals(sorted, crate::db::cross_agg_loop_budget());
    (stitched, !truncated_aggs.is_empty())
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
        return Ok(DiscoveryResult {
            loops: Vec::new(),
            partitions: Vec::new(),
            truncated: false,
            agg_recovery_truncated: false,
            enumeration_complete: enumeration_ran,
        });
    }

    // Build HashMap for O(1) link offset lookups during score computation
    let link_offset_map: LinkOffsetMap = link_offsets
        .iter()
        .map(|((from, to), offset)| ((from.clone(), to.clone()), *offset))
        .collect();

    if stocks.is_empty() {
        return Ok(DiscoveryResult {
            loops: Vec::new(),
            partitions: Vec::new(),
            truncated: false,
            agg_recovery_truncated: false,
            enumeration_complete: enumeration_ran,
        });
    }

    let step_count = results.step_count;

    // Hoist the integer-indexed topology build out of the candidate search:
    // the graph's edges and result slots are step-invariant. Both candidate
    // generators run over this one structure.
    let search = IndexedSearch::build(&link_offsets, stocks);

    // Cycle partitions serve both the enumeration retention pass
    // (full-universe denominators) and the final ranking; compute once.
    let partitions = causal_graph.compute_cycle_partitions();

    // Candidate cycles as `IndexedSearch` node-id paths, whichever generator
    // produced them; materialized into `Ident` paths once, below.
    let mut node_paths: Vec<Vec<u32>> = Vec::new();
    let mut truncated = false;
    let mut enumeration_complete = false;
    let mut agg_recovery_truncated = false;
    // Full-universe per-partition denominators from the enumeration path
    // (`None` on the fallback path, where `rank_and_filter` computes totals
    // over the discovered set instead -- a sample has no universe to offer).
    let mut external_totals: Option<HashMap<usize, Vec<f64>>> = None;

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
    // relative scores against the same totals via `external_totals`. Those
    // totals are the FULL enumerated universe's raw mass (retention
    // non-survivors included, GH #310) -- NOT the mass reported loops carry --
    // corrected below so that each distinct reported cycle contributes its
    // mass exactly once and by the series it reports: a module-traversing
    // loop's raw product is replaced by its per-exit-port override series
    // (they can differ by any factor), and a duplicate representative the
    // reported-cycle dedup discards has its raw mass subtracted back out.
    // Every other circuit -- retention non-survivors included -- keeps its
    // raw enumerated product in the totals unmodified.
    if enumeration_ran
        && let Some(activity) = ActivityGraph::build(&search, results, deadlines.enumeration, clock)
    {
        let candidates = enumerate_active_circuits(&activity, deadlines.enumeration, clock);
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
            if let Some(retention) = retain_circuits(
                &candidates,
                &activity,
                &stock_partition_of_node,
                &is_module_node,
                deadlines.enumeration,
                clock,
            ) {
                // The universe's denominators and the universe's circuit
                // counts are two views of one enumerated set, built in one
                // pass: a partition carrying mass with no circuit counted
                // against it would mean the two had drifted apart, and every
                // relative score normalized against that partition would be
                // measured against a population nothing described.
                debug_assert!(
                    retention.partition_totals.keys().all(|part| retention
                        .partition_circuit_counts
                        .get(part)
                        .is_some_and(|&n| n > 0)),
                    "each partition carrying enumerated mass must carry the \
                     count of circuits that produced it"
                );

                // Cross-agg stitching over the FULL enumerated set (GH #696).
                // Stitching must see pre-retention circuits: a petal can fail
                // retention while a stitched combination passes.
                //
                // Only a circuit visiting EXACTLY ONE synthetic agg node can be
                // a petal, and `collect_agg_petals` needs node paths rather than
                // the edge rows the enumerator emits, so the node paths are
                // materialized for those circuits alone -- on a model carrying
                // no agg node at all (every scalar model) that is none of them.
                // Pre-filtering changes nothing `collect_agg_petals` would keep:
                // it drops the same circuits itself, in the same order. The agg
                // count itself is taken off the edge rows via `edge_source`
                // (O(1), no allocation) so `circuit_nodes` -- which allocates a
                // `Vec` -- is called only for the circuits that pass the count
                // test, not for every enumerated circuit (World3's universe is
                // ~150k circuits, almost all of which visit zero agg nodes).
                let is_agg_node: Vec<bool> = search
                    .idents
                    .iter()
                    .map(|ident| crate::ltm_agg::is_synthetic_agg_name(ident.as_str()))
                    .collect();
                let petal_circuits: Vec<Vec<u32>> = if is_agg_node.contains(&true) {
                    (0..candidates.len())
                        .filter(|&ci| {
                            candidates
                                .circuit(ci)
                                .iter()
                                .filter(|&&row| is_agg_node[activity.edge_source(row) as usize])
                                .count()
                                == 1
                        })
                        .map(|ci| activity.circuit_nodes(candidates.circuit(ci)))
                        .collect()
                } else {
                    Vec::new()
                };
                let (stitched, stitch_truncated) =
                    stitch_cross_agg_node_paths(&search, &petal_circuits);
                agg_recovery_truncated = stitch_truncated;

                let survivors: Vec<Vec<u32>> = retention
                    .survivors
                    .iter()
                    .map(|&ci| activity.circuit_nodes(candidates.circuit(ci)))
                    .collect();
                let stitched = new_paths_by_rotation(&survivors, stitched);

                // Stitched loops join the candidate set like any other loop:
                // their mass joins the denominators and they are materialized
                // below.
                let mut totals = retention.partition_totals;
                for seq in &stitched {
                    if seq.iter().any(|&n| is_module_node[n as usize]) {
                        // A module-traversing loop reports the per-exit-port
                        // override series rather than the raw composite
                        // product, so its mass joins the denominators after
                        // materialization -- the same rule retention applies
                        // to the enumerated circuits.
                        //
                        // NOT independently covered by an end-to-end fixture
                        // (unlike the retention-side arm, pinned by
                        // `ltm_finding_tests.rs`'s
                        // `a_dropped_module_duplicate_leaves_the_denominator_untouched`
                        // and its siblings): a module can only be built this
                        // way today by reading a NAMED aux, and any such aux
                        // sitting strictly between a synthetic agg and its
                        // per-element consumers is, by construction, shared
                        // across every element's petal -- which makes those
                        // petals' `internal` node sets overlap
                        // (`db::stitch_cross_agg_petals`'s disjointness test),
                        // so `stitch_cross_agg_petals` never combines them and
                        // no stitched sequence can ever reach a module this
                        // way. A module that is only in ONE element's petal
                        // (not shared) does not create the double-counting
                        // risk this arm guards, since a lone petal never
                        // stitches with itself. If a future module-instancing
                        // shape reopens this path, add a fixture here rather
                        // than trusting this argument to still hold.
                        continue;
                    }
                    accumulate_series_into_totals(
                        seq,
                        &activity,
                        &stock_partition_of_node,
                        &mut totals,
                    );
                }

                node_paths = survivors;
                node_paths.extend(stitched);
                external_totals = Some(totals);
                enumeration_complete = true;
            }
        }
        // An incomplete enumeration (circuit budget, visit budget, edge-row
        // budget, or deadline) falls through to the fallback with whatever
        // wall-clock remains; the partial circuit list is discarded rather
        // than merged, because it is biased by node-id root order and its
        // per-partition totals are not the universe's.
    }

    if !enumeration_complete {
        // --- Fallback candidate generation: per (seed, step) shortest cycles
        // (`ltm_finding_fallback.rs`). `CandidateGen::Auto` uses the default
        // configuration; `FallbackOnly` names its own.
        let config = match candidate_gen {
            CandidateGen::Auto => FallbackConfig::DEFAULT,
            CandidateGen::FallbackOnly(config) => config,
        };
        let outcome = fallback::sweep(&search, results, config, deadlines.fallback, clock);
        truncated = outcome.truncated;
        debug_assert!(
            outcome.truncated || outcome.steps_processed == step_count.saturating_sub(1),
            "an untruncated sweep covers every saved step after step 0"
        );

        // Stitch cross-element-through-aggregate loops (GH #696) through the
        // SAME helper the enumeration path uses. Both generators emit only
        // ELEMENTARY cycles, so a feedback loop that traverses a hoisted
        // reducer's synthetic agg node more than once
        // (`pop[a] -> agg -> pop[b] -> agg -> pop[a]`) is structurally
        // unreachable to either, and exhaustive mode's petal stitching is what
        // recovers it in both.
        let (stitched, stitch_truncated) = stitch_cross_agg_node_paths(&search, &outcome.paths);
        agg_recovery_truncated = stitch_truncated;
        let stitched = new_paths_by_rotation(&outcome.paths, stitched);
        node_paths = outcome.paths;
        node_paths.extend(stitched);
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
        });
    }

    // Convert paths to FoundLoop objects with scores. Each is paired with
    // whether its path traverses a module instance -- the same predicate the
    // enumeration retention pass uses, so the mass a module loop is denied
    // there and the mass it is granted below are exactly complementary.
    let mut found_loops: Vec<(FoundLoop, bool)> = Vec::new();

    // The per-exit-port recompute below is keyed by
    // (module-input source, module instance, exit-port reader) and nothing
    // else, so within one discovery call the answer is memoizable. Without it
    // every loop through a module re-enumerates the sub-model's pathway map
    // once per link -- the same map, over the same sorted port set, every
    // time.
    let mut module_series_cache: ModuleSeriesCache = HashMap::new();

    for path in &all_paths {
        let traverses_module = path.iter().any(|n| causal_graph.module_graph(n).is_some());
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
        // verbatim everywhere this returns `None`.
        let link_override_series: Vec<Option<Vec<f64>>> = (0..links.len())
            .map(|i| {
                let n = links.len();
                let next = &links[(i + 1) % n];
                // Gate on the module instance before spelling a cache key: on
                // a model with no modules -- most of them -- this is the only
                // work the recompute costs per link.
                let module_name =
                    Ident::<Canonical>::new(crate::ltm::strip_subscript(links[i].to.as_str()));
                // No module instance at `links[i].to` at all: the recompute's
                // own gate (`module_graph(&module_name)?`) would decline for
                // the identical reason, so answer `None` directly instead of
                // re-stripping/re-interning the same names only to reach the
                // same `?` immediately.
                causal_graph.module_graph(&module_name)?;
                if next.from != links[i].to {
                    // A non-sequential link list has no exit port to read, and
                    // the recompute declines it; that case is outside what the
                    // cache key spells, so it is answered uncached. (The
                    // recompute's own guard for this is weaker -- it compares
                    // stripped names further along -- so delegating here still
                    // does real work, unlike the module-graph arm above.)
                    return recompute_module_input_edge_series(
                        causal_graph,
                        results,
                        &links,
                        i,
                        step_count,
                        sub_model_output_ports,
                    );
                }
                let key = (
                    Ident::<Canonical>::new(crate::ltm::strip_subscript(links[i].from.as_str())),
                    module_name,
                    Ident::<Canonical>::new(crate::ltm::strip_subscript(next.to.as_str())),
                );
                if let Some(cached) = module_series_cache.get(&key) {
                    return cached.clone();
                }
                let series = recompute_module_input_edge_series(
                    causal_graph,
                    results,
                    &links,
                    i,
                    step_count,
                    sub_model_output_ports,
                );
                module_series_cache.entry(key).or_insert(series).clone()
            })
            .collect();

        // Compute signed loop score at each timestep.
        // Time is derived from specs assuming evenly-spaced results at save_step intervals.
        let mut scores: Vec<(f64, f64)> = Vec::new();
        let mut abs_score_sum = 0.0;
        let mut valid_count = 0usize;

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

            if has_nan {
                scores.push((time, f64::NAN));
            } else {
                scores.push((time, loop_score));
                abs_score_sum += loop_score.abs();
                valid_count += 1;
            }
        }

        let avg_abs_score = if valid_count > 0 {
            abs_score_sum / valid_count as f64
        } else {
            0.0
        };

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

        found_loops.push((
            FoundLoop {
                loop_info,
                scores,
                avg_abs_score,
                // Filled in once partition denominators are known (rank_truncate_and_id).
                rel_scores: Vec::new(),
                // Filled in by attach_partition_metadata at the end of ranking.
                partition: None,
                polarity_confidence,
            },
            traverses_module,
        ));
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
    let mut deduped: Vec<(FoundLoop, bool)> = Vec::new();
    let mut dropped: Vec<(FoundLoop, bool)> = Vec::new();
    for entry in found_loops {
        let nodes: Vec<String> = entry
            .0
            .loop_info
            .links
            .iter()
            .map(|l| l.from.as_str().to_string())
            .collect();
        let key = crate::ltm::canonical_rotation(&nodes);
        match by_reported_cycle.get(&key) {
            Some(&idx) => {
                if entry.0.avg_abs_score > deduped[idx].0.avg_abs_score {
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

    // Correct the two classes of enumerated mass the totals bank under a
    // series different from the one actually reported. Every other circuit's
    // raw product stays in the totals untouched -- the totals remain the
    // enumerated universe's raw mass (retention non-survivors included), just
    // no longer carrying these two discrepancies.
    //
    // Enumeration banks each circuit's RAW product as its partition's mass,
    // which two classes of loop do not report. A module-traversing loop
    // reports the per-exit-port override series (the composite the raw product
    // uses max-abs-selects across ALL of the module's output ports, and so can
    // carry an entirely different magnitude), so it contributed nothing then
    // and contributes its materialized series now. And two circuits that trim
    // to the same reported loop each banked mass, while only the surviving
    // representative's score is reported, so the other's comes back out.
    //
    // Skipping this leaves a module-traversing loop's slice of the
    // denominator holding a composite product nothing reports, and leaves a
    // trimmed duplicate's mass in the denominator with no reported loop
    // behind it either.
    if let Some(totals) = external_totals.as_mut() {
        for (fl, traverses_module) in &deduped {
            if *traverses_module {
                add_reported_mass_to_totals(fl, &partitions, step_count, totals);
            }
        }
        for (fl, traverses_module) in &dropped {
            if !*traverses_module {
                subtract_reported_mass_from_totals(fl, &partitions, totals);
            }
        }
    }

    let mut found_loops: Vec<FoundLoop> = deduped.into_iter().map(|(fl, _)| fl).collect();

    let partition_meta = rank_and_filter(&mut found_loops, &partitions, external_totals.as_ref());

    Ok(DiscoveryResult {
        loops: found_loops,
        partitions: partition_meta,
        truncated,
        agg_recovery_truncated,
        enumeration_complete,
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

/// Add a loop's REPORTED |score| series into its partition's universe total.
///
/// Used for the loops enumeration deliberately withheld mass from: a
/// module-traversing loop's reported score comes from the per-exit-port
/// override series, which the raw product the retention pass sees cannot
/// stand in for. The partition may have no total yet -- every one of its
/// circuits may traverse a module -- so the entry is created on demand.
fn add_reported_mass_to_totals(
    fl: &FoundLoop,
    partitions: &CyclePartitions,
    step_count: usize,
    totals: &mut HashMap<usize, Vec<f64>>,
) {
    let Some(part) = loop_partition_slot(fl, partitions) else {
        return;
    };
    let series = totals.entry(part).or_insert_with(|| vec![0.0; step_count]);
    for (t, &(_, score)) in fl.scores.iter().enumerate() {
        if !score.is_nan() {
            series[t] += score.abs();
        }
    }
}

/// Remove a loop's |score| series from its partition's universe total.
///
/// Used for a duplicate representative the reported-cycle dedup discarded: its
/// mass was banked by the enumeration pass and no loop reports it. The
/// partition necessarily has a total (this loop's own mass built it), and the
/// result cannot go negative, since a float subtraction of a summand from a
/// sum of non-negative terms is bounded below by zero.
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
        if !score.is_nan() {
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
/// **External denominators (the enumeration path).** `external_totals`, when
/// provided, carries the per-engine-internal-partition per-step
/// `Σ|score_j[t]|` computed over the FULL enumerated loop universe (retention
/// non-survivors included), and it replaces the internally-computed totals
/// for every `NormGroup::Partition` group. Relative scores are then
/// normalized against the whole universe's mass -- matching exhaustive-mode
/// semantics, where the enumerated set IS the universe -- instead of against
/// only the loops in `found_loops`. Solo groups keep their own-series totals
/// either way. The competing-vs-solo classification is deliberately still
/// computed over `found_loops` (on the enumeration path: retention
/// survivors): classifying against the full universe would let never-active
/// or sub-threshold phantom co-members flip a genuinely-solo loop to
/// "competing" with mean relative score ~1.0 (its denominator gains no mass
/// from them), vaulting zero-information loops over real competing ones --
/// exactly the degeneracy the solo demotion exists to prevent.
fn rank_and_filter(
    found_loops: &mut Vec<FoundLoop>,
    partitions: &CyclePartitions,
    external_totals: Option<&HashMap<usize, Vec<f64>>>,
) -> Vec<DiscoveredPartition> {
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
    // (before retention or cap).  Drives both the relative-score
    // denominators below and the competing-vs-solo classification: a loop is
    // "competing" iff its group holds at least one other discovered loop,
    // the same population its denominator sums over -- so "solo" means
    // exactly "its relative score is ±1 by construction" (a Solo-group loop
    // is solo by definition).
    let mut partition_groups: HashMap<NormGroup, Vec<usize>> = HashMap::new();
    for (i, &group) in loop_groups.iter().enumerate() {
        partition_groups.entry(group).or_default().push(i);
    }
    let competing: Vec<bool> = loop_groups
        .iter()
        .map(|p| partition_groups[p].len() >= 2)
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
            if let (NormGroup::Partition(p), Some(external)) = (group, external_totals)
                && let Some(totals) = external.get(&p)
            {
                debug_assert_eq!(totals.len(), step_count);
                partition_totals.insert(group, totals.clone());
                continue;
            }
            let mut totals = vec![0.0; step_count];
            for &idx in indices {
                for (i, &(_, score)) in found_loops[idx].scores.iter().enumerate() {
                    if !score.is_nan() {
                        totals[i] += score.abs();
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
        // key alone for a stable order.
        found_loops.truncate(max_loops());
        assign_loop_ids(found_loops);
        found_loops.sort_by_cached_key(|fl| loop_sort_key(&fl.loop_info));
    }

    // Attach result-scoped partition metadata over the FINAL loop list (both
    // paths): partition indices are dense, in first-appearance order, so a
    // caller's `loops[0].partition` is always `Some(0)` or `None` and the
    // partition list is exactly the partitions the returned loops live in.
    attach_partition_metadata(found_loops, partitions)
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

/// Rank the retained loops competitive-first by partition-relative importance
/// (see [`cmp_relative_importance`]), truncate to the (possibly
/// test-overridden) cap, assign IDs, and leave the loops in the ranking order
/// callers consume.
///
/// `loop_groups[i]` is the normalization group of `found_loops[i]` (its
/// cycle partition, or its own Solo group when unresolved -- GH #750),
/// `competing[i]` whether that group holds at least one other discovered
/// loop, and `partition_totals` the per-group per-timestep denominator --
/// all as built by `rank_and_filter` over the full discovered set.
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
                    avg_abs: fl.avg_abs_score,
                    key,
                },
                fl,
            )
        })
        .collect();

    keyed.sort_by(|a, b| cmp_relative_importance(&a.0, &b.0));
    keyed.truncate(max_loops());

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
