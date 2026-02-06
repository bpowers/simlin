// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::f64::consts::PI;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element;
use crate::diagram::arrowhead::{ArrowheadType, render_arrowhead};
use crate::diagram::common::{Rect, display_name, js_format_number, merge_bounds};
use crate::diagram::constants::*;
use crate::diagram::label::{LabelProps, label_bounds, render_label};

pub fn render_flow(element: &view_element::Flow, sink: &ViewElement, is_arrayed: bool) -> String {
    let arrayed_offset = if is_arrayed { ARRAYED_OFFSET } else { 0.0 };

    let mut pts: Vec<(f64, f64)> = element.points.iter().map(|p| (p.x, p.y)).collect();

    if pts.len() < 2 {
        return String::new();
    }

    // If sink is a Cloud, adjust the last point inward by CloudRadius
    if let ViewElement::Cloud(_) = sink {
        let last_idx = pts.len() - 1;
        let (x, y) = pts[last_idx];
        let (prev_x, prev_y) = pts[last_idx - 1];

        if prev_x < x {
            pts[last_idx].0 = x - CLOUD_RADIUS;
        } else if prev_x > x {
            pts[last_idx].0 = x + CLOUD_RADIUS;
        }
        if prev_y < y {
            pts[last_idx].1 = y - CLOUD_RADIUS;
        } else if prev_y > y {
            pts[last_idx].1 = y + CLOUD_RADIUS;
        }
    }

    let final_adjust = 7.5;
    let mut spath = String::new();
    let mut arrow_theta: f64 = 0.0;

    for j in 0..pts.len() {
        let (mut x, mut y) = pts[j];
        if j == pts.len() - 1 {
            let (prev_x, prev_y) = pts[j - 1];
            let dx = x - prev_x;
            let dy = y - prev_y;
            let mut theta = dy.atan2(dx) * 180.0 / PI;
            if theta < 0.0 {
                theta += 360.0;
            }

            if !(45.0..315.0).contains(&theta) {
                x -= final_adjust;
                arrow_theta = 0.0;
            } else if (45.0..135.0).contains(&theta) {
                y -= final_adjust;
                arrow_theta = 90.0;
            } else if (135.0..225.0).contains(&theta) {
                x += final_adjust;
                arrow_theta = 180.0;
            } else {
                y += final_adjust;
                arrow_theta = 270.0;
            }
        }

        let prefix = if j == 0 { "M" } else { "L" };
        spath.push_str(&format!(
            "{}{},{}",
            prefix,
            js_format_number(x),
            js_format_number(y)
        ));
    }

    let cx = element.x;
    let cy = element.y;
    let r = AUX_RADIUS; // visual valve radius

    let last_pt = pts[pts.len() - 1];

    let label_props = LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
        .with_radii(r + arrayed_offset, r + arrayed_offset);

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-flow\">");

    // Outer path
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"simlin-outer\"></path>",
        spath
    ));

    // No sourceHitArea rect in embedded/export mode

    // Arrowhead
    svg.push_str(&render_arrowhead(
        last_pt.0,
        last_pt.1,
        arrow_theta,
        FLOW_ARROWHEAD_RADIUS,
        ArrowheadType::Flow,
    ));

    // Inner path
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"simlin-inner\"></path>",
        spath
    ));

    // Valve circles
    svg.push_str("<g>");
    if is_arrayed {
        for offset in [arrayed_offset, 0.0, -arrayed_offset] {
            svg.push_str(&format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"></circle>",
                js_format_number(cx + offset),
                js_format_number(cy + offset),
                js_format_number(r)
            ));
        }
    } else {
        svg.push_str(&format!(
            "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"></circle>",
            js_format_number(cx),
            js_format_number(cy),
            js_format_number(r)
        ));
    }
    // TODO(sparklines): render sparkline here when simulation results are available
    svg.push_str("</g>");

    // Label
    svg.push_str(&render_label(&label_props));

    svg.push_str("</g>");
    svg
}

pub fn flow_bounds(element: &view_element::Flow) -> Rect {
    let cx = element.x;
    let cy = element.y;
    // Flow valve bounds use r=6 (FLOW_VALVE_RADIUS), NOT AuxRadius
    let r = FLOW_VALVE_RADIUS;
    let mut bounds = Rect {
        top: cy - r,
        left: cx - r,
        right: cx + r,
        bottom: cy + r,
    };

    // Include label bounds
    let label_props =
        LabelProps::new(cx, cy, element.label_side, display_name(&element.name)).with_radii(r, r);
    let l_bounds = label_bounds(&label_props);
    bounds = merge_bounds(bounds, l_bounds);

    // Include flow path points
    for point in &element.points {
        bounds.left = bounds.left.min(point.x);
        bounds.right = bounds.right.max(point.x);
        bounds.top = bounds.top.min(point.y);
        bounds.bottom = bounds.bottom.max(point.y);
    }

    bounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element::{FlowPoint, LabelSide};

    fn make_flow(
        cx: f64,
        cy: f64,
        name: &str,
        points: Vec<(f64, f64, Option<i32>)>,
    ) -> view_element::Flow {
        view_element::Flow {
            name: name.to_string(),
            uid: 10,
            x: cx,
            y: cy,
            label_side: LabelSide::Bottom,
            points: points
                .into_iter()
                .map(|(x, y, attached)| FlowPoint {
                    x,
                    y,
                    attached_to_uid: attached,
                })
                .collect(),
        }
    }

    fn make_cloud(x: f64, y: f64, uid: i32) -> ViewElement {
        ViewElement::Cloud(view_element::Cloud {
            uid,
            flow_uid: 10,
            x,
            y,
        })
    }

    fn make_stock_ve(x: f64, y: f64, uid: i32) -> ViewElement {
        ViewElement::Stock(view_element::Stock {
            name: "stock".to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
        })
    }

    #[test]
    fn test_render_flow_basic() {
        let flow = make_flow(
            150.0,
            100.0,
            "flow1",
            vec![(100.0, 100.0, Some(1)), (200.0, 100.0, Some(2))],
        );
        let sink = make_cloud(200.0, 100.0, 2);

        let svg = render_flow(&flow, &sink, false);
        assert!(svg.contains("simlin-flow"));
        assert!(svg.contains("simlin-outer"));
        assert!(svg.contains("simlin-inner"));
        assert!(svg.contains("simlin-arrowhead-flow"));
        assert!(svg.contains("<circle"));
    }

    #[test]
    fn test_render_flow_arrayed() {
        let flow = make_flow(
            150.0,
            100.0,
            "flow1",
            vec![(100.0, 100.0, Some(1)), (200.0, 100.0, Some(2))],
        );
        let sink = make_stock_ve(200.0, 100.0, 2);

        let svg = render_flow(&flow, &sink, true);
        // 3 valve circles for arrayed
        let circle_count = svg.matches("<circle").count();
        assert_eq!(circle_count, 3);
    }

    #[test]
    fn test_flow_bounds_uses_valve_radius() {
        let flow = make_flow(
            150.0,
            100.0,
            "f",
            vec![(100.0, 100.0, Some(1)), (200.0, 100.0, Some(2))],
        );
        let bounds = flow_bounds(&flow);
        // Bounds should use FLOW_VALVE_RADIUS (6), not AUX_RADIUS (9)
        assert!(bounds.left <= 100.0);
        assert!(bounds.right >= 200.0);
    }

    #[test]
    fn test_flow_bounds_includes_points() {
        let flow = make_flow(
            150.0,
            100.0,
            "f",
            vec![(50.0, 80.0, Some(1)), (250.0, 120.0, Some(2))],
        );
        let bounds = flow_bounds(&flow);
        assert!(bounds.left <= 50.0);
        assert!(bounds.right >= 250.0);
        assert!(bounds.top <= 80.0);
        assert!(bounds.bottom >= 120.0);
    }

    #[test]
    fn test_render_flow_no_source_hit_area() {
        let flow = make_flow(
            150.0,
            100.0,
            "flow1",
            vec![(100.0, 100.0, Some(1)), (200.0, 100.0, Some(2))],
        );
        let sink = make_stock_ve(200.0, 100.0, 2);

        let svg = render_flow(&flow, &sink, false);
        // Should NOT contain a sourceHitArea rect with cursor:grab
        assert!(!svg.contains("cursor:grab"));
        assert!(!svg.contains("fill=\"transparent\""));
    }

    #[test]
    fn test_render_flow_empty_points() {
        let flow = make_flow(150.0, 100.0, "flow1", vec![]);
        let sink = make_stock_ve(200.0, 100.0, 2);

        let svg = render_flow(&flow, &sink, false);
        assert!(svg.is_empty());
    }
}
