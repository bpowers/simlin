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
//!   polarity surface that callers like `db_analysis` and `ltm_finding`
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
pub(crate) use types::normalize_module_ref;

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

#[cfg(test)]
mod tests;
