// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Conversion from Vensim view elements to datamodel::View structures.

use std::collections::HashMap;

use simlin_core::datamodel::{self, Rect, View, ViewElement, view_element};

use super::processing::{
    PrimaryMap, angle_from_points, associate_variables, compose_views, is_cloud_endpoint,
    xmile_angle_to_canvas,
};
use super::types::{VensimComment, VensimElement, VensimVariable, VensimView};

use crate::mdl::builtins::to_lower_space;
use crate::mdl::convert::VariableType;

/// Build datamodel Views from parsed Vensim views.
///
/// This function:
/// 1. Transforms coordinates for multi-view composition
/// 2. Associates variables with their primary views
/// 3. Converts each element to datamodel format
pub fn build_views(
    mut views: Vec<VensimView>,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
) -> Vec<View> {
    if views.is_empty() {
        return Vec::new();
    }

    // Transform coordinates and get uid offsets
    let _offsets = compose_views(&mut views);

    // Track primary variable definitions
    let primary_map = associate_variables(&views);

    // Collect view UID offsets for cross-view alias resolution
    let view_offsets: Vec<i32> = views.iter().map(|v| v.uid_offset).collect();

    // Convert views
    let is_multi_view = views.len() > 1;
    let mut result = Vec::new();

    for (view_idx, view) in views.iter().enumerate() {
        if let Some(dm_view) = convert_view(
            view,
            symbols,
            &primary_map,
            view_idx,
            is_multi_view,
            &view_offsets,
        ) {
            result.push(dm_view);
        }
    }

    // If multiple views, merge into one with group wrappers
    if result.len() > 1 {
        merge_views(result)
    } else {
        result
    }
}

/// Merge multiple views into a single StockFlow view.
///
/// When a Vensim MDL file contains multiple named views, they are combined
/// into a single datamodel View. Each original view's elements are wrapped
/// in a Group element (already added during convert_view when is_multi_view
/// is true), providing sector-like organization.
///
/// The combined view_box encompasses all elements from all original views.
fn merge_views(views: Vec<View>) -> Vec<View> {
    if views.is_empty() {
        return views;
    }

    let mut all_elements = Vec::new();
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for view in views {
        let View::StockFlow(sf) = view;
        // Extend view_box bounds
        min_x = min_x.min(sf.view_box.x);
        min_y = min_y.min(sf.view_box.y);
        max_x = max_x.max(sf.view_box.x + sf.view_box.width);
        max_y = max_y.max(sf.view_box.y + sf.view_box.height);

        all_elements.extend(sf.elements);
    }

    let merged = View::StockFlow(datamodel::StockFlow {
        elements: all_elements,
        view_box: Rect {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        },
        zoom: 1.0,
    });

    vec![merged]
}

/// Convert a single VensimView to a datamodel::View.
#[allow(clippy::too_many_arguments)]
fn convert_view(
    view: &VensimView,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
    primary_map: &PrimaryMap,
    view_idx: usize,
    is_multi_view: bool,
    view_offsets: &[i32],
) -> Option<View> {
    let mut elements = Vec::new();
    let uid_offset = view.uid_offset;

    // Track which comments are clouds (flow endpoints)
    let mut cloud_comments: HashMap<i32, i32> = HashMap::new(); // comment_uid -> flow_uid
    for (uid, _elem) in view.iter_with_uids() {
        if let Some(flow_uid) = is_cloud_endpoint(uid, view) {
            cloud_comments.insert(uid, flow_uid);
        }
    }

    // If multi-view, add a group element for this view
    if is_multi_view {
        let group = create_sector_group(view, uid_offset);
        elements.push(group);
    }

    // Convert elements
    for (local_uid, elem) in view.iter_with_uids() {
        let uid = uid_offset + local_uid;

        match elem {
            VensimElement::Variable(var) => {
                if let Some(view_elem) = convert_variable(
                    var,
                    uid,
                    symbols,
                    primary_map,
                    view,
                    view_idx,
                    uid_offset,
                    view_offsets,
                ) {
                    elements.push(view_elem);
                }
            }
            VensimElement::Valve(_) => {
                // Valves are handled as part of flow conversion
            }
            VensimElement::Comment(comment) => {
                if let Some(&flow_uid) = cloud_comments.get(&local_uid) {
                    // This comment is a cloud (flow endpoint)
                    let cloud = convert_comment_as_cloud(comment, uid, flow_uid + uid_offset);
                    elements.push(cloud);
                }
                // Non-cloud comments are ignored (or could be converted to labels/groups)
            }
            VensimElement::Connector(conn) => {
                if let Some(link) = convert_connector(conn, uid, view, uid_offset, symbols) {
                    elements.push(link);
                }
            }
        }
    }

    // Calculate view bounds
    let view_box = calculate_view_box(&elements);

    Some(View::StockFlow(datamodel::StockFlow {
        elements,
        view_box,
        zoom: 1.0,
    }))
}

/// Check if a variable name is a built-in system variable that should be filtered from views.
/// These are handled automatically by the XMILE runtime.
fn is_builtin_view_variable(canonical: &str) -> bool {
    canonical == "time"
}

/// Convert a variable element to the appropriate ViewElement type.
#[allow(clippy::too_many_arguments)]
fn convert_variable(
    var: &VensimVariable,
    uid: i32,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
    primary_map: &PrimaryMap,
    view: &VensimView,
    view_idx: usize,
    uid_offset: i32,
    view_offsets: &[i32],
) -> Option<ViewElement> {
    let canonical = to_lower_space(&var.name);

    // Skip built-in system variables like "Time"
    if is_builtin_view_variable(&canonical) {
        return None;
    }

    let xmile_name = var.name.replace(' ', "_");

    // Check if this is the primary definition or a ghost
    // Must match both view index AND uid within that view
    let is_primary = primary_map
        .get(&canonical)
        .map(|(idx, primary_uid)| *idx == view_idx && *primary_uid == var.uid)
        .unwrap_or(false);

    if !is_primary && var.is_ghost {
        // This is a ghost/alias - get the primary UID
        if let Some((primary_view_idx, primary_local_uid)) = primary_map.get(&canonical) {
            // Calculate the aliased UID with the primary's view offset
            let primary_offset = view_offsets.get(*primary_view_idx).copied().unwrap_or(0);
            let alias_of_uid = primary_offset + *primary_local_uid;

            return Some(ViewElement::Alias(view_element::Alias {
                uid,
                alias_of_uid,
                x: var.x as f64,
                y: var.y as f64,
                label_side: default_label_side_for_variable(&canonical, symbols),
            }));
        }
    }

    // Determine variable type from symbols
    let var_type = symbols
        .get(&canonical)
        .map(|info| info.var_type)
        .unwrap_or(VariableType::Aux);

    match var_type {
        VariableType::Stock => Some(ViewElement::Stock(view_element::Stock {
            name: xmile_name,
            uid,
            x: var.x as f64,
            y: var.y as f64,
            label_side: view_element::LabelSide::Top, // Stocks default to top
        })),
        VariableType::Flow => {
            // For flows, find the associated valve and compute flow points
            let (flow_x, flow_y, points) = compute_flow_data(var, view, uid_offset, symbols);

            Some(ViewElement::Flow(view_element::Flow {
                name: xmile_name,
                uid,
                x: flow_x as f64,
                y: flow_y as f64,
                label_side: view_element::LabelSide::Bottom,
                points,
            }))
        }
        VariableType::Aux => Some(ViewElement::Aux(view_element::Aux {
            name: xmile_name,
            uid,
            x: var.x as f64,
            y: var.y as f64,
            label_side: view_element::LabelSide::Bottom,
        })),
    }
}

/// Get the default label side for a variable based on its type.
fn default_label_side_for_variable(
    canonical: &str,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
) -> view_element::LabelSide {
    if let Some(info) = symbols.get(canonical) {
        match info.var_type {
            VariableType::Stock => view_element::LabelSide::Top,
            _ => view_element::LabelSide::Bottom,
        }
    } else {
        view_element::LabelSide::Bottom
    }
}

/// Compute flow data including position and flow points.
///
/// Returns (flow_x, flow_y, flow_points) where:
/// - flow_x, flow_y: Position of the flow (from valve if attached, else from variable)
/// - flow_points: Start and end points for the flow pipe with attached UIDs
///
/// Flow point computation searches for connectors from the valve to connected
/// stocks/clouds and determines directionality by checking if this flow appears
/// in the connected stock's inflows or outflows list.
fn compute_flow_data(
    var: &VensimVariable,
    view: &VensimView,
    uid_offset: i32,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
) -> (i32, i32, Vec<view_element::FlowPoint>) {
    // Look for valve at uid - 1 (typical Vensim layout)
    let valve_uid = var.uid - 1;
    let (flow_x, flow_y) = if let Some(VensimElement::Valve(valve)) = view.get(valve_uid) {
        if valve.attached {
            (valve.x, valve.y)
        } else {
            (var.x, var.y)
        }
    } else {
        (var.x, var.y)
    };

    // Get the flow's canonical name for endpoint detection
    let canonical = to_lower_space(&var.name);

    // Compute flow points using the processing module's algorithm
    let endpoints = super::processing::compute_flow_points(
        valve_uid, flow_x, flow_y, view, &canonical, symbols, uid_offset,
    );

    let points = vec![
        view_element::FlowPoint {
            x: endpoints.from_x as f64,
            y: endpoints.from_y as f64,
            attached_to_uid: endpoints.from_uid,
        },
        view_element::FlowPoint {
            x: endpoints.to_x as f64,
            y: endpoints.to_y as f64,
            attached_to_uid: endpoints.to_uid,
        },
    ];

    (flow_x, flow_y, points)
}

/// Convert a comment element that serves as a cloud (flow endpoint).
fn convert_comment_as_cloud(comment: &VensimComment, uid: i32, flow_uid: i32) -> ViewElement {
    ViewElement::Cloud(view_element::Cloud {
        uid,
        flow_uid,
        x: comment.x as f64,
        y: comment.y as f64,
    })
}

/// Convert a connector element to a Link.
fn convert_connector(
    conn: &super::types::VensimConnector,
    uid: i32,
    view: &VensimView,
    uid_offset: i32,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
) -> Option<ViewElement> {
    let from_uid = uid_offset + conn.from_uid;
    let to_uid = uid_offset + conn.to_uid;

    // Skip invalid connectors
    if conn.from_uid <= 0 || conn.to_uid <= 0 {
        return None;
    }

    // Get from/to elements
    let from_elem = view.get(conn.from_uid)?;
    let to_elem = view.get(conn.to_uid)?;

    // Handle valve indirection: if 'from' is a valve, use the next element (flow)
    let (actual_from, actual_from_uid) = match from_elem {
        VensimElement::Valve(v) if v.attached => {
            let flow = view.get(conn.from_uid + 1)?;
            (flow, uid_offset + conn.from_uid + 1)
        }
        _ => (from_elem, from_uid),
    };

    // Similarly for 'to'
    let (actual_to, actual_to_uid) = match to_elem {
        VensimElement::Valve(v) if v.attached => {
            let flow = view.get(conn.to_uid + 1)?;
            (flow, uid_offset + conn.to_uid + 1)
        }
        _ => (to_elem, to_uid),
    };

    // Skip connectors involving clouds (flow endpoints handled as part of Flow element)
    if matches!(actual_from, VensimElement::Comment(_))
        || matches!(actual_to, VensimElement::Comment(_))
    {
        return None;
    }

    // Skip connectors involving built-in variables like "Time"
    if let VensimElement::Variable(v) = actual_from
        && is_builtin_view_variable(&to_lower_space(&v.name))
    {
        return None;
    }
    if let VensimElement::Variable(v) = actual_to
        && is_builtin_view_variable(&to_lower_space(&v.name))
    {
        return None;
    }

    // Skip connectors to stocks (flow connections handled differently)
    if let VensimElement::Variable(v) = actual_to {
        let canonical = to_lower_space(&v.name);
        if let Some(info) = symbols.get(&canonical)
            && info.var_type == VariableType::Stock
        {
            return None;
        }
    }

    // Calculate angle
    let shape = calculate_link_shape(actual_from, actual_to, conn);

    Some(ViewElement::Link(view_element::Link {
        uid,
        from_uid: actual_from_uid,
        to_uid: actual_to_uid,
        shape,
    }))
}

/// Calculate the link shape (straight or arc) based on element positions and control point.
fn calculate_link_shape(
    from: &VensimElement,
    to: &VensimElement,
    conn: &super::types::VensimConnector,
) -> view_element::LinkShape {
    let from_x = from.x() as f64;
    let from_y = from.y() as f64;
    let to_x = to.x() as f64;
    let to_y = to.y() as f64;
    let ctrl_x = conn.control_point.0 as f64;
    let ctrl_y = conn.control_point.1 as f64;

    // If control point is (0, 0), it's a straight line
    if ctrl_x == 0.0 && ctrl_y == 0.0 {
        return view_element::LinkShape::Straight;
    }

    // Calculate angle using AngleFromPoints algorithm
    let xmile_angle = angle_from_points(from_x, from_y, ctrl_x, ctrl_y, to_x, to_y);
    let canvas_angle = xmile_angle_to_canvas(xmile_angle);

    // Check if the angle is close to the straight line angle
    let dx = to_x - from_x;
    let dy = to_y - from_y;
    let straight_angle = dy.atan2(dx).to_degrees();

    if (canvas_angle - straight_angle).abs() < 0.5 {
        view_element::LinkShape::Straight
    } else {
        view_element::LinkShape::Arc(canvas_angle)
    }
}

/// Create a sector/group element for multi-view composition.
fn create_sector_group(view: &VensimView, uid_offset: i32) -> ViewElement {
    let x = view.min_x().unwrap_or(100) as f64 - 40.0;
    let y = view.min_y().unwrap_or(100) as f64;
    let width = (view.max_x(200) - view.min_x().unwrap_or(100)) as f64 + 60.0;
    let height = (view.max_y(200) - view.min_y().unwrap_or(100)) as f64 + 40.0;

    ViewElement::Group(view_element::Group {
        uid: uid_offset,
        name: view.title().to_string(),
        x,
        y,
        width,
        height,
    })
}

/// Calculate the view box (bounding rectangle) for a set of elements.
fn calculate_view_box(elements: &[ViewElement]) -> Rect {
    if elements.is_empty() {
        return Rect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 600.0,
        };
    }

    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for elem in elements {
        let (x, y) = match elem {
            ViewElement::Aux(e) => (e.x, e.y),
            ViewElement::Stock(e) => (e.x, e.y),
            ViewElement::Flow(e) => (e.x, e.y),
            ViewElement::Module(e) => (e.x, e.y),
            ViewElement::Alias(e) => (e.x, e.y),
            ViewElement::Cloud(e) => (e.x, e.y),
            ViewElement::Group(e) => (e.x, e.y),
            ViewElement::Link(_) => continue, // Skip links for bounds calculation
        };

        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }

    // Add some padding
    let padding = 50.0;
    Rect {
        x: min_x - padding,
        y: min_y - padding,
        width: (max_x - min_x) + 2.0 * padding,
        height: (max_y - min_y) + 2.0 * padding,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl::view::types::{ViewHeader, ViewVersion};

    fn create_test_view() -> VensimView {
        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test View".to_string(),
        };
        let mut view = VensimView::new(header);

        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Stock A".to_string(),
                x: 100,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        view
    }

    #[test]
    fn test_build_empty_views() {
        let views: Vec<VensimView> = Vec::new();
        let symbols = HashMap::new();
        let result = build_views(views, &symbols);
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_single_view() {
        let view = create_test_view();
        let symbols = HashMap::new();
        let result = build_views(vec![view], &symbols);

        assert_eq!(result.len(), 1);
        let View::StockFlow(sf) = &result[0];
        assert!(!sf.elements.is_empty());
    }

    #[test]
    fn test_calculate_view_box() {
        let elements = vec![
            ViewElement::Aux(view_element::Aux {
                name: "A".to_string(),
                uid: 1,
                x: 100.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
            }),
            ViewElement::Aux(view_element::Aux {
                name: "B".to_string(),
                uid: 2,
                x: 300.0,
                y: 200.0,
                label_side: view_element::LabelSide::Bottom,
            }),
        ];

        let view_box = calculate_view_box(&elements);

        assert!(view_box.x < 100.0); // Includes padding
        assert!(view_box.y < 100.0);
        assert!(view_box.width > 200.0);
        assert!(view_box.height > 100.0);
    }

    #[test]
    fn test_create_sector_group() {
        let view = create_test_view();
        let group = create_sector_group(&view, 0);

        if let ViewElement::Group(g) = group {
            assert_eq!(g.name, "Test View");
            assert!(g.width > 0.0);
            assert!(g.height > 0.0);
        } else {
            panic!("Expected Group element");
        }
    }

    #[test]
    fn test_time_variable_filtered_out() {
        // Test that the special "Time" variable is filtered from views
        // since it's a built-in system variable that XMILE handles automatically.
        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test View".to_string(),
        };
        let mut view = VensimView::new(header);

        // Regular variable
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "x".to_string(),
                x: 100,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Time variable (built-in, typically a ghost reference)
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Time".to_string(),
                x: 200,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: true, // Usually marked as ghost
            }),
        );

        // Connector from Time to x
        view.insert(
            3,
            VensimElement::Connector(super::super::types::VensimConnector {
                uid: 3,
                from_uid: 2,
                to_uid: 1,
                polarity: None,
                control_point: (0, 0),
            }),
        );

        let symbols = HashMap::new();
        let result = build_views(vec![view], &symbols);

        assert_eq!(result.len(), 1);
        let View::StockFlow(sf) = &result[0];

        // Count element types
        let mut aux_count = 0;
        let mut link_count = 0;
        for elem in &sf.elements {
            match elem {
                ViewElement::Aux(a) => {
                    aux_count += 1;
                    // Time should not appear
                    assert_ne!(
                        a.name.to_lowercase(),
                        "time",
                        "Time variable should be filtered out"
                    );
                }
                ViewElement::Link(_) => link_count += 1,
                _ => {}
            }
        }

        // Should have 1 aux (x only, not Time) and 0 links (connector to Time filtered)
        assert_eq!(
            aux_count, 1,
            "Expected 1 aux element (Time should be filtered)"
        );
        assert_eq!(
            link_count, 0,
            "Expected 0 links (connector involving Time should be filtered)"
        );
    }

    #[test]
    fn test_ghost_variable_becomes_alias() {
        // Test that a ghost variable (is_ghost=true) becomes an Alias element
        // when there's a primary definition of the same variable.
        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test View".to_string(),
        };
        let mut view = VensimView::new(header);

        // Primary variable at uid 1
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Contact Rate".to_string(),
                x: 100,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false, // Primary definition
            }),
        );

        // Ghost variable at uid 2 (same name, is_ghost=true)
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Contact Rate".to_string(),
                x: 200,
                y: 200,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: true, // Ghost/alias
            }),
        );

        let symbols = HashMap::new();
        let result = build_views(vec![view], &symbols);

        assert_eq!(result.len(), 1);
        let View::StockFlow(sf) = &result[0];

        // Count element types
        let mut aux_count = 0;
        let mut alias_count = 0;
        for elem in &sf.elements {
            match elem {
                ViewElement::Aux(_) => aux_count += 1,
                ViewElement::Alias(_) => alias_count += 1,
                _ => {}
            }
        }

        // Should have 1 aux (primary) and 1 alias (ghost)
        assert_eq!(aux_count, 1, "Expected 1 aux element for primary variable");
        assert_eq!(
            alias_count, 1,
            "Expected 1 alias element for ghost variable"
        );
    }
}
