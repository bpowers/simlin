// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The annealing OBJECTIVE: the cost terms the crossing-reduction search
//! minimizes beyond raw segment crossings. The search will exploit any layout
//! defect its cost cannot see (it once collapsed two auxes onto the same spot
//! to remove a chord crossing), so every defect it could create while moving
//! nodes -- pile-ups, label intrusion -- must be charged here.

use std::collections::{HashMap, HashSet};

use super::LayoutState;
use super::graph::{Layout, Position};
use crate::datamodel::view_element::LabelSide;

/// Minimum center-to-center distance between two point nodes (auxes/modules)
/// before the annealing's cost charges them as piled up. Two auxes need at
/// least their shapes (2 x AUX_RADIUS = 18) plus label breathing room apart;
/// half a lane (50) is the same floor `MIN_AUX_LANE_OFFSET` builds on.
pub(super) const MIN_POINT_NODE_SEPARATION: f64 = 50.0;

/// The number of point-node pairs in `layout` that sit closer than
/// `MIN_POINT_NODE_SEPARATION`. Added to the annealing cost so the
/// crossing-reduction pass cannot "fix" a crossing by piling nodes on top of
/// each other -- crossings and pile-ups are both unreadable, and a
/// crossings-only objective is blind to the latter (it once collapsed two
/// auxes onto the same spot to remove a chord crossing).
pub(super) fn point_node_pileup_count(
    layout: &Layout<String>,
    point_node_ids: &HashSet<String>,
) -> usize {
    let positions: Vec<Position> = point_node_ids
        .iter()
        .filter_map(|node_id| layout.get(node_id).copied())
        .collect();
    let mut count = 0;
    for i in 0..positions.len() {
        for j in (i + 1)..positions.len() {
            let d = positions[i] - positions[j];
            if d.length() < MIN_POINT_NODE_SEPARATION {
                count += 1;
            }
        }
    }
    count
}

/// Per-node footprints for the annealing's overlap penalty: each point node's
/// shape box unioned with its label box at the default Bottom side, expressed
/// RELATIVE to the node center. Labels are measured exactly as the
/// layout-quality metric measures them (`diagram::label::label_bounds` over
/// the diagram display name), so what the penalty calls "overlapping" is what
/// the final score charges -- the same lesson as `park_isolated_nodes`: the
/// layout-internal Praxis text estimate disagrees on long names.
pub(super) fn point_node_footprints(
    point_idents: &HashSet<String>,
    var_to_node: &HashMap<String, String>,
    state: &LayoutState,
) -> HashMap<String, crate::diagram::common::Rect> {
    use crate::diagram::constants::AUX_RADIUS;
    use crate::diagram::label::{LabelProps, label_bounds};

    let mut footprints = HashMap::new();
    for ident in point_idents {
        let Some(node_id) = var_to_node.get(ident) else {
            continue;
        };
        let elem_name = super::format_label_with_line_breaks(&state.display_name(ident));
        let metric_text = crate::diagram::common::display_name(&elem_name);
        let props = LabelProps::new(0.0, 0.0, LabelSide::Bottom, metric_text)
            .with_radii(AUX_RADIUS, AUX_RADIUS);
        let label_rect = label_bounds(&props);
        footprints.insert(
            node_id.clone(),
            crate::diagram::common::Rect {
                left: label_rect.left.min(-AUX_RADIUS),
                right: label_rect.right.max(AUX_RADIUS),
                top: label_rect.top.min(-AUX_RADIUS),
                bottom: label_rect.bottom.max(AUX_RADIUS),
            },
        );
    }
    footprints
}

/// Sum of pairwise footprint-overlap fractions over the point nodes present in
/// `layout`: each overlapping pair contributes `overlap_area / min(area_a,
/// area_b)`, so a fully-stacked pair costs 1.0 -- the same as one crossing --
/// and partial label intrusion costs proportionally less. This is the
/// continuous, label-aware generalization of `point_node_pileup_count`: it
/// charges what the metric's `label_overlap`/`node_overlap` terms will charge,
/// and (unlike a count) it gives the search a gradient to follow.
///
/// Iterates in SORTED node-id order: float summation is order-dependent, and
/// `footprints` is a HashMap whose iteration order is per-process random --
/// unsorted accumulation would break per-seed layout determinism (#633).
pub(super) fn point_node_footprint_overlap(
    layout: &Layout<String>,
    footprints: &HashMap<String, crate::diagram::common::Rect>,
) -> f64 {
    use crate::diagram::common::{Rect as GeomRect, rect_area, rect_overlap_area};

    let mut nodes: Vec<(&String, &GeomRect)> = footprints
        .iter()
        .filter(|(id, _)| layout.contains_key(*id))
        .collect();
    nodes.sort_by(|a, b| a.0.cmp(b.0));

    let abs_rect = |rel: &GeomRect, pos: Position| GeomRect {
        left: rel.left + pos.x,
        right: rel.right + pos.x,
        top: rel.top + pos.y,
        bottom: rel.bottom + pos.y,
    };

    let mut total = 0.0;
    for i in 0..nodes.len() {
        let pos_i = layout[nodes[i].0];
        let rect_i = abs_rect(nodes[i].1, pos_i);
        let area_i = rect_area(&rect_i);
        for j in (i + 1)..nodes.len() {
            let pos_j = layout[nodes[j].0];
            let rect_j = abs_rect(nodes[j].1, pos_j);
            let overlap = rect_overlap_area(&rect_i, &rect_j);
            if overlap > 0.0 {
                let min_area = area_i.min(rect_area(&rect_j)).max(1e-9);
                total += overlap / min_area;
            }
        }
    }
    total
}
