// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `offset_segment`: moving one segment of a flow perpendicular to itself.

use smallvec::SmallVec;

use crate::datamodel::view_element::Flow;

use super::geometry::{
    Axis, FlowEnd, GEOMETRY_EPSILON, MIN_SEGMENT, MIN_SINK_SEGMENT, Obstacles, Path, Point, clamp,
};
use super::path::{
    apply_valve_margin, arc_position, normalize, path_length, place_valve, point_at_arc,
};
use super::terminal::{
    FaceAttachment, FlowGeometry, Terminal, Terminals, face_attachment, face_of_endpoint, path_of,
    stub_tip, with_geometry,
};
use super::validity::{Fault, path_quality};

/// The axis segment `i` runs along, and the coordinate it holds constant (the
/// one `offset_segment` sets).
pub(crate) fn segment_hold(points: &[Point], i: usize) -> (Axis, f64) {
    let (a, b) = (points[i], points[i + 1]);
    let axis = Axis::of_segment(a, b);
    (axis, a.coord(axis.other()))
}

/// How far past the request `offset_segment` looks for the far side of an
/// obstacle (a stock the segment or its cloud would enter): more than any
/// stock's extent.
const OBSTACLE_SCAN: f64 = 120.0;
const OBSTACLE_STEP: f64 = 2.0;

/// Move segment `segment_index` perpendicular to itself so it holds `coordinate`.
///
/// The coordinate is first resolved against the adjacent segments: a stock stub
/// keeps at least `MIN_SEGMENT` (`MIN_SINK_SEGMENT` at the sink), and an
/// adjacent riser or cloud segment is either at least its minimum or collapsed
/// to zero (removing that corner). These constraints are solved jointly, as the
/// feasible coordinate nearest the request, so a short riser collapses when the
/// request is within half a minimum of collapsing and is pushed out to the
/// minimum otherwise. When the resolved coordinate still puts the path through a
/// terminal body, a cloud inside a stock, or the body of any other stock in
/// `stocks`, the nearest valid coordinate on either side of that obstacle is
/// taken: the segment follows the pointer up to the obstacle and jumps across
/// once the far side is nearer (a documented feasibility transition).
///
/// At a terminal the tail is re-solved: a cloud moves with the segment; a stock
/// endpoint sits at the coordinate clamped to the face extent (minus
/// `CORNER_CLEARANCE`). While the coordinate is within `MIN_SEGMENT` beyond the
/// extent the segment stays at the extent; beyond that a stub plus a riser join
/// the endpoint to the segment. A tail that is already endpoint -> stub (at most
/// the end's minimum long) -> riser, with the dragged segment following the
/// riser, is re-solved the same way, so a bracket dragged back collapses to
/// straight and stubs never accumulate.
///
/// The valve stays where it is along its own segment while that segment
/// survives, clamped into the segment's new span; when its segment is removed
/// (a collapsed riser) it moves to the nearest point of the new path, so it
/// jumps no further than the path did. A non-finite coordinate or terminal
/// returns the base flow unchanged.
pub(crate) fn offset_segment<O: Obstacles + ?Sized>(
    flow: &Flow,
    segment_index: usize,
    coordinate: f64,
    terminals: &Terminals,
    stocks: &O,
) -> FlowGeometry {
    let pts = path_of(flow);
    if segment_index + 1 >= pts.len()
        || !coordinate.is_finite()
        || !terminals.source.is_finite()
        || !terminals.sink.is_finite()
        || !pts.iter().all(|p| p.is_finite())
        || pts[segment_index].same(pts[segment_index + 1])
    {
        return FlowGeometry::unchanged(flow);
    }
    let i = segment_index;
    let tails = Tails {
        source: resolvable_tail(&pts, i, FlowEnd::Source, terminals),
        sink: resolvable_tail(&pts, i, FlowEnd::Sink, terminals),
    };
    let constraints = adjacent_constraints(&pts, i, terminals, &tails);
    let resolve = |c: f64| resolve_coordinate(c, &constraints);
    // Validity is judged on the normalized path: a riser collapsed to zero
    // length is a corner removed, not a zero-length segment.
    let build = |c: f64| -> Path { normalize(&build_offset(&pts, i, c, &tails)) };
    let clear = |points: &[Point]| {
        let quality = path_quality(points, terminals, stocks, stocks);
        quality.fault == Fault::None && !quality.obstructed
    };
    let valid = |c: f64| clear(&build(c));
    let mut c = resolve(coordinate);
    if !valid(c) && clear(&pts) {
        c = nearest_valid(segment_hold(&pts, i).1, c, &resolve, &valid);
    }
    let points = build(c);
    let valve = offset_valve(flow, &pts, i, &points, effective_hold(c, &tails));
    with_geometry(flow, &points, valve, terminals)
}

/// The valid coordinate nearest `request`, looking back toward the (valid) base
/// and past the obstacle, each side found by bisection on the resolved
/// coordinate.
fn nearest_valid(
    base_hold: f64,
    request: f64,
    resolve: &impl Fn(f64) -> f64,
    valid: &impl Fn(f64) -> bool,
) -> f64 {
    let bisect = |mut good: f64, mut bad: f64| {
        for _ in 0..32 {
            let mid = (good + bad) / 2.0;
            if valid(resolve(mid)) {
                good = mid;
            } else {
                bad = mid;
            }
        }
        resolve(good)
    };
    let back = bisect(base_hold, request);
    let direction = if request > base_hold {
        1.0
    } else if request < base_hold {
        -1.0
    } else {
        1.0
    };
    let mut step = OBSTACLE_STEP;
    while step <= OBSTACLE_SCAN {
        let probe = request + direction * step;
        if valid(resolve(probe)) {
            let across = bisect(probe, request);
            return if (across - request).abs() < (back - request).abs() {
                across
            } else {
                back
            };
        }
        step += OBSTACLE_STEP;
    }
    back
}

/// The faces whose tails `offset_segment` re-solves, per end.
struct Tails {
    source: Option<FaceAttachment>,
    sink: Option<FaceAttachment>,
}

/// The face whose tail `offset_segment` re-solves at `end`, or `None` when the
/// tail is kept. A stock tail is re-solved when the dragged segment is the one
/// leaving the face, or when it follows a stub (at most the end's minimum long)
/// and a riser.
fn resolvable_tail(
    pts: &[Point],
    i: usize,
    end: FlowEnd,
    terminals: &Terminals,
) -> Option<FaceAttachment> {
    let (t, endpoint, adjacent, from_end, minimum) = match end {
        FlowEnd::Source => (&terminals.source, pts[0], pts[1], i, MIN_SEGMENT),
        FlowEnd::Sink => {
            let n = pts.len();
            (
                &terminals.sink,
                pts[n - 1],
                pts[n - 2],
                n - 2 - i,
                MIN_SINK_SEGMENT,
            )
        }
    };
    let Terminal::Stock { center, .. } = t else {
        return None;
    };
    let face = face_of_endpoint(*center, endpoint, Some(adjacent))?;
    let att = face_attachment(*center, face);
    if Axis::of_segment(pts[i], pts[i + 1]) != att.normal {
        return None;
    }
    let resolvable = from_end == 0
        || (from_end == 2 && endpoint.distance(adjacent) <= minimum + GEOMETRY_EPSILON);
    resolvable.then_some(att)
}

/// The hold a re-solved stock tail gives the dragged segment: the coordinate
/// itself within the face extent, the extent while within `MIN_SEGMENT` beyond
/// it, and the coordinate again past that (where a stub and riser appear).
fn tail_hold(att: &FaceAttachment, c: f64) -> f64 {
    if c > att.hi && c <= att.hi + MIN_SEGMENT {
        att.hi
    } else if c < att.lo && c >= att.lo - MIN_SEGMENT {
        att.lo
    } else {
        c
    }
}

/// The hold the dragged segment actually takes for coordinate `c`, after each
/// re-solved tail's extent band.
fn effective_hold(c: f64, tails: &Tails) -> f64 {
    let mut hold = c;
    if let Some(att) = &tails.source {
        hold = tail_hold(att, hold);
    }
    if let Some(att) = &tails.sink {
        hold = tail_hold(att, hold);
    }
    hold
}

#[derive(Default)]
struct Constraints {
    /// The coordinate must be at least `from` in direction `sign` (a stock
    /// stub's minimum).
    halves: SmallVec<[(f64, f64); 2]>,
    /// The coordinate must be at `t` (collapsed) or at least `m` from it (a
    /// riser or cloud segment).
    aways: SmallVec<[(f64, f64); 2]>,
}

fn adjacent_constraints(
    pts: &[Point],
    i: usize,
    terminals: &Terminals,
    tails: &Tails,
) -> Constraints {
    let n = pts.len();
    let last = n - 2;
    let (axis, _) = segment_hold(pts, i);
    let h = axis.other();
    let mut out = Constraints::default();
    let mut adjacent = |neighbor: usize, fixed_index: usize, end: Option<FlowEnd>| {
        let minimum = if end == Some(FlowEnd::Sink) {
            MIN_SINK_SEGMENT
        } else {
            MIN_SEGMENT
        };
        let t = match end {
            Some(FlowEnd::Source) => Some(&terminals.source),
            Some(FlowEnd::Sink) => Some(&terminals.sink),
            None => None,
        };
        if let Some(Terminal::Stock { center, .. }) = t
            && let Some(face) = face_of_endpoint(*center, pts[fixed_index], Some(pts[neighbor]))
        {
            let att = face_attachment(*center, face);
            out.halves.push((stub_tip(&att, minimum), att.sign));
            return;
        }
        out.aways.push((pts[fixed_index].coord(h), minimum));
    };
    if tails.source.is_none() && i >= 1 {
        adjacent(i, i - 1, (i - 1 == 0).then_some(FlowEnd::Source));
    }
    if tails.sink.is_none() && i < last {
        adjacent(i + 1, i + 2, (i + 1 == last).then_some(FlowEnd::Sink));
    }
    out
}

/// The coordinate nearest `c` satisfying every constraint. The candidates are
/// `c` clamped by the half-lines and each away constraint's collapse point and
/// its two minimum positions; when none satisfies everything (the constraints
/// contradict), the clamped request is returned and validity rejects the path.
fn resolve_coordinate(c: f64, constraints: &Constraints) -> f64 {
    let e = GEOMETRY_EPSILON;
    let mut clamped = c;
    for &(from, sign) in &constraints.halves {
        clamped = if sign > 0.0 {
            clamped.max(from)
        } else {
            clamped.min(from)
        };
    }
    let satisfies = |v: f64| {
        constraints
            .halves
            .iter()
            .all(|&(from, sign)| sign * (v - from) >= -e)
            && constraints
                .aways
                .iter()
                .all(|&(t, m)| (v - t).abs() <= e || (v - t).abs() >= m - e)
    };
    if satisfies(clamped) {
        return clamped;
    }
    let mut best: Option<f64> = None;
    for &(t, m) in &constraints.aways {
        for v in [t, t - m, t + m] {
            if satisfies(v) && best.is_none_or(|b| (v - c).abs() < (b - c).abs()) {
                best = Some(v);
            }
        }
    }
    best.unwrap_or(clamped)
}

/// The path with segment `i` at hold `c`. A free terminal's endpoint on the
/// dragged segment moves with it (its cloud follows the endpoint); a re-solved
/// stock tail is rebuilt from the face attachment.
fn build_offset(pts: &[Point], i: usize, c: f64, tails: &Tails) -> Path {
    let n = pts.len();
    let last = n - 2;
    let (axis, _) = segment_hold(pts, i);
    let hold = effective_hold(c, tails);
    let mut out = Path::new();
    let u = if let Some(att) = &tails.source {
        let along = clamp(hold, att.lo, att.hi);
        let endpoint = att.along.compose(along, att.plane);
        if (along - hold).abs() <= GEOMETRY_EPSILON {
            endpoint
        } else {
            let tip = stub_tip(att, MIN_SEGMENT);
            out.push(endpoint);
            out.push(att.normal.compose(tip, along));
            att.normal.compose(tip, hold)
        }
    } else if i == 0 {
        axis.compose(pts[0].coord(axis), hold)
    } else {
        out.extend_from_slice(&pts[..i]);
        axis.compose(pts[i].coord(axis), hold)
    };
    out.push(u);
    if let Some(att) = &tails.sink {
        let along = clamp(hold, att.lo, att.hi);
        let endpoint = att.along.compose(along, att.plane);
        if (along - hold).abs() <= GEOMETRY_EPSILON {
            out.push(endpoint);
        } else {
            let tip = stub_tip(att, MIN_SINK_SEGMENT);
            out.push(att.normal.compose(tip, hold));
            out.push(att.normal.compose(tip, along));
            out.push(endpoint);
        }
    } else if i == last {
        out.push(axis.compose(pts[n - 1].coord(axis), hold));
    } else {
        out.push(axis.compose(pts[i + 1].coord(axis), hold));
        out.extend_from_slice(&pts[i + 2..]);
    }
    out
}

/// The valve after an offset. Its base segment j survives when the new path
/// has a segment on the same axis holding the same coordinate (for j = i, the
/// new hold) whose span overlaps j's: the valve keeps its coordinate along that
/// axis, clamped into the surviving segment's span. Otherwise it moves to the
/// nearest point of the new path.
fn offset_valve(base: &Flow, pts: &[Point], i: usize, points: &[Point], c: f64) -> Point {
    let valve = Point::new(base.x, base.y);
    if !valve.is_finite() {
        return place_valve(points, FlowEnd::Source, None);
    }
    let s0 = arc_position(pts, valve);
    let mut start = 0.0;
    let mut j = 0;
    while j + 2 < pts.len() {
        let length = pts[j].distance(pts[j + 1]);
        if s0 <= start + length {
            break;
        }
        start += length;
        j += 1;
    }
    let (axis, hold) = segment_hold(pts, j);
    let new_hold = if j == i { c } else { hold };
    let along = valve.coord(axis);
    let span_lo = pts[j].coord(axis).min(pts[j + 1].coord(axis));
    let span_hi = pts[j].coord(axis).max(pts[j + 1].coord(axis));
    let mut best: Option<Point> = None;
    let mut best_gap = f64::INFINITY;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        if a.same(b)
            || Axis::of_segment(a, b) != axis
            || (a.coord(axis.other()) - new_hold).abs() > GEOMETRY_EPSILON
        {
            continue;
        }
        let lo = a.coord(axis).min(b.coord(axis));
        let hi = a.coord(axis).max(b.coord(axis));
        if hi.min(span_hi) - lo.max(span_lo) < -GEOMETRY_EPSILON {
            continue;
        }
        let at = clamp(along, lo, hi);
        let gap = (at - along).abs();
        if gap < best_gap {
            best_gap = gap;
            best = Some(axis.compose(at, new_hold));
        }
    }
    let at = best.unwrap_or(valve);
    point_at_arc(
        points,
        apply_valve_margin(path_length(points), arc_position(points, at)),
    )
}
