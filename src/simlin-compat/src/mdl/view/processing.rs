// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! View processing logic for Vensim sketch conversion.
//!
//! This module handles coordinate transformation, flow point computation,
//! angle calculation, and ghost/primary variable tracking.

use std::collections::HashMap;
use std::f64::consts::PI;

use super::types::{VensimElement, VensimView};

/// Calculate angle from three points (AngleFromPoints from xmutil).
///
/// Given a start point, control point, and end point, computes the tangent
/// angle at the start for an arc passing through all three points.
///
/// - If control_point is (0, 0), returns the straight-line angle.
/// - Returns angle in XMILE format [0, 360).
pub fn angle_from_points(
    start_x: f64,
    start_y: f64,
    point_x: f64,
    point_y: f64,
    end_x: f64,
    end_y: f64,
) -> f64 {
    // Calculate straight line angle as fallback
    let theta_straight = if end_x > start_x {
        -((end_y - start_y) / (end_x - start_x)).atan() * 180.0 / PI
    } else if end_x < start_x {
        180.0 - ((start_y - end_y) / (start_x - end_x)).atan() * 180.0 / PI
    } else if end_y > start_y {
        270.0
    } else {
        90.0
    };

    // Straight line connector: control point at (0,0) is the sentinel
    if point_x == 0.0 && point_y == 0.0 {
        return normalize_angle(theta_straight);
    }

    // Find circle center from perpendicular bisectors of:
    // 1. start-end segment
    // 2. point-end segment

    // Line 1: perpendicular bisector of start-end
    let line1_x = (start_x + end_x) / 2.0;
    let line1_y = (start_y + end_y) / 2.0;
    let (slope1_x, slope1_y) = if start_x == end_x {
        (1.0, 0.0)
    } else if start_y == end_y {
        (0.0, 1.0)
    } else {
        (end_y - start_y, start_x - end_x) // perpendicular: flip and negate
    };

    // Line 2: perpendicular bisector of point-end
    let line2_x = (point_x + end_x) / 2.0;
    let line2_y = (point_y + end_y) / 2.0;
    let (slope2_x, slope2_y) = if point_x == end_x {
        (1.0, 0.0)
    } else if point_y == end_y {
        (0.0, 1.0)
    } else {
        (end_y - point_y, point_x - end_x)
    };

    // Solve for intersection of the two perpendicular bisector lines
    // line1_x + delta1 * slope1_x = line2_x + delta2 * slope2_x
    // line1_y + delta1 * slope1_y = line2_y + delta2 * slope2_y
    let (delta1, _delta2) = if slope1_y.abs() < 1e-8 {
        if slope2_y.abs() < 1e-8 || slope1_x.abs() < 1e-8 {
            return normalize_angle(theta_straight);
        }
        let d2 = (line1_y - line2_y) / slope2_y;
        let d1 = (line2_x + d2 * slope2_x - line1_x) / slope1_x;
        (d1, d2)
    } else if slope1_x.abs() < 1e-8 {
        if slope2_x.abs() < 1e-8 {
            return normalize_angle(theta_straight);
        }
        let d2 = (line1_x - line2_x) / slope2_x;
        let d1 = (line2_y + d2 * slope2_y - line1_y) / slope1_y;
        (d1, d2)
    } else if slope2_y.abs() < 1e-8 {
        if slope2_x.abs() < 1e-8 {
            return normalize_angle(theta_straight);
        }
        let d1 = (line2_y - line1_y) / slope1_y;
        let d2 = (line1_x + d1 * slope1_x - line2_x) / slope2_x;
        (d1, d2)
    } else {
        let denom = slope2_x - slope1_x * slope2_y / slope1_y;
        if denom.abs() < 1e-8 {
            return normalize_angle(theta_straight);
        }
        let d2 = (line1_x + (line2_y - line1_y) / slope1_y * slope1_x - line2_x) / denom;
        let d1 = (line2_y + d2 * slope2_y - line1_y) / slope1_y;
        (d1, d2)
    };

    let center_x = line1_x + delta1 * slope1_x;
    let center_y = line1_y + delta1 * slope1_y;

    // Handle degenerate cases
    if (center_y - start_y).abs() < 1e-6 {
        return if point_y > start_y { 90.0 } else { 270.0 };
    }
    if (center_x - start_x).abs() < 1e-6 {
        return if point_x > start_x { 0.0 } else { 180.0 };
    }

    // Calculate angle using atan2
    let mut theta = (-(start_y - center_y)).atan2(start_x - center_x) * 180.0 / PI;

    // Adjust by +/-90 to ensure arc passes through control point
    let direct = (-(point_y - start_y)).atan2(point_x - start_x) * 180.0 / PI;

    let mut diff1 = direct - (theta - 90.0);
    while diff1 < 0.0 {
        diff1 += 360.0;
    }
    while diff1 > 180.0 {
        diff1 -= 360.0;
    }

    let mut diff2 = direct - (theta + 90.0);
    while diff2 < 0.0 {
        diff2 += 360.0;
    }
    while diff2 > 180.0 {
        diff2 -= 360.0;
    }

    if diff1.abs() < diff2.abs() {
        theta -= 90.0;
    } else {
        theta += 90.0;
    }

    normalize_angle(theta)
}

/// Normalize angle to [0, 360) range.
fn normalize_angle(mut angle: f64) -> f64 {
    while angle < 0.0 {
        angle += 360.0;
    }
    while angle >= 360.0 {
        angle -= 360.0;
    }
    angle
}

/// Convert XMILE angle [0, 360) to canvas angle [-180, 180].
///
/// XMILE uses counter-clockwise with Y-up; canvas uses Y-down.
pub fn xmile_angle_to_canvas(in_degrees: f64) -> f64 {
    let out_degrees = (360.0 - in_degrees) % 360.0;
    if out_degrees > 180.0 {
        out_degrees - 360.0
    } else {
        out_degrees
    }
}

/// Convert canvas angle [-180, 180] to XMILE angle [0, 360).
pub fn canvas_angle_to_xmile(in_degrees: f64) -> f64 {
    let out_degrees = if in_degrees < 0.0 {
        in_degrees + 360.0
    } else {
        in_degrees
    };
    (360.0 - out_degrees) % 360.0
}

/// Transform view coordinates with scaling and offset.
///
/// Finds minimum x/y, then transforms all elements:
/// - new_x = old_x * x_ratio + offset_x
/// - new_y = old_y * y_ratio + offset_y
///
/// Returns the next uid_offset (current offset + element count).
pub fn transform_view_coordinates(
    view: &mut VensimView,
    start_x: i32,
    start_y: i32,
    x_ratio: f64,
    y_ratio: f64,
    uid_offset: i32,
) -> i32 {
    view.uid_offset = uid_offset;

    if view.elements.is_empty() {
        return uid_offset;
    }

    // Find minimum coordinates
    let min_x = view.min_x().unwrap_or(0);
    let min_y = view.min_y().unwrap_or(0);

    // Calculate offsets to shift origin to start position
    let off_x = start_x as f64 - (min_x as f64 * x_ratio);
    let off_y = start_y as f64 - (min_y as f64 * y_ratio);

    // Transform all elements
    for elem in view.elements.iter_mut().flatten() {
        // Connectors have special handling: (0,0) control point is a sentinel
        // for straight lines and must not be scaled (xmutil VensimView.cpp:154-160)
        if let VensimElement::Connector(conn) = elem {
            if conn.control_point.0 != 0 || conn.control_point.1 != 0 {
                let new_x = (conn.control_point.0 as f64 * x_ratio + off_x).round() as i32;
                let new_y = (conn.control_point.1 as f64 * y_ratio + off_y).round() as i32;
                conn.control_point = (new_x, new_y);
            }
            continue;
        }

        let new_x = (elem.x() as f64 * x_ratio + off_x).round() as i32;
        let new_y = (elem.y() as f64 * y_ratio + off_y).round() as i32;
        let new_w = (elem.width() as f64 * x_ratio).round() as i32;
        let new_h = (elem.height() as f64 * y_ratio).round() as i32;

        elem.set_x(new_x);
        elem.set_y(new_y);
        elem.set_width(new_w);
        elem.set_height(new_h);
    }

    uid_offset + view.elements.len() as i32
}

/// Compose multiple views by stacking them vertically.
///
/// Starting at (100, 100), each view is offset by its height + 80 pixels.
/// Returns the uid_offset for each view.
pub fn compose_views(views: &mut [VensimView]) -> Vec<i32> {
    let mut offsets = Vec::with_capacity(views.len());
    let x = 100;
    let mut y = 100;
    let mut uid_off = 0;

    for view in views.iter_mut() {
        // Transform this view's coordinates
        uid_off = transform_view_coordinates(view, x, y + 20, 1.0, 1.0, uid_off);
        offsets.push(view.uid_offset);

        // Get view height and advance y
        let height = view.max_y(y + 80) - y;
        y += height + 80;
    }

    offsets
}

/// Result of computing flow endpoints.
#[derive(Debug)]
pub struct FlowEndpoints {
    /// "From" endpoint coordinates
    pub from_x: i32,
    pub from_y: i32,
    /// UID of the stock/cloud at the "from" endpoint (if any)
    pub from_uid: Option<i32>,
    /// "To" endpoint coordinates
    pub to_x: i32,
    pub to_y: i32,
    /// UID of the stock/cloud at the "to" endpoint (if any)
    pub to_uid: Option<i32>,
}

/// Compute flow points for a flow variable.
///
/// This implements the XMILEGenerator.cpp:987-1072 algorithm for determining
/// flow pipe endpoints based on connected stocks and clouds.
///
/// Key behaviors matching xmutil:
/// 1. Use connector control points for endpoint positions, NOT element centers
/// 2. Apply anchor snapping: if vertical (xpt[0] == xpt[1]), snap Y to anchors;
///    otherwise snap X to anchors
/// 3. Output points in order: from endpoint first, then to endpoint
///
/// `flow_name` is the canonical name of the flow variable.
/// The function looks up connected stocks and checks if this flow appears
/// in their inflows (making that stock the "to" endpoint) or outflows
/// (making that stock the "from" endpoint).
#[allow(clippy::too_many_arguments)]
pub fn compute_flow_points(
    valve_uid: i32,
    flow_x: i32,
    flow_y: i32,
    view: &VensimView,
    flow_name: &str,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
    uid_offset: i32,
) -> FlowEndpoints {
    use crate::mdl::builtins::to_lower_space;

    // Collect endpoint information (xmutil: xpt, ypt, xanchor, yanchor)
    struct EndpointInfo {
        uid: i32,
        // Control point from connector (xpt, ypt in xmutil)
        ctrl_x: i32,
        ctrl_y: i32,
        // Target element center (xanchor, yanchor in xmutil)
        anchor_x: i32,
        anchor_y: i32,
    }

    let mut endpoints: Vec<EndpointInfo> = Vec::new();
    let mut to_index: Option<usize> = None;

    // Find connectors from valve to stocks/clouds
    for elem in view.iter() {
        if let VensimElement::Connector(conn) = elem
            && conn.from_uid == valve_uid
            && let Some(target) = view.get(conn.to_uid)
        {
            // Determine if this is a valid endpoint and whether it's an inflow/outflow
            // xmutil only sets to_index for Variable (stock) endpoints, not comments
            let (is_valid, is_inflow) = match target {
                VensimElement::Variable(v) => {
                    let target_canonical = to_lower_space(&v.name);
                    // Look up the stock's SymbolInfo
                    if let Some(stock_info) = symbols.get(&target_canonical) {
                        // Check if this flow is in the stock's inflows or outflows
                        let is_inflow = stock_info.inflows.contains(&flow_name.to_string());
                        let is_outflow = stock_info.outflows.contains(&flow_name.to_string());
                        if is_inflow || is_outflow {
                            (true, Some(is_inflow))
                        } else {
                            (false, None)
                        }
                    } else {
                        (false, None)
                    }
                }
                VensimElement::Comment(_) => {
                    // Clouds are valid endpoints but DON'T affect to_index (xmutil behavior)
                    (true, None)
                }
                _ => (false, None),
            };

            if is_valid {
                // xmutil uses connector control point for xpt/ypt
                // and target element center for xanchor/yanchor
                let count = endpoints.len();

                // Only set to_index for Variable (stock) endpoints, not comments
                // xmutil: inflows set toind = count, outflows set toind = count ? 0 : 1
                // Inflows take precedence over outflows
                if let Some(is_inflow_endpoint) = is_inflow {
                    if is_inflow_endpoint {
                        // Inflow: this endpoint is the "to" - inflows always override
                        to_index = Some(count);
                    } else if to_index.is_none() {
                        // Outflow and no inflow found yet: the OTHER endpoint is the "to"
                        to_index = Some(if count > 0 { 0 } else { 1 });
                    }
                }

                endpoints.push(EndpointInfo {
                    uid: conn.to_uid,
                    ctrl_x: conn.control_point.0,
                    ctrl_y: conn.control_point.1,
                    anchor_x: target.x(),
                    anchor_y: target.y(),
                });

                if endpoints.len() >= 2 {
                    break;
                }
            }
        }
    }

    // Fall back to default if not enough endpoints found (xmutil: count < 2 || toind < 0)
    if endpoints.len() < 2 || to_index.is_none() {
        // xmutil uses flow element coordinates for defaults
        return FlowEndpoints {
            from_x: flow_x - 150,
            from_y: flow_y,
            from_uid: None,
            to_x: flow_x + 25,
            to_y: flow_y,
            to_uid: None,
        };
    }

    // Apply anchor snapping (xmutil XMILEGenerator.cpp:1052-1061)
    let mut xpt = [endpoints[0].ctrl_x, endpoints[1].ctrl_x];
    let mut ypt = [endpoints[0].ctrl_y, endpoints[1].ctrl_y];
    let xanchor = [endpoints[0].anchor_x, endpoints[1].anchor_x];
    let yanchor = [endpoints[0].anchor_y, endpoints[1].anchor_y];

    if xpt[0] == xpt[1] {
        // Vertical flow - snap Y coordinates to anchors
        ypt[0] = yanchor[0];
        ypt[1] = yanchor[1];
    } else {
        // Horizontal flow - snap X coordinates to anchors
        xpt[0] = xanchor[0];
        xpt[1] = xanchor[1];
    }

    // Determine indices for from/to
    // xmutil outputs [1-toind] first (from), then [toind] (to)
    let to_idx = to_index.unwrap();
    let from_idx = 1 - to_idx;

    FlowEndpoints {
        from_x: xpt[from_idx],
        from_y: ypt[from_idx],
        from_uid: Some(uid_offset + endpoints[from_idx].uid),
        to_x: xpt[to_idx],
        to_y: ypt[to_idx],
        to_uid: Some(uid_offset + endpoints[to_idx].uid),
    }
}

/// Determine if a comment element is used as a cloud (flow endpoint).
///
/// Returns the flow_uid if this comment is a flow endpoint, None otherwise.
pub fn is_cloud_endpoint(comment_uid: i32, view: &VensimView) -> Option<i32> {
    // Look for connectors that connect to this comment
    for elem in view.iter() {
        if let VensimElement::Connector(conn) = elem
            && conn.to_uid == comment_uid
        {
            // Check if the source is a valve
            if let Some(VensimElement::Valve(_)) = view.get(conn.from_uid) {
                // The flow is at uid + 1 after the valve
                return Some(conn.from_uid + 1);
            }
        }
    }
    None
}

/// Track which view contains the primary definition of each variable.
pub type PrimaryMap = HashMap<String, (usize, i32)>; // name -> (view_idx, uid)

/// Set of effective ghosts: (view_idx, uid) pairs that should be treated as ghosts.
pub type EffectiveGhosts = std::collections::HashSet<(usize, i32)>;

/// Associate variables with views, determining ghost vs primary status.
///
/// Returns:
/// - A map of canonical variable names to (view_index, uid) for primary definitions
/// - A set of (view_index, uid) pairs that are "effective ghosts" (duplicates even if
///   not marked as ghost in the MDL file)
///
/// This implements xmutil's two-pass algorithm:
/// 1. First pass: Find primaries, mark duplicates as effective ghosts
/// 2. Second pass: Promote first occurrence to primary if variable has no primary
pub fn associate_variables(views: &[VensimView]) -> (PrimaryMap, EffectiveGhosts) {
    use crate::mdl::builtins::to_lower_space;

    let mut primary_map = HashMap::new();
    let mut effective_ghosts: EffectiveGhosts = std::collections::HashSet::new();
    let mut assigned: std::collections::HashSet<String> = std::collections::HashSet::new();

    // First pass: find primaries, mark duplicates as effective ghosts
    // This mirrors xmutil's VensimVariableElement constructor: if variable already
    // has a view, it's forced to be a ghost.
    for (view_idx, view) in views.iter().enumerate() {
        for (uid, elem) in view.iter_with_uids() {
            if let VensimElement::Variable(var) = elem {
                let canonical = to_lower_space(&var.name);

                if assigned.contains(&canonical) {
                    // xmutil: if variable already has view, it's a ghost
                    // Only add to effective_ghosts if not already marked as ghost in MDL
                    if !var.is_ghost {
                        effective_ghosts.insert((view_idx, uid));
                    }
                } else if !var.is_ghost {
                    // First non-ghost appearance becomes primary
                    primary_map.insert(canonical.clone(), (view_idx, uid));
                    assigned.insert(canonical);
                }
            }
        }
    }

    // Second pass: promote first occurrence to primary if variable has no primary
    // This mirrors xmutil's CheckGhostOwners
    for (view_idx, view) in views.iter().enumerate() {
        for (uid, elem) in view.iter_with_uids() {
            if let VensimElement::Variable(var) = elem {
                let canonical = to_lower_space(&var.name);
                if let std::collections::hash_map::Entry::Vacant(e) = primary_map.entry(canonical) {
                    // Promote this (first encountered) to primary
                    e.insert((view_idx, uid));
                    // Remove from effective ghosts if it was there
                    effective_ghosts.remove(&(view_idx, uid));
                    // Don't break - continue to promote all missing primaries
                }
            }
        }
    }

    (primary_map, effective_ghosts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_angle() {
        assert_eq!(normalize_angle(0.0), 0.0);
        assert_eq!(normalize_angle(90.0), 90.0);
        assert_eq!(normalize_angle(360.0), 0.0);
        assert_eq!(normalize_angle(-90.0), 270.0);
        assert_eq!(normalize_angle(450.0), 90.0);
    }

    #[test]
    fn test_angle_straight_line() {
        // Control point at (0,0) -> straight line
        let angle = angle_from_points(0.0, 0.0, 0.0, 0.0, 100.0, 0.0);
        assert!((angle - 0.0).abs() < 0.01 || (angle - 360.0).abs() < 0.01);

        // Straight line going up
        let angle = angle_from_points(0.0, 0.0, 0.0, 0.0, 0.0, -100.0);
        assert!((angle - 90.0).abs() < 0.01);

        // Straight line going down
        let angle = angle_from_points(0.0, 0.0, 0.0, 0.0, 0.0, 100.0);
        assert!((angle - 270.0).abs() < 0.01);
    }

    #[test]
    fn test_angle_with_control_point() {
        // Arc with control point
        let angle = angle_from_points(0.0, 0.0, 50.0, 50.0, 100.0, 0.0);
        // Should be some angle that's not the straight line
        assert!((0.0..360.0).contains(&angle));
    }

    #[test]
    fn test_xmile_angle_to_canvas() {
        assert!((xmile_angle_to_canvas(0.0) - 0.0).abs() < 0.01);
        assert!((xmile_angle_to_canvas(90.0) - (-90.0)).abs() < 0.01);
        assert!((xmile_angle_to_canvas(180.0) - 180.0).abs() < 0.01);
        assert!((xmile_angle_to_canvas(270.0) - 90.0).abs() < 0.01);
    }

    #[test]
    fn test_canvas_angle_to_xmile() {
        assert!((canvas_angle_to_xmile(0.0) - 0.0).abs() < 0.01);
        assert!((canvas_angle_to_xmile(-90.0) - 90.0).abs() < 0.01);
        assert!((canvas_angle_to_xmile(180.0) - 180.0).abs() < 0.01);
        assert!((canvas_angle_to_xmile(90.0) - 270.0).abs() < 0.01);
    }

    #[test]
    fn test_transform_view_coordinates() {
        use super::super::types::{VensimVariable, ViewHeader, ViewVersion};

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "A".to_string(),
                x: 50,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        let next_offset = transform_view_coordinates(&mut view, 200, 300, 1.0, 1.0, 0);

        // Check that coordinates were transformed
        if let Some(VensimElement::Variable(v)) = view.get(1) {
            // Original min was (50, 100), offset to start at (200, 300)
            assert_eq!(v.x, 200); // 50 - 50 + 200 = 200
            assert_eq!(v.y, 300); // 100 - 100 + 300 = 300
        }

        assert!(next_offset > 0);
    }

    #[test]
    fn test_associate_variables() {
        use super::super::types::{VensimVariable, ViewHeader, ViewVersion};

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        // Primary variable
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Test Var".to_string(),
                x: 100,
                y: 200,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Ghost of the same variable
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Test Var".to_string(),
                x: 300,
                y: 400,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: true,
            }),
        );

        let (primary_map, effective_ghosts) = associate_variables(&[view]);

        // to_lower_space canonicalizes to "test var" (underscores to spaces, lowercase)
        assert_eq!(primary_map.get("test var"), Some(&(0, 1)));
        assert!(!primary_map.contains_key("test var ghost"));
        // The ghost at uid 2 is a true ghost (is_ghost=true), not an effective ghost
        assert!(!effective_ghosts.contains(&(0, 2)));
    }

    // Helper to create test SymbolInfo
    fn make_symbol_info<'a>(
        var_type: crate::mdl::convert::VariableType,
        inflows: Vec<String>,
        outflows: Vec<String>,
    ) -> crate::mdl::convert::SymbolInfo<'a> {
        crate::mdl::convert::SymbolInfo {
            var_type,
            equations: vec![],
            inflows,
            outflows,
            unwanted: false,
            alternate_name: None,
        }
    }

    #[test]
    fn test_compute_flow_points_no_connectors() {
        use super::super::types::{VensimValve, VensimVariable, ViewHeader, ViewVersion};

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        // Valve at uid 1
        view.insert(
            1,
            VensimElement::Valve(VensimValve {
                uid: 1,
                name: "444".to_string(),
                x: 100,
                y: 100,
                width: 6,
                height: 8,
                attached: true,
            }),
        );

        // Flow at uid 2
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Flow Rate".to_string(),
                x: 100,
                y: 120,
                width: 40,
                height: 20,
                attached: true,
                is_ghost: false,
            }),
        );

        let symbols = std::collections::HashMap::new();
        let endpoints = compute_flow_points(1, 100, 100, &view, "flow rate", &symbols, 0);

        // Fallback: no connectors found, so use default points
        assert_eq!(endpoints.from_x, 100 - 150);
        assert_eq!(endpoints.from_y, 100);
        assert!(endpoints.from_uid.is_none());
        assert_eq!(endpoints.to_x, 100 + 25);
        assert_eq!(endpoints.to_y, 100);
        assert!(endpoints.to_uid.is_none());
    }

    #[test]
    fn test_compute_flow_points_single_endpoint_uses_defaults() {
        // xmutil requires 2 endpoints with valid to_index to use collected endpoints.
        // With only 1 endpoint, it falls back to defaults (XMILEGenerator.cpp:1047-1051)
        use super::super::types::{
            VensimConnector, VensimValve, VensimVariable, ViewHeader, ViewVersion,
        };
        use crate::mdl::convert::VariableType;

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        // Stock at uid 1
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Stock A".to_string(),
                x: 200,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Valve at uid 2
        view.insert(
            2,
            VensimElement::Valve(VensimValve {
                uid: 2,
                name: "444".to_string(),
                x: 100,
                y: 100,
                width: 6,
                height: 8,
                attached: true,
            }),
        );

        // Flow at uid 3
        view.insert(
            3,
            VensimElement::Variable(VensimVariable {
                uid: 3,
                name: "Flow Rate".to_string(),
                x: 100,
                y: 120,
                width: 40,
                height: 20,
                attached: true,
                is_ghost: false,
            }),
        );

        // Connector from valve (2) to stock (1)
        view.insert(
            4,
            VensimElement::Connector(VensimConnector {
                uid: 4,
                from_uid: 2,
                to_uid: 1,
                polarity: None,
                control_point: (150, 100),
            }),
        );

        // Stock A has "flow rate" as an inflow
        let mut symbols = std::collections::HashMap::new();
        symbols.insert(
            "stock a".to_string(),
            make_symbol_info(VariableType::Stock, vec!["flow rate".to_string()], vec![]),
        );

        let endpoints = compute_flow_points(2, 100, 100, &view, "flow rate", &symbols, 0);

        // Single endpoint: xmutil uses defaults for BOTH endpoints when count < 2
        assert_eq!(endpoints.from_x, 100 - 150); // flow_x - 150
        assert_eq!(endpoints.from_y, 100); // flow_y
        assert!(endpoints.from_uid.is_none());
        assert_eq!(endpoints.to_x, 100 + 25); // flow_x + 25
        assert_eq!(endpoints.to_y, 100); // flow_y
        assert!(endpoints.to_uid.is_none());
    }

    // Note: test_compute_flow_points_single_outflow_endpoint removed - xmutil behavior
    // requires 2 endpoints; single endpoint case covered by test_compute_flow_points_single_endpoint_uses_defaults

    #[test]
    fn test_compute_flow_points_two_endpoints_horizontal() {
        // Test horizontal flow with anchor snapping
        // xmutil: if xpt[0] != xpt[1] (different x), snap x coords to anchors
        use super::super::types::{
            VensimConnector, VensimValve, VensimVariable, ViewHeader, ViewVersion,
        };
        use crate::mdl::convert::VariableType;

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        // Stock A (source) at uid 1, x=50
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Stock A".to_string(),
                x: 50,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Stock B (destination) at uid 2, x=250
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Stock B".to_string(),
                x: 250,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Valve at uid 3, x=150 (between the stocks)
        view.insert(
            3,
            VensimElement::Valve(VensimValve {
                uid: 3,
                name: "444".to_string(),
                x: 150,
                y: 100,
                width: 6,
                height: 8,
                attached: true,
            }),
        );

        // Flow at uid 4
        view.insert(
            4,
            VensimElement::Variable(VensimVariable {
                uid: 4,
                name: "Flow Rate".to_string(),
                x: 150,
                y: 120,
                width: 40,
                height: 20,
                attached: true,
                is_ghost: false,
            }),
        );

        // Connector from valve to Stock A - control point between valve and stock
        view.insert(
            5,
            VensimElement::Connector(VensimConnector {
                uid: 5,
                from_uid: 3,
                to_uid: 1,
                polarity: None,
                control_point: (100, 100), // x=100, different from connector to Stock B
            }),
        );

        // Connector from valve to Stock B - control point between valve and stock
        view.insert(
            6,
            VensimElement::Connector(VensimConnector {
                uid: 6,
                from_uid: 3,
                to_uid: 2,
                polarity: None,
                control_point: (200, 100), // x=200, different from connector to Stock A
            }),
        );

        // Stock A has "flow rate" as outflow, Stock B has it as inflow
        let mut symbols = std::collections::HashMap::new();
        symbols.insert(
            "stock a".to_string(),
            make_symbol_info(VariableType::Stock, vec![], vec!["flow rate".to_string()]),
        );
        symbols.insert(
            "stock b".to_string(),
            make_symbol_info(VariableType::Stock, vec!["flow rate".to_string()], vec![]),
        );

        let endpoints = compute_flow_points(3, 150, 100, &view, "flow rate", &symbols, 0);

        // xpt[0]=100, xpt[1]=200 → different x → horizontal → snap x to anchors
        // xanchor[0]=50 (Stock A), xanchor[1]=250 (Stock B)
        // Result: x snapped to anchors, y stays from control points
        // Stock A is from (outflow), Stock B is to (inflow)
        assert_eq!(endpoints.from_x, 50); // snapped to Stock A's x
        assert_eq!(endpoints.from_y, 100); // control point y
        assert_eq!(endpoints.from_uid, Some(1)); // Stock A's uid
        assert_eq!(endpoints.to_x, 250); // snapped to Stock B's x
        assert_eq!(endpoints.to_y, 100); // control point y
        assert_eq!(endpoints.to_uid, Some(2)); // Stock B's uid
    }

    #[test]
    fn test_compute_flow_points_two_endpoints_vertical() {
        // Test vertical flow with anchor snapping
        // xmutil: if xpt[0] == xpt[1] (same x), snap y coords to anchors
        use super::super::types::{
            VensimConnector, VensimValve, VensimVariable, ViewHeader, ViewVersion,
        };
        use crate::mdl::convert::VariableType;

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        // Stock A (source) at uid 1, y=50
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Stock A".to_string(),
                x: 100,
                y: 50,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Stock B (destination) at uid 2, y=250
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Stock B".to_string(),
                x: 100,
                y: 250,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Valve at uid 3, y=150 (between the stocks)
        view.insert(
            3,
            VensimElement::Valve(VensimValve {
                uid: 3,
                name: "444".to_string(),
                x: 100,
                y: 150,
                width: 6,
                height: 8,
                attached: true,
            }),
        );

        // Flow at uid 4
        view.insert(
            4,
            VensimElement::Variable(VensimVariable {
                uid: 4,
                name: "Flow Rate".to_string(),
                x: 120,
                y: 150,
                width: 40,
                height: 20,
                attached: true,
                is_ghost: false,
            }),
        );

        // Connector from valve to Stock A - same x as other connector (vertical)
        view.insert(
            5,
            VensimElement::Connector(VensimConnector {
                uid: 5,
                from_uid: 3,
                to_uid: 1,
                polarity: None,
                control_point: (100, 100), // same x=100 as connector to Stock B
            }),
        );

        // Connector from valve to Stock B - same x as other connector (vertical)
        view.insert(
            6,
            VensimElement::Connector(VensimConnector {
                uid: 6,
                from_uid: 3,
                to_uid: 2,
                polarity: None,
                control_point: (100, 200), // same x=100 as connector to Stock A
            }),
        );

        // Stock A has "flow rate" as outflow, Stock B has it as inflow
        let mut symbols = std::collections::HashMap::new();
        symbols.insert(
            "stock a".to_string(),
            make_symbol_info(VariableType::Stock, vec![], vec!["flow rate".to_string()]),
        );
        symbols.insert(
            "stock b".to_string(),
            make_symbol_info(VariableType::Stock, vec!["flow rate".to_string()], vec![]),
        );

        let endpoints = compute_flow_points(3, 100, 150, &view, "flow rate", &symbols, 0);

        // xpt[0]=100, xpt[1]=100 → same x → vertical → snap y to anchors
        // yanchor[0]=50 (Stock A), yanchor[1]=250 (Stock B)
        // Result: x stays from control points, y snapped to anchors
        // Stock A is from (outflow), Stock B is to (inflow)
        assert_eq!(endpoints.from_x, 100); // control point x
        assert_eq!(endpoints.from_y, 50); // snapped to Stock A's y
        assert_eq!(endpoints.from_uid, Some(1)); // Stock A's uid
        assert_eq!(endpoints.to_x, 100); // control point x
        assert_eq!(endpoints.to_y, 250); // snapped to Stock B's y
        assert_eq!(endpoints.to_uid, Some(2)); // Stock B's uid
    }

    #[test]
    fn test_compute_flow_points_with_cloud() {
        // Test flow between cloud and stock with anchor snapping
        use super::super::types::{
            VensimComment, VensimConnector, VensimValve, VensimVariable, ViewHeader, ViewVersion,
        };
        use crate::mdl::convert::VariableType;

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        // Cloud (comment) at uid 1, x=50
        view.insert(
            1,
            VensimElement::Comment(VensimComment {
                uid: 1,
                text: "".to_string(),
                x: 50,
                y: 100,
                width: 15,
                height: 15,
                scratch_name: false,
            }),
        );

        // Stock B at uid 2, x=250
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Stock B".to_string(),
                x: 250,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Valve at uid 3, x=150 (between cloud and stock)
        view.insert(
            3,
            VensimElement::Valve(VensimValve {
                uid: 3,
                name: "444".to_string(),
                x: 150,
                y: 100,
                width: 6,
                height: 8,
                attached: true,
            }),
        );

        // Flow at uid 4
        view.insert(
            4,
            VensimElement::Variable(VensimVariable {
                uid: 4,
                name: "Flow Rate".to_string(),
                x: 150,
                y: 120,
                width: 40,
                height: 20,
                attached: true,
                is_ghost: false,
            }),
        );

        // Connector from valve to cloud - control point between valve and cloud
        view.insert(
            5,
            VensimElement::Connector(VensimConnector {
                uid: 5,
                from_uid: 3,
                to_uid: 1,
                polarity: None,
                control_point: (100, 100), // x=100, different from other connector
            }),
        );

        // Connector from valve to Stock B - control point between valve and stock
        view.insert(
            6,
            VensimElement::Connector(VensimConnector {
                uid: 6,
                from_uid: 3,
                to_uid: 2,
                polarity: None,
                control_point: (200, 100), // x=200, different from other connector
            }),
        );

        // Stock B has "flow rate" as inflow
        let mut symbols = std::collections::HashMap::new();
        symbols.insert(
            "stock b".to_string(),
            make_symbol_info(VariableType::Stock, vec!["flow rate".to_string()], vec![]),
        );

        let endpoints = compute_flow_points(3, 150, 100, &view, "flow rate", &symbols, 0);

        // xpt[0]=100, xpt[1]=200 → different x → horizontal → snap x to anchors
        // Cloud at uid 1 should be from (since Stock B is to), Stock B is to
        assert_eq!(endpoints.from_x, 50); // snapped to cloud's x
        assert_eq!(endpoints.from_y, 100); // control point y
        assert_eq!(endpoints.from_uid, Some(1)); // Cloud's uid
        assert_eq!(endpoints.to_x, 250); // snapped to Stock B's x
        assert_eq!(endpoints.to_y, 100); // control point y
        assert_eq!(endpoints.to_uid, Some(2)); // Stock B's uid
    }

    #[test]
    fn test_compute_flow_points_with_uid_offset() {
        // Test that UID offset is properly applied to returned UIDs
        use super::super::types::{
            VensimConnector, VensimValve, VensimVariable, ViewHeader, ViewVersion,
        };
        use crate::mdl::convert::VariableType;

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        // Stock A (source) at uid 1, x=50
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Stock A".to_string(),
                x: 50,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Stock B (destination) at uid 2, x=250
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Stock B".to_string(),
                x: 250,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Valve at uid 3
        view.insert(
            3,
            VensimElement::Valve(VensimValve {
                uid: 3,
                name: "444".to_string(),
                x: 150,
                y: 100,
                width: 6,
                height: 8,
                attached: true,
            }),
        );

        // Flow at uid 4
        view.insert(
            4,
            VensimElement::Variable(VensimVariable {
                uid: 4,
                name: "Flow Rate".to_string(),
                x: 150,
                y: 120,
                width: 40,
                height: 20,
                attached: true,
                is_ghost: false,
            }),
        );

        // Connector from valve (3) to Stock A (1)
        view.insert(
            5,
            VensimElement::Connector(VensimConnector {
                uid: 5,
                from_uid: 3,
                to_uid: 1,
                polarity: None,
                control_point: (100, 100),
            }),
        );

        // Connector from valve (3) to Stock B (2)
        view.insert(
            6,
            VensimElement::Connector(VensimConnector {
                uid: 6,
                from_uid: 3,
                to_uid: 2,
                polarity: None,
                control_point: (200, 100),
            }),
        );

        // Stock A has "flow rate" as outflow, Stock B has it as inflow
        let mut symbols = std::collections::HashMap::new();
        symbols.insert(
            "stock a".to_string(),
            make_symbol_info(VariableType::Stock, vec![], vec!["flow rate".to_string()]),
        );
        symbols.insert(
            "stock b".to_string(),
            make_symbol_info(VariableType::Stock, vec!["flow rate".to_string()], vec![]),
        );

        // Use uid_offset of 100
        let endpoints = compute_flow_points(3, 150, 100, &view, "flow rate", &symbols, 100);

        // The returned UIDs should include the offset
        assert_eq!(endpoints.from_uid, Some(101)); // Stock A's uid (1) + offset (100)
        assert_eq!(endpoints.to_uid, Some(102)); // Stock B's uid (2) + offset (100)
    }

    #[test]
    fn test_multiple_all_ghost_variables_get_promoted() {
        // Test that when multiple variables only appear as ghosts (all-ghost),
        // all of them get promoted to primaries, not just the first one.
        // This tests the Issue 3 fix (removed break statement).
        use super::super::types::{VensimVariable, ViewHeader, ViewVersion};

        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test".to_string(),
        };
        let mut view = VensimView::new(header);

        // Variable A - only appears as ghost
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Var A".to_string(),
                x: 100,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: true, // ghost
            }),
        );

        // Variable B - only appears as ghost (different variable)
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Var B".to_string(),
                x: 200,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: true, // ghost
            }),
        );

        let (primary_map, effective_ghosts) = associate_variables(&[view]);

        // Both should be promoted to primaries since they have no non-ghost appearances
        assert!(
            primary_map.contains_key("var a"),
            "Var A should be promoted to primary"
        );
        assert!(
            primary_map.contains_key("var b"),
            "Var B should be promoted to primary"
        );

        // Neither should be in effective_ghosts since they were promoted
        assert!(
            !effective_ghosts.contains(&(0, 1)),
            "Var A should not be an effective ghost"
        );
        assert!(
            !effective_ghosts.contains(&(0, 2)),
            "Var B should not be an effective ghost"
        );
    }
}
