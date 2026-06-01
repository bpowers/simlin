// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Modeler-pinned feedback loops (the `LOOPSCORE` capability, LTM ref
//! section 10).
//!
//! A practitioner pins a loop by naming its variable *set* (via the
//! `SetLoopName` patch primitive, which writes `LoopMetadata`). The LTM
//! discovery heuristic may not surface a loop the modeler cares about; pinning
//! forces it to ALWAYS be scored, in both exhaustive and discovery mode.
//!
//! `model_pinned_loops` is the single salsa-tracked place a pinned loop's
//! variable set is validated against the causal graph and ordered into a
//! scored [`Loop`]. Both `model_ltm_variables` (which emits the pinned
//! `loop_score` synthetic var) and `model_detected_loops` (the FFI loop
//! surface) read it, so a pinned loop appears identically through both paths.

use std::collections::HashSet;

use crate::common::{Canonical, Ident};
use crate::db::{
    CycleClass, Db, SourceModel, SourceProject, causal_graph_with_modules, classify_cycle,
    model_causal_edges, model_edge_shapes, project_datamodel_dims, variable_dimensions,
};
use crate::ltm::Loop;

use super::loops::build_a2a_loop_stocks;

/// A pinned loop the LTM pipeline must always score, paired with the user's
/// chosen name so a caller can map the synthetic `pin{n}` id back to a label.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct PinnedLoop {
    /// The fully-annotated loop (links, polarity, stocks), with a stable
    /// `pin{n}` id assigned in declaration order.
    pub loop_: Loop,
    /// The user-supplied loop name. Preserved so a caller can recover the
    /// label behind a `pin{n}` id (the FFI loop surface reports the id +
    /// variable set + this name; callers that prefer can match on the variable
    /// set the way they do for enumerated loops).
    pub name: String,
}

/// One model's resolved pinned loops, plus the names of pins that failed
/// validation so the caller can surface a diagnostic without re-deriving the
/// failure.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub struct PinnedLoopsResult {
    pub loops: Vec<PinnedLoop>,
    /// `(name, reason)` for each pinned loop whose variable set did not form a
    /// scorable feedback loop. The reason is a human-readable explanation for
    /// the surfaced diagnostic.
    pub invalid: Vec<(String, String)>,
}

/// Resolve and validate a model's pinned loops against its causal graph.
///
/// For each non-deleted pin (already projected to canonical variable names at
/// sync time), this recovers the loop's cyclic order from the causal graph,
/// confirms the named set forms a closed cycle containing at least one stock,
/// and assigns a stable `pin{n}` id (n = declaration index, so ids never
/// collide with the enumerator's `r{n}`/`b{n}`/`u{n}` namespace). Pins that
/// fail any check land in `invalid` rather than producing a garbage score.
///
/// The resolved cycle is then dimension-classified exactly the way the
/// exhaustive enumerator classifies cycles ([`classify_cycle`], GH #653):
///
/// - **PureScalar** -> a scalar `Loop` (the pre-#653 behavior, correct for
///   scalar models).
/// - **PureSameElementA2A** -> the `Loop` carries the cycle's shared
///   `dimensions` and element-level stocks, so its loop score is emitted as
///   an arrayed (per-element) variable and its cycle partition resolves per
///   slot.
/// - **CrossElementOrMixed** -> currently still the scalar fallback (its
///   loop-score equation may fail to compile and surface the
///   fragment-diagnostics Warning); element-level expansion for these cycles
///   is the next step of GH #653.
///
/// Returned by value (not `salsa::tracked`) because `Loop` does not implement
/// the `PartialEq`/`Update` salsa caching requires; callers invoke it directly
/// off the salsa-tracked `causal_graph_with_modules` / `pinned_loops` inputs,
/// so the underlying graph build is still incrementally cached.
pub(crate) fn model_pinned_loops(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> PinnedLoopsResult {
    let specs = model.pinned_loops(db);
    if specs.is_empty() {
        return PinnedLoopsResult::default();
    }

    // A stock-free model has no feedback loops at all; every pin is invalid.
    let edges = model_causal_edges(db, model, project);
    let graph = causal_graph_with_modules(db, model, project);
    // Classification inputs: per-edge access shapes and per-variable
    // dimensions, the same data the tiered enumerator classifies cycles with.
    let edge_shapes = model_edge_shapes(db, model, project);
    let source_vars = model.variables(db);
    let dm_dims = project_datamodel_dims(db, project);

    let mut result = PinnedLoopsResult::default();
    for (idx, spec) in specs.iter().enumerate() {
        let id = format!("pin{}", idx + 1);

        if spec.variables.len() < 2 {
            result.invalid.push((
                spec.name.clone(),
                format!(
                    "a pinned loop must name at least two variables that form a feedback loop; \
                     '{}' names {}",
                    spec.name,
                    spec.variables.len()
                ),
            ));
            continue;
        }

        let vars: HashSet<Ident<Canonical>> =
            spec.variables.iter().map(|v| Ident::new(v)).collect();

        let Some(cycle) = graph.order_variable_cycle(&vars) else {
            result.invalid.push((
                spec.name.clone(),
                format!(
                    "the variables named by pinned loop '{}' do not form a closed feedback loop \
                     in the model's causal graph: [{}]",
                    spec.name,
                    spec.variables.join(", ")
                ),
            ));
            continue;
        };

        // A standard feedback loop includes at least one stock (LTM ref 2.1).
        // A purely-instantaneous cycle would be a compile-time circular
        // dependency, not a feedback loop, so reject it with a clear message.
        let has_stock = cycle.iter().any(|n| edges.stocks.contains(n.as_str()));
        if !has_stock {
            result.invalid.push((
                spec.name.clone(),
                format!(
                    "pinned loop '{}' contains no stock; a feedback loop must pass through at \
                     least one stock",
                    spec.name
                ),
            ));
            continue;
        }

        // Dimension-classify the cycle (GH #653). Module nodes report empty
        // dimensions, matching the tiered enumerator's treatment of modules
        // as scalar graph nodes.
        let cycle_strs: Vec<String> = cycle.iter().map(|c| c.as_str().to_string()).collect();
        let dim_lookup = |name: &str| -> Vec<crate::dimensions::Dimension> {
            source_vars
                .get(name)
                .map(|sv| variable_dimensions(db, *sv, project).to_vec())
                .unwrap_or_default()
        };
        let loop_ = match classify_cycle(&cycle_strs, edge_shapes, &dim_lookup) {
            CycleClass::PureSameElementA2A { dimensions } => {
                build_a2a_pin_loop(&graph, &cycle, id, &dimensions, dm_dims)
            }
            // PureScalar: the pre-#653 scalar construction is correct.
            // CrossElementOrMixed: element-level expansion is the next step
            // of GH #653; until then these keep the scalar fallback (whose
            // mis-resolved equation surfaces the fragment-diagnostics
            // Warning rather than passing silently).
            CycleClass::PureScalar | CycleClass::CrossElementOrMixed => {
                graph.build_loop_from_cycle(&cycle, id)
            }
        };
        result.loops.push(PinnedLoop {
            loop_,
            name: spec.name.clone(),
        });
    }

    result
}

/// Build the `Loop` for a pinned PureSameElementA2A cycle: variable-level
/// links, the cycle's shared dimensions (mapped to datamodel casing so the
/// loop-score equation parses), and element-level stocks (the `Loop`
/// docstring's granularity invariant, required for per-slot partition
/// resolution).
///
/// Mirrors `build_loops_from_tiered`'s fast-path construction. Module stock
/// enrichment is not needed: a cycle containing a module node classifies as
/// CrossElementOrMixed (modules are scalar graph nodes), so it never reaches
/// this function.
fn build_a2a_pin_loop(
    graph: &crate::ltm::CausalGraph,
    cycle: &[Ident<Canonical>],
    id: String,
    canonical_dims: &[String],
    dm_dims: &[crate::datamodel::Dimension],
) -> Loop {
    let links = graph.circuit_to_links(cycle);
    let var_stocks = graph.find_stocks_in_loop(cycle);
    let polarity = graph.calculate_polarity(&links);
    let dimensions: Vec<String> = canonical_dims
        .iter()
        .map(|canonical| {
            dm_dims
                .iter()
                .find(|dm| crate::common::canonicalize(dm.name()).as_ref() == canonical.as_str())
                .map(|dm| dm.name().to_string())
                .unwrap_or_else(|| canonical.to_string())
        })
        .collect();
    let stocks = build_a2a_loop_stocks(&var_stocks, &dimensions, dm_dims);
    Loop {
        id,
        links,
        stocks,
        polarity,
        dimensions,
        slot_links: vec![],
    }
}
