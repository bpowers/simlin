// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Stock-to-stock cycle partitions: SCC grouping over the parent-level
//! stock graph. Used by relative-loop-score normalization to bucket loops
//! into the strongly-connected component of stocks they participate in.

use std::collections::{HashMap, HashSet};

use crate::common::{Canonical, Ident};

use super::types::Loop;

/// Cycle partitions: groups of stocks connected by feedback loops.
///
/// Two stocks are in the same partition if they are mutually reachable
/// through the causal graph (i.e., they form a strongly connected component
/// in the stock-to-stock reachability graph). Relative loop scores should
/// only be compared within the same partition.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct CyclePartitions {
    /// Partitions as sorted Vecs of stock idents.
    /// Outer Vec is sorted by the lexicographically smallest stock in each partition.
    pub partitions: Vec<Vec<Ident<Canonical>>>,
    /// Stock -> partition index (into `partitions`).
    pub stock_partition: HashMap<Ident<Canonical>, usize>,
}

/// Extract the subscript content of an element-level node name.
///
/// `"pop[nyc]"` -> `Some("nyc")`, `"migration[nyc,boston]"` ->
/// `Some("nyc,boston")`, `"pop"` -> `None`.  Uses the *last* `]` so a
/// (hypothetical) nested bracket inside the subscript would still resolve
/// the outer span; in practice subscripts never nest.
fn element_subscript(name: &str) -> Option<&str> {
    let start = name.find('[')?;
    let end = name.rfind(']')?;
    if end > start {
        Some(&name[start + 1..end])
    } else {
        None
    }
}

/// Enumerate an A2A loop's per-slot element-tuple suffixes, in the same
/// row-major-over-declaration-order layout the runtime / FFI use.
///
/// `dimensions` are the loop's datamodel dimension names (in equation-
/// language order); `dims` is the project's declared dimension list.  For
/// each dimension the element list is the canonical Named element names in
/// declared order, or `["1", "2", ...]` for an Indexed dimension -- exactly
/// what `crate::ltm_augment::dimension_element_names` produces, so the
/// suffixes match the element-level stock node names that
/// `model_element_cycle_partitions` keys on.  The cartesian product is
/// row-major (first dimension slowest-varying, last dimension fastest), the
/// same layout `crate::ltm_post::LoopElementIndex::resolve` and the VM's
/// arrayed-variable slot allocation use -- so slot index `k` here means the
/// same element `k` there, and the FFI's
/// `simlin_analyze_get_relative_loop_score("r1[elem]")` (which resolves
/// `elem` to a slot index via `LoopElementIndex`) indexes consistently into
/// the resulting `loop_partitions` vector.
///
/// Returns an empty `Vec` when `dimensions` is empty or any named dimension
/// is missing from `dims` (a mid-edit inconsistency); callers fall back to
/// the slot suffixes actually present on the loop's stocks in that case.
pub(crate) fn loop_dimension_element_tuples(
    dimensions: &[String],
    dims: &[crate::datamodel::Dimension],
) -> Vec<String> {
    use crate::canonicalize;
    use crate::datamodel::DimensionElements;

    if dimensions.is_empty() {
        return Vec::new();
    }
    let mut element_lists: Vec<Vec<String>> = Vec::with_capacity(dimensions.len());
    for dim_name in dimensions {
        let canon = canonicalize(dim_name);
        let Some(dim) = dims
            .iter()
            .find(|d| canonicalize(d.name()).as_ref() == canon.as_ref())
        else {
            return Vec::new();
        };
        let elements: Vec<String> = match &dim.elements {
            DimensionElements::Named(names) => {
                names.iter().map(|n| canonicalize(n).into_owned()).collect()
            }
            DimensionElements::Indexed(size) => (1..=*size).map(|i| i.to_string()).collect(),
        };
        element_lists.push(elements);
    }
    // Row-major cartesian product: `element_lists[0]` is the outermost
    // (slowest-varying) dimension, `element_lists[last]` the innermost.  This
    // mirrors `db::ltm::cartesian_subscripts` and `LoopElementIndex::resolve`.
    let mut result: Vec<String> = element_lists[0].clone();
    for next in &element_lists[1..] {
        let mut expanded = Vec::with_capacity(result.len() * next.len());
        for existing in &result {
            for elem in next {
                expanded.push(format!("{existing},{elem}"));
            }
        }
        result = expanded;
    }
    result
}

impl CyclePartitions {
    /// Resolve a loop's cycle partition(s) -- one entry per slot.
    ///
    /// The returned vector has **one entry per conceptual slot of the loop**:
    ///
    /// - **scalar and cross-element/mixed loops** (`loop_item.dimensions`
    ///   empty): a singleton.  These loops carry element-level stock names
    ///   (`"pop[nyc]"`; plain names for a scalar model), all in one SCC; the
    ///   single entry is that partition (`None` if no stock resolves -- a loop
    ///   genuinely below the parent graph, e.g. a pure module-internal loop).
    ///   Module-internal stocks (namespaced with interpunct, e.g.
    ///   `smooth·smoothed`) are implicitly in the same partition as the parent
    ///   stocks they coexist with but don't appear in the partition map, so
    ///   they're never the resolving stock.
    ///
    /// - **A2A loops** (`loop_item.dimensions` non-empty): one entry per
    ///   element of the loop's dimension element space, in the same row-major
    ///   slot order the runtime / FFI use (see
    ///   [`loop_dimension_element_tuples`]).  Slot `k`'s entry is the partition
    ///   of that slot's element-level stocks (`"{var}[{elem-tuple-k}]"`).
    ///   *Within* a slot every parent-level stock shares a partition (the
    ///   per-element SCC is connected through that slot's diagonal hop);
    ///   *across* slots of an element-wise-uncoupled A2A dimension they may
    ///   differ -- two disconnected per-element feedback subsystems get
    ///   distinct partition indices, which is precisely the cross-element
    ///   normalization bug this granularity fixes (GH #487).  When `dims`
    ///   doesn't cover the loop's declared dimensions (a mid-edit
    ///   inconsistency), we fall back to the distinct slot suffixes actually
    ///   present on `loop_item.stocks`, sorted -- not necessarily the
    ///   runtime's row-major order, but the only deterministic choice without
    ///   the dimension element order.
    ///
    /// `dims` is the project's declared dimension list; it is unused for
    /// scalar/cross-element loops and callers that only have scalar loops
    /// (`ltm_finding::rank_and_filter`) may pass `&[]`.
    pub fn partition_for_loop(
        &self,
        loop_item: &Loop,
        dims: &[crate::datamodel::Dimension],
    ) -> Vec<Option<usize>> {
        if loop_item.dimensions.is_empty() {
            // One conceptual slot.  `loop_item.stocks` are element-level names
            // for these loops (or plain names for a scalar model); all in one
            // SCC, so any resolving stock gives the partition.
            let result = loop_item
                .stocks
                .iter()
                .find_map(|s| self.stock_partition.get(s).copied());
            debug_assert!(
                loop_item
                    .stocks
                    .iter()
                    .filter_map(|s| self.stock_partition.get(s).copied())
                    .all(|p| Some(p) == result),
                "all stocks in a scalar/cross-element loop must be in the same partition"
            );
            return vec![result];
        }

        // A2A loop: one slot per element of the dimension element space.
        let tuples = loop_dimension_element_tuples(&loop_item.dimensions, dims);
        if !tuples.is_empty() {
            return tuples
                .iter()
                .map(|suffix| self.partition_for_a2a_slot(loop_item, suffix))
                .collect();
        }
        // Fallback (mid-edit inconsistency: `dims` doesn't cover the loop's
        // declared dimensions).  Use the slot suffixes actually present on the
        // loop's stocks, sorted for determinism.
        let mut suffixes: Vec<&str> = loop_item
            .stocks
            .iter()
            .filter_map(|s| element_subscript(s.as_str()))
            .collect();
        suffixes.sort_unstable();
        suffixes.dedup();
        if suffixes.is_empty() {
            return vec![None];
        }
        suffixes
            .iter()
            .map(|suffix| self.partition_for_a2a_slot(loop_item, suffix))
            .collect()
    }

    /// Resolve one slot of an A2A loop: the partition of the element-level
    /// stocks `"{var}[{suffix}]"`.  All of the loop's stock nodes whose
    /// subscript equals `suffix` must share one partition (the per-slot
    /// consistency invariant); `None` when none of them resolves.
    fn partition_for_a2a_slot(&self, loop_item: &Loop, suffix: &str) -> Option<usize> {
        let result = loop_item
            .stocks
            .iter()
            .filter(|s| element_subscript(s.as_str()) == Some(suffix))
            .find_map(|s| self.stock_partition.get(s).copied());
        debug_assert!(
            loop_item
                .stocks
                .iter()
                .filter(|s| element_subscript(s.as_str()) == Some(suffix))
                .filter_map(|s| self.stock_partition.get(s).copied())
                .all(|p| Some(p) == result),
            "all parent-level stocks in one A2A slot must be in the same partition (slot {suffix:?})"
        );
        result
    }
}

/// Standard Tarjan's SCC algorithm on a directed graph of stock nodes.
///
/// Takes a deterministically-ordered list of stock nodes and a reachability
/// map, returns strongly connected components.
pub(super) fn tarjan_scc(
    nodes: &[Ident<Canonical>],
    reachability: &HashMap<Ident<Canonical>, HashSet<Ident<Canonical>>>,
) -> Vec<Vec<Ident<Canonical>>> {
    struct TarjanState {
        index_counter: usize,
        stack: Vec<Ident<Canonical>>,
        on_stack: HashSet<Ident<Canonical>>,
        indices: HashMap<Ident<Canonical>, usize>,
        lowlinks: HashMap<Ident<Canonical>, usize>,
        sccs: Vec<Vec<Ident<Canonical>>>,
    }

    fn strongconnect(
        v: &Ident<Canonical>,
        reachability: &HashMap<Ident<Canonical>, HashSet<Ident<Canonical>>>,
        state: &mut TarjanState,
    ) {
        state.indices.insert(v.clone(), state.index_counter);
        state.lowlinks.insert(v.clone(), state.index_counter);
        state.index_counter += 1;
        state.stack.push(v.clone());
        state.on_stack.insert(v.clone());

        if let Some(neighbors) = reachability.get(v) {
            let mut sorted_neighbors: Vec<_> = neighbors.iter().collect();
            sorted_neighbors.sort_by(|a, b| a.as_str().cmp(b.as_str()));

            for w in sorted_neighbors {
                if !state.indices.contains_key(w) {
                    strongconnect(w, reachability, state);
                    let w_lowlink = state.lowlinks[w];
                    let v_lowlink = state.lowlinks.get_mut(v).unwrap();
                    if w_lowlink < *v_lowlink {
                        *v_lowlink = w_lowlink;
                    }
                } else if state.on_stack.contains(w) {
                    let w_index = state.indices[w];
                    let v_lowlink = state.lowlinks.get_mut(v).unwrap();
                    if w_index < *v_lowlink {
                        *v_lowlink = w_index;
                    }
                }
            }
        }

        if state.lowlinks[v] == state.indices[v] {
            let mut scc = Vec::new();
            loop {
                let w = state.stack.pop().unwrap();
                state.on_stack.remove(&w);
                scc.push(w.clone());
                if &w == v {
                    break;
                }
            }
            state.sccs.push(scc);
        }
    }

    let mut state = TarjanState {
        index_counter: 0,
        stack: Vec::new(),
        on_stack: HashSet::new(),
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        sccs: Vec::new(),
    };

    for node in nodes {
        if !state.indices.contains_key(node) {
            strongconnect(node, reachability, &mut state);
        }
    }

    state.sccs
}
