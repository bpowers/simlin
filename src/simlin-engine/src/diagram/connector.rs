// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::f64::consts::PI;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{self, LinkShape};
use crate::diagram::arrowhead::{ArrowheadType, render_arrowhead};
use crate::diagram::common::{
    Circle, Point, deg_to_rad, escape_xml_attr, is_inf, is_zero, js_format_number, rad_to_deg,
    square,
};
use crate::diagram::constants::*;

enum ElementShape {
    Circle { r: f64 },
    Rect { hw: f64, hh: f64 },
}

fn element_shape(element: &ViewElement) -> ElementShape {
    match element {
        ViewElement::Stock(_) => ElementShape::Rect {
            hw: STOCK_WIDTH / 2.0,
            hh: STOCK_HEIGHT / 2.0,
        },
        ViewElement::Module(_) => ElementShape::Rect {
            hw: MODULE_WIDTH / 2.0,
            hh: MODULE_HEIGHT / 2.0,
        },
        _ => ElementShape::Circle { r: AUX_RADIUS },
    }
}

fn ray_rect_intersection(cx: f64, cy: f64, hw: f64, hh: f64, theta: f64) -> Point {
    let cos_t = theta.cos();
    let sin_t = theta.sin();

    let t = if is_zero(cos_t) {
        hh / sin_t.abs()
    } else if is_zero(sin_t) {
        hw / cos_t.abs()
    } else {
        let t_x = hw / cos_t.abs();
        let t_y = hh / sin_t.abs();
        t_x.min(t_y)
    };

    Point {
        x: cx + t * cos_t,
        y: cy + t * sin_t,
    }
}

fn circle_rect_intersections(circ: &Circle, cx: f64, cy: f64, hw: f64, hh: f64) -> Vec<Point> {
    let eps = 1e-9;
    let mut points = Vec::with_capacity(8);

    // Horizontal edges (y = cy +/- hh)
    for y_edge in [cy - hh, cy + hh] {
        let dy = y_edge - circ.y;
        let disc = circ.r * circ.r - dy * dy;
        if disc >= 0.0 {
            let sqrt_disc = disc.sqrt();
            for x in [circ.x + sqrt_disc, circ.x - sqrt_disc] {
                if x >= cx - hw - eps && x <= cx + hw + eps {
                    points.push(Point { x, y: y_edge });
                }
            }
        }
    }

    // Vertical edges (x = cx +/- hw)
    for x_edge in [cx - hw, cx + hw] {
        let dx = x_edge - circ.x;
        let disc = circ.r * circ.r - dx * dx;
        if disc >= 0.0 {
            let sqrt_disc = disc.sqrt();
            for y in [circ.y + sqrt_disc, circ.y - sqrt_disc] {
                if y >= cy - hh - eps && y <= cy + hh + eps {
                    let is_dup = points
                        .iter()
                        .any(|p| (p.x - x_edge).abs() < eps && (p.y - y).abs() < eps);
                    if !is_dup {
                        points.push(Point { x: x_edge, y });
                    }
                }
            }
        }
    }

    points
}

fn is_element_arrayed(element: &ViewElement, is_arrayed_fn: &dyn Fn(&str) -> bool) -> bool {
    match element {
        ViewElement::Aux(a) => is_arrayed_fn(&a.name),
        ViewElement::Stock(s) => is_arrayed_fn(&s.name),
        ViewElement::Flow(f) => is_arrayed_fn(&f.name),
        _ => false,
    }
}

fn get_visual_center(element: &ViewElement, is_arrayed_fn: &dyn Fn(&str) -> bool) -> (f64, f64) {
    let (cx, cy) = match element {
        ViewElement::Aux(a) => (a.x, a.y),
        ViewElement::Stock(s) => (s.x, s.y),
        ViewElement::Flow(f) => (f.x, f.y),
        ViewElement::Module(m) => (m.x, m.y),
        ViewElement::Alias(a) => (a.x, a.y),
        ViewElement::Cloud(c) => (c.x, c.y),
        ViewElement::Link(_) | ViewElement::Group(_) => (0.0, 0.0),
    };

    let offset = if is_element_arrayed(element, is_arrayed_fn) {
        ARRAYED_OFFSET
    } else {
        0.0
    };

    (cx - offset, cy - offset)
}

#[cfg(test)]
fn circle_from_points(p1: Point, p2: Point, p3: Point) -> Result<Circle, &'static str> {
    let off = square(p2.x) + square(p2.y);
    let bc = (square(p1.x) + square(p1.y) - off) / 2.0;
    let cd = (off - square(p3.x) - square(p3.y)) / 2.0;
    let det = (p1.x - p2.x) * (p2.y - p3.y) - (p2.x - p3.x) * (p1.y - p2.y);

    if is_zero(det) {
        return Err("zero determinant");
    }

    let idet = 1.0 / det;
    let cx = (bc * (p2.y - p3.y) - cd * (p1.y - p2.y)) * idet;
    let cy = (cd * (p1.x - p2.x) - bc * (p2.x - p3.x)) * idet;
    let r = (square(p2.x - cx) + square(p2.y - cy)).sqrt();

    Ok(Circle { x: cx, y: cy, r })
}

fn opposite_theta(theta: f64) -> f64 {
    let mut t = theta + PI;
    if t > PI {
        t -= 2.0 * PI;
    }
    t
}

fn intersect_element_straight(
    element: &ViewElement,
    theta: f64,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> Point {
    let (cx, cy) = get_visual_center(element, is_arrayed_fn);

    match element_shape(element) {
        ElementShape::Circle { r } => Point {
            x: cx + r * theta.cos(),
            y: cy + r * theta.sin(),
        },
        ElementShape::Rect { hw, hh } => ray_rect_intersection(cx, cy, hw, hh, theta),
    }
}

fn intersect_element_arc(
    element: &ViewElement,
    circ: &Circle,
    inv: bool,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> Point {
    let (cx, cy) = get_visual_center(element, is_arrayed_fn);

    match element_shape(element) {
        ElementShape::Circle { r } => {
            // Matches TypeScript: Math.tan(r / circ.r), not atan
            let off_theta = (r / circ.r).tan();
            let element_center_theta = (cy - circ.y).atan2(cx - circ.x);
            let theta = element_center_theta + if inv { 1.0 } else { -1.0 } * off_theta;

            Point {
                x: circ.x + circ.r * theta.cos(),
                y: circ.y + circ.r * theta.sin(),
            }
        }
        ElementShape::Rect { hw, hh } => {
            let intersections = circle_rect_intersections(circ, cx, cy, hw, hh);
            if intersections.is_empty() {
                let dir = (cy - circ.y).atan2(cx - circ.x);
                return ray_rect_intersection(cx, cy, hw, hh, dir);
            }

            // Use a reference point on the arc to pick the correct intersection.
            // atan (not tan) gives a monotonic, bounded angular offset that avoids
            // discontinuities when r_approx/circ.r crosses tan's asymptotes.
            let r_approx = hw.max(hh);
            let off_theta = (r_approx / circ.r).atan();
            let element_center_theta = (cy - circ.y).atan2(cx - circ.x);
            let target_theta = element_center_theta + if inv { 1.0 } else { -1.0 } * off_theta;
            let target = Point {
                x: circ.x + circ.r * target_theta.cos(),
                y: circ.y + circ.r * target_theta.sin(),
            };

            intersections
                .into_iter()
                .min_by(|a, b| {
                    let da = square(a.x - target.x) + square(a.y - target.y);
                    let db = square(b.x - target.x) + square(b.y - target.y);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap()
        }
    }
}

fn is_straight_line(
    element: &view_element::Link,
    from: &ViewElement,
    to: &ViewElement,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> bool {
    match &element.shape {
        LinkShape::Straight => true,
        LinkShape::Arc(arc) => {
            let from_visual = get_visual_center(from, is_arrayed_fn);
            let to_visual = get_visual_center(to, is_arrayed_fn);
            let mid_theta = (to_visual.1 - from_visual.1).atan2(to_visual.0 - from_visual.0);
            let takeoff_angle = deg_to_rad(*arc);
            (mid_theta - takeoff_angle).abs() < deg_to_rad(STRAIGHT_LINE_MAX)
        }
        LinkShape::MultiPoint(_) => false,
    }
}

fn arc_circle(
    element: &view_element::Link,
    from: &ViewElement,
    to: &ViewElement,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> Option<Circle> {
    let from_visual = get_visual_center(from, is_arrayed_fn);
    let to_visual = get_visual_center(to, is_arrayed_fn);

    let arc = match &element.shape {
        LinkShape::Arc(a) => *a,
        _ => return None,
    };

    let slope_takeoff = deg_to_rad(arc).tan();
    let mut slope_perp_to_takeoff = -1.0 / slope_takeoff;
    if is_zero(slope_perp_to_takeoff) {
        slope_perp_to_takeoff = 0.0;
    } else if is_inf(slope_perp_to_takeoff) {
        slope_perp_to_takeoff = if slope_perp_to_takeoff > 0.0 {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        };
    }

    let b_from = from_visual.1 - slope_perp_to_takeoff * from_visual.0;

    let (cx, cy) = if from_visual.1 == to_visual.1 {
        let cx = (from_visual.0 + to_visual.0) / 2.0;
        let cy = slope_perp_to_takeoff * cx + b_from;
        (cx, cy)
    } else {
        let slope_bisector = (from_visual.1 - to_visual.1) / (from_visual.0 - to_visual.0);
        let slope_perp_to_bisector = -1.0 / slope_bisector;
        let midx = (from_visual.0 + to_visual.0) / 2.0;
        let midy = (from_visual.1 + to_visual.1) / 2.0;
        let b_perp = midy - slope_perp_to_bisector * midx;

        if is_inf(slope_perp_to_takeoff) {
            let cx = from_visual.0;
            let cy = slope_perp_to_bisector * cx + b_perp;
            (cx, cy)
        } else {
            let cx = (b_from - b_perp) / (slope_perp_to_bisector - slope_perp_to_takeoff);
            let cy = slope_perp_to_takeoff * cx + b_from;
            (cx, cy)
        }
    };

    let cr = (square(from_visual.0 - cx) + square(from_visual.1 - cy)).sqrt();
    Some(Circle {
        x: cx,
        y: cy,
        r: cr,
    })
}

fn render_straight_line(
    _element: &view_element::Link,
    from: &ViewElement,
    to: &ViewElement,
    is_to_stock: bool,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> String {
    let from_visual = get_visual_center(from, is_arrayed_fn);
    let to_visual = get_visual_center(to, is_arrayed_fn);
    let theta = (to_visual.1 - from_visual.1).atan2(to_visual.0 - from_visual.0);
    let start = intersect_element_straight(from, theta, is_arrayed_fn);
    let end = intersect_element_straight(to, opposite_theta(theta), is_arrayed_fn);

    let arrow_theta = rad_to_deg(theta);
    let path = format!(
        "M{},{}L{},{}",
        js_format_number(start.x),
        js_format_number(start.y),
        js_format_number(end.x),
        js_format_number(end.y)
    );

    let connector_class = if is_to_stock {
        "simlin-connector simlin-connector-dashed"
    } else {
        "simlin-connector"
    };

    let mut svg = String::new();
    svg.push_str("<g>");
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"simlin-connector-bg\"></path>",
        escape_xml_attr(&path)
    ));
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"{}\"></path>",
        escape_xml_attr(&path),
        connector_class
    ));
    svg.push_str(&render_arrowhead(
        end.x,
        end.y,
        arrow_theta,
        ARROWHEAD_RADIUS,
        ArrowheadType::Connector,
    ));
    svg.push_str("</g>");
    svg
}

fn render_arc(
    element: &view_element::Link,
    from: &ViewElement,
    to: &ViewElement,
    is_to_stock: bool,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> String {
    let from_visual = get_visual_center(from, is_arrayed_fn);
    let to_visual = get_visual_center(to, is_arrayed_fn);

    let circ = match arc_circle(element, from, to, is_arrayed_fn) {
        Some(c) => c,
        None => return "<g></g>".to_string(),
    };

    let takeoff_angle = match &element.shape {
        LinkShape::Arc(arc) => deg_to_rad(*arc),
        _ => return "<g></g>".to_string(),
    };

    let from_theta = (from_visual.1 - circ.y).atan2(from_visual.0 - circ.x);
    let to_theta = (to_visual.1 - circ.y).atan2(to_visual.0 - circ.x);
    let mut span_theta = to_theta - from_theta;
    if span_theta > deg_to_rad(180.0) {
        span_theta -= deg_to_rad(360.0);
    }

    let mut inv = span_theta > 0.0 || span_theta <= deg_to_rad(-180.0);

    let side1 = (circ.x - from_visual.0) * (to_visual.1 - from_visual.1)
        - (circ.y - from_visual.1) * (to_visual.0 - from_visual.0);
    let start_a = intersect_element_arc(from, &circ, inv, is_arrayed_fn);
    let start_r = (square(start_a.x - from_visual.0) + square(start_a.y - from_visual.1)).sqrt();
    let takeoff_point = Point {
        x: from_visual.0 + start_r * takeoff_angle.cos(),
        y: from_visual.1 + start_r * takeoff_angle.sin(),
    };
    let side2 = (takeoff_point.x - from_visual.0) * (to_visual.1 - from_visual.1)
        - (takeoff_point.y - from_visual.1) * (to_visual.0 - from_visual.0);

    let sweep = (side1 < 0.0) == (side2 < 0.0);
    if sweep {
        inv = !inv;
    }

    let start = Point {
        x: from_visual.0,
        y: from_visual.1,
    };
    let arc_end = Point {
        x: to_visual.0,
        y: to_visual.1,
    };
    let end = intersect_element_arc(to, &circ, !inv, is_arrayed_fn);

    let path = format!(
        "M{},{}A{},{} 0 {},{} {},{}",
        js_format_number(start.x),
        js_format_number(start.y),
        js_format_number(circ.r),
        js_format_number(circ.r),
        sweep as u8,
        inv as u8,
        js_format_number(arc_end.x),
        js_format_number(arc_end.y)
    );

    let mut arrow_theta = rad_to_deg((end.y - circ.y).atan2(end.x - circ.x)) - 90.0;
    if inv {
        arrow_theta += 180.0;
    }

    let connector_class = if is_to_stock {
        "simlin-connector simlin-connector-dashed"
    } else {
        "simlin-connector"
    };

    let mut svg = String::new();
    svg.push_str("<g>");
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"simlin-connector-bg\"></path>",
        escape_xml_attr(&path)
    ));
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"{}\"></path>",
        escape_xml_attr(&path),
        connector_class
    ));
    svg.push_str(&render_arrowhead(
        end.x,
        end.y,
        arrow_theta,
        ARROWHEAD_RADIUS,
        ArrowheadType::Connector,
    ));
    svg.push_str("</g>");
    svg
}

pub fn render_connector(
    element: &view_element::Link,
    from: &ViewElement,
    to: &ViewElement,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> String {
    let is_to_stock = matches!(to, ViewElement::Stock(_));

    if is_straight_line(element, from, to, is_arrayed_fn) {
        render_straight_line(element, from, to, is_to_stock, is_arrayed_fn)
    } else {
        render_arc(element, from, to, is_to_stock, is_arrayed_fn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element::LabelSide;

    fn make_aux_ve(x: f64, y: f64, name: &str, uid: i32) -> ViewElement {
        ViewElement::Aux(view_element::Aux {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
        })
    }

    fn make_stock_ve(x: f64, y: f64, name: &str, uid: i32) -> ViewElement {
        ViewElement::Stock(view_element::Stock {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
        })
    }

    fn not_arrayed(_name: &str) -> bool {
        false
    }

    #[test]
    fn test_circle_from_points() {
        let p1 = Point { x: 0.0, y: 1.0 };
        let p2 = Point { x: 1.0, y: 0.0 };
        let p3 = Point { x: -1.0, y: 0.0 };
        let c = circle_from_points(p1, p2, p3).unwrap();
        assert!((c.x - 0.0).abs() < 1e-6);
        assert!((c.y - 0.0).abs() < 1e-6);
        assert!((c.r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_circle_from_collinear_points() {
        let p1 = Point { x: 0.0, y: 0.0 };
        let p2 = Point { x: 1.0, y: 1.0 };
        let p3 = Point { x: 2.0, y: 2.0 };
        assert!(circle_from_points(p1, p2, p3).is_err());
    }

    #[test]
    fn test_render_connector_straight() {
        let link = view_element::Link {
            uid: 10,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
            polarity: None,
        };
        let from = make_aux_ve(100.0, 100.0, "a", 1);
        let to = make_aux_ve(200.0, 100.0, "b", 2);

        let svg = render_connector(&link, &from, &to, &not_arrayed);
        assert!(svg.contains("simlin-connector"));
        assert!(svg.contains("simlin-arrowhead-link"));
        assert!(!svg.contains("simlin-connector-dashed"));
    }

    #[test]
    fn test_render_connector_to_stock_is_dashed() {
        let link = view_element::Link {
            uid: 10,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Straight,
            polarity: None,
        };
        let from = make_aux_ve(100.0, 100.0, "a", 1);
        let to = make_stock_ve(200.0, 100.0, "b", 2);

        let svg = render_connector(&link, &from, &to, &not_arrayed);
        assert!(svg.contains("simlin-connector-dashed"));
    }

    #[test]
    fn test_render_connector_arc() {
        let link = view_element::Link {
            uid: 10,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Arc(30.0),
            polarity: None,
        };
        let from = make_aux_ve(100.0, 100.0, "a", 1);
        let to = make_aux_ve(200.0, 200.0, "b", 2);

        let svg = render_connector(&link, &from, &to, &not_arrayed);
        assert!(svg.contains("<g>"));
        assert!(svg.contains("simlin-arrowhead-link"));
    }

    // --- ray_rect_intersection tests ---

    fn assert_on_rect_boundary(p: Point, cx: f64, cy: f64, hw: f64, hh: f64) {
        let on_left_right = ((p.x - cx).abs() - hw).abs() < 1e-6;
        let on_top_bottom = ((p.y - cy).abs() - hh).abs() < 1e-6;
        assert!(
            on_left_right || on_top_bottom,
            "Point ({}, {}) is not on boundary of rect centered ({}, {}) with hw={}, hh={}",
            p.x,
            p.y,
            cx,
            cy,
            hw,
            hh
        );
        assert!(
            (p.x - cx).abs() <= hw + 1e-6,
            "x={} is outside rect x-range",
            p.x
        );
        assert!(
            (p.y - cy).abs() <= hh + 1e-6,
            "y={} is outside rect y-range",
            p.y
        );
    }

    #[test]
    fn test_ray_rect_cardinal_right() {
        let p = ray_rect_intersection(100.0, 100.0, 22.5, 17.5, 0.0);
        assert!((p.x - 122.5).abs() < 1e-6);
        assert!((p.y - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_ray_rect_cardinal_down() {
        let p = ray_rect_intersection(100.0, 100.0, 22.5, 17.5, PI / 2.0);
        assert!((p.x - 100.0).abs() < 1e-6);
        assert!((p.y - 117.5).abs() < 1e-6);
    }

    #[test]
    fn test_ray_rect_cardinal_left() {
        let p = ray_rect_intersection(100.0, 100.0, 22.5, 17.5, PI);
        assert!((p.x - 77.5).abs() < 1e-6);
        assert!((p.y - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_ray_rect_cardinal_up() {
        let p = ray_rect_intersection(100.0, 100.0, 22.5, 17.5, -PI / 2.0);
        assert!((p.x - 100.0).abs() < 1e-6);
        assert!((p.y - 82.5).abs() < 1e-6);
    }

    #[test]
    fn test_ray_rect_diagonal_45_hits_top_bottom() {
        // For a 45x35 rect (hw=22.5, hh=17.5), 45 degrees hits the
        // right edge since hw > hh: t_x = 22.5, t_y = 17.5, min is t_y
        // Wait: t_x = hw/|cos(45)| = 22.5/0.707 = 31.8
        //       t_y = hh/|sin(45)| = 17.5/0.707 = 24.7
        // So t_y < t_x, ray hits top/bottom edge first
        let p = ray_rect_intersection(0.0, 0.0, 22.5, 17.5, PI / 4.0);
        assert!((p.y - 17.5).abs() < 1e-6, "should hit bottom edge");
        assert_on_rect_boundary(p, 0.0, 0.0, 22.5, 17.5);
    }

    #[test]
    fn test_ray_rect_preserves_angle() {
        let cx = 50.0;
        let cy = 80.0;
        for angle_deg in [15.0, 30.0, 60.0, 75.0, 120.0, 210.0, 300.0, 350.0] {
            let theta = deg_to_rad(angle_deg);
            let p = ray_rect_intersection(cx, cy, 22.5, 17.5, theta);
            let actual_theta = (p.y - cy).atan2(p.x - cx);
            let diff = (actual_theta - theta).abs();
            let diff = if diff > PI { 2.0 * PI - diff } else { diff };
            assert!(
                diff < 1e-6,
                "angle mismatch at {} degrees: expected {}, got {}",
                angle_deg,
                theta,
                actual_theta
            );
            assert_on_rect_boundary(p, cx, cy, 22.5, 17.5);
        }
    }

    // --- circle_rect_intersections tests ---

    fn assert_on_circle(p: Point, circ: &Circle) {
        let dist = (square(p.x - circ.x) + square(p.y - circ.y)).sqrt();
        assert!(
            (dist - circ.r).abs() < 1e-6,
            "Point ({}, {}) is not on circle center ({}, {}) r={}. dist={}",
            p.x,
            p.y,
            circ.x,
            circ.y,
            circ.r,
            dist
        );
    }

    #[test]
    fn test_circle_rect_no_intersection() {
        // Circle far from rectangle
        let circ = Circle {
            x: 500.0,
            y: 500.0,
            r: 10.0,
        };
        let points = circle_rect_intersections(&circ, 0.0, 0.0, 22.5, 17.5);
        assert!(points.is_empty());
    }

    #[test]
    fn test_circle_rect_inside_no_intersection() {
        // Circle entirely inside rectangle
        let circ = Circle {
            x: 0.0,
            y: 0.0,
            r: 5.0,
        };
        let points = circle_rect_intersections(&circ, 0.0, 0.0, 22.5, 17.5);
        assert!(points.is_empty());
    }

    #[test]
    fn test_circle_rect_crosses_right_edge() {
        // Circle centered to the right, crossing the right edge
        let circ = Circle {
            x: 30.0,
            y: 0.0,
            r: 10.0,
        };
        let points = circle_rect_intersections(&circ, 0.0, 0.0, 22.5, 17.5);
        assert!(!points.is_empty());
        for p in &points {
            assert_on_circle(*p, &circ);
            assert_on_rect_boundary(*p, 0.0, 0.0, 22.5, 17.5);
        }
    }

    #[test]
    fn test_circle_rect_large_circle_multiple_edges() {
        // Large circle centered outside, crossing multiple edges
        let circ = Circle {
            x: 50.0,
            y: 0.0,
            r: 50.0,
        };
        let points = circle_rect_intersections(&circ, 0.0, 0.0, 22.5, 17.5);
        assert!(
            points.len() >= 2,
            "expected at least 2 intersections, got {}",
            points.len()
        );
        for p in &points {
            assert_on_circle(*p, &circ);
            assert_on_rect_boundary(*p, 0.0, 0.0, 22.5, 17.5);
        }
    }

    #[test]
    fn test_circle_rect_centered_at_rect_center() {
        // Circle centered at rectangle center; r=25 is between
        // max(hw,hh)=22.5 and corner distance=28.5, so it crosses all 4 edges
        let circ = Circle {
            x: 0.0,
            y: 0.0,
            r: 25.0,
        };
        let points = circle_rect_intersections(&circ, 0.0, 0.0, 22.5, 17.5);
        assert_eq!(
            points.len(),
            8,
            "expected 8 intersections (2 per edge), got {}",
            points.len()
        );
        for p in &points {
            assert_on_circle(*p, &circ);
            assert_on_rect_boundary(*p, 0.0, 0.0, 22.5, 17.5);
        }
    }

    #[test]
    fn test_circle_rect_no_corner_duplicates() {
        // Circle passing through corner of rectangle should not duplicate
        let hw: f64 = 3.0;
        let hh: f64 = 4.0;
        let corner_dist = (hw * hw + hh * hh).sqrt();
        let circ = Circle {
            x: 0.0,
            y: 0.0,
            r: corner_dist,
        };
        let points = circle_rect_intersections(&circ, 0.0, 0.0, hw, hh);
        // Should have exactly 4 corners, no duplicates
        assert_eq!(
            points.len(),
            4,
            "expected 4 corner points, got {}",
            points.len()
        );
    }

    // --- intersect_element_straight with stock tests ---

    #[test]
    fn test_straight_connector_to_stock_from_left() {
        let stock = make_stock_ve(200.0, 100.0, "s", 2);

        // theta = 0 (pointing right toward stock)
        let end = intersect_element_straight(&stock, PI, &not_arrayed);
        // Should hit the left edge of the stock: x = 200 - 22.5 = 177.5
        assert!((end.x - 177.5).abs() < 1e-6, "x={}, expected 177.5", end.x);
        assert!((end.y - 100.0).abs() < 1e-6, "y={}, expected 100.0", end.y);
    }

    #[test]
    fn test_straight_connector_to_stock_from_above() {
        let stock = make_stock_ve(200.0, 200.0, "s", 2);

        // theta = -PI/2 (pointing up, so connector approaches from above)
        let end = intersect_element_straight(&stock, PI / 2.0, &not_arrayed);
        // Should hit the top edge of the stock: y = 200 - 17.5 = 182.5
        assert!((end.x - 200.0).abs() < 1e-6, "x={}, expected 200.0", end.x);
        assert!((end.y - 217.5).abs() < 1e-6, "y={}, expected 217.5", end.y);
    }

    #[test]
    fn test_straight_connector_to_stock_diagonal() {
        let stock = make_stock_ve(200.0, 200.0, "s", 2);
        let theta = deg_to_rad(225.0); // approaching from bottom-right

        let end = intersect_element_straight(&stock, theta, &not_arrayed);
        assert_on_rect_boundary(end, 200.0, 200.0, STOCK_WIDTH / 2.0, STOCK_HEIGHT / 2.0);
    }

    #[test]
    fn test_straight_connector_to_aux_unchanged() {
        let aux = make_aux_ve(200.0, 100.0, "a", 1);
        let theta = 0.0;

        let end = intersect_element_straight(&aux, theta, &not_arrayed);
        // Should use circle formula with AUX_RADIUS
        assert!((end.x - (200.0 + AUX_RADIUS)).abs() < 1e-6);
        assert!((end.y - 100.0).abs() < 1e-6);
    }

    // --- intersect_element_arc with stock tests ---

    #[test]
    fn test_arc_connector_to_stock_on_boundary() {
        let stock = make_stock_ve(200.0, 200.0, "s", 2);
        // Arc circle that passes through the stock center
        let circ = Circle {
            x: 100.0,
            y: 100.0,
            r: (square(200.0 - 100.0) + square(200.0 - 100.0)).sqrt(),
        };

        let end = intersect_element_arc(&stock, &circ, false, &not_arrayed);
        assert_on_rect_boundary(end, 200.0, 200.0, STOCK_WIDTH / 2.0, STOCK_HEIGHT / 2.0);
    }

    #[test]
    fn test_arc_connector_to_stock_on_arc_circle() {
        let stock = make_stock_ve(200.0, 200.0, "s", 2);
        let circ = Circle {
            x: 100.0,
            y: 100.0,
            r: (square(200.0 - 100.0) + square(200.0 - 100.0)).sqrt(),
        };

        let end = intersect_element_arc(&stock, &circ, false, &not_arrayed);
        assert_on_circle(end, &circ);
    }

    #[test]
    fn test_arc_connector_to_aux_unchanged() {
        let aux = make_aux_ve(200.0, 200.0, "a", 1);
        let circ = Circle {
            x: 100.0,
            y: 100.0,
            r: (square(200.0 - 100.0) + square(200.0 - 100.0)).sqrt(),
        };

        let end = intersect_element_arc(&aux, &circ, false, &not_arrayed);
        // Should still be on the circle (existing behavior)
        assert_on_circle(end, &circ);
    }

    #[test]
    fn test_arc_connector_to_stock_inv_flag_matters() {
        let stock = make_stock_ve(200.0, 200.0, "s", 2);
        let circ = Circle {
            x: 150.0,
            y: 50.0,
            r: 180.0,
        };

        let end_no_inv = intersect_element_arc(&stock, &circ, false, &not_arrayed);
        let end_inv = intersect_element_arc(&stock, &circ, true, &not_arrayed);

        // The two points should be different (different sides of the element)
        let dx = end_no_inv.x - end_inv.x;
        let dy = end_no_inv.y - end_inv.y;
        let dist = (dx * dx + dy * dy).sqrt();
        assert!(
            dist > 1.0,
            "inv flag should produce different points, got dist={}",
            dist
        );
    }

    #[test]
    fn test_arc_connector_to_stock_small_radius_inv_matters() {
        let stock = make_stock_ve(200.0, 200.0, "s", 2);
        // Small arc radius where r_approx/circ.r > pi/2 would cause tan()
        // to cross an asymptote (22.5 / 13 ≈ 1.73 > pi/2 ≈ 1.57)
        let circ = Circle {
            x: 190.0,
            y: 190.0,
            r: 13.0,
        };

        let end_no_inv = intersect_element_arc(&stock, &circ, false, &not_arrayed);
        let end_inv = intersect_element_arc(&stock, &circ, true, &not_arrayed);

        assert_on_rect_boundary(
            end_no_inv,
            200.0,
            200.0,
            STOCK_WIDTH / 2.0,
            STOCK_HEIGHT / 2.0,
        );
        assert_on_rect_boundary(end_inv, 200.0, 200.0, STOCK_WIDTH / 2.0, STOCK_HEIGHT / 2.0);
        assert_on_circle(end_no_inv, &circ);
        assert_on_circle(end_inv, &circ);

        let dx = end_no_inv.x - end_inv.x;
        let dy = end_no_inv.y - end_inv.y;
        let dist = (dx * dx + dy * dy).sqrt();
        assert!(
            dist > 1.0,
            "inv flag should select different points even with small arc radius, got dist={}",
            dist
        );
    }
}
