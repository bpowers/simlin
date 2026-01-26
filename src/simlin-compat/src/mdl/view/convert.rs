// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Conversion from Vensim view elements to datamodel::View structures.

use std::collections::HashMap;

use simlin_core::datamodel::{self, View, ViewElement, view_element};

use std::collections::HashSet;

use super::processing::{
    EffectiveGhosts, PrimaryMap, angle_from_points, associate_variables, compose_views,
    is_cloud_endpoint, xmile_angle_to_canvas,
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
///
/// `all_names` includes variable names, dimension names, and group names
/// for collision-free view title deduplication (matching xmutil's full namespace).
pub fn build_views(
    mut views: Vec<VensimView>,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
    all_names: &HashSet<String>,
) -> Vec<View> {
    if views.is_empty() {
        return Vec::new();
    }

    // Compute model-level letter polarity flag: true if any connector
    // in any view used S/O notation (matching xmutil's bLetterPolarity).
    let use_lettered_polarity = views.iter().any(|view| {
        view.iter().any(|elem| {
            matches!(
                elem,
                VensimElement::Connector(conn) if conn.letter_polarity
            )
        })
    });

    // Normalize view titles and make them unique (xmutil MakeViewNamesUnique)
    make_view_names_unique(&mut views, all_names);

    // Transform coordinates and get uid offsets
    let _offsets = compose_views(&mut views);

    // Track primary variable definitions and effective ghosts
    let (primary_map, effective_ghosts) = associate_variables(&views);

    // Collect view UID offsets for cross-view alias resolution
    let view_offsets: Vec<i32> = views.iter().map(|v| v.uid_offset).collect();

    // Convert views
    // Track start positions for group geometry (matches compose_views logic)
    let is_multi_view = views.len() > 1;
    let mut result = Vec::new();
    let start_x = 100;
    let mut start_y = 100;

    for (view_idx, view) in views.iter().enumerate() {
        if let Some(dm_view) = convert_view(
            view,
            symbols,
            &primary_map,
            &effective_ghosts,
            view_idx,
            is_multi_view,
            &view_offsets,
            start_x,
            start_y,
            use_lettered_polarity,
        ) {
            result.push(dm_view);
        }
        // Advance start_y for next view (same formula as compose_views)
        let height = view.max_y(start_y + 80) - start_y;
        start_y += height + 80;
    }

    // If multiple views, merge into one with group wrappers
    let mut result = if result.len() > 1 {
        merge_views(result)
    } else {
        result
    };

    // Post-processing: adjust flow points to stock edges and reassign sequential UIDs.
    // This matches the XMILE path's normalize() sequence:
    //   1. assign_uids() — sequential UIDs
    //   2. fixup_clouds() — sets pt.uid on flow points
    //   3. fixup_flow_takeoffs() — adjusts flow point coords to stock edges
    // We do steps 3 then 1 (flow points already have attached_to_uid from compute_flow_points).
    for view in &mut result {
        let View::StockFlow(sf) = view;
        fixup_flow_takeoffs(&mut sf.elements);
        reassign_uids_sequential(&mut sf.elements);
    }

    result
}

// Stock dimensions matching the XMILE constants in xmile.rs
const STOCK_WIDTH: f64 = 45.0;
const STOCK_HEIGHT: f64 = 35.0;

/// Adjust flow point coordinates from stock centers to stock edges.
///
/// Matches the XMILE path's `fixup_flow_takeoffs()` in xmile.rs.
/// When a flow point is attached to a stock, the coordinate is snapped
/// to the nearest edge of the stock rectangle rather than its center.
fn fixup_flow_takeoffs(elements: &mut [ViewElement]) {
    // Collect stock positions by UID
    let stocks: HashMap<i32, (f64, f64)> = elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Stock(s) = e {
                Some((s.uid, (s.x, s.y)))
            } else {
                None
            }
        })
        .collect();

    for elem in elements.iter_mut() {
        if let ViewElement::Flow(flow) = elem {
            if flow.points.len() != 2 {
                continue;
            }
            let source = flow.points[0].clone();
            let sink = flow.points[1].clone();

            // Adjust source point if attached to a stock
            if let Some(stock_uid) = source.attached_to_uid
                && let Some(&(sx, sy)) = stocks.get(&stock_uid)
            {
                adjust_takeoff_point(&mut flow.points[0], sx, sy, &sink);
            }

            // Adjust sink point if attached to a stock
            if let Some(stock_uid) = sink.attached_to_uid
                && let Some(&(sx, sy)) = stocks.get(&stock_uid)
            {
                adjust_takeoff_point(&mut flow.points[1], sx, sy, &source);
            }
        }
    }
}

/// Snap a flow point to the nearest edge of its attached stock.
///
/// `sx, sy` is the stock center. `other` is the flow point at the other end.
/// The point is moved to the stock edge facing the other endpoint.
fn adjust_takeoff_point(
    pt: &mut view_element::FlowPoint,
    sx: f64,
    sy: f64,
    other: &view_element::FlowPoint,
) {
    if other.x > sx + STOCK_WIDTH / 2.0 && (other.y - sy).abs() < STOCK_HEIGHT / 2.0 {
        // Other point is to the right
        pt.x = sx + STOCK_WIDTH / 2.0;
    } else if other.x < sx - STOCK_WIDTH / 2.0 && (other.y - sy).abs() < STOCK_HEIGHT / 2.0 {
        // Other point is to the left
        pt.x = sx - STOCK_WIDTH / 2.0;
    } else if other.y < sy - STOCK_HEIGHT / 2.0 && (other.x - sx).abs() < STOCK_WIDTH / 2.0 {
        // Other point is above
        pt.y = sy - STOCK_HEIGHT / 2.0;
    } else if other.y > sy + STOCK_HEIGHT / 2.0 && (other.x - sx).abs() < STOCK_WIDTH / 2.0 {
        // Other point is below
        pt.y = sy + STOCK_HEIGHT / 2.0;
    }
}

/// Reassign UIDs sequentially starting from 1 and update all cross-references.
///
/// Matches the XMILE path's `assign_uids()` in xmile.rs, which assigns
/// UIDs sequentially in element order starting from 1.
fn reassign_uids_sequential(elements: &mut [ViewElement]) {
    // Build old_uid -> new_uid mapping
    let mut uid_map: HashMap<i32, i32> = HashMap::new();
    let mut next_uid = 1;
    for elem in elements.iter() {
        let old_uid = elem.get_uid();
        uid_map.insert(old_uid, next_uid);
        next_uid += 1;
    }

    let remap = |uid: i32| -> i32 { uid_map.get(&uid).copied().unwrap_or(uid) };

    // Apply new UIDs and update cross-references
    for elem in elements.iter_mut() {
        match elem {
            ViewElement::Aux(a) => a.uid = remap(a.uid),
            ViewElement::Stock(s) => s.uid = remap(s.uid),
            ViewElement::Flow(f) => {
                f.uid = remap(f.uid);
                for pt in &mut f.points {
                    pt.attached_to_uid = pt.attached_to_uid.map(&remap);
                }
            }
            ViewElement::Link(l) => {
                l.uid = remap(l.uid);
                l.from_uid = remap(l.from_uid);
                l.to_uid = remap(l.to_uid);
            }
            ViewElement::Module(m) => m.uid = remap(m.uid),
            ViewElement::Alias(a) => {
                a.uid = remap(a.uid);
                a.alias_of_uid = remap(a.alias_of_uid);
            }
            ViewElement::Cloud(c) => {
                c.uid = remap(c.uid);
                c.flow_uid = remap(c.flow_uid);
            }
            ViewElement::Group(g) => g.uid = remap(g.uid),
        }
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
    let mut use_lettered_polarity = false;

    for view in views {
        let View::StockFlow(sf) = view;
        use_lettered_polarity = use_lettered_polarity || sf.use_lettered_polarity;
        all_elements.extend(sf.elements);
    }

    let merged = View::StockFlow(datamodel::StockFlow {
        elements: all_elements,
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity,
    });

    vec![merged]
}

/// Convert a single VensimView to a datamodel::View.
#[allow(clippy::too_many_arguments)]
fn convert_view(
    view: &VensimView,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
    primary_map: &PrimaryMap,
    effective_ghosts: &EffectiveGhosts,
    view_idx: usize,
    is_multi_view: bool,
    view_offsets: &[i32],
    start_x: i32,
    start_y: i32,
    use_lettered_polarity: bool,
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
        let group = create_sector_group(view, uid_offset, start_x, start_y);
        elements.push(group);
    }

    // Two-phase conversion to avoid dangling cloud references:
    // Phase 1: Convert variables, track emitted flow UIDs
    // Phase 2: Create clouds only for flows that were actually emitted
    let mut emitted_flow_uids: HashSet<i32> = HashSet::new();

    // Deferred clouds: (local_uid, uid, comment, flow_uid_with_offset)
    let mut deferred_clouds: Vec<(&VensimComment, i32, i32)> = Vec::new();

    for (local_uid, elem) in view.iter_with_uids() {
        let uid = uid_offset + local_uid;

        match elem {
            VensimElement::Variable(var) => {
                if let Some(view_elem) = convert_variable(
                    var,
                    uid,
                    symbols,
                    primary_map,
                    effective_ghosts,
                    view,
                    view_idx,
                    uid_offset,
                    view_offsets,
                ) {
                    if matches!(&view_elem, ViewElement::Flow(_)) {
                        emitted_flow_uids.insert(uid);
                    }
                    elements.push(view_elem);
                }
            }
            VensimElement::Valve(_) => {
                // Valves are handled as part of flow conversion
            }
            VensimElement::Comment(comment) => {
                if let Some(&flow_uid) = cloud_comments.get(&local_uid) {
                    let flow_uid_with_offset = flow_uid + uid_offset;
                    deferred_clouds.push((comment, uid, flow_uid_with_offset));
                }
                // Non-cloud comments are ignored
            }
            VensimElement::Connector(conn) => {
                if let Some(link) = convert_connector(conn, uid, view, uid_offset, symbols) {
                    elements.push(link);
                }
            }
        }
    }

    // Phase 2: Emit clouds only for flows that were actually emitted
    for (comment, uid, flow_uid_with_offset) in deferred_clouds {
        if emitted_flow_uids.contains(&flow_uid_with_offset) {
            elements.push(convert_comment_as_cloud(comment, uid, flow_uid_with_offset));
        }
    }

    Some(View::StockFlow(datamodel::StockFlow {
        elements,
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity,
    }))
}

/// Check if a variable should be filtered from views.
///
/// Variables are filtered if:
/// 1. They are the "Time" built-in variable (handled automatically by XMILE runtime)
/// 2. They are "unwanted" control variables (INITIAL TIME, FINAL TIME, TIME STEP, SAVEPER)
///
/// Note: xmutil also filters XMILE_Type_ARRAY and XMILE_Type_ARRAY_ELM from views
/// (XMILEGenerator.cpp:355-360, VensimView.cpp:375-376). In our code, these correspond
/// to subscript definitions (Equation::SubscriptDef) which are stored in `dimensions`,
/// not in `symbols`. So convert_variable's early `symbols.get(&canonical)?` already
/// returns None for these, effectively filtering them without an explicit check here.
fn should_filter_from_view(
    canonical: &str,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
) -> bool {
    // Time variable (case-insensitive via canonical)
    if canonical == "time" {
        return true;
    }

    // Check if it's an unwanted variable (control variables like sim specs)
    if let Some(info) = symbols.get(canonical)
        && info.unwanted
    {
        return true;
    }

    false
}

/// Convert a variable element to the appropriate ViewElement type.
#[allow(clippy::too_many_arguments)]
fn convert_variable(
    var: &VensimVariable,
    uid: i32,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
    primary_map: &PrimaryMap,
    effective_ghosts: &EffectiveGhosts,
    view: &VensimView,
    view_idx: usize,
    uid_offset: i32,
    view_offsets: &[i32],
) -> Option<ViewElement> {
    let canonical = to_lower_space(&var.name);

    // Skip Time and unwanted control variables
    if should_filter_from_view(&canonical, symbols) {
        return None;
    }

    // Skip variables not in symbol table (xmutil behavior) -- check early
    // to avoid creating dangling alias references for unknown variables
    let symbol_info = symbols.get(&canonical)?;

    let xmile_name = var.name.replace(' ', "_");

    // Check if this is the primary definition or a ghost
    // Must match both view index AND uid within that view
    let is_primary = primary_map
        .get(&canonical)
        .map(|(idx, primary_uid)| *idx == view_idx && *primary_uid == var.uid)
        .unwrap_or(false);

    // A variable is an effective ghost if:
    // 1. It's marked as ghost in the MDL file (var.is_ghost), OR
    // 2. It's a duplicate that was marked as effective ghost during association
    let is_effective_ghost = var.is_ghost || effective_ghosts.contains(&(view_idx, var.uid));

    if !is_primary && is_effective_ghost {
        // This is a ghost/alias - get the primary UID
        if let Some((primary_view_idx, primary_local_uid)) = primary_map.get(&canonical) {
            // Calculate the aliased UID with the primary's view offset
            let primary_offset = view_offsets.get(*primary_view_idx).copied().unwrap_or(0);
            let alias_of_uid = primary_offset + *primary_local_uid;

            // For stock ghosts, apply xmutil offset: x - 22, y - 17
            // (XMILEGenerator.cpp:921-929)
            let (alias_x, alias_y) = if symbol_info.var_type == VariableType::Stock {
                (var.x as f64 - 22.0, var.y as f64 - 17.0)
            } else {
                (var.x as f64, var.y as f64)
            };

            return Some(ViewElement::Alias(view_element::Alias {
                uid,
                alias_of_uid,
                x: alias_x,
                y: alias_y,
                label_side: view_element::LabelSide::Bottom,
            }));
        }
    }

    let var_type = symbol_info.var_type;

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
    // xmutil requires BOTH conditions:
    // 1. Flow variable has attached=true (vele->Attached())
    // 2. Preceding element is a valve (elements[local_uid - 1]->Type() == VALVE)
    let valve_uid = var.uid - 1;
    let (flow_x, flow_y) = if var.attached  // Flow must be attached
        && let Some(VensimElement::Valve(_valve)) = view.get(valve_uid)
    {
        // Use valve coordinates for flow element position
        (_valve.x, _valve.y)
    } else {
        // Use flow variable coordinates
        (var.x, var.y)
    };

    // Get the flow's canonical name for endpoint detection
    let canonical = to_lower_space(&var.name);

    // Compute flow points using the processing module's algorithm
    // Pass the flow variable's coordinates for fallback (not valve's) per xmutil behavior
    let endpoints = super::processing::compute_flow_points(
        valve_uid, var.x, var.y, view, &canonical, symbols, uid_offset,
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

    // After valve indirection, verify both endpoints are variables
    if !matches!(actual_from, VensimElement::Variable(_))
        || !matches!(actual_to, VensimElement::Variable(_))
    {
        return None;
    }

    // Skip connectors involving Time or unwanted control variables
    if let VensimElement::Variable(v) = actual_from {
        let canonical = to_lower_space(&v.name);
        if should_filter_from_view(&canonical, symbols) {
            return None;
        }
        // Skip connectors involving unknown variables (consistent with variable skipping)
        if !symbols.contains_key(&canonical) {
            return None;
        }
    }
    if let VensimElement::Variable(v) = actual_to {
        let canonical = to_lower_space(&v.name);
        if should_filter_from_view(&canonical, symbols) {
            return None;
        }
        // Skip connectors involving unknown variables (consistent with variable skipping)
        if !symbols.contains_key(&canonical) {
            return None;
        }
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

    let polarity = match conn.polarity {
        Some('+') => Some(view_element::LinkPolarity::Positive),
        Some('-') => Some(view_element::LinkPolarity::Negative),
        _ => None,
    };

    Some(ViewElement::Link(view_element::Link {
        uid,
        from_uid: actual_from_uid,
        to_uid: actual_to_uid,
        shape,
        polarity,
    }))
}

/// Epsilon for comparing angles - angles within this threshold are considered equal.
/// This is tight to ensure roundtrip fidelity (matching xmile.rs behavior).
const ANGLE_EPSILON_DEGREES: f64 = 0.01;

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

    // If control point is (0, 0), it's a straight line sentinel
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

    // Handle wrap-around (e.g., -179 vs 179 should be close)
    let diff = (canvas_angle - straight_angle).abs();
    let diff = if diff > 180.0 { 360.0 - diff } else { diff };

    if diff < ANGLE_EPSILON_DEGREES {
        view_element::LinkShape::Straight
    } else {
        view_element::LinkShape::Arc(canvas_angle)
    }
}

/// Create a sector/group element for multi-view composition.
///
/// Uses xmutil's exact formula (XMILEGenerator.cpp:886-892) for bounds,
/// then converts to center coordinates for datamodel.
///
/// xmutil formula gives top-left:
/// - top_left_x = start_x - 40
/// - top_left_y = start_y (the y before +20 content offset)
/// - width = max_x + 60 (absolute coordinate)
/// - height = (max_y - start_y) + 40
///
/// datamodel::view_element::Group expects CENTER coordinates.
fn create_sector_group(
    view: &VensimView,
    uid_offset: i32,
    start_x: i32,
    start_y: i32,
) -> ViewElement {
    // xmutil formula gives top-left coordinates
    let top_left_x = (start_x - 40) as f64;
    let top_left_y = start_y as f64;

    // xmutil formula: width = GetViewMaxX(100) + 60
    // After transformation, GetViewMaxX returns the absolute max_x
    let width = (view.max_x(100) + 60) as f64;

    // xmutil formula: height = GetViewMaxY(starty + 80) - starty + 40
    let height = (view.max_y(start_y + 80) - start_y + 40) as f64;

    // Convert to center coordinates for datamodel
    ViewElement::Group(view_element::Group {
        uid: uid_offset,
        name: view.title().to_string(),
        x: top_left_x + width / 2.0,
        y: top_left_y + height / 2.0,
        width,
        height,
    })
}

/// Normalize a view title by replacing special characters with spaces.
///
/// Implements xmutil's MakeViewNamesUnique normalization (Model.cpp:587-592):
/// - Replace `.`, `-`, `+`, `,`, `/`, `*`, `^` with space
/// - Collapse consecutive spaces
fn normalize_view_title(title: &str) -> String {
    let mut result = String::new();
    for c in title.chars() {
        let c = match c {
            '.' | '-' | '+' | ',' | '/' | '*' | '^' => ' ',
            _ => c,
        };
        // Collapse spaces: only add if non-space or prev not space
        if c != ' ' || (!result.is_empty() && !result.ends_with(' ')) {
            result.push(c);
        }
    }
    result
}

/// Make view names unique by normalizing titles and deduplicating.
///
/// Implements xmutil's MakeViewNamesUnique (Model.cpp:580-600):
/// 1. Normalize title (replace special chars, collapse spaces)
/// 2. If empty, use "Module " (note trailing space)
/// 3. Append "1" until unique against symbol namespace AND already-used names
fn make_view_names_unique(views: &mut [VensimView], symbol_names: &HashSet<String>) {
    // used_names uses raw (non-canonicalized) strings for case-sensitive comparison,
    // matching xmutil's std::set<std::string> which does case-sensitive find().
    // The symbol_names check remains case-insensitive (via to_lower_space),
    // matching xmutil's GetNameSpace()->Find() which uses ToLowerSpace internally.
    let mut used_names: HashSet<String> = HashSet::new();

    for view in views.iter_mut() {
        let mut name = normalize_view_title(view.title());

        if name.is_empty() {
            name = "Module ".to_string(); // Note trailing space per xmutil
        }

        while symbol_names.contains(&to_lower_space(&name)) || used_names.contains(&name) {
            name.push('1');
        }

        used_names.insert(name.clone());
        view.set_title(name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mdl::convert::SymbolInfo;
    use crate::mdl::view::types::{ViewHeader, ViewVersion};

    fn make_symbol_info(var_type: VariableType) -> SymbolInfo<'static> {
        SymbolInfo {
            var_type,
            equations: vec![],
            inflows: vec![],
            outflows: vec![],
            unwanted: false,
            alternate_name: None,
        }
    }

    fn names_from_symbols(
        symbols: &HashMap<String, SymbolInfo<'_>>,
    ) -> std::collections::HashSet<String> {
        symbols.keys().cloned().collect()
    }

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
        let result = build_views(views, &symbols, &names_from_symbols(&symbols));
        assert!(result.is_empty());
    }

    #[test]
    fn test_build_single_view() {
        let view = create_test_view();
        let mut symbols = HashMap::new();
        symbols.insert("stock a".to_string(), make_symbol_info(VariableType::Stock));
        let result = build_views(vec![view], &symbols, &names_from_symbols(&symbols));

        assert_eq!(result.len(), 1);
        let View::StockFlow(sf) = &result[0];
        assert!(!sf.elements.is_empty());
    }

    #[test]
    fn test_create_sector_group() {
        let view = create_test_view();
        // Typical starting position: x=100, y=100
        let group = create_sector_group(&view, 0, 100, 100);

        if let ViewElement::Group(g) = group {
            assert_eq!(g.name, "Test View");
            assert!(g.width > 0.0);
            assert!(g.height > 0.0);
            // top_left_x = start_x - 40 = 60, x = top_left_x + width/2
            // top_left_y = start_y = 100, y = top_left_y + height/2
            // Group coordinates should be CENTER, not top-left
            assert_eq!(g.x, 60.0 + g.width / 2.0);
            assert_eq!(g.y, 100.0 + g.height / 2.0);
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
                letter_polarity: false,
                control_point: (0, 0),
            }),
        );

        // Add symbol info for x (but not Time - Time is filtered by should_filter_from_view)
        let mut symbols = HashMap::new();
        symbols.insert("x".to_string(), make_symbol_info(VariableType::Aux));
        let result = build_views(vec![view], &symbols, &names_from_symbols(&symbols));

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

        // Add symbol info for the variable
        let mut symbols = HashMap::new();
        symbols.insert(
            "contact rate".to_string(),
            make_symbol_info(VariableType::Aux),
        );
        let result = build_views(vec![view], &symbols, &names_from_symbols(&symbols));

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

    #[test]
    fn test_view_title_collision_with_symbol_terminates() {
        // Test that view named "Population" with variable "population" terminates
        // without infinite loop (Issue 1 fix)
        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Population".to_string(), // Same as variable name after canonicalization
        };
        let mut view = VensimView::new(header);

        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Population".to_string(),
                x: 100,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        let mut symbols = HashMap::new();
        symbols.insert(
            "population".to_string(),
            make_symbol_info(VariableType::Stock),
        );

        // This should complete without infinite loop
        let result = build_views(vec![view], &symbols, &names_from_symbols(&symbols));

        assert_eq!(result.len(), 1);
        let View::StockFlow(sf) = &result[0];
        assert!(!sf.elements.is_empty());
    }

    #[test]
    fn test_unknown_variable_skipped() {
        // Test that a variable not in symbols is not emitted (Issue 6 fix)
        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test View".to_string(),
        };
        let mut view = VensimView::new(header);

        // Known variable
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Known Var".to_string(),
                x: 100,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Unknown variable (not in symbols)
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Unknown Var".to_string(),
                x: 200,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Only add Known Var to symbols
        let mut symbols = HashMap::new();
        symbols.insert("known var".to_string(), make_symbol_info(VariableType::Aux));

        let result = build_views(vec![view], &symbols, &names_from_symbols(&symbols));

        assert_eq!(result.len(), 1);
        let View::StockFlow(sf) = &result[0];

        // Should only have 1 element (Known Var), not 2
        let aux_count = sf
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Aux(_)))
            .count();
        assert_eq!(aux_count, 1, "Unknown variable should be skipped");
    }

    #[test]
    fn test_connector_unknown_endpoint_skipped() {
        // Test that a connector with unknown variable endpoint is not emitted (Issue 7 fix)
        let header = ViewHeader {
            version: ViewVersion::V300,
            title: "Test View".to_string(),
        };
        let mut view = VensimView::new(header);

        // Known variable
        view.insert(
            1,
            VensimElement::Variable(VensimVariable {
                uid: 1,
                name: "Known Var".to_string(),
                x: 100,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Unknown variable (not in symbols)
        view.insert(
            2,
            VensimElement::Variable(VensimVariable {
                uid: 2,
                name: "Unknown Var".to_string(),
                x: 200,
                y: 100,
                width: 40,
                height: 20,
                attached: false,
                is_ghost: false,
            }),
        );

        // Connector from Known to Unknown
        view.insert(
            3,
            VensimElement::Connector(super::super::types::VensimConnector {
                uid: 3,
                from_uid: 1,
                to_uid: 2,
                polarity: None,
                letter_polarity: false,
                control_point: (0, 0),
            }),
        );

        // Only add Known Var to symbols
        let mut symbols = HashMap::new();
        symbols.insert("known var".to_string(), make_symbol_info(VariableType::Aux));

        let result = build_views(vec![view], &symbols, &names_from_symbols(&symbols));

        assert_eq!(result.len(), 1);
        let View::StockFlow(sf) = &result[0];

        // Should have 0 links (connector to unknown variable should be skipped)
        let link_count = sf
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Link(_)))
            .count();
        assert_eq!(
            link_count, 0,
            "Connector with unknown endpoint should be skipped"
        );
    }
}
