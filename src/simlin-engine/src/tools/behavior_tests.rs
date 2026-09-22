// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The behavior modes of series the engine simulates, one model per mode, and
//! the invariances a mode must have.

use super::*;
use crate::datamodel::SimMethod;
use crate::test_common::TestProject;

/// Simulate `project` and return the saved times and `variable`'s series.
fn simulate(project: TestProject, variable: &str) -> (Vec<f64>, Vec<f64>) {
    let results = project.run_vm_expecting_success();
    (results["time"].clone(), results[variable].clone())
}

/// A second-order system settling toward 100: position `x`, velocity `v`, with
/// damping ratio `zeta` and natural frequency 0.5.
fn second_order(zeta: f64, stop: f64, method: SimMethod) -> TestProject {
    let omega: f64 = 0.5;
    TestProject::new("second_order")
        .with_sim_time(0.0, stop, 0.0625)
        .with_sim_method(method)
        .stock("x", "0", &["dx"], &[], None)
        .stock("v", "0", &["dv"], &[], None)
        .flow("dx", "v", None)
        .flow(
            "dv",
            &format!("{} * (100 - x) - {} * v", omega * omega, 2.0 * zeta * omega),
            None,
        )
}

/// A model whose `s` shows `kind`, and the direction or damping it must show.
fn fixture(kind: ModeKind) -> (TestProject, &'static str) {
    match kind {
        ModeKind::AtRest => (
            TestProject::new("rest")
                .with_sim_time(0.0, 20.0, 0.25)
                .stock("s", "100", &["f"], &[], None)
                .flow("f", "(100 - s) / 5", None),
            "s",
        ),
        ModeKind::Linear => (
            TestProject::new("linear")
                .with_sim_time(0.0, 20.0, 0.25)
                .stock("s", "0", &["f"], &[], None)
                .flow("f", "2", None),
            "s",
        ),
        ModeKind::Exponential => (
            TestProject::new("exponential")
                .with_sim_time(0.0, 40.0, 0.25)
                .stock("s", "1", &["f"], &[], None)
                .flow("f", "s * 0.1", None),
            "s",
        ),
        ModeKind::GoalSeeking => (
            TestProject::new("goal")
                .with_sim_time(0.0, 40.0, 0.25)
                .stock("s", "0", &["f"], &[], None)
                .flow("f", "(100 - s) / 5", None),
            "s",
        ),
        ModeKind::SShaped => (
            TestProject::new("logistic")
                .with_sim_time(0.0, 60.0, 0.25)
                .stock("s", "1", &["f"], &[], None)
                .flow("f", "0.3 * s * (1 - s / 1000)", None),
            "s",
        ),
        ModeKind::Overshoot => (second_order(0.7, 40.0, SimMethod::RungeKutta4), "x"),
        ModeKind::RiseAndFall => (
            // A population growing on a resource it depletes.
            TestProject::new("depletion")
                .with_sim_time(0.0, 100.0, 0.125)
                .stock("s", "10", &["births"], &["deaths"], None)
                .stock("resource", "1000", &[], &["use"], None)
                .flow("births", "s * 0.2 * resource / 1000", None)
                .flow("deaths", "s * 0.1", None)
                .flow("use", "s * 0.5", None),
            "s",
        ),
        ModeKind::FallAndRise => (
            // At rest until an outflow drains it for two time units, then it
            // refills toward where it was.
            TestProject::new("dip")
                .with_sim_time(0.0, 60.0, 0.125)
                .stock("s", "100", &["refill"], &["drain"], None)
                .flow("refill", "(100 - s) / 5", None)
                .flow("drain", "STEP(20, 10) - STEP(20, 12)", None),
            "s",
        ),
        ModeKind::Oscillation => (second_order(0.1, 80.0, SimMethod::RungeKutta4), "x"),
        ModeKind::Undefined => (
            TestProject::new("undefined")
                .with_sim_time(0.0, 10.0, 1.0)
                .aux("s", "1 / (TIME - 5)", None),
            "s",
        ),
        ModeKind::Other => (
            // A staircase: it rises, pauses, rises, pauses.
            TestProject::new("staircase")
                .with_sim_time(0.0, 30.0, 0.25)
                .stock("s", "0", &["f"], &[], None)
                .flow(
                    "f",
                    "STEP(1, 0) - STEP(1, 5) + STEP(1, 10) - STEP(1, 15) + STEP(1, 20) - STEP(1, 25)",
                    None,
                ),
            "s",
        ),
    }
}

#[test]
fn every_mode_is_the_mode_of_a_model_that_shows_it() {
    for kind in ModeKind::ALL {
        let (project, variable) = fixture(kind);
        let (times, values) = simulate(project, variable);
        let mode = classify(&times, &values);
        assert_eq!(mode.kind, kind, "{kind:?}: {mode:?}");
        let expected_direction = match kind {
            ModeKind::AtRest | ModeKind::Oscillation | ModeKind::Undefined => None,
            ModeKind::FallAndRise => Some(Direction::Falling),
            ModeKind::Linear
            | ModeKind::Exponential
            | ModeKind::GoalSeeking
            | ModeKind::SShaped
            | ModeKind::Overshoot
            | ModeKind::RiseAndFall
            | ModeKind::Other => Some(Direction::Rising),
        };
        assert_eq!(mode.direction, expected_direction, "{kind:?}: {mode:?}");
    }
}

#[test]
fn a_step_response_is_goal_seeking_from_the_step_and_a_slow_start_is_not_a_still_one() {
    let project = TestProject::new("step")
        .with_sim_time(0.0, 40.0, 0.25)
        .stock("s", "0", &["f"], &[], None)
        .flow("f", "(STEP(100, 10) - s) / 5", None);
    let (times, values) = simulate(project, "s");
    let mode = classify(&times, &values);
    assert_eq!(mode.kind, ModeKind::GoalSeeking, "{mode:?}");
    assert_eq!(mode.starts_at, Some(10.0), "{mode:?}");

    // Logistic growth begins slowly, but it begins at once.
    let (project, variable) = fixture(ModeKind::SShaped);
    let (times, values) = simulate(project, variable);
    assert_eq!(classify(&times, &values).starts_at, None);
}

#[test]
fn a_series_that_reaches_its_level_settles_there() {
    let (project, variable) = fixture(ModeKind::GoalSeeking);
    let (times, values) = simulate(project, variable);
    let mode = classify(&times, &values);
    // Within 1% of the final value after about 4.6 time constants of 5.
    let settles = mode.settles_at.expect("it settles");
    assert!((20.0..30.0).contains(&settles), "{settles}");
}

#[test]
fn an_oscillation_is_damped_sustained_or_growing_by_its_swings() {
    for (zeta, method, expected) in [
        (0.1, SimMethod::RungeKutta4, Damping::Damped),
        (0.0, SimMethod::RungeKutta4, Damping::Sustained),
        (-0.05, SimMethod::RungeKutta4, Damping::Growing),
    ] {
        let (times, values) = simulate(second_order(zeta, 80.0, method), "x");
        let mode = classify(&times, &values);
        assert_eq!(mode.kind, ModeKind::Oscillation, "zeta {zeta}: {mode:?}");
        assert_eq!(mode.damping, Some(expected), "zeta {zeta}: {mode:?}");
    }
}

/// With two turns there is one swing: a series that settles within half of
/// it is damped, and one the run cuts off is undetermined.
#[test]
fn an_oscillation_of_one_swing_is_damped_only_when_it_settles() {
    let times: Vec<f64> = (0..=160).map(|i| i as f64 * 0.1).collect();
    let cut_off: Vec<f64> = times.iter().map(|t| (t * 0.5).sin()).collect();
    let mode = classify(&times, &cut_off);
    assert_eq!(mode.kind, ModeKind::Oscillation, "{mode:?}");
    assert_eq!(mode.damping, Some(Damping::Undetermined), "{mode:?}");

    // Heavily damped: overshoot, undershoot, and settle.
    let settling: Vec<f64> = times
        .iter()
        .map(|t| 100.0 - 100.0 * (-0.35 * t).exp() * (0.9 * t).cos())
        .collect();
    let mode = classify(&times, &settling);
    assert_eq!(mode.kind, ModeKind::Oscillation, "{mode:?}");
    assert_eq!(mode.damping, Some(Damping::Damped), "{mode:?}");
}

#[test]
fn an_undefined_series_says_when_it_became_so() {
    let (project, variable) = fixture(ModeKind::Undefined);
    let (times, values) = simulate(project, variable);
    assert_eq!(classify(&times, &values).starts_at, Some(5.0));
}

#[test]
fn decay_toward_zero_is_goal_seeking_downward() {
    let project = TestProject::new("decay")
        .with_sim_time(0.0, 40.0, 0.25)
        .stock("s", "100", &[], &["f"], None)
        .flow("f", "s * 0.2", None);
    let (times, values) = simulate(project, "s");
    let mode = classify(&times, &values);
    assert_eq!(
        (mode.kind, mode.direction),
        (ModeKind::GoalSeeking, Some(Direction::Falling))
    );
}

/// A mode is a statement about shape: scaling or shifting a series, or
/// stretching its time axis, does not change it, and negating it flips its
/// direction and nothing else.
#[test]
fn a_mode_is_unchanged_by_scale_offset_and_time_units() {
    for kind in ModeKind::ALL {
        let (project, variable) = fixture(kind);
        let (times, values) = simulate(project, variable);
        let mode = classify(&times, &values);

        let affine: Vec<f64> = values.iter().map(|v| 3.0 * v + 250.0).collect();
        assert_eq!(classify(&times, &affine).kind, mode.kind, "{kind:?} scaled");

        let stretched: Vec<f64> = times.iter().map(|t| t * 12.0).collect();
        assert_eq!(
            classify(&stretched, &values).kind,
            mode.kind,
            "{kind:?} in other time units"
        );

        let negated: Vec<f64> = values.iter().map(|v| -v).collect();
        let flipped = classify(&times, &negated);
        let expected = match mode.kind {
            // A rise and fall upside down is a fall and rise.
            ModeKind::RiseAndFall => ModeKind::FallAndRise,
            ModeKind::FallAndRise => ModeKind::RiseAndFall,
            other => other,
        };
        assert_eq!(flipped.kind, expected, "{kind:?} negated");
        let opposite = mode.direction.map(|d| match d {
            Direction::Rising => Direction::Falling,
            Direction::Falling => Direction::Rising,
        });
        assert_eq!(flipped.direction, opposite, "{kind:?} negated");
    }
}

#[test]
fn noise_below_the_tolerance_makes_no_turning_point() {
    let (project, variable) = fixture(ModeKind::GoalSeeking);
    let (times, values) = simulate(project, variable);
    let (min, max) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let wiggle = 0.4 * NOISE_FRACTION * (max - min);
    let noisy: Vec<f64> = values
        .iter()
        .enumerate()
        .map(|(i, v)| v + wiggle * (i as f64 * 1.7).sin())
        .collect();
    let shape = shape(&times, &noisy);
    assert!(shape.turns.is_empty(), "{:?}", shape.turns);
    assert_eq!(shape.mode.kind, ModeKind::GoalSeeking);
}

#[test]
fn a_series_too_short_to_have_a_shape_is_at_rest_or_linear() {
    assert_eq!(classify(&[], &[]).kind, ModeKind::AtRest);
    assert_eq!(classify(&[0.0], &[5.0]).kind, ModeKind::AtRest);
    let mode = classify(&[0.0, 1.0], &[5.0, 3.0]);
    assert_eq!(
        (mode.kind, mode.direction),
        (ModeKind::Linear, Some(Direction::Falling))
    );
}

#[test]
fn the_turning_points_are_where_the_series_turns() {
    let (times, values) = simulate(second_order(0.1, 80.0, SimMethod::RungeKutta4), "x");
    let shape = shape(&times, &values);
    assert!(shape.turns.len() >= 4, "{:?}", shape.turns);
    for pair in shape.turns.windows(2) {
        let (a, b) = (values[pair[0]], values[pair[1]]);
        assert!(
            (a - 100.0).signum() != (b - 100.0).signum(),
            "turns alternate about the goal"
        );
    }
    // The first turn is the first peak, the overshoot past 100.
    assert!(values[shape.turns[0]] > 100.0);
}
