// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM-backed feedback-loop detection for layout metadata.
//!
//! A submodule of `layout` (split out purely for the per-file line cap;
//! `scripts/lint-project.sh` rule 2): `compute_metadata` calls
//! `try_detect_ltm_loops` to build `metadata::FeedbackLoop`s -- with
//! importance series and cycle partitions -- from the incremental salsa LTM
//! pipeline, falling back to persisted `loop_metadata` when any step fails.

use std::collections::HashMap;

use super::LoopPolarity;
use super::metadata;

/// Try to detect feedback loops using LTM analysis via the incremental
/// salsa compilation path. Compiles the project, detects loops, augments
/// with synthetic LTM variables, simulates, and extracts importance time
/// series. Returns `None` if any step fails, signaling the caller to fall
/// back to persisted loop_metadata.
pub(super) fn try_detect_ltm_loops(
    db: &mut crate::db::SimlinDb,
    source_project: crate::db::SourceProject,
    model_name: &str,
) -> Option<Vec<metadata::FeedbackLoop>> {
    try_detect_ltm_loops_incremental(db, source_project, model_name)
}

/// Incremental salsa path for LTM loop detection.
fn try_detect_ltm_loops_incremental(
    db: &mut crate::db::SimlinDb,
    source_project: crate::db::SourceProject,
    actual_name: &str,
) -> Option<Vec<metadata::FeedbackLoop>> {
    use salsa::Setter;

    let actual_name_owned = actual_name.to_string();

    // Phase 1: Model lookup and loop detection.
    let (source_model, detected) = {
        let canonical_name = crate::canonicalize(&actual_name_owned);
        let source_model = *source_project.models(db).get(canonical_name.as_ref())?;
        let detected = crate::db::model_detected_loops(db, source_model, source_project);
        (source_model, detected)
    };

    if detected.loops.is_empty() {
        return Some(Vec::new());
    }

    // Phase 2: LTM compile and simulate.
    source_project.set_ltm_enabled(db).to(true);
    let vm_result = crate::db::compile_project_incremental(db, source_project, &actual_name_owned)
        .ok()
        .and_then(|compiled_sim| crate::vm::Vm::new(compiled_sim).ok())
        .and_then(|mut vm| {
            vm.run_to_end().ok()?;
            Some(vm)
        });

    // Capture the loop_partitions mapping AND per-loop slot counts while
    // LTM is still enabled so the cached `model_ltm_variables` query sees
    // the same flag value the VM ran under.  Per-element rel scores need
    // both the partition map (which loops normalize together) and the
    // per-loop slot count (how many elements each A2A loop occupies).
    let (loop_partitions, n_slots_by_loop) = if vm_result.is_some() {
        let ltm_vars = crate::db::model_ltm_variables(db, source_model, source_project);
        let dm_dims = crate::db::project_datamodel_dims(db, source_project);
        let dim_size: HashMap<&str, usize> = dm_dims.iter().map(|d| (d.name(), d.len())).collect();
        let prefix = "$\u{205A}ltm\u{205A}loop_score\u{205A}";
        let n_slots: HashMap<String, usize> = ltm_vars
            .vars
            .iter()
            .filter_map(|v| {
                let id = v.name.strip_prefix(prefix)?;
                let n = if v.dimensions.is_empty() {
                    1
                } else {
                    v.dimensions
                        .iter()
                        .map(|d| dim_size.get(d.as_str()).copied().unwrap_or(1))
                        .product()
                };
                Some((id.to_string(), n))
            })
            .collect();
        (ltm_vars.loop_partitions.clone(), n_slots)
    } else {
        (indexmap::IndexMap::new(), HashMap::new())
    };

    source_project.set_ltm_enabled(db).to(false);

    let vm = vm_result?;
    let results = vm.into_results();

    // `rel_loop_score` is no longer a VM variable; derive it post-sim from
    // the `loop_score` series the VM does emit, using the per-slot partition
    // mapping cached on `model_ltm_variables`.  See
    // `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`.
    //
    // For arrayed (A2A) loops we compute per-element rel scores then
    // aggregate to a single signed series via argmax-abs across slots --
    // i.e. each step's importance is the dominant element's contribution,
    // with sign preserved.  For scalar loops this reduces to identity.
    // The aggregation is delegated to `ltm_post::aggregate_per_element_argmax_abs`
    // so the partition-stride handling (mixed partitions where stride >
    // per-loop n_slots) is centralized and unit-testable.  See issue #463.
    // `compute_rel_loop_scores_per_element` derives each loop's slot count
    // from `loop_partitions[id].len()`, so no separate slot-count map is
    // threaded; `aggregate_per_element_argmax_abs` still takes one.
    let per_element_rel_scores =
        crate::ltm_post::compute_rel_loop_scores_per_element(&results, &loop_partitions);
    let importance_by_loop = crate::ltm_post::aggregate_per_element_argmax_abs(
        &per_element_rel_scores,
        &n_slots_by_loop,
        results.step_count,
    );

    // Phase 3: Build feedback loop structs from VM results.
    let mut feedback_loops = Vec::new();
    for dl in &detected.loops {
        // metadata::LoopPolarity only carries R/B/U: the layout legend does
        // not visually distinguish "mostly R" from "R" today, so the
        // mostly-* variants collapse onto their dominant equivalents here.
        // The polarity_confidence on `dl` is dropped at this boundary --
        // when the layout pipeline learns to surface confidence it should
        // pass `dl.polarity_confidence` through alongside the polarity.
        let polarity = match dl.polarity {
            crate::db::DetectedLoopPolarity::Reinforcing
            | crate::db::DetectedLoopPolarity::MostlyReinforcing => LoopPolarity::Reinforcing,
            crate::db::DetectedLoopPolarity::Balancing
            | crate::db::DetectedLoopPolarity::MostlyBalancing => LoopPolarity::Balancing,
            crate::db::DetectedLoopPolarity::Undetermined => LoopPolarity::Undetermined,
        };

        let variables: Vec<String> = {
            // A cross-element loop's variables carry element subscripts
            // (`pool[a]` -- the detected surface shares the scored surface's
            // per-element loop builder since GH #746), but layout matches
            // chain entries against view-element idents, which are
            // variable-level. Strip the subscripts so the loop-aware
            // placement heuristics keep firing for arrayed models.
            let mut vars: Vec<String> = dl
                .variables
                .iter()
                .map(|v| crate::ltm::strip_subscript(v).to_string())
                .collect();
            if let Some(first) = vars.first().cloned() {
                vars.push(first);
            }
            vars
        };

        let importance_series = importance_by_loop.get(&dl.id).cloned().unwrap_or_default();

        feedback_loops.push(metadata::FeedbackLoop {
            name: dl.id.clone(),
            polarity,
            variables,
            importance_series,
            dominant_period: None,
            // The detected loop's result-scoped cycle partition (an A2A
            // loop's index is its first resolving slot's partition), so
            // dominant-period selection competes partition-mates only
            // (GH #998).
            partition: dl.partition,
        });
    }

    Some(feedback_loops)
}
