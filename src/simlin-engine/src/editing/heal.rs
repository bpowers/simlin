// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `heal`: repairing a flow an edit is about to route (imported or legacy data).

use crate::datamodel::view_element::Flow;

use super::geometry::{
    Axis, Bounds, FlowEnd, GEOMETRY_EPSILON, NoObstacles, Obstacles, Path, Point,
    VALVE_CLAMP_MARGIN,
};
use super::path::{
    apply_valve_margin, arc_position, distance_to_path, normalize, path_length, place_valve,
    point_at_arc,
};
use super::route::{route, route_between};
use super::terminal::{
    FlowGeometry, Terminal, Terminals, cloud_updates, face_attachment, face_of_endpoint,
    face_point, nearest_face_attachment, path_of, with_geometry,
};
use super::validity::{Fault, path_quality};

/// Repair a flow an edit is about to route. The identity on a valid flow, and
/// idempotent.
///
/// In order: attach the endpoints to their terminals (a stock endpoint off its
/// face, or inside the corner clearance, is re-pinned to the nearest valid
/// point, along the face its adjacent segment is perpendicular to when there is
/// one; a cloud is moved onto its endpoint rather than the pipe onto the cloud,
/// and out of any stock it would sit inside), snap slightly-diagonal segments
/// along their dominant axis, normalize, and only when the result still
/// violates G2-G6, re-route. The valve is re-projected onto the healed path and
/// the margin applied. A non-finite terminal returns the flow unchanged;
/// non-finite points (or fewer than two) route afresh.
pub(crate) fn heal<O: Obstacles + ?Sized>(
    flow: &Flow,
    terminals: &Terminals,
    stocks: &O,
) -> FlowGeometry {
    if !terminals.source.is_finite() || !terminals.sink.is_finite() {
        return FlowGeometry::unchanged(flow);
    }
    let pts = path_of(flow);
    if pts.len() < 2 || !pts.iter().all(|p| p.is_finite()) {
        let mut draft = flow.clone();
        draft.points.clear();
        return route(
            terminals.source,
            terminals.sink,
            &draft,
            FlowEnd::Source,
            &[],
            &NoObstacles,
        );
    }
    if is_healthy(flow, &pts, terminals, stocks) {
        return FlowGeometry {
            flow: flow.clone(),
            clouds: cloud_updates(&pts, terminals),
        };
    }
    let mut healed: Path = pts.clone();
    let n = healed.len();
    for (t, index, adjacent) in [(&terminals.source, 0, 1), (&terminals.sink, n - 1, n - 2)] {
        let Terminal::Stock { center, .. } = t else {
            continue;
        };
        let p = healed[index];
        let q = healed[adjacent];
        healed[index] = match face_of_endpoint(*center, p, Some(q)) {
            None => nearest_face_attachment(*center, p, Some(q)).1,
            Some(face) => {
                let att = face_attachment(*center, face);
                face_point(&att, p.coord(att.along))
            }
        };
    }
    let movable = |index: usize| {
        (index > 0 && index < n - 1)
            || (index == 0 && terminals.source.is_free())
            || (index == n - 1 && terminals.sink.is_free())
    };
    for i in 0..n - 1 {
        let (a, b) = (healed[i], healed[i + 1]);
        if (a.x - b.x).abs() <= GEOMETRY_EPSILON || (a.y - b.y).abs() <= GEOMETRY_EPSILON {
            continue;
        }
        let h = Axis::of_segment(a, b).other();
        if movable(i + 1) {
            healed[i + 1] = h.compose(a.coord(h), b.coord(h.other()));
        } else if movable(i) {
            healed[i] = h.compose(b.coord(h), a.coord(h.other()));
        }
    }
    if terminals.source.is_free() {
        healed[0] = out_of_stocks(healed[0], healed[1], stocks);
    }
    if terminals.sink.is_free() {
        healed[n - 1] = out_of_stocks(healed[n - 1], healed[n - 2], stocks);
    }
    // A free terminal is wherever its endpoint was healed to (its cloud
    // follows), so a re-route starts from there, not from the cloud's original
    // center: that center may be exactly the stock interior the endpoint was
    // just moved out of.
    let moved_terminal = |t: &Terminal, at: Point| match *t {
        Terminal::Free { cloud, .. } => Terminal::Free { point: at, cloud },
        stock => stock,
    };
    let moved = Terminals {
        source: moved_terminal(&terminals.source, healed[0]),
        sink: moved_terminal(&terminals.sink, healed[n - 1]),
    };
    let mut points = normalize(&healed);
    if path_quality(&points, &moved, stocks, &NoObstacles).fault != Fault::None {
        points = route_between(&moved, false, false, &points, &[], true, &NoObstacles);
    }
    let valve = Point::new(flow.x, flow.y);
    let valve = if valve.is_finite() {
        point_at_arc(
            &points,
            apply_valve_margin(path_length(&points), arc_position(&points, valve)),
        )
    } else {
        place_valve(&points, FlowEnd::Source, None)
    };
    with_geometry(flow, &points, valve, &moved)
}

/// A free endpoint inside a stock moved along its adjacent segment's axis to
/// the nearer edge of that stock, so the cloud lands on the boundary (not
/// inside, G6) and the segment stays orthogonal.
fn out_of_stocks<O: Obstacles + ?Sized>(p: Point, adjacent: Point, stocks: &O) -> Point {
    let mut containing: Option<Point> = None;
    stocks.any_near(Bounds::point(p), |center| {
        if Bounds::stock(center).strictly_contains(p) {
            containing = Some(center);
            true
        } else {
            false
        }
    });
    let Some(stock) = containing else {
        return p;
    };
    let axis = Axis::of_segment(p, adjacent);
    let body = Bounds::stock(stock);
    let (lo, hi) = match axis {
        Axis::X => (body.min_x, body.max_x),
        Axis::Y => (body.min_y, body.max_y),
    };
    let v = p.coord(axis);
    axis.compose(
        if v - lo <= hi - v { lo } else { hi },
        p.coord(axis.other()),
    )
}

/// A flow `heal` leaves alone: attached to its terminals, valid (G2-G6, clouds
/// outside `stocks`), and its valve on the path within the margin. A free
/// terminal always sits at its own endpoint here: `flow_terminals` reads a
/// cloud terminal at the cloud's center, and a cloud off its endpoint is
/// reported (and moved) through `cloud_updates` either way.
fn is_healthy<O: Obstacles + ?Sized>(
    flow: &Flow,
    pts: &[Point],
    terminals: &Terminals,
    stocks: &O,
) -> bool {
    let n = flow.points.len();
    if flow.points[0].attached_to_uid != terminals.source.uid()
        || flow.points[n - 1].attached_to_uid != terminals.sink.uid()
    {
        return false;
    }
    if flow.points[1..n - 1]
        .iter()
        .any(|p| p.attached_to_uid.is_some())
    {
        return false;
    }
    let valve = Point::new(flow.x, flow.y);
    if path_quality(pts, terminals, stocks, &NoObstacles).fault != Fault::None || !valve.is_finite()
    {
        return false;
    }
    if distance_to_path(pts, valve) > GEOMETRY_EPSILON {
        return false;
    }
    let length = path_length(pts);
    if length < 2.0 * VALVE_CLAMP_MARGIN {
        return true;
    }
    let s = arc_position(pts, valve);
    s.min(length - s) >= VALVE_CLAMP_MARGIN - GEOMETRY_EPSILON
}
