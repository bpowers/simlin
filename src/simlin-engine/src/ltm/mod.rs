// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loops That Matter (LTM) implementation for loop dominance analysis.
//!
//! Submodule layout:
//!
//! - [`types`]: public LTM vocabulary -- `LinkPolarity`, `Link`, `Loop`,
//!   `LoopPolarity`, `TruncatedByBudget`, `ModuleLtmRole`, plus the
//!   small `classify_module_for_ltm` / `normalize_module_ref` helpers.
//! - [`partitions`]: stock-to-stock SCC grouping. Provides
//!   `CyclePartitions` and the generic Tarjan SCC used by
//!   `compute_cycle_partitions`.
//! - [`polarity`]: static polarity analysis on `Expr2` ASTs --
//!   `analyze_link_polarity` and the small expression predicates it
//!   leans on (`flip_polarity`, `expr_references_var`,
//!   `is_positive_constant`, etc.).
//! - [`indexed`]: compact integer-indexed graph plus the Johnson 1975
//!   elementary-circuit enumerator and Tarjan SCC. The Tiernan 1970
//!   enumerator is retained under `cfg(test)` as the equivalence oracle.
//! - [`graph`]: public `CausalGraph` type that ties everything
//!   together: model adjacency, stock identity, variable AST references
//!   for polarity, and recursively-built sub-graphs for dynamic
//!   modules. Owns the elementary-circuit / cycle-partition / link-
//!   polarity surface that callers like `db::analysis` and `ltm_finding`
//!   consume.

use crate::common::Result;
use crate::model::ModelStage1;
use crate::project::Project;

mod graph;
mod indexed;
mod partitions;
mod polarity;
mod types;

// Public LTM API. These names are reachable as `crate::ltm::Foo`
// exactly as before the split so external callers (libsimlin, db_*,
// ltm_augment, ltm_finding, json_sdai, etc.) compile unchanged.
pub use graph::CausalGraph;
pub use partitions::CyclePartitions;
pub use types::{
    Link, LinkPolarity, Loop, LoopPolarity, POLARITY_CONFIDENCE_THRESHOLD, TruncatedByBudget,
};

// Crate-internal helpers used from sibling modules in `simlin-engine`.
pub(crate) use graph::assign_loop_ids;
pub(crate) use partitions::loop_dimension_element_tuples;
pub(crate) use types::{is_synthetic_node_name, normalize_module_ref};

// Shared SCC primitive over an `Ident`-keyed adjacency list. Used in
// production by the `db/dep_graph.rs` element-cycle refinement
// (`resolve_recurrence_sccs` -- single-variable self-recurrence
// resolution in the dt AND init cycle gates, which calls it over both
// the whole-variable phase adjacency and each SCC's induced element
// graph), plus the `#[cfg(test)]` dt-phase cycle accessor
// (`crate::db::dep_graph::dt_cycle_sccs`).
pub(crate) use indexed::scc_components;

/// Maximum number of nodes in any single strongly-connected component
/// before [`crate::db::model_ltm_variables`] auto-flips from exhaustive
/// to discovery mode.
///
/// As of the 2026-05-06 tiered loop enumerator (#482), the gate is
/// evaluated at two granularities and fires when **either** exceeds the
/// threshold:
///
/// - The *variable-level* SCC, computed by Tarjan on the variable graph
///   before any Johnson run. This is the early gate that catches dense
///   scalar-feedback models like WRLD3 (166-node variable SCC) without
///   paying for variable-level Johnson at all.
/// - The *slow-path subgraph* SCC, computed by Tarjan on the
///   cross-element / mixed slice inside `model_loop_circuits_tiered`.
///   Pure-A2A and pure-scalar cycles contribute nothing to this
///   subgraph, so this gate fires only on legitimate cross-element
///   pressure. The legacy "full element-graph SCC" gate is gone:
///   pre-#482 it was effectively `max(variable SCC, cross-element SCC)`
///   anyway, and pure-A2A models with huge N stop tripping it now.
///
/// The gate is applied **before** Johnson's circuit enumeration so that
/// the downstream `build_element_level_loops` /
/// `generate_loop_score_variables` pipeline is never entered on inputs
/// whose per-partition materialization would exceed WASM's 4 GiB linear
/// memory budget.  With the largest-SCC gate in place there is no
/// separate cap on total circuit count; enumeration itself is bounded
/// only by the per-call `max_circuits` argument supplied by the caller.
///
/// ## Why 50
///
/// The 2026-04-18 LTM cap-lift diagnosis
/// (`docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`) measured
/// two structural cliffs on WRLD3's 166-node SCC:
///
/// - Cliff A: `build_element_level_loops` allocates ~17 GB of
///   `Loop`/`Link` structs for 1.86M enumerated circuits.
/// - Cliff B: each `rel_loop_score` equation is 75,303,840 bytes
///   (~75.3 MB); full emission projects to ~140 TB of equation text.
///
/// The diagnosis recommends gating on largest-SCC size rather than
/// total circuit count because the O(P²) rel_loop_score term binds on
/// partition size (each partition maps to one SCC), and the size of
/// the largest SCC upper-bounds the worst-case partition size.
/// Threshold 50 keeps every existing LTM test model on the exhaustive
/// path while catching dense feedback graphs like WRLD3 (166-node SCC)
/// well before they reach the cliffs.
pub const MAX_LTM_SCC_NODES: usize = 50;

/// Detect all feedback loops in a single model.
///
/// Runs Johnson's enumeration with no per-call circuit budget; the
/// upstream [`crate::db::model_ltm_variables`] pipeline is responsible
/// for skipping LTM on models whose element-level SCC exceeds
/// [`MAX_LTM_SCC_NODES`].  Since `usize::MAX` is passed as the budget,
/// the `TruncatedByBudget` branch is unreachable.
pub fn detect_loops(model: &ModelStage1, project: &Project) -> Result<Vec<Loop>> {
    let graph = CausalGraph::from_model(model, project)?;
    Ok(graph
        .find_loops_with_limit(usize::MAX)
        .expect("usize::MAX budget cannot be exhausted by Johnson's enumeration"))
}

/// Return the start index of the **canonical cyclic rotation** of `s`.
///
/// `s` is treated as a closed cycle and the returned index `k` denotes
/// the rotation `s[k], s[k+1 % n], ..., s[k-1 % n]` whose full ordered
/// element sequence is lex-smallest among all `n` rotations.  Element-
/// wise comparison proceeds left-to-right; the first differing position
/// decides.  In the elementary-cycle setting (no repeated nodes, which
/// Johnson's and the discovery DFS both enforce) the lex-smallest
/// **starting element** alone determines the canonical rotation; the
/// full-sequence comparison keeps the helper correct on hypothetical
/// inputs with equal starting elements.
///
/// Returns 0 for empty or single-element input.
///
/// ## Complexity
///
/// Worst-case O(n^2).  We deliberately keep the simple cyclic
/// comparison loop instead of an O(n) algorithm such as Booth (1980)
/// or Duval/Lyndon factorization: empirical timing on the WRLD3 LTM
/// element-level enumeration (~1.86M raw cycles, mean cycle length
/// ~47, alphabet of distinct `u32` indices) shows the simple loop is
/// 10-15% faster end-to-end because the inner comparison almost
/// always exits at the first differing element when the cycle uses
/// distinct nodes -- so the algorithm runs in roughly O(n) on real
/// inputs while paying near-zero per-element overhead.  The O(n)
/// algorithms incur higher constant factors (failure-function or
/// equivalent bookkeeping per position) that dominate for the small
/// `n` permitted by [`MAX_LTM_SCC_NODES`] (50).  See PR #494 for the
/// timing data.
pub(crate) fn lex_smallest_rotation_start<T: Ord>(s: &[T]) -> usize {
    let n = s.len();
    if n <= 1 {
        return 0;
    }
    let mut best = 0usize;
    for candidate in 1..n {
        // Compare the rotation starting at `candidate` against the
        // rotation starting at `best`, position by position.
        for i in 0..n {
            let cur = &s[(best + i) % n];
            let cand = &s[(candidate + i) % n];
            match cand.cmp(cur) {
                std::cmp::Ordering::Less => {
                    best = candidate;
                    break;
                }
                std::cmp::Ordering::Greater => break,
                std::cmp::Ordering::Equal => continue,
            }
        }
    }
    best
}

/// Return the **canonical cyclic rotation** of `circuit`.
///
/// The input is treated as a closed elementary cycle: position `i`
/// edges to position `(i + 1) % len`.  Two slices that represent the
/// same directed cycle but at different starting positions
/// (e.g. `[A, B, C]` and `[B, C, A]`) canonicalize to the same output.
/// Two distinct directed cycles over the same node set
/// (e.g. `[A, B, C]` representing `A -> B -> C -> A` and `[A, C, B]`
/// representing `A -> C -> B -> A`) canonicalize to **different**
/// outputs and are thus retained as distinct loops by callers that
/// dedup on this key.
///
/// Convenience wrapper around [`lex_smallest_rotation_start`] that
/// allocates a fresh `Vec`.  The hot Johnson enumerator in
/// `IndexedGraph::johnson_circuit` calls
/// [`lex_smallest_rotation_start`] directly so it can write the
/// rotation halves into a reusable buffer, avoiding the per-emission
/// allocation this wrapper performs.
///
/// Empty input returns empty.
pub(crate) fn canonical_rotation<T: Ord + Clone>(circuit: &[T]) -> Vec<T> {
    let n = circuit.len();
    if n == 0 {
        return Vec::new();
    }
    let start = lex_smallest_rotation_start(circuit);
    let mut out = Vec::with_capacity(n);
    out.extend_from_slice(&circuit[start..]);
    out.extend_from_slice(&circuit[..start]);
    out
}

/// Strip a trailing `[...]` element subscript from an LTM node or
/// link-endpoint name.
///
/// `"population[nyc]"` -> `"population"`; multi-dimensional subscripts
/// like `"x[nyc,boston]"` also collapse to `"x"` (the last `[` is the
/// truncation point); a name without `[` is returned unchanged. Shared
/// by the loop builder (`db::ltm`) and the LTM equation generators
/// (`ltm_augment`), which both operate on these element-level
/// identifier strings.
pub(crate) fn strip_subscript(name: &str) -> &str {
    match name.rfind('[') {
        Some(pos) => &name[..pos],
        None => name,
    }
}

/// Split an LTM node name into `(variable_level_name, Some(subscript))`,
/// or `(name, None)` when there is no `[...]` subscript.
///
/// `"migration_pressure[boston]"` -> `("migration_pressure", Some("boston"))`;
/// `"x[nyc,boston]"` -> `("x", Some("nyc,boston"))`; `"x"` -> `("x", None)`.
/// The subscript text is returned without the surrounding brackets.
pub(crate) fn split_node_subscript(name: &str) -> (&str, Option<&str>) {
    match (name.rfind('['), name.rfind(']')) {
        (Some(open), Some(close)) if close > open => (&name[..open], Some(&name[open + 1..close])),
        _ => (name, None),
    }
}

#[cfg(test)]
mod tests;
