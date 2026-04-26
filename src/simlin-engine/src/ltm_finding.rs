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

use crate::common::{Canonical, Ident, Result};
use crate::datamodel;
use crate::db::LtmSyntheticVar;
#[cfg(test)]
use crate::ltm::Link;
use crate::ltm::{CausalGraph, CyclePartitions, Loop, LoopPolarity};
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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
struct ScoredEdge {
    to: Ident<Canonical>,
    /// Absolute value of link score at this timestep
    score: f64,
}

/// The search graph for one timestep: adjacency list with edges sorted by |score| desc.
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

    /// Add loop to results if it hasn't been seen before (deduplicate by node set).
    fn add_loop_if_unique(
        stack: &[Ident<Canonical>],
        found_loops: &mut Vec<Vec<Ident<Canonical>>>,
        seen_sets: &mut HashSet<Vec<String>>,
    ) {
        if stack.is_empty() {
            return;
        }

        // Create a sorted node set as the deduplication key
        let mut node_set: Vec<String> = stack.iter().map(|id| id.as_str().to_string()).collect();
        node_set.sort();

        if seen_sets.insert(node_set) {
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
/// Cross-dimensional per-element scores (names containing `[` in the
/// from-name, e.g. `$..link_score..population[nyc]->total_pop`) are
/// already element-level and map directly to one offset.
///
/// When `ltm_vars` is empty (e.g. in the non-salsa convenience path),
/// all link scores are treated as scalar (no expansion).
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

    let mut link_offsets = Vec::new();

    for (var_name, &offset) in &results.offsets {
        let name_str = var_name.as_str();
        if let Some(suffix) = name_str.strip_prefix(LINK_SCORE_PREFIX) {
            // Split on the arrow separator to get from and to
            if let Some((from_str, to_str)) = suffix.split_once(LTM_LINK_SEP) {
                // Check if this is a cross-dimensional per-element score
                // (already element-level, contains `[` in from-name).
                if from_str.contains('[') || to_str.contains('[') {
                    let from = Ident::new(from_str);
                    let to = Ident::new(to_str);
                    link_offsets.push(((from, to), offset));
                    continue;
                }

                // Look up the LtmSyntheticVar for this link score to get
                // its dimensions. If found and dimensions are non-empty,
                // this is an A2A link score that needs expansion.
                let var_dims = ltm_var_map
                    .get(name_str)
                    .map(|v| &v.dimensions[..])
                    .unwrap_or(&[]);

                if var_dims.is_empty() {
                    // Scalar link score: one entry at the base offset.
                    let from = Ident::new(from_str);
                    let to = Ident::new(to_str);
                    link_offsets.push(((from, to), offset));
                } else {
                    // A2A link score: expand to N element-level edges.
                    expand_a2a_link_offsets(
                        from_str,
                        to_str,
                        offset,
                        var_dims,
                        dims,
                        &mut link_offsets,
                    );
                }
            }
        }
    }

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
    // Resolve dimension element names. For each dimension name in
    // var_dims, look up the datamodel::Dimension to get element names.
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
        // Dimension resolution failed; fall back to a single scalar entry.
        let from = Ident::new(from_var);
        let to = Ident::new(to_var);
        link_offsets.push(((from, to), base_offset));
        return;
    }

    // Compute cartesian product of element names (row-major order).
    let mut tuples: Vec<Vec<&str>> = vec![vec![]];
    for elements in &dim_elements {
        let mut new_tuples = Vec::with_capacity(tuples.len() * elements.len());
        for existing in &tuples {
            for elem in elements {
                let mut extended = existing.clone();
                extended.push(elem.as_str());
                new_tuples.push(extended);
            }
        }
        tuples = new_tuples;
    }

    for (idx, elems) in tuples.iter().enumerate() {
        let subscript = if elems.len() == 1 {
            elems[0].to_string()
        } else {
            elems.join(",")
        };
        let from = Ident::new(&format!("{from_var}[{subscript}]"));
        let to = Ident::new(&format!("{to_var}[{subscript}]"));
        link_offsets.push(((from, to), base_offset + idx));
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
    discover_loops_with_graph(results, &causal_graph, &stocks, &[], &[])
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
pub fn discover_loops_with_graph(
    results: &Results,
    causal_graph: &CausalGraph,
    stocks: &[Ident<Canonical>],
    ltm_vars: &[LtmSyntheticVar],
    dims: &[datamodel::Dimension],
) -> Result<Vec<FoundLoop>> {
    let link_offsets = parse_link_offsets(results, ltm_vars, dims);
    if link_offsets.is_empty() {
        return Ok(Vec::new());
    }

    // Build HashMap for O(1) link offset lookups during score computation
    let link_offset_map: LinkOffsetMap = link_offsets
        .iter()
        .map(|((from, to), offset)| ((from.clone(), to.clone()), *offset))
        .collect();

    if stocks.is_empty() {
        return Ok(Vec::new());
    }

    // Collect all unique loop paths across all timesteps
    let mut all_paths: Vec<Vec<Ident<Canonical>>> = Vec::new();
    let mut seen_sets: HashSet<Vec<String>> = HashSet::new();

    let step_count = results.step_count;

    // Skip step 0 where link scores are NaN (PREVIOUS values don't exist)
    for step in 1..step_count {
        let graph = SearchGraph::from_results(results, step, &link_offsets, stocks);
        let paths = graph.find_strongest_loops();

        for path in paths {
            let mut node_set: Vec<String> = path.iter().map(|id| id.as_str().to_string()).collect();
            node_set.sort();

            if seen_sets.insert(node_set) {
                all_paths.push(path);
            }
        }
    }

    if all_paths.is_empty() {
        return Ok(Vec::new());
    }

    // Convert paths to FoundLoop objects with scores
    let mut found_loops: Vec<FoundLoop> = Vec::new();

    for path in &all_paths {
        // Convert path to links using CausalGraph
        let links = causal_graph.circuit_to_links(path);
        let loop_stocks = causal_graph.find_stocks_in_loop(path);
        let polarity_structural = causal_graph.calculate_polarity(&links);

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

        // Determine runtime polarity from scores
        let runtime_scores: Vec<f64> = scores.iter().map(|(_, s)| *s).collect();
        let polarity =
            LoopPolarity::from_runtime_scores(&runtime_scores).unwrap_or(polarity_structural);

        let loop_info = Loop {
            id: String::new(), // Will be assigned below
            links,
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

    Ok(found_loops)
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
        // Group loops by partition
        let mut partition_groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
        for (i, fl) in found_loops.iter().enumerate() {
            let partition = partitions.partition_for_loop(&fl.loop_info);
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
        let loop_partitions: Vec<Option<usize>> = found_loops
            .iter()
            .map(|fl| partitions.partition_for_loop(&fl.loop_info))
            .collect();

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
        found.loop_info.id = match found.loop_info.polarity {
            LoopPolarity::Reinforcing => {
                let id = format!("r{r_counter}");
                r_counter += 1;
                id
            }
            LoopPolarity::Balancing => {
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
                equation: String::new(),
                dimensions: vec!["Region".to_string()],
            },
            crate::db::LtmSyntheticVar {
                name: "$\u{205A}ltm\u{205A}link_score\u{205A}scalar_a\u{2192}scalar_b".to_string(),
                equation: String::new(),
                dimensions: vec![],
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
                            // Shape is populated in Phase 4 at loop construction.
                            shape: None,
                        },
                        Link {
                            from: Ident::new("y"),
                            to: Ident::new("x"),
                            polarity: crate::ltm::LinkPolarity::Positive,
                            // Shape is populated in Phase 4 at loop construction.
                            shape: None,
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
                            // Shape is populated in Phase 4 at loop construction.
                            shape: None,
                        },
                        Link {
                            from: Ident::new("b"),
                            to: Ident::new("a"),
                            polarity: crate::ltm::LinkPolarity::Positive,
                            // Shape is populated in Phase 4 at loop construction.
                            shape: None,
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
                // Shape is populated in Phase 4 at loop construction.
                shape: None,
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
}
