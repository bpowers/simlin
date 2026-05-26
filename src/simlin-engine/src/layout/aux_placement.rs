// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::f64::consts::PI;

use super::graph::{Layout, Position};
use super::metadata;

const AUX_ANCHOR_TRACE_DEPTH: usize = 4;
pub(super) const MIN_AUX_LANE_OFFSET: f64 = 56.0;
const AUX_SIBLING_SPACING: f64 = 70.0;

#[derive(Clone, Debug)]
pub(super) struct AuxiliaryInitialPosition {
    pub(super) position: Position,
    base: Position,
    anchor_key: Vec<String>,
    axis: Position,
    side: Position,
}

pub(super) struct AuxiliaryPlacementContext<'a> {
    dep_graph: &'a BTreeMap<String, BTreeSet<String>>,
    reverse_dep_graph: &'a BTreeMap<String, BTreeSet<String>>,
    flow_to_stocks: &'a HashMap<String, (Option<String>, Option<String>)>,
    feedback_loops: &'a [metadata::FeedbackLoop],
}

impl<'a> AuxiliaryPlacementContext<'a> {
    pub(super) fn new(
        dep_graph: &'a BTreeMap<String, BTreeSet<String>>,
        reverse_dep_graph: &'a BTreeMap<String, BTreeSet<String>>,
        flow_to_stocks: &'a HashMap<String, (Option<String>, Option<String>)>,
        feedback_loops: &'a [metadata::FeedbackLoop],
    ) -> Self {
        Self {
            dep_graph,
            reverse_dep_graph,
            flow_to_stocks,
            feedback_loops,
        }
    }
}

/// Centroid of a non-empty set of positions.
fn centroid(positions: &[Position]) -> Position {
    let n = positions.len() as f64;
    let sum_x: f64 = positions.iter().map(|p| p.x).sum();
    let sum_y: f64 = positions.iter().map(|p| p.y).sum();
    Position::new(sum_x / n, sum_y / n)
}

fn scale_position(v: Position, s: f64) -> Position {
    Position::new(v.x * s, v.y * s)
}

fn normalize_or(v: Position, fallback: Position) -> Position {
    let len = v.length();
    if len > 1e-9 {
        scale_position(v, 1.0 / len)
    } else {
        fallback
    }
}

fn weighted_centroid(anchors: &[(String, Position, usize)]) -> Option<Position> {
    if anchors.is_empty() {
        return None;
    }

    let mut sum = Position::default();
    let mut weight_sum = 0.0;
    for (_, pos, depth) in anchors {
        let weight = 1.0 / (*depth).max(1) as f64;
        sum.x += pos.x * weight;
        sum.y += pos.y * weight;
        weight_sum += weight;
    }

    if weight_sum > 0.0 {
        Some(Position::new(sum.x / weight_sum, sum.y / weight_sum))
    } else {
        None
    }
}

fn collect_positioned_anchors(
    root: &str,
    graph: &BTreeMap<String, BTreeSet<String>>,
    positioned: &HashMap<String, Position>,
) -> Vec<(String, Position, usize)> {
    let mut anchors = Vec::new();
    let mut visited = BTreeSet::new();
    let mut queue: VecDeque<(String, usize)> = graph
        .get(root)
        .into_iter()
        .flat_map(|neighbors| neighbors.iter())
        .filter(|neighbor| neighbor.as_str() != root)
        .map(|neighbor| (neighbor.clone(), 1))
        .collect();

    while let Some((ident, depth)) = queue.pop_front() {
        if ident == root || !visited.insert(ident.clone()) {
            continue;
        }

        if let Some(&pos) = positioned.get(&ident) {
            anchors.push((ident, pos, depth));
            continue;
        }

        if depth >= AUX_ANCHOR_TRACE_DEPTH {
            continue;
        }

        if let Some(neighbors) = graph.get(&ident) {
            for neighbor in neighbors {
                if neighbor != root {
                    queue.push_back((neighbor.clone(), depth + 1));
                }
            }
        }
    }

    anchors
}

fn collect_direct_positioned_anchors(
    root: &str,
    graph: &BTreeMap<String, BTreeSet<String>>,
    positioned: &HashMap<String, Position>,
) -> Vec<(String, Position, usize)> {
    graph
        .get(root)
        .into_iter()
        .flat_map(|neighbors| neighbors.iter())
        .filter(|neighbor| neighbor.as_str() != root)
        .filter_map(|neighbor| {
            positioned
                .get(neighbor)
                .copied()
                .map(|pos| (neighbor.clone(), pos, 1))
        })
        .collect()
}

fn local_axis_for_anchor(
    anchor: &str,
    exclude: &str,
    positioned: &HashMap<String, Position>,
    dep_graph: &BTreeMap<String, BTreeSet<String>>,
    reverse_dep_graph: &BTreeMap<String, BTreeSet<String>>,
) -> Option<Position> {
    let anchor_pos = *positioned.get(anchor)?;
    let mut neighbors = Vec::new();

    for graph in [dep_graph, reverse_dep_graph] {
        if let Some(idents) = graph.get(anchor) {
            for ident in idents {
                if ident == exclude {
                    continue;
                }
                if let Some(&pos) = positioned.get(ident) {
                    neighbors.push(pos);
                }
            }
        }
    }

    if neighbors.is_empty() {
        return None;
    }

    let center = centroid(&neighbors);
    let axis = anchor_pos - center;
    if axis.length() > 1e-9 {
        Some(axis)
    } else {
        None
    }
}

fn structural_flow_axis_for_anchor(
    anchor: &str,
    positioned: &HashMap<String, Position>,
    flow_to_stocks: &HashMap<String, (Option<String>, Option<String>)>,
) -> Option<Position> {
    let (from_stock, to_stock) = flow_to_stocks.get(anchor)?;
    let flow_pos = positioned.get(anchor).copied();

    let axis = match (from_stock.as_ref(), to_stock.as_ref()) {
        (Some(from), Some(to)) => {
            let from_pos = positioned.get(from)?;
            let to_pos = positioned.get(to)?;
            *to_pos - *from_pos
        }
        (Some(from), None) => flow_pos? - *positioned.get(from)?,
        (None, Some(to)) => *positioned.get(to)? - flow_pos?,
        (None, None) => return None,
    };

    (axis.length() > 1e-9).then_some(axis)
}

fn loop_outward_unit(
    ident: &str,
    positioned: &HashMap<String, Position>,
    feedback_loops: &[metadata::FeedbackLoop],
) -> Option<Position> {
    for loop_info in feedback_loops {
        let chain = loop_info.causal_chain();
        let Some(idx) = chain.iter().position(|v| v == ident) else {
            continue;
        };

        let member_positions: Vec<Position> = chain
            .iter()
            .filter(|member| member.as_str() != ident)
            .filter_map(|member| positioned.get(member).copied())
            .collect();
        if member_positions.is_empty() {
            continue;
        }
        let center = centroid(&member_positions);

        if let Some(&pos) = positioned.get(ident) {
            let outward = pos - center;
            if outward.length() > 1e-9 {
                return Some(normalize_or(outward, Position::new(0.0, -1.0)));
            }
        }

        let prev = idx
            .checked_sub(1)
            .and_then(|i| chain.get(i))
            .and_then(|v| positioned.get(v));
        let next = chain.get(idx + 1).and_then(|v| positioned.get(v));
        if let (Some(prev), Some(next)) = (prev, next) {
            let midpoint = Position::new((prev.x + next.x) / 2.0, (prev.y + next.y) / 2.0);
            let outward = midpoint - center;
            if outward.length() > 1e-9 {
                return Some(normalize_or(outward, Position::new(0.0, -1.0)));
            }
        }

        let unique_len = if chain.first() == chain.last() && chain.len() > 1 {
            chain.len() - 1
        } else {
            chain.len()
        };
        if unique_len > 0 {
            let angle = idx as f64 * 2.0 * PI / unique_len as f64;
            return Some(Position::new(angle.cos(), angle.sin()));
        }
    }

    None
}

fn preferred_auxiliary_side(
    ident: &str,
    axis: Position,
    positioned: &HashMap<String, Position>,
    feedback_loops: &[metadata::FeedbackLoop],
) -> Position {
    if let Some(outward) = loop_outward_unit(ident, positioned, feedback_loops) {
        return outward;
    }

    chain_side_for_axis(axis)
}

fn chain_side_for_axis(axis: Position) -> Position {
    let axis = normalize_or(axis, Position::new(1.0, 0.0));
    let mut side = normalize_or(Position::new(-axis.y, axis.x), Position::new(0.0, -1.0));

    // SVG coordinates grow downward. For the common left-to-right stock-flow
    // chain, put causal auxiliaries above the pipe.
    if side.y > 0.0 || (side.y.abs() < 1e-9 && side.x < 0.0) {
        side = scale_position(side, -1.0);
    }
    side
}

fn anchor_key_for(
    upstream: &[(String, Position, usize)],
    downstream: &[(String, Position, usize)],
    fallback: &str,
) -> Vec<String> {
    let mut key: Vec<String> = upstream
        .iter()
        .chain(downstream.iter())
        .map(|(ident, _, _)| ident.clone())
        .collect();
    key.sort();
    key.dedup();
    if key.is_empty() {
        key.push(fallback.to_string());
    }
    key
}

fn axis_for_auxiliary(
    ident: &str,
    upstream: &[(String, Position, usize)],
    downstream: &[(String, Position, usize)],
    positioned: &HashMap<String, Position>,
    dep_graph: &BTreeMap<String, BTreeSet<String>>,
    reverse_dep_graph: &BTreeMap<String, BTreeSet<String>>,
    flow_to_stocks: &HashMap<String, (Option<String>, Option<String>)>,
) -> Position {
    if upstream.is_empty()
        && downstream.len() == 1
        && let Some(axis) =
            structural_flow_axis_for_anchor(&downstream[0].0, positioned, flow_to_stocks)
    {
        return axis;
    }

    if let (Some(up), Some(down)) = (weighted_centroid(upstream), weighted_centroid(downstream)) {
        let axis = down - up;
        if axis.length() > 1e-9 {
            return axis;
        }
    }

    for (anchor, _, _) in downstream.iter().chain(upstream.iter()) {
        if let Some(axis) =
            local_axis_for_anchor(anchor, ident, positioned, dep_graph, reverse_dep_graph)
        {
            return axis;
        }
    }

    Position::new(1.0, 0.0)
}

pub(super) fn auxiliary_initial_position(
    ident: &str,
    positioned: &HashMap<String, Position>,
    ctx: &AuxiliaryPlacementContext<'_>,
    global_center: Position,
    fallback_index: usize,
) -> Option<AuxiliaryInitialPosition> {
    let upstream = collect_positioned_anchors(ident, ctx.dep_graph, positioned);
    let downstream = collect_positioned_anchors(ident, ctx.reverse_dep_graph, positioned);
    let direct_upstream = collect_direct_positioned_anchors(ident, ctx.dep_graph, positioned);
    let direct_downstream =
        collect_direct_positioned_anchors(ident, ctx.reverse_dep_graph, positioned);
    let single_flow_input = direct_upstream.is_empty()
        && direct_downstream.len() == 1
        && ctx.flow_to_stocks.contains_key(&direct_downstream[0].0);
    let axis = axis_for_auxiliary(
        ident,
        if single_flow_input {
            &direct_upstream
        } else {
            &upstream
        },
        if single_flow_input {
            &direct_downstream
        } else {
            &downstream
        },
        positioned,
        ctx.dep_graph,
        ctx.reverse_dep_graph,
        ctx.flow_to_stocks,
    );
    let side = if single_flow_input {
        chain_side_for_axis(axis)
    } else {
        preferred_auxiliary_side(ident, axis, positioned, ctx.feedback_loops)
    };

    let upstream_for_base = if single_flow_input {
        &direct_upstream
    } else {
        &upstream
    };
    let downstream_for_base = if single_flow_input {
        &direct_downstream
    } else {
        &downstream
    };
    let base = match (
        weighted_centroid(upstream_for_base),
        weighted_centroid(downstream_for_base),
    ) {
        (Some(up), Some(down)) => Position::new((up.x + down.x) / 2.0, (up.y + down.y) / 2.0),
        (Some(up), None) => up,
        (None, Some(down)) => down,
        (None, None) => {
            let angle = fallback_index as f64 * 2.0 * PI / 8.0;
            let position = Position::new(
                global_center.x + 120.0 * angle.cos(),
                global_center.y + 120.0 * angle.sin(),
            );
            return Some(AuxiliaryInitialPosition {
                position,
                base: global_center,
                anchor_key: vec![format!("disconnected:{fallback_index}")],
                axis,
                side,
            });
        }
    };

    let position = base + scale_position(side, MIN_AUX_LANE_OFFSET);
    Some(AuxiliaryInitialPosition {
        position,
        base,
        anchor_key: anchor_key_for(upstream_for_base, downstream_for_base, ident),
        axis,
        side,
    })
}

pub(super) fn positioned_variables_from_layout(
    var_to_node: &HashMap<String, String>,
    layout: &Layout<String>,
) -> HashMap<String, Position> {
    var_to_node
        .iter()
        .filter_map(|(ident, node_id)| layout.get(node_id).map(|&pos| (ident.clone(), pos)))
        .collect()
}

pub(super) fn spread_auxiliary_initial_positions(
    proposals: Vec<(String, String, AuxiliaryInitialPosition)>,
) -> Vec<(String, Position)> {
    let mut by_key: BTreeMap<Vec<String>, Vec<(String, String, AuxiliaryInitialPosition)>> =
        BTreeMap::new();

    for proposal in proposals {
        by_key
            .entry(proposal.2.anchor_key.clone())
            .or_default()
            .push(proposal);
    }

    let mut positioned = Vec::new();
    for group in by_key.values_mut() {
        group.sort_by(|a, b| a.0.cmp(&b.0));
        let count = group.len();
        let mid = (count.saturating_sub(1)) as f64 / 2.0;

        for (idx, (_ident, node_id, proposal)) in group.iter().enumerate() {
            let mut pos = proposal.position;
            if count > 1 {
                let lateral = normalize_or(proposal.axis, Position::new(1.0, 0.0));
                let lane = if idx % 2 == 0 { 1.0 } else { -1.0 };
                let ring = (idx / 2) as f64;
                let side_offset = lane * MIN_AUX_LANE_OFFSET * (1.0 + 0.45 * ring);
                let lateral_offset = (ring - mid / 2.0) * AUX_SIBLING_SPACING;
                pos = proposal.base
                    + scale_position(proposal.side, side_offset)
                    + scale_position(lateral, lateral_offset);
            }
            positioned.push((node_id.clone(), pos));
        }
    }

    positioned.sort_by(|a, b| a.0.cmp(&b.0));
    positioned
}

pub(super) fn enforce_auxiliary_lane_clearance(
    layout: &mut Layout<String>,
    var_to_node: &HashMap<String, String>,
    ctx: &AuxiliaryPlacementContext<'_>,
    point_idents: &HashSet<String>,
    fan_direct_flow_inputs: bool,
) {
    let positioned = positioned_variables_from_layout(var_to_node, layout);
    let mut sorted_idents: Vec<&String> = point_idents.iter().collect();
    sorted_idents.sort();

    for ident in &sorted_idents {
        let ident = ident.as_str();
        let Some(node_id) = var_to_node.get(ident) else {
            continue;
        };
        let Some(&pos) = layout.get(node_id) else {
            continue;
        };

        let upstream = collect_positioned_anchors(ident, ctx.dep_graph, &positioned);
        let downstream = collect_positioned_anchors(ident, ctx.reverse_dep_graph, &positioned);
        let direct_upstream = collect_direct_positioned_anchors(ident, ctx.dep_graph, &positioned);
        let direct_downstream =
            collect_direct_positioned_anchors(ident, ctx.reverse_dep_graph, &positioned);
        let single_flow_input = direct_upstream.is_empty()
            && direct_downstream.len() == 1
            && ctx.flow_to_stocks.contains_key(&direct_downstream[0].0);
        if upstream.is_empty() && downstream.is_empty() {
            continue;
        }
        if !single_flow_input && anchor_key_for(&upstream, &downstream, ident).len() > 2 {
            continue;
        }

        let axis = axis_for_auxiliary(
            ident,
            if single_flow_input {
                &direct_upstream
            } else {
                &upstream
            },
            if single_flow_input {
                &direct_downstream
            } else {
                &downstream
            },
            &positioned,
            ctx.dep_graph,
            ctx.reverse_dep_graph,
            ctx.flow_to_stocks,
        );
        let side = if single_flow_input {
            chain_side_for_axis(axis)
        } else {
            preferred_auxiliary_side(ident, axis, &positioned, ctx.feedback_loops)
        };
        let upstream_for_base = if single_flow_input {
            &direct_upstream
        } else {
            &upstream
        };
        let downstream_for_base = if single_flow_input {
            &direct_downstream
        } else {
            &downstream
        };
        let Some(base) = (match (
            weighted_centroid(upstream_for_base),
            weighted_centroid(downstream_for_base),
        ) {
            (Some(up), Some(down)) => {
                Some(Position::new((up.x + down.x) / 2.0, (up.y + down.y) / 2.0))
            }
            (Some(up), None) => Some(up),
            (None, Some(down)) => Some(down),
            (None, None) => None,
        }) else {
            continue;
        };

        let mut adjusted = pos;
        let mut changed = false;

        let offset = (adjusted - base).dot(side);
        if offset < MIN_AUX_LANE_OFFSET {
            adjusted = adjusted + scale_position(side, MIN_AUX_LANE_OFFSET - offset);
            changed = true;
        }

        if changed {
            layout.insert(node_id.clone(), adjusted);
        }
    }

    if !fan_direct_flow_inputs {
        return;
    }

    let positioned_after = positioned_variables_from_layout(var_to_node, layout);
    let mut direct_flow_inputs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for ident in &sorted_idents {
        let ident = ident.as_str();
        let direct_downstream =
            collect_direct_positioned_anchors(ident, ctx.reverse_dep_graph, &positioned_after);
        let direct_flow_downstream: Vec<&String> = direct_downstream
            .iter()
            .map(|(flow_ident, _, _)| flow_ident)
            .filter(|flow_ident| ctx.flow_to_stocks.contains_key(*flow_ident))
            .collect();
        if direct_flow_downstream.len() == 1 {
            direct_flow_inputs
                .entry(direct_flow_downstream[0].clone())
                .or_default()
                .push(ident.to_string());
        }
    }

    for (flow_ident, mut idents) in direct_flow_inputs {
        if idents.len() < 2 {
            continue;
        }

        let Some(&flow_pos) = positioned_after.get(&flow_ident) else {
            continue;
        };
        let axis =
            structural_flow_axis_for_anchor(&flow_ident, &positioned_after, ctx.flow_to_stocks)
                .unwrap_or_else(|| Position::new(1.0, 0.0));
        let side = chain_side_for_axis(axis);
        let lateral = normalize_or(axis, Position::new(1.0, 0.0));

        idents.sort_by(|a, b| {
            let a_has_upstream =
                !collect_direct_positioned_anchors(a, ctx.dep_graph, &positioned_after).is_empty();
            let b_has_upstream =
                !collect_direct_positioned_anchors(b, ctx.dep_graph, &positioned_after).is_empty();
            a_has_upstream.cmp(&b_has_upstream).then_with(|| a.cmp(b))
        });

        let mid = (idents.len().saturating_sub(1)) as f64 / 2.0;
        let spacing = AUX_SIBLING_SPACING * 1.25;
        for (idx, ident) in idents.iter().enumerate() {
            let Some(node_id) = var_to_node.get(ident) else {
                continue;
            };
            let lateral_offset = (idx as f64 - mid) * spacing;
            let pos = flow_pos
                + scale_position(side, MIN_AUX_LANE_OFFSET)
                + scale_position(lateral, lateral_offset);
            layout.insert(node_id.clone(), pos);
        }
    }
}
