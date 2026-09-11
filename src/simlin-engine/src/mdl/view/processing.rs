// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! View processing logic for Vensim sketch conversion.
//!
//! This module handles coordinate transformation, flow point computation,
//! angle calculation, and ghost/primary variable tracking.

use std::collections::HashMap;
use std::f64::consts::PI;

use super::types::{VensimElement, VensimValve, VensimVariable, VensimView};

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
    view.x_offset = off_x.round() as i32;
    view.y_offset = off_y.round() as i32;

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

/// Build lookup tables between attached valves and attached flow variables.
///
/// Legacy MDL commonly uses `flow_uid = valve_uid + 1`, but writer output may
/// allocate non-adjacent valve UIDs to avoid collisions. This resolver accepts
/// both forms:
/// 1. Prefer the legacy adjacency pair when present.
/// 2. Otherwise, pair remaining attached valves/flows by nearest attached layout
///    position (flow label anchored below the valve).
pub fn build_attached_valve_flow_maps(view: &VensimView) -> (HashMap<i32, i32>, HashMap<i32, i32>) {
    const ATTACHED_FLOW_LABEL_OFFSET_Y: i32 = 16;

    let mut attached_flows: Vec<(i32, i32, i32)> = Vec::new();
    let mut unmatched_valves: HashMap<i32, (i32, i32)> = HashMap::new();

    for elem in view.iter() {
        match elem {
            VensimElement::Variable(flow) if flow.attached => {
                attached_flows.push((flow.uid, flow.x, flow.y));
            }
            VensimElement::Valve(valve) if valve.attached => {
                unmatched_valves.insert(valve.uid, (valve.x, valve.y));
            }
            _ => {}
        }
    }

    let mut valve_to_flow: HashMap<i32, i32> = HashMap::new();
    let mut flow_to_valve: HashMap<i32, i32> = HashMap::new();
    let mut unmatched_flows: Vec<(i32, i32, i32)> = Vec::new();

    // Prefer legacy adjacent UIDs when available.
    for (flow_uid, flow_x, flow_y) in attached_flows {
        let legacy_valve_uid = flow_uid - 1;
        if unmatched_valves.remove(&legacy_valve_uid).is_some() {
            valve_to_flow.insert(legacy_valve_uid, flow_uid);
            flow_to_valve.insert(flow_uid, legacy_valve_uid);
        } else {
            unmatched_flows.push((flow_uid, flow_x, flow_y));
        }
    }

    // Pair the rest by closest attached layout position.
    for (flow_uid, flow_x, flow_y) in unmatched_flows {
        let mut best: Option<(i32, i64)> = None;
        for (&valve_uid, &(valve_x, valve_y)) in &unmatched_valves {
            let dx = i64::from(flow_x) - i64::from(valve_x);
            let dy =
                i64::from(flow_y) - (i64::from(valve_y) + i64::from(ATTACHED_FLOW_LABEL_OFFSET_Y));
            let score = dx * dx + dy * dy;

            match best {
                None => best = Some((valve_uid, score)),
                Some((best_uid, best_score))
                    if score < best_score || (score == best_score && valve_uid < best_uid) =>
                {
                    best = Some((valve_uid, score))
                }
                _ => {}
            }
        }

        if let Some((valve_uid, _)) = best {
            unmatched_valves.remove(&valve_uid);
            valve_to_flow.insert(valve_uid, flow_uid);
            flow_to_valve.insert(flow_uid, valve_uid);
        }
    }

    (valve_to_flow, flow_to_valve)
}

/// Resolve an attached flow UID from a valve UID.
///
/// Uses the precomputed map when available and falls back to legacy
/// `valve_uid + 1` adjacency for compatibility with older assumptions.
pub fn resolve_flow_uid_for_valve(
    valve_uid: i32,
    view: &VensimView,
    valve_to_flow: &HashMap<i32, i32>,
) -> Option<i32> {
    valve_to_flow.get(&valve_uid).copied().or_else(|| {
        let candidate = valve_uid.checked_add(1)?;
        match view.get(candidate) {
            Some(VensimElement::Variable(flow)) if flow.attached => Some(candidate),
            _ => None,
        }
    })
}

/// The valve a flow record is drawn with: an attached flow record's paired
/// attached valve (`flow_to_valve`), falling back to the legacy `uid - 1`
/// adjacency. `None` for a flow drawn as a bare label.
pub fn flow_valve<'a>(
    var: &VensimVariable,
    view: &'a VensimView,
    flow_to_valve: &HashMap<i32, i32>,
) -> Option<&'a VensimValve> {
    if !var.attached {
        return None;
    }
    let valve_uid = flow_to_valve.get(&var.uid).copied().unwrap_or(var.uid - 1);
    match view.get(valve_uid) {
        Some(VensimElement::Valve(valve)) => Some(valve),
        _ => None,
    }
}

/// What a flow's pipe end is drawn into.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum PipeTarget {
    /// A stock, by canonical (`to_lower_space`) name.
    Stock(String),
    /// A comment drawn as a cloud, by its local uid in the view.
    Cloud(i32),
}

/// One end of an imported flow as its sketch pipe and the model's stock lists
/// resolve it.
///
/// The attachments follow the model: a side the model links to a stock ends
/// on that stock, and every other side ends in a cloud. The sketch supplies
/// where an end is drawn when it can; `convert` places the ends it cannot.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum FlowEnd {
    /// The pipe ends at what this side attaches to: the stock the model links
    /// on this side, or a cloud. `x`/`y` is the pipe end.
    Pipe { x: i32, y: i32, target: PipeTarget },
    /// The pipe ends at a stock that does not list the flow on this side (the
    /// importer gave that stock a synthesized net flow), so this side has no
    /// stock and ends in a cloud clear of that stock, which the sketch drew at
    /// `stock`.
    PipeAtUnlinkedStock { x: i32, y: i32, stock: (i32, i32) },
    /// The model links this side to the stock with this canonical name, and
    /// the pipe does not reach it (or there is no pipe).
    Stock(String),
    /// No stock on this side and no pipe end for it.
    Free,
}

/// Both ends of an imported flow.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub struct FlowEnds {
    pub source: FlowEnd,
    pub sink: FlowEnd,
}

/// The stock the model links a flow to on one side: the stock listing it as
/// an inflow (`sink`) or an outflow. The MDL importer lets a flow fill at most
/// one stock and drain at most one (a rate that would break that becomes a
/// synthesized net flow), so there is at most one; the sort only makes a
/// degenerate symbol table deterministic.
fn model_stock(
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
    flow_name: &str,
    sink: bool,
) -> Option<String> {
    use crate::mdl::convert::VariableType;

    let mut names: Vec<&String> = symbols
        .iter()
        .filter(|(_, info)| {
            let list = if sink { &info.inflows } else { &info.outflows };
            info.var_type == VariableType::Stock && list.iter().any(|f| f == flow_name)
        })
        .map(|(name, _)| name)
        .collect();
    names.sort();
    names.first().map(|name| (*name).clone())
}

/// Resolve a flow's two ends from its sketch pipe and the model's stock lists.
///
/// The pipe is the connectors from the flow's valve to stocks and comments,
/// in sketch order (a connector from the valve to any other variable is a
/// causal link drawn from the flow, not a pipe end). An end keeps xmutil's
/// anchor snapping (XMILEGenerator.cpp:1052-1061): a pipe whose two control
/// points share an x is vertical and its ends take their targets' y, otherwise
/// they take their targets' x; the other coordinate is the control point's.
///
/// A pipe end at the stock the model links on a side is that side's end. The
/// remaining pipe ends -- clouds, and stocks that do not list the flow -- serve
/// the sides the model gives no stock. When exactly one side is open and the
/// other side's stock is not a pipe end, the route to that stock continues
/// through the valve, so the open side takes the pipe end on the valve's far
/// side from the stock. When both sides are open (a flow that touches no
/// stock), sketch order decides the source: unverified against Vensim, and it
/// decides only which end of a stockless flow carries the arrowhead.
pub fn resolve_flow_ends(
    valve: Option<&VensimValve>,
    view: &VensimView,
    flow_name: &str,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
) -> FlowEnds {
    use crate::mdl::builtins::to_lower_space;
    use crate::mdl::convert::VariableType;

    struct RawEnd {
        control: (i32, i32),
        anchor: (i32, i32),
        target: PipeTarget,
    }

    let mut raw: Vec<RawEnd> = Vec::new();
    if let Some(valve) = valve {
        for elem in view.iter() {
            let VensimElement::Connector(conn) = elem else {
                continue;
            };
            if conn.from_uid != valve.uid || conn.to_uid == valve.uid {
                continue;
            }
            let (target, anchor) = match view.get(conn.to_uid) {
                Some(VensimElement::Variable(v)) => {
                    let name = to_lower_space(&v.name);
                    let is_stock = symbols
                        .get(&name)
                        .is_some_and(|info| info.var_type == VariableType::Stock);
                    if !is_stock {
                        continue;
                    }
                    (PipeTarget::Stock(name), (v.x, v.y))
                }
                Some(VensimElement::Comment(c)) => (PipeTarget::Cloud(conn.to_uid), (c.x, c.y)),
                _ => continue,
            };
            raw.push(RawEnd {
                control: conn.control_point,
                anchor,
                target,
            });
            if raw.len() == 2 {
                break;
            }
        }
    }

    let vertical = match raw.as_slice() {
        [a, b] => a.control.0 == b.control.0,
        [a] => valve.is_some_and(|v| a.control.0 == v.x),
        _ => false,
    };
    let point_of = |end: &RawEnd| -> (i32, i32) {
        if vertical {
            (end.control.0, end.anchor.1)
        } else {
            (end.anchor.0, end.control.1)
        }
    };
    let pipe_end = |end: &RawEnd| -> FlowEnd {
        let (x, y) = point_of(end);
        FlowEnd::Pipe {
            x,
            y,
            target: end.target.clone(),
        }
    };
    let spare_end = |end: &RawEnd| -> FlowEnd {
        let (x, y) = point_of(end);
        match &end.target {
            PipeTarget::Cloud(_) => pipe_end(end),
            PipeTarget::Stock(_) => FlowEnd::PipeAtUnlinkedStock {
                x,
                y,
                stock: end.anchor,
            },
        }
    };

    let source_stock = model_stock(symbols, flow_name, false);
    let sink_stock = model_stock(symbols, flow_name, true);
    let mut source: Option<FlowEnd> = None;
    let mut sink: Option<FlowEnd> = None;
    let mut spare: Vec<&RawEnd> = Vec::new();
    for end in &raw {
        match &end.target {
            PipeTarget::Stock(name) if sink.is_none() && sink_stock.as_ref() == Some(name) => {
                sink = Some(pipe_end(end));
            }
            PipeTarget::Stock(name) if source.is_none() && source_stock.as_ref() == Some(name) => {
                source = Some(pipe_end(end));
            }
            _ => spare.push(end),
        }
    }

    let source_open = source.is_none() && source_stock.is_none();
    let sink_open = sink.is_none() && sink_stock.is_none();
    if source_open && sink_open {
        let mut spares = spare.iter();
        source = spares.next().map(|end| spare_end(end));
        sink = spares.next().map(|end| spare_end(end));
    } else if source_open || sink_open {
        // The other side's stock, when the pipe does not reach it.
        let unreached = if source_open {
            sink.is_none().then_some(sink_stock.as_deref()).flatten()
        } else {
            source
                .is_none()
                .then_some(source_stock.as_deref())
                .flatten()
        };
        let stock_along = unreached.and_then(|name| {
            view.iter().find_map(|e| match e {
                VensimElement::Variable(v) if to_lower_space(&v.name) == name => {
                    Some(if vertical { v.y } else { v.x })
                }
                _ => None,
            })
        });
        let valve_along = valve.map(|v| if vertical { v.y } else { v.x });
        let far_side = match (stock_along, valve_along) {
            (Some(s), Some(v)) => spare.iter().copied().find(|end| {
                let (x, y) = point_of(end);
                let e = if vertical { y } else { x };
                (e - v).signum() != (s - v).signum()
            }),
            _ => None,
        };
        let chosen = far_side.or_else(|| spare.first().copied()).map(spare_end);
        if source_open {
            source = chosen;
        } else {
            sink = chosen;
        }
    }

    FlowEnds {
        source: source.unwrap_or_else(|| source_stock.map_or(FlowEnd::Free, FlowEnd::Stock)),
        sink: sink.unwrap_or_else(|| sink_stock.map_or(FlowEnd::Free, FlowEnd::Stock)),
    }
}

/// How well a sketch record of a flow can present the flow, higher first: 2
/// for a record carrying the flow's pipe (an attached record with a valve), 1
/// for a label with a connector into a stock that lists the flow, 0 otherwise.
fn flow_copy_rank(
    view: &VensimView,
    var: &VensimVariable,
    flow_to_valve: &HashMap<i32, i32>,
    flow_name: &str,
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
) -> u8 {
    use crate::mdl::builtins::to_lower_space;

    if flow_valve(var, view, flow_to_valve).is_some() {
        return 2;
    }
    let links_its_stock = view.iter().any(|elem| match elem {
        VensimElement::Connector(conn) if conn.from_uid == var.uid => match view.get(conn.to_uid) {
            Some(VensimElement::Variable(target)) => symbols
                .get(&to_lower_space(&target.name))
                .is_some_and(|info| {
                    info.inflows.iter().any(|f| f == flow_name)
                        || info.outflows.iter().any(|f| f == flow_name)
                }),
            _ => false,
        },
        _ => false,
    });
    u8::from(links_its_stock)
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
/// The first two passes are xmutil's algorithm:
/// 1. First pass: Find primaries, mark duplicates as effective ghosts
/// 2. Second pass: Promote first occurrence to primary if variable has no primary
///
/// The third pass decides which copy of a FLOW presents it. The datamodel has
/// one Flow element per flow and it must carry the flow's pipe, but the
/// sketch's primary bit does not follow the pipe: a flow's primary record can
/// be a bare label while its valve and pipe are drawn on a ghost copy in
/// another view (`free 6.mdl`, `C-LEARN v77 for Vensim.mdl`). The best-ranked
/// copy (`flow_copy_rank`) presents the flow, the displaced primary becomes an
/// effective ghost, and a tie keeps the xmutil primary.
pub fn associate_variables(
    views: &[VensimView],
    symbols: &HashMap<String, crate::mdl::convert::SymbolInfo<'_>>,
) -> (PrimaryMap, EffectiveGhosts) {
    use crate::mdl::builtins::to_lower_space;
    use crate::mdl::convert::VariableType;

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

    // Third pass: the copy of each flow that presents it.
    let mut ranks: HashMap<(usize, i32), u8> = HashMap::new();
    let mut best: HashMap<String, (u8, usize, i32)> = HashMap::new();
    for (view_idx, view) in views.iter().enumerate() {
        let (_, flow_to_valve) = build_attached_valve_flow_maps(view);
        for (uid, elem) in view.iter_with_uids() {
            let VensimElement::Variable(var) = elem else {
                continue;
            };
            let canonical = to_lower_space(&var.name);
            if symbols
                .get(&canonical)
                .is_none_or(|info| info.var_type != VariableType::Flow)
            {
                continue;
            }
            let rank = flow_copy_rank(view, var, &flow_to_valve, &canonical, symbols);
            ranks.insert((view_idx, uid), rank);
            if best.get(&canonical).is_none_or(|(r, _, _)| rank > *r) {
                best.insert(canonical, (rank, view_idx, uid));
            }
        }
    }
    for (canonical, (rank, view_idx, uid)) in best {
        let Some(&primary) = primary_map.get(&canonical) else {
            continue;
        };
        if primary == (view_idx, uid) || rank <= ranks.get(&primary).copied().unwrap_or(0) {
            continue;
        }
        effective_ghosts.insert(primary);
        effective_ghosts.remove(&(view_idx, uid));
        primary_map.insert(canonical, (view_idx, uid));
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
            font: None,
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
                bits: 3,
                shape: 0,
                tail: String::new(),
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
            font: None,
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
                bits: 3,
                shape: 0,
                tail: String::new(),
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
                bits: 2,
                shape: 0,
                tail: String::new(),
            }),
        );

        let (primary_map, effective_ghosts) = associate_variables(&[view], &HashMap::new());

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

    // Sketch fixtures for `resolve_flow_ends` and the flow copy ranking. They
    // build `VensimView`s directly because the functions under test take the
    // parsed sketch; what the importer produces end to end is pinned through
    // `open_vensim` on corpus files (`convert::flow_resolution_tests`).
    mod flow_ends {
        use super::super::super::types::{
            VensimComment, VensimConnector, VensimValve, VensimVariable, ViewHeader, ViewVersion,
        };
        use super::*;
        use crate::mdl::convert::{SymbolInfo, VariableType};

        fn variable(
            uid: i32,
            name: &str,
            x: i32,
            y: i32,
            attached: bool,
            ghost: bool,
        ) -> VensimElement {
            VensimElement::Variable(VensimVariable {
                uid,
                name: name.to_string(),
                x,
                y,
                width: 40,
                height: 20,
                attached,
                is_ghost: ghost,
                bits: if ghost { 2 } else { 3 },
                shape: if attached { 32 } else { 0 },
                tail: String::new(),
            })
        }

        fn valve(uid: i32, x: i32, y: i32) -> VensimElement {
            VensimElement::Valve(VensimValve {
                uid,
                name: "444".to_string(),
                x,
                y,
                width: 6,
                height: 8,
                attached: true,
                bits: 3,
                shape: 34,
                tail: String::new(),
            })
        }

        fn cloud(uid: i32, x: i32, y: i32) -> VensimElement {
            VensimElement::Comment(VensimComment {
                uid,
                text: "48".to_string(),
                x,
                y,
                width: 10,
                height: 8,
                scratch_name: false,
                bits: 3,
                shape: 0,
                tail: String::new(),
            })
        }

        fn connector(uid: i32, from: i32, to: i32, control: (i32, i32)) -> VensimElement {
            VensimElement::Connector(VensimConnector {
                uid,
                from_uid: from,
                to_uid: to,
                field4: 0,
                polarity: None,
                letter_polarity: false,
                control_point: control,
                field10: 0,
            })
        }

        fn view_of(elements: Vec<VensimElement>) -> VensimView {
            let mut view = VensimView::new(ViewHeader {
                version: ViewVersion::V300,
                title: "t".to_string(),
                font: None,
            });
            for e in elements {
                view.insert(e.uid(), e);
            }
            view
        }

        fn stock(inflows: &[&str], outflows: &[&str]) -> SymbolInfo<'static> {
            make_symbol_info(
                VariableType::Stock,
                inflows.iter().map(|s| s.to_string()).collect(),
                outflows.iter().map(|s| s.to_string()).collect(),
            )
        }

        fn symbols(
            entries: Vec<(&str, SymbolInfo<'static>)>,
        ) -> HashMap<String, SymbolInfo<'static>> {
            entries
                .into_iter()
                .map(|(n, s)| (n.to_string(), s))
                .collect()
        }

        fn ends_of(view: &VensimView, symbols: &HashMap<String, SymbolInfo<'static>>) -> FlowEnds {
            let (_, flow_to_valve) = build_attached_valve_flow_maps(view);
            let VensimElement::Variable(flow) = view.get(4).unwrap() else {
                unreachable!()
            };
            resolve_flow_ends(
                flow_valve(flow, view, &flow_to_valve),
                view,
                "flow rate",
                symbols,
            )
        }

        /// A pipe between two stocks that list the flow: each end attaches to
        /// its stock, with xmutil's anchor snapping on both axes.
        #[test]
        fn pipe_ends_at_linked_stocks_attach_to_them() {
            let symbols = symbols(vec![
                ("stock a", stock(&[], &["flow rate"])),
                ("stock b", stock(&["flow rate"], &[])),
            ]);
            // Horizontal: control points differ in x, so x takes the anchors.
            let view = view_of(vec![
                variable(1, "Stock A", 50, 100, false, false),
                variable(2, "Stock B", 250, 100, false, false),
                valve(3, 150, 100),
                variable(4, "Flow Rate", 150, 120, true, false),
                connector(5, 3, 1, (100, 100)),
                connector(6, 3, 2, (200, 100)),
            ]);
            let ends = ends_of(&view, &symbols);
            assert_eq!(
                ends.source,
                FlowEnd::Pipe {
                    x: 50,
                    y: 100,
                    target: PipeTarget::Stock("stock a".to_string())
                }
            );
            assert_eq!(
                ends.sink,
                FlowEnd::Pipe {
                    x: 250,
                    y: 100,
                    target: PipeTarget::Stock("stock b".to_string())
                }
            );

            // Vertical: control points share an x, so y takes the anchors.
            let view = view_of(vec![
                variable(1, "Stock A", 100, 50, false, false),
                variable(2, "Stock B", 100, 250, false, false),
                valve(3, 100, 150),
                variable(4, "Flow Rate", 120, 150, true, false),
                connector(5, 3, 1, (100, 100)),
                connector(6, 3, 2, (100, 200)),
            ]);
            let ends = ends_of(&view, &symbols);
            assert_eq!(
                ends.source,
                FlowEnd::Pipe {
                    x: 100,
                    y: 50,
                    target: PipeTarget::Stock("stock a".to_string())
                }
            );
            assert_eq!(
                ends.sink,
                FlowEnd::Pipe {
                    x: 100,
                    y: 250,
                    target: PipeTarget::Stock("stock b".to_string())
                }
            );
        }

        /// A cloud serves the side the model gives no stock; a pipe end at a
        /// stock that does not list the flow does too, as a cloud clear of that
        /// stock.
        #[test]
        fn clouds_and_unlinked_stocks_serve_the_open_side() {
            let view = view_of(vec![
                cloud(1, 50, 100),
                variable(2, "Stock B", 250, 100, false, false),
                valve(3, 150, 100),
                variable(4, "Flow Rate", 150, 120, true, false),
                connector(5, 3, 1, (100, 100)),
                connector(6, 3, 2, (200, 100)),
            ]);
            let linked = symbols(vec![("stock b", stock(&["flow rate"], &[]))]);
            let ends = ends_of(&view, &linked);
            assert_eq!(
                ends.source,
                FlowEnd::Pipe {
                    x: 50,
                    y: 100,
                    target: PipeTarget::Cloud(1)
                }
            );
            assert_eq!(
                ends.sink,
                FlowEnd::Pipe {
                    x: 250,
                    y: 100,
                    target: PipeTarget::Stock("stock b".to_string())
                }
            );

            let view = view_of(vec![
                variable(1, "Stock A", 50, 100, false, false),
                variable(2, "Stock B", 250, 100, false, false),
                valve(3, 150, 100),
                variable(4, "Flow Rate", 150, 120, true, false),
                connector(5, 3, 1, (100, 100)),
                connector(6, 3, 2, (200, 100)),
            ]);
            let unlinked_sink = symbols(vec![
                ("stock a", stock(&[], &["flow rate"])),
                ("stock b", stock(&["stock b net flow"], &[])),
            ]);
            let ends = ends_of(&view, &unlinked_sink);
            assert_eq!(
                ends.source,
                FlowEnd::Pipe {
                    x: 50,
                    y: 100,
                    target: PipeTarget::Stock("stock a".to_string())
                }
            );
            assert_eq!(
                ends.sink,
                FlowEnd::PipeAtUnlinkedStock {
                    x: 250,
                    y: 100,
                    stock: (250, 100)
                }
            );
        }

        /// A flow drawn as a bare label has no pipe: the linked side names its
        /// stock and the other side is free. A connector from a valve to a
        /// variable that is not a stock is a causal link, not a pipe end.
        #[test]
        fn a_side_without_a_pipe_end_names_its_stock_or_is_free() {
            let symbols = symbols(vec![
                ("stock a", stock(&["flow rate"], &[])),
                (
                    "helper",
                    make_symbol_info(VariableType::Aux, vec![], vec![]),
                ),
            ]);
            let label_only = view_of(vec![
                variable(1, "Stock A", 250, 100, false, false),
                variable(4, "Flow Rate", 150, 120, false, false),
                connector(5, 4, 1, (0, 0)),
            ]);
            let ends = ends_of(&label_only, &symbols);
            assert_eq!(ends.source, FlowEnd::Free);
            assert_eq!(ends.sink, FlowEnd::Stock("stock a".to_string()));

            let causal_link_only = view_of(vec![
                variable(1, "Stock A", 250, 100, false, false),
                variable(2, "Helper", 150, 60, false, false),
                valve(3, 150, 100),
                variable(4, "Flow Rate", 150, 120, true, false),
                connector(5, 3, 2, (0, 0)),
            ]);
            let ends = ends_of(&causal_link_only, &symbols);
            assert_eq!(ends.source, FlowEnd::Free);
            assert_eq!(ends.sink, FlowEnd::Stock("stock a".to_string()));
        }

        /// One open side while the other side's stock is not a pipe end: the
        /// open side takes the pipe end on the valve's far side from that
        /// stock (whatever the sketch order), because the route to the stock
        /// continues through the valve.
        #[test]
        fn the_open_side_takes_the_pipe_end_away_from_an_unreached_stock() {
            let symbols = symbols(vec![
                ("stock b", stock(&["stock b net flow"], &[])),
                ("stock c", stock(&["flow rate"], &[])),
            ]);
            for (cloud_x, stock_b_x) in [(200, 100), (100, 200)] {
                let view = view_of(vec![
                    cloud(1, cloud_x, 100),
                    variable(2, "Stock B", stock_b_x, 100, false, false),
                    valve(3, 150, 100),
                    variable(4, "Flow Rate", 150, 120, true, false),
                    connector(5, 3, 1, (cloud_x, 100)),
                    connector(6, 3, 2, (stock_b_x, 100)),
                    variable(7, "Stock C", 400, 100, false, false),
                ]);
                let ends = ends_of(&view, &symbols);
                // Stock C is right of the valve, so the source is the pipe end
                // on the left.
                let expected = if cloud_x < 150 {
                    FlowEnd::Pipe {
                        x: 100,
                        y: 100,
                        target: PipeTarget::Cloud(1),
                    }
                } else {
                    FlowEnd::PipeAtUnlinkedStock {
                        x: 100,
                        y: 100,
                        stock: (100, 100),
                    }
                };
                assert_eq!(ends.source, expected, "cloud at x={cloud_x}");
                assert_eq!(ends.sink, FlowEnd::Stock("stock c".to_string()));
            }
        }

        /// The copy of a flow that carries its pipe presents the flow, even
        /// when the sketch marks it a ghost and a label copy primary; with no
        /// pipe anywhere, a label with an arrow into the flow's stock wins over
        /// one without.
        #[test]
        fn the_pipe_carrying_copy_presents_a_flow() {
            let symbols = symbols(vec![
                ("stock a", stock(&["flow rate"], &[])),
                (
                    "flow rate",
                    make_symbol_info(VariableType::Flow, vec![], vec![]),
                ),
            ]);
            let view = view_of(vec![
                variable(1, "Flow Rate", 20, 20, false, false),
                valve(2, 150, 100),
                variable(3, "Flow Rate", 150, 120, true, true),
                variable(4, "Stock A", 250, 100, false, false),
                connector(5, 2, 4, (200, 100)),
            ]);
            let (primary, ghosts) = associate_variables(&[view], &symbols);
            assert_eq!(primary.get("flow rate"), Some(&(0, 3)));
            assert!(ghosts.contains(&(0, 1)));
            assert!(!ghosts.contains(&(0, 3)));

            let view = view_of(vec![
                variable(1, "Flow Rate", 20, 20, false, false),
                variable(3, "Flow Rate", 250, 60, false, true),
                variable(4, "Stock A", 250, 100, false, false),
                connector(5, 3, 4, (0, 0)),
            ]);
            let (primary, ghosts) = associate_variables(&[view], &symbols);
            assert_eq!(primary.get("flow rate"), Some(&(0, 3)));
            assert!(ghosts.contains(&(0, 1)));
        }
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
            font: None,
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
                bits: 2,
                shape: 0,
                tail: String::new(),
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
                bits: 2,
                shape: 0,
                tail: String::new(),
            }),
        );

        let (primary_map, effective_ghosts) = associate_variables(&[view], &HashMap::new());

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
