// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Face attachment and terminals: the one owner of how a pipe attaches to a
//! stock. Route candidates, `route_end`'s pinned terminal, `offset_segment`'s
//! tail re-solve and `heal`'s re-pin all derive endpoints, outward directions
//! and stub tips from `face_attachment` and `stub_tip`.
//!
//! These functions run for every route candidate, so none allocates.

use smallvec::SmallVec;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{Flow, FlowPoint};

use super::geometry::{
    Axis, Bounds, CORNER_CLEARANCE, Face, GEOMETRY_EPSILON, HALF_HEIGHT, HALF_WIDTH, Path, Point,
    Positioned, clamp,
};

/// Where a pipe may attach to one face of a stock.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct FaceAttachment {
    pub face: Face,
    /// The axis a segment leaving this face runs along.
    pub normal: Axis,
    /// The axis the face itself runs along.
    pub along: Axis,
    /// +1 when leaving the face increases the normal coordinate, else -1.
    pub sign: f64,
    /// The face's coordinate on the normal axis.
    pub plane: f64,
    /// The stock center's coordinate on the along axis.
    pub center: f64,
    /// Valid endpoint positions on the along axis: the face extent minus
    /// `CORNER_CLEARANCE` at each end.
    pub lo: f64,
    pub hi: f64,
}

pub(crate) fn face_attachment(stock: Point, face: Face) -> FaceAttachment {
    match face {
        Face::Left | Face::Right => {
            let extent = HALF_HEIGHT - CORNER_CLEARANCE;
            let right = face == Face::Right;
            FaceAttachment {
                face,
                normal: Axis::X,
                along: Axis::Y,
                sign: if right { 1.0 } else { -1.0 },
                plane: stock.x + if right { HALF_WIDTH } else { -HALF_WIDTH },
                center: stock.y,
                lo: stock.y - extent,
                hi: stock.y + extent,
            }
        }
        Face::Top | Face::Bottom => {
            let extent = HALF_WIDTH - CORNER_CLEARANCE;
            let bottom = face == Face::Bottom;
            FaceAttachment {
                face,
                normal: Axis::Y,
                along: Axis::X,
                sign: if bottom { 1.0 } else { -1.0 },
                plane: stock.y + if bottom { HALF_HEIGHT } else { -HALF_HEIGHT },
                center: stock.x,
                lo: stock.x - extent,
                hi: stock.x + extent,
            }
        }
    }
}

/// The normal-axis coordinate of a stub `min` long leaving the face.
pub(crate) fn stub_tip(att: &FaceAttachment, min: f64) -> f64 {
    att.plane + att.sign * min
}

/// The endpoint at `along` on the face, clamped into the valid range.
pub(crate) fn face_point(att: &FaceAttachment, along: f64) -> Point {
    att.along.compose(clamp(along, att.lo, att.hi), att.plane)
}

/// The face an endpoint lies on (within `GEOMETRY_EPSILON`), or `None` when it
/// is on none. A corner point is on two faces; when `adjacent` is given the one
/// the adjacent segment leaves perpendicular to wins, else the side face.
pub(crate) fn face_of_endpoint(stock: Point, p: Point, adjacent: Option<Point>) -> Option<Face> {
    let dx = p.x - stock.x;
    let dy = p.y - stock.y;
    let e = GEOMETRY_EPSILON;
    let side = ((dx.abs() - HALF_WIDTH).abs() <= e && dy.abs() <= HALF_HEIGHT + e)
        .then_some(if dx > 0.0 { Face::Right } else { Face::Left });
    let cap = ((dy.abs() - HALF_HEIGHT).abs() <= e && dx.abs() <= HALF_WIDTH + e)
        .then_some(if dy > 0.0 { Face::Bottom } else { Face::Top });
    match (side, cap, adjacent) {
        // At a corner the adjacent segment's axis picks: a horizontal segment
        // leaves a side face perpendicular, a vertical one a cap.
        (Some(side), Some(cap), Some(q)) => match Axis::of_segment(p, q) {
            Axis::X => Some(side),
            Axis::Y => Some(cap),
        },
        (Some(side), _, _) => Some(side),
        (None, cap, _) => cap,
    }
}

/// The nearest valid face point to `p` over all four faces. When `adjacent` is
/// given, a tie (a corner point, equidistant from two faces) goes to the face
/// the adjacent segment leaves perpendicular to, so a stub keeps its direction.
pub(crate) fn nearest_face_attachment(
    stock: Point,
    p: Point,
    adjacent: Option<Point>,
) -> (Face, Point) {
    let preferred = adjacent.map(|q| Axis::of_segment(p, q));
    let mut best = (
        Face::Left,
        face_point(&face_attachment(stock, Face::Left), p.y),
    );
    let mut best_distance = f64::INFINITY;
    for face in Face::ALL {
        let att = face_attachment(stock, face);
        let point = face_point(&att, p.coord(att.along));
        let d = point.distance(p);
        let tie = (d - best_distance).abs() <= GEOMETRY_EPSILON;
        if d < best_distance - GEOMETRY_EPSILON || (tie && preferred == Some(att.normal)) {
            best_distance = d.min(best_distance);
            best = (face, point);
        }
    }
    best
}

/// A cloud as a free terminal carries it: its uid and its center as the view
/// holds it, which an operation moving the endpoint moves onto the endpoint.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct CloudRef {
    pub uid: i32,
    pub at: Point,
}

/// One end of a flow as routing sees it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Terminal {
    /// A stock. `face` and `offset` describe the BASE flow's attachment
    /// (`offset` is the endpoint's along-face distance from the face center),
    /// so stickiness needs no previous frame; both are `None` for a stock the
    /// flow was not attached to before (a new target).
    Stock {
        uid: i32,
        center: Point,
        face: Option<Face>,
        offset: Option<f64>,
    },
    /// A cloud, or a point with no element (the pointer before a cloud
    /// exists). The endpoint is attached to `cloud` when there is one.
    Free {
        point: Point,
        cloud: Option<CloudRef>,
    },
}

impl Terminal {
    /// The element the endpoint attaches to.
    pub(crate) fn uid(&self) -> Option<i32> {
        match self {
            Terminal::Stock { uid, .. } => Some(*uid),
            Terminal::Free { cloud, .. } => cloud.map(|c| c.uid),
        }
    }

    pub(crate) fn body(&self) -> Bounds {
        match self {
            Terminal::Stock { center, .. } => Bounds::stock(*center),
            Terminal::Free { point, .. } => Bounds::point(*point),
        }
    }

    /// Whether every coordinate the terminal carries is finite: a NaN pointer
    /// is a caller bug, never geometry, and operations return the base flow.
    pub(crate) fn is_finite(&self) -> bool {
        match self {
            Terminal::Stock { center, offset, .. } => {
                center.is_finite() && offset.is_none_or(f64::is_finite)
            }
            Terminal::Free { point, .. } => point.is_finite(),
        }
    }

    pub(crate) fn stock_center(&self) -> Option<Point> {
        match self {
            Terminal::Stock { center, .. } => Some(*center),
            Terminal::Free { .. } => None,
        }
    }

    pub(crate) fn is_free(&self) -> bool {
        matches!(self, Terminal::Free { .. })
    }
}

/// A flow's two terminals.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Terminals {
    pub source: Terminal,
    pub sink: Terminal,
}

impl Terminals {
    pub(crate) fn both(&self) -> [&Terminal; 2] {
        [&self.source, &self.sink]
    }
}

/// A stock terminal whose base attachment is read from `endpoint` relative to
/// `from` (the stock's base position). A planner moving a stock passes the
/// moved center and the base center, so the base face and offset travel with
/// the stock. An endpoint on no face attaches to the nearest face.
pub(crate) fn stock_terminal(
    uid: i32,
    center: Point,
    endpoint: Option<Point>,
    adjacent: Option<Point>,
    from: Point,
) -> Terminal {
    let Some(endpoint) = endpoint.filter(|p| p.is_finite()) else {
        return target_stock_terminal(uid, center);
    };
    let face = face_of_endpoint(from, endpoint, adjacent)
        .unwrap_or_else(|| nearest_face_attachment(from, endpoint, adjacent).0);
    let att = face_attachment(from, face);
    Terminal::Stock {
        uid,
        center,
        face: Some(face),
        offset: Some(endpoint.coord(att.along) - att.center),
    }
}

/// A new stock target: no base attachment.
pub(crate) fn target_stock_terminal(uid: i32, center: Point) -> Terminal {
    Terminal::Stock {
        uid,
        center,
        face: None,
        offset: None,
    }
}

pub(crate) fn free_terminal(point: Point, cloud: Option<CloudRef>) -> Terminal {
    Terminal::Free { point, cloud }
}

/// A flow's terminals as the view supplies them, through `lookup` (uid to
/// element): a stock endpoint's base face and offset are read from the
/// endpoint (and its adjacent point, which picks the perpendicular face at a
/// corner); a cloud endpoint's terminal is its cloud, at the cloud's center; an
/// unattached or dangling endpoint is a free point with no cloud, which no
/// operation attaches.
pub(crate) fn flow_terminals<'a>(
    flow: &Flow,
    lookup: impl Fn(i32) -> Option<&'a ViewElement>,
) -> Terminals {
    let pts = &flow.points;
    let terminal_at = |index: usize, adjacent: usize| -> Terminal {
        let Some(p) = pts.get(index) else {
            return free_terminal(Point::new(flow.x, flow.y), None);
        };
        let adjacent = pts.get(adjacent).map(Positioned::point);
        match p.attached_to_uid.and_then(&lookup) {
            Some(ViewElement::Stock(stock)) => {
                let center = Point::new(stock.x, stock.y);
                stock_terminal(stock.uid, center, Some(p.point()), adjacent, center)
            }
            Some(ViewElement::Cloud(cloud)) => {
                let at = Point::new(cloud.x, cloud.y);
                free_terminal(at, Some(CloudRef { uid: cloud.uid, at }))
            }
            _ => free_terminal(p.point(), None),
        }
    };
    let n = pts.len();
    Terminals {
        source: terminal_at(0, 1),
        sink: terminal_at(n.saturating_sub(1), n.saturating_sub(2)),
    }
}

/// A cloud whose center an operation moved onto its endpoint.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct CloudMove {
    pub uid: i32,
    pub at: Point,
}

/// The clouds one operation moves: at most one per end.
pub(crate) type CloudMoves = SmallVec<[CloudMove; 2]>;

/// The geometry an operation produced: the new flow, plus every cloud whose
/// center changed because its endpoint moved (a cloud endpoint always equals
/// its cloud's center, G7).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct FlowGeometry {
    pub flow: Flow,
    pub clouds: CloudMoves,
}

impl FlowGeometry {
    /// The base flow, unchanged.
    pub(crate) fn unchanged(flow: &Flow) -> FlowGeometry {
        FlowGeometry {
            flow: flow.clone(),
            clouds: CloudMoves::new(),
        }
    }
}

pub(crate) fn cloud_updates(points: &[Point], terminals: &Terminals) -> CloudMoves {
    let mut out = CloudMoves::new();
    let (Some(&first), Some(&last)) = (points.first(), points.last()) else {
        return out;
    };
    for (t, p) in [(terminals.source, first), (terminals.sink, last)] {
        // An exact comparison: a cloud a float away from its endpoint is still
        // moved onto it, so G7 holds exactly after every operation.
        if let Terminal::Free {
            cloud: Some(cloud), ..
        } = t
            && (cloud.at.x != p.x || cloud.at.y != p.y)
        {
            out.push(CloudMove {
                uid: cloud.uid,
                at: p,
            });
        }
    }
    out
}

pub(crate) fn attach_points(points: &[Point], terminals: &Terminals) -> Vec<FlowPoint> {
    let last = points.len().saturating_sub(1);
    points
        .iter()
        .enumerate()
        .map(|(i, p)| FlowPoint {
            x: p.x,
            y: p.y,
            attached_to_uid: if i == 0 {
                terminals.source.uid()
            } else if i == last {
                terminals.sink.uid()
            } else {
                None
            },
        })
        .collect()
}

/// `flow` with `points` attached to `terminals` and its valve at `valve`, plus
/// the clouds the new endpoints moved.
pub(crate) fn with_geometry(
    flow: &Flow,
    points: &[Point],
    valve: Point,
    terminals: &Terminals,
) -> FlowGeometry {
    let mut next = flow.clone();
    next.x = valve.x;
    next.y = valve.y;
    next.points = attach_points(points, terminals);
    FlowGeometry {
        flow: next,
        clouds: cloud_updates(points, terminals),
    }
}

/// A flow's points as geometry.
pub(crate) fn path_of(flow: &Flow) -> Path {
    flow.points.iter().map(Positioned::point).collect()
}
