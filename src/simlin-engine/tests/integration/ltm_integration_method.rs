// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM under every integration method and every save step.
//!
//! A link score is a ratio of integration-step (dt) deltas, reported at the
//! saved steps: `PREVIOUS` reads the state the previous dt step ended in
//! (the VM snapshots `prev_values` on every dt iteration, before the
//! save/advance logic decides whether the row is recorded), and under
//! RK2/RK4 the VM re-evaluates the flows at the restored end-of-step state
//! before it snapshots that state (the RK stages' trial-point evaluations
//! are overwritten). This is the 2020 paper's form (Schoenberg, Davidsen and
//! Eberlein, section 6.1: the scores are "computed at each dt"); the paper
//! puts Runge-Kutta compatibility as "in principle", and the dt-step ratio
//! over the method's own trajectory is the form it takes here.
//!
//! Two pins:
//!
//! * A model whose flows are proportional to its stock scores identically
//!   under Euler, RK2 and RK4 at every saved step -- the scores are ratios of
//!   flow deltas, and those cancel the stock's step -- even though the stock
//!   trajectories, and with them the flows and the net-flow auxes, differ.
//!   That equality is a property of PROPORTIONAL flows, not of the method:
//!   in general a score follows the method's trajectory, and the nonlinear
//!   model below scores differently under Euler and RK4.
//! * A saved step that spans two dt steps reports the ratio over the LAST dt
//!   step, the same number the dt-resolution run reports at that time; it
//!   does not re-difference the flows over the saved interval.

use simlin_engine::datamodel::{Project, SimMethod};
use simlin_engine::ltm_post;
use simlin_engine::test_common::TestProject;

use crate::test_helpers::{ltm_run, ltm_series};

/// The isolated reinforcing loop `s -> births -> s`.
fn isolated_loop(method: SimMethod) -> Project {
    TestProject::new("iso_method")
        .with_sim_time(0.0, 8.0, 1.0)
        .with_sim_method(method)
        .stock("s", "100", &["births"], &[], None)
        .flow("births", "s * 0.1", None)
        .build_datamodel()
}

/// Births `0.1 * a` against deaths `a / 20`: the 67/33 split.
fn births_deaths(method: SimMethod) -> Project {
    TestProject::new("bd_method")
        .with_sim_time(0.0, 8.0, 1.0)
        .with_sim_method(method)
        .stock("a", "100", &["births"], &["deaths"], None)
        .flow("births", "0.1 * a", None)
        .flow("deaths", "a / 20", None)
        .build_datamodel()
}

/// Births `0.1 * s` against deaths `0.02 * s ^ 1.3`: the outflow is not
/// proportional to the stock, so a score depends on which two points of the
/// trajectory it differences, and the two flows nearly balance, so that
/// dependence is large.
fn nonlinear_deaths(method: SimMethod, dt: f64, save_step: f64) -> Project {
    TestProject::new("nonlinear_method")
        .with_sim_time(0.0, 4.0, dt)
        .with_save_step(save_step)
        .with_sim_method(method)
        .stock("s", "100", &["births"], &["deaths"], None)
        .flow("births", "0.1 * s", None)
        .flow("deaths", "0.02 * s ^ 1.3", None)
        .build_datamodel()
}

/// `$⁚ltm⁚link_score⁚{from}→{to}`.
fn link_score(from: &str, to: &str) -> String {
    format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}")
}

fn max_abs_diff(a: &[f64], b: &[f64]) -> f64 {
    assert_eq!(a.len(), b.len(), "series lengths differ");
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0, f64::max)
}

/// One model of the parity check: its label, its stock, and its builder.
type ModelCase = (&'static str, &'static str, fn(SimMethod) -> Project);

#[test]
fn every_integration_method_scores_a_proportional_flow_identically() {
    let models: [ModelCase; 2] = [
        ("isolated loop", "s", isolated_loop),
        ("births/deaths", "a", births_deaths),
    ];
    for (label, stock, build) in models {
        let euler = ltm_run(&build(SimMethod::Euler), false);
        let euler_rel = ltm_post::compute_rel_loop_scores(&euler.results, &euler.loop_partitions);
        let mut ltm_keys: Vec<String> = euler
            .results
            .offsets
            .keys()
            .map(|k| k.as_str().to_string())
            .filter(|k| k.starts_with("$\u{205A}ltm\u{205A}"))
            .collect();
        ltm_keys.sort();
        assert!(
            ltm_keys.iter().any(|k| k.contains("loop_score")),
            "{label}: the Euler run carries LTM scores"
        );

        for method in [SimMethod::RungeKutta2, SimMethod::RungeKutta4] {
            let rk = ltm_run(&build(method), false);
            // The trajectories genuinely differ, so the equal scores below
            // are not the same numbers scored twice.
            let stock_gap = max_abs_diff(
                &ltm_series(&euler.results, stock, 0),
                &ltm_series(&rk.results, stock, 0),
            );
            assert!(
                stock_gap > 1e-3,
                "{label} under {method:?}: the stock trajectory should differ from Euler's, \
                 max gap {stock_gap:e}"
            );
            let mut rk_keys: Vec<String> = rk
                .results
                .offsets
                .keys()
                .map(|k| k.as_str().to_string())
                .filter(|k| k.starts_with("$\u{205A}ltm\u{205A}"))
                .collect();
            rk_keys.sort();
            assert_eq!(
                rk_keys, ltm_keys,
                "{label} under {method:?}: the same LTM variables"
            );
            // The SCORES agree; the net-flow aux is a flow value and follows
            // the trajectory like the flows do.
            let mut scores_compared = 0;
            for key in &ltm_keys {
                let is_score = key.contains("\u{205A}link_score\u{205A}")
                    || key.contains("\u{205A}loop_score\u{205A}");
                if !is_score {
                    continue;
                }
                scores_compared += 1;
                let diff = max_abs_diff(
                    &ltm_series(&euler.results, key, 0),
                    &ltm_series(&rk.results, key, 0),
                );
                assert!(
                    diff < 1e-9,
                    "{label} under {method:?}: {key} differs from Euler by {diff:e}"
                );
            }
            assert!(
                scores_compared >= 3,
                "{label}: link and loop scores were compared"
            );
            let rk_rel = ltm_post::compute_rel_loop_scores(&rk.results, &rk.loop_partitions);
            assert_eq!(rk_rel.len(), euler_rel.len());
            for (id, series) in &euler_rel {
                let diff = max_abs_diff(series, &rk_rel[id]);
                assert!(
                    diff < 1e-9,
                    "{label} under {method:?}: relative score of {id} differs from Euler by {diff:e}"
                );
            }
        }
    }
}

/// The boundary of the parity above: with a nonlinear outflow the scores
/// follow the method's trajectory, so Euler and RK4 disagree.
#[test]
fn a_nonlinear_flow_scores_differently_under_euler_and_rk4() {
    let euler = ltm_run(&nonlinear_deaths(SimMethod::Euler, 0.5, 0.5), false);
    let rk4 = ltm_run(&nonlinear_deaths(SimMethod::RungeKutta4, 0.5, 0.5), false);
    let key = link_score("deaths", "s");
    let euler_score = ltm_series(&euler.results, &key, 0);
    let rk4_score = ltm_series(&rk4.results, &key, 0);
    // t = 2 is index 4 at save_step 0.5.
    let gap = (euler_score[4] - rk4_score[4]).abs();
    assert!(
        gap > 1e-3,
        "deaths -> s at t = 2: Euler {} vs RK4 {} should differ",
        euler_score[4],
        rk4_score[4]
    );
}

/// With save_step = 2 * dt the recorded score at a saved time is the ratio
/// over the last dt step ending there -- the number the dt-resolution run
/// records at that time -- and not the ratio over the whole saved interval.
/// The two forms differ by more than a unit of score on this model, so a
/// change to when `prev_values` is snapshotted would fail here.
#[test]
fn a_saved_step_spanning_two_dt_steps_reports_the_last_dt_step_ratio() {
    let key = link_score("deaths", "s");
    let net = "$\u{205A}ltm\u{205A}net\u{205A}s";
    // The Euler ratio at t = 2 over [1.5, 2] and, for the record, the same
    // ratio under RK4: the two methods' trajectories differ, so the scores
    // do too (the parity boundary).
    let pinned_at_t2 = [
        (SimMethod::Euler, -22.744),
        (SimMethod::RungeKutta4, -22.749),
    ];
    for (method, pinned) in pinned_at_t2 {
        let fine = ltm_run(&nonlinear_deaths(method, 0.5, 0.5), false);
        let coarse = ltm_run(&nonlinear_deaths(method, 0.5, 1.0), false);
        let fine_score = ltm_series(&fine.results, &key, 0);
        let coarse_score = ltm_series(&coarse.results, &key, 0);
        let fine_deaths = ltm_series(&fine.results, "deaths", 0);
        let fine_net = ltm_series(&fine.results, net, 0);
        assert_eq!(fine_score.len(), 9, "{method:?}: 0..=4 at save_step 0.5");
        assert_eq!(coarse_score.len(), 5, "{method:?}: 0..=4 at save_step 1");

        // Same dt, so the same trajectory: the coarse rows are the fine rows
        // at the saved times, scores included.
        for t in 1..=4usize {
            let fine_at_t = fine_score[2 * t];
            let coarse_at_t = coarse_score[t];
            assert!(
                (fine_at_t - coarse_at_t).abs() < 1e-12,
                "{method:?} at t = {t}: coarse {coarse_at_t} vs fine {fine_at_t}"
            );
            // The ratio over the whole saved interval [t - 1, t] is a
            // different number.
            let d_deaths = fine_deaths[2 * t] - fine_deaths[2 * t - 2];
            let d_net = fine_net[2 * t] - fine_net[2 * t - 2];
            let over_saved_interval = -(d_deaths / d_net).abs();
            assert!(
                (over_saved_interval - coarse_at_t).abs() > 0.1,
                "{method:?} at t = {t}: the saved-interval ratio {over_saved_interval} \
                 should be distinguishable from the recorded {coarse_at_t}"
            );
        }
        assert!(
            (coarse_score[2] - pinned).abs() < 5e-4,
            "{method:?} at t = 2: deaths -> s recorded {} (pinned {pinned})",
            coarse_score[2]
        );
    }
}
