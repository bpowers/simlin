// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Classifying a path against its terminals (G2-G6).
//!
//! `path_quality` runs once per route candidate, so it allocates nothing and
//! asks the obstacle set only about the region the path covers.

use super::geometry::{
    Axis, Bounds, GEOMETRY_EPSILON, MIN_SEGMENT, MIN_SINK_SEGMENT, Obstacles, Point,
    segment_through,
};
use super::terminal::{Terminal, Terminals, face_attachment, face_of_endpoint};

/// How a path violates G2-G6, least severe first. When nothing is valid the
/// fallback keeps the least severe fault: G6 (crossing) is given up before the
/// G3 minima (short), and those before G2/G4/G5 (structure).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Fault {
    None,
    Crossing,
    Short,
    Structure,
}

/// G3's "whenever the terminals leave room": the source body inflated by
/// `MIN_SEGMENT` and the sink body inflated by `MIN_SINK_SEGMENT` are disjoint.
pub(crate) fn terminals_leave_room(terminals: &Terminals) -> bool {
    !terminals
        .source
        .body()
        .inflate(MIN_SEGMENT)
        .overlaps(terminals.sink.body().inflate(MIN_SINK_SEGMENT))
}

/// G6's precondition: the two terminal bodies, each inflated by `MIN_SEGMENT`,
/// do not overlap.
pub(crate) fn bodies_apart(terminals: &Terminals) -> bool {
    !terminals
        .source
        .body()
        .inflate(MIN_SEGMENT)
        .overlaps(terminals.sink.body().inflate(MIN_SEGMENT))
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct PathQuality {
    /// `Fault::None` when the path holds G2-G6.
    pub fault: Fault,
    /// Whether the path crosses a terminal body, puts a free endpoint inside a
    /// stock, or is `obstructed`, computed whether or not G6's precondition
    /// holds. With the bodies apart the first two are a fault; with them
    /// overlapping they are G6's best effort, and an obstruction is never a
    /// fault. The ranking honors every one as a preference.
    pub crossing: bool,
    /// Whether some segment passes through an obstacle stock that is not a
    /// terminal.
    pub obstructed: bool,
    /// Whether some segment is under its G3 minimum, computed whether or not
    /// the terminals leave room. With room it is a fault; without, the minima
    /// are best effort and a candidate that meets them anyway is preferred, so
    /// a drag across the room boundary does not flip between a route that
    /// respects the minima and one that merely stopped being required to.
    pub short: bool,
}

const STRUCTURE: PathQuality = PathQuality {
    fault: Fault::Structure,
    crossing: false,
    obstructed: false,
    short: false,
};

/// Classify a path against its terminals.
///
/// `stocks` are the view's stocks a free endpoint must not sit inside (G6's
/// cloud clause); the terminal stocks are always checked. `obstacles` are
/// stocks the path should not pass through although they are not its
/// terminals (a pipe through a stock reads as attached to it): a path through
/// one is `obstructed` and ranks as crossing, but is never a fault, so a route
/// still exists where nothing else does. An obstacle at a terminal stock's
/// center is that terminal, whose crossing G6 governs.
pub(crate) fn path_quality<S, O>(
    points: &[Point],
    terminals: &Terminals,
    stocks: &S,
    obstacles: &O,
) -> PathQuality
where
    S: Obstacles + ?Sized,
    O: Obstacles + ?Sized,
{
    let n = points.len();
    if n < 2 || !points.iter().all(|p| p.is_finite()) {
        return STRUCTURE;
    }
    let e = GEOMETRY_EPSILON;
    let mut previous_axis: Option<Axis> = None;
    for w in points.windows(2) {
        let flat_x = (w[0].x - w[1].x).abs() <= e;
        let flat_y = (w[0].y - w[1].y).abs() <= e;
        // Both flat is a zero-length segment, neither a diagonal.
        if flat_x == flat_y {
            return STRUCTURE;
        }
        let axis = if flat_y { Axis::X } else { Axis::Y };
        if previous_axis == Some(axis) {
            return STRUCTURE;
        }
        previous_axis = Some(axis);
    }
    for (t, index, adjacent) in [(&terminals.source, 0, 1), (&terminals.sink, n - 1, n - 2)] {
        let Terminal::Stock { center, .. } = t else {
            continue;
        };
        let p = points[index];
        let q = points[adjacent];
        // With the adjacent point, face_of_endpoint picks the face the
        // orthogonal adjacent segment is perpendicular to whenever there is
        // one, so only the outward direction remains to check.
        let Some(face) = face_of_endpoint(*center, p, Some(q)) else {
            return STRUCTURE;
        };
        let att = face_attachment(*center, face);
        let along = p.coord(att.along);
        if along < att.lo - e
            || along > att.hi + e
            || att.sign * (q.coord(att.normal) - att.plane) <= e
        {
            return STRUCTURE;
        }
    }
    let terminal_crossing = crosses_bodies(points, terminals, stocks);
    let obstructed = through_obstacles(points, terminals, obstacles);
    let crossing = terminal_crossing || obstructed;
    let short = points.windows(2).enumerate().any(|(i, w)| {
        let minimum = if i == n - 2 {
            MIN_SINK_SEGMENT
        } else {
            MIN_SEGMENT
        };
        w[0].distance(w[1]) < minimum - e
    });
    if short && terminals_leave_room(terminals) {
        return PathQuality {
            fault: Fault::Short,
            crossing,
            obstructed,
            short,
        };
    }
    let fault = if terminal_crossing && bodies_apart(terminals) {
        Fault::Crossing
    } else {
        Fault::None
    };
    PathQuality {
        fault,
        crossing,
        obstructed,
        short,
    }
}

/// Whether a segment passes through the body of an obstacle other than a
/// terminal. Only obstacles near the path's bounding box are examined.
fn through_obstacles<O: Obstacles + ?Sized>(
    points: &[Point],
    terminals: &Terminals,
    obstacles: &O,
) -> bool {
    let e = GEOMETRY_EPSILON;
    let bbox = points
        .iter()
        .skip(1)
        .fold(Bounds::point(points[0]), |b, &p| b.union(Bounds::point(p)));
    let terminal_centers = [
        terminals.source.stock_center(),
        terminals.sink.stock_center(),
    ];
    obstacles.any_near(bbox, |center| {
        let body = Bounds::stock(center);
        if body.max_x <= bbox.min_x
            || body.min_x >= bbox.max_x
            || body.max_y <= bbox.min_y
            || body.min_y >= bbox.max_y
        {
            return false;
        }
        let is_terminal = terminal_centers
            .iter()
            .flatten()
            .any(|t| (t.x - center.x).abs() <= e && (t.y - center.y).abs() <= e);
        !is_terminal && points.windows(2).any(|w| segment_through(w[0], w[1], body))
    })
}

fn crosses_bodies<S: Obstacles + ?Sized>(
    points: &[Point],
    terminals: &Terminals,
    stocks: &S,
) -> bool {
    let n = points.len();
    let terminal_bodies = [
        terminals.source.stock_center().map(Bounds::stock),
        terminals.sink.stock_center().map(Bounds::stock),
    ];
    for body in terminal_bodies.iter().flatten() {
        if points
            .windows(2)
            .any(|w| segment_through(w[0], w[1], *body))
        {
            return true;
        }
    }
    for (t, index) in [(&terminals.source, 0), (&terminals.sink, n - 1)] {
        if !t.is_free() {
            continue;
        }
        let p = points[index];
        let in_terminal = terminal_bodies
            .iter()
            .flatten()
            .any(|body| body.strictly_contains(p));
        if in_terminal || stocks.contains_strictly(p) {
            return true;
        }
    }
    false
}

/// Classify a routed flow's path (G2-G6). The planner uses this to decide
/// whether committing onto a target yields a view that holds the invariants;
/// `stocks` extends G6's cloud clause to the rest of the view. A path through a
/// non-terminal stock is a routing preference, not a fault, so it has no say.
pub(crate) fn flow_fault<S: Obstacles + ?Sized>(
    points: &[Point],
    terminals: &Terminals,
    stocks: &S,
) -> Fault {
    path_quality(points, terminals, stocks, &super::geometry::NoObstacles).fault
}
