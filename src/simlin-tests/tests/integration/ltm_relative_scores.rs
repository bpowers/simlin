// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Relative loop scores through the real pipeline, checked against hand
//! calculations on a fixture whose per-element loops share one cycle
//! partition.
//!
//! The owner of the normalization is `ltm_post::compute_rel_loop_scores`:
//! every `(loop, slot)` of a partition divides by the same sum of `|score|`
//! over all of the partition's members -- scalar loops and every slot of
//! every arrayed loop alike.  These tests pin that rule on
//! `test/cross_element_ltm`, where the two stocks are coupled through
//! migration and so every loop of the model, arrayed or not, lives in one
//! partition.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;

use simlin_engine::db::{
    DetectedLoop, SimlinDb, compile_project_incremental, model_detected_loops, model_ltm_variables,
    sync_from_datamodel_incremental,
};
use simlin_engine::indexmap::IndexMap;
use simlin_engine::{Results, Vm, ltm_post, xmile};

fn load_xmile_model(path: &str) -> simlin_engine::datamodel::Project {
    let f = File::open(path).unwrap_or_else(|e| panic!("failed to open {path}: {e}"));
    let mut f = BufReader::new(f);
    xmile::project_from_reader(&mut f)
        .unwrap_or_else(|e| panic!("failed to parse XMILE from {path}: {e}"))
}

/// Compile `project` with the LTM overlay, run it, and return the results
/// with the loop list and the per-slot partition map the same derivation
/// produced -- exactly what production feeds the owner.
fn simulate_with_ltm(
    project: &simlin_engine::datamodel::Project,
) -> (
    Results,
    Vec<DetectedLoop>,
    IndexMap<String, Vec<Option<usize>>>,
) {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled =
        compile_project_incremental(&db, sync.project, "main", simlin_engine::db::LtmOverlay::On)
            .expect("the fixture compiles with LTM");
    let source_model = sync.models["main"].source_model;
    let loop_partitions = model_ltm_variables(&db, source_model, sync.project)
        .loop_partitions
        .clone();
    let loops = model_detected_loops(&db, source_model, sync.project)
        .loops
        .clone();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("the fixture simulates");
    (vm.into_results(), loops, loop_partitions)
}

/// The loop whose variable set is exactly `vars` (subscripts included).
fn loop_with_variables<'a>(loops: &'a [DetectedLoop], vars: &[&str]) -> &'a DetectedLoop {
    let want: HashSet<&str> = vars.iter().copied().collect();
    loops
        .iter()
        .find(|l| {
            l.variables
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>()
                == want
        })
        .unwrap_or_else(|| {
            panic!(
                "no loop over {vars:?}; loops: {:?}",
                loops
                    .iter()
                    .map(|l| (l.id.as_str(), l.variables.clone()))
                    .collect::<Vec<_>>()
            )
        })
}

/// `test/cross_element_ltm` at dt = 1.  Region = {NYC, Boston};
/// `population[NYC]` starts at 1000, `population[Boston]` at 500.
///
/// Hand calculation.  Because `migration_in[NYC]` and `migration_out[NYC]`
/// are both `(pop[NYC] - pop[Boston]) * 0.01` while Boston's two migration
/// flows are clamped to 0 by `MAX`, each stock's net flow is exactly
/// `0.02 * pop[r]`: both populations grow at 2% per step forever, and so
/// does their difference.  Every link score is therefore a ratio of
/// quantities that all scale by 1.02 per step, and the raw loop scores are
/// constant once active:
///
/// - births loop, per element: `pop[r] -> births[r]` is `1` (linear), and
///   `births[r] -> pop[r]` is `Δbirths / Δnet = 1` (births IS the net flow):
///   raw `+1` at both slots.
/// - migration_out loop at NYC (`pop[NYC] -> mp[NYC] -> out[NYC] ->
///   pop[NYC]`): `Δ_{pop[NYC]} mp[NYC] / Δmp[NYC] = 0.01*Δpop[NYC] /
///   (0.01*(Δpop[NYC] - Δpop[Boston]))` = `20 / (20 - 10) = 2`; `mp -> out`
///   is `1`; `out -> pop` is `-(Δout / Δnet) = -(0.1 / 0.4) = -0.25`: raw
///   `-0.5`.  At Boston every migration flow is 0, so that slot scores 0.
/// - the cross-element loop `pop[NYC] -> mp[Boston] -> in[NYC] -> pop[NYC]`:
///   `pop[NYC] -> mp[Boston]` is `-2` (same magnitudes, opposite sign),
///   `mp[Boston] -> in[NYC]` is `-1`, `in[NYC] -> pop[NYC]` is `+0.25`:
///   raw `+0.5`.
/// - every other loop crosses a Boston migration flow, whose link scores
///   are 0, so it scores 0.
///
/// One partition holds both stocks, so the partition sum is
/// `1 + 1 + 0.5 + 0.5 = 3` and the shares are `1/3, 1/3, -1/6, +1/6`.
/// Grouping by `(partition, slot)` instead gives the births loop `1/2` at
/// NYC and `2/3` at Boston, and hands the scalar cross-element loop two
/// different series (`1/4` and `1/3`): none of those numbers is the share of
/// anything.
#[test]
fn cross_element_loops_normalize_over_the_whole_partition() {
    let project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");
    let (results, loops, loop_partitions) = simulate_with_ltm(&project);
    let rel = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);

    let births = loop_with_variables(&loops, &["population", "births"]);
    let out_loop = loop_with_variables(
        &loops,
        &["population", "migration_pressure", "migration_out"],
    );
    let cross = loop_with_variables(
        &loops,
        &[
            "population[nyc]",
            "migration_pressure[boston]",
            "migration_in[nyc]",
        ],
    );
    assert_eq!(
        loop_partitions[&births.id].len(),
        2,
        "births is A2A over Region"
    );
    assert_eq!(
        loop_partitions[&out_loop.id].len(),
        2,
        "migration_out loop is A2A"
    );
    assert_eq!(
        loop_partitions[&cross.id].len(),
        1,
        "the cross-element loop is scalar"
    );
    let partition: HashSet<Option<usize>> = loop_partitions.values().flatten().copied().collect();
    assert_eq!(
        partition.len(),
        1,
        "the coupled stocks put every slot of every loop in one partition: {loop_partitions:?}"
    );

    // Both flow-to-stock scores are defined from the second saved step on;
    // the ratios are time-invariant, so any later step reads the same
    // shares.  Steps 2, 10 and 30 (t = 2, 10, 30 at dt = 1).
    for step in [2usize, 10, 30] {
        let at = |id: &str, n_slots: usize, k: usize| rel[id][step * n_slots + k];
        let close = |got: f64, want: f64, what: &str| {
            assert!(
                (got - want).abs() < 1e-9,
                "step {step}: {what} = {got}, expected {want}"
            );
        };
        close(at(&births.id, 2, 0), 1.0 / 3.0, "births[nyc]");
        close(at(&births.id, 2, 1), 1.0 / 3.0, "births[boston]");
        close(
            at(&out_loop.id, 2, 0),
            -1.0 / 6.0,
            "migration_out loop[nyc]",
        );
        close(at(&out_loop.id, 2, 1), 0.0, "migration_out loop[boston]");
        assert_eq!(
            rel[&cross.id].len(),
            results.step_count,
            "a scalar loop has one series"
        );
        close(
            at(&cross.id, 1, 0),
            1.0 / 6.0,
            "cross-element migration_in loop",
        );

        // The partition identity: the magnitudes over EVERY member -- each
        // slot of each loop -- sum to 1, and the four members above are the
        // only active ones.
        let mut total = 0.0;
        for (id, series) in &rel {
            let n_slots = loop_partitions[id].len().max(1);
            for k in 0..n_slots {
                let v = series[step * n_slots + k];
                assert!(v.is_finite(), "step {step}: {id} slot {k} is {v}");
                total += v.abs();
            }
        }
        assert!(
            (total - 1.0).abs() < 1e-9,
            "step {step}: partition shares sum to {total}"
        );
    }
}

/// The bare-id aggregate of an arrayed loop is the argmax-abs across its
/// slots, taken from the same per-slot series: for the births loop both
/// slots read `1/3`, for the migration_out loop the NYC slot's `-1/6` wins
/// over Boston's `0`.
#[test]
fn cross_element_bare_id_aggregate_picks_the_dominant_slot() {
    let project = load_xmile_model("../../test/cross_element_ltm/cross_element.stmx");
    let (results, loops, loop_partitions) = simulate_with_ltm(&project);
    let rel = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    let agg = ltm_post::aggregate_per_element_argmax_abs(&rel, results.step_count);

    let births = loop_with_variables(&loops, &["population", "births"]);
    let out_loop = loop_with_variables(
        &loops,
        &["population", "migration_pressure", "migration_out"],
    );
    for step in [2usize, 10, 30] {
        assert!((agg[&births.id][step] - 1.0 / 3.0).abs() < 1e-9);
        assert!((agg[&out_loop.id][step] - (-1.0 / 6.0)).abs() < 1e-9);
    }
    for series in agg.values() {
        assert_eq!(series.len(), results.step_count);
    }
}
