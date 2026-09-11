// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::f64::consts::PI;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element;
use crate::diagram::arrowhead::{
    ArrowheadGeometry, ArrowheadType, arrowhead_geometry, render_arrowhead_geometry,
};
use crate::diagram::common::{
    Circle, Frame, Point, Rect, arrayed_offsets, display_name, js_format_number, merge_bounds,
    svg_circle,
};
use crate::diagram::constants::*;
use crate::diagram::label::{LabelProps, label_bounds, render_label};

/// How far the pipe's last vertex backs off from the arrowhead tip, along the
/// final segment's cardinal direction, so the pipe ends under the arrowhead
/// rather than poking through its point.
const PIPE_FINAL_ADJUST: f64 = 7.5;

/// Everything a drawn flow is made of. `render_flow` prints it and the scene
/// display list reads it, so the two cannot place a flow differently.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(crate) struct FlowGeometry {
    /// The pipe's vertices: the view's points with a cloud sink's retraction
    /// and the arrowhead adjustment applied to the last one.
    pub pipe: Vec<Point>,
    /// The arrowhead, anchored at the retracted last point.
    pub arrowhead: ArrowheadGeometry,
    /// The valve circles, back to front.
    pub valves: Vec<Circle>,
    pub label: LabelProps,
    /// The box the web canvas draws the flow's sparkline in (`Flow.tsx`):
    /// the valve's inner square. The static SVG has no simulation results
    /// and draws no sparkline.
    pub sparkline: Frame,
}

/// The drawn geometry of `element` flowing into `sink`, or `None` for a flow
/// with fewer than two points, which draws nothing.
pub(crate) fn flow_geometry(
    element: &view_element::Flow,
    sink: &ViewElement,
    is_arrayed: bool,
) -> Option<FlowGeometry> {
    if element.points.len() < 2 {
        return None;
    }
    let arrayed_offset = if is_arrayed { ARRAYED_OFFSET } else { 0.0 };

    let mut pts: Vec<Point> = element
        .points
        .iter()
        .map(|p| Point { x: p.x, y: p.y })
        .collect();
    let last_idx = pts.len() - 1;

    // If sink is a Cloud, pull the last point back by CLOUD_RADIUS along the
    // final segment's direction so the arrowhead lands on the cloud's edge.
    // The retraction follows the segment's unit vector (matching the
    // TypeScript renderer): independent per-axis shifts would over-retract a
    // diagonal segment by sqrt(2)*CLOUD_RADIUS. A zero-length final segment
    // is left unchanged.
    if let ViewElement::Cloud(_) = sink {
        let Point { x, y } = pts[last_idx];
        let Point {
            x: prev_x,
            y: prev_y,
        } = pts[last_idx - 1];
        let dx = x - prev_x;
        let dy = y - prev_y;
        let len = (dx * dx + dy * dy).sqrt();
        if len > 0.0 {
            pts[last_idx].x = x - (CLOUD_RADIUS * dx) / len;
            pts[last_idx].y = y - (CLOUD_RADIUS * dy) / len;
        }
    }

    let tip = pts[last_idx];

    // Walk back past coincident points: a degenerate (zero-length) final
    // segment must not read as "pointing right" via atan2(0, 0) == 0
    // (matches the TypeScript renderer).
    let mut theta_opt: Option<f64> = None;
    for i in (0..last_idx).rev() {
        let p = pts[i];
        let dx = tip.x - p.x;
        let dy = tip.y - p.y;
        if dx != 0.0 || dy != 0.0 {
            let mut theta = dy.atan2(dx) * 180.0 / PI;
            if theta < 0.0 {
                theta += 360.0;
            }
            theta_opt = Some(theta);
            break;
        }
    }

    let mut pipe = pts;
    let arrow_theta = match theta_opt {
        Some(theta) if !(45.0..315.0).contains(&theta) => {
            pipe[last_idx].x -= PIPE_FINAL_ADJUST;
            0.0
        }
        Some(theta) if (45.0..135.0).contains(&theta) => {
            pipe[last_idx].y -= PIPE_FINAL_ADJUST;
            90.0
        }
        Some(theta) if (135.0..225.0).contains(&theta) => {
            pipe[last_idx].x += PIPE_FINAL_ADJUST;
            180.0
        }
        Some(_) => {
            pipe[last_idx].y += PIPE_FINAL_ADJUST;
            270.0
        }
        None => 0.0,
    };

    let cx = element.x;
    let cy = element.y;
    let r = AUX_RADIUS; // visual valve radius

    Some(FlowGeometry {
        pipe,
        arrowhead: arrowhead_geometry(tip.x, tip.y, arrow_theta, FLOW_ARROWHEAD_RADIUS),
        valves: arrayed_offsets(is_arrayed)
            .iter()
            .map(|offset| Circle {
                x: cx + offset,
                y: cy + offset,
                r,
            })
            .collect(),
        label: LabelProps::new(cx, cy, element.label_side, display_name(&element.name))
            .with_radii(r + arrayed_offset, r + arrayed_offset),
        sparkline: Frame {
            x: cx - arrayed_offset + 1.0 - r / 2.0,
            y: cy - arrayed_offset + 1.0 - r / 2.0,
            width: r - 2.0,
            height: r - 2.0,
        },
    })
}

pub fn render_flow(element: &view_element::Flow, sink: &ViewElement, is_arrayed: bool) -> String {
    let Some(g) = flow_geometry(element, sink, is_arrayed) else {
        return String::new();
    };

    let mut spath = String::new();
    for (j, p) in g.pipe.iter().enumerate() {
        let prefix = if j == 0 { "M" } else { "L" };
        spath.push_str(&format!(
            "{}{},{}",
            prefix,
            js_format_number(p.x),
            js_format_number(p.y)
        ));
    }

    let mut svg = String::new();
    svg.push_str("<g class=\"simlin-flow\">");

    // Outer path
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"simlin-outer\"></path>",
        spath
    ));

    // No sourceHitArea rect in embedded/export mode

    svg.push_str(&render_arrowhead_geometry(
        &g.arrowhead,
        ArrowheadType::Flow,
    ));

    // Inner path
    svg.push_str(&format!(
        "<path d=\"{}\" class=\"simlin-inner\"></path>",
        spath
    ));

    // Valve circles
    svg.push_str("<g>");
    for valve in &g.valves {
        svg.push_str(&svg_circle(valve));
    }
    svg.push_str("</g>");

    // Label
    svg.push_str(&render_label(&g.label));

    svg.push_str("</g>");
    svg
}

/// The flow's bare *shape* box (the valve plus the pipe polyline points),
/// WITHOUT its label. `flow_bounds` is this box merged with the label; see
/// `diagram::elements::aux_shape_bounds` for why the label-free shape is
/// exposed separately. The flow path points ARE part of the shape (the drawn
/// pipe), so they stay included here.
pub(crate) fn flow_shape_bounds(element: &view_element::Flow) -> Rect {
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

    // Include flow path points (the drawn pipe).
    for point in &element.points {
        bounds.left = bounds.left.min(point.x);
        bounds.right = bounds.right.max(point.x);
        bounds.top = bounds.top.min(point.y);
        bounds.bottom = bounds.bottom.max(point.y);
    }

    bounds
}

pub fn flow_bounds(element: &view_element::Flow) -> Rect {
    let cx = element.x;
    let cy = element.y;
    let r = FLOW_VALVE_RADIUS;
    let shape = flow_shape_bounds(element);

    // Include label bounds
    let label_props =
        LabelProps::new(cx, cy, element.label_side, display_name(&element.name)).with_radii(r, r);
    let l_bounds = label_bounds(&label_props);

    merge_bounds(shape, l_bounds)
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
            compat: None,
            label_compat: None,
        }
    }

    fn make_cloud(x: f64, y: f64, uid: i32) -> ViewElement {
        ViewElement::Cloud(view_element::Cloud {
            uid,
            flow_uid: 10,
            x,
            y,
            compat: None,
        })
    }

    fn make_stock_ve(x: f64, y: f64, uid: i32) -> ViewElement {
        ViewElement::Stock(view_element::Stock {
            name: "stock".to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
            compat: None,
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
        // A POINT-LESS flow, so the valve circle is the whole shape and the
        // radius is what the bounds are made of. With a pipe attached the
        // endpoints dominate the horizontal extent and 6 vs 9 is invisible --
        // which is why this fixture has none. The label-free
        // `flow_shape_bounds` is the right entry point: `flow_bounds` merges
        // the label box, which can also outgrow the circle.
        let flow = make_flow(150.0, 100.0, "f", vec![]);
        let bounds = flow_shape_bounds(&flow);
        assert_eq!(bounds.left, 150.0 - FLOW_VALVE_RADIUS);
        assert_eq!(bounds.right, 150.0 + FLOW_VALVE_RADIUS);
        assert_eq!(bounds.top, 100.0 - FLOW_VALVE_RADIUS);
        assert_eq!(bounds.bottom, 100.0 + FLOW_VALVE_RADIUS);
        assert_ne!(
            FLOW_VALVE_RADIUS, AUX_RADIUS,
            "the two radii must differ, or this test cannot tell them apart"
        );
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
        assert!(flow_geometry(&flow, &sink, false).is_none());
    }
}
