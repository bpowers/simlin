// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Strongest-path loop discovery algorithm from Eberlein & Schoenberg (2020).
//!
//! This module implements the heuristic "loops that matter" discovery algorithm
//! described in Appendix I of "Finding the Loops That Matter" (2020). Instead of
//! exhaustively enumerating all feedback loops (which grows factorially with model
//! size), this algorithm uses a DFS guided by link score magnitudes to find the
//! most important loops at each simulation timestep.
//!
//! The algorithm runs as a post-processing step on simulation results that include
//! link score synthetic variables (generated with `ltm_discovery_mode` enabled).

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::common::{Canonical, Ident, Result};
use crate::datamodel;
use crate::db::LtmSyntheticVar;
use crate::ltm::{CausalGraph, CyclePartitions, Link, LinkPolarity, Loop, LoopPolarity};
use crate::project::Project;
use crate::results::Results;

// --- Types ---

/// A parsed link score offset: ((from_variable, to_variable), offset_in_results).
type LinkOffset = ((Ident<Canonical>, Ident<Canonical>), usize);

/// HashMap for O(1) link offset lookup by (from, to) key.
type LinkOffsetMap = HashMap<(Ident<Canonical>, Ident<Canonical>), usize>;

/// Per-sub-model emitted LTM output-port set, keyed by the sub-model's
/// canonical name. The discovery-mode per-exit-port recompute (GH #698) uses
/// it to enumerate pathway indices against the SAME sorted port set the
/// sub-model emitted its `$⁚ltm⁚path⁚{port}⁚{idx}` vars against -- see
/// `recompute_module_input_edge_series` and `discover_loops_with_graph`.
pub type SubModelOutputPorts = HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>;

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

/// An outbound edge in the search graph: target variable and |link_score|.
///
/// `SearchGraph` is the original `Ident`-keyed, per-timestep-rebuilt reference
/// implementation. Production discovery now runs over the integer-indexed
/// `IndexedSearch` (built once, no per-step string hashing); `SearchGraph` is
/// retained as the test-only correctness oracle that documents the reference
/// algorithm and is cross-checked against `IndexedSearch` for equivalence.
#[cfg(test)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
struct ScoredEdge {
    to: Ident<Canonical>,
    /// Absolute value of link score at this timestep
    score: f64,
}

/// The search graph for one timestep: adjacency list with edges sorted by |score| desc.
#[cfg(test)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
struct SearchGraph {
    /// variable -> outbound edges, sorted by |score| descending
    adj: HashMap<Ident<Canonical>, Vec<ScoredEdge>>,
    /// stock variables (search starts from each stock)
    stocks: Vec<Ident<Canonical>>,
}

// --- Public types ---

/// A loop found by the strongest-path algorithm, with its scores over time.
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
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct DiscoveredPartition {
    /// The partition's stock names (element-level for arrayed models, e.g.
    /// `population[nyc]`), sorted lexicographically.
    pub stocks: Vec<String>,
    /// Number of loops in the RETURNED loop list that belong to this
    /// partition (post-filter, post-cap -- the count a caller can verify
    /// against the `loops` it received, not the discovered-but-dropped total).
    pub loop_count: usize,
}

/// The outcome of a strongest-path discovery run.
///
/// `truncated` is `true` when a caller-supplied time budget elapsed before the
/// per-timestep DFS sweep finished, so `loops` reflects only the timesteps
/// processed before the budget ran out (and is therefore *possibly* partial:
/// loops only dominant in unprocessed timesteps will be absent, and the
/// per-step importance series of the loops that *were* found is complete,
/// since each loop's score is recomputed across all steps once its path is
/// known). Discovery on large models can be infeasibly slow (GH #647), so the
/// budget lets callers bound wall-clock time and report partial results rather
/// than hang.
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
}

#[cfg(test)]
impl SearchGraph {
    /// Build from a list of (from, to, abs_score) triples.
    ///
    /// Zero-score (and NaN) edges are excluded from the graph: a loop through
    /// such a link has loop score exactly 0 at this timestep, so traversing
    /// it cannot surface a loop that matters here (GH #647). A loop whose
    /// links are all simultaneously nonzero at some sampled timestep remains
    /// discoverable there. The caveat: a "baton-passing" loop whose links are
    /// only ever active at *different* sampled steps (or only between save
    /// points) is never discoverable -- see `discovery_decoupled_stocks` in
    /// tests/simulate_ltm.rs for a real example exhaustive mode catches.
    fn from_edges(
        edges: Vec<(Ident<Canonical>, Ident<Canonical>, f64)>,
        stocks: Vec<Ident<Canonical>>,
    ) -> Self {
        let mut adj: HashMap<Ident<Canonical>, Vec<ScoredEdge>> = HashMap::new();

        for (from, to, score) in edges {
            // Treat NaN as 0
            let score = if score.is_nan() { 0.0 } else { score };
            if score == 0.0 {
                continue;
            }
            adj.entry(from).or_default().push(ScoredEdge { to, score });
        }

        // Sort each edge list by |score| descending
        for edges in adj.values_mut() {
            edges.sort_by(|a, b| {
                b.score
                    .abs()
                    .partial_cmp(&a.score.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        SearchGraph { adj, stocks }
    }

    /// Build from simulation results at a specific timestep.
    ///
    /// Scans results.offsets for variables matching the LTM link score prefix
    /// `$⁚ltm⁚link_score⁚{from}→{to}`, reads values at the given step,
    /// and builds the adjacency list.
    fn from_results(
        results: &Results,
        step: usize,
        link_offsets: &[LinkOffset],
        stocks: &[Ident<Canonical>],
    ) -> Self {
        let mut edges = Vec::with_capacity(link_offsets.len());

        for ((from, to), offset) in link_offsets {
            let value = results.data[step * results.step_size + *offset];
            // Use absolute value for the search graph; NaN -> 0
            let abs_score = if value.is_nan() { 0.0 } else { value.abs() };
            edges.push((from.clone(), to.clone(), abs_score));
        }

        SearchGraph::from_edges(edges, stocks.to_vec())
    }

    /// Compute the SCC id of every node (adjacency keys, edge targets, and
    /// stocks) over this graph's edges. Returns the id map plus per-id sizes.
    ///
    /// The reference-oracle counterpart of the per-step SCC restriction in
    /// `IndexedSearch`: loops can only exist within a strongly-connected
    /// component, so each stock's DFS is confined to its own component.
    fn scc_map(&self) -> (HashMap<Ident<Canonical>, u32>, Vec<u32>) {
        // Assign dense indices to every node.
        let mut index_of: HashMap<Ident<Canonical>, u32> = HashMap::new();
        let mut order: Vec<Ident<Canonical>> = Vec::new();
        let intern = |id: &Ident<Canonical>,
                      index_of: &mut HashMap<Ident<Canonical>, u32>,
                      order: &mut Vec<Ident<Canonical>>| {
            *index_of.entry(id.clone()).or_insert_with(|| {
                order.push(id.clone());
                (order.len() - 1) as u32
            })
        };
        for (from, edges) in &self.adj {
            intern(from, &mut index_of, &mut order);
            for e in edges {
                intern(&e.to, &mut index_of, &mut order);
            }
        }
        for s in &self.stocks {
            intern(s, &mut index_of, &mut order);
        }

        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); order.len()];
        for (from, edges) in &self.adj {
            let fi = index_of[from] as usize;
            for e in edges {
                adj[fi].push(index_of[&e.to]);
            }
        }
        let (ids, sizes) = tarjan_scc_ids(&adj);
        let id_map = index_of
            .into_iter()
            .map(|(ident, idx)| (ident, ids[idx as usize]))
            .collect();
        (id_map, sizes)
    }

    /// Run the strongest-path search, returning discovered loop paths.
    ///
    /// Each returned path is a `Vec<Ident<Canonical>>` of variables forming
    /// the loop (not including the starting stock repeated at the end).
    ///
    /// Implements the algorithm from Appendix I of Eberlein & Schoenberg
    /// (2020), with the GH #647 scalability amendments: zero-score edges are
    /// excluded (`from_edges`), each stock's DFS is restricted to its own
    /// strongly-connected component (exact -- a loop cannot leave an SCC),
    /// and the paper's `best_score` pruning is replaced by a per-node
    /// expansion cap scaled to the component size
    /// ([`EXPANSION_BUDGET_PER_SEARCH`]).
    fn find_strongest_loops(&self) -> Vec<Vec<Ident<Canonical>>> {
        let mut found_loops: Vec<Vec<Ident<Canonical>>> = Vec::new();
        let mut seen_sets: HashSet<Vec<String>> = HashSet::new();

        let (scc_ids, scc_sizes) = self.scc_map();

        // For each stock, set TARGET = stock and run the DFS. Per-node
        // expansion counts reset per stock so one stock's search does not
        // limit loops reachable from another stock (the same isolation the
        // paper's per-stock `best_score` reset provided -- Section 12.5).
        for stock in &self.stocks {
            let stock_scc = scc_ids[stock];
            // A stock in a single-node component can only be on a loop if it
            // has a self-edge.
            if scc_sizes[stock_scc as usize] < 2
                && !self
                    .adj
                    .get(stock)
                    .is_some_and(|edges| edges.iter().any(|e| &e.to == stock))
            {
                continue;
            }

            let per_node_cap = (EXPANSION_BUDGET_PER_SEARCH / scc_sizes[stock_scc as usize]).max(1);
            let mut expansions: HashMap<Ident<Canonical>, u32> = HashMap::new();

            let mut visiting: HashSet<Ident<Canonical>> = HashSet::new();
            let mut stack: Vec<Ident<Canonical>> = Vec::new();

            self.check_outbound_uses(
                stock,
                stock,
                stock_scc,
                &scc_ids,
                per_node_cap,
                &mut visiting,
                &mut stack,
                &mut expansions,
                &mut found_loops,
                &mut seen_sets,
            );
        }

        found_loops
    }

    /// Recursive DFS from Appendix I of the paper, amended per GH #647.
    ///
    /// `variable`: current variable being explored
    /// `target`: the stock we're trying to return to
    /// `target_scc` / `scc_ids`: the per-graph SCC restriction -- only edges
    ///   whose destination shares the target's component are followed
    /// `per_node_cap`: per-node expansion cap for this search
    /// `visiting`: set of variables on the current DFS path
    /// `stack`: the current path for recording discovered loops
    /// `expansions`: per-node expansion counts (reset for each target stock)
    ///
    /// Edges are walked in descending |score| order (established at graph
    /// build), which is where the "strongest path" character of the search
    /// lives; accumulated path products are not tracked.
    ///
    /// Recursion depth is bounded by the number of unique variables in the model
    /// (the `visiting` set prevents revisiting nodes on the current path). For
    /// typical SD models (tens to low hundreds of variables) this is safe; very
    /// large models (1000+ variables) could in theory approach stack limits.
    #[allow(clippy::too_many_arguments)]
    fn check_outbound_uses(
        &self,
        variable: &Ident<Canonical>,
        target: &Ident<Canonical>,
        target_scc: u32,
        scc_ids: &HashMap<Ident<Canonical>, u32>,
        per_node_cap: u32,
        visiting: &mut HashSet<Ident<Canonical>>,
        stack: &mut Vec<Ident<Canonical>>,
        expansions: &mut HashMap<Ident<Canonical>, u32>,
        found_loops: &mut Vec<Vec<Ident<Canonical>>>,
        seen_sets: &mut HashSet<Vec<String>>,
    ) {
        // If variable.visiting is true:
        if visiting.contains(variable) {
            // If variable = TARGET: found a loop
            if variable == target {
                Self::add_loop_if_unique(stack, found_loops, seen_sets);
            }
            return;
        }

        // Bounded re-expansion (see [`EXPANSION_BUDGET_PER_SEARCH`]).
        let expansion_count = expansions.entry(variable.clone()).or_insert(0);
        if *expansion_count >= per_node_cap {
            return;
        }
        *expansion_count += 1;

        // Set variable.visiting = true, add to stack
        visiting.insert(variable.clone());
        stack.push(variable.clone());

        // For each outbound edge (already sorted by |score| desc)
        if let Some(edges) = self.adj.get(variable) {
            for edge in edges {
                // Stay inside the target's component.
                if scc_ids.get(&edge.to) != Some(&target_scc) {
                    continue;
                }
                self.check_outbound_uses(
                    &edge.to,
                    target,
                    target_scc,
                    scc_ids,
                    per_node_cap,
                    visiting,
                    stack,
                    expansions,
                    found_loops,
                    seen_sets,
                );
            }
        }

        // Set variable.visiting = false, remove from stack
        visiting.remove(variable);
        stack.pop();
    }

    /// Add loop to results if it hasn't been seen before, deduplicated
    /// by **canonical edge-sequence rotation** (see
    /// `crate::ltm::canonical_rotation`).  Two distinct directed cycles
    /// over the same node set (e.g. `A -> B -> C -> A` and
    /// `A -> C -> B -> A` on a multidigraph) canonicalize to different
    /// rotations and are correctly retained as separate loops --
    /// matching the elementary-circuit identity used by the LTM
    /// literature and the exhaustive enumerator in `ltm/indexed.rs`.
    /// Issue #308.
    fn add_loop_if_unique(
        stack: &[Ident<Canonical>],
        found_loops: &mut Vec<Vec<Ident<Canonical>>>,
        seen_sets: &mut HashSet<Vec<String>>,
    ) {
        if stack.is_empty() {
            return;
        }

        let path: Vec<String> = stack.iter().map(|id| id.as_str().to_string()).collect();
        let key = crate::ltm::canonical_rotation(&path);

        if seen_sets.insert(key) {
            found_loops.push(stack.to_vec());
        }
    }
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

fn parse_link_offsets(
    results: &Results,
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
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
            // Bare A2A link score: expand to N element-level edges
            // with the source AND target both subscripted.
            expand_a2a_link_offsets(from_str, to_str, offset, var_dims, dims, &mut entries);
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
    // Without this, `SearchGraph::from_results` would register parallel
    // edges and `discover_loops_with_graph::link_offset_map` would pick
    // one nondeterministically (HashMap iteration order over
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
    // HashMap iteration above is not). Downstream `SearchGraph` sorts
    // its adjacency lists by score, but the input order influences
    // tie-breaking and reproducibility of intermediate diagnostics.
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

/// Expand an A2A link score into per-element `LinkOffset` entries.
///
/// Given a link score name like `birth_rate→births` with dimensions
/// `["Region"]` and base offset, produces one entry per element:
/// `(birth_rate[nyc], births[nyc])` at `base + 0`,
/// `(birth_rate[boston], births[boston])` at `base + 1`, etc.
///
/// The element order matches the layout allocation order: row-major
/// cartesian product of dimension elements.
fn expand_a2a_link_offsets(
    from_var: &str,
    to_var: &str,
    base_offset: usize,
    var_dims: &[String],
    dims: &[datamodel::Dimension],
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

    for (idx, elems) in tuples.iter().enumerate() {
        let subscript = subscript_from_elements(elems);
        let from = Ident::new(&format!("{from_var}[{subscript}]"));
        let to = Ident::new(&format!("{to_var}[{subscript}]"));
        link_offsets.push(((from, to), base_offset + idx));
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

/// Run the strongest-path loop discovery on simulation results.
///
/// Reads link score values from `results` (computed during simulation via
/// LTM synthetic variables), then runs the strongest-path DFS at each saved
/// timestep to discover important loops.
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
    Ok(discover_loops_with_graph(
        results,
        &causal_graph,
        &stocks,
        &[],
        &[],
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
/// The accumulated composite payload for one collapsed edge: its current
/// (strongest-path) polarity and its composite score series (`None` until a
/// scored path contributes).
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

/// Integer-indexed search graph shared across all timesteps.
///
/// The graph *topology* -- which `(from -> to)` edges exist and which
/// result slot each reads its score from -- is identical at every saved
/// timestep; only the per-edge score value changes. `SearchGraph::from_results`
/// rebuilt the entire `HashMap<Ident, Vec<ScoredEdge>>` (cloning every `Ident`,
/// re-hashing every name) for each of the ~250 timesteps, and the DFS then
/// keyed `best_score`/`visiting`/`adj` on `Ident` -- each access hashing the
/// full (often long, element-level) identifier string. For a model like
/// C-LEARN (thousands of edges, hundreds of stocks, 250 steps) that string
/// hashing dominated, pushing discovery past 19 minutes.
///
/// This structure hoists the topology build once: every node that appears as
/// a `from` or `to` endpoint (or is a stock) gets a dense `u32` id, edges are
/// stored as `(to_id, result_offset)` in their original `link_offsets` order,
/// and the per-timestep DFS runs entirely over integer-indexed `Vec`s. The
/// result set matches the `SearchGraph` test oracle: same node universe, same
/// per-step zero-edge exclusion and SCC restriction, same per-node expansion
/// caps, same stable score-descending edge order, same NaN->0 handling, same
/// canonical rotation dedup.
struct IndexedSearch {
    /// node id -> canonical identifier (for reconstructing discovered paths)
    idents: Vec<Ident<Canonical>>,
    /// node id -> outbound edges, in original `link_offsets` insertion order.
    /// The per-timestep DFS re-sorts a permutation of each list by |score|;
    /// the static topology here never changes.
    adj: Vec<Vec<IndexedEdge>>,
    /// stock node ids, in the input `stocks` order (drives per-stock DFS order)
    stock_ids: Vec<u32>,
}

/// A static outbound edge: target node id plus the result slot its score is
/// read from each timestep.
struct IndexedEdge {
    to: u32,
    offset: usize,
}

/// A timestep-resolved outbound edge: target node id and the |score| weight
/// that orders edge exploration (strongest first). Built per timestep, already
/// sorted by `score` descending (stable), so the DFS never sorts per visit.
#[derive(Clone, Copy)]
struct StepEdge {
    to: u32,
    score: f64,
}

/// How many DFS node visits to allow between wall-clock deadline checks.
///
/// Reading `Instant::now()` on every visit would dominate the per-visit cost
/// on dense graphs, so the check is amortized: with a power-of-two interval
/// the counter test is a single mask. 8192 visits is well under a millisecond
/// of DFS work, so deadline overshoot stays negligible while clock reads stay
/// under 0.1% of visits.
const DEADLINE_CHECK_INTERVAL: u32 = 8192;

/// Total node-expansion budget for one (stock, timestep) search, divided by
/// the size of the stock's strongly-connected component to give the per-node
/// expansion cap: `per_node_cap = max(1, EXPANSION_BUDGET_PER_SEARCH / |SCC|)`.
///
/// This cap replaces the paper's `best_score` pruning as the search's
/// work-bounding mechanism. The paper's pruning (`score < best_score`)
/// assumes path-score products shrink as paths extend; two score patterns
/// common in real models defeat it (GH #647): exact ties (chains of
/// single-input links all score exactly 1.0, so re-arrivals are never
/// strictly weaker) and super-unit scores (links with |score| > 1 -- C-LEARN
/// has hundreds per timestep -- make products *grow* along paths). Each
/// non-pruned re-arrival re-explores the node's full subtree, which is
/// exponential in parallel-path structures. Worse, when the pruning *does*
/// fire it costs completeness: a loop whose entry path is weaker than an
/// already-explored sibling's is silently dropped (the paper's Figure 7
/// failure mode).
///
/// The expansion cap inverts the trade-off. Work is bounded at
/// `per_node_cap * SCC_edges` traversals per (stock, step) -- so roughly
/// `EXPANSION_BUDGET_PER_SEARCH * avg_degree` regardless of component size --
/// while small components (the common case once the search is restricted to
/// per-step nonzero-score SCCs) get effectively exhaustive enumeration:
/// every elementary cycle through the stock is found, strictly better than
/// the paper's heuristic. Edges are explored in descending score order, so
/// when the cap does bind on a large component, the strongest paths are the
/// ones already explored.
const EXPANSION_BUDGET_PER_SEARCH: u32 = 4096;

/// Per-timestep mutable DFS state, allocated once per `discover_loops_with_graph`
/// call and reused across stocks and timesteps to avoid per-search reallocation.
struct DfsScratch {
    /// node id -> whether it is on the current DFS path. A per-stock generation
    /// stamp avoids clearing the whole vector between stocks: a node is
    /// "visiting" iff `visiting_gen[id] == cur_gen`.
    visiting_gen: Vec<u32>,
    /// current generation counter for `visiting_gen`
    cur_gen: u32,
    /// current DFS path as node ids (mirrors the original `stack` of idents)
    stack: Vec<u32>,
    /// node id -> outbound edges for the current timestep, already sorted by
    /// `|score|` descending (stable). Rebuilt once per timestep by
    /// `load_step_scores`; the DFS reads it without sorting -- mirroring the
    /// original `SearchGraph::from_results`, which sorted each adjacency list
    /// once per timestep, NOT once per node visit. The per-visit sort was the
    /// dominant DFS cost on a dense element graph (a node is re-entered many
    /// times across the 116-stock x 250-step search).
    ///
    /// Edges whose |score| is 0 (or NaN) at the current timestep are
    /// excluded: a loop containing such a link has loop score exactly 0 at
    /// this step, so it cannot be a "loop that matters" here. A loop whose
    /// links are all simultaneously nonzero at some sampled step remains
    /// discoverable there (GH #647); a loop whose links are only ever
    /// active at *different* sampled steps is never discoverable (see
    /// `SearchGraph::from_edges` for the full caveat).
    step_adj: Vec<Vec<StepEdge>>,
    /// node id -> strongly-connected-component id of the current timestep's
    /// nonzero-score subgraph. Computed once per step by `discover_step`.
    /// Feedback loops can only exist within an SCC, so each stock's DFS is
    /// restricted to its own component -- exploration outside it is provably
    /// wasted work (GH #647).
    scc_ids: Vec<u32>,
    /// SCC id -> component node count for the current timestep.
    scc_sizes: Vec<u32>,
    /// Reusable projection buffer (`step_adj` stripped to target ids) for the
    /// per-step Tarjan SCC computation. Inner `Vec`s keep their capacity
    /// across steps.
    scc_adj: Vec<Vec<u32>>,
    /// node id -> number of times this node has been expanded in the current
    /// (stock, timestep) search. See [`EXPANSION_BUDGET_PER_SEARCH`].
    expansions: Vec<u32>,
    /// Per-node expansion cap for the current (stock, timestep) search:
    /// `max(1, EXPANSION_BUDGET_PER_SEARCH / |stock's SCC|)`. Set per stock by
    /// `discover_step`.
    per_node_cap: u32,
    /// Wall-clock deadline for the discovery sweep, or `None` for an
    /// unbudgeted run (which never reads the clock). On a graph where a
    /// SINGLE timestep's DFS can run for hours (GH #647's element-level
    /// blowup), checking the budget only between timesteps is not enough to
    /// honor the caller's time budget -- the DFS itself must notice expiry.
    deadline: Option<Instant>,
    /// Node-visit counter used to amortize deadline clock reads.
    visit_count: u32,
    /// Set once `deadline` has passed; every DFS level then unwinds
    /// immediately and the sweep stops.
    deadline_expired: bool,
}

impl DfsScratch {
    /// Allocate reusable DFS state sized for `search`'s node universe, with each
    /// node's per-timestep edge buffer pre-reserved to its static out-degree.
    fn new(search: &IndexedSearch) -> Self {
        let n_nodes = search.node_count();
        DfsScratch {
            visiting_gen: vec![0; n_nodes],
            cur_gen: 0,
            stack: Vec::with_capacity(n_nodes),
            step_adj: search
                .adj
                .iter()
                .map(|e| Vec::with_capacity(e.len()))
                .collect(),
            scc_ids: vec![0; n_nodes],
            scc_sizes: Vec::new(),
            scc_adj: vec![Vec::new(); n_nodes],
            expansions: vec![0; n_nodes],
            per_node_cap: EXPANSION_BUDGET_PER_SEARCH,
            deadline: None,
            visit_count: 0,
            deadline_expired: false,
        }
    }

    /// Recompute the per-step SCC structure from the (already loaded,
    /// zero-score-edge-free) `step_adj`.
    fn compute_step_sccs(&mut self) {
        for (node, row) in self.step_adj.iter().enumerate() {
            let proj = &mut self.scc_adj[node];
            proj.clear();
            proj.extend(row.iter().map(|e| e.to));
        }
        let (ids, sizes) = tarjan_scc_ids(&self.scc_adj);
        self.scc_ids = ids;
        self.scc_sizes = sizes;
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
        // `link_offsets` insertion order within each `from` node so the
        // per-timestep stable score sort breaks ties identically to the
        // original `SearchGraph::from_edges` (which pushed in the same order
        // before its stable `sort_by`).
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

        // Stocks that never appeared as an edge endpoint still need ids (the
        // DFS starts from every stock; a stock with no outbound edges simply
        // has an empty adjacency list, matching the original behavior).
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

    /// Rebuild each node's sorted outbound-edge list for `step` into
    /// `scratch.step_adj`.
    ///
    /// Reads each edge's result slot, applies the same NaN->0 then |value|
    /// transform `SearchGraph::from_results`/`from_edges` did, then stable-sorts
    /// the node's edges by `|score|` descending -- exactly the per-timestep sort
    /// `SearchGraph::from_results` performed once per node. Doing it here (once
    /// per timestep) rather than inside the DFS (once per node *visit*) is the
    /// key cost reduction without changing the visited edge order.
    ///
    /// Zero-score edges (including NaN, which maps to 0) are dropped from the
    /// per-step graph entirely: any loop through them has loop score exactly 0
    /// at this step, so traversing them cannot surface a loop that matters
    /// here, and on real models the zero edges are the overwhelming majority
    /// (~94% of C-LEARN's edges at any given step -- GH #647).
    fn load_step_scores(&self, results: &Results, step: usize, scratch: &mut DfsScratch) {
        let base = step * results.step_size;
        for (node, edges) in self.adj.iter().enumerate() {
            let row = &mut scratch.step_adj[node];
            row.clear();
            for edge in edges {
                let value = results.data[base + edge.offset];
                let score = if value.is_nan() { 0.0 } else { value.abs() };
                if score != 0.0 {
                    row.push(StepEdge { to: edge.to, score });
                }
            }

            // Stable sort by score descending. `sort_by` is stable, so ties keep
            // the `link_offsets` insertion order -- byte-identical to the
            // original `SearchGraph::from_edges`/`from_results` ordering.
            row.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }

    /// Run the strongest-path search at the current step, appending newly
    /// discovered loops (deduped by canonical rotation) to `all_paths`.
    ///
    /// Equivalent to `SearchGraph::from_results(..).find_strongest_loops()`
    /// followed by the caller's cross-step rotation dedup, but without
    /// rebuilding the graph or hashing idents in the inner loop. The single
    /// `seen_sets` passed in spans all timesteps, so the original's two dedup
    /// layers (per-timestep inside `find_strongest_loops`, then cross-timestep
    /// in `discover_loops_with_graph`) collapse to one without changing which
    /// paths survive: a path is kept iff its canonical rotation is new, and the
    /// per-stock/per-step visitation order is preserved.
    fn discover_step(
        &self,
        scratch: &mut DfsScratch,
        seen_sets: &mut HashSet<Vec<u32>>,
        all_paths: &mut Vec<Vec<Ident<Canonical>>>,
    ) {
        // Identify the step's cyclic core: the SCCs of the nonzero-score
        // subgraph. Each stock's DFS is restricted to its own component, and
        // stocks outside any cycle are skipped outright -- on large models
        // this is the difference between searching a ~65-node component and
        // wandering a ~4,700-node graph (GH #647).
        scratch.compute_step_sccs();

        for stock_idx in 0..self.stock_ids.len() {
            // A deadline that expired during the previous stock's DFS ends the
            // whole step: the caller observes `deadline_expired` and truncates.
            if scratch.deadline_expired {
                return;
            }
            let stock = self.stock_ids[stock_idx];
            let stock_scc = scratch.scc_ids[stock as usize];

            // A stock in a single-node component can only be on a loop if it
            // has a self-edge; otherwise no loop through it exists at this
            // step and the whole search is skipped.
            if scratch.scc_sizes[stock_scc as usize] < 2
                && !scratch.step_adj[stock as usize]
                    .iter()
                    .any(|e| e.to == stock)
            {
                continue;
            }

            // Reset per-node expansion counts for this target stock and size
            // its expansion cap to the component: small components get
            // effectively exhaustive enumeration, large ones stay bounded
            // (see [`EXPANSION_BUDGET_PER_SEARCH`]). A fresh generation marks
            // the visiting set empty without touching every slot.
            scratch.expansions.iter_mut().for_each(|e| *e = 0);
            scratch.per_node_cap =
                (EXPANSION_BUDGET_PER_SEARCH / scratch.scc_sizes[stock_scc as usize]).max(1);
            scratch.cur_gen = scratch.cur_gen.wrapping_add(1);
            if scratch.cur_gen == 0 {
                // Generation wrapped; clear so a stale stamp can't read as live.
                scratch.visiting_gen.iter_mut().for_each(|g| *g = 0);
                scratch.cur_gen = 1;
            }
            scratch.stack.clear();

            self.dfs(stock, stock, stock_scc, scratch, seen_sets, all_paths);
        }
    }

    /// Recursive DFS mirroring `SearchGraph::check_outbound_uses`, but over
    /// integer node ids and the pre-sorted per-timestep edge lists. The edge
    /// order is established once per timestep in `load_step_scores`, so the DFS
    /// just walks `scratch.step_adj[node]` -- no per-visit sorting.
    ///
    /// `target_scc` is the per-step SCC id of `target`; only edges whose
    /// destination is in the same component are followed. A path that leaves
    /// the component can never return to `target` (mutual reachability is
    /// exactly what defines the SCC), so the restriction loses no loops.
    ///
    /// The "strongest path" character of the search lives in the edge order:
    /// each node's edges are walked in descending |score| order, so when the
    /// per-node expansion cap binds, the strongest paths are the ones that
    /// have been explored. Accumulated path products are not tracked -- loop
    /// scores are recomputed exactly from the link-score series after
    /// discovery.
    fn dfs(
        &self,
        node: u32,
        target: u32,
        target_scc: u32,
        scratch: &mut DfsScratch,
        seen_sets: &mut HashSet<Vec<u32>>,
        all_paths: &mut Vec<Vec<Ident<Canonical>>>,
    ) {
        // Once the deadline has passed every level unwinds immediately, so an
        // expired budget collapses even a deep recursion in O(depth) time. The
        // clock itself is read only every DEADLINE_CHECK_INTERVAL visits --
        // checking it per visit would dominate the per-visit cost.
        if scratch.deadline_expired {
            return;
        }
        scratch.visit_count = scratch.visit_count.wrapping_add(1);
        if scratch.visit_count & (DEADLINE_CHECK_INTERVAL - 1) == 0
            && let Some(deadline) = scratch.deadline
            && Instant::now() >= deadline
        {
            scratch.deadline_expired = true;
            return;
        }

        let idx = node as usize;

        if scratch.visiting_gen[idx] == scratch.cur_gen {
            if node == target {
                self.record_loop(&scratch.stack, seen_sets, all_paths);
            }
            return;
        }

        // Bounded re-expansion -- the search's work-bounding mechanism,
        // replacing the paper's `best_score` pruning (which both blows up on
        // tied/super-unit scores and silently drops sibling loops). See
        // [`EXPANSION_BUDGET_PER_SEARCH`].
        if scratch.expansions[idx] >= scratch.per_node_cap {
            return;
        }
        scratch.expansions[idx] += 1;

        scratch.visiting_gen[idx] = scratch.cur_gen;
        scratch.stack.push(node);

        // Walk the node's pre-sorted (|score| desc) edge list. `step_adj` is not
        // mutated during the DFS, so we index it by position and copy each
        // `StepEdge` (it is `Copy`) -- this keeps the recursive `&mut scratch`
        // borrow legal without cloning the whole row.
        let n_edges = scratch.step_adj[idx].len();
        for k in 0..n_edges {
            let edge = scratch.step_adj[idx][k];
            // Stay inside the target's component (see fn doc).
            if scratch.scc_ids[edge.to as usize] != target_scc {
                continue;
            }
            self.dfs(edge.to, target, target_scc, scratch, seen_sets, all_paths);
        }

        scratch.visiting_gen[idx] = 0;
        scratch.stack.pop();
    }

    /// Record the current path as a loop if its canonical rotation is new,
    /// mirroring `SearchGraph::add_loop_if_unique` over reconstructed idents.
    ///
    /// The dedup key is the canonical rotation of the path's *node ids* rather
    /// than its identifier strings. Within a single `IndexedSearch` the id <->
    /// string map is a bijection, so two paths are rotations of one another in
    /// id space iff they are in string space: the dedup equivalence classes are
    /// identical. Keying on `u32` avoids allocating a `Vec<String>` (and hashing
    /// long element-level names) on every loop closure -- the dominant
    /// remaining per-closure cost. `canonical_rotation` over ids picks a
    /// (possibly different) representative rotation, but that representative is
    /// only used as a set key; the *stored* loop is the original `stack`, so the
    /// reported paths and their first-seen order are unchanged.
    fn record_loop(
        &self,
        stack: &[u32],
        seen_sets: &mut HashSet<Vec<u32>>,
        all_paths: &mut Vec<Vec<Ident<Canonical>>>,
    ) {
        if stack.is_empty() {
            return;
        }
        let key = crate::ltm::canonical_rotation(stack);
        if seen_sets.insert(key) {
            all_paths.push(
                stack
                    .iter()
                    .map(|&id| self.idents[id as usize].clone())
                    .collect(),
            );
        }
    }
}

/// Iterative Tarjan strongly-connected components over a dense
/// integer-indexed adjacency list.
///
/// Returns `(component_id_per_node, component_sizes)`: two nodes share a
/// component id iff they are mutually reachable, and `sizes[id]` is that
/// component's node count. Component ids are dense but otherwise arbitrary.
///
/// Used by discovery to identify each graph's *cyclic core*: feedback loops
/// can only exist within a strongly-connected component, so any DFS work
/// outside the target stock's SCC is provably wasted (GH #647).
fn tarjan_scc_ids(adj: &[Vec<u32>]) -> (Vec<u32>, Vec<u32>) {
    const UNVISITED: i32 = -1;
    let n = adj.len();
    let mut indices: Vec<i32> = vec![UNVISITED; n];
    let mut lowlinks: Vec<i32> = vec![0; n];
    let mut on_stack: Vec<bool> = vec![false; n];
    let mut stack: Vec<u32> = Vec::new();
    let mut comp_ids: Vec<u32> = vec![0; n];
    let mut comp_sizes: Vec<u32> = Vec::new();
    let mut next_index: i32 = 0;

    // Iterative frames mirroring `ltm::indexed::IndexedGraph::tarjan_scc`:
    // Enter pushes a node onto Tarjan's stack; Resume continues iterating its
    // successors and pops the SCC when this node is its own root.
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
                    let succs = &adj[v as usize];
                    if (next_child as usize) < succs.len() {
                        let w = succs[next_child as usize];
                        frames.push(Frame::Resume {
                            v,
                            next_child: next_child + 1,
                        });
                        if indices[w as usize] == UNVISITED {
                            frames.push(Frame::Enter(w));
                        } else if on_stack[w as usize] && indices[w as usize] < lowlinks[v as usize]
                        {
                            lowlinks[v as usize] = indices[w as usize];
                        }
                    } else {
                        if let Some(Frame::Resume { v: parent, .. }) = frames.last()
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

    (comp_ids, comp_sizes)
}

/// Per-sampled-timestep statistics about the discovery search graph.
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
    /// Edges with |score| > 1 at this step. These defeat the strongest-path
    /// pruning assumption that path products shrink as paths extend.
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

/// Structural statistics about a discovery search graph, quantifying why the
/// strongest-path DFS is or is not tractable on a given model (GH #647).
///
/// This is the diagnostics surface behind the discovery-feasibility
/// benchmarks (`examples/clearn_discover.rs`). It reports the graph's size,
/// its cyclic core (SCC structure -- the only place loops can live), and how
/// many edges actually carry signal at sampled timesteps.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct DiscoveryGraphStats {
    /// Total nodes in the search graph (link-score edge endpoints + stocks).
    pub n_nodes: usize,
    /// Total directed edges (parsed link-score columns, post-A2A expansion).
    pub n_edges: usize,
    /// Number of stocks (DFS start points).
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
    sample_steps: &[usize],
) -> DiscoveryGraphStats {
    let link_offsets = parse_link_offsets(results, ltm_vars, dims);
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
/// (GH #698 / PR #705 r3353758167). NOTE: an arrayed loop through a multi-output
/// module is not yet discoverable end-to-end -- a scalar module output feeding
/// an arrayed reader emits a single scalar constant-0 link score that drops the
/// loop (GH #716) -- so this is currently latent parity defense.
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

/// Run the strongest-path loop discovery using a pre-built `CausalGraph`.
///
/// This is the implementation shared by `discover_loops` (which builds
/// the graph from a `Project`) and callers that have a salsa-derived
/// `CausalGraph`.
///
/// When `ltm_vars` and `dims` are provided, A2A link scores are expanded
/// into per-element edges so the DFS operates on the element-level graph.
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
/// `budget` optionally bounds the wall-clock time spent in the per-timestep DFS
/// sweep. Expiry is checked both between timesteps and *inside* each step's
/// DFS (every `DEADLINE_CHECK_INTERVAL` node visits), so even a model whose
/// single-step DFS would run for hours (GH #647) returns within roughly the
/// budget. The returned `DiscoveryResult::truncated` records whether the
/// budget elapsed before the sweep finished. A `None` budget runs to
/// completion. Note the budget covers only this discovery sweep -- the
/// caller's compilation and simulation time are outside it.
pub fn discover_loops_with_graph(
    results: &Results,
    causal_graph: &CausalGraph,
    stocks: &[Ident<Canonical>],
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
    sub_model_output_ports: &SubModelOutputPorts,
    budget: Option<Duration>,
) -> Result<DiscoveryResult> {
    let link_offsets = parse_link_offsets(results, ltm_vars, dims);
    if link_offsets.is_empty() {
        return Ok(DiscoveryResult {
            loops: Vec::new(),
            partitions: Vec::new(),
            truncated: false,
            agg_recovery_truncated: false,
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
        });
    }

    // Collect all unique loop paths across all timesteps, dedup-keyed
    // on the canonical edge-sequence rotation so opposite-direction
    // cycles over the same node set are kept as distinct loops (see
    // `crate::ltm::canonical_rotation` and issue #308). The key is the
    // rotation of the path's integer node ids (a bijection with the
    // string names within this search), so the dedup classes match the
    // string-keyed original without allocating a string per closure.
    let mut all_paths: Vec<Vec<Ident<Canonical>>> = Vec::new();
    let mut seen_sets: HashSet<Vec<u32>> = HashSet::new();

    let step_count = results.step_count;

    // Hoist the integer-indexed topology build out of the per-timestep loop:
    // the graph's edges and result slots are step-invariant, so rebuilding the
    // `Ident`-keyed `SearchGraph` (and re-hashing every name) per step was pure
    // waste. Only the per-edge scores change, which the DFS re-reads each step.
    let search = IndexedSearch::build(&link_offsets, stocks);
    let mut scratch = DfsScratch::new(&search);

    // Skip step 0 where link scores are NaN (PREVIOUS values don't exist).
    // The original ran a fresh per-step `find_strongest_loops` (whose own
    // per-step `seen_sets` deduped within a step) and then deduped again across
    // steps here; both layers keyed on the canonical rotation, so collapsing
    // them onto the single cross-step `seen_sets` keeps exactly the paths the
    // two-layer version kept, in the same first-seen order.
    //
    // The optional time budget is enforced at two granularities. Between
    // timesteps, the cheap check below stops before starting a step we can't
    // afford. *Within* a step, the DFS itself checks the same deadline every
    // `DEADLINE_CHECK_INTERVAL` node visits (see `IndexedSearch::dfs`):
    // on dense element-level graphs a SINGLE step's DFS can run for hours
    // (GH #647), so a between-steps-only check would not honor the budget at
    // all. Loops recorded before mid-step expiry are kept -- each is a real,
    // fully-traversed cycle, so partial-step results are still valid loops.
    // `start` is captured lazily so an unbudgeted run never reads the clock.
    let mut truncated = false;
    let start = budget.map(|_| Instant::now());
    scratch.deadline = budget.zip(start).map(|(limit, started)| started + limit);
    for step in 1..step_count {
        if let (Some(limit), Some(started)) = (budget, start)
            && started.elapsed() >= limit
        {
            truncated = true;
            break;
        }
        search.load_step_scores(results, step, &mut scratch);
        search.discover_step(&mut scratch, &mut seen_sets, &mut all_paths);
        if scratch.deadline_expired {
            truncated = true;
            break;
        }
    }

    if all_paths.is_empty() {
        return Ok(DiscoveryResult {
            loops: Vec::new(),
            partitions: Vec::new(),
            truncated,
            agg_recovery_truncated: false,
        });
    }

    // Stitch cross-element-through-aggregate loops (GH #696). The discovery DFS
    // emits only *elementary* element-graph circuits, so a feedback loop that
    // traverses a hoisted reducer's synthetic agg node more than once
    // (`pop[a] → agg → pop[b] → agg → pop[a]`) is structurally unreachable --
    // its `visiting` set forbids revisiting the agg. Exhaustive mode recovers
    // these by stitching the single-agg "petals" together; do the same here,
    // reusing the SAME combinatorial core (`stitch_cross_agg_petals`) so
    // discovery recovers exactly the loops exhaustive does. Each discovered
    // elementary path that visits one agg once IS a petal; the stitched
    // sequences are appended to `all_paths` and flow through the identical
    // FoundLoop construction / trim / dedup / rank pipeline below. The stitched
    // loop's edge multiset is the union of its petals' (disjoint) edges, so the
    // per-step loop score read from `link_offset_map` is exactly the product of
    // the petals' link scores -- scored identically to any discovered loop.
    let agg_recovery_truncated = {
        // `collect_agg_petals` keys the petal map on `&str` agg names borrowed
        // from `all_paths`, but the stitched sequences own their `Ident` nodes,
        // so compute the (owned) stitched paths + truncation flag in an inner
        // scope that ends the immutable borrow before we push to `all_paths`.
        let (stitched, was_truncated) = {
            let petals_by_agg = crate::db::collect_agg_petals(&all_paths, |id| id.as_str());
            let mut sorted: Vec<(&str, Vec<crate::db::StitchPetal<Ident<Canonical>>>)> =
                petals_by_agg.into_iter().collect();
            sorted.sort_by(|a, b| a.0.cmp(b.0));
            let (stitched, truncated_aggs) =
                crate::db::stitch_cross_agg_petals(sorted, crate::db::cross_agg_loop_budget());
            (stitched, !truncated_aggs.is_empty())
        };
        // Append each stitched cycle as a new path, deduped against the already-
        // discovered paths (and each other) by canonical-rotation of the node
        // names -- the same dedup notion `seen_sets` uses above, so a stitched
        // loop never duplicates an elementary one.
        let mut seen_strs: HashSet<Vec<String>> = all_paths
            .iter()
            .map(|p| {
                crate::ltm::canonical_rotation(
                    &p.iter().map(|n| n.as_str().to_string()).collect::<Vec<_>>(),
                )
            })
            .collect();
        for seq in stitched {
            let key = crate::ltm::canonical_rotation(
                &seq.iter()
                    .map(|n| n.as_str().to_string())
                    .collect::<Vec<_>>(),
            );
            if seen_strs.insert(key) {
                all_paths.push(seq);
            }
        }
        was_truncated
    };

    // Convert paths to FoundLoop objects with scores
    let mut found_loops: Vec<FoundLoop> = Vec::new();

    for path in &all_paths {
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
                recompute_module_input_edge_series(
                    causal_graph,
                    results,
                    &links,
                    i,
                    step_count,
                    sub_model_output_ports,
                )
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
    for fl in found_loops {
        let nodes: Vec<String> = fl
            .loop_info
            .links
            .iter()
            .map(|l| l.from.as_str().to_string())
            .collect();
        let key = crate::ltm::canonical_rotation(&nodes);
        match by_reported_cycle.get(&key) {
            Some(&idx) => {
                if fl.avg_abs_score > deduped[idx].avg_abs_score {
                    deduped[idx] = fl;
                }
            }
            None => {
                by_reported_cycle.insert(key, deduped.len());
                deduped.push(fl);
            }
        }
    }
    let mut found_loops = deduped;

    let partitions = causal_graph.compute_cycle_partitions();
    let partition_meta = rank_and_filter(&mut found_loops, &partitions);

    Ok(DiscoveryResult {
        loops: found_loops,
        partitions: partition_meta,
        truncated,
        agg_recovery_truncated,
    })
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
    key: (String, Vec<String>),
}

/// Order two loops for ranking: active loops before never-active (`NaN`)
/// ones, competing loops before trivially-isolated (solo-partition) ones,
/// then descending mean relative importance, then content-based tie-breaking.
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
    by_nan
        .then(by_competing)
        .then(by_score)
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
fn rank_and_filter(
    found_loops: &mut Vec<FoundLoop>,
    partitions: &CyclePartitions,
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
    // parent-level partition (a pure module-internal loop) maps to `None`,
    // which groups with its peers exactly as any other partition key.
    let slot0 = |fl: &FoundLoop| -> Option<usize> {
        partitions
            .partition_for_loop(&fl.loop_info, &[])
            .first()
            .copied()
            .flatten()
    };
    let loop_partitions: Vec<Option<usize>> = found_loops.iter().map(slot0).collect();

    // Group loops by partition over the FULL discovered set (before retention
    // or cap).  Drives both the relative-score denominators below and the
    // competing-vs-solo classification: a loop is "competing" iff its
    // partition holds at least one other discovered loop, the same population
    // its denominator sums over -- so "solo" means exactly "its relative
    // score is ±1 by construction".
    let mut partition_groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, &partition) in loop_partitions.iter().enumerate() {
        partition_groups.entry(partition).or_default().push(i);
    }
    let competing: Vec<bool> = loop_partitions
        .iter()
        .map(|p| partition_groups[p].len() >= 2)
        .collect();

    // Per-partition per-timestep totals: Σ|score_j[t]| over the partition's
    // loops, NaN excluded (an undefined score is not signal; matches GH #542's
    // denom_summand). Inf is kept -- a real divergence at a dominance
    // inflection. Computed over ALL discovered loops, before any cap, so the
    // denominator reflects the whole partition (the truncate-before-filter
    // order of GH #310 used to compute totals over only the top-200 survivors).
    let mut partition_totals: HashMap<Option<usize>, Vec<f64>> = HashMap::new();
    if step_count > 0 {
        for (&partition, indices) in &partition_groups {
            let mut totals = vec![0.0; step_count];
            for &idx in indices {
                for (i, &(_, score)) in found_loops[idx].scores.iter().enumerate() {
                    if !score.is_nan() {
                        totals[i] += score.abs();
                    }
                }
            }
            partition_totals.insert(partition, totals);
        }
    }

    // Partition-aware MIN_CONTRIBUTION retention filter (peak semantics,
    // unchanged): keep a loop if at ANY single timestep its |score| is
    // >= MIN_CONTRIBUTION of its partition's total at that step. Runs BEFORE
    // the cap (GH #310).
    if step_count > 0 {
        let mut keep = vec![false; found_loops.len()];
        for (idx, fl) in found_loops.iter().enumerate() {
            let totals = &partition_totals[&loop_partitions[idx]];
            keep[idx] = fl.scores.iter().enumerate().any(|(i, &(_, score))| {
                !score.is_nan() && totals[i] > 0.0 && score.abs() / totals[i] >= MIN_CONTRIBUTION
            });
        }
        // Partitions and competing flags of the surviving loops, in the same
        // order `retain` will leave them, so the relative-importance pass
        // below indexes the right partition for each loop.
        let retained_partitions: Vec<Option<usize>> = loop_partitions
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
        debug_assert_eq!(retained_partitions.len(), found_loops.len());

        rank_truncate_and_id(
            found_loops,
            &retained_partitions,
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
/// `loop_partitions[i]` is the cycle partition of `found_loops[i]`,
/// `competing[i]` whether that partition holds at least one other discovered
/// loop, and `partition_totals` the per-partition per-timestep denominator --
/// all as built by `rank_and_filter` over the full discovered set.
fn rank_truncate_and_id(
    found_loops: &mut Vec<FoundLoop>,
    loop_partitions: &[Option<usize>],
    competing: &[bool],
    partition_totals: &HashMap<Option<usize>, Vec<f64>>,
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
            let totals = &partition_totals[&loop_partitions[idx]];
            let mean_rel = mean_relative_contribution(&fl, totals);
            let key = loop_sort_key(&fl.loop_info);
            fl.rel_scores = signed_relative_scores(&fl, totals);
            (
                RelativeImportance {
                    mean_rel,
                    competing: competing[idx],
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
/// sibling cycles so the stable-sort fallback no longer leaks the discovery
/// DFS's (process-order-dependent) emission order into the assigned ids.
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
