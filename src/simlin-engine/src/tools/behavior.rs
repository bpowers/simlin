// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Behavior modes: what a series does, in system dynamics' vocabulary.
//!
//! [`classify`] names a series' mode -- at rest, linear, exponential, goal
//! seeking, S-shaped, overshoot, rise and fall, fall and rise, oscillation, or
//! none of these -- the fundamental modes and their combinations of *Business
//! Dynamics* ch. 4. It is the one statement of a behavior mode: experiments,
//! `read_behavior`, the battery, and a host comparing a person's predicted
//! behavior with a run all ask it.
//!
//! It reads the series as the saved steps give it, on first differences, with
//! one tolerance: a movement smaller than [`NOISE_FRACTION`] of the series'
//! range is noise. So:
//!
//! - a still start and a settled end are trimmed first (the series "starts
//!   at" and "settles at" a time), so a step response reads as the goal
//!   seeking it is from its onset rather than as the speed-up and slow-down
//!   of an S. A start is still only where the series does not move at all --
//!   the slow first steps of exponential growth are part of its shape -- and
//!   an end is settled where the series stays within the tolerance of its
//!   final value, the settling time of control;
//! - a turning point is a reversal larger than the tolerance (hysteresis), so
//!   noise never makes one;
//! - a series with no turning point is named by how its speed changes: steady
//!   (linear), rising (exponential), falling (goal seeking), or rising then
//!   falling (S-shaped);
//! - one turning point is an overshoot when the series comes back less than
//!   half the way it went, else a rise and fall (overshoot and collapse) or a
//!   fall and rise;
//! - two or more are an oscillation, damped, sustained or growing by how its
//!   swings change.
//!
//! What it names is the run's behavior over the run's horizon, not the
//! structure's: exponential growth at 1% a year over ten years is, over those
//! ten years, linear.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

/// The fraction of a series' range below which a movement is noise.
pub(crate) const NOISE_FRACTION: f64 = 0.01;

/// A series whose range is below this fraction of its magnitude is at rest:
/// the precision a summary reports numbers to (five significant digits), so a
/// series every reported number of which reads the same is not called moving.
pub(crate) const REST_FRACTION: f64 = 1e-5;

/// The range below which a series near zero is at rest whatever its
/// magnitude: arithmetic noise around an equilibrium of zero.
const REST_FLOOR: f64 = 1e-9;

/// A start is still while the series stays within this fraction of its range
/// of where it began: not moving at all, as before a step, rather than moving
/// slowly, as exponential growth begins.
const STILL_FRACTION: f64 = 1e-6;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ModeKind {
    /// Does not move: an equilibrium.
    AtRest,
    /// Moves at a steady speed.
    Linear,
    /// Moves ever faster: exponential growth, or an accelerating decline.
    Exponential,
    /// Moves ever slower toward a level: approach to a goal, or decay.
    GoalSeeking,
    /// Speeds up, then slows toward a level.
    SShaped,
    /// Passes the level it settles at once, and comes back less than half the
    /// way it went.
    Overshoot,
    /// Rises, peaks, and falls back more than half the way it rose: overshoot
    /// and collapse.
    RiseAndFall,
    /// Falls, bottoms, and rises back more than half the way it fell.
    FallAndRise,
    /// Swings back and forth.
    Oscillation,
    /// Takes a non-finite value.
    Undefined,
    /// None of these.
    Other,
}

impl ModeKind {
    pub const ALL: [ModeKind; 11] = [
        ModeKind::AtRest,
        ModeKind::Linear,
        ModeKind::Exponential,
        ModeKind::GoalSeeking,
        ModeKind::SShaped,
        ModeKind::Overshoot,
        ModeKind::RiseAndFall,
        ModeKind::FallAndRise,
        ModeKind::Oscillation,
        ModeKind::Undefined,
        ModeKind::Other,
    ];
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Rising,
    Falling,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Damping {
    /// Each swing smaller than the last.
    Damped,
    /// Swings of about the same size.
    Sustained,
    /// Each swing larger than the last.
    Growing,
    /// Too few swings in the run to tell.
    Undetermined,
}

/// A series' behavior mode.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct BehaviorMode {
    pub kind: ModeKind,
    /// Which way it moves: for an overshoot or a single turn, the way it
    /// first went; absent at rest and for an oscillation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
    /// An oscillation's damping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damping: Option<Damping>,
    /// When a series that sat still at first began to move.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<f64>,
    /// When a series stopped moving, if that was before the run ended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settles_at: Option<f64>,
}

impl BehaviorMode {
    fn of(kind: ModeKind) -> BehaviorMode {
        BehaviorMode {
            kind,
            direction: None,
            damping: None,
            starts_at: None,
            settles_at: None,
        }
    }
}

/// The share of a run a quiet start or a flat end must cover to be reported.
const EDGE_FRACTION: f64 = 0.05;

/// A series' behavior mode and the turning points it was judged by.
pub(crate) struct Shape {
    pub mode: BehaviorMode,
    /// The indices, into the series, of its turning points: each a peak or
    /// trough it reverses from by more than the noise tolerance.
    pub turns: Vec<usize>,
}

/// Name the behavior mode of `values`, saved at `times` (the same length, in
/// increasing order).
pub fn classify(times: &[f64], values: &[f64]) -> BehaviorMode {
    shape(times, values).mode
}

/// [`classify`], with the turning points it found.
pub(crate) fn shape(times: &[f64], values: &[f64]) -> Shape {
    debug_assert_eq!(times.len(), values.len());
    let still = |mode: BehaviorMode| Shape {
        mode,
        turns: vec![],
    };
    if let Some(i) = values.iter().position(|v| !v.is_finite()) {
        return still(BehaviorMode {
            starts_at: times.get(i).copied(),
            ..BehaviorMode::of(ModeKind::Undefined)
        });
    }
    let n = values.len();
    if n == 0 {
        return still(BehaviorMode::of(ModeKind::AtRest));
    }
    let (min, max) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let range = max - min;
    let magnitude = min.abs().max(max.abs());
    if range <= (REST_FRACTION * magnitude).max(REST_FLOOR) {
        return still(BehaviorMode::of(ModeKind::AtRest));
    }
    let tolerance = NOISE_FRACTION * range;

    // The moving segment: from the last point before the series first leaves
    // where it began to the first point after which it stays within the
    // tolerance of its end.
    let first = values[0];
    let last = values[n - 1];
    let start = values
        .iter()
        .position(|v| (v - first).abs() > STILL_FRACTION * range)
        .map_or(0, |i| i.saturating_sub(1));
    let end = values
        .iter()
        .rposition(|v| (v - last).abs() > tolerance)
        .map_or(n - 1, |i| (i + 1).min(n - 1));
    let span = times[n - 1] - times[0];
    let edge = |time: f64, from: f64| (time - from).abs() > EDGE_FRACTION * span;
    let starts_at = (start > 0 && edge(times[start], times[0])).then_some(times[start]);
    let settles_at = (end < n - 1 && edge(times[n - 1], times[end])).then_some(times[end]);

    let segment = &values[start..=end];
    let turns = turning_points(segment, tolerance);
    let mut mode = match turns.len() {
        0 => monotone(segment),
        1 => single_turn(segment, turns[0]),
        _ => BehaviorMode {
            damping: Some(damping(segment, &turns, settles_at.is_some())),
            ..BehaviorMode::of(ModeKind::Oscillation)
        },
    };
    mode.starts_at = starts_at;
    mode.settles_at = settles_at;
    Shape {
        mode,
        turns: turns.into_iter().map(|i| i + start).collect(),
    }
}

/// The indices of `values`' interior turning points: each a peak or trough
/// the series reverses from by more than `tolerance`.
fn turning_points(values: &[f64], tolerance: f64) -> Vec<usize> {
    let mut turns = Vec::new();
    // Direction so far: 0 until the series first moves more than the
    // tolerance, then 1 rising or -1 falling. `extreme` is the index of the
    // highest (rising) or lowest (falling) value since the last turn.
    let mut direction = 0i8;
    let mut extreme = 0;
    for (i, &v) in values.iter().enumerate().skip(1) {
        match direction {
            0 => {
                if (v - values[0]).abs() > tolerance {
                    direction = if v > values[0] { 1 } else { -1 };
                    extreme = i;
                }
            }
            1 => {
                if v >= values[extreme] {
                    extreme = i;
                } else if values[extreme] - v > tolerance {
                    turns.push(extreme);
                    direction = -1;
                    extreme = i;
                }
            }
            _ => {
                if v <= values[extreme] {
                    extreme = i;
                } else if v - values[extreme] > tolerance {
                    turns.push(extreme);
                    direction = 1;
                    extreme = i;
                }
            }
        }
    }
    turns
}

fn direction_of(from: f64, to: f64) -> Direction {
    if to >= from {
        Direction::Rising
    } else {
        Direction::Falling
    }
}

/// A series with no turning point, named by how its speed changes.
fn monotone(values: &[f64]) -> BehaviorMode {
    let direction = Some(direction_of(values[0], values[values.len() - 1]));
    let speeds: Vec<f64> = values.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    let m = speeds.len();
    if m < 3 {
        return BehaviorMode {
            direction,
            ..BehaviorMode::of(ModeKind::Linear)
        };
    }
    let (fastest_at, fastest) =
        speeds
            .iter()
            .copied()
            .enumerate()
            .fold((0, f64::NEG_INFINITY), |best, (i, s)| {
                if s > best.1 { (i, s) } else { best }
            });
    let slowest = speeds.iter().copied().fold(f64::INFINITY, f64::min);
    let third = (m / 3).max(1);
    let mean = |slice: &[f64]| slice.iter().sum::<f64>() / slice.len() as f64;
    let early = mean(&speeds[..third]);
    let late = mean(&speeds[m - third..]);
    let at = fastest_at as f64 / (m - 1) as f64;
    let kind = if fastest - slowest <= fastest / 3.0 {
        ModeKind::Linear
    } else if late > 1.5 * early && at >= 0.8 {
        ModeKind::Exponential
    } else if early > 1.5 * late && at <= 0.2 {
        ModeKind::GoalSeeking
    } else if (0.2..=0.8).contains(&at) && early < 0.6 * fastest && late < 0.6 * fastest {
        ModeKind::SShaped
    } else {
        ModeKind::Other
    };
    BehaviorMode {
        direction,
        ..BehaviorMode::of(kind)
    }
}

/// A series that turns once at `turn`: an overshoot when it comes back less
/// than half the way it went, else a rise and fall or a fall and rise.
fn single_turn(values: &[f64], turn: usize) -> BehaviorMode {
    let (from, at, to) = (values[0], values[turn], values[values.len() - 1]);
    let went = at - from;
    let came_back = at - to;
    let direction = direction_of(from, at);
    let kind = if came_back.abs() < 0.5 * went.abs() {
        ModeKind::Overshoot
    } else if direction == Direction::Rising {
        ModeKind::RiseAndFall
    } else {
        ModeKind::FallAndRise
    };
    BehaviorMode {
        direction: Some(direction),
        ..BehaviorMode::of(kind)
    }
}

/// How an oscillation's swings change: the last swing against the first, with
/// three turns or more. With two there is one swing, and the series is damped
/// only when it then settles within half of it; a sustained oscillation the
/// run cuts off after its second turn has not settled, and is undetermined.
fn damping(values: &[f64], turns: &[usize], settles: bool) -> Damping {
    let swings: Vec<f64> = turns
        .windows(2)
        .map(|w| (values[w[1]] - values[w[0]]).abs())
        .collect();
    if swings.len() >= 2 {
        let ratio = swings[swings.len() - 1] / swings[0];
        return if ratio < 0.8 {
            Damping::Damped
        } else if ratio > 1.25 {
            Damping::Growing
        } else {
            Damping::Sustained
        };
    }
    let tail = (values[values.len() - 1] - values[turns[1]]).abs();
    if settles && tail < 0.5 * swings[0] {
        Damping::Damped
    } else {
        Damping::Undetermined
    }
}

#[cfg(test)]
#[path = "behavior_tests.rs"]
mod tests;
