// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The clock is a frozen input of a ceteris-paribus partial (GH #1016).
//!
//! The 2020 paper's partial `Delta_x z = f(x_t, y_{t-1}) - z_{t-1}` reads
//! every input but the isolated one at the previous step, and the clock is an
//! input like any other: `TIME` and the time-dependent builtins (`STEP`,
//! `RAMP`, `PULSE`) are read at the previous step inside the partial. What
//! that buys, in order:
//!
//! * a source with no influence on its target scores 0 even while an
//!   exogenous forcing moves the target
//!   (`a_source_with_no_influence_scores_zero_under_a_step_forcing`);
//! * an exogenous forcing on an isolated loop is NOT credited to the loop
//!   (`an_exogenous_ramp_takes_its_share_of_an_isolated_loop`);
//! * a target that reads the clock directly scores the paper's partial,
//!   `x_t * TIME_{t-1} - z_{t-1}`
//!   (`a_target_reading_time_scores_the_changed_first_partial`);
//! * a reducer body reading the clock keeps the `PREVIOUS(agg)` anchor, so a
//!   row that is never the argmin scores 0 (GH #763,
//!   `a_time_bearing_reducer_body_keeps_the_frozen_argmin_at_zero`).
//!
//! The stated residual: a time-dependent call whose arguments read the live
//! source keeps its clock live for that call
//! (`a_time_call_reading_the_live_source_keeps_its_clock_live`). And the
//! boundary of the freeze: a clock read inside a frozen dependency's subscript
//! index is lagged exactly once, by the enclosing freeze
//! (`a_clock_read_in_a_frozen_deps_index_is_lagged_once`).

use simlin_engine::datamodel::Project;
use simlin_engine::test_common::TestProject;

use crate::test_helpers::{ltm_run, ltm_series};

/// `$⁚ltm⁚link_score⁚{from}→{to}`.
fn link_score(from: &str, to: &str) -> String {
    format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}")
}

/// `$⁚ltm⁚loop_score⁚{id}`.
fn loop_score(id: &str) -> String {
    format!("$\u{205A}ltm\u{205A}loop_score\u{205A}{id}")
}

fn assert_series_eq(got: &[f64], want: &[f64], tol: f64, what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    for (t, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() < tol,
            "{what} at step {t}: got {g}, want {w} (got {got:?}, want {want:?})"
        );
    }
}

/// `x = 5 + STEP(2, 2) + 0 * s` on the structural loop `s -> x -> s`: `s`
/// has no influence on `x`, and the step forcing moves `x` at t = 2.
fn inert_source_under_step() -> Project {
    TestProject::new("inert_step")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("s", "0", &["x"], &[], None)
        .flow("x", "5 + STEP(2, 2) + 0 * s", None)
        .build_datamodel()
}

#[test]
fn a_source_with_no_influence_scores_zero_under_a_step_forcing() {
    let run = ltm_run(&inert_source_under_step(), false);
    let x = ltm_series(&run.results, "x", 0);
    // The forcing is real: x jumps from 5 to 7 at t = 2.
    assert_series_eq(&x, &[5.0, 5.0, 7.0, 7.0, 7.0], 1e-12, "x");
    // s changes at every step (its inflow is never 0), so the zero below is
    // the partial's, not the guard's Δsource = 0 arm.
    let s = ltm_series(&run.results, "s", 0);
    assert!(s.windows(2).all(|w| w[1] > w[0]), "s must move: {s:?}");
    let score = ltm_series(&run.results, &link_score("s", "x"), 0);
    assert_series_eq(&score, &[0.0; 5], 1e-12, "LS(s -> x)");
}

/// The isolated loop `s -> f -> s` with `f = 0.1 * s + RAMP(1, 0)`.
fn ramped_isolated_loop() -> Project {
    TestProject::new("ramped_loop")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("s", "100", &["f"], &[], None)
        .flow("f", "0.1 * s + RAMP(1, 0)", None)
        .build_datamodel()
}

#[test]
fn an_exogenous_ramp_takes_its_share_of_an_isolated_loop() {
    let run = ltm_run(&ramped_isolated_loop(), false);
    let s = ltm_series(&run.results, "s", 0);
    let f = ltm_series(&run.results, "f", 0);
    let s_to_f = ltm_series(&run.results, &link_score("s", "f"), 0);
    let f_to_s = ltm_series(&run.results, &link_score("f", "s"), 0);
    let loop_id = run.loop_through("f").to_string();
    let loop_series = ltm_series(&run.results, &loop_score(&loop_id), 0);

    // Hand calculation: the partial holds the ramp at its previous value, so
    // the loop's share of Δf is 0.1 * Δs and the ramp's slope (1 per time
    // unit, dt = 1) is the rest.
    let mut want = vec![0.0];
    for t in 1..s.len() {
        let ds = s[t] - s[t - 1];
        let df = f[t] - f[t - 1];
        want.push(0.1 * ds / df.abs());
    }
    assert_series_eq(&s_to_f, &want, 1e-12, "LS(s -> f)");
    // The pinned values: 0.500, 0.545, 0.587, 0.624, 0.658 at t = 1..5.
    assert_series_eq(
        &s_to_f,
        &[0.0, 0.500, 0.545, 0.587, 0.624, 0.658],
        5e-4,
        "LS(s -> f) rounded",
    );
    // The single inflow scores 1 into its stock, so the loop score IS the
    // stock-to-flow share.
    assert_series_eq(
        &f_to_s,
        &[0.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        1e-12,
        "LS(f -> s)",
    );
    assert_series_eq(&loop_series, &want, 1e-12, "loop score");
}

/// `z = x * TIME` on the loop `s -> x -> z -> s`.
fn target_reading_time() -> Project {
    TestProject::new("time_target")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("s", "10", &["z"], &[], None)
        .aux("x", "2 + 0.01 * s", None)
        .flow("z", "x * TIME", None)
        .build_datamodel()
}

#[test]
fn a_target_reading_time_scores_the_changed_first_partial() {
    let run = ltm_run(&target_reading_time(), false);
    let x = ltm_series(&run.results, "x", 0);
    let z = ltm_series(&run.results, "z", 0);
    let time = ltm_series(&run.results, "time", 0);
    let score = ltm_series(&run.results, &link_score("x", "z"), 0);

    // The paper's changed-first numerator with the clock frozen:
    //   N = x_t * TIME_{t-1} - z_{t-1},
    // scored as SAFEDIV(N, |Δz|, 0) * SIGN(Δx). With the clock live the
    // numerator would be x_t * TIME_t - z_{t-1} = Δz, a score of 1.
    let mut want = vec![0.0];
    let mut live_clock_would_give = vec![0.0];
    for t in 1..z.len() {
        let dz = z[t] - z[t - 1];
        let dx = x[t] - x[t - 1];
        let n = x[t] * time[t - 1] - z[t - 1];
        want.push(n / dz.abs() * dx.signum());
        live_clock_would_give.push((x[t] * time[t] - z[t - 1]) / dz.abs() * dx.signum());
    }
    assert_series_eq(&score, &want, 1e-12, "LS(x -> z)");
    // The two conventions are distinguishable on this model at every step
    // after the first: the clock-live numerator is the whole of Δz.
    for t in 2..z.len() {
        assert!(
            (live_clock_would_give[t] - 1.0).abs() < 1e-12 && (want[t] - 1.0).abs() > 0.1,
            "step {t}: frozen-clock {} vs live-clock {}",
            want[t],
            live_clock_would_give[t]
        );
    }
}

/// `grow = 1 + MIN(pop[*] * TIME)` (GH #763's repro) on a loop through both
/// rows: `south` starts larger and grows at the same rate, so it is never
/// the argmin.
fn time_bearing_min_body() -> Project {
    TestProject::new("min_time_body")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("Region", &["north", "south"])
        .array_stock("pop[Region]", "10", &["growth"], &[], None)
        .array_flow("growth[Region]", "pop * 0.1 * grow", None)
        .aux("grow", "1 + MIN(pop[*] * TIME)", None)
        .build_datamodel()
}

#[test]
fn a_time_bearing_reducer_body_keeps_the_frozen_argmin_at_zero() {
    let mut project = time_bearing_min_body();
    // south starts at 20: the same equations, a larger start, never the argmin.
    for var in &mut project.models[0].variables {
        if let simlin_engine::datamodel::Variable::Stock(stock) = var
            && stock.ident == "pop"
        {
            stock.equation = simlin_engine::datamodel::Equation::Arrayed(
                vec!["Region".to_string()],
                vec![
                    ("north".to_string(), "10".to_string(), None, None),
                    ("south".to_string(), "20".to_string(), None, None),
                ],
                None,
                false,
            );
        }
    }
    let run = ltm_run(&project, false);
    // The hoisted MIN is `$⁚ltm⁚agg⁚0`, scored per source element.
    let agg = "$\u{205A}ltm\u{205A}agg\u{205A}0";
    let north = ltm_series(&run.results, &link_score("pop[north]", agg), 0);
    let south = ltm_series(&run.results, &link_score("pop[south]", agg), 0);
    // At t = 1 the frozen clock reads TIME = 0, so every term is 0 and the
    // whole change of MIN is the clock's: north scores from t = 2.
    assert!(
        north[2..].iter().all(|v| v.abs() > 1e-9),
        "north is the argmin at every step and carries the score: {north:?}"
    );
    assert_series_eq(&south, &[0.0; 5], 1e-12, "LS(pop[south] -> MIN)");
}

/// `z = x + arr[TIME]` with `arr = [1, 2, 4, 8, 16]` on the loop
/// `s -> x -> z -> s`, run from t = 1 so `TIME` indexes `arr` directly.
fn frozen_dep_indexed_by_time() -> Project {
    let mut project = TestProject::new("time_index")
        .with_sim_time(1.0, 5.0, 1.0)
        .named_dimension("Slot", &["a1", "a2", "a3", "a4", "a5"])
        .array_aux("arr[Slot]", "1")
        .stock("s", "10", &["z"], &[], None)
        .aux("x", "0.1 * s", None)
        .flow("z", "x + arr[TIME]", None)
        .build_datamodel();
    for var in &mut project.models[0].variables {
        if let simlin_engine::datamodel::Variable::Aux(aux) = var
            && aux.ident == "arr"
        {
            aux.equation = simlin_engine::datamodel::Equation::Arrayed(
                vec!["Slot".to_string()],
                ["1", "2", "4", "8", "16"]
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (format!("a{}", i + 1), v.to_string(), None, None))
                    .collect(),
                None,
                false,
            );
        }
    }
    project
}

/// `arr` is a frozen dependency of `z` for the `x -> z` link, so the partial
/// reads `PREVIOUS(arr[TIME])`: one capture evaluates `arr[TIME]` at `t` and
/// `PREVIOUS` reads it a step back, index included, which is the paper's
/// `arr_{t-1}[TIME_{t-1}]`. Lagging the clock inside as well would read
/// `arr_{t-1}[TIME_{t-2}]` and flip the score's sign from the second scored
/// step.
#[test]
fn a_clock_read_in_a_frozen_deps_index_is_lagged_once() {
    let run = ltm_run(&frozen_dep_indexed_by_time(), false);
    let x = ltm_series(&run.results, "x", 0);
    let z = ltm_series(&run.results, "z", 0);
    let score = ltm_series(&run.results, &link_score("x", "z"), 0);
    let arr = [1.0, 2.0, 4.0, 8.0, 16.0];
    // The partial is x_t + arr[TIME_{t-1}] and the anchor z_{t-1} is
    // x_{t-1} + arr[TIME_{t-1}], so the numerator is Δx.
    let mut want = vec![0.0];
    let mut double_lag_would_give = vec![0.0];
    for t in 1..z.len() {
        let dx = x[t] - x[t - 1];
        let dz = z[t] - z[t - 1];
        want.push(dx / dz.abs() * dx.signum());
        // TIME at step t is t + 1 (the run starts at 1); arr[TIME_{t-2}] is
        // arr[t - 1] 1-based, i.e. index t - 2, undefined at the first step.
        if t >= 2 {
            let n = dx + arr[t - 2] - arr[t - 1];
            double_lag_would_give.push(n / dz.abs() * dx.signum());
        } else {
            double_lag_would_give.push(want[t]);
        }
    }
    assert_series_eq(&score, &want, 1e-12, "LS(x -> z)");
    assert_series_eq(
        &score,
        &[0.0, 0.1667, 0.1379, 0.1213, 0.1118],
        5e-5,
        "LS(x -> z) rounded",
    );
    // The double-lag form is a different, sign-flipped number from t = 3.
    for t in 2..z.len() {
        assert!(
            double_lag_would_give[t] < 0.0 && want[t] > 0.0,
            "step {t}: once-lagged {} vs twice-lagged {}",
            want[t],
            double_lag_would_give[t]
        );
    }
}

/// `f = STEP(0.1 * s, 2)` on the isolated loop: the call reads the live
/// source, so its clock stays live (the stated residual), and the step of the
/// forcing is credited to the loop.
fn time_call_reading_the_source() -> Project {
    TestProject::new("step_of_source")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("s", "100", &["f"], &[], None)
        .flow("f", "1 + STEP(0.1 * s, 2)", None)
        .build_datamodel()
}

#[test]
fn a_time_call_reading_the_live_source_keeps_its_clock_live() {
    let run = ltm_run(&time_call_reading_the_source(), false);
    let f = ltm_series(&run.results, "f", 0);
    let s_to_f = ltm_series(&run.results, &link_score("s", "f"), 0);
    // f is 1 until the step fires at t = 2, then 1 + 0.1 * s.
    assert!((f[1] - 1.0).abs() < 1e-12 && f[2] > 10.0, "f: {f:?}");
    // At t = 2 the partial is 1 + STEP(0.1 * s_t, 2) with the clock live, so
    // the whole jump is attributed to s: the score is 1 (the residual). From
    // t = 3 the step is on in both the partial and the anchor and the score
    // is the ordinary 0.1 * Δs / |Δf| = 1.
    assert_series_eq(&s_to_f, &[0.0, 0.0, 1.0, 1.0, 1.0], 1e-12, "LS(s -> f)");
}
