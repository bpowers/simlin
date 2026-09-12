// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Link arcs and label sides: a link drawn or curved through the pointer,
//! links that follow their moved endpoints, and the side a dragged label
//! snaps to.
//!
//! An arc is stored as the angle (degrees, y down) a link leaves its source at;
//! the renderer draws the circle through both elements' visual centers that
//! leaves the source at that angle (`diagram::connector::arc_circle`). Angles
//! here are measured between visual centers, where an arrayed element draws its
//! front copy.

use std::collections::HashMap;
use std::f64::consts::PI;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{FlowPoint, LabelSide, Link, LinkShape};
use crate::diagram::common::is_zero;
use crate::diagram::connector::get_visual_center;
use crate::diagram::constants::STRAIGHT_LINE_MAX;

use super::base::{BaseView, position_of};
use super::geometry::Point;

/// Endpoints moving by the same amount within this tolerance translate a link
/// rather than bend it: float noise between two routed positions is not
/// rotation.
const MOVEMENT_EQUALITY_EPSILON: f64 = 0.1;

pub(crate) fn visual_center(base: &BaseView, element: &ViewElement) -> Point {
    let (x, y) = get_visual_center(element, &|name| base.is_arrayed(name));
    Point::new(x, y)
}

/// The center of the circle through three points; `None` when they are
/// collinear.
fn circle_center(p1: Point, p2: Point, p3: Point) -> Option<Point> {
    let off = p2.x * p2.x + p2.y * p2.y;
    let bc = (p1.x * p1.x + p1.y * p1.y - off) / 2.0;
    let cd = (off - p3.x * p3.x - p3.y * p3.y) / 2.0;
    let det = (p1.x - p2.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p2.y);
    if is_zero(det) {
        return None;
    }
    let idet = 1.0 / det;
    Some(Point::new(
        (bc * (p2.y - p3.y) - cd * (p1.y - p2.y)) * idet,
        (cd * (p1.x - p2.x) - bc * (p2.x - p3.x)) * idet,
    ))
}

/// The angle (radians) a link from `from` to `to` leaves its source at to curve
/// through `through`: tangent to the circle through the three points, on the
/// side that passes the point; the straight bearing when the three are
/// collinear. `None` when `through` is the target's center, where no circle is
/// defined.
pub(crate) fn takeoff_through(from: Point, to: Point, through: Point) -> Option<f64> {
    if through == to {
        return None;
    }
    let Some(center) = circle_center(from, to, through) else {
        return Some((to.y - from.y).atan2(to.x - from.x));
    };
    let from_theta = (from.y - center.y).atan2(from.x - center.x);
    let to_theta = (to.y - center.y).atan2(to.x - center.x);
    let mut span = to_theta - from_theta;
    if span > PI {
        span -= 2.0 * PI;
    }
    let inv = span > 0.0 || span <= -PI;
    // Whether the circle's center and the pointer lie on the same side of the
    // line from source to target decides which way round the tangent points.
    let side = |p: Point| (p.x - from.x) * (to.y - from.y) - (p.y - from.y) * (to.x - from.x);
    let sweep = (side(center) < 0.0) == (side(through) < 0.0);
    let quarter = if sweep == inv { -PI / 2.0 } else { PI / 2.0 };
    Some(from_theta + quarter)
}

/// The difference `a - b` wrapped into (-pi, pi], so bearings either side of
/// the +/-180 degree seam compare as the small angle they are.
fn wrapped(a: f64, b: f64) -> f64 {
    let mut d = (a - b) % (2.0 * PI);
    if d > PI {
        d -= 2.0 * PI;
    } else if d <= -PI {
        d += 2.0 * PI;
    }
    d
}

/// The shape of a link created or reattached through `pointer`: straight when
/// its takeoff is within `STRAIGHT_LINE_MAX` of the direct bearing, otherwise
/// the arc through the pointer.
pub(crate) fn shape_through(from: Point, to: Point, pointer: Point) -> LinkShape {
    let direct = (to.y - from.y).atan2(to.x - from.x);
    match takeoff_through(from, to, pointer) {
        Some(takeoff)
            if takeoff.is_finite()
                && wrapped(direct, takeoff).abs() >= STRAIGHT_LINE_MAX.to_radians() =>
        {
            LinkShape::Arc(takeoff.to_degrees())
        }
        _ => LinkShape::Straight,
    }
}

/// Links whose endpoint elements a gesture moved, updated once from the final
/// elements: ends that moved alike keep the arc (and translate a multi-point
/// path); otherwise an arc turns with the line between the ends' visual centers,
/// so the curve keeps its shape relative to that line, and a straight or
/// multi-point link keeps its shape. A link the gesture changed itself is left
/// alone.
pub(crate) fn follow_links(base: &BaseView, changed: &HashMap<i32, ViewElement>) -> Vec<Link> {
    let mut candidates: Vec<usize> = changed
        .keys()
        .flat_map(|&uid| base.touching_link_indices(uid).iter().copied())
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    let mut out = Vec::new();
    for index in candidates {
        let ViewElement::Link(link) = &base.elements()[index] else {
            continue;
        };
        if changed.contains_key(&link.uid) {
            continue;
        }
        let (Some(old_from), Some(old_to)) = (base.get(link.from_uid), base.get(link.to_uid))
        else {
            continue;
        };
        let new_from = changed.get(&link.from_uid).unwrap_or(old_from);
        let new_to = changed.get(&link.to_uid).unwrap_or(old_to);
        let (Some(of), Some(nf), Some(ot), Some(nt)) = (
            position_of(old_from),
            position_of(new_from),
            position_of(old_to),
            position_of(new_to),
        ) else {
            continue;
        };
        let from_moved = nf.minus(of);
        let to_moved = nt.minus(ot);
        if from_moved.x == 0.0 && from_moved.y == 0.0 && to_moved.x == 0.0 && to_moved.y == 0.0 {
            continue;
        }
        let alike = (from_moved.x - to_moved.x).abs() < MOVEMENT_EQUALITY_EPSILON
            && (from_moved.y - to_moved.y).abs() < MOVEMENT_EQUALITY_EPSILON;
        let shape = match (&link.shape, alike) {
            (LinkShape::MultiPoint(points), true) => LinkShape::MultiPoint(
                points
                    .iter()
                    .map(|p| FlowPoint {
                        x: p.x + from_moved.x,
                        y: p.y + from_moved.y,
                        attached_to_uid: p.attached_to_uid,
                    })
                    .collect(),
            ),
            (LinkShape::Arc(arc), false) => {
                let bearing = |a: Point, b: Point| (b.y - a.y).atan2(b.x - a.x);
                let old_theta = bearing(visual_center(base, old_from), visual_center(base, old_to));
                let new_theta = bearing(visual_center(base, new_from), visual_center(base, new_to));
                LinkShape::Arc(arc - (old_theta - new_theta).to_degrees())
            }
            _ => continue,
        };
        out.push(Link {
            shape,
            ..link.clone()
        });
    }
    out
}

/// The side a label snaps to for a pointer at `pointer` around an element at
/// `center`: the quadrant of the direction from the pointer toward the center,
/// so a pointer left of the element puts its label on the left.
pub(crate) fn label_side_for_pointer(center: Point, pointer: Point) -> LabelSide {
    let angle = (center.y - pointer.y)
        .atan2(center.x - pointer.x)
        .to_degrees();
    if -45.0 < angle && angle <= 45.0 {
        LabelSide::Left
    } else if 45.0 < angle && angle <= 135.0 {
        LabelSide::Top
    } else if -135.0 < angle && angle <= -45.0 {
        LabelSide::Bottom
    } else {
        LabelSide::Right
    }
}
