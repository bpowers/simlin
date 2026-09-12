// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Paths and the valve: normalization, arc length, the valve policy (an
//! arc-length position measured from an end, margin applied once), translation
//! and valve sliding.

use smallvec::SmallVec;

use crate::datamodel::view_element::Flow;

use super::geometry::{FlowEnd, GEOMETRY_EPSILON, Point, Positioned, VALVE_CLAMP_MARGIN, clamp};

/// Remove zero-length segments and collinear interior points (G3's structural
/// arms). Endpoints are always kept: they carry the attachments. Removing one
/// point can make its neighbors collinear, so removal repeats to a fixed point.
/// Inputs are orthogonal (heal snaps diagonals first), where a zero-length
/// segment is always also collinear with its neighbor, so one test covers both.
pub(crate) fn normalize<T: Positioned + Clone>(points: &[T]) -> SmallVec<[T; 12]> {
    let mut current: SmallVec<[T; 12]> = points.iter().cloned().collect();
    loop {
        if current.len() <= 2 {
            return current;
        }
        let before = current.len();
        let e = GEOMETRY_EPSILON;
        let mut kept: SmallVec<[T; 12]> = SmallVec::with_capacity(before);
        kept.push(current[0].clone());
        for i in 1..before - 1 {
            let prev = kept[kept.len() - 1].point();
            let curr = current[i].point();
            let next = current[i + 1].point();
            let horizontal = (prev.y - curr.y).abs() <= e && (curr.y - next.y).abs() <= e;
            let vertical = (prev.x - curr.x).abs() <= e && (curr.x - next.x).abs() <= e;
            if !(horizontal || vertical) {
                kept.push(current[i].clone());
            }
        }
        kept.push(current[before - 1].clone());
        if kept.len() == before {
            return kept;
        }
        current = kept;
    }
}

pub(crate) fn path_length<T: Positioned>(points: &[T]) -> f64 {
    points
        .windows(2)
        .map(|w| w[0].point().distance(w[1].point()))
        .sum()
}

/// The parameter along segment `a`-`b` of the point nearest `p`, in `[0, 1]`.
fn nearest_t(a: Point, b: Point, p: Point) -> f64 {
    let length = a.distance(b);
    if length == 0.0 {
        return 0.0;
    }
    clamp(
        ((p.x - a.x) * (b.x - a.x) + (p.y - a.y) * (b.y - a.y)) / (length * length),
        0.0,
        1.0,
    )
}

/// The arc-length position of the point on the path nearest `p` (the earliest
/// one on a tie).
pub(crate) fn arc_position<T: Positioned>(points: &[T], p: Point) -> f64 {
    let mut best = f64::INFINITY;
    let mut position = 0.0;
    let mut traversed = 0.0;
    for w in points.windows(2) {
        let (a, b) = (w[0].point(), w[1].point());
        let length = a.distance(b);
        let t = nearest_t(a, b, p);
        let d = (p.x - (a.x + t * (b.x - a.x))).hypot(p.y - (a.y + t * (b.y - a.y)));
        if d < best - GEOMETRY_EPSILON {
            best = d;
            position = traversed + t * length;
        }
        traversed += length;
    }
    position
}

/// The distance from `p` to the nearest point on the path.
pub(crate) fn distance_to_path<T: Positioned>(points: &[T], p: Point) -> f64 {
    points
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0].point(), w[1].point());
            let t = nearest_t(a, b, p);
            (p.x - (a.x + t * (b.x - a.x))).hypot(p.y - (a.y + t * (b.y - a.y)))
        })
        .fold(f64::INFINITY, f64::min)
}

/// The point at arc length `s` along the path, clamped to the path. An empty
/// path has no points to return one of, and yields NaN coordinates, which
/// every caller's finiteness check rejects.
pub(crate) fn point_at_arc<T: Positioned>(points: &[T], s: f64) -> Point {
    let Some(first) = points.first() else {
        return Point::new(f64::NAN, f64::NAN);
    };
    if points.len() == 1 || s <= 0.0 {
        return first.point();
    }
    let mut remaining = s;
    for w in points.windows(2) {
        let (a, b) = (w[0].point(), w[1].point());
        let length = a.distance(b);
        if remaining <= length {
            let t = if length == 0.0 {
                0.0
            } else {
                remaining / length
            };
            return Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
        }
        remaining -= length;
    }
    points[points.len() - 1].point()
}

/// The valve's arc-length distance from `from` on the base path, or `None` when
/// the base path has no length (a creation draft), which places the valve at
/// the midpoint of whatever path is routed. The margin is not applied here.
pub(crate) fn valve_distance<T: Positioned>(
    points: &[T],
    valve: Point,
    from: FlowEnd,
) -> Option<f64> {
    let length = path_length(points);
    if length <= GEOMETRY_EPSILON || !valve.is_finite() {
        return None;
    }
    let s = arc_position(points, valve);
    Some(match from {
        FlowEnd::Source => s,
        FlowEnd::Sink => length - s,
    })
}

/// Place the valve `distance` along the path from `from`, clamped to the path,
/// then apply `VALVE_CLAMP_MARGIN`. The margin is applied once, here, at the
/// end: applying it while measuring would move a valve that sat inside the
/// margin on the base path further inward whenever the path grows at the far
/// end.
pub(crate) fn place_valve(points: &[Point], from: FlowEnd, distance: Option<f64>) -> Point {
    let length = path_length(points);
    let s = match distance {
        None => length / 2.0,
        Some(d) => clamp(
            match from {
                FlowEnd::Source => d,
                FlowEnd::Sink => length - d,
            },
            0.0,
            length,
        ),
    };
    point_at_arc(points, apply_valve_margin(length, s))
}

/// G8's margin. At exactly two margins long the only valid position is the
/// midpoint, which the clamp also produces, so the `<` boundary is a choice
/// without consequence.
pub(crate) fn apply_valve_margin(length: f64, s: f64) -> f64 {
    if length < 2.0 * VALVE_CLAMP_MARGIN {
        return length / 2.0;
    }
    clamp(s, VALVE_CLAMP_MARGIN, length - VALVE_CLAMP_MARGIN)
}

/// Move a flow whose two terminals both move by `delta`: every point and the
/// valve translate. The caller moves the terminal elements, so there are no
/// clouds to report. A non-finite delta returns the flow unchanged.
pub(crate) fn translate(flow: &Flow, delta: Point) -> Flow {
    let mut next = flow.clone();
    if !delta.is_finite() {
        return next;
    }
    next.x += delta.x;
    next.y += delta.y;
    for p in &mut next.points {
        p.x += delta.x;
        p.y += delta.y;
    }
    next
}

/// One segment of positive length, for sliding along the path.
#[derive(Clone, Copy)]
struct Segment {
    start: f64,
    length: f64,
    tx: f64,
    ty: f64,
}

/// Slide the valve along the path by the pointer delta projected onto the path.
///
/// The delta is applied as if the pointer traveled straight from the press: on
/// each segment the valve moves at the rate the delta projects onto that
/// segment's direction, and when it reaches a corner it continues onto the
/// next segment with the time that is left, if the delta projects forward along
/// it (otherwise it rests at the corner). Carrying the remaining TIME rather
/// than the remaining vector is what keeps this continuous: a component
/// perpendicular to the valve's segment is never banked and released all at
/// once at a corner. The valve crosses corners instead of hopping to whichever
/// segment is nearest; it lags the pointer at a corner by design, and `delta`
/// is measured from the press, so the grab offset is kept. The path changes
/// nothing a cloud sits on, so there are no clouds to report.
pub(crate) fn slide_valve(flow: &Flow, delta: Point) -> Flow {
    let pts = &flow.points;
    let mut segments: SmallVec<[Segment; 12]> = SmallVec::new();
    let mut traversed = 0.0;
    for w in pts.windows(2) {
        let (a, b) = (w[0].point(), w[1].point());
        let length = a.distance(b);
        if length > GEOMETRY_EPSILON {
            segments.push(Segment {
                start: traversed,
                length,
                tx: (b.x - a.x) / length,
                ty: (b.y - a.y) / length,
            });
        }
        traversed += length;
    }
    if segments.is_empty() || !delta.is_finite() || !pts.iter().all(|p| p.point().is_finite()) {
        return flow.clone();
    }
    let total = traversed;
    let valve = Point::new(flow.x, flow.y);
    let mut pos = if valve.is_finite() {
        arc_position(pts, valve)
    } else {
        total / 2.0
    };
    let rate = |j: usize| delta.x * segments[j].tx + delta.y * segments[j].ty;
    let mut j = segments
        .iter()
        .position(|seg| pos <= seg.start + seg.length)
        .unwrap_or(segments.len() - 1);
    // A valve exactly on a corner belongs to whichever adjacent segment the
    // delta moves it along.
    if rate(j).abs() <= GEOMETRY_EPSILON
        && j + 1 < segments.len()
        && pos >= segments[j + 1].start - GEOMETRY_EPSILON
    {
        j += 1;
    }
    let mut time = 1.0;
    while time > 0.0 {
        let seg = segments[j];
        let v = rate(j);
        // Only a neighbor the delta still moves the valve along is entered, so
        // the valve travels one way and the loop ends within the segment count.
        if v > GEOMETRY_EPSILON {
            let need = (seg.start + seg.length - pos) / v;
            if need >= time || j + 1 >= segments.len() || rate(j + 1) <= GEOMETRY_EPSILON {
                pos = (pos + v * time).min(seg.start + seg.length);
                break;
            }
            pos = seg.start + seg.length;
            time -= need;
            j += 1;
        } else if v < -GEOMETRY_EPSILON {
            let need = (pos - seg.start) / -v;
            if need >= time || j == 0 || rate(j - 1) >= -GEOMETRY_EPSILON {
                pos = (pos + v * time).max(seg.start);
                break;
            }
            pos = seg.start;
            time -= need;
            j -= 1;
        } else {
            break;
        }
    }
    let at = point_at_arc(pts, apply_valve_margin(total, pos));
    let mut next = flow.clone();
    next.x = at.x;
    next.y = at.y;
    next
}
