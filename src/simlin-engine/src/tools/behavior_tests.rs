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

/// With two turns there is one swing, and the movement after it: one that
/// ends in a reversal is a second swing, under a movement, compared with the
/// first; one the run cuts off is damped when the series settles within half
/// of the swing, and undetermined when it does not.
#[test]
fn an_oscillation_of_one_swing_is_judged_by_the_movement_after_it() {
    let legs = |levels: &[f64]| -> Vec<f64> {
        let mut values = vec![levels[0]];
        for pair in levels.windows(2) {
            values.extend((1..=20).map(|i| pair[0] + (pair[1] - pair[0]) * f64::from(i) / 20.0));
        }
        values
    };
    // (levels, whether it settles, damping): a swing, then a movement that
    // ends where the series turns back. After a swing from 1 to 0.3, a
    // second of 0.3 (to 0.6, back 0.07, part-way up again) is damped. After
    // one from 1 to 0, a second of 0.95 is sustained, whether the reversal is
    // under a tenth of the range (back 0.07), the start of a movement the run
    // cut off (back 0.2: the swing is to the furthest it went), or completes
    // where the series has settled (back 0.015 to where it stays).
    for (levels, settles, expected) in [
        (vec![0.0, 1.0, 0.3, 0.6, 0.53, 0.58], false, Damping::Damped),
        (vec![0.0, 1.0, 0.0, 0.95, 0.88], false, Damping::Sustained),
        (vec![0.0, 1.0, 0.0, 0.95, 0.75], false, Damping::Sustained),
        (vec![0.0, 1.0, 0.0, 0.95, 0.935], true, Damping::Sustained),
    ] {
        let values = legs(&levels);
        let found = shape(&evenly(values.len()), &values);
        assert_eq!(found.mode.kind, ModeKind::Oscillation, "{levels:?}");
        assert_eq!(found.mode.settles_at.is_some(), settles, "{levels:?}");
        assert_eq!(found.mode.damping, Some(expected), "{levels:?}");
    }
    // A movement that does not end where the series turns back is the one
    // the run cut off, and is no second swing: back from its furthest by no
    // more than the noise, or going on past a wiggle at its start.
    for levels in [
        vec![0.0, 1.0, 0.0, 0.95, 0.945],
        vec![0.0, 1.0, 0.0, 0.03, 0.015, 0.9],
    ] {
        let values = legs(&levels);
        let mode = classify(&evenly(values.len()), &values);
        assert_eq!(mode.kind, ModeKind::Oscillation, "{levels:?}");
        assert_eq!(mode.damping, Some(Damping::Undetermined), "{levels:?}");
    }

    let times: Vec<f64> = (0..=160).map(|i| i as f64 * 0.1).collect();
    let cut_off: Vec<f64> = times.iter().map(|t| (t * 0.5).sin()).collect();
    let mode = classify(&times, &cut_off);
    assert_eq!(mode.kind, ModeKind::Oscillation, "{mode:?}");
    assert_eq!(mode.damping, Some(Damping::Undetermined), "{mode:?}");

    // Cut off a third of a swing past its second turn: it has come back
    // less than half a swing, as a damped one would have, but it has not
    // settled there.
    let mode = classify(&times[..121], &cut_off[..121]);
    assert_eq!(mode.kind, ModeKind::Oscillation, "{mode:?}");
    assert_eq!(mode.damping, Some(Damping::Undetermined), "{mode:?}");

    // One whole swing, from 1 to -1, and a movement after it already larger
    // than the swing (or not): growing (or undetermined).
    for (end, damping) in [(2.0, Damping::Growing), (1.2, Damping::Undetermined)] {
        let levels = [0.0, 1.0, -1.0, end];
        let values: Vec<f64> = levels
            .windows(2)
            .flat_map(|pair| {
                (0..20).map(move |i| pair[0] + (pair[1] - pair[0]) * f64::from(i) / 20.0)
            })
            .chain([end])
            .collect();
        let mode = classify(&times[..values.len()], &values);
        assert_eq!(mode.kind, ModeKind::Oscillation, "{mode:?}");
        assert_eq!(mode.damping, Some(damping), "ending at {end}: {mode:?}");
    }
    // The movement after the swing is measured from the swing's last turn,
    // not from a wiggle before the swing: from -1 to 2 is over 1.25 swings.
    let values = legs(&[0.0, 0.05, 0.0, 1.0, -1.0, 2.0]);
    let mode = classify(&evenly(values.len()), &values);
    assert_eq!(mode.kind, ModeKind::Oscillation, "{mode:?}");
    assert_eq!(mode.damping, Some(Damping::Growing), "{mode:?}");

    // Damped: overshoot, undershoot, and settle. The swing back from the
    // undershoot is 17% of the range, a movement, so it is one whole swing
    // that then settles.
    let times: Vec<f64> = (0..=300).map(|i| f64::from(i) * 0.1).collect();
    let response = |decay: f64| -> Vec<f64> {
        times
            .iter()
            .map(|t| 100.0 - 100.0 * (-decay * t).exp() * (0.9 * t).cos())
            .collect()
    };
    let mode = classify(&times, &response(0.25));
    assert_eq!(mode.kind, ModeKind::Oscillation, "{mode:?}");
    assert_eq!(mode.damping, Some(Damping::Damped), "{mode:?}");
    assert!(mode.settles_at.is_some());

    // More heavily damped, the swing back is 9% of the range, under a
    // movement: the response is an overshoot (by 30%) that settles, and its
    // undershoot is among its turning points.
    let found = shape(&times, &response(0.35));
    assert_eq!(found.mode.kind, ModeKind::Overshoot, "{:?}", found.mode);
    assert!(found.mode.settles_at.is_some());
    assert!(found.turns.len() >= 2, "{:?}", found.turns);
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

// ── Generated curves ──────────────────────────────────────────────────
//
// The classifier against curves written down analytically, so the expected
// mode is the curve's by construction: every family below across its
// parameters, sampling densities, scales, offsets, directions and horizons,
// clean and with noise under the tolerance.

/// A deterministic generator, so a sweep is the same every run.
struct Lcg(u64);

impl Lcg {
    fn unit(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }

    fn between(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.unit() * items.len() as f64) as usize]
    }
}

/// A family of curves with one mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Family {
    Linear,
    ExponentialGrowth,
    GoalSeeking,
    Logistic,
    Overshoot,
    RiseAndFall,
    SustainedOscillation,
    DampedOscillation,
    GrowingOscillation,
    /// Growth that small dips disturb early on, as a historical record
    /// before a projection does.
    DisturbedGrowth,
    /// A rise and a collapse, with a slight recovery after it.
    CollapseAndUptick,
}

impl Family {
    const ALL: [Family; 11] = [
        Family::Linear,
        Family::ExponentialGrowth,
        Family::GoalSeeking,
        Family::Logistic,
        Family::Overshoot,
        Family::RiseAndFall,
        Family::SustainedOscillation,
        Family::DampedOscillation,
        Family::GrowingOscillation,
        Family::DisturbedGrowth,
        Family::CollapseAndUptick,
    ];

    /// The mode a rising curve of the family has.
    fn mode(self) -> (ModeKind, Option<Damping>) {
        match self {
            Family::Linear => (ModeKind::Linear, None),
            Family::ExponentialGrowth | Family::DisturbedGrowth => (ModeKind::Exponential, None),
            Family::GoalSeeking => (ModeKind::GoalSeeking, None),
            Family::Logistic => (ModeKind::SShaped, None),
            Family::Overshoot => (ModeKind::Overshoot, None),
            Family::RiseAndFall | Family::CollapseAndUptick => (ModeKind::RiseAndFall, None),
            Family::SustainedOscillation => (ModeKind::Oscillation, Some(Damping::Sustained)),
            Family::DampedOscillation => (ModeKind::Oscillation, Some(Damping::Damped)),
            Family::GrowingOscillation => (ModeKind::Oscillation, Some(Damping::Growing)),
        }
    }

    /// A curve of the family on the unit interval, its parameters drawn from
    /// ranges inside which the family's name is not in question; the fewest
    /// samples that resolve it; and its parameters, for a failure's message.
    fn curve(self, rng: &mut Lcg) -> (Box<dyn Fn(f64) -> f64>, usize, String) {
        use std::f64::consts::TAU;
        match self {
            Family::Linear => (Box::new(|x| x), 5, String::new()),
            Family::ExponentialGrowth => {
                // Grows by a factor of e^g over the run: from 2.7 to 3000.
                let g = rng.between(1.0, 8.0);
                (Box::new(move |x| (g * x).exp()), 5, format!("g={g:.2}"))
            }
            Family::GoalSeeking => {
                // The run is 1.5 to 10 time constants long.
                let k = rng.between(1.5, 10.0);
                (
                    Box::new(move |x| 1.0 - (-k * x).exp()),
                    8,
                    format!("horizon/tau={k:.2}"),
                )
            }
            Family::Logistic => {
                // Its turn between two fifths and three fifths of the way,
                // and steep enough that it has both started slow and ended
                // slow: an S that turns earlier, later or more gently shades
                // into goal seeking or exponential growth.
                let mid = rng.between(0.4, 0.6);
                let r = rng.between(12.0, 30.0);
                (
                    Box::new(move |x| 1.0 / (1.0 + (-r * (x - mid)).exp())),
                    12,
                    format!("mid={mid:.2} r={r:.1}"),
                )
            }
            Family::Overshoot => {
                // A second-order step response whose overshoot is over 2% of
                // the step and whose undershoot is under half a percent.
                let zeta = rng.between(0.68, 0.77);
                let periods = rng.between(1.5, 4.0);
                let wd = TAU * periods;
                let wn = wd / (1.0 - zeta * zeta).sqrt();
                (
                    Box::new(move |x| {
                        1.0 - (-zeta * wn * x).exp()
                            * ((wd * x).cos() + zeta / (1.0 - zeta * zeta).sqrt() * (wd * x).sin())
                    }),
                    25,
                    format!("zeta={zeta:.2} periods={periods:.1}"),
                )
            }
            Family::RiseAndFall => {
                // x e^{-x/tau}: peaks at tau, and has fallen most of the way
                // back by the end.
                let tau = rng.between(0.08, 0.2);
                (
                    Box::new(move |x| x * (-x / tau).exp()),
                    25,
                    format!("tau={tau:.2}"),
                )
            }
            Family::SustainedOscillation
            | Family::DampedOscillation
            | Family::GrowingOscillation => {
                let cycles = rng.between(2.5, 8.0);
                let phase = rng.between(0.0, TAU);
                // The envelope changes by a factor of e^d over the run.
                let d = match self {
                    Family::SustainedOscillation => 0.0,
                    Family::DampedOscillation => -rng.between(1.5, 4.0),
                    _ => rng.between(1.5, 4.0),
                };
                (
                    Box::new(move |x| (d * x).exp() * (TAU * cycles * x + phase).sin()),
                    (cycles * 12.0).ceil() as usize,
                    format!("cycles={cycles:.1} d={d:.1}"),
                )
            }
            Family::CollapseAndUptick => {
                // x e^{-x/tau}, which has collapsed to a hundredth of its
                // peak by the end, and then a recovery of 3% to 6% of the
                // peak over the last fifth of the run: World3's food per
                // capita is this shape.
                let tau = rng.between(0.07, 0.12);
                let uptick = rng.between(0.05, 0.08);
                (
                    Box::new(move |x| {
                        let late = ((x - 0.8) / 0.2).max(0.0);
                        x * (-x / tau).exp() / (tau * (-1.0f64).exp()) + uptick * late * late
                    }),
                    50,
                    format!("tau={tau:.2} uptick={uptick:.3}"),
                )
            }
            Family::DisturbedGrowth => {
                // Growth by a factor of e^g, with four dips of 3% to 6% of its
                // range in the first two fifths of the run, where it has moved
                // little: the shape of a temperature record before a
                // projection (C-LEARN's is one).
                let g = rng.between(2.5, 5.0);
                let top = g.exp() - 1.0;
                let dips: Vec<(f64, f64, f64)> = (0..4)
                    .map(|i| {
                        (
                            0.08 + 0.08 * f64::from(i) + rng.between(0.0, 0.03),
                            rng.between(0.03, 0.06),
                            rng.between(0.01, 0.02),
                        )
                    })
                    .collect();
                let params = format!("g={g:.2} dips={dips:.2?}");
                (
                    Box::new(move |x| {
                        let dip: f64 = dips
                            .iter()
                            .map(|(at, depth, width)| depth * (-((x - at) / width).powi(2)).exp())
                            .sum();
                        ((g * x).exp() - 1.0) / top - dip
                    }),
                    200,
                    params,
                )
            }
        }
    }
}

/// How one generated series is sampled and transformed.
#[derive(Clone, Copy, Debug)]
struct Sampling {
    points: usize,
    scale: f64,
    offset: f64,
    negate: bool,
    start: f64,
    horizon: f64,
    /// Noise, as a fraction of the series' range.
    noise: f64,
}

/// Sample `curve` under `sampling`: the times and the values.
fn sample(curve: &dyn Fn(f64) -> f64, sampling: Sampling, rng: &mut Lcg) -> (Vec<f64>, Vec<f64>) {
    let n = sampling.points;
    let at = |i: usize| i as f64 / (n - 1) as f64;
    let times: Vec<f64> = (0..n)
        .map(|i| sampling.start + sampling.horizon * at(i))
        .collect();
    let raw: Vec<f64> = (0..n).map(|i| curve(at(i))).collect();
    let (lo, hi) = raw
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let values = raw
        .iter()
        .map(|v| {
            let unit = (v - lo) / (hi - lo) + sampling.noise * (2.0 * rng.unit() - 1.0);
            let v = sampling.offset + sampling.scale * unit;
            if sampling.negate { -v } else { v }
        })
        .collect();
    (times, values)
}

/// The mode a series of `family` has once negated or not.
fn expected(family: Family, negate: bool) -> (ModeKind, Option<Damping>) {
    match (family.mode(), negate) {
        ((ModeKind::RiseAndFall, damping), true) => (ModeKind::FallAndRise, damping),
        (mode, _) => mode,
    }
}

/// Classify `per_family` curves of every family at every density, under a
/// transform drawn for each, and return how many were named right of how
/// many, with the first mistake of each kind.
fn sweep(per_family: usize, densities: &[usize], noise: f64) -> (usize, usize, Vec<String>) {
    let mut rng = Lcg(0x5eed);
    let (mut right, mut total) = (0, 0);
    let mut mistakes: std::collections::BTreeMap<(Family, String), String> = Default::default();
    for family in Family::ALL {
        for _ in 0..per_family {
            let (curve, fewest, params) = family.curve(&mut rng);
            for &points in densities.iter().filter(|&&points| points >= fewest) {
                let scale = rng.pick(&[1e-12, 1e-6, 1.0, 37.5, 1e6]);
                let sampling = Sampling {
                    points,
                    scale,
                    // Up to a thousand ranges: past ten thousand a series
                    // reads the same to every reported digit.
                    offset: rng.pick(&[0.0, 0.0, 10.0, -250.0, 1000.0]) * scale,
                    negate: rng.unit() < 0.5,
                    start: rng.pick(&[0.0, 1990.0]),
                    horizon: rng.pick(&[1.0, 40.0, 3650.0]),
                    noise,
                };
                let (times, values) = sample(&*curve, sampling, &mut rng);
                let mode = classify(&times, &values);
                let got = (mode.kind, mode.damping);
                total += 1;
                if got == expected(family, sampling.negate) {
                    right += 1;
                } else {
                    mistakes
                        .entry((family, format!("{got:?}")))
                        .or_insert_with(|| format!("{params} {sampling:?}"));
                }
            }
        }
    }
    let mistakes = mistakes
        .into_iter()
        .map(|((family, got), example)| format!("{family:?} read as {got}: {example}"))
        .collect();
    (right, total, mistakes)
}

/// Every family is named for what it is: a few curves of each, from five
/// saved points to four hundred, at every scale, clean and with noise a
/// fifth of the tolerance.
#[test]
fn generated_curves_of_every_family_are_named_for_their_family() {
    for noise in [0.0, 0.2 * NOISE_FRACTION] {
        let (right, total, mistakes) = sweep(4, &[5, 12, 50, 200, 400], noise);
        assert!(total >= 100, "{total} series");
        assert_eq!(right, total, "noise {noise}: {mistakes:#?}");
    }
}

/// The same, over forty curves of each family and up to five thousand saved
/// points: clean curves are all named right, and with noise a fifth of the
/// tolerance at most one in a hundred is not.
#[test]
#[ignore = "sweeps the classifier over thousands of generated curves; run under the gates profile"]
fn the_classifier_names_generated_curves_across_parameters_and_densities() {
    let densities = [5, 8, 12, 25, 50, 100, 400, 1000, 5000];
    let (right, total, mistakes) = sweep(40, &densities, 0.0);
    assert!(total >= 1900, "{total} series");
    assert_eq!(right, total, "clean: {mistakes:#?}");
    let (right, total, mistakes) = sweep(40, &densities, 0.2 * NOISE_FRACTION);
    assert!(
        right * 100 >= total * 99,
        "with noise {right} of {total}: {mistakes:#?}"
    );
}

// ── Series from real models ───────────────────────────────────────────
//
// The generated curves above are the classifier's own author's; these are
// not. Series from the corpus's largest models, each labeled with the mode a
// modeler gives it and why.

/// One labeled series of `behavior_real_series.tsv`.
struct RealSeries {
    model: &'static str,
    variable: &'static str,
    label: Label,
    why: &'static str,
    times: Vec<f64>,
    values: Vec<f64>,
}

/// A mode as a label names it: its kind, its direction, and for an
/// oscillation the damping where the label names one.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Label {
    kind: ModeKind,
    direction: Option<Direction>,
    damping: Option<Damping>,
}

impl Label {
    /// A label from its two columns: `kind` or `kind/damping`, and `rising`,
    /// `falling` or `-`.
    fn named(mode: &str, direction: &str) -> Label {
        let (kind, damping) = match mode.split_once('/') {
            Some((kind, damping)) => (kind, Some(damping)),
            None => (mode, None),
        };
        Label {
            kind: serde_json::from_value(serde_json::Value::String(kind.to_string()))
                .unwrap_or_else(|_| panic!("{kind} is no mode")),
            direction: match direction {
                "rising" => Some(Direction::Rising),
                "falling" => Some(Direction::Falling),
                "-" => None,
                other => panic!("{other} is no direction"),
            },
            damping: damping.map(|damping| match damping {
                "damped" => Damping::Damped,
                "sustained" => Damping::Sustained,
                "growing" => Damping::Growing,
                "undetermined" => Damping::Undetermined,
                other => panic!("{other} is no damping"),
            }),
        }
    }

    /// Whether `mode` is this: its kind and direction, and its damping where
    /// this names one.
    fn names(&self, mode: &BehaviorMode) -> bool {
        (self.kind, self.direction) == (mode.kind, mode.direction)
            && self
                .damping
                .is_none_or(|damping| mode.damping == Some(damping))
    }
}

/// What is wrong with the classifier's reading `mode` of a series a modeler
/// labels `label`, if anything: it names it otherwise and the disagreement is
/// not known; it is known to name it otherwise and now agrees; or it names it
/// otherwise than the disagreement known.
fn misreading(model: &str, variable: &str, label: Label, mode: &BehaviorMode) -> Option<String> {
    let known = KNOWN_DISAGREEMENTS
        .iter()
        .find(|row| row.0 == model && row.1 == variable);
    match known {
        None if !label.names(mode) => Some(format!(
            "{variable} is {label:?} and reads {:?} {:?} {:?}",
            mode.kind, mode.direction, mode.damping
        )),
        None => None,
        Some(_) if label.names(mode) => Some(format!(
            "{variable} is a known disagreement and now reads as a modeler does: delete its row"
        )),
        Some(&(_, _, says, direction, why)) => {
            let says = Label::named(says, direction);
            (!says.names(mode)).then(|| {
                format!(
                    "{variable} is known to read {says:?} ({why}) and reads {:?} {:?} {:?}",
                    mode.kind, mode.direction, mode.damping
                )
            })
        }
    }
}

/// The checked-in series: a handful, written out from the models' own runs
/// (the gate below holds each to its model).
fn real_series() -> Vec<RealSeries> {
    include_str!("behavior_real_series.tsv")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| {
            let fields: Vec<&'static str> = line.split('\t').collect();
            let [model, variable, mode, direction, why, first, step, values] = fields[..] else {
                panic!("a row has eight fields: {line}");
            };
            let number = |text: &str| -> f64 { text.parse().expect("a number") };
            let values: Vec<f64> = values.split(' ').map(number).collect();
            let (first, step) = (number(first), number(step));
            RealSeries {
                model,
                variable,
                label: Label::named(mode, direction),
                why,
                times: (0..values.len()).map(|i| first + step * i as f64).collect(),
                values,
            }
        })
        .collect()
}

/// Series from World3, C-LEARN and two small models are named as a modeler
/// names them -- the collapse that recovers slightly at the end is a rise
/// and fall, the fall and rise that starts with a brief rise is a fall and
/// rise, the noisy record before a long rise is the rise, the brief hump
/// before a long decline is the decline -- except where
/// [`KNOWN_DISAGREEMENTS`] says the classifier names one otherwise, and how.
#[test]
fn series_from_real_models_are_named_as_a_modeler_names_them() {
    let series = real_series();
    assert!(series.len() >= 24, "{} series", series.len());
    let misread: Vec<String> = series
        .iter()
        .filter_map(|labeled| {
            let mode = classify(&labeled.times, &labeled.values);
            misreading(labeled.model, labeled.variable, labeled.label, &mode)
                .map(|problem| format!("{problem} ({})", labeled.why))
        })
        .collect();
    assert!(misread.is_empty(), "{misread:#?}");
}

/// A model of the corpus, run: a Vensim model, or an XMILE one.
fn corpus_run(path: &str) -> crate::results::Results {
    let full = format!("{}/../../test/{path}", env!("CARGO_MANIFEST_DIR"));
    let contents = std::fs::read_to_string(&full).unwrap_or_else(|err| panic!("{full}: {err}"));
    let project = if path.ends_with(".mdl") {
        crate::compat::open_vensim(&contents)
    } else {
        crate::compat::open_xmile(&mut std::io::BufReader::new(contents.as_bytes()))
    }
    .expect("the model opens");
    let mut db = crate::db::SimlinDb::default();
    let source = db.sync(&project);
    let Ok(mut vm) = crate::build_sim(
        &mut db,
        source,
        &project,
        "main",
        crate::db::LtmOverlay::Off,
    ) else {
        panic!("{path} compiles");
    };
    assert!(vm.run_to_end().is_ok(), "{path} runs");
    vm.into_results()
}

const WORLD3: &str = "metasd/WRLD3-03/wrld3-03.mdl";
const CLEARN: &str = "xmutil_test_models/C-LEARN v77 for Vensim.mdl";
const LAND: &str = "land_model/land_model.stmx";
const HARES: &str = "test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile";

/// A wider set of the two models' series, each with the mode a modeler gives
/// it and why: (model, variable, mode, direction, why).
const LABELED: &[(&str, &str, &str, &str, &str)] = &[
    // World3's standard run: growth, overshoot and collapse.
    (
        WORLD3,
        "population",
        "rise_and_fall",
        "rising",
        "overshoot and collapse",
    ),
    (
        WORLD3,
        "population_0_to_14",
        "rise_and_fall",
        "rising",
        "peaks and falls two thirds of the way back",
    ),
    (
        WORLD3,
        "labor_force",
        "rise_and_fall",
        "rising",
        "follows population up and down",
    ),
    (
        WORLD3,
        "industrial_output",
        "rise_and_fall",
        "rising",
        "exponential growth, a peak, collapse",
    ),
    (
        WORLD3,
        "industrial_output_per_capita",
        "rise_and_fall",
        "rising",
        "exponential growth, a peak, collapse",
    ),
    (
        WORLD3,
        "industrial_capital",
        "rise_and_fall",
        "rising",
        "accumulates, then depreciates away",
    ),
    (
        WORLD3,
        "food",
        "rise_and_fall",
        "rising",
        "rises with inputs and falls with fertility",
    ),
    (
        WORLD3,
        "service_output_per_capita",
        "rise_and_fall",
        "rising",
        "rises and collapses",
    ),
    (
        WORLD3,
        "persistent_pollution",
        "rise_and_fall",
        "rising",
        "lags industrial output up and down",
    ),
    (
        WORLD3,
        "persistent_pollution_index",
        "rise_and_fall",
        "rising",
        "lags industrial output up and down",
    ),
    (
        WORLD3,
        "life_expectancy",
        "rise_and_fall",
        "rising",
        "rises with services and food, falls with both",
    ),
    (
        WORLD3,
        "human_welfare_index",
        "rise_and_fall",
        "rising",
        "rises and collapses",
    ),
    (
        WORLD3,
        "land_yield",
        "rise_and_fall",
        "rising",
        "rises with inputs, falls as they are withdrawn",
    ),
    (
        WORLD3,
        "deaths_65_plus",
        "rise_and_fall",
        "rising",
        "a late rise and a fall of more than half of it",
    ),
    (
        WORLD3,
        "fertility_control_effectiveness",
        "rise_and_fall",
        "rising",
        "rises to its ceiling, holds, and falls two thirds of the way back",
    ),
    (
        WORLD3,
        "fraction_of_output_in_services",
        "rise_and_fall",
        "rising",
        "a small dip first, then a rise and a fall of more than half of it",
    ),
    // A collapse with a slight recovery in the last years.
    (
        WORLD3,
        "food_per_capita",
        "rise_and_fall",
        "rising",
        "the recovery at the end is 2% of its range",
    ),
    (
        WORLD3,
        "food_ratio",
        "rise_and_fall",
        "rising",
        "the recovery at the end is 2% of its range",
    ),
    (
        WORLD3,
        "perceived_food_ratio",
        "rise_and_fall",
        "rising",
        "a smoothed food ratio",
    ),
    // A long fall and a recovery, after a start-up rise or early wiggles.
    (
        WORLD3,
        "birth_rate",
        "fall_and_rise",
        "falling",
        "the start-up rise is a seventh of its range",
    ),
    (
        WORLD3,
        "total_fertility",
        "fall_and_rise",
        "falling",
        "the start-up rise is a sixth of its range",
    ),
    (
        WORLD3,
        "desired_total_fertility",
        "fall_and_rise",
        "falling",
        "the start-up rise is a fifth of its range",
    ),
    (
        WORLD3,
        "death_rate",
        "fall_and_rise",
        "falling",
        "falls with rising life expectancy, then rises past its start",
    ),
    (
        WORLD3,
        "mortality_0_to_14",
        "fall_and_rise",
        "falling",
        "falls and rises with life expectancy",
    ),
    (
        WORLD3,
        "deaths_0_to_14",
        "fall_and_rise",
        "falling",
        "early wiggles of a few percent",
    ),
    (
        WORLD3,
        "deaths_15_to_44",
        "fall_and_rise",
        "falling",
        "early wiggles of a few percent",
    ),
    (
        WORLD3,
        "land_fertility",
        "fall_and_rise",
        "falling",
        "degrades, then regenerates part of the way",
    ),
    (
        WORLD3,
        "average_life_of_land",
        "fall_and_rise",
        "falling",
        "falls with yield, recovers as yield falls",
    ),
    // One late rise that comes part of the way back, after an early bump.
    (
        WORLD3,
        "deaths",
        "overshoot",
        "rising",
        "rises late and falls back a third",
    ),
    (
        WORLD3,
        "deaths_45_to_64",
        "overshoot",
        "rising",
        "rises late and falls back a third",
    ),
    (
        WORLD3,
        "arable_land",
        "overshoot",
        "rising",
        "grows, peaks and erodes part of the way back",
    ),
    // Monotone: depletion and saturation.
    (
        WORLD3,
        "nonrenewable_resources",
        "s_shaped",
        "falling",
        "slow, fast, then slow as little is left",
    ),
    (
        WORLD3,
        "fraction_of_resources_remaining",
        "s_shaped",
        "falling",
        "slow, fast, then slow as little is left",
    ),
    (
        WORLD3,
        "potentially_arable_land",
        "s_shaped",
        "falling",
        "developed until none is left",
    ),
    (
        WORLD3,
        "urban_and_industrial_land",
        "s_shaped",
        "rising",
        "grows and then holds its level",
    ),
    (
        WORLD3,
        "development_cost_per_hectare",
        "s_shaped",
        "rising",
        "rises to its ceiling as land runs out",
    ),
    (WORLD3, "time", "linear", "rising", "the clock"),
    // Several swings of a quarter of the range and more.
    (
        WORLD3,
        "labor_utilization_fraction",
        "oscillation/growing",
        "-",
        "falls, recovers, falls and spikes at the end: its last swing is its largest",
    ),
    (
        WORLD3,
        "fraction_of_industrial_output_allocated_to_agriculture",
        "rise_and_fall",
        "rising",
        "rises to a plateau that dips a seventh and recovers, then falls back over half its rise",
    ),
    (
        WORLD3,
        "capacity_utilization_fraction",
        "exponential",
        "falling",
        "at one for most of two centuries, then an accelerating fall of 1.3% in the last decade",
    ),
    (
        WORLD3,
        "jobs",
        "rise_and_fall",
        "rising",
        "grows, falls two thirds of the way back, and rebounds a third in the last five years",
    ),
    // C-LEARN: a noisy record, then a projection.
    (
        CLEARN,
        "temperature_change_from_preindustrial[deterministic]",
        "exponential",
        "rising",
        "dips of up to 8% of the range before 2000, then a smooth rise",
    ),
    (
        CLEARN,
        "temperature_change_from_preindustrial[low_2xco2_sensitivity]",
        "exponential",
        "rising",
        "the same record beside a smaller rise",
    ),
    (
        CLEARN,
        "temperature_change_from_preindustrial[high_2xco2_sensitivity]",
        "exponential",
        "rising",
        "the same record beside a larger rise",
    ),
    (
        CLEARN,
        "bau_temperature_change_from_preindustrial",
        "exponential",
        "rising",
        "the reference scenario's temperature",
    ),
    (
        CLEARN,
        "atm_conc_co2[deterministic]",
        "exponential",
        "rising",
        "accelerating growth with no turning point",
    ),
    (
        CLEARN,
        "c_in_atmosphere[deterministic]",
        "exponential",
        "rising",
        "accelerating growth with no turning point",
    ),
    (
        CLEARN,
        "ch4_atm_conc[deterministic]",
        "exponential",
        "rising",
        "accelerating growth with no turning point",
    ),
    (
        CLEARN,
        "ch4_fractional_uptake[deterministic]",
        "s_shaped",
        "falling",
        "its decline per decade is largest in the 2040s and nearly halves by the 2090s",
    ),
    (
        CLEARN,
        "aggregated_population[developing_a_countries]",
        "overshoot",
        "rising",
        "peaks about 2050 and declines by a tenth",
    ),
    (
        CLEARN,
        "aggregated_population[developing_b_countries]",
        "s_shaped",
        "rising",
        "its growth per decade peaks in the 2040s and falls by 45% by the 2090s",
    ),
    (
        CLEARN,
        "emissions_per_gdp[g77_india]",
        "rise_and_fall",
        "rising",
        "rises to a peak in 1993 and falls four fifths of the way back",
    ),
    (
        CLEARN,
        "equilibrium_temperature[low_2xco2_sensitivity]",
        "exponential",
        "rising",
        "volcanic dips of up to a third of its range to 2000, then a smooth accelerating rise",
    ),
    (
        CLEARN,
        "semi_agg_forestry_emissions[other_developed]",
        "rise_and_fall",
        "rising",
        "rises with a dip to a spike in 1958, then falls to its floor and holds",
    ),
    (
        CLEARN,
        "proportion_of_global_to_cop_ch4[g77_india]",
        "fall_and_rise",
        "falling",
        "falls over half its range to 1965, rises past its start, and holds with small swings",
    ),
    (
        WORLD3,
        "family_income_expectation",
        "rise_and_fall",
        "rising",
        "rises a third of its range to 1932, falls the whole of it to 2030, recovers a fifth by 2072",
    ),
    (
        WORLD3,
        "family_response_to_social_norm",
        "rise_and_fall",
        "rising",
        "rises a third of its range to 1932, falls the whole of it to 2030, recovers a fifth by 2072",
    ),
    (
        CLEARN,
        "co2eq_emissions_from_f_gases[g77_china]",
        "rise_and_fall",
        "rising",
        "a spike to 2006, back down by 2020, then a creep of 16% of its range over 80 years",
    ),
    // Two small models: a start-up hump before a long decline, and a long
    // small fall before a large rise.
    (
        LAND,
        "waste_land",
        "linear",
        "falling",
        "a start-up rise of 15% of its range, then a steady decline",
    ),
    (
        HARES,
        "hares·deaths",
        "fall_and_rise",
        "falling",
        "falls a quarter of its range over half the run, then rises to four times its start",
    ),
];

/// The labeled series the classifier names differently from the modeler,
/// each with what it says and why: (model, variable, what the classifier
/// says, its direction, why). A row here that starts agreeing, or that the
/// classifier comes to name a third way, fails the tests that read it, so
/// the table says what the classifier does today.
const KNOWN_DISAGREEMENTS: &[(&str, &str, &str, &str, &str)] = &[
    // A curve that inflects late stays under its chord and reads as growth
    // that is still speeding up.
    (
        CLEARN,
        "ch4_fractional_uptake[deterministic]",
        "exponential",
        "falling",
        "its inflection is four fifths of the way along, so it runs below its chord",
    ),
    (
        CLEARN,
        "aggregated_population[developing_b_countries]",
        "exponential",
        "rising",
        "its inflection is four fifths of the way along, so it runs below its chord",
    ),
    // Dips and rebounds over a tenth of the range are movements, so they
    // make an oscillation of a rise, a fall or a rise and fall.
    (
        CLEARN,
        "equilibrium_temperature[low_2xco2_sensitivity]",
        "oscillation/sustained",
        "-",
        "its volcanic dips of a seventh to a third of its range are swings",
    ),
    (
        CLEARN,
        "emissions_per_gdp[g77_india]",
        "oscillation/damped",
        "-",
        "a rebound of a tenth of its range on the way down is a swing",
    ),
    (
        CLEARN,
        "semi_agg_forestry_emissions[other_developed]",
        "oscillation/growing",
        "-",
        "a dip of a quarter of its range on the way up is a swing",
    ),
    (
        CLEARN,
        "proportion_of_global_to_cop_ch4[g77_india]",
        "oscillation/growing",
        "-",
        "its swings of a tenth to a fifth of its range after the rise are movements",
    ),
    (
        WORLD3,
        "fraction_of_industrial_output_allocated_to_agriculture",
        "oscillation/sustained",
        "-",
        "the dip of a seventh on its plateau, and the recovery from it, are swings",
    ),
    (
        WORLD3,
        "jobs",
        "oscillation/undetermined",
        "-",
        "the late rebound is half the fall before it, too large to be the run being cut off",
    ),
    // A small last movement that outlasts a quarter of the run is no
    // cut-off, so it is a swing.
    (
        WORLD3,
        "family_income_expectation",
        "oscillation/damped",
        "-",
        "its last movement, a recovery of a fifth of its range from 2030 to the end, lasts 70 of 200 years",
    ),
    (
        WORLD3,
        "family_response_to_social_norm",
        "oscillation/damped",
        "-",
        "its last movement, a recovery of a fifth of its range from 2030 to the end, lasts 70 of 200 years",
    ),
    (
        CLEARN,
        "co2eq_emissions_from_f_gases[g77_china]",
        "oscillation/undetermined",
        "-",
        "its last movement, a creep of 16% of its range from 2020 to the end, lasts 80 of 250 years",
    ),
    // Its range is the fall at the end, so a dip of half a percent in its
    // first year is most of it.
    (
        WORLD3,
        "capacity_utilization_fraction",
        "oscillation/growing",
        "-",
        "its range is a fall of 1.3%, so a dip of 0.5% in its first year is a movement",
    ),
    // Damping is judged on the swings between turns, which shrink; the
    // movement the run's end cuts off is its largest.
    (
        WORLD3,
        "labor_utilization_fraction",
        "oscillation/damped",
        "-",
        "its swings between turns shrink, and the last movement, cut off by the run's end, is not one",
    ),
];

/// World3's, C-LEARN's and two small models' series are named as a modeler
/// names them over the wider labeled set, except as [`KNOWN_DISAGREEMENTS`]
/// says; every known disagreement is a labeled series; and each series of
/// the checked-in file is its model's own, to the digits the file keeps.
#[test]
#[ignore = "runs World3, C-LEARN and two small models and classifies their labeled series; run under the gates profile"]
fn the_classifier_names_labeled_series_as_a_modeler_does() {
    let mut models: Vec<&str> = LABELED.iter().map(|row| row.0).collect();
    models.sort_unstable();
    models.dedup();
    let runs: Vec<(&str, crate::results::Results)> = models
        .into_iter()
        .map(|path| (path, corpus_run(path)))
        .collect();
    let series = |model: &str, variable: &str| -> (Vec<f64>, Vec<f64>) {
        let (_, results) = runs
            .iter()
            .find(|(path, _)| *path == model)
            .unwrap_or_else(|| panic!("{model} is a labeled model"));
        let offset = *results
            .offsets
            .get(&crate::common::Ident::new(variable))
            .unwrap_or_else(|| panic!("{model} has no {variable}"));
        (
            results.iter().map(|row| row[0]).collect(),
            results.iter().map(|row| row[offset]).collect(),
        )
    };

    let mut misread = Vec::new();
    for &(model, variable, mode, direction, why) in LABELED {
        let (times, values) = series(model, variable);
        let found = classify(&times, &values);
        if let Some(problem) = misreading(model, variable, Label::named(mode, direction), &found) {
            misread.push(format!("{problem} ({why})"));
        }
    }
    assert!(misread.is_empty(), "{misread:#?}");

    for &(model, variable, ..) in KNOWN_DISAGREEMENTS {
        assert!(
            LABELED
                .iter()
                .any(|row| row.0 == model && row.1 == variable),
            "{variable} of {model} is a known disagreement of no labeled series"
        );
    }

    for labeled in real_series() {
        let row = LABELED
            .iter()
            .find(|row| row.0 == labeled.model && row.1 == labeled.variable)
            .unwrap_or_else(|| panic!("{} is in the wider set too", labeled.variable));
        assert_eq!(
            Label::named(row.2, row.3),
            labeled.label,
            "{} is labeled alike in both",
            labeled.variable
        );
        let (times, values) = series(labeled.model, labeled.variable);
        assert_eq!(times.len(), labeled.times.len(), "{}", labeled.variable);
        for (row, (ours, theirs)) in labeled.values.iter().zip(&values).enumerate() {
            let close = (ours - theirs).abs() <= 1e-5 * theirs.abs().max(1e-300);
            assert!(
                close && (labeled.times[row] - times[row]).abs() < 1e-9,
                "{} row {row}: the file has {ours} at {}, the model {theirs} at {}",
                labeled.variable,
                labeled.times[row],
                times[row]
            );
        }
    }
}

// ── Thresholds ────────────────────────────────────────────────────────

fn evenly(n: usize) -> Vec<f64> {
    (0..n).map(|i| i as f64 / (n - 1) as f64).collect()
}

fn kind_of(values: &[f64]) -> ModeKind {
    classify(&evenly(values.len()), values).kind
}

/// A line becomes a curve where it runs [`BEND`] of its range from its chord:
/// growth by a factor of 1.4 over the run is linear and by 1.7 exponential,
/// and an approach that covers as much of its gap is goal seeking. Nothing
/// between the two is left without a name.
#[test]
fn a_curve_is_a_line_until_it_bends_from_its_chord() {
    let grow = |g: f64| -> Vec<f64> { evenly(101).iter().map(|x| (g * x).exp()).collect() };
    let seek = |g: f64| -> Vec<f64> { evenly(101).iter().map(|x| -(-g * x).exp()).collect() };
    for (factor, growth, approach) in [
        (1.2f64, ModeKind::Linear, ModeKind::Linear),
        (1.4, ModeKind::Linear, ModeKind::Linear),
        (1.7, ModeKind::Exponential, ModeKind::GoalSeeking),
        (3.0, ModeKind::Exponential, ModeKind::GoalSeeking),
    ] {
        assert_eq!(kind_of(&grow(factor.ln())), growth, "growth by {factor}");
        assert_eq!(
            kind_of(&seek(factor.ln())),
            approach,
            "an approach by {factor}"
        );
    }
    for step in 0..=40 {
        let g = 0.1 + 0.03 * f64::from(step);
        assert_ne!(kind_of(&grow(g)), ModeKind::Other, "growth by e^{g}");
        assert_ne!(kind_of(&seek(g)), ModeKind::Other, "an approach by e^{g}");
    }
}

/// A reversal is a turning point once it is larger than [`NOISE_FRACTION`] of
/// the range: a rise that ends by falling back 0.8% is still a rise, and one
/// that falls back 1.5% has overshot.
#[test]
fn a_reversal_under_the_noise_tolerance_is_no_turning_point() {
    let rise_then_back = |back: f64| -> Vec<f64> {
        let mut values: Vec<f64> = (0..=80).map(|i| f64::from(i) / 80.0).collect();
        values.extend((1..=20).map(|i| 1.0 - back * f64::from(i) / 20.0));
        values
    };
    let small = rise_then_back(0.008);
    assert!(shape(&evenly(small.len()), &small).turns.is_empty());
    let large = rise_then_back(0.015);
    let found = shape(&evenly(large.len()), &large);
    assert_eq!(found.turns, [80]);
    assert_eq!(found.mode.kind, ModeKind::Overshoot);
}

/// Where a series has settled it stays within the tolerance of its last
/// value, and its wiggles there are not its movement: a swing from one edge
/// of that band to the other is larger than the tolerance and is still no
/// turning point.
#[test]
fn wiggles_where_a_series_has_settled_are_not_turning_points() {
    let times = evenly(201);
    let values: Vec<f64> = times
        .iter()
        .map(|&x| {
            if x <= 0.5 {
                1.0 - (-12.0 * x).exp()
            } else {
                1.0 + 0.007 * (std::f64::consts::TAU * 4.0 * (x - 0.5)).sin()
            }
        })
        .collect();
    let found = shape(&times, &values);
    assert!(found.turns.is_empty(), "{:?}", found.turns);
    assert_eq!(found.mode.kind, ModeKind::GoalSeeking);
    let settles = found.mode.settles_at.expect("it settles");
    assert!((0.3..0.5).contains(&settles), "{settles}");
}

/// A series is the curve its rows sample, read against their times: a line
/// saved every hundredth at first and every tenth later is a line, where by
/// row count it would bend.
#[test]
fn a_series_is_read_against_its_times_not_its_rows() {
    let times: Vec<f64> = (0..=50)
        .map(|i| f64::from(i) / 100.0)
        .chain((6..=10).map(|i| f64::from(i) / 10.0))
        .collect();
    assert_eq!(classify(&times, &times).kind, ModeKind::Linear);
    let growth: Vec<f64> = times.iter().map(|t| (3.0 * t).exp()).collect();
    assert_eq!(classify(&times, &growth).kind, ModeKind::Exponential);
    let approach: Vec<f64> = times.iter().map(|t| 1.0 - (-4.0 * t).exp()).collect();
    assert_eq!(classify(&times, &approach).kind, ModeKind::GoalSeeking);
}

/// The distance from the chord is smoothed over [`SMOOTHING`] of the run:
/// enough that a dip of a tenth of the range lasting a twentieth of the run
/// does not bend the line it interrupts, and not so much that a movement
/// lasting a fifth of the run is averaged away: the speeding-up of an S that
/// turns early, or the pause between two stages of growth.
#[test]
fn the_distance_from_the_chord_is_smoothed_over_a_tenth_of_the_run() {
    let times = evenly(201);
    let dipped: Vec<f64> = times
        .iter()
        .map(|&x| x - 0.095 * (1.0 - (x - 0.3).abs() / 0.025).max(0.0))
        .collect();
    let found = shape(&times, &dipped);
    assert_eq!(found.turns.len(), 2, "the dip is a reversal");
    assert_eq!(found.mode.kind, ModeKind::Linear);

    let early_s: Vec<f64> = times
        .iter()
        .map(|&x| 1.0 / (1.0 + (-20.0 * (x - 0.25)).exp()))
        .collect();
    assert_eq!(classify(&times, &early_s).kind, ModeKind::SShaped);

    // Two S-curves one after the other are not one S.
    let s_curve = |x: f64, mid: f64| 1.0 / (1.0 + (-40.0 * (x - mid)).exp());
    let two_stage: Vec<f64> = times
        .iter()
        .map(|&x| 0.5 * (s_curve(x, 0.25) + s_curve(x, 0.75)))
        .collect();
    assert_eq!(classify(&times, &two_stage).kind, ModeKind::Other);
}

/// A point's smoothing window is the rows within a twentieth of the run of
/// it, a row exactly a twentieth away included, on every clock: rows saved
/// evenly put a row exactly that far from every other whenever their count
/// allows, and its time rounds differently in another unit of time or from
/// another start.
#[test]
fn a_smoothing_window_is_the_same_rows_on_every_clock() {
    assert_eq!(SMOOTHING, 0.1, "a half window is a twentieth of the run");
    for rows in [61usize, 141, 150, 401] {
        let reach = (rows - 1) / 20;
        let expected: Vec<(usize, usize)> = (0..rows)
            .map(|i| (i.saturating_sub(reach), (i + reach).min(rows - 1)))
            .collect();
        let unit: Vec<f64> = (0..rows).map(|i| i as f64 / (rows - 1) as f64).collect();
        for (start, stretch) in [
            (0.0, 1.0),
            (0.0, 1e4),
            (0.0, 1.0 / 365.0),
            (-50.0, 12.0),
            (1990.0, 1.0 / 365.0),
            (1990.0, 1e4),
        ] {
            let clock: Vec<f64> = unit.iter().map(|t| start + stretch * t).collect();
            assert_eq!(
                smoothing_windows(&run_shares(&clock)),
                expected,
                "{rows} rows from {start} by {stretch}"
            );
        }
    }
}

/// With one turning point, coming back less than half the way is an
/// overshoot and more than half a rise and fall.
#[test]
fn an_overshoot_comes_back_less_than_half_the_way() {
    let up_then_back = |back: f64| -> Vec<f64> {
        let mut values: Vec<f64> = (0..=50).map(|i| f64::from(i) / 50.0).collect();
        values.extend((1..=50).map(|i| 1.0 - back * f64::from(i) / 50.0));
        values
    };
    assert_eq!(kind_of(&up_then_back(0.4)), ModeKind::Overshoot);
    assert_eq!(kind_of(&up_then_back(0.6)), ModeKind::RiseAndFall);
    let negated: Vec<f64> = up_then_back(0.6).iter().map(|v| -v).collect();
    assert_eq!(kind_of(&negated), ModeKind::FallAndRise);
}

/// An oscillation is damped when its last swing is under 0.8 of its first,
/// growing when over 1.25, and sustained between.
#[test]
fn an_oscillations_damping_is_its_last_swing_against_its_first() {
    // Triangle waves whose swings change by `ratio` from the first to the
    // last: turning points at 1, then alternating, each swing scaled.
    let wave = |ratio: f64| -> Vec<f64> {
        let swings = [1.0, ratio.sqrt(), ratio];
        let mut values = vec![0.0];
        let mut level: f64 = 0.0;
        let mut up = true;
        for swing in std::iter::once(0.5)
            .chain(swings)
            .chain(std::iter::once(0.5))
        {
            for i in 1..=20 {
                let step = swing * f64::from(i) / 20.0;
                values.push(if up { level + step } else { level - step });
            }
            level = if up { level + swing } else { level - swing };
            up = !up;
        }
        values
    };
    // A wiggle of 3% before a sustained oscillation is one of its turning
    // points and none of its swings.
    let after_a_wiggle: Vec<f64> = [0.0, 0.03, 0.0]
        .windows(2)
        .flat_map(|pair| (0..10).map(move |i| pair[0] + (pair[1] - pair[0]) * f64::from(i) / 10.0))
        .chain(wave(1.0))
        .collect();
    let found = shape(&evenly(after_a_wiggle.len()), &after_a_wiggle);
    assert_eq!(found.mode.damping, Some(Damping::Sustained));
    assert_eq!(found.turns.len(), 6, "{:?}", found.turns);

    for (ratio, expected) in [
        (0.7, Damping::Damped),
        (0.9, Damping::Sustained),
        (1.2, Damping::Sustained),
        (1.3, Damping::Growing),
    ] {
        let values = wave(ratio);
        let mode = classify(&evenly(values.len()), &values);
        assert_eq!(mode.kind, ModeKind::Oscillation, "{ratio}");
        assert_eq!(
            mode.damping,
            Some(expected),
            "a last swing {ratio} of the first"
        );
    }
}

/// A series is named for its large movements. A reversal under
/// [`PROMINENCE`] of its range is a disturbance of the movement it interrupts
/// wherever it falls -- before the movement, part-way through it, after it --
/// and is still one of the turning points a summary lists.
#[test]
fn a_small_reversal_does_not_name_a_series_wherever_it_falls() {
    // A line from 0 to 1 over 200 steps with a dip at each of `at`: down for
    // five steps and back to the line in five more. The line rises 0.025
    // while the dip goes down, so a dip of `fall + 0.025` below the line is a
    // fall of `fall`, which is that share of the range.
    let dipped = |fall: f64, at: &[usize]| -> Vec<f64> {
        (0..=200)
            .map(|i| {
                let depth: f64 = at
                    .iter()
                    .map(|&center| 1.0 - (i as f64 - center as f64).abs() / 5.0)
                    .fold(0.0, f64::max);
                i as f64 / 200.0 - (fall + 0.025) * depth
            })
            .collect()
    };
    // Falls of 7% of the range: a line, wherever they are (the rise between
    // two of them is the line's own movement, not a reversal).
    for at in [vec![30, 50], vec![30, 110], vec![100], vec![185]] {
        let values = dipped(0.07, &at);
        let found = shape(&evenly(values.len()), &values);
        assert_eq!(found.mode.kind, ModeKind::Linear, "falls at {at:?}");
        assert_eq!(
            found.turns.len(),
            2 * at.len(),
            "the falls at {at:?} are still turning points"
        );
    }
    // Falls of 13% of the range are movements.
    assert_eq!(kind_of(&dipped(0.13, &[30, 50])), ModeKind::Oscillation);
    assert_eq!(kind_of(&dipped(0.13, &[100])), ModeKind::Oscillation);

    // Straight legs between the levels given, twenty steps each.
    let legs = |levels: &[f64]| -> Vec<f64> {
        let mut values = vec![levels[0]];
        for pair in levels.windows(2) {
            values.extend((1..=20).map(|i| pair[0] + (pair[1] - pair[0]) * f64::from(i) / 20.0));
        }
        values
    };
    // A rise and fall with a slight recovery after it, and with a slight
    // wiggle before it: a rise and fall.
    for levels in [
        vec![0.3, 1.0, 0.0, 0.03],
        vec![0.3, 0.33, 0.3, 1.0, 0.0],
        vec![0.3, 1.0, 0.95, 1.0, 0.0],
    ] {
        let values = legs(&levels);
        let found = shape(&evenly(values.len()), &values);
        assert_eq!(found.mode.kind, ModeKind::RiseAndFall, "{levels:?}");
        assert_eq!(found.mode.direction, Some(Direction::Rising), "{levels:?}");
        assert!(found.turns.len() >= 2, "{levels:?}: {:?}", found.turns);
    }
}

/// The first and the last movement are the ones the run's window cuts: one
/// under [`TRANSIENT`] of the movement beside it that lasts no more than
/// [`TRANSIENT_SHARE`] of the run is the run starting up or being cut off,
/// and one larger or longer is a movement. A series whose start-up transient
/// is dropped is one movement read from where the transient ends.
#[test]
fn a_first_or_last_movement_is_a_transient_when_it_is_small_and_brief() {
    assert_eq!((TRANSIENT, TRANSIENT_SHARE), (0.25, 0.25));
    // Straight legs from the first level to each `(level, rows)` in turn.
    let legs = |first: f64, rest: &[(f64, usize)]| -> Vec<f64> {
        let mut values = vec![first];
        for &(level, rows) in rest {
            let from = values[values.len() - 1];
            values.extend((1..=rows).map(|i| from + (level - from) * i as f64 / rows as f64));
        }
        values
    };
    // (the series, its mode), each a hundred rows.
    for (values, kind) in [
        // A start-up rise of 0.2 for a tenth of the run before a fall of 1
        // and a rise: a fall and rise. At 0.3 it is the first swing of an
        // oscillation, and so it is at 0.2 lasting three tenths.
        (
            legs(0.8, &[(1.0, 10), (0.0, 45), (0.6, 45)]),
            ModeKind::FallAndRise,
        ),
        (
            legs(0.7, &[(1.0, 10), (0.0, 45), (0.6, 45)]),
            ModeKind::Oscillation,
        ),
        (
            legs(0.8, &[(1.0, 30), (0.0, 35), (0.6, 35)]),
            ModeKind::Oscillation,
        ),
        // A dip of 0.2 for a tenth of the run before a rise of 1 is the
        // rise's start, and the rise from it is a line. At 0.3, or lasting
        // half the run, it is a fall and rise.
        (legs(0.2, &[(0.0, 10), (1.0, 90)]), ModeKind::Linear),
        (legs(0.3, &[(0.0, 10), (1.0, 90)]), ModeKind::FallAndRise),
        (legs(0.2, &[(0.0, 50), (1.0, 50)]), ModeKind::FallAndRise),
        // A recovery of 0.2 for a tenth of the run after a rise and a fall
        // of 1: a rise and fall. At 0.3, or lasting three tenths, it is an
        // oscillation's next swing.
        (
            legs(0.0, &[(1.0, 45), (0.0, 45), (0.2, 10)]),
            ModeKind::RiseAndFall,
        ),
        (
            legs(0.0, &[(1.0, 45), (0.0, 45), (0.3, 10)]),
            ModeKind::Oscillation,
        ),
        (
            legs(0.0, &[(1.0, 35), (0.0, 35), (0.2, 30)]),
            ModeKind::Oscillation,
        ),
        // A hump of 0.24 over the first tenth, a fall of 1, and a rebound of
        // 0.2 over the last fifth: the overshoot the same curve without the
        // hump is. The rebound is measured against the way down from the
        // start, as the overshoot is read, so it is a movement. Its mirror
        // likewise.
        (
            legs(0.76, &[(1.0, 10), (0.0, 70), (0.2, 20)]),
            ModeKind::Overshoot,
        ),
        (
            legs(0.24, &[(0.0, 10), (1.0, 70), (0.8, 20)]),
            ModeKind::Overshoot,
        ),
        // A fall back of 0.2 after a rise of 1 is an overshoot either way:
        // the rise is the one movement, and it came back.
        (legs(0.0, &[(1.0, 85), (0.8, 15)]), ModeKind::Overshoot),
        (legs(0.0, &[(1.0, 60), (0.8, 40)]), ModeKind::Overshoot),
        (legs(0.0, &[(1.0, 85), (0.7, 15)]), ModeKind::Overshoot),
        (legs(0.0, &[(1.0, 85), (0.4, 15)]), ModeKind::RiseAndFall),
    ] {
        assert_eq!(values.len(), 101);
        assert_eq!(kind_of(&values), kind, "{values:?}");
    }
}

/// A series that is one movement overshot when it ends short of the furthest
/// it went by more than the noise tolerance; a dip on the way is no
/// overshoot, and neither is ending where it peaked.
#[test]
fn one_movement_that_ends_short_of_its_furthest_overshot() {
    let rise_then_back = |back: f64| -> Vec<f64> {
        (0..=80)
            .map(|i| 1.0 - (-f64::from(i) / 16.0).exp())
            .chain((1..=40).map(|i| 0.9933 - back * f64::from(i.min(20)) / 20.0))
            .collect()
    };
    let mode = classify(&evenly(121), &rise_then_back(0.05));
    assert_eq!(
        (mode.kind, mode.direction),
        (ModeKind::Overshoot, Some(Direction::Rising))
    );
    assert_eq!(kind_of(&rise_then_back(0.015)), ModeKind::Overshoot);
    assert_eq!(kind_of(&rise_then_back(0.005)), ModeKind::GoalSeeking);
    // Saved so coarsely that one row steps past the final level, by less
    // than the tolerance: no overshoot.
    assert_eq!(
        kind_of(&[0.0, 0.5, 1.004, 1.0, 1.0, 1.0, 1.0, 1.0]),
        ModeKind::Linear
    );

    // A rise that dips 6% on the way and ends 3% short of its peak has ended
    // on a smaller dip than it already made: one more disturbance of the
    // rise. Ending 8% short, the largest fall back it makes, it overshot.
    let dipped_then_back = |back: f64| -> Vec<f64> {
        (0..=40)
            .map(|i| 0.5 * f64::from(i) / 40.0)
            .chain((1..=10).map(|i| 0.5 - 0.06 * f64::from(i) / 10.0))
            .chain((1..=40).map(|i| 0.44 + 0.56 * f64::from(i) / 40.0))
            .chain((1..=10).map(|i| 1.0 - back * f64::from(i) / 10.0))
            .collect()
    };
    // (What it is instead is the rise's shape against its chord.)
    assert_ne!(kind_of(&dipped_then_back(0.03)), ModeKind::Overshoot);
    assert_eq!(kind_of(&dipped_then_back(0.08)), ModeKind::Overshoot);
    let fallen: Vec<f64> = rise_then_back(0.05).iter().map(|v| -v).collect();
    let mode = classify(&evenly(121), &fallen);
    assert_eq!(
        (mode.kind, mode.direction),
        (ModeKind::Overshoot, Some(Direction::Falling))
    );
}

/// A series is at rest when its least and greatest values are less than one
/// unit in the last reported digit apart, at any scale: just under it reads
/// as one number, and just over it does not.
#[test]
fn a_series_is_at_rest_when_its_extremes_read_the_same() {
    for scale in [1e-12, 1e-3, 1.0, 1e6] {
        // Around 1.2345 of the scale: the last reported digit is 1e-4.
        let moving = |spread: f64| -> Vec<f64> {
            evenly(50)
                .iter()
                .map(|x| scale * (1.2345 + spread * x))
                .collect()
        };
        assert_eq!(kind_of(&moving(0.9e-4)), ModeKind::AtRest, "scale {scale}");
        assert_eq!(kind_of(&moving(1.5e-4)), ModeKind::Linear, "scale {scale}");
        let swing: Vec<f64> = evenly(200)
            .iter()
            .map(|x| scale * (25.0 * x).sin())
            .collect();
        assert_eq!(kind_of(&swing), ModeKind::Oscillation, "scale {scale}");
    }
    assert_eq!(kind_of(&[0.0; 20]), ModeKind::AtRest);
}

/// A caller that knows the scale a series is computed at says so, and a
/// series that never leaves zero by more than the rounding floating point
/// leaves at that scale ([`RESIDUE_EPSILONS`] units of `f64::EPSILON` of it)
/// is at rest: residue of 1.1e-15 beside quantities of order one. Read
/// alone, or at a scale whose rounding is smaller than what it reaches, the
/// same series is the movement its numbers show, as a model whose quantities
/// are all that small must be.
#[test]
fn a_series_that_never_leaves_zero_at_its_scale_is_at_rest() {
    let times = evenly(81);
    let creeping: Vec<f64> = times.iter().map(|x| 1.1e-15 * x).collect();
    assert_eq!(classify(&times, &creeping).kind, ModeKind::Linear);
    assert_eq!(residue_bound(1.0), RESIDUE_EPSILONS * f64::EPSILON);
    // The scale whose residue bound is what the series reaches.
    let edge = 1.1e-15 / residue_bound(1.0);
    for (scale, kind) in [
        (0.0, ModeKind::Linear),
        (1e-15, ModeKind::Linear),
        (1e-6, ModeKind::Linear),
        (0.99 * edge, ModeKind::Linear),
        (1.01 * edge, ModeKind::AtRest),
        (6.0, ModeKind::AtRest),
    ] {
        assert_eq!(
            classify_at(&times, &creeping, scale).kind,
            kind,
            "at a scale of {scale}"
        );
    }
    // Residue either side of zero, and residue that swings.
    let swinging: Vec<f64> = times.iter().map(|x| 3e-16 * (40.0 * x).sin()).collect();
    assert_eq!(classify(&times, &swinging).kind, ModeKind::Oscillation);
    assert_eq!(classify_at(&times, &swinging, 1.0).kind, ModeKind::AtRest);
    // A scale says nothing of a series that does leave zero: it is read as
    // it is, whatever it is computed from.
    let moving: Vec<f64> = times.iter().map(|x| 1e-3 * x).collect();
    assert_eq!(classify_at(&times, &moving, 1e5).kind, ModeKind::Linear);
    assert_eq!(magnitude(&[1.0, -3.0, f64::NAN, 2.0]), 3.0);
    assert_eq!(magnitude(&[]), 0.0);
}

/// A still start or a settled end is reported once it covers
/// [`EDGE_FRACTION`] of the run.
#[test]
fn a_still_start_and_a_settled_end_are_reported_past_a_twentieth_of_the_run() {
    let held = |before: usize, after: usize| -> Vec<f64> {
        std::iter::repeat_n(0.0, before)
            .chain((0..=100 - before - after).map(|i| i as f64))
            .chain(std::iter::repeat_n((100 - before - after) as f64, after))
            .collect()
    };
    let times: Vec<f64> = (0..=100).map(f64::from).collect();
    let short = classify(&times, &held(3, 3));
    assert_eq!((short.starts_at, short.settles_at), (None, None));
    let long = classify(&times, &held(8, 8));
    assert_eq!((long.starts_at, long.settles_at), (Some(8.0), Some(92.0)));
    assert_eq!(long.kind, ModeKind::Linear);
}

/// A still start and a settled end of exactly [`EDGE_FRACTION`] of the run
/// are not past it, on any clock, and one row longer they are. Rows saved
/// evenly put an edge on exactly that share whenever their count allows, and
/// the times that measure it round differently in another unit of time or
/// from another start (on the unit clock a twentieth of 140 rows measures
/// `1.0 - 0.95`, a little over 0.05; ten thousand times longer it measures
/// exactly 500 of 10000).
#[test]
fn an_edge_of_exactly_a_twentieth_of_the_run_is_not_reported_on_any_clock() {
    for rows in [61usize, 101, 141, 401] {
        let twentieth = (rows - 1) / 20;
        let unit: Vec<f64> = (0..rows).map(|i| i as f64 / (rows - 1) as f64).collect();
        // Edges of a twentieth and of one row more: (rows still at the start
        // and held at the end, whether they are reported).
        for (edge, reported) in [(twentieth, false), (twentieth + 1, true)] {
            let ramp = rows - 1 - 2 * edge;
            // Still for `edge` rows, then half the way up over the ramp, then
            // the rest at once and held for `edge` rows: the last row that
            // moves and the first that stays are an edge from the ends.
            let values: Vec<f64> = (0..rows)
                .map(|i| {
                    if i <= edge {
                        0.0
                    } else if i < rows - 1 - edge {
                        0.5 * (i - edge) as f64 / ramp as f64
                    } else {
                        1.0
                    }
                })
                .collect();
            for (start, stretch) in [
                (0.0, 1.0),
                (0.0, 1e4),
                (0.0, 1.0 / 365.0),
                (-50.0, 12.0),
                (1990.0, 1.0 / 365.0),
                (1990.0, 1e4),
            ] {
                let clock: Vec<f64> = unit.iter().map(|t| start + stretch * t).collect();
                let mode = classify(&clock, &values);
                let expected = (
                    reported.then_some(clock[edge]),
                    reported.then_some(clock[rows - 1 - edge]),
                );
                assert_eq!(
                    (mode.starts_at, mode.settles_at),
                    expected,
                    "{rows} rows, an edge of {edge}, from {start} by {stretch}"
                );
            }
        }
    }
}

// ── Invariances ───────────────────────────────────────────────────────

mod invariance {
    use super::*;
    use proptest::prelude::*;

    /// A generated series: its family, and its samples, at three times the
    /// fewest rows that resolve the family's curve or more, so that every
    /// third row still does.
    fn a_series() -> impl Strategy<Value = (Family, Vec<f64>, Vec<f64>)> {
        (
            0..Family::ALL.len(),
            any::<u64>(),
            prop::sample::select(vec![60usize, 200, 400]),
        )
            .prop_map(|(family, seed, points)| {
                let family = Family::ALL[family];
                let mut rng = Lcg(seed);
                let (curve, fewest, _) = family.curve(&mut rng);
                let sampling = Sampling {
                    points: points.max(3 * fewest),
                    scale: 1.0,
                    offset: 0.0,
                    negate: false,
                    start: 0.0,
                    horizon: 1.0,
                    noise: 0.0,
                };
                let (times, values) = sample(&*curve, sampling, &mut rng);
                (family, times, values)
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// A mode is a statement about shape: a series scaled and shifted has
        /// the mode it had, and so do its turning points.
        #[test]
        fn scaling_and_shifting_a_series_keeps_its_mode(
            (_, times, values) in a_series(),
            scale in prop::sample::select(vec![1e-9, 0.01, 3.0, 2.5e7]),
            offset in -1000.0..1000.0f64,
        ) {
            let moved: Vec<f64> = values.iter().map(|v| scale * (v + offset)).collect();
            let (was, is) = (shape(&times, &values), shape(&times, &moved));
            prop_assert_eq!(was.mode.kind, is.mode.kind);
            prop_assert_eq!(was.mode.direction, is.mode.direction);
            prop_assert_eq!(was.mode.damping, is.mode.damping);
            prop_assert_eq!(was.turns, is.turns);
        }

        /// Upside down, a series goes the other way and is otherwise the
        /// same: a rise and fall is a fall and rise.
        #[test]
        fn negating_a_series_flips_its_direction_and_nothing_else(
            (_, times, values) in a_series(),
        ) {
            let negated: Vec<f64> = values.iter().map(|v| -v).collect();
            let (was, is) = (shape(&times, &values), shape(&times, &negated));
            let flipped = match was.mode.kind {
                ModeKind::RiseAndFall => ModeKind::FallAndRise,
                ModeKind::FallAndRise => ModeKind::RiseAndFall,
                other => other,
            };
            prop_assert_eq!(is.mode.kind, flipped);
            prop_assert_eq!(
                is.mode.direction,
                was.mode.direction.map(|d| match d {
                    Direction::Rising => Direction::Falling,
                    Direction::Falling => Direction::Rising,
                })
            );
            prop_assert_eq!(was.mode.damping, is.mode.damping);
            prop_assert_eq!(was.turns, is.turns);
        }

        /// The units of time and where the clock starts do not change a
        /// mode, and the times it reports move with them.
        #[test]
        fn another_clock_keeps_a_series_mode(
            (_, times, values) in a_series(),
            stretch in prop::sample::select(vec![1.0 / 365.0, 12.0, 1e4]),
            start in prop::sample::select(vec![0.0, -50.0, 1990.0]),
        ) {
            let clock: Vec<f64> = times.iter().map(|t| start + stretch * t).collect();
            let (was, is) = (shape(&times, &values), shape(&clock, &values));
            prop_assert_eq!(was.mode.kind, is.mode.kind);
            prop_assert_eq!(was.mode.damping, is.mode.damping);
            prop_assert_eq!(was.turns, is.turns);
            let moved = |t: Option<f64>| t.map(|t| start + stretch * t);
            let close = |a: Option<f64>, b: Option<f64>| match (a, b) {
                (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * (1.0 + a.abs().max(b.abs())),
                (None, None) => true,
                _ => false,
            };
            prop_assert!(close(moved(was.mode.starts_at), is.mode.starts_at));
            prop_assert!(close(moved(was.mode.settles_at), is.mode.settles_at));
        }

        /// A series is the curve its rows sample, however the rows are
        /// spaced, as long as they resolve it: saved unevenly (every row,
        /// then every second, then every third, as a save step off the DT
        /// grid saves), with the rows that are left still enough to resolve
        /// the curve, it has the mode it has saved evenly.
        #[test]
        fn rows_saved_unevenly_keep_a_series_mode(
            (family, times, values) in a_series(),
            phase in 0usize..3,
        ) {
            // Uneven rows keep a gap of one, two or three rows, in turn, and
            // the last row.
            let mut kept = vec![0];
            let mut gap = phase;
            while kept[kept.len() - 1] + gap % 3 + 1 < times.len() {
                kept.push(kept[kept.len() - 1] + gap % 3 + 1);
                gap += 1;
            }
            if kept[kept.len() - 1] != times.len() - 1 {
                kept.push(times.len() - 1);
            }
            let uneven_times: Vec<f64> = kept.iter().map(|&i| times[i]).collect();
            let uneven_values: Vec<f64> = kept.iter().map(|&i| values[i]).collect();
            let is = classify(&uneven_times, &uneven_values);
            prop_assert_eq!((is.kind, is.damping), family.mode());
            let was = classify(&times, &values);
            prop_assert_eq!((was.kind, was.damping), family.mode());
        }
    }
}
