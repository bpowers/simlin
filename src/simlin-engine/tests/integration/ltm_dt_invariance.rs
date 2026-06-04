// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Regression tests for the `dt`-invariance of LTM (Loops That Matter)
//! loop scores -- "Finding 1" of the LTM deep review.
//!
//! ## The invariant being pinned
//!
//! Schoenberg, Davidsen & Eberlein (2020), "Understanding model behavior
//! using loops that matter" (sec. 4.1 / Appendix B), establishes that the
//! *raw* loop score of an **isolated loop** -- a loop that is the only
//! loop acting on its stock(s) -- is exactly `+/-1` at every timestep,
//! regardless of loop gain *and regardless of the integration step `dt`*.
//!
//! ## The bug these tests guard against
//!
//! `generate_flow_to_stock_equation` in `ltm_augment.rs` builds the
//! flow-to-stock link score. The 2023 paper's Eq. 3 writes it as
//! `sign * |Delta(flow) / (Delta(S_t) - Delta(S_{t-dt}))|`. Taken
//! literally that is only correct for `dt = 1`: the denominator is the
//! second-order stock change, which under Euler integration is
//! `dt * (netflow(t-dt) - netflow(t-2dt))` and so already carries one
//! factor of `dt`, while the raw flow delta in the numerator carries
//! none. The dimensionally-correct discretization of the paper's
//! continuous form (Eq. 6, `|di/dt / d^2S/dt^2|`, which is manifestly
//! dimensionless) multiplies the numerator by `dt`.
//!
//! Without that factor each flow-to-stock link score is `1/dt` too
//! large, and because a loop score is the product of its link scores
//! the error compounds once per flow-to-stock link: an isolated
//! one-stock loop reads `1/dt`, an isolated two-stock loop reads
//! `1/dt^2`, and so on. Normalization into relative loop scores does
//! *not* cancel it when a partition mixes loops of different stock
//! counts -- so at `dt != 1` the dominant loop of a partition can flip
//! purely as an artifact of the step size.
//!
//! The three tests below pin, respectively: the one-stock isolated-loop
//! value, the two-stock isolated-loop value (proving the error does not
//! merely rescale every loop equally), and the `dt`-invariance of the
//! relative scores in a mixed-stock-count partition (the real-world
//! impact).

use std::collections::HashMap;

use simlin_engine::db::{
    SimlinDb, compile_project_incremental, model_ltm_variables, set_project_ltm_enabled,
    sync_from_datamodel_incremental,
};
use simlin_engine::test_common::TestProject;
use simlin_engine::{Results, Vm, ltm_post};

/// Canonical prefix of an LTM `loop_score` synthetic variable. The
/// separator is U+205A (TWO DOT PUNCTUATION), the character LTM uses to
/// namespace its synthetic variables.
const LOOP_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}loop_score\u{205A}";

/// Compile `project` with LTM enabled (exhaustive mode), run it to
/// completion, and return the simulation results alongside the
/// loop-id -> per-slot cycle-partition map that the post-simulation
/// relative-score computation consumes.
fn run_ltm(
    project: &simlin_engine::datamodel::Project,
) -> (Results, HashMap<String, Vec<Option<usize>>>) {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("LTM-enabled compilation should succeed");
    let loop_partitions = model_ltm_variables(&db, sync.models["main"].source_model, sync.project)
        .loop_partitions
        .clone();
    let mut vm = Vm::new(compiled).expect("VM construction should succeed");
    vm.run_to_end()
        .expect("VM simulation should run to completion");
    (vm.into_results(), loop_partitions)
}

/// Sorted names of every `loop_score` synthetic variable in `results`.
fn loop_score_names(results: &Results) -> Vec<String> {
    let mut names: Vec<String> = results
        .offsets
        .keys()
        .filter(|k| k.as_str().starts_with(LOOP_SCORE_PREFIX))
        .map(|k| k.as_str().to_string())
        .collect();
    names.sort();
    names
}

/// The full `loop_score` timeseries for `name`, bounded to the
/// simulation's real step count. (`Results::iter` already yields exactly
/// `step_count` chunks, so trailing unused rows in the backing buffer
/// are never read.)
fn loop_score_series(results: &Results, name: &str) -> Vec<f64> {
    let ident = results
        .offsets
        .keys()
        .find(|k| k.as_str() == name)
        .unwrap_or_else(|| {
            panic!(
                "loop score variable {name:?} not found in results; have: {:?}",
                results
                    .offsets
                    .keys()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
            )
        });
    let offset = results.offsets[ident];
    results.iter().map(|row| row[offset]).collect()
}

/// Number of saved steps at the start of a flow-to-stock link score
/// series that are pinned to `0` by the equation's startup guard
/// (`TIME = INITIAL_TIME` and `PREVIOUS(TIME, INITIAL_TIME) =
/// INITIAL_TIME`): the second-order stock difference in the score's
/// denominator needs two steps of history before it is defined.
const STARTUP_STEPS: usize = 2;

/// Absolute tolerance for the isolated-loop `+/-1` check. Observed
/// floating-point error in the settled region is ~1e-13; the bug this
/// guards against shifts the score by a whole `(1/dt)^k - 1 >= 1`, so
/// any tolerance well below 1 catches it.
const SETTLED_TOL: f64 = 1e-6;

/// Assert that `series` is an isolated loop's raw `loop_score`: exactly
/// `0` through the startup guard, then within [`SETTLED_TOL`] of
/// `expected` (`+/-1`) at every later step. `dt` is threaded through
/// only to make failure messages actionable.
fn assert_isolated_loop_series(series: &[f64], expected: f64, dt: f64) {
    assert!(
        series.len() > STARTUP_STEPS + 1,
        "dt={dt}: loop_score series has only {} step(s); too short to verify the \
         isolated-loop invariant",
        series.len()
    );
    for (step, &value) in series.iter().enumerate() {
        if step < STARTUP_STEPS {
            assert_eq!(
                value, 0.0,
                "dt={dt}: step {step} is inside the startup guard and must be exactly 0, \
                 got {value}"
            );
        } else {
            assert!(
                (value - expected).abs() < SETTLED_TOL,
                "dt={dt}: settled step {step} has loop_score {value}, expected {expected}. \
                 An isolated loop's raw score is exactly +/-1 at every dt; a value scaled \
                 by (1/dt)^k means the flow-to-stock link score lost its `dt` factor \
                 (LTM review Finding 1)."
            );
        }
    }
}

/// Finding 1, one-stock case: an isolated single-stock reinforcing loop
/// (`s -> births -> s`) has a raw loop score of exactly `+1` at every
/// `dt`. `dt = 1.0` is the control -- the missing `dt` factor is
/// invisible there, so this case also guards against an over-correction
/// -- while `dt = 0.5` and `dt = 0.25` are where a regressed formula
/// would read `1/dt` (`2.0` and `4.0` respectively).
#[test]
fn isolated_one_stock_loop_raw_score_is_one_at_every_dt() {
    for dt in [1.0_f64, 0.5, 0.25] {
        let project = TestProject::new("iso_one_stock")
            .with_sim_time(0.0, 8.0, dt)
            .stock("s", "100", &["births"], &[], None)
            .flow("births", "s * 0.1", None)
            .build_datamodel();

        let (results, _) = run_ltm(&project);

        let names = loop_score_names(&results);
        assert_eq!(
            names.len(),
            1,
            "dt={dt}: the one-stock model has exactly one feedback loop, found {names:?}"
        );

        let series = loop_score_series(&results, &names[0]);
        // The loop is reinforcing (more s -> more births -> more s), so
        // its isolated-loop score is +1, not -1.
        assert_isolated_loop_series(&series, 1.0, dt);
    }
}

/// Finding 1, two-stock case: an isolated two-stock reinforcing loop
/// (`a -> in_b -> b -> in_a -> a`) also has a raw loop score of exactly
/// `+1` at every `dt`. This is the key test that the missing-`dt` error
/// does *not* simply rescale every loop equally: the loop has two
/// flow-to-stock links, so a regressed formula compounds the error to
/// `1/dt^2` (`4.0` at `dt = 0.5`, `16.0` at `dt = 0.25`) -- a different
/// power of `dt` than the one-stock loop, which is precisely why
/// normalization cannot rescue a mixed-stock-count partition.
#[test]
fn isolated_two_stock_loop_raw_score_is_one_at_every_dt() {
    for dt in [1.0_f64, 0.5, 0.25] {
        let project = TestProject::new("iso_two_stock")
            .with_sim_time(0.0, 8.0, dt)
            .stock("a", "100", &["in_a"], &[], None)
            .stock("b", "100", &["in_b"], &[], None)
            .flow("in_a", "b * 0.1", None)
            .flow("in_b", "a * 0.1", None)
            .build_datamodel();

        let (results, _) = run_ltm(&project);

        let names = loop_score_names(&results);
        assert_eq!(
            names.len(),
            1,
            "dt={dt}: the two-stock model has exactly one feedback loop, found {names:?}"
        );

        let series = loop_score_series(&results, &names[0]);
        assert_isolated_loop_series(&series, 1.0, dt);
    }
}

/// Average absolute relative loop score for every loop in a
/// single-partition model that mixes a one-stock balancing loop and a
/// two-stock reinforcing loop, simulated at integration step `dt`.
///
/// Model (all rates are small positive constants, keeping the dynamics
/// near-linear so the "true" relative split barely moves with the
/// integration scheme):
///
/// * stock `a` -- inflow `in_a = b * r1`, outflow `loss_a = a * r3`
/// * stock `b` -- inflow `in_b = a * r2`
///
/// yielding two loops that share stock `a` (hence one cycle partition):
/// the two-stock reinforcing loop `a -> in_b -> b -> in_a -> a` and the
/// one-stock balancing loop `a -> loss_a -> a`. The averages are taken
/// over the back half of the run, well past the startup guard.
fn relative_scores_at_dt(dt: f64) -> HashMap<String, f64> {
    let project = TestProject::new("mixed_stock_counts")
        .with_sim_time(0.0, 6.0, dt)
        .stock("a", "100", &["in_a"], &["loss_a"], None)
        .stock("b", "100", &["in_b"], &[], None)
        .aux("r1", "0.02", None)
        .aux("r2", "0.02", None)
        .aux("r3", "0.05", None)
        .flow("in_a", "b * r1", None)
        .flow("in_b", "a * r2", None)
        .flow("loss_a", "a * r3", None)
        .build_datamodel();

    let (results, loop_partitions) = run_ltm(&project);
    let rel_scores = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);
    assert!(
        !rel_scores.is_empty(),
        "dt={dt}: expected relative loop scores for the mixed-stock-count model"
    );

    rel_scores
        .into_iter()
        .map(|(id, series)| {
            let tail = &series[series.len() / 2..];
            let avg = tail.iter().map(|v| v.abs()).sum::<f64>() / tail.len().max(1) as f64;
            (id, avg)
        })
        .collect()
}

/// The id of the loop with the largest average absolute relative score.
fn dominant_loop(scores: &HashMap<String, f64>) -> String {
    scores
        .iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).expect("relative scores are finite"))
        .map(|(id, _)| id.clone())
        .expect("at least one loop")
}

/// Finding 1, real-world impact: in a single cycle partition that mixes
/// a one-stock and a two-stock loop, the *relative* loop scores must be
/// near-invariant to `dt` for a near-linear model.
///
/// Normalization divides each loop score by the partition sum, so a
/// per-loop factor that is the *same* for every loop cancels. The
/// missing-`dt` error is *not* the same for every loop: it is
/// `(1/dt)^(stock count)`, so it survives normalization whenever a
/// partition mixes stock counts. Pre-fix, the relative split of this
/// model swung ~0.33 between `dt = 1.0` and `dt = 0.25` and the dominant
/// loop flipped; post-fix the drift is ~0.01 and the ordering is stable.
#[test]
fn relative_loop_scores_are_dt_invariant_for_mixed_stock_counts() {
    let at_full_dt = relative_scores_at_dt(1.0);
    let at_quarter_dt = relative_scores_at_dt(0.25);

    assert_eq!(
        at_full_dt.len(),
        2,
        "expected exactly two loops (one-stock balancing + two-stock reinforcing), got {:?}",
        at_full_dt.keys().collect::<Vec<_>>()
    );

    let mut ids_full: Vec<&String> = at_full_dt.keys().collect();
    let mut ids_quarter: Vec<&String> = at_quarter_dt.keys().collect();
    ids_full.sort();
    ids_quarter.sort();
    assert_eq!(
        ids_full, ids_quarter,
        "the same loop ids must be present at both dt values"
    );

    // Per-loop drift bound. Observed post-fix drift is ~0.01; the
    // pre-fix drift was ~0.33. 0.05 leaves comfortable margin above the
    // floating-point / near-linearity noise floor while staying far
    // below the regressed behavior.
    const MAX_DRIFT: f64 = 0.05;
    for (id, full_score) in &at_full_dt {
        let quarter_score = at_quarter_dt[id];
        let drift = (full_score - quarter_score).abs();
        assert!(
            drift < MAX_DRIFT,
            "loop {id}: relative score drifted {drift:.4} between dt=1.0 ({full_score:.4}) \
             and dt=0.25 ({quarter_score:.4}). A drift this large means the flow-to-stock \
             link score lost its `dt` factor, inflating each loop by (1/dt)^(stock count) \
             so the error survives partition normalization (LTM review Finding 1)."
        );
    }

    // The headline symptom of Finding 1: the dominant loop of the
    // partition flipping purely because the integration step changed.
    let dominant_full = dominant_loop(&at_full_dt);
    let dominant_quarter = dominant_loop(&at_quarter_dt);
    assert_eq!(
        dominant_full, dominant_quarter,
        "the dominant loop must not depend on dt: it is {dominant_full} at dt=1.0 but \
         {dominant_quarter} at dt=0.25"
    );
}
