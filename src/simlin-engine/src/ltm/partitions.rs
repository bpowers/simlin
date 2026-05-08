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

impl CyclePartitions {
    /// Look up the partition index for a loop using its parent-level stocks.
    /// Returns None for loops with no parent-level stocks.
    ///
    /// Module-internal stocks (namespaced with interpunct, e.g.
    /// `smooth·smoothed`) are implicitly in the same partition as the parent
    /// stocks they coexist with in a loop, but they don't appear in the
    /// partition map since partitions are computed on the parent graph.
    ///
    /// All *parent-level* stocks in a feedback loop are guaranteed to be in the
    /// same SCC: if a loop passes through stocks A and B, there are paths
    /// A->B and B->A through the loop, making them mutually reachable.
    pub fn partition_for_loop(&self, loop_item: &Loop) -> Option<usize> {
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
            "all parent-level stocks in a loop must be in the same partition"
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
