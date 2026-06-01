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
use crate::db::{Db, SourceModel, SourceProject, causal_graph_with_modules, model_causal_edges};
use crate::ltm::Loop;

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

        let loop_ = graph.build_loop_from_cycle(&cycle, id);
        result.loops.push(PinnedLoop {
            loop_,
            name: spec.name.clone(),
        });
    }

    result
}
