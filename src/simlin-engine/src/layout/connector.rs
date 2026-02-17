// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::view_element::FlowPoint;

use super::graph::Position;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowOrientation {
    Horizontal,
    Vertical,
}

/// Determine whether a flow runs primarily horizontally or vertically
/// based on its first and last points.
pub fn compute_flow_orientation(points: &[FlowPoint]) -> FlowOrientation {
    if points.len() < 2 {
        return FlowOrientation::Horizontal;
    }

    let first = &points[0];
    let last = &points[points.len() - 1];
    let dx = (last.x - first.x).abs();
    let dy = (last.y - first.y).abs();

    if dx >= dy {
        FlowOrientation::Horizontal
    } else {
        FlowOrientation::Vertical
    }
}

/// Normalize an angle in degrees to the half-open interval (-180, 180].
pub fn normalize_angle(angle: f64) -> f64 {
    let mut a = angle % 360.0;
    if a > 180.0 {
        a -= 360.0;
    } else if a <= -180.0 {
        a += 360.0;
    }
    a
}

/// Calculate the arc angle for a structural stock-to-flow connector.
///
/// The returned angle (in degrees) determines the curvature of the arc
/// connecting a stock to its adjacent flow valve.
pub fn calc_stock_flow_arc_angle(stock_pos: Position, flow_pos: Position) -> f64 {
    let dx = flow_pos.x - stock_pos.x;
    let dy = flow_pos.y - stock_pos.y;

    if dx == 0.0 && dy == 0.0 {
        return -45.0;
    }

    if dx.abs() >= dy.abs() {
        if dx >= 0.0 { -45.0 } else { -135.0 }
    } else {
        let base_angle = dy.atan2(dx).to_degrees();
        let adjustment = if dx >= 0.0 { -45.0 } else { 45.0 };
        normalize_angle(base_angle + adjustment)
    }
}

/// Calculate the arc angle for a feedback loop connector.
///
/// Uses the loop center to determine which way the connector should curve,
/// with `curvature_factor` controlling the strength. The returned angle
/// (in degrees) is the takeoff direction from the source node, which
/// determines the arc's visual curvature.
pub fn calculate_loop_arc_angle(
    from_pos: Position,
    to_pos: Position,
    loop_center: Position,
    curvature_factor: f64,
) -> f64 {
    let base_angle = (to_pos.y - from_pos.y)
        .atan2(to_pos.x - from_pos.x)
        .to_degrees();

    let angle_to_center = (loop_center.y - from_pos.y)
        .atan2(loop_center.x - from_pos.x)
        .to_degrees();

    let angle_diff = normalize_angle(angle_to_center - base_angle);

    let mut takeoff = base_angle - (angle_diff * curvature_factor);

    // Apply a minimum curve when the loop center is nearly collinear
    // with the connector direction.
    if angle_diff.abs() < 15.0 {
        takeoff = base_angle - 20.0;
    }

    normalize_angle(takeoff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element::FlowPoint;

    #[test]
    fn test_compute_flow_orientation_horizontal() {
        let points = vec![
            FlowPoint {
                x: 0.0,
                y: 0.0,
                attached_to_uid: None,
            },
            FlowPoint {
                x: 100.0,
                y: 10.0,
                attached_to_uid: None,
            },
        ];
        assert_eq!(
            compute_flow_orientation(&points),
            FlowOrientation::Horizontal
        );
    }

    #[test]
    fn test_compute_flow_orientation_vertical() {
        let points = vec![
            FlowPoint {
                x: 0.0,
                y: 0.0,
                attached_to_uid: None,
            },
            FlowPoint {
                x: 10.0,
                y: 100.0,
                attached_to_uid: None,
            },
        ];
        assert_eq!(compute_flow_orientation(&points), FlowOrientation::Vertical);
    }

    #[test]
    fn test_compute_flow_orientation_insufficient_points() {
        assert_eq!(compute_flow_orientation(&[]), FlowOrientation::Horizontal);
        let single = vec![FlowPoint {
            x: 5.0,
            y: 5.0,
            attached_to_uid: None,
        }];
        assert_eq!(
            compute_flow_orientation(&single),
            FlowOrientation::Horizontal
        );
    }

    #[test]
    fn test_normalize_angle_in_range() {
        assert!((normalize_angle(45.0) - 45.0).abs() < 1e-10);
        assert!((normalize_angle(-90.0) - (-90.0)).abs() < 1e-10);
        assert!((normalize_angle(180.0) - 180.0).abs() < 1e-10);
    }

    #[test]
    fn test_normalize_angle_negative() {
        // -270 should normalize to 90
        assert!((normalize_angle(-270.0) - 90.0).abs() < 1e-10);
        // -180 wraps to +180 since the interval is (-180, 180]
        // -180 % 360 = -180, then -180 <= -180, so +360 = 180
        assert!((normalize_angle(-180.0) - 180.0).abs() < 1e-10);
    }

    #[test]
    fn test_normalize_angle_large() {
        // 540 = 360 + 180 => 180
        assert!((normalize_angle(540.0) - 180.0).abs() < 1e-10);
        // 270 => -90
        assert!((normalize_angle(270.0) - (-90.0)).abs() < 1e-10);
        // 720 => 0
        assert!(normalize_angle(720.0).abs() < 1e-10);
    }

    #[test]
    fn test_calc_stock_flow_arc_angle_right() {
        let stock = Position::new(0.0, 0.0);
        let flow = Position::new(100.0, 0.0);
        assert!((calc_stock_flow_arc_angle(stock, flow) - (-45.0)).abs() < 1e-10);
    }

    #[test]
    fn test_calc_stock_flow_arc_angle_left() {
        let stock = Position::new(100.0, 0.0);
        let flow = Position::new(0.0, 0.0);
        assert!((calc_stock_flow_arc_angle(stock, flow) - (-135.0)).abs() < 1e-10);
    }

    #[test]
    fn test_calc_stock_flow_arc_angle_coincident() {
        let pos = Position::new(50.0, 50.0);
        assert!((calc_stock_flow_arc_angle(pos, pos) - (-45.0)).abs() < 1e-10);
    }

    #[test]
    fn test_calc_stock_flow_arc_angle_vertical() {
        // Flow directly below stock: dy > dx, dx = 0 (>= 0), so adjustment = -45
        // base_angle = atan2(100, 0) = 90 degrees, result = 90 - 45 = 45
        let stock = Position::new(0.0, 0.0);
        let flow = Position::new(0.0, 100.0);
        assert!((calc_stock_flow_arc_angle(stock, flow) - 45.0).abs() < 1e-10);
    }

    #[test]
    fn test_calculate_loop_arc_angle_basic() {
        let from = Position::new(0.0, 0.0);
        let to = Position::new(100.0, 0.0);
        let center = Position::new(50.0, 50.0);

        let angle = calculate_loop_arc_angle(from, to, center, 0.5);
        // base_angle = 0, angle_to_center = atan2(50,50) = 45 degrees
        // angle_diff = normalize(45 - 0) = 45
        // takeoff = 0 - (45 * 0.5) = -22.5
        assert!((angle - (-22.5)).abs() < 1e-10);
    }

    #[test]
    fn test_calculate_loop_arc_angle_collinear() {
        // Loop center nearly along the connector direction triggers minimum curve
        let from = Position::new(0.0, 0.0);
        let to = Position::new(100.0, 0.0);
        let center = Position::new(200.0, 1.0);

        let angle = calculate_loop_arc_angle(from, to, center, 0.5);
        // angle_to_center ~= 0.28 degrees, angle_diff ~= 0.28, |0.28| < 15
        // so takeoff = base_angle - 20 = 0 - 20 = -20
        assert!((angle - (-20.0)).abs() < 1.0);
    }

    #[test]
    fn test_calculate_loop_arc_angle_curvature_strength() {
        let from = Position::new(0.0, 0.0);
        let to = Position::new(100.0, 0.0);
        let center = Position::new(50.0, 50.0);

        // Higher curvature factor should produce a more deflected takeoff
        let weak = calculate_loop_arc_angle(from, to, center, 0.2);
        let strong = calculate_loop_arc_angle(from, to, center, 0.8);
        assert!(strong.abs() > weak.abs());
    }
}
