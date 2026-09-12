// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Units, points, axes and boxes the editing core shares.
//!
//! Positions are absolute model coordinates (px at zoom 1). The constants are
//! the thresholds of the flow invariants G1-G8 in
//! `docs/design-plans/2026-09-10-diagram-editing-core.md` ("Units"), shared by
//! every producer of flow geometry: the interactive core here and the import
//! and layout normalization in `diagram::flow_geometry`.

use smallvec::SmallVec;

use crate::datamodel::view_element::FlowPoint;
use crate::diagram::constants::{FLOW_ARROWHEAD_RADIUS, STOCK_HEIGHT, STOCK_WIDTH};

/// A pipe's points. 98.8% of imported flows have two points and none has four
/// or more; routing builds at most five-segment paths and a preserved tail adds
/// a base prefix, so twelve inline points keep every routed path off the heap.
pub(crate) type Path = SmallVec<[Point; 12]>;

/// A stock endpoint stays this far from its face's corners (G4). A pipe drawn
/// into a corner reads as attached to two faces at once, and its arrowhead
/// overlaps the stock's outline.
pub const CORNER_CLEARANCE: f64 = 3.0;

/// The shortest routed stub (the first segment) or riser (an interior
/// segment) when the terminals leave room (G3).
pub const MIN_SEGMENT: f64 = 10.0;

/// The valve keeps this arc-length distance from the path's ends when the path
/// is at least twice this long (G8).
pub const VALVE_CLAMP_MARGIN: f64 = 10.0;

/// The shortest final segment when the terminals leave room (G3): the
/// renderer pulls the path back 7.5px to seat the arrowhead, so a shorter
/// final segment tucks the arrowhead into the preceding turn.
pub const MIN_SINK_SEGMENT: f64 = FLOW_ARROWHEAD_RADIUS + 7.5;

/// The preferred distance between two endpoints on one face: a routing
/// preference for a flow newly landing on a face, not an invariant.
pub const PIPE_SPACING: f64 = 10.0;

/// Coordinates within this distance are equal. Face points are fractions of
/// the stock size and valves are interpolated, so exact comparison would
/// report float noise; every defect the invariants exist for is a pixel or
/// more.
pub const GEOMETRY_EPSILON: f64 = 1e-6;

pub(crate) const HALF_WIDTH: f64 = STOCK_WIDTH / 2.0;
pub(crate) const HALF_HEIGHT: f64 = STOCK_HEIGHT / 2.0;

/// A point in model coordinates.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    pub(crate) fn distance(self, other: Point) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }

    /// Within `GEOMETRY_EPSILON` on both axes.
    pub(crate) fn same(self, other: Point) -> bool {
        (self.x - other.x).abs() <= GEOMETRY_EPSILON && (self.y - other.y).abs() <= GEOMETRY_EPSILON
    }

    pub(crate) fn coord(self, axis: Axis) -> f64 {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
        }
    }

    pub(crate) fn offset(self, delta: Point) -> Point {
        Point::new(self.x + delta.x, self.y + delta.y)
    }

    /// The vector from `from` to `self`.
    pub(crate) fn minus(self, from: Point) -> Point {
        Point::new(self.x - from.x, self.y - from.y)
    }
}

/// Anything with a position the geometry reads: points and flow points.
pub(crate) trait Positioned {
    fn point(&self) -> Point;
}

impl Positioned for Point {
    fn point(&self) -> Point {
        *self
    }
}

impl Positioned for FlowPoint {
    fn point(&self) -> Point {
        Point::new(self.x, self.y)
    }
}

/// The axis a segment runs along: `X` for a horizontal segment, `Y` for a
/// vertical one.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Axis {
    X,
    Y,
}

impl Axis {
    pub(crate) const ALL: [Axis; 2] = [Axis::X, Axis::Y];

    pub(crate) fn other(self) -> Axis {
        match self {
            Axis::X => Axis::Y,
            Axis::Y => Axis::X,
        }
    }

    /// The point whose coordinate on `self` is `v` and on the other axis `w`.
    pub(crate) fn compose(self, v: f64, w: f64) -> Point {
        match self {
            Axis::X => Point::new(v, w),
            Axis::Y => Point::new(w, v),
        }
    }

    /// A segment's working axis. Routing produces exactly axis-aligned
    /// segments; imported data can carry a few pixels of drift, which is
    /// classified by its dominant axis (a tie counts as horizontal).
    pub(crate) fn of_segment(a: Point, b: Point) -> Axis {
        if (b.y - a.y).abs() <= (b.x - a.x).abs() {
            Axis::X
        } else {
            Axis::Y
        }
    }
}

/// A face of a stock.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Left,
    Right,
    Top,
    Bottom,
}

impl Face {
    pub const ALL: [Face; 4] = [Face::Left, Face::Right, Face::Top, Face::Bottom];
}

/// One end of a flow.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum FlowEnd {
    Source,
    Sink,
}

impl FlowEnd {
    pub const ALL: [FlowEnd; 2] = [FlowEnd::Source, FlowEnd::Sink];

    pub fn other(self) -> FlowEnd {
        match self {
            FlowEnd::Source => FlowEnd::Sink,
            FlowEnd::Sink => FlowEnd::Source,
        }
    }
}

/// An axis-aligned box.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Bounds {
    pub min_x: f64,
    pub max_x: f64,
    pub min_y: f64,
    pub max_y: f64,
}

impl Bounds {
    /// The body of a stock centered at `center`: every renderer draws a stock
    /// as a `STOCK_WIDTH` x `STOCK_HEIGHT` box whatever size its producer drew.
    pub(crate) fn stock(center: Point) -> Bounds {
        Bounds {
            min_x: center.x - HALF_WIDTH,
            max_x: center.x + HALF_WIDTH,
            min_y: center.y - HALF_HEIGHT,
            max_y: center.y + HALF_HEIGHT,
        }
    }

    pub(crate) fn point(p: Point) -> Bounds {
        Bounds {
            min_x: p.x,
            max_x: p.x,
            min_y: p.y,
            max_y: p.y,
        }
    }

    pub(crate) fn inflate(self, by: f64) -> Bounds {
        Bounds {
            min_x: self.min_x - by,
            max_x: self.max_x + by,
            min_y: self.min_y - by,
            max_y: self.max_y + by,
        }
    }

    pub(crate) fn union(self, other: Bounds) -> Bounds {
        Bounds {
            min_x: self.min_x.min(other.min_x),
            max_x: self.max_x.max(other.max_x),
            min_y: self.min_y.min(other.min_y),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// Whether the boxes share any point, edges included: the conservative
    /// test region queries prefilter with.
    pub(crate) fn touches(self, other: Bounds) -> bool {
        self.min_x <= other.max_x
            && other.min_x <= self.max_x
            && self.min_y <= other.max_y
            && other.min_y <= self.max_y
    }

    /// Touching boxes do not overlap: two bodies exactly `MIN_SEGMENT` apart
    /// leave exactly enough room for a routed stub.
    pub(crate) fn overlaps(self, other: Bounds) -> bool {
        self.min_x < other.max_x
            && other.min_x < self.max_x
            && self.min_y < other.max_y
            && other.min_y < self.max_y
    }

    /// Whether `p` lies in the open interior, `GEOMETRY_EPSILON` in from every edge.
    pub(crate) fn strictly_contains(self, p: Point) -> bool {
        let e = GEOMETRY_EPSILON;
        p.x > self.min_x + e && p.x < self.max_x - e && p.y > self.min_y + e && p.y < self.max_y - e
    }
}

/// Whether segment `a`-`b` passes through the open interior of `bounds` with
/// positive length. A segment starting on a face or running along an edge line
/// is not "through". On orthogonal paths a larger length tolerance would change
/// nothing observable: an axis-aligned segment can only overlap the interior
/// shallowly by ending inside it, where the adjacent segment or the endpoint
/// check reports the crossing anyway.
pub(crate) fn segment_through(a: Point, b: Point, bounds: Bounds) -> bool {
    let e = GEOMETRY_EPSILON;
    if (a.y - b.y).abs() <= e {
        if !(a.y > bounds.min_y + e && a.y < bounds.max_y - e) {
            return false;
        }
        let lo = a.x.min(b.x).max(bounds.min_x + e);
        let hi = a.x.max(b.x).min(bounds.max_x - e);
        return hi - lo > e;
    }
    if (a.x - b.x).abs() <= e {
        if !(a.x > bounds.min_x + e && a.x < bounds.max_x - e) {
            return false;
        }
        let lo = a.y.min(b.y).max(bounds.min_y + e);
        let hi = a.y.max(b.y).min(bounds.max_y - e);
        return hi - lo > e;
    }
    // A diagonal is structurally invalid on its own and routing never produces
    // one; sampling it keeps an imported diagonal reading as crossing when it
    // does.
    (0..=20).any(|i| {
        let t = f64::from(i) / 20.0;
        bounds.strictly_contains(Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t))
    })
}

/// The stocks a routed path should not pass through, queried by region.
///
/// A view can hold hundreds of stocks while a route only meets the few near
/// its terminals, so routing never walks the whole set: the planner implements
/// this over a grid built once per gesture, and a slice implements it for
/// callers with a handful of stocks (tests, a single flow).
pub(crate) trait Obstacles {
    /// Calls `f` with the center of every stock whose body touches `region`,
    /// possibly more (the result is a superset), stopping as soon as `f`
    /// returns true; returns whether it did.
    fn any_near(&self, region: Bounds, f: impl FnMut(Point) -> bool) -> bool;

    /// Whether `p` lies strictly inside any stock's body.
    fn contains_strictly(&self, p: Point) -> bool {
        self.any_near(Bounds::point(p), |center| {
            Bounds::stock(center).strictly_contains(p)
        })
    }
}

impl Obstacles for [Point] {
    fn any_near(&self, region: Bounds, mut f: impl FnMut(Point) -> bool) -> bool {
        self.iter()
            .any(|&center| Bounds::stock(center).touches(region) && f(center))
    }
}

/// No obstacles at all.
pub(crate) struct NoObstacles;

impl Obstacles for NoObstacles {
    fn any_near(&self, _region: Bounds, _f: impl FnMut(Point) -> bool) -> bool {
        false
    }
}

/// `v` limited to `[lo, hi]`, tolerating an empty range (`lo > hi` yields `lo`
/// below it and `hi` above it) where `f64::clamp` would panic: callers clamp
/// into bands built from terminal positions, which can cross.
pub(crate) fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// -1, 0 or 1: the sign of `v`, zero for zero (unlike `f64::signum`).
pub(crate) fn sign(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}
