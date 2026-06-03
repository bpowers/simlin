// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Final-pass orthogonalization of flow pipes.
//!
//! System-dynamics convention draws a flow (a stock-to-stock rate) as a "pipe"
//! made of horizontal and vertical segments only -- never a diagonal. The
//! placement and chain passes position stocks freely, so a flow between two
//! stocks that share neither an x nor a y coordinate would otherwise render as
//! a straight diagonal line. This pass rewrites any such pipe into an
//! axis-aligned poly-line (an `L` with one bend, or a `Z` with two), so the
//! orthogonality invariant holds structurally regardless of where placement put
//! the stocks.
//!
//! The pass is deliberately conservative and idempotent:
//!
//! * a pipe whose every segment is already axis-aligned is left untouched (so a
//!   hand-routed flow preserved by the incremental path is never clobbered, and
//!   re-running the pass is a no-op);
//! * only pipes that actually contain a diagonal segment are rebuilt, and they
//!   are rebuilt from their two *attached* endpoints (the faces the placement /
//!   resnap passes already chose), inserting bends so each segment leaves its
//!   stock face perpendicular.
//!
//! The valve (the flow's `(x, y)`) is left where the layout put it. For the
//! common stock-to-stock case (both endpoints on left/right faces) the rebuilt
//! `Z` route's middle segment passes through the valve column, so the valve
//! still sits on the rendered pipe.

use std::collections::HashMap;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::FlowPoint;
use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};

/// Tolerance (in diagram units) for deciding whether two coordinates are equal
/// (a segment is axis-aligned) or a point sits on a stock face.
const EPS: f64 = 1e-6;

/// Which face of a stock an attached endpoint sits on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Face {
    /// A left or right (vertical) face: the segment leaving it is horizontal.
    Horizontal,
    /// A top or bottom (horizontal) face: the segment leaving it is vertical.
    Vertical,
}

/// True when the segment `(a) -> (b)` is axis-aligned (horizontal or vertical).
fn segment_is_orthogonal(a: &FlowPoint, b: &FlowPoint) -> bool {
    (a.x - b.x).abs() < EPS || (a.y - b.y).abs() < EPS
}

/// True when every segment of `points` is axis-aligned.
fn pipe_is_orthogonal(points: &[FlowPoint]) -> bool {
    points
        .windows(2)
        .all(|w| segment_is_orthogonal(&w[0], &w[1]))
}

/// The face a stock-attached endpoint sits on, derived from its offset from the
/// stock center. After resnap an endpoint lies exactly on a face, so the
/// dominant offset axis identifies the face; we fall back to the larger
/// (aspect-normalized) offset if neither lands exactly on an edge.
fn face_of(p: &FlowPoint, stock: (f64, f64)) -> Face {
    let half_w = STOCK_WIDTH / 2.0;
    let half_h = STOCK_HEIGHT / 2.0;
    let dx = (p.x - stock.0).abs();
    let dy = (p.y - stock.1).abs();
    let on_vertical_edge = (dx - half_w).abs() < EPS && dy <= half_h + EPS;
    let on_horizontal_edge = (dy - half_h).abs() < EPS && dx <= half_w + EPS;
    if on_vertical_edge && !on_horizontal_edge {
        Face::Horizontal
    } else if on_horizontal_edge && !on_vertical_edge {
        Face::Vertical
    } else {
        // Not exactly on a single edge (a corner, or moved): pick the face the
        // aspect-normalized rule would, matching `resnap_flow_endpoints`.
        if half_h * dx >= half_w * dy {
            Face::Horizontal
        } else {
            Face::Vertical
        }
    }
}

/// Build the axis-aligned interior between `from` and `to`, given each endpoint's
/// face (if it is attached to a stock) and the valve position. Returns the full
/// poly-line including `from` and `to`.
///
/// * both Horizontal faces -> `Z` with a vertical middle segment at the valve's
///   x (so the valve sits on the pipe);
/// * both Vertical faces -> `Z` with a horizontal middle segment at the valve's
///   y;
/// * one of each -> a single-bend `L`;
/// * a free (cloud / unattached) endpoint -> the bend is driven by the attached
///   end's face so its segment still leaves the stock perpendicular.
fn route_orthogonal(
    from: FlowPoint,
    to: FlowPoint,
    from_face: Option<Face>,
    to_face: Option<Face>,
    valve: (f64, f64),
) -> Vec<FlowPoint> {
    let bend = |x: f64, y: f64| FlowPoint {
        x,
        y,
        attached_to_uid: None,
    };
    let (fx, fy) = (from.x, from.y);
    let (tx, ty) = (to.x, to.y);

    let points = match (from_face, to_face) {
        (Some(Face::Horizontal), Some(Face::Horizontal)) => {
            vec![from, bend(valve.0, fy), bend(valve.0, ty), to]
        }
        (Some(Face::Vertical), Some(Face::Vertical)) => {
            vec![from, bend(fx, valve.1), bend(tx, valve.1), to]
        }
        (Some(Face::Horizontal), Some(Face::Vertical)) => {
            vec![from, bend(tx, fy), to]
        }
        (Some(Face::Vertical), Some(Face::Horizontal)) => {
            vec![from, bend(fx, ty), to]
        }
        // One end free: route perpendicular to the attached end's face.
        (Some(Face::Horizontal), None) => vec![from, bend(tx, fy), to],
        (Some(Face::Vertical), None) => vec![from, bend(fx, ty), to],
        (None, Some(Face::Horizontal)) => vec![from, bend(fx, ty), to],
        (None, Some(Face::Vertical)) => vec![from, bend(tx, fy), to],
        // Neither attached: a plain L through the valve corner.
        (None, None) => vec![from, bend(tx, fy), to],
    };

    dedup_collinear(points)
}

/// Drop duplicate and collinear interior points so a degenerate route (e.g. a
/// `Z` whose two stocks happen to share a row) collapses to the simplest
/// poly-line.
fn dedup_collinear(points: Vec<FlowPoint>) -> Vec<FlowPoint> {
    if points.len() <= 2 {
        return points;
    }
    let mut out: Vec<FlowPoint> = Vec::with_capacity(points.len());
    for p in points {
        // Drop a point coincident with the previous one.
        if let Some(last) = out.last()
            && (last.x - p.x).abs() < EPS
            && (last.y - p.y).abs() < EPS
        {
            continue;
        }
        // Drop the middle of three collinear points.
        if out.len() >= 2 {
            let a = &out[out.len() - 2];
            let b = &out[out.len() - 1];
            let collinear_h = (a.y - b.y).abs() < EPS && (b.y - p.y).abs() < EPS;
            let collinear_v = (a.x - b.x).abs() < EPS && (b.x - p.x).abs() < EPS;
            if collinear_h || collinear_v {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// Rewrite every diagonal flow pipe in `elements` into axis-aligned segments.
///
/// Reads stock centers from `elements` (so it must run after positions are
/// final). Flows whose pipes are already orthogonal are left untouched.
pub(crate) fn orthogonalize_flow_pipes(elements: &mut [ViewElement]) {
    let stocks: HashMap<i32, (f64, f64)> = elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y))),
            _ => None,
        })
        .collect();

    for elem in elements.iter_mut() {
        let ViewElement::Flow(f) = elem else { continue };
        if f.points.len() < 2 || pipe_is_orthogonal(&f.points) {
            continue;
        }

        let from = f.points.first().unwrap().clone();
        let to = f.points.last().unwrap().clone();
        let from_face = from
            .attached_to_uid
            .and_then(|uid| stocks.get(&uid))
            .map(|&s| face_of(&from, s));
        let to_face = to
            .attached_to_uid
            .and_then(|uid| stocks.get(&uid))
            .map(|&s| face_of(&to, s));

        f.points = route_orthogonal(from, to, from_face, to_face, (f.x, f.y));
    }
}

/// The number of genuine bends (interior vertices that change the pipe's
/// direction) across all flows. A straight pipe has 0; an `L` has 1; a `Z` has
/// 2. Used by the layout-quality metric to prefer naturally-aligned stocks
/// (which need no bend) over misaligned ones.
pub(crate) fn flow_bend_count(points: &[FlowPoint]) -> usize {
    if points.len() < 3 {
        return 0;
    }
    let mut bends = 0;
    for w in points.windows(3) {
        let (a, b, c) = (&w[0], &w[1], &w[2]);
        // b is a bend unless a, b, c are collinear.
        let collinear_h = (a.y - b.y).abs() < EPS && (b.y - c.y).abs() < EPS;
        let collinear_v = (a.x - b.x).abs() < EPS && (b.x - c.x).abs() < EPS;
        if !(collinear_h || collinear_v) {
            bends += 1;
        }
    }
    bends
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element::{Flow, LabelSide, Stock};

    fn fp(x: f64, y: f64, attached: Option<i32>) -> FlowPoint {
        FlowPoint {
            x,
            y,
            attached_to_uid: attached,
        }
    }

    fn stock(uid: i32, x: f64, y: f64) -> ViewElement {
        ViewElement::Stock(Stock {
            name: format!("s{uid}"),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
            compat: None,
        })
    }

    fn flow(uid: i32, vx: f64, vy: f64, points: Vec<FlowPoint>) -> ViewElement {
        ViewElement::Flow(Flow {
            name: format!("f{uid}"),
            uid,
            x: vx,
            y: vy,
            label_side: LabelSide::Bottom,
            points,
            compat: None,
            label_compat: None,
        })
    }

    fn flow_points(elem: &ViewElement) -> &[FlowPoint] {
        match elem {
            ViewElement::Flow(f) => &f.points,
            _ => panic!("not a flow"),
        }
    }

    fn assert_orthogonal(points: &[FlowPoint]) {
        for w in points.windows(2) {
            assert!(
                segment_is_orthogonal(&w[0], &w[1]),
                "segment ({:.1},{:.1})->({:.1},{:.1}) is diagonal",
                w[0].x,
                w[0].y,
                w[1].x,
                w[1].y
            );
        }
    }

    /// Two stocks on the same row -> the flow is already a horizontal pipe and
    /// must be left exactly as-is.
    #[test]
    fn aligned_horizontal_flow_is_untouched() {
        let half_w = STOCK_WIDTH / 2.0;
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            stock(2, 300.0, 100.0),
            flow(
                3,
                200.0,
                100.0,
                vec![
                    fp(100.0 + half_w, 100.0, Some(1)),
                    fp(300.0 - half_w, 100.0, Some(2)),
                ],
            ),
        ];
        let before = flow_points(&elements[2]).to_vec();
        orthogonalize_flow_pipes(&mut elements);
        assert_eq!(flow_points(&elements[2]), before.as_slice());
    }

    /// Two stocks offset in both x and y, both endpoints snapped to left/right
    /// faces -> a Z route with a vertical middle segment through the valve.
    #[test]
    fn diagonal_stock_pair_becomes_orthogonal_z() {
        let half_w = STOCK_WIDTH / 2.0;
        // A at (100,100), B at (300,250). Both endpoints on horizontal faces.
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            stock(2, 300.0, 250.0),
            flow(
                3,
                200.0,
                175.0,
                vec![
                    fp(100.0 + half_w, 100.0, Some(1)),
                    fp(300.0 - half_w, 250.0, Some(2)),
                ],
            ),
        ];
        orthogonalize_flow_pipes(&mut elements);
        let pts = flow_points(&elements[2]);
        assert_orthogonal(pts);
        assert!(pts.len() >= 3, "expected a bend, got {} points", pts.len());
        // Endpoints stay attached to their stocks.
        assert_eq!(pts.first().unwrap().attached_to_uid, Some(1));
        assert_eq!(pts.last().unwrap().attached_to_uid, Some(2));
        // The valve (200,175) lies on the vertical middle segment x=200.
        assert!(pts.iter().any(|p| (p.x - 200.0).abs() < EPS));
    }

    /// One endpoint on a horizontal face, the other on a vertical face -> a
    /// single-bend L.
    #[test]
    fn mixed_faces_become_single_bend_l() {
        let half_w = STOCK_WIDTH / 2.0;
        let half_h = STOCK_HEIGHT / 2.0;
        // A exits its right face; B is entered from its top face.
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            stock(2, 300.0, 300.0),
            flow(
                3,
                200.0,
                200.0,
                vec![
                    fp(100.0 + half_w, 100.0, Some(1)),
                    fp(300.0, 300.0 - half_h, Some(2)),
                ],
            ),
        ];
        orthogonalize_flow_pipes(&mut elements);
        let pts = flow_points(&elements[2]);
        assert_orthogonal(pts);
        assert_eq!(pts.len(), 3, "an L should have exactly one bend");
        assert_eq!(flow_bend_count(pts), 1);
    }

    /// Re-running the pass on an already-orthogonalized pipe changes nothing.
    #[test]
    fn orthogonalization_is_idempotent() {
        let half_w = STOCK_WIDTH / 2.0;
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            stock(2, 300.0, 250.0),
            flow(
                3,
                200.0,
                175.0,
                vec![
                    fp(100.0 + half_w, 100.0, Some(1)),
                    fp(300.0 - half_w, 250.0, Some(2)),
                ],
            ),
        ];
        orthogonalize_flow_pipes(&mut elements);
        let once = flow_points(&elements[2]).to_vec();
        orthogonalize_flow_pipes(&mut elements);
        let twice = flow_points(&elements[2]).to_vec();
        assert_eq!(once, twice);
    }

    /// A stock-to-cloud diagonal flow (only one attached endpoint) still
    /// becomes orthogonal.
    #[test]
    fn stock_to_cloud_diagonal_becomes_orthogonal() {
        let half_w = STOCK_WIDTH / 2.0;
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            // last point is unattached (a cloud sink), offset in both axes.
            flow(
                3,
                160.0,
                140.0,
                vec![fp(100.0 + half_w, 100.0, Some(1)), fp(220.0, 180.0, None)],
            ),
        ];
        orthogonalize_flow_pipes(&mut elements);
        let pts = flow_points(&elements[1]);
        assert_orthogonal(pts);
    }

    #[test]
    fn flow_bend_count_counts_direction_changes() {
        // Straight: 0 bends.
        assert_eq!(
            flow_bend_count(&[fp(0.0, 0.0, None), fp(100.0, 0.0, None)]),
            0
        );
        // L: 1 bend.
        assert_eq!(
            flow_bend_count(&[
                fp(0.0, 0.0, None),
                fp(100.0, 0.0, None),
                fp(100.0, 50.0, None),
            ]),
            1
        );
        // Z: 2 bends.
        assert_eq!(
            flow_bend_count(&[
                fp(0.0, 0.0, None),
                fp(50.0, 0.0, None),
                fp(50.0, 50.0, None),
                fp(100.0, 50.0, None),
            ]),
            2
        );
        // Collinear interior point: not a bend.
        assert_eq!(
            flow_bend_count(&[
                fp(0.0, 0.0, None),
                fp(50.0, 0.0, None),
                fp(100.0, 0.0, None),
            ]),
            0
        );
    }
}
