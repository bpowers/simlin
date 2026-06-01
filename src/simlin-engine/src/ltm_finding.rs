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

// --- Constants (from the paper) ---

/// Maximum loops to retain after discovery (paper uses 200)
const MAX_LOOPS: usize = 200;

/// Minimum average relative contribution to keep a loop (paper uses 0.1%)
const MIN_CONTRIBUTION: f64 = 0.001;

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
    /// Whether the time budget elapsed before discovery finished.
    pub truncated: bool,
}

#[cfg(test)]
impl SearchGraph {
    /// Build from a list of (from, to, abs_score) triples.
    fn from_edges(
        edges: Vec<(Ident<Canonical>, Ident<Canonical>, f64)>,
        stocks: Vec<Ident<Canonical>>,
    ) -> Self {
        let mut adj: HashMap<Ident<Canonical>, Vec<ScoredEdge>> = HashMap::new();

        for (from, to, score) in edges {
            // Treat NaN as 0
            let score = if score.is_nan() { 0.0 } else { score };
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

    /// Run the strongest-path search, returning discovered loop paths.
    ///
    /// Each returned path is a `Vec<Ident<Canonical>>` of variables forming
    /// the loop (not including the starting stock repeated at the end).
    ///
    /// Implements the algorithm from Appendix I of Eberlein & Schoenberg (2020).
    fn find_strongest_loops(&self) -> Vec<Vec<Ident<Canonical>>> {
        let mut found_loops: Vec<Vec<Ident<Canonical>>> = Vec::new();
        let mut seen_sets: HashSet<Vec<String>> = HashSet::new();

        // For each stock, set TARGET = stock and run the DFS.
        // Reset best_score per stock so one stock's search does not prune
        // loops reachable from another stock (Section 12.5 of the reference).
        for stock in &self.stocks {
            let mut best_score: HashMap<Ident<Canonical>, f64> = HashMap::new();
            for var in self.adj.keys() {
                best_score.insert(var.clone(), 0.0);
            }
            for s in &self.stocks {
                best_score.entry(s.clone()).or_insert(0.0);
            }

            let mut visiting: HashSet<Ident<Canonical>> = HashSet::new();
            let mut stack: Vec<Ident<Canonical>> = Vec::new();

            self.check_outbound_uses(
                stock,
                1.0,
                stock,
                &mut visiting,
                &mut stack,
                &mut best_score,
                &mut found_loops,
                &mut seen_sets,
            );
        }

        found_loops
    }

    /// Recursive DFS from Appendix I of the paper.
    ///
    /// `variable`: current variable being explored
    /// `score`: accumulated path score (product of |link_scores| along the path)
    /// `target`: the stock we're trying to return to
    /// `visiting`: set of variables on the current DFS path
    /// `stack`: the current path for recording discovered loops
    /// `best_score`: highest score seen at each variable (reset for each target stock)
    ///
    /// Recursion depth is bounded by the number of unique variables in the model
    /// (the `visiting` set prevents revisiting nodes on the current path). For
    /// typical SD models (tens to low hundreds of variables) this is safe; very
    /// large models (1000+ variables) could in theory approach stack limits.
    #[allow(clippy::too_many_arguments)]
    fn check_outbound_uses(
        &self,
        variable: &Ident<Canonical>,
        score: f64,
        target: &Ident<Canonical>,
        visiting: &mut HashSet<Ident<Canonical>>,
        stack: &mut Vec<Ident<Canonical>>,
        best_score: &mut HashMap<Ident<Canonical>, f64>,
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

        // If score < variable.best_score: prune (strict less-than)
        let current_best = best_score.get(variable).copied().unwrap_or(0.0);
        if score < current_best {
            return;
        }

        // Set variable.best_score = score
        best_score.insert(variable.clone(), score);

        // Set variable.visiting = true, add to stack
        visiting.insert(variable.clone());
        stack.push(variable.clone());

        // For each outbound edge (already sorted by |score| desc)
        if let Some(edges) = self.adj.get(variable) {
            for edge in edges {
                self.check_outbound_uses(
                    &edge.to,
                    score * edge.score.abs(),
                    target,
                    visiting,
                    stack,
                    best_score,
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
    // The convenience path is unbudgeted: it builds the graph from a `Project`
    // and is used by small-model callers that never hit the GH #647 slowness.
    Ok(discover_loops_with_graph(results, &causal_graph, &stocks, &[], &[], None)?.loops)
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
/// Mirrors the engine's `generate_max_abs_chain_str` selection
/// (`if ABS(a) >= ABS(b) then a else b`): because `NaN` comparisons are
/// false, a `NaN` candidate loses to a finite one at that step. A present
/// series always beats an absent one (we cannot compare against nothing).
fn max_abs_score_series(a: Option<Vec<f64>>, b: Option<Vec<f64>>) -> Option<Vec<f64>> {
    match (a, b) {
        (Some(a), Some(b)) => {
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
/// result set is bit-identical to the `SearchGraph` path: same node universe,
/// same per-stock `best_score` reset, same strict-less-than pruning, same
/// stable score-descending edge order, same NaN->0 handling, same canonical
/// rotation dedup.
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

/// A timestep-resolved outbound edge: target node id and the |score| weight the
/// DFS multiplies into the running path product. Built per timestep, already
/// sorted by `score` descending (stable), so the DFS never sorts per visit.
#[derive(Clone, Copy)]
struct StepEdge {
    to: u32,
    score: f64,
}

/// Per-timestep mutable DFS state, allocated once per `discover_loops_with_graph`
/// call and reused across stocks and timesteps to avoid per-search reallocation.
struct DfsScratch {
    /// node id -> highest path score seen at that node for the current target
    /// stock (reset to 0.0 between stocks). Mirrors the original `best_score`
    /// map, whose absent keys also defaulted to 0.0.
    best_score: Vec<f64>,
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
    step_adj: Vec<Vec<StepEdge>>,
}

impl DfsScratch {
    /// Allocate reusable DFS state sized for `search`'s node universe, with each
    /// node's per-timestep edge buffer pre-reserved to its static out-degree.
    fn new(search: &IndexedSearch) -> Self {
        let n_nodes = search.node_count();
        DfsScratch {
            best_score: vec![0.0; n_nodes],
            visiting_gen: vec![0; n_nodes],
            cur_gen: 0,
            stack: Vec::with_capacity(n_nodes),
            step_adj: search
                .adj
                .iter()
                .map(|e| Vec::with_capacity(e.len()))
                .collect(),
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
    fn load_step_scores(&self, results: &Results, step: usize, scratch: &mut DfsScratch) {
        let base = step * results.step_size;
        for (node, edges) in self.adj.iter().enumerate() {
            let row = &mut scratch.step_adj[node];
            row.clear();
            for edge in edges {
                let value = results.data[base + edge.offset];
                let score = if value.is_nan() { 0.0 } else { value.abs() };
                row.push(StepEdge { to: edge.to, score });
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
        for stock_idx in 0..self.stock_ids.len() {
            let stock = self.stock_ids[stock_idx];

            // Reset best_score for this target stock. A fresh generation marks
            // the visiting set empty without touching every slot.
            scratch.best_score.iter_mut().for_each(|b| *b = 0.0);
            scratch.cur_gen = scratch.cur_gen.wrapping_add(1);
            if scratch.cur_gen == 0 {
                // Generation wrapped; clear so a stale stamp can't read as live.
                scratch.visiting_gen.iter_mut().for_each(|g| *g = 0);
                scratch.cur_gen = 1;
            }
            scratch.stack.clear();

            self.dfs(stock, 1.0, stock, scratch, seen_sets, all_paths);
        }
    }

    /// Recursive DFS mirroring `SearchGraph::check_outbound_uses`, but over
    /// integer node ids and the pre-sorted per-timestep edge lists. The edge
    /// order is established once per timestep in `load_step_scores`, so the DFS
    /// just walks `scratch.step_adj[node]` -- no per-visit sorting.
    fn dfs(
        &self,
        node: u32,
        score: f64,
        target: u32,
        scratch: &mut DfsScratch,
        seen_sets: &mut HashSet<Vec<u32>>,
        all_paths: &mut Vec<Vec<Ident<Canonical>>>,
    ) {
        let idx = node as usize;

        if scratch.visiting_gen[idx] == scratch.cur_gen {
            if node == target {
                self.record_loop(&scratch.stack, seen_sets, all_paths);
            }
            return;
        }

        // Strict less-than pruning, identical to the original.
        if score < scratch.best_score[idx] {
            return;
        }
        scratch.best_score[idx] = score;

        scratch.visiting_gen[idx] = scratch.cur_gen;
        scratch.stack.push(node);

        // Walk the node's pre-sorted (|score| desc) edge list. `step_adj` is not
        // mutated during the DFS, so we index it by position and copy each
        // `StepEdge` (it is `Copy`) -- this keeps the recursive `&mut scratch`
        // borrow legal without cloning the whole row.
        let n_edges = scratch.step_adj[idx].len();
        for k in 0..n_edges {
            let edge = scratch.step_adj[idx][k];
            self.dfs(
                edge.to,
                score * edge.score,
                target,
                scratch,
                seen_sets,
                all_paths,
            );
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
/// `budget` optionally bounds the wall-clock time spent in the per-timestep DFS
/// sweep. The elapsed time is checked once per timestep (cheap relative to the
/// DFS over the whole graph), and the sweep stops early once the budget is
/// exceeded. The returned `DiscoveryResult::truncated` records whether that
/// happened. A `None` budget runs to completion. Discovery on very large models
/// can be infeasibly slow (GH #647); the budget lets callers bound it.
pub fn discover_loops_with_graph(
    results: &Results,
    causal_graph: &CausalGraph,
    stocks: &[Ident<Canonical>],
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
    budget: Option<Duration>,
) -> Result<DiscoveryResult> {
    let link_offsets = parse_link_offsets(results, ltm_vars, dims);
    if link_offsets.is_empty() {
        return Ok(DiscoveryResult {
            loops: Vec::new(),
            truncated: false,
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
            truncated: false,
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
    // The optional time budget is checked once per timestep, *before* running
    // that step's DFS. The check is O(1) and dwarfed by the per-step DFS over
    // the whole graph, so it adds no meaningful overhead. We stop before a step
    // we can't afford rather than mid-DFS so each processed step's loop set is
    // internally consistent. `start` is captured lazily so an unbudgeted run
    // never reads the clock.
    let mut truncated = false;
    let start = budget.map(|_| Instant::now());
    for step in 1..step_count {
        if let (Some(limit), Some(started)) = (budget, start)
            && started.elapsed() >= limit
        {
            truncated = true;
            break;
        }
        search.load_step_scores(results, step, &mut scratch);
        search.discover_step(&mut scratch, &mut seen_sets, &mut all_paths);
    }

    if all_paths.is_empty() {
        return Ok(DiscoveryResult {
            loops: Vec::new(),
            truncated,
        });
    }

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

            for &offset in &link_result_offsets {
                let value = results.data[step * results.step_size + offset];
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

        // Determine runtime polarity from scores. The confidence ratio
        // returned alongside the polarity is discarded here because
        // `FoundLoop` does not carry one; downstream consumers that need
        // it (such as `DetectedLoop`) call `from_runtime_scores`
        // directly. Falling back to the structural polarity for empty
        // valid sets keeps behaviour identical to the pre-confidence
        // implementation.
        let runtime_scores: Vec<f64> = scores.iter().map(|(_, s)| *s).collect();
        let polarity = LoopPolarity::from_runtime_scores(&runtime_scores)
            .map(|(p, _confidence)| p)
            .unwrap_or(polarity_structural);

        let loop_info = Loop {
            id: String::new(), // Will be assigned below
            links: reported_links,
            stocks: loop_stocks,
            polarity,
            dimensions: vec![],
        };

        found_loops.push(FoundLoop {
            loop_info,
            scores,
            avg_abs_score,
        });
    }

    let partitions = causal_graph.compute_cycle_partitions();
    rank_and_filter(&mut found_loops, &partitions);

    Ok(DiscoveryResult {
        loops: found_loops,
        truncated,
    })
}

/// Rank, truncate, filter, and assign IDs to discovered loops.
///
/// 1. Sort by average |score| descending
/// 2. Truncate to MAX_LOOPS (200)
///    NOTE: This truncation happens before partition-aware filtering. A loop
///    dominant in a small partition but globally ranked below 200th could be
///    truncated. In practice MAX_LOOPS is generous enough that this is
///    extremely unlikely.
/// 3. Filter loops contributing less than MIN_CONTRIBUTION (0.1%) of
///    partition-scoped total score at any timestep
/// 4. Assign deterministic polarity-based IDs (r1, b1, etc.)
/// 5. Re-sort by score descending for callers
///
/// The `partitions` argument can be either variable-level or element-level.
/// When the discovery pipeline operates on an element-level graph, the
/// partitions are element-level (e.g., `population[nyc]` is a distinct
/// stock node), and loop stocks are element-specific. The threshold
/// filtering logic is partition-agnostic -- it compares each loop's
/// score to the total within its partition regardless of naming granularity.
fn rank_and_filter(found_loops: &mut Vec<FoundLoop>, partitions: &CyclePartitions) {
    // Sort by average |score| descending
    found_loops.sort_by(|a, b| {
        b.avg_abs_score
            .partial_cmp(&a.avg_abs_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Truncate to MAX_LOOPS
    found_loops.truncate(MAX_LOOPS);

    // Filter by peak per-timestep relative contribution within partition:
    // retain a loop if at any single timestep its |score| is >= MIN_CONTRIBUTION
    // of the partition-scoped total |score| at that timestep.
    let step_count = found_loops.first().map_or(0, |l| l.scores.len());
    debug_assert!(
        found_loops.iter().all(|l| l.scores.len() == step_count),
        "all loops must have the same number of timesteps"
    );
    if step_count > 0 {
        // Discovered `FoundLoop`s are always scalar (`loop_info.dimensions`
        // is `vec![]`), so `partition_for_loop` returns a length-1 vector;
        // collapse it to slot 0.  The empty `dims` slice is fine -- it's
        // only consulted for A2A loops, which discovery never produces.
        let slot0 = |fl: &FoundLoop| -> Option<usize> {
            partitions
                .partition_for_loop(&fl.loop_info, &[])
                .first()
                .copied()
                .flatten()
        };

        // Group loops by partition
        let mut partition_groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
        for (i, fl) in found_loops.iter().enumerate() {
            let partition = slot0(fl);
            partition_groups.entry(partition).or_default().push(i);
        }

        // Compute per-partition, per-timestep totals
        let mut partition_totals: HashMap<Option<usize>, Vec<f64>> = HashMap::new();
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

        // Assign partition key to each loop before retain (since retain borrows mutably)
        let loop_partitions: Vec<Option<usize>> = found_loops.iter().map(slot0).collect();

        let mut keep = vec![false; found_loops.len()];
        for (idx, fl) in found_loops.iter().enumerate() {
            let partition = loop_partitions[idx];
            let totals = &partition_totals[&partition];
            keep[idx] = fl.scores.iter().enumerate().any(|(i, &(_, score))| {
                !score.is_nan() && totals[i] > 0.0 && score.abs() / totals[i] >= MIN_CONTRIBUTION
            });
        }

        let mut keep_iter = keep.iter();
        found_loops.retain(|_| *keep_iter.next().unwrap());
    }

    // Assign deterministic IDs (sorts by content key for stable naming)
    assign_loop_ids(found_loops);

    // Re-sort by score descending so callers get results ranked by importance
    found_loops.sort_by(|a, b| {
        b.avg_abs_score
            .partial_cmp(&a.avg_abs_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Assign deterministic IDs to discovered loops based on polarity and content.
fn assign_loop_ids(loops: &mut [FoundLoop]) {
    // Sort by a deterministic key for stable ID assignment
    loops.sort_by(|a, b| {
        let key_a = loop_sort_key(&a.loop_info);
        let key_b = loop_sort_key(&b.loop_info);
        key_a.cmp(&key_b)
    });

    let mut r_counter = 1;
    let mut b_counter = 1;
    let mut u_counter = 1;

    for found in loops.iter_mut() {
        // ID prefix follows the dominant polarity so MostlyReinforcing /
        // MostlyBalancing share counters with their pure counterparts; this
        // mirrors `crate::ltm::assign_loop_ids` for the structural side.
        found.loop_info.id = match found.loop_info.polarity {
            LoopPolarity::Reinforcing | LoopPolarity::MostlyReinforcing => {
                let id = format!("r{r_counter}");
                r_counter += 1;
                id
            }
            LoopPolarity::Balancing | LoopPolarity::MostlyBalancing => {
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

fn loop_sort_key(loop_info: &Loop) -> String {
    let mut vars: Vec<String> = loop_info
        .links
        .iter()
        .flat_map(|link| vec![link.from.as_str().to_string(), link.to.as_str().to_string()])
        .collect();
    vars.sort();
    vars.dedup();
    vars.join("_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::canonicalize;

    /// Helper to build edges from tuples
    fn edges(tuples: &[(&str, &str, f64)]) -> Vec<(Ident<Canonical>, Ident<Canonical>, f64)> {
        tuples
            .iter()
            .map(|(from, to, score)| (Ident::new(from), Ident::new(to), *score))
            .collect()
    }

    /// Helper to build stock list from names
    fn stock_list(names: &[&str]) -> Vec<Ident<Canonical>> {
        names.iter().map(|n| Ident::new(n)).collect()
    }

    /// Helper to extract sorted node set from a path for comparison
    fn sorted_node_set(path: &[Ident<Canonical>]) -> Vec<String> {
        let mut set: Vec<String> = path.iter().map(|id| id.as_str().to_string()).collect();
        set.sort();
        set
    }

    // --- collapse_synthetic_links ---

    fn clink(
        from: &str,
        to: &str,
        polarity: LinkPolarity,
        score: Option<Vec<f64>>,
    ) -> CollapsibleLink {
        CollapsibleLink {
            from: Ident::new(from),
            to: Ident::new(to),
            polarity,
            score,
        }
    }

    /// Look up a collapsed edge by (from, to) in the result.
    fn find_edge<'a>(
        links: &'a [CollapsibleLink],
        from: &str,
        to: &str,
    ) -> Option<&'a CollapsibleLink> {
        links
            .iter()
            .find(|l| l.from.as_str() == from && l.to.as_str() == to)
    }

    #[test]
    fn collapse_passes_through_a_graph_with_no_synthetic_nodes() {
        // A purely real graph is returned unchanged (modulo nothing).
        let input = vec![
            clink("a", "b", LinkPolarity::Positive, Some(vec![1.0, 2.0])),
            clink("b", "c", LinkPolarity::Negative, Some(vec![3.0, 4.0])),
        ];
        let out = collapse_synthetic_links(input);
        assert_eq!(out.len(), 2);
        assert!(find_edge(&out, "a", "b").is_some());
        assert!(find_edge(&out, "b", "c").is_some());
    }

    #[test]
    fn collapse_single_chain_through_a_macro_node() {
        // Mirrors the SMTH1 edge structure from model_causal_edges:
        //   level -> $⁚smoothed_level⁚0⁚smth1 -> smoothed_level
        // plus a dangling synthetic arg helper feeding the module that has no
        // real predecessor. The chain collapses to one composite edge
        // `level -> smoothed_level` (product polarity, product score); the
        // arg-helper chain is dropped (no real source).
        let smth = "$\u{205A}smoothed_level\u{205A}0\u{205A}smth1";
        let arg = "$\u{205A}smoothed_level\u{205A}0\u{205A}arg1";
        let input = vec![
            clink("level", smth, LinkPolarity::Positive, Some(vec![2.0, -3.0])),
            clink(
                smth,
                "smoothed_level",
                LinkPolarity::Negative,
                Some(vec![5.0, 7.0]),
            ),
            clink(arg, smth, LinkPolarity::Positive, Some(vec![9.0, 9.0])),
        ];
        let out = collapse_synthetic_links(input);
        // No synthetic node survives.
        assert!(
            out.iter()
                .all(|l| !l.from.as_str().starts_with('$') && !l.to.as_str().starts_with('$')),
            "no synthetic node should remain: {:?}",
            out.iter()
                .map(|l| (l.from.as_str(), l.to.as_str()))
                .collect::<Vec<_>>()
        );
        // The composite `level -> smoothed_level` carries product polarity and
        // per-step product score.
        let edge = find_edge(&out, "level", "smoothed_level")
            .expect("level -> smoothed_level composite edge");
        assert_eq!(edge.polarity, LinkPolarity::Negative); // + composed with -
        assert_eq!(edge.score.as_deref(), Some(&[10.0, -21.0][..]));
        // The arg-helper chain produced no edge (it has no real source).
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn collapse_picks_max_magnitude_path_score() {
        // Two disjoint synthetic paths from a -> z. The composite link score is
        // the per-timestep larger-magnitude path score (ref 6.3); the reported
        // polarity follows the dominant path.
        let s1 = "$\u{205A}m\u{205A}0\u{205A}f"; // path 1 internal
        let s2 = "$\u{205A}m\u{205A}1\u{205A}g"; // path 2 internal
        let input = vec![
            // path 1: a -> s1 -> z, scores 1*1 and 1*1 = [1, 1], Positive
            clink("a", s1, LinkPolarity::Positive, Some(vec![1.0, 1.0])),
            clink(s1, "z", LinkPolarity::Positive, Some(vec![1.0, 1.0])),
            // path 2: a -> s2 -> z, scores 10*1 and 0.5*0.5 = [10, 0.25], Negative
            clink("a", s2, LinkPolarity::Negative, Some(vec![10.0, 0.5])),
            clink(s2, "z", LinkPolarity::Positive, Some(vec![1.0, 0.5])),
        ];
        let out = collapse_synthetic_links(input);
        let edge = find_edge(&out, "a", "z").expect("a -> z composite");
        // step 0: |10| > |1| -> path 2 (10, Negative); step 1: |1| > |0.25| ->
        // path 1 (1). Max-abs keeps the per-step winner's sign.
        assert_eq!(edge.score.as_deref(), Some(&[10.0, 1.0][..]));
        // Aggregate magnitude: path2 sum |10|+|0.25| = 10.25 > path1 sum 2.0,
        // so the dominant-path polarity is Negative.
        assert_eq!(edge.polarity, LinkPolarity::Negative);
    }

    #[test]
    fn collapse_drops_a_fully_internal_cycle() {
        // A synthetic-only cycle (s1 -> s2 -> s1) with no real entry/exit must
        // not loop forever and must produce no user-visible edge.
        let s1 = "$\u{205A}m\u{205A}0\u{205A}f";
        let s2 = "$\u{205A}m\u{205A}1\u{205A}g";
        let input = vec![
            clink(s1, s2, LinkPolarity::Positive, Some(vec![1.0])),
            clink(s2, s1, LinkPolarity::Positive, Some(vec![1.0])),
        ];
        let out = collapse_synthetic_links(input);
        assert!(out.is_empty(), "fully-internal cycle yields no edges");
    }

    #[test]
    fn collapse_structural_only_path_has_no_scores() {
        // No score series (structural-only caller): the composite still
        // collapses, polarity composes, and the score stays None.
        let smth = "$\u{205A}v\u{205A}0\u{205A}smth1";
        let input = vec![
            clink("x", smth, LinkPolarity::Negative, None),
            clink(smth, "y", LinkPolarity::Negative, None),
        ];
        let out = collapse_synthetic_links(input);
        let edge = find_edge(&out, "x", "y").expect("x -> y composite");
        assert_eq!(edge.polarity, LinkPolarity::Positive); // - composed with -
        assert!(edge.score.is_none());
    }

    #[test]
    fn collapse_folds_two_disagreeing_structural_paths_to_unknown() {
        // Two scoreless (structural-only) paths reach the same real endpoint
        // with disagreeing polarity, and the FIRST is genuinely Unknown:
        //   a --Unknown--> c                          (direct)
        //   a --+--> $synth --+--> c                  (composes to Positive)
        // The merged edge must be Unknown (two disagreeing structural paths,
        // per pick_stronger_polarity's both-None arm). Regression guard: when
        // (Unknown, None) doubled as the uninitialized map sentinel, the first
        // path was silently overwritten and the edge wrongly reported Positive.
        let smth = "$\u{205A}v\u{205A}0\u{205A}smth1";
        let input = vec![
            clink("a", "c", LinkPolarity::Unknown, None),
            clink("a", smth, LinkPolarity::Positive, None),
            clink(smth, "c", LinkPolarity::Positive, None),
        ];
        let out = collapse_synthetic_links(input);
        let edge = find_edge(&out, "a", "c").expect("a -> c composite");
        assert_eq!(edge.polarity, LinkPolarity::Unknown);
        assert!(edge.score.is_none());
    }

    // --- Test 1: SearchGraph construction ---

    #[test]
    fn test_search_graph_construction() {
        let graph = SearchGraph::from_edges(
            edges(&[
                ("a", "b", 10.0),
                ("a", "d", 100.0),
                ("b", "c", 10.0),
                ("c", "a", 10.0),
                ("d", "c", 0.1),
                ("d", "b", 100.0),
            ]),
            stock_list(&["a", "b", "c", "d"]),
        );

        // Verify adjacency list exists for all source nodes
        assert!(graph.adj.contains_key(&*canonicalize("a")));
        assert!(graph.adj.contains_key(&*canonicalize("b")));
        assert!(graph.adj.contains_key(&*canonicalize("c")));
        assert!(graph.adj.contains_key(&*canonicalize("d")));

        // Verify edges are sorted by |score| descending
        let a_edges = &graph.adj[&*canonicalize("a")];
        assert_eq!(a_edges.len(), 2);
        assert_eq!(a_edges[0].to.as_str(), "d"); // score 100
        assert_eq!(a_edges[1].to.as_str(), "b"); // score 10

        let d_edges = &graph.adj[&*canonicalize("d")];
        assert_eq!(d_edges.len(), 2);
        assert_eq!(d_edges[0].to.as_str(), "b"); // score 100
        assert_eq!(d_edges[1].to.as_str(), "c"); // score 0.1

        // Verify stocks
        assert_eq!(graph.stocks.len(), 4);
    }

    // --- Test 2: Trivial loop ---

    #[test]
    fn test_trivial_loop() {
        // Single stock with a flow forming one loop: stock -> flow -> stock
        let graph = SearchGraph::from_edges(
            edges(&[("stock", "flow", 1.0), ("flow", "stock", 1.0)]),
            stock_list(&["stock"]),
        );

        let loops = graph.find_strongest_loops();
        assert_eq!(loops.len(), 1, "Should find exactly one loop");

        let loop_nodes = sorted_node_set(&loops[0]);
        assert_eq!(loop_nodes, vec!["flow", "stock"]);
    }

    // --- Test 3: Figure 7 from the paper ---

    #[test]
    fn test_figure_7_paper() {
        // Edges from the paper's Figure 7:
        // a->b:10, a->d:100, b->c:10, c->a:10, d->c:0.1, d->b:100
        // All nodes are stocks for this test.
        let graph = SearchGraph::from_edges(
            edges(&[
                ("a", "b", 10.0),
                ("a", "d", 100.0),
                ("b", "c", 10.0),
                ("c", "a", 10.0),
                ("d", "c", 0.1),
                ("d", "b", 100.0),
            ]),
            stock_list(&["a", "b", "c", "d"]),
        );

        let loops = graph.find_strongest_loops();

        // With per-stock reset, each stock starts with fresh best_scores.
        // From stock a: finds [a,d,b,c] (the 4-node loop).
        // From stock b: finds [b,c,a] (the a->b->c->a loop, score 1000).
        // From stock c: finds [c,a,d] (the a->d->c->a loop, score 100).
        // From stock d: all loops already seen (deduped).
        //
        // The paper's Figure 7 demonstrates the heuristic's failure mode
        // within a single stock's search (a->b->c->a is missed when
        // starting from a), but per-stock reset recovers it from stock b.
        assert_eq!(
            loops.len(),
            3,
            "Figure 7: should find all 3 loops with per-stock reset, found {}",
            loops.len()
        );

        let mut loop_sets: Vec<Vec<String>> = loops.iter().map(|l| sorted_node_set(l)).collect();
        loop_sets.sort();
        assert_eq!(
            loop_sets,
            vec![
                vec!["a", "b", "c"],
                vec!["a", "b", "c", "d"],
                vec!["a", "c", "d"],
            ],
        );
    }

    // --- Test 4: best_score resets per stock ---

    #[test]
    fn test_best_score_resets_per_stock() {
        // Graph:
        //   a -> x (score 1000)
        //   x -> a (score 1000)  -- strong loop through a
        //   b -> x (score 1)     -- weak path from b
        //   x -> b (score 1)     -- weak path back
        //
        // With per-stock reset, each stock starts with fresh best_scores:
        //
        // TARGET=a: finds [a, x] (strong loop)
        // TARGET=b: best_scores reset to 0, finds [b, x] (weak loop)
        //
        // Both loops are found because stock B is not pruned by stock A's scores.
        let graph = SearchGraph::from_edges(
            edges(&[
                ("a", "x", 1000.0),
                ("x", "a", 1000.0),
                ("x", "b", 1.0),
                ("b", "x", 1.0),
            ]),
            stock_list(&["a", "b"]),
        );

        let loops = graph.find_strongest_loops();

        assert_eq!(
            loops.len(),
            2,
            "Per-stock reset should find both loops, found {}",
            loops.len()
        );

        let mut loop_sets: Vec<Vec<String>> = loops.iter().map(|l| sorted_node_set(l)).collect();
        loop_sets.sort();
        assert_eq!(loop_sets, vec![vec!["a", "x"], vec!["b", "x"]]);
    }

    // --- Test 5: Loop deduplication ---

    #[test]
    fn test_loop_deduplication() {
        // Create a graph where the same loop could be found from two different
        // starting stocks if best_score didn't prevent it. With equally-scored
        // paths, the first stock finds the loop and sets best_scores that
        // WOULD prune the second stock. But let's test with a structure where
        // deduplication matters.
        //
        // Use equal scores so best_score allows re-exploration (0 is NOT < 0).
        // Actually with score=1 starting, after traversing edges with score=1,
        // the accumulated score stays 1.0 which equals the initial best_score
        // of 0... wait, initial best_score is 0, and 1.0 is NOT < 0, so it proceeds.
        //
        // Stock a and stock b both participate in the same loop: a -> b -> a
        let graph = SearchGraph::from_edges(
            edges(&[("a", "b", 1.0), ("b", "a", 1.0)]),
            stock_list(&["a", "b"]),
        );

        let loops = graph.find_strongest_loops();

        // Even though both stocks can reach the loop, deduplication should ensure
        // it appears only once
        assert_eq!(loops.len(), 1, "Same loop should appear only once");

        let loop_nodes = sorted_node_set(&loops[0]);
        assert_eq!(loop_nodes, vec!["a", "b"]);
    }

    /// Issue #308 regression test for `add_loop_if_unique`:
    /// the discovery DFS must keep both directions of a directed
    /// 3-cycle as distinct loops when they share a node set.
    ///
    /// We exercise the helper directly because constructing a
    /// `SearchGraph` whose strongest-path DFS naturally surfaces both
    /// directions of a 3-cycle is fragile (the heuristic prunes
    /// alternate paths via `best_score`).  Calling
    /// `add_loop_if_unique` with the two paths is a precise check
    /// that the dedup key distinguishes them.
    #[test]
    fn add_loop_if_unique_keeps_distinct_directed_three_cycles() {
        let mut found_loops: Vec<Vec<Ident<Canonical>>> = Vec::new();
        let mut seen: HashSet<Vec<String>> = HashSet::new();

        let forward: Vec<Ident<Canonical>> =
            vec![Ident::new("a"), Ident::new("b"), Ident::new("c")];
        let reverse: Vec<Ident<Canonical>> =
            vec![Ident::new("a"), Ident::new("c"), Ident::new("b")];

        SearchGraph::add_loop_if_unique(&forward, &mut found_loops, &mut seen);
        SearchGraph::add_loop_if_unique(&reverse, &mut found_loops, &mut seen);

        assert_eq!(
            found_loops.len(),
            2,
            "opposite-direction 3-cycles must be retained as distinct loops"
        );
        assert_eq!(found_loops[0], forward);
        assert_eq!(found_loops[1], reverse);

        // Calling again with a rotation of one of the existing cycles
        // must still dedup (rotations of the same directed cycle
        // canonicalize to the same key).
        let forward_rotation: Vec<Ident<Canonical>> =
            vec![Ident::new("b"), Ident::new("c"), Ident::new("a")];
        SearchGraph::add_loop_if_unique(&forward_rotation, &mut found_loops, &mut seen);
        assert_eq!(
            found_loops.len(),
            2,
            "a rotation of an already-seen directed cycle must be deduped"
        );
    }

    // --- Test 6: Empty graph ---

    #[test]
    fn test_empty_graph() {
        let graph = SearchGraph::from_edges(vec![], stock_list(&[]));
        let loops = graph.find_strongest_loops();
        assert!(loops.is_empty(), "Empty graph should have no loops");
    }

    #[test]
    fn test_no_edges() {
        // Graph with stocks but no edges
        let graph = SearchGraph::from_edges(vec![], stock_list(&["a", "b"]));
        let loops = graph.find_strongest_loops();
        assert!(loops.is_empty(), "Graph with no edges should have no loops");
    }

    // --- Test 7: Zero-score edges ---

    #[test]
    fn test_zero_score_edges() {
        // A link with score 0 is still traversed (0 is NOT < 0, strict less-than),
        // but the accumulated score drops to 0 and won't improve beyond the
        // initial best_score of 0 at downstream nodes.
        let graph = SearchGraph::from_edges(
            edges(&[
                ("a", "b", 0.0), // zero-score link
                ("b", "a", 10.0),
            ]),
            stock_list(&["a"]),
        );

        let loops = graph.find_strongest_loops();

        // With score=0 on a->b, the accumulated score at b is 1.0*0=0.
        // best_score[b] starts at 0, and 0 is NOT < 0 (strict less-than),
        // so we DO proceed to explore from b.
        // From b: b->a with score 10, accumulated = 0*10 = 0.
        // a is visiting AND a=TARGET, so we FIND the loop.
        assert_eq!(
            loops.len(),
            1,
            "Zero-score edge should still allow traversal (strict less-than)"
        );
    }

    // --- Test 8: NaN handling ---

    #[test]
    fn test_nan_handling() {
        // NaN scores should be treated as 0
        let graph = SearchGraph::from_edges(
            edges(&[("a", "b", f64::NAN), ("b", "a", 10.0)]),
            stock_list(&["a"]),
        );

        let loops = graph.find_strongest_loops();

        // NaN is treated as 0, same behavior as zero-score test
        assert_eq!(
            loops.len(),
            1,
            "NaN should be treated as 0 (still traversable with strict less-than)"
        );
    }

    // --- Additional edge case tests ---

    #[test]
    fn test_self_loop_found() {
        // A self-loop (a -> a): check(a,1) sets visiting={a}, pushes a,
        // then explores edge a->a: check(a, score) finds a IS visiting
        // AND a=TARGET -> loop [a] is recorded.
        let graph = SearchGraph::from_edges(edges(&[("a", "a", 5.0)]), stock_list(&["a"]));

        let loops = graph.find_strongest_loops();
        assert_eq!(loops.len(), 1, "Self-loop should be found");
        assert_eq!(loops[0].len(), 1);
        assert_eq!(loops[0][0].as_str(), "a");
    }

    #[test]
    fn test_two_separate_loops() {
        // Two disconnected loops: a<->b and c<->d
        // With equal scores, both should be found since they're in separate
        // components and best_score from one doesn't affect the other.
        let graph = SearchGraph::from_edges(
            edges(&[
                ("a", "b", 1.0),
                ("b", "a", 1.0),
                ("c", "d", 1.0),
                ("d", "c", 1.0),
            ]),
            stock_list(&["a", "c"]),
        );

        let loops = graph.find_strongest_loops();
        assert_eq!(loops.len(), 2, "Should find two separate loops");
    }

    #[test]
    fn test_stocks_without_outbound_edges() {
        // A stock that has no outbound edges shouldn't cause errors
        let graph = SearchGraph::from_edges(
            edges(&[("a", "b", 1.0), ("b", "a", 1.0)]),
            stock_list(&["a", "c"]), // c has no edges
        );

        let loops = graph.find_strongest_loops();
        assert_eq!(loops.len(), 1, "Should find the a-b loop, c is harmless");
    }

    #[test]
    fn test_parse_link_offsets() {
        // Test the link offset parsing from variable names.
        // Use Ident::new() directly to match how the VM stores keys.
        let mut offsets = HashMap::new();
        offsets.insert(Ident::new("$⁚ltm⁚link_score⁚population→births"), 0usize);
        offsets.insert(Ident::new("$⁚ltm⁚link_score⁚births→population"), 1usize);
        offsets.insert(Ident::new("population"), 2usize);

        let results = Results {
            offsets,
            data: vec![0.0; 9].into_boxed_slice(),
            step_size: 3,
            step_count: 3,
            specs: crate::results::Specs {
                start: 0.0,
                stop: 2.0,
                dt: 1.0,
                save_step: 1.0,
                method: crate::results::Method::Euler,
                n_chunks: 3,
            },
            is_vensim: false,
        };

        let parsed = parse_link_offsets(&results, &[], &[]);
        assert_eq!(parsed.len(), 2, "Should find 2 link score variables");

        // Verify the parsed entries
        let has_pop_to_births = parsed
            .iter()
            .any(|((f, t), _)| f.as_str() == "population" && t.as_str() == "births");
        let has_births_to_pop = parsed
            .iter()
            .any(|((f, t), _)| f.as_str() == "births" && t.as_str() == "population");

        assert!(has_pop_to_births, "Should parse population->births link");
        assert!(has_births_to_pop, "Should parse births->population link");
    }

    #[test]
    fn test_parse_link_offsets_a2a_expansion() {
        // An A2A link score `birth_rate->births` with dimension Region
        // (NYC, Boston, Chicago) should expand to 3 element-level entries.
        let mut offsets = HashMap::new();
        offsets.insert(Ident::new("$⁚ltm⁚link_score⁚birth_rate→births"), 10usize);
        // A scalar link score for comparison
        offsets.insert(Ident::new("$⁚ltm⁚link_score⁚scalar_a→scalar_b"), 20usize);

        let results = Results {
            offsets,
            data: vec![0.0; 30].into_boxed_slice(),
            step_size: 30,
            step_count: 1,
            specs: crate::results::Specs {
                start: 0.0,
                stop: 0.0,
                dt: 1.0,
                save_step: 1.0,
                method: crate::results::Method::Euler,
                n_chunks: 1,
            },
            is_vensim: false,
        };

        let ltm_vars = vec![
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}link_score\u{205A}birth_rate\u{2192}births".to_string(),
                equation: datamodel::Equation::Scalar(String::new()),
                dimensions: vec!["Region".to_string()],
                compile_directly: false,
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}link_score\u{205A}scalar_a\u{2192}scalar_b".to_string(),
                equation: datamodel::Equation::Scalar(String::new()),
                dimensions: vec![],
                compile_directly: false,
            },
        ];
        let dims = vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec![
                "NYC".to_string(),
                "Boston".to_string(),
                "Chicago".to_string(),
            ],
        )];

        let parsed = parse_link_offsets(&results, &ltm_vars, &dims);

        // Should have 3 element-level entries for A2A + 1 scalar = 4 total
        assert_eq!(parsed.len(), 4, "3 A2A elements + 1 scalar = 4 total");

        // Check A2A expansion: birth_rate[nyc]->births[nyc] at offset 10
        let nyc = parsed
            .iter()
            .find(|((f, t), _)| f.as_str() == "birth_rate[nyc]" && t.as_str() == "births[nyc]");
        assert!(nyc.is_some(), "Should have birth_rate[nyc]->births[nyc]");
        assert_eq!(nyc.unwrap().1, 10);

        let boston = parsed.iter().find(|((f, t), _)| {
            f.as_str() == "birth_rate[boston]" && t.as_str() == "births[boston]"
        });
        assert!(
            boston.is_some(),
            "Should have birth_rate[boston]->births[boston]"
        );
        assert_eq!(boston.unwrap().1, 11);

        let chicago = parsed.iter().find(|((f, t), _)| {
            f.as_str() == "birth_rate[chicago]" && t.as_str() == "births[chicago]"
        });
        assert!(
            chicago.is_some(),
            "Should have birth_rate[chicago]->births[chicago]"
        );
        assert_eq!(chicago.unwrap().1, 12);

        // Check scalar is unchanged
        let scalar = parsed
            .iter()
            .find(|((f, t), _)| f.as_str() == "scalar_a" && t.as_str() == "scalar_b");
        assert!(scalar.is_some(), "Scalar link should be preserved");
        assert_eq!(scalar.unwrap().1, 20);
    }

    #[test]
    fn test_parse_link_offsets_cross_dim_passthrough() {
        // Cross-dimensional per-element scores (with `[` in the name)
        // should pass through directly without expansion.
        let mut offsets = HashMap::new();
        offsets.insert(
            Ident::new("$⁚ltm⁚link_score⁚population[nyc]→total_pop"),
            5usize,
        );

        let results = Results {
            offsets,
            data: vec![0.0; 10].into_boxed_slice(),
            step_size: 10,
            step_count: 1,
            specs: crate::results::Specs {
                start: 0.0,
                stop: 0.0,
                dt: 1.0,
                save_step: 1.0,
                method: crate::results::Method::Euler,
                n_chunks: 1,
            },
            is_vensim: false,
        };

        // Even with ltm_vars and dims, cross-dim scores pass through directly
        let parsed = parse_link_offsets(&results, &[], &[]);
        assert_eq!(parsed.len(), 1);
        let ((from, to), offset) = &parsed[0];
        assert_eq!(from.as_str(), "population[nyc]");
        assert_eq!(to.as_str(), "total_pop");
        assert_eq!(*offset, 5);
    }

    /// Helper: build a single-step Results object with the given offsets.
    /// Tests in this module only care about the variable->offset mapping
    /// (parse_link_offsets does not read data values), so the data buffer
    /// is sized generously and zeroed.
    fn make_results_with_offsets(
        offsets: HashMap<Ident<Canonical>, usize>,
        step_size: usize,
    ) -> Results {
        Results {
            offsets,
            data: vec![0.0; step_size].into_boxed_slice(),
            step_size,
            step_count: 1,
            specs: crate::results::Specs {
                start: 0.0,
                stop: 0.0,
                dt: 1.0,
                save_step: 1.0,
                method: crate::results::Method::Euler,
                n_chunks: 1,
            },
            is_vensim: false,
        }
    }

    /// Test 4: A FixedIndex A2A link score (`pop[nyc]→rel_pop` with
    /// non-empty dimensions). The `from_str` already carries the source
    /// element subscript; the per-slot expansion runs over the *target*
    /// dimension. Each slot represents the link score for `(pop[nyc],
    /// rel_pop[d])` at element `d`.
    #[test]
    fn test_parse_link_offsets_fixed_index_from_a2a_expansion() {
        let mut offsets = HashMap::new();
        offsets.insert(
            Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop"),
            100usize,
        );

        let results = make_results_with_offsets(offsets, 110);

        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop".to_string(),
            equation: datamodel::Equation::Scalar(String::new()),
            dimensions: vec!["Region".to_string()],
            compile_directly: false,
        }];
        let dims = vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec![
                "NYC".to_string(),
                "Boston".to_string(),
                "Chicago".to_string(),
            ],
        )];

        let parsed = parse_link_offsets(&results, &ltm_vars, &dims);

        assert_eq!(
            parsed.len(),
            3,
            "FixedIndex A2A should expand into one entry per target element"
        );

        // The from-name is fixed as `pop[nyc]` for all entries; only the
        // to-name varies per element, with the offset incrementing by 1.
        let nyc = parsed
            .iter()
            .find(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "rel_pop[nyc]");
        assert!(
            nyc.is_some(),
            "Should have pop[nyc]->rel_pop[nyc] at base offset"
        );
        assert_eq!(nyc.unwrap().1, 100);

        let boston = parsed
            .iter()
            .find(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "rel_pop[boston]");
        assert!(
            boston.is_some(),
            "Should have pop[nyc]->rel_pop[boston] at base+1"
        );
        assert_eq!(boston.unwrap().1, 101);

        let chicago = parsed
            .iter()
            .find(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "rel_pop[chicago]");
        assert!(
            chicago.is_some(),
            "Should have pop[nyc]->rel_pop[chicago] at base+2"
        );
        assert_eq!(chicago.unwrap().1, 102);
    }

    /// Test 5: A FixedIndex scalar link score (`pop[nyc]→total` with empty
    /// dimensions) is element-level on the source side and scalar on the
    /// target side. It should yield a single LinkOffset with no expansion.
    #[test]
    fn test_parse_link_offsets_fixed_index_from_scalar() {
        let mut offsets = HashMap::new();
        offsets.insert(
            Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total"),
            42usize,
        );

        let results = make_results_with_offsets(offsets, 50);

        let ltm_vars = vec![crate::db::LtmSyntheticVar {
            name: "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total".to_string(),
            equation: datamodel::Equation::Scalar(String::new()),
            dimensions: vec![],
            compile_directly: false,
        }];

        let parsed = parse_link_offsets(&results, &ltm_vars, &[]);

        assert_eq!(
            parsed.len(),
            1,
            "FixedIndex scalar should produce a single LinkOffset"
        );
        let ((from, to), offset) = &parsed[0];
        assert_eq!(from.as_str(), "pop[nyc]");
        assert_eq!(to.as_str(), "total");
        assert_eq!(*offset, 42);
    }

    /// AC3.3: A scalar-source -> arrayed-target link score named
    /// `$⁚ltm⁚link_score⁚total_pop→migration[nyc]` (one scalar
    /// `LtmSyntheticVar` per target element, `dimensions: vec![]`) resolves
    /// to the edge `(total_pop, migration[nyc])` -- the scalar source stays
    /// unsubscripted and the element survives on the `to` side.
    ///
    /// This is the discovery-side contract that `try_scalar_to_arrayed_link_scores`
    /// relies on: the `[`-in-`to` single-passthrough branch (Branch 2 of
    /// `parse_link_offsets`'s four-way dispatch) handles the new name shape
    /// with no parser change, exactly as the source-subscripted mirror
    /// (`test_parse_link_offsets_fixed_index_from_scalar`) does. Pre-fix,
    /// these edges were named as Bare-A2A vars with `dimensions = [target_dims]`,
    /// which `expand_a2a_link_offsets` mis-expanded by inventing a
    /// `total_pop[nyc]` node that doesn't match the unsubscripted `total_pop`
    /// node from the reducer edges -- making the loop unreachable.
    #[test]
    fn test_parse_link_offsets_scalar_to_arrayed() {
        let mut offsets = HashMap::new();
        offsets.insert(
            Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}total_pop\u{2192}migration[nyc]"),
            0usize,
        );

        let results = make_results_with_offsets(offsets, 10);

        // No `ltm_vars` entry needed: with empty `var_dims`, the `[`-in-`to`
        // passthrough branch fires regardless of the lookup result.
        let parsed = parse_link_offsets(&results, &[], &[]);

        assert_eq!(
            parsed.len(),
            1,
            "scalar-to-arrayed per-target-element link score should produce a single LinkOffset"
        );
        let ((from, to), offset) = &parsed[0];
        assert_eq!(
            from.as_str(),
            "total_pop",
            "the scalar source must stay unsubscripted"
        );
        assert_eq!(
            to.as_str(),
            "migration[nyc]",
            "the target element must survive on the `to` side"
        );
        assert_eq!(*offset, 0);
    }

    /// ltm-503-cross-element-agg.AC4.6 (discovery side): a partial-reduce
    /// link score `$⁚ltm⁚link_score⁚matrix[a,x]→agg[a]` -- element-level on
    /// *both* sides, `dimensions: vec![]` -- resolves to the single edge
    /// `(matrix[a,x], agg[a])`. It rides the same `[`-in-`from`-or-`to`
    /// single-passthrough branch (Branch 2) the full-reduce per-source-element
    /// names already use; no parser change is needed. Crucially it must NOT
    /// be broadcast over `D1` (which the alternative `dimensions = ["D1"]`
    /// shape would route through `expand_fixed_from_a2a_link_offsets`).
    #[test]
    fn test_parse_link_offsets_partial_reduce_passthrough() {
        let mut offsets = HashMap::new();
        offsets.insert(
            Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}matrix[a,x]\u{2192}agg[a]"),
            0usize,
        );

        let results = make_results_with_offsets(offsets, 10);

        // No `ltm_vars` entry needed: with empty `var_dims`, the
        // element-level passthrough branch fires regardless of the lookup.
        let parsed = parse_link_offsets(&results, &[], &[]);

        assert_eq!(
            parsed.len(),
            1,
            "partial-reduce per-(d1,d2) link score should produce a single LinkOffset"
        );
        let ((from, to), offset) = &parsed[0];
        assert_eq!(
            from.as_str(),
            "matrix[a,x]",
            "the source subscript carries both the surviving and reduced axes"
        );
        assert_eq!(
            to.as_str(),
            "agg[a]",
            "the target subscript carries only the surviving axis"
        );
        assert_eq!(*offset, 0);
    }

    /// Regression test: when both a Bare A2A link score (`pop→share`)
    /// and a FixedIndex A2A link score (`pop[nyc]→share`) exist for
    /// the same edge -- e.g., `share[Region] = pop + pop[NYC]` -- both
    /// expand to the per-element key `(pop[nyc], share[nyc])` at
    /// different offsets. FixedIndex names carry the `FixedIndex` rank
    /// (a bracketed `from`), so this collision is broken deterministically
    /// in Bare's favor rather than left tied and resolved by HashMap
    /// insertion order over `results.offsets`.
    #[test]
    fn test_parse_link_offsets_dedupes_a2a_bare_over_fixed_index() {
        let mut offsets = HashMap::new();
        offsets.insert(
            Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share"),
            10usize,
        );
        offsets.insert(
            Ident::new("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share"),
            20usize,
        );

        let results = make_results_with_offsets(offsets, 30);

        let ltm_vars = vec![
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share".to_string(),
                equation: datamodel::Equation::Scalar(String::new()),
                dimensions: vec!["Region".to_string()],
                compile_directly: false,
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string(),
                equation: datamodel::Equation::Scalar(String::new()),
                dimensions: vec!["Region".to_string()],
                compile_directly: false,
            },
        ];
        let dims = vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string()],
        )];

        let parsed = parse_link_offsets(&results, &ltm_vars, &dims);

        // The aliased per-element key (pop[nyc], share[nyc]) appears
        // in both Bare A2A and FixedIndex A2A expansions; dedup must
        // pick Bare deterministically.
        let nyc_aliased: Vec<&LinkOffset> = parsed
            .iter()
            .filter(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "share[nyc]")
            .collect();
        assert_eq!(
            nyc_aliased.len(),
            1,
            "aliased per-element key (pop[nyc], share[nyc]) must dedupe to one entry; \
             got {} entries: {parsed:?}",
            nyc_aliased.len(),
        );
        assert_eq!(
            nyc_aliased[0].1, 10,
            "must pick Bare A2A's offset (10) over FixedIndex A2A's (20)",
        );

        // The non-aliased FixedIndex entry (pop[nyc], share[boston])
        // -- which Bare A2A doesn't produce -- must survive at
        // FixedIndex's offset.
        let boston_only_fixed: Vec<&LinkOffset> = parsed
            .iter()
            .filter(|((f, t), _)| f.as_str() == "pop[nyc]" && t.as_str() == "share[boston]")
            .collect();
        assert_eq!(
            boston_only_fixed.len(),
            1,
            "non-aliased FixedIndex entry (pop[nyc], share[boston]) must survive",
        );
        assert_eq!(
            boston_only_fixed[0].1, 21,
            "non-aliased FixedIndex entry must keep its offset (FixedIndex base 20 + boston index 1)",
        );
    }

    #[test]
    fn test_assign_loop_ids() {
        let mut loops = vec![
            FoundLoop {
                loop_info: Loop {
                    id: String::new(),
                    links: vec![
                        Link {
                            from: Ident::new("x"),
                            to: Ident::new("y"),
                            polarity: crate::ltm::LinkPolarity::Positive,
                        },
                        Link {
                            from: Ident::new("y"),
                            to: Ident::new("x"),
                            polarity: crate::ltm::LinkPolarity::Positive,
                        },
                    ],
                    stocks: vec![],
                    polarity: LoopPolarity::Reinforcing,
                    dimensions: vec![],
                },
                scores: vec![],
                avg_abs_score: 1.0,
            },
            FoundLoop {
                loop_info: Loop {
                    id: String::new(),
                    links: vec![
                        Link {
                            from: Ident::new("a"),
                            to: Ident::new("b"),
                            polarity: crate::ltm::LinkPolarity::Negative,
                        },
                        Link {
                            from: Ident::new("b"),
                            to: Ident::new("a"),
                            polarity: crate::ltm::LinkPolarity::Positive,
                        },
                    ],
                    stocks: vec![],
                    polarity: LoopPolarity::Balancing,
                    dimensions: vec![],
                },
                scores: vec![],
                avg_abs_score: 0.5,
            },
        ];

        assign_loop_ids(&mut loops);

        // After sorting by content key, a_b comes before x_y
        let a_b_loop = loops
            .iter()
            .find(|l| {
                l.loop_info
                    .links
                    .iter()
                    .any(|link| link.from.as_str() == "a")
            })
            .unwrap();
        let x_y_loop = loops
            .iter()
            .find(|l| {
                l.loop_info
                    .links
                    .iter()
                    .any(|link| link.from.as_str() == "x")
            })
            .unwrap();

        assert_eq!(a_b_loop.loop_info.id, "b1");
        assert_eq!(x_y_loop.loop_info.id, "r1");
    }

    /// Helper to create a FoundLoop with given variable names, polarity, and score.
    /// Populates a single timestep of score data so per-timestep filtering works.
    fn make_found_loop(
        var_pairs: &[(&str, &str)],
        stocks: &[&str],
        polarity: LoopPolarity,
        avg_abs_score: f64,
    ) -> FoundLoop {
        make_found_loop_with_scores(
            var_pairs,
            stocks,
            polarity,
            avg_abs_score,
            vec![(0.0, avg_abs_score)],
        )
    }

    fn make_found_loop_with_scores(
        var_pairs: &[(&str, &str)],
        stocks: &[&str],
        polarity: LoopPolarity,
        avg_abs_score: f64,
        scores: Vec<(f64, f64)>,
    ) -> FoundLoop {
        let links: Vec<Link> = var_pairs
            .iter()
            .map(|(from, to)| Link {
                from: Ident::new(from),
                to: Ident::new(to),
                polarity: crate::ltm::LinkPolarity::Positive,
            })
            .collect();
        FoundLoop {
            loop_info: Loop {
                id: String::new(),
                links,
                stocks: stocks.iter().map(|s| Ident::new(s)).collect(),
                polarity,
                dimensions: vec![],
            },
            scores,
            avg_abs_score,
        }
    }

    /// Create a CyclePartitions where all given stocks are in a single partition.
    fn single_partition(stocks: &[&str]) -> CyclePartitions {
        let stock_idents: Vec<Ident<Canonical>> = stocks.iter().map(|s| Ident::new(s)).collect();
        let stock_partition: HashMap<Ident<Canonical>, usize> =
            stock_idents.iter().map(|s| (s.clone(), 0)).collect();
        CyclePartitions {
            partitions: vec![stock_idents],
            stock_partition,
        }
    }

    #[test]
    fn test_rank_and_filter_truncates_to_max_loops() {
        // Create MAX_LOOPS + 50 loops and verify truncation
        let stock_names: Vec<String> = (0..MAX_LOOPS + 50)
            .map(|i| format!("stock_{i:04}"))
            .collect();
        let mut loops: Vec<FoundLoop> = (0..MAX_LOOPS + 50)
            .map(|i| {
                let name_a = format!("var_a_{i:04}");
                let name_b = format!("var_b_{i:04}");
                make_found_loop(
                    &[(&name_a, &name_b), (&name_b, &name_a)],
                    &[&stock_names[i]],
                    LoopPolarity::Reinforcing,
                    // Give all loops equal score so none are filtered by MIN_CONTRIBUTION
                    1.0,
                )
            })
            .collect();

        // All stocks in one partition so filtering works like before
        let all_stocks: Vec<&str> = stock_names.iter().map(|s| s.as_str()).collect();
        let partitions = single_partition(&all_stocks);

        assert_eq!(loops.len(), MAX_LOOPS + 50);
        rank_and_filter(&mut loops, &partitions);
        assert_eq!(
            loops.len(),
            MAX_LOOPS,
            "Should truncate to MAX_LOOPS ({})",
            MAX_LOOPS
        );
    }

    #[test]
    fn test_rank_and_filter_removes_low_contribution() {
        // Create loops where one dominates and others have negligible contribution.
        // The dominant loop has score 1000; the tiny loop has score 0.0001.
        // Total = 1000.0001, tiny/total ~= 0.0000001 < MIN_CONTRIBUTION (0.001).
        let mut loops = vec![
            make_found_loop(
                &[("big_a", "big_b"), ("big_b", "big_a")],
                &["stock_x"],
                LoopPolarity::Reinforcing,
                1000.0,
            ),
            make_found_loop(
                &[("tiny_a", "tiny_b"), ("tiny_b", "tiny_a")],
                &["stock_x"],
                LoopPolarity::Balancing,
                0.0001,
            ),
        ];

        let partitions = single_partition(&["stock_x"]);
        rank_and_filter(&mut loops, &partitions);

        // Only the dominant loop should remain
        assert_eq!(
            loops.len(),
            1,
            "Loops below MIN_CONTRIBUTION should be filtered out"
        );
        assert_eq!(loops[0].avg_abs_score, 1000.0);
    }

    #[test]
    fn test_rank_and_filter_preserves_score_ordering() {
        let mut loops = vec![
            make_found_loop(
                &[("low_a", "low_b"), ("low_b", "low_a")],
                &["stock_x"],
                LoopPolarity::Balancing,
                1.0,
            ),
            make_found_loop(
                &[("high_a", "high_b"), ("high_b", "high_a")],
                &["stock_x"],
                LoopPolarity::Reinforcing,
                100.0,
            ),
            make_found_loop(
                &[("mid_a", "mid_b"), ("mid_b", "mid_a")],
                &["stock_x"],
                LoopPolarity::Reinforcing,
                50.0,
            ),
        ];

        let partitions = single_partition(&["stock_x"]);
        rank_and_filter(&mut loops, &partitions);

        // Should be sorted by score descending
        assert_eq!(loops.len(), 3);
        assert_eq!(loops[0].avg_abs_score, 100.0);
        assert_eq!(loops[1].avg_abs_score, 50.0);
        assert_eq!(loops[2].avg_abs_score, 1.0);

        // IDs should be assigned (deterministically by content, but present)
        assert!(!loops[0].loop_info.id.is_empty());
        assert!(!loops[1].loop_info.id.is_empty());
        assert!(!loops[2].loop_info.id.is_empty());
    }

    #[test]
    fn test_rank_and_filter_retains_briefly_dominant_loop() {
        // A loop that is dominant at 1 out of 100 timesteps (strong spike) but
        // has tiny average should be retained by per-timestep filtering.
        let n = 100;

        // Build score vectors: "spike" loop has score 100 at step 50, 0 elsewhere
        let spike_scores: Vec<(f64, f64)> = (0..n)
            .map(|i| {
                let t = i as f64;
                if i == 50 { (t, 100.0) } else { (t, 0.0) }
            })
            .collect();
        // avg_abs_score = 100/100 = 1.0
        let spike_loop = make_found_loop_with_scores(
            &[("spike_a", "spike_b"), ("spike_b", "spike_a")],
            &["stock_x"],
            LoopPolarity::Reinforcing,
            1.0,
            spike_scores,
        );

        // "steady" loop has score 50 at every step
        let steady_scores: Vec<(f64, f64)> = (0..n).map(|i| (i as f64, 50.0)).collect();
        let steady_loop = make_found_loop_with_scores(
            &[("steady_a", "steady_b"), ("steady_b", "steady_a")],
            &["stock_x"],
            LoopPolarity::Reinforcing,
            50.0,
            steady_scores,
        );

        let partitions = single_partition(&["stock_x"]);
        let mut loops = vec![spike_loop, steady_loop];
        rank_and_filter(&mut loops, &partitions);

        // Both loops should be retained: the spike loop has 100/(100+50) = 66.7%
        // contribution at step 50, well above MIN_CONTRIBUTION.
        assert_eq!(
            loops.len(),
            2,
            "Briefly dominant loop should be retained by per-timestep filtering"
        );
    }

    #[test]
    fn test_rank_and_filter_partitioned_filtering() {
        // Two partitions: partition A has a dominant loop and a tiny loop.
        // Partition B has a single loop that would be globally negligible
        // but is the ONLY loop in its partition.
        //
        // Without partition-aware filtering, loop_b would be filtered out
        // because its score is tiny relative to the global total.
        // With partition-aware filtering, it's retained because it's 100%
        // of its partition's total.
        let mut loops = vec![
            make_found_loop(
                &[("big_a", "big_b"), ("big_b", "big_a")],
                &["stock_a"],
                LoopPolarity::Reinforcing,
                1000.0,
            ),
            make_found_loop(
                &[("small_a", "small_b"), ("small_b", "small_a")],
                &["stock_a"],
                LoopPolarity::Balancing,
                100.0,
            ),
            make_found_loop(
                &[("other_a", "other_b"), ("other_b", "other_a")],
                &["stock_x"],
                LoopPolarity::Reinforcing,
                0.01,
            ),
        ];

        let partitions = CyclePartitions {
            partitions: vec![vec![Ident::new("stock_a")], vec![Ident::new("stock_x")]],
            stock_partition: vec![(Ident::new("stock_a"), 0), (Ident::new("stock_x"), 1)]
                .into_iter()
                .collect(),
        };

        rank_and_filter(&mut loops, &partitions);

        // All 3 loops should be retained: the "other" loop is 100% of
        // its own partition's total at its timestep
        assert_eq!(
            loops.len(),
            3,
            "Loop dominant in its own partition should be retained even if globally tiny"
        );
    }

    /// AC7.5: SearchGraph built from element-level LinkOffset entries reads the
    /// correct weight value from the correct result slot for each element.
    ///
    /// A2A expansion maps `birth_rate→births` (with dimension Region = [nyc,
    /// boston, chicago]) to three element-level `LinkOffset` entries:
    ///   `birth_rate[nyc]→births[nyc]`        at base_offset
    ///   `birth_rate[boston]→births[boston]`  at base_offset + 1
    ///   `birth_rate[chicago]→births[chicago]` at base_offset + 2
    ///
    /// This test verifies that `SearchGraph::from_results` reads the value
    /// stored at `base_offset + element_index` for each element-level edge,
    /// not the value at `base_offset` for all of them. If the offset mapping
    /// were wrong, each edge would carry the same weight (the value at
    /// `base_offset`), and the assertions on per-element weights would fail.
    #[test]
    fn test_search_graph_from_results_element_level_weights() {
        let base_offset = 10usize;

        // Build a Results object: step_size large enough to hold all offsets.
        // One timestep (step=0); distinct values at base_offset/+1/+2 so we
        // can confirm each element-level edge reads its own result slot.
        //   nyc=0.8, boston=0.3, chicago=0.5
        let step_size = 20;
        let step_count = 1;
        let mut data = vec![0.0f64; step_size * step_count];
        data[base_offset] = 0.8; // birth_rate[nyc]    -> births[nyc]    (element 0)
        data[base_offset + 1] = 0.3; // birth_rate[boston] -> births[boston] (element 1)
        data[base_offset + 2] = 0.5; // birth_rate[chicago]-> births[chicago](element 2)

        let results = Results {
            offsets: HashMap::new(), // from_results does not use offsets
            data: data.into_boxed_slice(),
            step_size,
            step_count,
            specs: crate::results::Specs {
                start: 0.0,
                stop: 0.0,
                dt: 1.0,
                save_step: 1.0,
                method: crate::results::Method::Euler,
                n_chunks: 1,
            },
            is_vensim: false,
        };

        // Element-level LinkOffset entries produced by expand_a2a_link_offsets
        // for an A2A link score with three dimension elements.
        let link_offsets: Vec<LinkOffset> = vec![
            (
                (Ident::new("birth_rate[nyc]"), Ident::new("births[nyc]")),
                base_offset,
            ),
            (
                (
                    Ident::new("birth_rate[boston]"),
                    Ident::new("births[boston]"),
                ),
                base_offset + 1,
            ),
            (
                (
                    Ident::new("birth_rate[chicago]"),
                    Ident::new("births[chicago]"),
                ),
                base_offset + 2,
            ),
        ];

        let stocks = vec![Ident::new("population[nyc]")];
        let graph = SearchGraph::from_results(&results, 0, &link_offsets, &stocks);

        // Each element-level edge must carry the value stored at its own slot.
        // The SearchGraph adjacency list is keyed by the canonical "from" ident.
        let nyc_key = canonicalize("birth_rate[nyc]");
        let boston_key = canonicalize("birth_rate[boston]");
        let chicago_key = canonicalize("birth_rate[chicago]");

        let nyc_edges = graph.adj.get(&*nyc_key);
        assert!(
            nyc_edges.is_some(),
            "birth_rate[nyc] should have an outbound edge"
        );
        let nyc_score = nyc_edges.unwrap()[0].score;
        assert!(
            (nyc_score - 0.8).abs() < 1e-10,
            "birth_rate[nyc]->births[nyc] should have weight 0.8 (slot base_offset), got {nyc_score}"
        );

        let boston_edges = graph.adj.get(&*boston_key);
        assert!(
            boston_edges.is_some(),
            "birth_rate[boston] should have an outbound edge"
        );
        let boston_score = boston_edges.unwrap()[0].score;
        assert!(
            (boston_score - 0.3).abs() < 1e-10,
            "birth_rate[boston]->births[boston] should have weight 0.3 (slot base+1), got {boston_score}"
        );

        let chicago_edges = graph.adj.get(&*chicago_key);
        assert!(
            chicago_edges.is_some(),
            "birth_rate[chicago] should have an outbound edge"
        );
        let chicago_score = chicago_edges.unwrap()[0].score;
        assert!(
            (chicago_score - 0.5).abs() < 1e-10,
            "birth_rate[chicago]->births[chicago] should have weight 0.5 (slot base+2), got {chicago_score}"
        );

        // If all offsets pointed to base_offset+0 (wrong), all weights would
        // be 0.8. Distinct values (0.8, 0.3, 0.5) make this bug visible.
        assert!(
            (nyc_score - boston_score).abs() > 1e-10,
            "nyc and boston weights must differ; both being {nyc_score} indicates an offset bug"
        );
        assert!(
            (boston_score - chicago_score).abs() > 1e-10,
            "boston and chicago weights must differ; both being {boston_score} indicates an offset bug"
        );
    }

    #[test]
    fn test_rank_and_filter_element_level_partitions() {
        // Element-level partitions: population[nyc] and population[boston]
        // are separate stocks in the same partition. A tiny loop through
        // population[chicago] in a separate partition should be retained
        // because it dominates its own partition.
        let mut loops = vec![
            make_found_loop(
                &[
                    ("population[nyc]", "births[nyc]"),
                    ("births[nyc]", "population[nyc]"),
                ],
                &["population[nyc]"],
                LoopPolarity::Reinforcing,
                500.0,
            ),
            make_found_loop(
                &[
                    ("population[boston]", "births[boston]"),
                    ("births[boston]", "population[boston]"),
                ],
                &["population[boston]"],
                LoopPolarity::Reinforcing,
                400.0,
            ),
            make_found_loop(
                &[
                    ("population[chicago]", "births[chicago]"),
                    ("births[chicago]", "population[chicago]"),
                ],
                &["population[chicago]"],
                LoopPolarity::Reinforcing,
                0.01,
            ),
        ];

        // Two partitions: NYC+Boston share a partition (connected by
        // some cross-element feedback), Chicago is alone.
        let partitions = CyclePartitions {
            partitions: vec![
                vec![
                    Ident::new("population[boston]"),
                    Ident::new("population[nyc]"),
                ],
                vec![Ident::new("population[chicago]")],
            ],
            stock_partition: vec![
                (Ident::new("population[nyc]"), 0),
                (Ident::new("population[boston]"), 0),
                (Ident::new("population[chicago]"), 1),
            ]
            .into_iter()
            .collect(),
        };

        rank_and_filter(&mut loops, &partitions);

        // All 3 loops should be retained: Chicago's loop is 100% of its
        // partition's total, even though globally it's tiny.
        assert_eq!(
            loops.len(),
            3,
            "Element-level loop dominant in its partition should be retained"
        );

        // Verify ordering: NYC (500) > Boston (400) > Chicago (0.01)
        assert_eq!(loops[0].avg_abs_score, 500.0);
        assert_eq!(loops[1].avg_abs_score, 400.0);
        assert!((loops[2].avg_abs_score - 0.01).abs() < 1e-10);
    }

    // --- IndexedSearch vs. SearchGraph equivalence oracle ---
    //
    // `discover_loops_with_graph` was optimized from a per-timestep
    // `SearchGraph` rebuild (Ident-keyed HashMaps, full-string hashing in the
    // DFS) to a once-built `IndexedSearch` over dense integer ids. The two
    // must discover *exactly* the same loop paths in the same first-seen order.
    // These tests lock that equivalence in by running both paths over a range
    // of synthetic graphs and comparing the resulting `all_paths` verbatim.

    /// The original cross-step discovery loop, reproduced over the retained
    /// `SearchGraph` reference implementation. Returns the deduped `all_paths`
    /// in first-seen order, exactly as the pre-optimization
    /// `discover_loops_with_graph` body did.
    fn reference_all_paths(
        results: &Results,
        link_offsets: &[LinkOffset],
        stocks: &[Ident<Canonical>],
    ) -> Vec<Vec<Ident<Canonical>>> {
        let mut all_paths: Vec<Vec<Ident<Canonical>>> = Vec::new();
        let mut seen_sets: HashSet<Vec<String>> = HashSet::new();
        for step in 1..results.step_count {
            let graph = SearchGraph::from_results(results, step, link_offsets, stocks);
            for path in graph.find_strongest_loops() {
                let path_strings: Vec<String> =
                    path.iter().map(|id| id.as_str().to_string()).collect();
                let key = crate::ltm::canonical_rotation(&path_strings);
                if seen_sets.insert(key) {
                    all_paths.push(path);
                }
            }
        }
        all_paths
    }

    /// The optimized discovery loop in isolation (the integer-indexed path
    /// inside `discover_loops_with_graph`), returning the same `all_paths`.
    fn indexed_all_paths(
        results: &Results,
        link_offsets: &[LinkOffset],
        stocks: &[Ident<Canonical>],
    ) -> Vec<Vec<Ident<Canonical>>> {
        let mut all_paths: Vec<Vec<Ident<Canonical>>> = Vec::new();
        let mut seen_sets: HashSet<Vec<u32>> = HashSet::new();
        let search = IndexedSearch::build(link_offsets, stocks);
        let mut scratch = DfsScratch::new(&search);
        for step in 1..results.step_count {
            search.load_step_scores(results, step, &mut scratch);
            search.discover_step(&mut scratch, &mut seen_sets, &mut all_paths);
        }
        all_paths
    }

    fn paths_as_strings(paths: &[Vec<Ident<Canonical>>]) -> Vec<Vec<String>> {
        paths
            .iter()
            .map(|p| p.iter().map(|id| id.as_str().to_string()).collect())
            .collect()
    }

    /// Build a multi-step `Results` whose per-edge scores follow a deterministic
    /// pseudo-random sequence, so the per-timestep edge sort order (and thus the
    /// DFS traversal/pruning) varies across steps -- exercising the tie-breaking
    /// and score-dependent branches in both implementations.
    fn synthetic_results(n_offsets: usize, step_count: usize, seed: u64) -> Results {
        let step_size = n_offsets;
        let mut data = vec![0.0f64; step_size * step_count];
        // Step 0 is all NaN (PREVIOUS values don't exist), matching production;
        // discovery skips it. Remaining steps get varied finite scores, with a
        // few deliberate zeros/NaNs to exercise those branches.
        let mut state = seed | 1;
        let mut next = || {
            // xorshift64* -- deterministic, no external deps.
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state.wrapping_mul(0x2545F4914F6CDD1D)
        };
        for slot in data.iter_mut().take(n_offsets) {
            *slot = f64::NAN;
        }
        for step in 1..step_count {
            for off in 0..n_offsets {
                let r = next();
                let v = match r % 16 {
                    0 => 0.0,
                    1 => f64::NAN,
                    _ => {
                        let mag = ((r >> 8) % 1000) as f64 / 100.0;
                        if r & 1 == 0 { mag } else { -mag }
                    }
                };
                data[step * step_size + off] = v;
            }
        }
        Results {
            offsets: HashMap::new(),
            data: data.into_boxed_slice(),
            step_size,
            step_count,
            specs: crate::results::Specs {
                start: 0.0,
                stop: (step_count - 1) as f64,
                dt: 1.0,
                save_step: 1.0,
                method: crate::results::Method::Euler,
                n_chunks: step_count,
            },
            is_vensim: false,
        }
    }

    #[test]
    fn indexed_search_matches_reference_on_synthetic_graphs() {
        // A fully-connected-ish 5-node graph plus a couple of disconnected
        // nodes, several stocks, parallel edges, and a self-loop -- the shapes
        // the unit tests above exercise individually, combined and stressed
        // over many timesteps with varying scores.
        let names = ["a", "b", "c", "d", "e", "f", "g"];
        let mut edge_pairs: Vec<(&str, &str)> = Vec::new();
        for &from in &names[..5] {
            for &to in &names[..5] {
                edge_pairs.push((from, to)); // includes self-loops
            }
        }
        // A node ("g") that is only ever an edge target (no outbound edges)
        // and a duplicate (parallel) edge to stress tie-breaking / dedup.
        edge_pairs.push(("a", "g"));
        edge_pairs.push(("a", "b")); // parallel to the existing a->b

        let link_offsets: Vec<LinkOffset> = edge_pairs
            .iter()
            .enumerate()
            .map(|(i, (from, to))| ((Ident::new(from), Ident::new(to)), i))
            .collect();

        // Stocks include a node with no incident edges ("f") to mirror the
        // `test_stocks_without_outbound_edges` shape.
        let stocks: Vec<Ident<Canonical>> =
            ["a", "c", "e", "f"].iter().map(|s| Ident::new(s)).collect();

        // Run several independent seeds so the per-step sort order (and the
        // resulting traversal/pruning) varies widely.
        for seed in [1u64, 7, 42, 1000, 999_983] {
            let results = synthetic_results(link_offsets.len(), 40, seed);
            let reference = reference_all_paths(&results, &link_offsets, &stocks);
            let indexed = indexed_all_paths(&results, &link_offsets, &stocks);
            // Guard against a vacuous pass: a future fixture edit that produced
            // no loops would make the equality below trivially true.
            assert!(
                !reference.is_empty(),
                "synthetic fixture must produce loops (seed {seed})"
            );
            assert_eq!(
                paths_as_strings(&indexed),
                paths_as_strings(&reference),
                "IndexedSearch must discover the identical loop paths in the \
                 identical first-seen order as the SearchGraph reference \
                 (seed {seed})"
            );
        }
    }

    #[test]
    fn indexed_search_matches_reference_element_level_names() {
        // Long element-level identifiers (the C-LEARN-style names whose string
        // hashing the optimization eliminates) over a denser graph.
        let names = [
            "population[nyc]",
            "births[nyc]",
            "deaths[nyc]",
            "population[boston]",
            "births[boston]",
            "migration_pressure[chicago]",
        ];
        let mut edge_pairs: Vec<(&str, &str)> = Vec::new();
        for &from in &names {
            for &to in &names {
                if from != to {
                    edge_pairs.push((from, to));
                }
            }
        }
        let link_offsets: Vec<LinkOffset> = edge_pairs
            .iter()
            .enumerate()
            .map(|(i, (from, to))| ((Ident::new(from), Ident::new(to)), i))
            .collect();
        let stocks: Vec<Ident<Canonical>> = ["population[nyc]", "population[boston]"]
            .iter()
            .map(|s| Ident::new(s))
            .collect();

        for seed in [3u64, 17, 55, 12_345] {
            let results = synthetic_results(link_offsets.len(), 30, seed);
            let reference = reference_all_paths(&results, &link_offsets, &stocks);
            let indexed = indexed_all_paths(&results, &link_offsets, &stocks);
            assert!(
                !reference.is_empty(),
                "element-level fixture must produce loops (seed {seed})"
            );
            assert_eq!(
                paths_as_strings(&indexed),
                paths_as_strings(&reference),
                "element-level discovery must match the reference (seed {seed})"
            );
        }
    }
}
