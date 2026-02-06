// Copyright 2024 The Simlin Authors. All rights reserved.
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

const fn element_radius(element: &ViewElement) -> f64 {
    match element {
        ViewElement::Module(_) => 25.0,
        ViewElement::Stock(_) => 15.0,
        _ => AUX_RADIUS,
    }
}

fn is_element_zero_radius(_element: &ViewElement) -> bool {
    false
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

    if is_element_zero_radius(element) {
        return (cx, cy);
    }

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
    let mut r = element_radius(element);
    if is_element_zero_radius(element) {
        r = 0.0;
    }

    let (cx, cy) = get_visual_center(element, is_arrayed_fn);
    Point {
        x: cx + r * theta.cos(),
        y: cy + r * theta.sin(),
    }
}

fn intersect_element_arc(
    element: &ViewElement,
    circ: &Circle,
    inv: bool,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> Point {
    let mut r = element_radius(element);
    if is_element_zero_radius(element) {
        r = 0.0;
    }

    let (cx, cy) = get_visual_center(element, is_arrayed_fn);
    let off_theta = (r / circ.r).atan();
    let element_center_theta = (cy - circ.y).atan2(cx - circ.x);

    Point {
        x: circ.x
            + circ.r * (element_center_theta + if inv { 1.0 } else { -1.0 } * off_theta).cos(),
        y: circ.y
            + circ.r * (element_center_theta + if inv { 1.0 } else { -1.0 } * off_theta).sin(),
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
}
