// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Where a flow pipe meets the elements it connects.
//!
//! Every renderer -- this crate's `diagram`, the TypeScript editor -- draws a
//! stock as a `STOCK_WIDTH` x `STOCK_HEIGHT` box at its center, whatever size
//! the producing tool drew it, and a cloud as a glyph centered on its point.
//! The geometry of a flow is judged against those shapes, at the units of the
//! editor's design plan (`docs/design-plans/2026-09-10-diagram-editing-core.md`,
//! "Units" and "Invariants"). The standing invariants of a flow's points:
//!
//! - every segment is axis-aligned (XMILE 1.0 section 6.1.2, `pts`: "Flows can
//!   have any arbitrary number of points, but those points MUST form right
//!   angles", `docs/reference/xmile-v1.0.html`);
//! - no segment has zero length and no two consecutive segments are collinear;
//!   whenever the terminals leave room (`Minima`), the first segment is at
//!   least `MIN_SEGMENT`, every interior segment at least `MIN_SEGMENT`, and the
//!   final segment at least `MIN_SINK_SEGMENT`;
//! - the first and last points are attached (to a stock, or to a cloud owned by
//!   the flow) and interior points are not;
//! - a stock endpoint lies on a face with at least `CORNER_CLEARANCE` from the
//!   corners, and the adjacent segment is perpendicular to that face and leaves
//!   outward, so no segment runs through the stock's body;
//! - a cloud endpoint equals the cloud's center;
//! - the valve (the flow's `x`, `y`) lies on the pipe, at least
//!   `VALVE_CLAMP_MARGIN` from the path's ends by arc length when the path is at
//!   least twice that long.
//!
//! `normalize_flow_geometry` is the one pass that establishes them, run by the
//! importers and as the layout's finishing pass, and `clamp_to_face_span` is
//! the one statement of the corner clearance the layout's endpoint placement
//! also reads.

use std::collections::{HashMap, HashSet};

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::FlowPoint;

use super::constants::{FLOW_ARROWHEAD_RADIUS, STOCK_HEIGHT, STOCK_WIDTH};

/// The minimum distance between a stock endpoint and the nearest corner of
/// the face it sits on. A pipe drawn into a corner reads as attached to two
/// faces at once, and its arrowhead overlaps the stock's outline.
pub(crate) const CORNER_CLEARANCE: f64 = 3.0;

/// The shortest first segment (a stub out of the source) or interior segment
/// (a riser) a pipe is held to when its terminals leave room.
pub(crate) const MIN_SEGMENT: f64 = 10.0;

/// The shortest final segment a pipe is held to when its terminals leave room:
/// the arrowhead's radius plus 7.5, so the arrowhead does not swallow it.
pub(crate) const MIN_SINK_SEGMENT: f64 = FLOW_ARROWHEAD_RADIUS + 7.5;

/// How far the valve is kept from the path's ends, by arc length, when the
/// path is at least twice this long.
pub(crate) const VALVE_CLAMP_MARGIN: f64 = 10.0;

/// The distance a flow that newly lands on a face keeps from the ends already
/// there, where the face has room: the design plan's routing preference ("a
/// flow that newly lands on a face takes a slot at least `PIPE_SPACING` from
/// existing endpoints on that face if one exists, else the slot maximizing
/// the minimum distance"). A preference of the layout's placement, not an
/// invariant this module establishes.
pub(crate) const PIPE_SPACING: f64 = 10.0;

/// The segment minimum when the terminals crowd each other. G3 demands no
/// minimum then, so no segment is short for the checker; this is only the
/// shortest segment an attach arm builds, so that no arm leaves a segment of
/// no length, and the length under which `collapse_short_segment` still
/// removes a segment that keeps a stock end off its face.
const CROWDED_MIN_SEGMENT: f64 = 1.0;

/// How many passes a flow gets to reach a fixed point. A pass can leave what
/// the next one removes -- a slide that shortens a riser the collapse then
/// merges -- so passes repeat until one changes nothing; the bound keeps a
/// pathological input from looping.
const MAX_PASSES: usize = 8;

/// Tolerance for "same coordinate" and "on the face" comparisons.
const EPS: f64 = 1e-6;

const HALF_W: f64 = STOCK_WIDTH / 2.0;
const HALF_H: f64 = STOCK_HEIGHT / 2.0;

/// `v` clamped to the span of a stock face, keeping `CORNER_CLEARANCE` from
/// both corners. `center` is the stock center's coordinate along the face and
/// `half_extent` the stock's half-size in that direction (`STOCK_WIDTH / 2`
/// for the top and bottom faces, `STOCK_HEIGHT / 2` for the left and right).
pub(crate) fn clamp_to_face_span(v: f64, center: f64, half_extent: f64) -> f64 {
    let reach = half_extent - CORNER_CLEARANCE;
    v.clamp(center - reach, center + reach)
}

/// The segment minima one flow is held to (G3): `MIN_SEGMENT` for the stub
/// and every riser and `MIN_SINK_SEGMENT` for the final segment, when the
/// terminals leave room -- the source body inflated by `MIN_SEGMENT` and the
/// sink body inflated by `MIN_SINK_SEGMENT` do not overlap, a cloud counting
/// as its point and an unattached end as no terminal at all -- and
/// `CROWDED_MIN_SEGMENT` otherwise. `room` records which: only with room does
/// the checker hold a segment to a minimum.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy)]
struct Minima {
    stub: f64,
    riser: f64,
    sink: f64,
    room: bool,
}

impl Minima {
    fn of(points: &[FlowPoint], stocks: &HashMap<i32, (f64, f64)>) -> Minima {
        if terminals_leave_room(points, stocks) {
            Minima {
                stub: MIN_SEGMENT,
                riser: MIN_SEGMENT,
                sink: MIN_SINK_SEGMENT,
                room: true,
            }
        } else {
            Minima {
                stub: CROWDED_MIN_SEGMENT,
                riser: CROWDED_MIN_SEGMENT,
                sink: CROWDED_MIN_SEGMENT,
                room: false,
            }
        }
    }

    /// The minimum of segment `i` of an `n`-segment pipe: the final segment is
    /// the sink's (a one-segment pipe's included), the first the stub, any
    /// other a riser.
    fn segment(&self, i: usize, n: usize) -> f64 {
        if i + 1 == n {
            self.sink
        } else if i == 0 {
            self.stub
        } else {
            self.riser
        }
    }
}

fn terminals_leave_room(points: &[FlowPoint], stocks: &HashMap<i32, (f64, f64)>) -> bool {
    terminals_clear(points, stocks, MIN_SEGMENT, MIN_SINK_SEGMENT)
}

/// Whether the terminal bodies of `points` -- the source's inflated by
/// `source_inflate`, the sink's by `sink_inflate` -- do not overlap, a cloud
/// counting as its point and an unattached end as no terminal at all.
fn terminals_clear(
    points: &[FlowPoint],
    stocks: &HashMap<i32, (f64, f64)>,
    source_inflate: f64,
    sink_inflate: f64,
) -> bool {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return true;
    };
    if first.attached_to_uid.is_none() || last.attached_to_uid.is_none() {
        return true;
    }
    let body = |p: &FlowPoint, inflate: f64| -> (f64, f64, f64, f64) {
        let (x, y, hw, hh) = match p.attached_to_uid.and_then(|uid| stocks.get(&uid)) {
            Some(&(sx, sy)) => (sx, sy, HALF_W, HALF_H),
            None => (p.x, p.y, 0.0, 0.0),
        };
        (
            x - hw - inflate,
            y - hh - inflate,
            x + hw + inflate,
            y + hh + inflate,
        )
    };
    let a = body(first, source_inflate);
    let b = body(last, sink_inflate);
    !(a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3)
}

/// The orientation of an axis-aligned segment.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    fn of_segment(a: &FlowPoint, b: &FlowPoint) -> Option<Axis> {
        let same_y = (a.y - b.y).abs() <= EPS;
        let same_x = (a.x - b.x).abs() <= EPS;
        match (same_x, same_y) {
            (false, true) => Some(Axis::Horizontal),
            (true, false) => Some(Axis::Vertical),
            _ => None,
        }
    }

    /// The coordinate that varies along a segment of this orientation.
    fn along(self, x: f64, y: f64) -> f64 {
        match self {
            Axis::Horizontal => x,
            Axis::Vertical => y,
        }
    }

    /// The coordinate that is fixed along a segment of this orientation.
    fn cross(self, x: f64, y: f64) -> f64 {
        match self {
            Axis::Horizontal => y,
            Axis::Vertical => x,
        }
    }

    fn point(self, along: f64, cross: f64, attached_to_uid: Option<i32>) -> FlowPoint {
        let (x, y) = match self {
            Axis::Horizontal => (along, cross),
            Axis::Vertical => (cross, along),
        };
        FlowPoint {
            x,
            y,
            attached_to_uid,
        }
    }

    fn set_cross(self, p: &mut FlowPoint, cross: f64) {
        match self {
            Axis::Horizontal => p.y = cross,
            Axis::Vertical => p.x = cross,
        }
    }

    fn set_along(self, p: &mut FlowPoint, along: f64) {
        match self {
            Axis::Horizontal => p.x = along,
            Axis::Vertical => p.y = along,
        }
    }

    /// The stock's half-size along a segment of this orientation: the
    /// distance from the center to the face such a segment meets
    /// perpendicularly.
    fn half_along(self) -> f64 {
        match self {
            Axis::Horizontal => HALF_W,
            Axis::Vertical => HALF_H,
        }
    }

    fn half_cross(self) -> f64 {
        match self {
            Axis::Horizontal => HALF_H,
            Axis::Vertical => HALF_W,
        }
    }
}

/// Whether a stock endpoint `p` with adjacent point `q` already satisfies the
/// invariants: on a face with corner clearance, and the segment to `q`
/// perpendicular to that face and leaving outward.
fn stock_endpoint_is_valid(p: &FlowPoint, q: &FlowPoint, stock: (f64, f64)) -> bool {
    let dx = p.x - stock.0;
    let dy = p.y - stock.1;
    let on_side = (dx.abs() - HALF_W).abs() <= EPS && dy.abs() <= HALF_H - CORNER_CLEARANCE + EPS;
    let on_cap = (dy.abs() - HALF_H).abs() <= EPS && dx.abs() <= HALF_W - CORNER_CLEARANCE + EPS;
    if on_side {
        (q.y - p.y).abs() <= EPS && dx.signum() * (q.x - p.x) > EPS
    } else if on_cap {
        (q.x - p.x).abs() <= EPS && dy.signum() * (q.y - p.y) > EPS
    } else {
        false
    }
}

/// Whether every stock endpoint of `points` is valid.
fn stock_ends_valid(points: &[FlowPoint], stocks: &HashMap<i32, (f64, f64)>) -> bool {
    let n = points.len();
    if n < 2 {
        return false;
    }
    [(0, 1), (n - 1, n - 2)].iter().all(|&(idx, adj)| {
        match points[idx].attached_to_uid.and_then(|uid| stocks.get(&uid)) {
            Some(&stock) => stock_endpoint_is_valid(&points[idx], &points[adj], stock),
            None => true,
        }
    })
}

/// How an end segment's stock endpoint is brought onto the stock.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
enum EndFix {
    /// Already valid: the endpoint pins the segment's line where it is. Only
    /// when neither a shared line nor a jog can bring the segment's other end
    /// in does it give ground, sliding the least it can within its own
    /// clearance span, where it stays valid.
    Keep,
    /// The line passes within the end's minimum segment of the face it
    /// approaches, so the pipe enters that face: the line slides into the
    /// face's clearance span (never further than that margin) and the endpoint
    /// moves along the line onto the face.
    Face,
    /// The line misses the face by more than that: the line stays, the
    /// endpoint becomes a bend above the stock, and a perpendicular leg runs
    /// into the face the line runs past. A leg the shared line leaves shorter
    /// than the end's minimum segment is removed on the next pass by
    /// `collapse_short_segment`.
    Leg,
    /// A `Face` end whose clearance span excludes the line the segment's
    /// other end needs (a valid slot pins it, or the two spans are disjoint):
    /// the pipe steps from the shared line to this end's own line (the value)
    /// the end's minimum segment short of the face, then enters the face.
    Jog(f64),
}

/// The lines every `Keep` and `Face` end of a segment can live with, except
/// the end at `skip`. A `Keep` end pins the line where it is, or, with
/// `relax_valid`, accepts any line in its own clearance span (it stays valid
/// there). `Leg` ends constrain nothing here: a leg is built from whatever
/// line the others settle on.
fn shared_line_span(
    ends: &[StockEnd],
    axis: Axis,
    line: f64,
    skip: Option<usize>,
    relax_valid: bool,
) -> (f64, f64) {
    let reach = axis.half_cross() - CORNER_CLEARANCE;
    let (mut lo, mut hi) = (f64::NEG_INFINITY, f64::INFINITY);
    for (i, end) in ends.iter().enumerate() {
        if Some(i) == skip {
            continue;
        }
        let s_cross = axis.cross(end.stock.0, end.stock.1);
        match end.fix {
            EndFix::Keep if !relax_valid => {
                lo = lo.max(line);
                hi = hi.min(line);
            }
            EndFix::Keep | EndFix::Face => {
                lo = lo.max(s_cross - reach);
                hi = hi.min(s_cross + reach);
            }
            EndFix::Leg | EndFix::Jog(_) => {}
        }
    }
    (lo, hi)
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
struct StockEnd {
    idx: usize,
    adj: usize,
    stock: (f64, f64),
    fix: EndFix,
    /// The minimum length of the segment this end's fix creates into the face:
    /// the stub's for the source, the sink's for the sink.
    min: f64,
}

fn point_segment_distance(p: (f64, f64), a: &FlowPoint, b: &FlowPoint) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 == 0.0 {
        return (p.0 - a.x).hypot(p.1 - a.y);
    }
    let t = (((p.0 - a.x) * dx + (p.1 - a.y) * dy) / len2).clamp(0.0, 1.0);
    (p.0 - (a.x + t * dx)).hypot(p.1 - (a.y + t * dy))
}

/// Bring the stock endpoints of one end segment of a pipe onto their stocks.
///
/// The segment is `points[0..=1]` when `source`, `points[n-2..=n-1]` when
/// `sink`, and the whole pipe when both (a two-point flow, whose two ends share
/// one line). Works on a copy, and commits the result unless it runs more
/// segments through the terminal stocks' bodies than the pipe did, where G6
/// holds (`adds_a_body_crossing`); an end segment
/// that is diagonal, or whose ends ask for incompatible lines, is left as it
/// was. The valve is not read: `project_valve_onto_pipe`, which runs after
/// the passes, owns where it ends up.
fn attach_end_segment(
    points: &mut Vec<FlowPoint>,
    stocks: &HashMap<i32, (f64, f64)>,
    minima: &Minima,
    source: bool,
    sink: bool,
) {
    let n = points.len();
    let (a, b) = if source { (0, 1) } else { (n - 2, n - 1) };
    let Some(axis) = Axis::of_segment(&points[a], &points[b]) else {
        return;
    };
    let line = axis.cross(points[a].x, points[a].y);
    let half_cross = axis.half_cross();

    let mut ends: Vec<StockEnd> = Vec::new();
    for (enabled, idx, adj, min) in [
        (source, 0, 1, minima.stub),
        (sink, n - 1, n - 2, minima.sink),
    ] {
        if !enabled {
            continue;
        }
        let Some(&stock) = points[idx].attached_to_uid.and_then(|uid| stocks.get(&uid)) else {
            continue;
        };
        let fix = if stock_endpoint_is_valid(&points[idx], &points[adj], stock) {
            EndFix::Keep
        } else if (line - axis.cross(stock.0, stock.1)).abs() < half_cross + min {
            EndFix::Face
        } else {
            EndFix::Leg
        };
        ends.push(StockEnd {
            idx,
            adj,
            stock,
            fix,
            min,
        });
    }
    if ends.iter().all(|e| e.fix == EndFix::Keep) {
        return;
    }

    // In order of preference: one line every end lives with (valid slots
    // pinned); a jog for one `Face` end, keeping the others' line; and, when
    // no jog fits, the least slide of a valid slot within its own clearance
    // span. The last moves authored geometry, so it comes last.
    let (lo, hi) = shared_line_span(&ends, axis, line, None, false);
    let new_line = if lo <= hi + EPS {
        line.clamp(lo, hi.max(lo))
    } else {
        let reach = half_cross - CORNER_CLEARANCE;
        let mut jog = None;
        for (i, end) in ends.iter().enumerate() {
            if end.fix != EndFix::Face {
                continue;
            }
            let (olo, ohi) = shared_line_span(&ends, axis, line, Some(i), false);
            if olo > ohi + EPS {
                continue;
            }
            let shared = line.clamp(olo, ohi.max(olo));
            let s_cross = axis.cross(end.stock.0, end.stock.1);
            let own = shared.clamp(s_cross - reach, s_cross + reach);
            if (shared - own).abs() >= minima.riser - EPS {
                jog = Some((i, shared, own));
                break;
            }
        }
        if let Some((i, shared, own)) = jog {
            ends[i].fix = EndFix::Jog(own);
            shared
        } else {
            let (rlo, rhi) = shared_line_span(&ends, axis, line, None, true);
            if rlo > rhi + EPS {
                return;
            }
            line.clamp(rlo, rhi.max(rlo))
        }
    };

    let mut pts = points.clone();
    if (new_line - line).abs() > EPS {
        // The slide moves the segment perpendicular to itself, so each
        // interior neighbour lengthens or shortens along its own axis. Nothing
        // is refused for what that does to a neighbour: one shortened under a
        // riser is merged by the collapse, and one folded back over its far
        // point is a spur the next pass's `simplify` removes. No neighbour is
        // collinear with the segment or of no length, which a slide would pull
        // off its axis: a pass starts from the simplified pipe. A diagonal
        // neighbour stays diagonal, the producer's to resolve.
        axis.set_cross(&mut pts[a], new_line);
        axis.set_cross(&mut pts[b], new_line);
    }

    // Highest index first, so the sink's insertion cannot shift the source.
    ends.sort_by_key(|end| std::cmp::Reverse(end.idx));
    for end in &ends {
        let s_along = axis.along(end.stock.0, end.stock.1);
        let s_cross = axis.cross(end.stock.0, end.stock.1);
        let half_along = axis.half_along();
        match end.fix {
            EndFix::Keep => {}
            EndFix::Face => {
                let q_along = axis.along(pts[end.adj].x, pts[end.adj].y);
                let side = if q_along > s_along + EPS {
                    1.0
                } else if q_along < s_along - EPS {
                    -1.0
                } else {
                    return;
                };
                let face = s_along + side * half_along;
                // The segment the end leaves is held to the end's own minimum.
                // A two-point pipe's one segment is the sink's, but while the
                // pipe stays two points room already puts its ends further
                // apart than both minima, and where the sink's arm has inserted
                // points first (ends are fixed highest index first) the source's
                // segment is a stub.
                if side * (q_along - face) < end.min - EPS {
                    return;
                }
                axis.set_along(&mut pts[end.idx], face);
            }
            EndFix::Leg => {
                let p = pts[end.idx].clone();
                let q_along = axis.along(pts[end.adj].x, pts[end.adj].y);
                let bend_along = clamp_to_face_span(axis.along(p.x, p.y), s_along, half_along);
                if (q_along - bend_along).abs() < minima.riser - EPS {
                    return;
                }
                let side = (new_line - s_cross).signum();
                let bend = axis.point(bend_along, new_line, None);
                let on_face =
                    axis.point(bend_along, s_cross + side * half_cross, p.attached_to_uid);
                if end.idx == 0 {
                    pts[0] = bend;
                    pts.insert(0, on_face);
                } else {
                    let last = pts.len() - 1;
                    pts[last] = bend;
                    pts.push(on_face);
                }
            }
            EndFix::Jog(own) => {
                let q_along = axis.along(pts[end.adj].x, pts[end.adj].y);
                let side = if q_along > s_along + EPS {
                    1.0
                } else if q_along < s_along - EPS {
                    -1.0
                } else {
                    return;
                };
                let face = s_along + side * half_along;
                // The step sits the end's minimum segment from the face, and is
                // refused only where it would pass the adjacent point and double
                // back: no invariant names that route (crowded stocks give such
                // inputs), so this guard is what keeps one out. A step that
                // leaves the rest of the shared line under a riser is built:
                // with room the collapse merges that run on a later pass, and
                // with crowded terminals the checker demands no minimum of it.
                // The valve is not a point the step must stay behind; it is
                // projected onto the settled pipe.
                if side * (q_along - face) < end.min - EPS {
                    return;
                }
                let step = face + side * end.min;
                let attached = pts[end.idx].attached_to_uid;
                let on_line = axis.point(step, new_line, None);
                let on_own = axis.point(step, own, None);
                let on_face = axis.point(face, own, attached);
                if end.idx == 0 {
                    pts[0] = on_line;
                    pts.insert(0, on_own);
                    pts.insert(0, on_face);
                } else {
                    let last = pts.len() - 1;
                    pts[last] = on_line;
                    pts.push(on_own);
                    pts.push(on_face);
                }
            }
        }
    }

    // The commit guard: where G6 holds, no step may add a segment through a
    // terminal stock's body. A leg can drop into one stock's face across the
    // other's body, and such a result is not committed. An end an arm leaves
    // invalid is committed: the next pass tries that end again from the new
    // geometry.
    if adds_a_body_crossing(&pts, points, stocks) {
        return;
    }
    *points = pts;
}

/// Bring both end segments of a pipe onto their stocks: the whole pipe at once
/// for a two-point flow, whose ends share one line.
fn attach_ends(points: &mut Vec<FlowPoint>, stocks: &HashMap<i32, (f64, f64)>, minima: &Minima) {
    if points.len() < 2 {
        return;
    }
    if points.len() == 2 {
        attach_end_segment(points, stocks, minima, true, true);
    } else {
        attach_end_segment(points, stocks, minima, true, false);
        attach_end_segment(points, stocks, minima, false, true);
    }
}

/// Drop the points that draw no route: a repeated point (a repeated endpoint
/// keeps its attachment), and an unattached interior point whose two segments
/// lie on one line -- continuing onward, or turning back along the line, where
/// the point is the tip of a spur drawn over itself and the route without the
/// spur is the same route. A removal can leave the new last three points on
/// one line or two points repeated, so the tail is examined again until
/// nothing more drops.
fn simplify(points: &mut Vec<FlowPoint>) {
    let mut out: Vec<FlowPoint> = Vec::with_capacity(points.len());
    for p in points.drain(..) {
        out.push(p);
        loop {
            let n = out.len();
            if n >= 2
                && (out[n - 2].x - out[n - 1].x).abs() <= EPS
                && (out[n - 2].y - out[n - 1].y).abs() <= EPS
            {
                if let Some(repeat) = out.pop()
                    && repeat.attached_to_uid.is_some()
                {
                    out[n - 2].attached_to_uid = repeat.attached_to_uid;
                }
                continue;
            }
            if n >= 3 && out[n - 2].attached_to_uid.is_none() {
                let axis = Axis::of_segment(&out[n - 3], &out[n - 2]);
                if axis.is_some() && axis == Axis::of_segment(&out[n - 2], &out[n - 1]) {
                    out.remove(n - 2);
                    continue;
                }
            }
            break;
        }
    }
    *points = out;
}

fn segment_length(a: &FlowPoint, b: &FlowPoint) -> f64 {
    (b.x - a.x).hypot(b.y - a.y)
}

/// Remove the first segment shorter than the minima allow, where the pipe can
/// give it up; returns whether the pipe changed.
///
/// A segment is short where the checker holds it to a minimum, which is only
/// when the terminals leave room (`Minima::room`). With crowded terminals a
/// segment under `CROWDED_MIN_SEGMENT` is removed only while a stock endpoint
/// is invalid, where the segment is what keeps the attach arms from bringing
/// that end onto its face; a pipe whose ends are valid is never moved for a
/// segment the checker accepts.
///
/// - A short end segment: the end moves to the bend, so the next segment is
///   the end segment, and the attach arms bring a stock end onto its face.
/// - A short riser between two parallel segments: the later run moves onto
///   the earlier run's line, or the earlier onto the later's, and the attach
///   arms settle the ends the move reaches. A candidate that leaves every
///   valid stock slot where it was is preferred, so the free side of the pipe
///   gives way before an authored slot does.
///
/// A candidate is not committed when it runs more segments through the bodies
/// of the stocks it ends on than the pipe did, where G6 holds
/// (`adds_a_body_crossing`). It need not leave every
/// stock end valid: the next pass's attach step brings such an end onto its
/// face, and requiring it would refuse routes the passes finish. A pipe of one
/// segment is never short with room to spare: room means its terminals are
/// further apart than `MIN_SEGMENT + MIN_SINK_SEGMENT`.
fn collapse_short_segment(
    points: &mut Vec<FlowPoint>,
    stocks: &HashMap<i32, (f64, f64)>,
    minima: &Minima,
) -> bool {
    let n = points.len();
    if n < 3 || (!minima.room && stock_ends_valid(points, stocks)) {
        return false;
    }
    let segments = n - 1;
    let Some(i) = (0..segments).find(|&i| {
        let len = segment_length(&points[i], &points[i + 1]);
        len > EPS && len < minima.segment(i, segments) - EPS
    }) else {
        return false;
    };

    let mut candidates: Vec<Vec<FlowPoint>> = Vec::new();
    if i == 0 || i + 1 == segments {
        let (end, bend) = if i == 0 { (0, 1) } else { (n - 1, n - 2) };
        let mut pts = points.clone();
        pts[bend].attached_to_uid = pts[end].attached_to_uid;
        pts.remove(end);
        candidates.push(pts);
    } else {
        let earlier = Axis::of_segment(&points[i - 1], &points[i]);
        let later = Axis::of_segment(&points[i + 1], &points[i + 2]);
        if earlier.is_none() || earlier != later {
            return false;
        }
        let axis = earlier.unwrap_or(Axis::Horizontal);
        let earlier_line = axis.cross(points[i].x, points[i].y);
        let later_line = axis.cross(points[i + 1].x, points[i + 1].y);
        let mut onto_earlier = points.clone();
        axis.set_cross(&mut onto_earlier[i + 1], earlier_line);
        axis.set_cross(&mut onto_earlier[i + 2], earlier_line);
        candidates.push(onto_earlier);
        let mut onto_later = points.clone();
        axis.set_cross(&mut onto_later[i - 1], later_line);
        axis.set_cross(&mut onto_later[i], later_line);
        candidates.push(onto_later);
    }

    // The valid stock slots the pipe had, by what they attach to.
    let slots: Vec<(Option<i32>, (f64, f64))> = [(0, 1), (n - 1, n - 2)]
        .iter()
        .filter_map(|&(idx, adj)| {
            let &stock = points[idx]
                .attached_to_uid
                .and_then(|uid| stocks.get(&uid))?;
            stock_endpoint_is_valid(&points[idx], &points[adj], stock)
                .then_some((points[idx].attached_to_uid, (points[idx].x, points[idx].y)))
        })
        .collect();
    let mut accepted: Option<Vec<FlowPoint>> = None;
    for mut pts in candidates {
        simplify(&mut pts);
        if pts.len() < 2 {
            continue;
        }
        attach_ends(&mut pts, stocks, minima);
        if pts == *points || adds_a_body_crossing(&pts, points, stocks) {
            continue;
        }
        let last = pts.len() - 1;
        let keeps_slots = slots.iter().all(|(uid, at)| {
            [&pts[0], &pts[last]]
                .iter()
                .any(|p| p.attached_to_uid == *uid && (p.x, p.y) == *at)
        });
        if keeps_slots {
            accepted = Some(pts);
            break;
        }
        if accepted.is_none() {
            accepted = Some(pts);
        }
    }
    let Some(pts) = accepted else {
        return false;
    };
    *points = pts;
    true
}

/// Whether `candidate` runs more segments through the bodies of the stocks it
/// ends on than `original` does, where G6 demands no crossing at all: the two
/// terminal bodies, each inflated by `MIN_SEGMENT`, do not overlap (the design
/// plan's G6). Where they overlap, G6 is best effort while G1-G5 must still
/// hold, so a step that brings the ends onto their faces is not refused for
/// running through the other body. A step may keep the crossings a pipe has,
/// but never adds to them.
fn adds_a_body_crossing(
    candidate: &[FlowPoint],
    original: &[FlowPoint],
    stocks: &HashMap<i32, (f64, f64)>,
) -> bool {
    terminals_clear(original, stocks, MIN_SEGMENT, MIN_SEGMENT)
        && body_crossings(candidate, stocks) > body_crossings(original, stocks)
}

/// How many (segment, terminal stock) pairs of `points` have the segment
/// entering the stock's body; a stock both ends attach to counts once.
fn body_crossings(points: &[FlowPoint], stocks: &HashMap<i32, (f64, f64)>) -> usize {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return 0;
    };
    let mut terminals: Vec<i32> = [first.attached_to_uid, last.attached_to_uid]
        .into_iter()
        .flatten()
        .filter(|uid| stocks.contains_key(uid))
        .collect();
    terminals.dedup();
    terminals
        .iter()
        .map(|uid| {
            let stock = stocks[uid];
            points
                .windows(2)
                .filter(|w| segment_enters_body(&w[0], &w[1], stock))
                .count()
        })
        .sum()
}

/// Straighten a two-point pipe whose producer wrote it slightly off axis
/// (xmutil's Vensim conversions routinely do), onto the average of the two
/// cross coordinates of its dominant axis; the valve joins that line. Returns
/// whether it moved anything.
fn straighten_two_point_pipe(points: &mut [FlowPoint], valve: &mut (f64, f64)) -> bool {
    if points.len() != 2 {
        return false;
    }
    let (dx, dy) = (
        (points[1].x - points[0].x).abs(),
        (points[1].y - points[0].y).abs(),
    );
    if dx <= EPS || dy <= EPS {
        return false;
    }
    if dx > dy {
        let y = (points[0].y + points[1].y) / 2.0;
        points[0].y = y;
        points[1].y = y;
        valve.1 = y;
    } else {
        let x = (points[0].x + points[1].x) / 2.0;
        points[0].x = x;
        points[1].x = x;
        valve.0 = x;
    }
    true
}

/// The point `arc` along `points`, by arc length from the first point.
fn point_at_arc(points: &[FlowPoint], arc: f64) -> (f64, f64) {
    let mut remaining = arc;
    let segments = points.len().saturating_sub(1);
    for (i, w) in points.windows(2).enumerate() {
        let len = segment_length(&w[0], &w[1]);
        if remaining <= len + EPS || i + 1 == segments {
            if len == 0.0 {
                return (w[0].x, w[0].y);
            }
            let t = (remaining / len).clamp(0.0, 1.0);
            return (
                w[0].x + (w[1].x - w[0].x) * t,
                w[0].y + (w[1].y - w[0].y) * t,
            );
        }
        remaining -= len;
    }
    points.last().map_or((0.0, 0.0), |p| (p.x, p.y))
}

/// Move the valve onto the pipe: to its nearest point when it is off the pipe,
/// and, on a path at least `2 * VALVE_CLAMP_MARGIN` long, to within the path
/// at least `VALVE_CLAMP_MARGIN` from either end by arc length. A valve that
/// already satisfies both is not moved.
pub(crate) fn project_valve_onto_pipe(points: &[FlowPoint], valve: &mut (f64, f64)) {
    if points.len() < 2 {
        return;
    }
    let mut best: Option<(f64, f64)> = None; // (distance, arc position)
    let mut traversed = 0.0;
    for w in points.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let len = segment_length(a, b);
        let t = if len == 0.0 {
            0.0
        } else {
            (((valve.0 - a.x) * (b.x - a.x) + (valve.1 - a.y) * (b.y - a.y)) / (len * len))
                .clamp(0.0, 1.0)
        };
        let d = point_segment_distance(*valve, a, b);
        if best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, traversed + t * len));
        }
        traversed += len;
    }
    let Some((d, mut arc)) = best else {
        return;
    };
    let total = traversed;
    let long_enough = total >= 2.0 * VALVE_CLAMP_MARGIN;
    let within_margin = !long_enough
        || (arc >= VALVE_CLAMP_MARGIN - EPS && arc <= total - VALVE_CLAMP_MARGIN + EPS);
    if d <= EPS && within_margin {
        return;
    }
    if long_enough {
        arc = arc.clamp(VALVE_CLAMP_MARGIN, total - VALVE_CLAMP_MARGIN);
    }
    *valve = point_at_arc(points, arc);
}

/// Keep a valve the layout placed `VALVE_CLAMP_MARGIN` from the ends of the
/// segment it sits on, the margin the editor clamps a dragged valve to along
/// its closest segment (`clampToSegment` in `src/diagram/drawing/Flow.tsx`):
/// along that segment when it is at least twice the margin long, else at the
/// middle of the longest segment (the first, on a tie) when that one is. A
/// valve already clear is not moved, and a pipe whose every segment is too
/// short keeps it. Only for valves the layout owns: an imported valve is held
/// to the arc-length margin alone (`project_valve_onto_pipe`), which this
/// never undoes, since both placements are at least the margin from the path's
/// ends.
pub(crate) fn settle_laid_out_valve(points: &[FlowPoint], valve: &mut (f64, f64)) {
    let Some((i, _)) = points
        .windows(2)
        .enumerate()
        .map(|(i, w)| (i, point_segment_distance(*valve, &w[0], &w[1])))
        .min_by(|a, b| a.1.total_cmp(&b.1))
    else {
        return;
    };
    let (a, b) = (&points[i], &points[i + 1]);
    let span = |p: f64, q: f64| (p.min(q) + VALVE_CLAMP_MARGIN, p.max(q) - VALVE_CLAMP_MARGIN);
    if segment_length(a, b) >= 2.0 * VALVE_CLAMP_MARGIN {
        let clamped = match Axis::of_segment(a, b) {
            Some(Axis::Horizontal) => {
                let (lo, hi) = span(a.x, b.x);
                (valve.0.clamp(lo, hi), a.y)
            }
            Some(Axis::Vertical) => {
                let (lo, hi) = span(a.y, b.y);
                (a.x, valve.1.clamp(lo, hi))
            }
            None => return,
        };
        if (clamped.0 - valve.0).abs() > EPS || (clamped.1 - valve.1).abs() > EPS {
            *valve = clamped;
        }
        return;
    }
    let mut longest: Option<(&FlowPoint, &FlowPoint)> = None;
    for w in points.windows(2) {
        if longest.is_none_or(|(p, q)| segment_length(&w[0], &w[1]) > segment_length(p, q) + EPS) {
            longest = Some((&w[0], &w[1]));
        }
    }
    if let Some((p, q)) = longest
        && segment_length(p, q) >= 2.0 * VALVE_CLAMP_MARGIN
    {
        *valve = ((p.x + q.x) / 2.0, (p.y + q.y) / 2.0);
    }
}

/// Bring every flow in an imported view to the invariants in the module docs.
///
/// Geometry that already satisfies them -- including an off-center slot a
/// modeler chose on a face -- is not moved. Otherwise, per flow, until a pass
/// changes nothing: repeated and collinear points are dropped (`simplify`); a
/// two-point pipe written off axis is straightened (and the straightening
/// undone when the attach step cannot then bring its ends onto their stocks);
/// each stock endpoint is brought onto its stock as `EndFix` describes; and a
/// segment shorter than the minima allow is collapsed where the pipe can give
/// it up (`collapse_short_segment`). Then an off-pipe valve,
/// or one within the margin of an end, is projected onto the pipe, and each
/// cloud an endpoint is attached to is recentered on that endpoint (clouds are
/// created from a producer's raw endpoints, before any of this, so they are
/// the thing that moves).
///
/// Unattached endpoints and diagonal multi-point segments are left alone:
/// the importer that produced them owns resolving them.
pub(crate) fn normalize_flow_geometry(elements: &mut [ViewElement]) {
    normalize_flow_geometry_where(elements, |_| true);
}

/// `normalize_flow_geometry` over only the flows `include` selects (by uid).
/// Stocks and clouds are read from the whole view, but only the selected
/// flows and the clouds their ends are attached to move: incremental layout
/// normalizes the flows it creates and leaves every preserved flow as it was.
pub(crate) fn normalize_flow_geometry_where(
    elements: &mut [ViewElement],
    include: impl Fn(i32) -> bool,
) {
    let stocks: HashMap<i32, (f64, f64)> = elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y))),
            _ => None,
        })
        .collect();
    let clouds: HashSet<i32> = elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Cloud(c) => Some(c.uid),
            _ => None,
        })
        .collect();

    let mut cloud_centers: HashMap<i32, (f64, f64)> = HashMap::new();
    for elem in elements.iter_mut() {
        let ViewElement::Flow(f) = elem else { continue };
        if !include(f.uid) || f.points.len() < 2 {
            continue;
        }
        let mut valve = (f.x, f.y);
        for _ in 0..MAX_PASSES {
            let before = (f.points.clone(), valve);
            let mut simplified = f.points.clone();
            simplify(&mut simplified);
            if simplified.len() >= 2 {
                f.points = simplified;
            }
            let minima = Minima::of(&f.points, &stocks);
            if straighten_two_point_pipe(&mut f.points, &mut valve) {
                attach_ends(&mut f.points, &stocks, &minima);
                if !stock_ends_valid(&f.points, &stocks) {
                    (f.points, valve) = before.clone();
                }
            } else {
                attach_ends(&mut f.points, &stocks, &minima);
            }
            collapse_short_segment(&mut f.points, &stocks, &minima);
            if (f.points.clone(), valve) == before {
                break;
            }
        }
        project_valve_onto_pipe(&f.points, &mut valve);
        (f.x, f.y) = valve;

        let last = f.points.len() - 1;
        for end in [0, last] {
            let p = &f.points[end];
            if let Some(uid) = p.attached_to_uid
                && clouds.contains(&uid)
            {
                cloud_centers.insert(uid, (p.x, p.y));
            }
        }
    }
    for elem in elements.iter_mut() {
        if let ViewElement::Cloud(c) = elem
            && let Some(&(x, y)) = cloud_centers.get(&c.uid)
        {
            c.x = x;
            c.y = y;
        }
    }
}

/// The invariants above as a checker, one message per violation, for tests of
/// every producer of flow geometry (the importers and the layout). It is the
/// test oracle, stated independently of the code that establishes the
/// invariants, and follows the editor's checker for the same invariants
/// (`docs/design-plans/2026-09-10-diagram-editing-core.md`, "Invariants").
#[cfg(test)]
pub(crate) fn flow_invariant_violations(elements: &[ViewElement]) -> Vec<String> {
    let by_uid: HashMap<i32, &ViewElement> = elements.iter().map(|e| (e.get_uid(), e)).collect();
    let mut out = Vec::new();
    for elem in elements {
        let ViewElement::Flow(f) = elem else { continue };
        let name = &f.name;
        let pts = &f.points;
        if pts.len() < 2 {
            out.push(format!("{name}: fewer than two points"));
            continue;
        }
        for (i, w) in pts.windows(2).enumerate() {
            let (dx, dy) = ((w[0].x - w[1].x).abs(), (w[0].y - w[1].y).abs());
            if dx <= EPS && dy <= EPS {
                out.push(format!("{name}: segment {i} has zero length"));
            } else if dx > EPS && dy > EPS {
                out.push(format!("{name}: segment {i} is diagonal"));
            }
        }
        for i in 0..pts.len().saturating_sub(2) {
            let horizontal =
                |a: &FlowPoint, b: &FlowPoint| (a.y - b.y).abs() <= EPS && (a.x - b.x).abs() > EPS;
            let vertical =
                |a: &FlowPoint, b: &FlowPoint| (a.x - b.x).abs() <= EPS && (a.y - b.y).abs() > EPS;
            let (a, b, c) = (&pts[i], &pts[i + 1], &pts[i + 2]);
            if (horizontal(a, b) && horizontal(b, c)) || (vertical(a, b) && vertical(b, c)) {
                out.push(format!("{name}: segments {i} and {} are collinear", i + 1));
            }
        }
        for (i, p) in pts.iter().enumerate().take(pts.len() - 1).skip(1) {
            if p.attached_to_uid.is_some() {
                out.push(format!("{name}: interior point {i} is attached"));
            }
        }
        let last = pts.len() - 1;
        let mut end_stocks: Vec<(f64, f64)> = Vec::new();
        let mut bodies: Vec<Option<(f64, f64, f64, f64)>> = Vec::new();
        for (end, adj) in [(0, 1), (last, last - 1)] {
            let p = &pts[end];
            let q = &pts[adj];
            let Some(uid) = p.attached_to_uid else {
                out.push(format!("{name}: endpoint {end} is unattached"));
                bodies.push(None);
                continue;
            };
            match by_uid.get(&uid) {
                Some(ViewElement::Stock(s)) => {
                    end_stocks.push((s.x, s.y));
                    bodies.push(Some((
                        s.x - HALF_W,
                        s.y - HALF_H,
                        s.x + HALF_W,
                        s.y + HALF_H,
                    )));
                    if let Some(problem) = stock_endpoint_problem(p, q, (s.x, s.y)) {
                        out.push(format!(
                            "{name}: endpoint {end} on stock {}: {problem}",
                            s.name
                        ));
                    }
                }
                Some(ViewElement::Cloud(c)) => {
                    bodies.push(Some((c.x, c.y, c.x, c.y)));
                    if (c.x - p.x).abs() > EPS || (c.y - p.y).abs() > EPS {
                        out.push(format!(
                            "{name}: endpoint {end} at ({}, {}) but its cloud is at ({}, {})",
                            p.x, p.y, c.x, c.y
                        ));
                    }
                    if c.flow_uid != f.uid {
                        out.push(format!(
                            "{name}: endpoint {end}'s cloud belongs to another flow"
                        ));
                    }
                }
                Some(_) => {
                    bodies.push(None);
                    out.push(format!("{name}: endpoint {end} attached to a non-stock"));
                }
                None => {
                    bodies.push(None);
                    out.push(format!("{name}: endpoint {end} attached to a missing uid"));
                }
            }
        }
        for s in &end_stocks {
            for (i, w) in pts.windows(2).enumerate() {
                if segment_enters_body(&w[0], &w[1], *s) {
                    out.push(format!(
                        "{name}: segment {i} runs through an endpoint stock"
                    ));
                }
            }
        }
        // G3's minima, demanded when the source body inflated by MIN_SEGMENT
        // and the sink body inflated by MIN_SINK_SEGMENT do not overlap.
        let room = match (bodies[0], bodies[1]) {
            (Some(a), Some(b)) => {
                let a = (
                    a.0 - MIN_SEGMENT,
                    a.1 - MIN_SEGMENT,
                    a.2 + MIN_SEGMENT,
                    a.3 + MIN_SEGMENT,
                );
                let b = (
                    b.0 - MIN_SINK_SEGMENT,
                    b.1 - MIN_SINK_SEGMENT,
                    b.2 + MIN_SINK_SEGMENT,
                    b.3 + MIN_SINK_SEGMENT,
                );
                !(a.0 < b.2 && b.0 < a.2 && a.1 < b.3 && b.1 < a.3)
            }
            _ => true,
        };
        if room {
            let segments = pts.len() - 1;
            for (i, w) in pts.windows(2).enumerate() {
                let len = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
                if len <= EPS {
                    continue;
                }
                let (kind, min) = if i + 1 == segments {
                    ("final segment", MIN_SINK_SEGMENT)
                } else if i == 0 {
                    ("first segment", MIN_SEGMENT)
                } else {
                    ("interior segment", MIN_SEGMENT)
                };
                if len < min - EPS {
                    out.push(format!("{name}: {kind} {i} is {len:.2}, under {min}"));
                }
            }
        }
        let mut best: Option<(f64, f64)> = None;
        let mut traversed = 0.0;
        for w in pts.windows(2) {
            let len = (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
            let d = point_segment_distance((f.x, f.y), &w[0], &w[1]);
            let along = (f.x - w[0].x).hypot(f.y - w[0].y).min(len);
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, traversed + along));
            }
            traversed += len;
        }
        match best {
            Some((d, _)) if d > EPS => {
                out.push(format!("{name}: valve ({}, {}) is off the pipe", f.x, f.y));
            }
            Some((_, arc)) if traversed >= 2.0 * VALVE_CLAMP_MARGIN => {
                let from_end = arc.min(traversed - arc);
                if from_end < VALVE_CLAMP_MARGIN - EPS {
                    out.push(format!(
                        "{name}: valve {from_end:.2} from an end of the path, under {VALVE_CLAMP_MARGIN}"
                    ));
                }
            }
            _ => {}
        }
    }
    // A cloud drawn inside a stock's box reads as part of the stock.
    for elem in elements {
        let ViewElement::Cloud(c) = elem else {
            continue;
        };
        for other in elements {
            if let ViewElement::Stock(s) = other
                && (c.x - s.x).abs() < HALF_W - EPS
                && (c.y - s.y).abs() < HALF_H - EPS
            {
                out.push(format!(
                    "cloud {} at ({}, {}) is inside stock {}",
                    c.uid, c.x, c.y, s.name
                ));
            }
        }
    }
    out
}

#[cfg(test)]
fn stock_endpoint_problem(p: &FlowPoint, q: &FlowPoint, stock: (f64, f64)) -> Option<String> {
    let dx = p.x - stock.0;
    let dy = p.y - stock.1;
    let on_side = (dx.abs() - HALF_W).abs() <= EPS && dy.abs() <= HALF_H + EPS;
    let on_cap = (dy.abs() - HALF_H).abs() <= EPS && dx.abs() <= HALF_W + EPS;
    if on_side && !on_cap {
        if HALF_H - dy.abs() < CORNER_CLEARANCE - EPS {
            return Some(format!("corner clearance {:.2}", HALF_H - dy.abs()));
        }
        let outward = dx.signum() * (q.x - p.x);
        if (q.y - p.y).abs() > EPS || outward <= EPS {
            return Some("adjacent segment is not perpendicular and outward".to_string());
        }
        None
    } else if on_cap && !on_side {
        if HALF_W - dx.abs() < CORNER_CLEARANCE - EPS {
            return Some(format!("corner clearance {:.2}", HALF_W - dx.abs()));
        }
        let outward = dy.signum() * (q.y - p.y);
        if (q.x - p.x).abs() > EPS || outward <= EPS {
            return Some("adjacent segment is not perpendicular and outward".to_string());
        }
        None
    } else if on_side && on_cap {
        Some("at a corner".to_string())
    } else {
        Some(format!("off the faces, offset ({dx:.2}, {dy:.2})"))
    }
}

/// Whether the axis-aligned segment `a`-`b` enters the open body of the stock
/// centered at `stock`: its fixed coordinate strictly inside the body's span
/// and its extent overlapping the other span. A diagonal segment is not
/// judged here (the checker reports it separately, and no step builds one).
fn segment_enters_body(a: &FlowPoint, b: &FlowPoint, stock: (f64, f64)) -> bool {
    let (xmin, xmax) = (stock.0 - HALF_W + EPS, stock.0 + HALF_W - EPS);
    let (ymin, ymax) = (stock.1 - HALF_H + EPS, stock.1 + HALF_H - EPS);
    if (a.y - b.y).abs() <= EPS {
        let (lo, hi) = (a.x.min(b.x), a.x.max(b.x));
        a.y > ymin && a.y < ymax && hi > xmin && lo < xmax
    } else if (a.x - b.x).abs() <= EPS {
        let (lo, hi) = (a.y.min(b.y), a.y.max(b.y));
        a.x > xmin && a.x < xmax && hi > ymin && lo < ymax
    } else {
        false
    }
}

#[cfg(test)]
#[path = "flow_geometry_tests.rs"]
mod tests;
