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
//! It reads the series against its saved times, so rows that are not evenly
//! spaced (a save step off the DT grid) read as the curve they sample, as
//! long as they sample it: a bend or a swing a dozen rows cross is read the
//! same however the rows are spaced, and one that two or three rows cross is
//! read as those rows show it.
//!
//! - A series is at rest when it does not move at the scale it is read at.
//!   Read alone, that is its own: its least and greatest values differ by
//!   less than one unit in the last digit a summary reports. A caller that
//!   knows what the series is computed from says so ([`classify_at`]): a
//!   series that never leaves zero by more than the rounding floating point
//!   leaves at that scale ([`RESIDUE_EPSILONS`] times `f64::EPSILON` of it)
//!   is the arithmetic residue of quantities that cancel, not a movement,
//!   and is at rest too. A series cannot say this of
//!   itself -- residue of `1e-16` and a model whose quantities are all of
//!   order `1e-16` are the same numbers -- so there is no fixed floor: the
//!   scale is the caller's to give (a stock's is its flows over the run's
//!   horizon; two runs compared share the larger of their magnitudes).
//! - A still start and a settled end are trimmed first (the series "starts
//!   at" and "settles at" a time), so a step response reads as the goal
//!   seeking it is from its onset rather than as the speed-up and slow-down
//!   of an S. A start is still only where the series does not move at all --
//!   the slow first steps of exponential growth are part of its shape -- and
//!   an end is settled where the series stays within the noise tolerance of
//!   its final value, the settling time of control.
//! - A turning point is a reversal larger than the noise tolerance
//!   ([`NOISE_FRACTION`] of the series' range, with hysteresis), so noise
//!   never makes one. These are the turning points a summary lists.
//! - The mode is named for the series' large movements: its turning points
//!   at a coarser scale, reversals of more than [`PROMINENCE`] of the range.
//!   A smaller reversal is a disturbance of the movement it interrupts,
//!   wherever it falls: early wiggles before a long rise, a dip part-way up,
//!   a slight recovery after a collapse. The first and the last movement are
//!   the ones the run's window cuts, and one under [`TRANSIENT`] of the
//!   movement beside it that lasts no more than [`TRANSIENT_SHARE`] of the
//!   run is the run starting up or being cut off, not a movement of its own:
//!   a brief rise before a long fall and recovery is a fall and rise.
//!   - With none, the series is one movement, read from where a start-up
//!     transient ends. It is an overshoot when it
//!     ends short of the furthest it went (by more than the noise tolerance,
//!     and by more than any dip it made on the way there): it passed the
//!     level it ends at and came back. Otherwise it is named
//!     by where it runs against its own chord, the straight line from its
//!     first point to its last: along it (linear), below it (speeding up:
//!     exponential), above it (slowing down: goal seeking), or below and
//!     then above (S-shaped; so is one that speeds up all the way to a level
//!     it then holds). The distance is taken in the unit square and smoothed
//!     over a tenth of the run, so noise and brief disturbances do not set
//!     it and it does not depend on how densely the run was saved; one
//!     threshold ([`BEND`]) separates a line from every curve, so no curve
//!     falls between two tests.
//!   - With one, it is an overshoot when the series comes back less than
//!     half the way it went, else a rise and fall (overshoot and collapse)
//!     or a fall and rise.
//!   - With two or more it is an oscillation, damped, sustained or growing
//!     by how its swings change.
//!
//! What it names is the run's behavior over the run's horizon, not the
//! structure's: exponential growth at 1% a year over ten years is, over those
//! ten years, linear.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use super::series::SIGNIFICANT_DIGITS;

/// The fraction of a series' range below which a movement is noise.
pub(crate) const NOISE_FRACTION: f64 = 0.01;

/// A start is still while the series stays within this fraction of its range
/// of where it began: not moving at all, as before a step, rather than moving
/// slowly, as exponential growth begins.
const STILL_FRACTION: f64 = 1e-6;

/// How far from its chord, as a fraction of its own range, a series with no
/// turning point runs before it is a curve and not a line: the largest
/// distance, smoothed. Exponential growth by half over the run (a factor of
/// 1.5) bends about this much.
const BEND: f64 = 0.05;

/// The share of the run the distance from the chord is smoothed over.
const SMOOTHING: f64 = 0.1;

/// The fraction of a series' range a reversal has to exceed to be one of the
/// movements the mode is named for; a smaller one is a disturbance of the
/// movement it interrupts.
const PROMINENCE: f64 = 0.1;

/// The fraction of the movement beside it that a series' first or last
/// movement has to reach to be a movement: a quarter. The run's window cuts
/// both, so a small and brief one ([`TRANSIENT_SHARE`]) is where the run
/// happened to start or stop (a start-up transient, an oscillation cut off
/// just past a turn), where a small movement between two others is a swing
/// the series made whole.
const TRANSIENT: f64 = 0.25;

/// The share of the run a first or last movement under [`TRANSIENT`] of the
/// one beside it may last and still be the run starting up or being cut
/// off: a quarter, as for its size. A small movement that lasts longer is
/// one the series makes (a fall over half the run before a large rise is a
/// fall and rise), and one shorter is not (World3's total fertility rises a
/// sixth of its range over the first fifth of its run before its fall and
/// rise).
const TRANSIENT_SHARE: f64 = 0.25;

/// How many units of rounding (`f64::EPSILON`) of the scale a series is
/// read at it may stray from zero and still be residue: 64.
///
/// Residue is what floating point leaves, so it is bounded by floating point.
/// A sum of terms of magnitude `F` that cancel is off by at most half an ulp
/// of `F` per rounding that produced it. A stock's scale is its flows'
/// magnitude `F` times the horizon `H`, and Euler (or any of the methods,
/// whose weights sum to one) adds `dt * net` once a step, so a net flow that
/// is off by `k` units of `EPSILON * F` every step leaves the stock off by
/// at most `N * dt * k * EPSILON * F = k * EPSILON * F * H` after `N` steps:
/// the steps are already in the scale and are not counted again. A
/// multiple of 64 is a net flow, or a sum, off by 128 half-ulp roundings at
/// its terms' magnitude, more than an equation of terms that size takes, and
/// is `1.4e-14` of the scale: a real net change of a part in `1e13` of what
/// it is computed from is still seen moving.
pub(crate) const RESIDUE_EPSILONS: f64 = 64.0;

/// The most a series that is residue at `scale` strays from zero.
pub(crate) fn residue_bound(scale: f64) -> f64 {
    RESIDUE_EPSILONS * f64::EPSILON * scale
}

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
    /// Passes the level it ends at, and comes back less than half the way it
    /// went.
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

/// How far past a share of the run a stretch of it must reach to be past it,
/// as a share of the run: a millionth.
///
/// Rows saved evenly put a stretch on exactly a share whenever their count
/// allows (a twentieth of 140 rows is 7 of them), and the times that measure
/// it round differently in another unit of time or from another start, so
/// without a margin the same series would cross a threshold on one clock and
/// not on another. Rounding moves a share by some `1e-16` times the ratio of
/// the clock's magnitude to the run's length; a millionth is far above that
/// for any clock a model keeps and far below any share a threshold means.
const SHARE_TOLERANCE: f64 = 1e-6;

/// Whether `part` of a run of length `run` is more than `share` of it, the
/// same on every clock: a part of exactly that share is not.
fn past_share(part: f64, run: f64, share: f64) -> bool {
    part > (share + SHARE_TOLERANCE) * run
}

/// A series' behavior mode and its turning points.
pub(crate) struct Shape {
    pub mode: BehaviorMode,
    /// The indices, into the series, of its turning points: each a peak or
    /// trough it reverses from by more than the noise tolerance. The mode is
    /// named for the large ones; the small ones are in the series, and here,
    /// though they are not its mode.
    pub turns: Vec<usize>,
}

/// Name the behavior mode of `values`, saved at `times` (the same length, in
/// increasing order), read at its own scale.
pub fn classify(times: &[f64], values: &[f64]) -> BehaviorMode {
    shape(times, values).mode
}

/// [`classify`], read at `scale`: the magnitude of what the series is
/// computed from, or of what it is compared with (the module docs say why a
/// caller gives it). A scale of zero is the series' own.
pub fn classify_at(times: &[f64], values: &[f64], scale: f64) -> BehaviorMode {
    shape_at(times, values, scale).mode
}

/// The largest magnitude among `values` that are numbers: what a caller
/// comparing two series takes the larger of as the scale they share.
pub(crate) fn magnitude(values: &[f64]) -> f64 {
    values
        .iter()
        .filter(|v| v.is_finite())
        .fold(0.0, |m, v| m.max(v.abs()))
}

/// One unit in the last digit a summary reports of a number of magnitude
/// `magnitude`: two numbers of that magnitude closer than this can read the
/// same, and two further apart cannot.
fn last_reported_digit(magnitude: f64) -> f64 {
    if magnitude == 0.0 {
        return 0.0;
    }
    10f64.powi(magnitude.log10().floor() as i32 - (SIGNIFICANT_DIGITS - 1))
}

/// [`classify`], with the turning points it found.
pub(crate) fn shape(times: &[f64], values: &[f64]) -> Shape {
    shape_at(times, values, 0.0)
}

/// [`classify_at`], with the turning points it found.
pub(crate) fn shape_at(times: &[f64], values: &[f64], scale: f64) -> Shape {
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
    let (min, max) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
            (lo.min(v), hi.max(v))
        });
    let range = max - min;
    let magnitude = min.abs().max(max.abs());
    // At rest to the precision a summary reports: its least and greatest
    // values are less than one unit in the last reported digit apart, so
    // nothing it reports of the series shows it moving. A series that is not
    // at rest this way has a least and a greatest value that read
    // differently.
    let reads_the_same = range < last_reported_digit(magnitude) || range == 0.0;
    // Or at rest at the scale it is read at: it never leaves zero by more
    // than residue of that scale.
    let never_leaves_zero = magnitude <= residue_bound(scale);
    if n == 0 || reads_the_same || never_leaves_zero {
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
    let edge = |time: f64, from: f64| past_share((time - from).abs(), span, EDGE_FRACTION);
    let starts_at = (start > 0 && edge(times[start], times[0])).then_some(times[start]);
    let settles_at = (end < n - 1 && edge(times[n - 1], times[end])).then_some(times[end]);

    let segment = &values[start..=end];
    let segment_times = &times[start..=end];
    // A turning point is one the series reverses from by more than the
    // tolerance, wherever the reversal completes: a peak just before the
    // series settles is one though the fall from it ends inside the settled
    // tail. The tail's own wiggles, within the tolerance of the last value,
    // are not the series' movement.
    let mut turns = turning_points(&values[start..], tolerance);
    turns.retain(|&turn| turn < end - start);
    let settled = settles_at.is_some();
    // The movements the mode is named for: the turning points at the scale
    // of the series' range. A reversal smaller than that is a disturbance of
    // the movement it interrupts, wherever in the series it falls.
    let (movements, begins) = movements(segment_times, segment, PROMINENCE * range, span);
    let mut mode = match movements.as_slice() {
        [] => match overshoot(segment, last, tolerance) {
            Some(direction) => BehaviorMode {
                direction: Some(direction),
                ..BehaviorMode::of(ModeKind::Overshoot)
            },
            // One movement, named by where it runs against its chord from
            // where it begins: a start-up transient before it is no part of
            // its curve (a brief hump before a long decline would put the
            // decline below a chord drawn from the hump's foot).
            None => trend(&segment_times[begins..], &segment[begins..], settled),
        },
        [turn] => single_turn(segment, *turn),
        _ => BehaviorMode {
            damping: Some(damping(segment, &movements, &turns, settled)),
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

/// The turning points between a series' movements, and the index the
/// movements begin at: its reversals of more than `prominence`, less those
/// that end a first or a last movement that is a transient -- under
/// [`TRANSIENT`] of the movement beside it and no longer than
/// [`TRANSIENT_SHARE`] of the run's `span`. The movements begin at the turn
/// that ends the last start-up transient dropped (the start when none is),
/// and the first movement is measured from there.
fn movements(times: &[f64], values: &[f64], prominence: f64, span: f64) -> (Vec<usize>, usize) {
    let mut turns = turning_points(values, prominence);
    let last = values.len() - 1;
    let leg = |from: usize, to: usize| (values[to] - values[from]).abs();
    let brief =
        |from: usize, to: usize| !past_share(times[to] - times[from], span, TRANSIENT_SHARE);
    let mut begins = 0;
    // The first movement, from where the movements begin to the first turn,
    // beside the one from there to the next turn (or the end); and the last
    // likewise, beside the one from the turn before it -- or from the start,
    // a dropped start-up transient included, when it follows the only turn:
    // `overshoot` reads the whole segment, so a hump larger than the
    // come-back is the movement's largest reversal, and only the way from the
    // start says the come-back is a movement of its own (a hump, a fall of 1
    // and a rebound of 0.2 is the overshoot the same curve without the hump
    // is).
    while let Some((&first, &final_turn)) = turns.first().zip(turns.last()) {
        let after_first = turns.get(1).copied().unwrap_or(last);
        let before_final = if turns.len() > 1 {
            turns[turns.len() - 2]
        } else {
            0
        };
        // Measuring the first movement from the start instead would answer
        // the same: a second start-up drop cannot happen, since its first
        // leg would have to be under a sixteenth of the range, below a turn.
        if leg(begins, first) < TRANSIENT * leg(first, after_first) && brief(begins, first) {
            begins = turns.remove(0);
        } else if leg(final_turn, last) < TRANSIENT * leg(before_final, final_turn)
            && brief(final_turn, last)
        {
            turns.pop();
        } else {
            break;
        }
    }
    (turns, begins)
}

/// Whether a series that is one movement overshot: it went further than
/// `last`, the level it ends at, by more than `tolerance`, came back, and
/// that is the largest reversal it makes. The way it went, when it did.
///
/// A movement with dips along the way that ends on one more of them has not
/// overshot: where it ends is one of its disturbances. (The way back is
/// under [`PROMINENCE`] of the range, or it would be a movement of its own,
/// so an overshoot here came back less than half the way.)
fn overshoot(values: &[f64], last: f64, tolerance: f64) -> Option<Direction> {
    let first = values[0];
    let direction = direction_of(first, last);
    // The series as a rise, whichever way it goes.
    let up = |v: f64| match direction {
        Direction::Rising => v,
        Direction::Falling => -v,
    };
    // The furthest it went, and the largest fall back it made on the way
    // there.
    let (mut furthest, mut largest_dip) = (up(first), 0.0f64);
    let mut dip_since_furthest = 0.0f64;
    for &v in values {
        if up(v) >= furthest {
            furthest = up(v);
            largest_dip = largest_dip.max(dip_since_furthest);
            dip_since_furthest = 0.0;
        } else {
            dip_since_furthest = dip_since_furthest.max(furthest - up(v));
        }
    }
    let back = furthest - up(last);
    (back > tolerance && back >= largest_dip).then_some(direction)
}

fn direction_of(from: f64, to: f64) -> Direction {
    if to >= from {
        Direction::Rising
    } else {
        Direction::Falling
    }
}

/// Where each of `times` (increasing, the last after the first) is in the
/// run, as a share of it: zero at the first, one at the last.
fn run_shares(times: &[f64]) -> Vec<f64> {
    let (first, run) = (times[0], times[times.len() - 1] - times[0]);
    times.iter().map(|t| (t - first) / run).collect()
}

/// For each point at a share `at` of the run (increasing), the first and the
/// last point of its smoothing window: the points within half of
/// [`SMOOTHING`] of the run on either side of it, one exactly that far away
/// included, on every clock.
fn smoothing_windows(at: &[f64]) -> Vec<(usize, usize)> {
    let n = at.len();
    let (mut from, mut to) = (0, 0);
    (0..n)
        .map(|i| {
            while past_share(at[i] - at[from], 1.0, SMOOTHING / 2.0) {
                from += 1;
            }
            while to + 1 < n && !past_share(at[to + 1] - at[i], 1.0, SMOOTHING / 2.0) {
                to += 1;
            }
            (from, to)
        })
        .collect()
}

/// A series named for its movement from its first point to its last, by
/// where it runs against its chord (the module docs say how). `settled` says
/// the series then sat at its last value for a time: the plateau a settled
/// end was trimmed of. A series that speeds up all the way to a level it then
/// holds has the plateau for the top of its S.
fn trend(times: &[f64], values: &[f64], settled: bool) -> BehaviorMode {
    let n = values.len();
    let (first, last) = (values[0], values[n - 1]);
    let mode = |kind| BehaviorMode {
        direction: Some(direction_of(first, last)),
        ..BehaviorMode::of(kind)
    };
    let (rise, run) = (last - first, times[n - 1] - times[0]);
    if n < 4 || rise == 0.0 || run <= 0.0 {
        return mode(ModeKind::Linear);
    }
    // Where each point is in the run, and how far above its chord it is, both
    // as fractions: of the run's time and of the series' movement.
    let at = run_shares(times);
    let above: Vec<f64> = values
        .iter()
        .zip(&at)
        .map(|(v, x)| (v - first) / rise - x)
        .collect();
    // Smoothed over a window of the run's time around each point: the mean
    // of its points, from running sums.
    let mut sums = Vec::with_capacity(n + 1);
    sums.push(0.0);
    for distance in &above {
        sums.push(sums[sums.len() - 1] + distance);
    }
    let smooth: Vec<f64> = smoothing_windows(&at)
        .into_iter()
        .map(|(from, to)| (sums[to + 1] - sums[from]) / (to - from + 1) as f64)
        .collect();
    // The sides of the chord the series runs on, in order: an excursion below
    // it is -1 and one above it 1, each counted once however long it lasts.
    let mut sides: Vec<i8> = Vec::new();
    for &distance in &smooth {
        let side = if distance > BEND {
            1
        } else if distance < -BEND {
            -1
        } else {
            continue;
        };
        if sides.last() != Some(&side) {
            sides.push(side);
        }
    }
    mode(match sides.as_slice() {
        [] => ModeKind::Linear,
        [-1] if settled => ModeKind::SShaped,
        [-1] => ModeKind::Exponential,
        [1] => ModeKind::GoalSeeking,
        [-1, 1] => ModeKind::SShaped,
        _ => ModeKind::Other,
    })
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

/// How an oscillation's swings change, from the turning points it is named
/// for (`movements`): the last swing against the first, with three or more.
/// With two there is one whole swing and the movement after it. When that
/// movement ends where the series turns back (the furthest it went is one of
/// its `turns`: a swing smaller than the ones it is named for, or the start
/// of a movement the run cut off), it is a second swing. Otherwise the run
/// cut it off: the series is growing when it is already larger than the
/// swing, damped when it settles within half of it, and undetermined
/// otherwise (a sustained oscillation cut off after its second turn looks no
/// different from one about to grow or die away).
fn damping(values: &[f64], movements: &[usize], turns: &[usize], settles: bool) -> Damping {
    let mut swings: Vec<f64> = movements
        .windows(2)
        .map(|w| (values[w[1]] - values[w[0]]).abs())
        .collect();
    if let &[first, last] = movements {
        // How far the series has come back from the last turn at `i`.
        let back = (values[first] - values[last]).signum();
        let away = |i: usize| back * (values[i] - values[last]);
        let furthest = (last..values.len()).map(away).fold(0.0, f64::max);
        if turns.iter().any(|&t| t > last && away(t) >= furthest) {
            swings.push(furthest);
        }
    }
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
    let tail = (values[values.len() - 1] - values[movements[1]]).abs();
    if tail > 1.25 * swings[0] {
        Damping::Growing
    } else if settles && tail < 0.5 * swings[0] {
        Damping::Damped
    } else {
        Damping::Undetermined
    }
}

#[cfg(test)]
#[path = "behavior_tests.rs"]
mod tests;
