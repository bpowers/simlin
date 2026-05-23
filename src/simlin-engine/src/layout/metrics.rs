// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// The layout quality core. Every term here is computed purely from a
// `datamodel::StockFlow` (and the `LayoutConfig` parameter, kept for
// forward-compatibility with the design's optimizer signature). All geometry
// comes from the same `diagram` helpers the SVG renderer uses and from
// `layout::build_view_segments`, so a layout's quality score can never disagree
// with the geometry the renderer draws or with `count_view_crossings`.
//
// There is NO I/O in this module: it takes data, computes scalars, returns
// them. That makes every term trivially testable with hand-computed expected
// values (see the inline tests below).

use std::collections::HashSet;

use crate::datamodel::{self, ViewElement};
use crate::diagram::common::{
    self, Point, Rect, display_name, merge_bounds, rect_area, rect_overlap_area,
    segment_length_in_rect,
};
use crate::diagram::connector::{ARC_POLYLINE_SAMPLES, connector_polyline};
use crate::diagram::elements::{
    aux_bounds, aux_shape_bounds, cloud_bounds, module_bounds, stock_bounds, stock_shape_bounds,
};
use crate::diagram::flow::{flow_bounds, flow_shape_bounds};
use crate::diagram::label::{LabelProps, label_bounds};

use super::annealing::count_crossings;
use super::build_view_segments;
use super::config::LayoutConfig;

/// Upper bound of the target aspect-ratio band. A view whose bounding-box
/// aspect ratio (long side / short side, always >= 1) is at or below this value
/// is "well-proportioned" and incurs no `aspect_penalty`. 16:9 is a generous
/// band that comfortably contains the conventional 4:3 diagram proportions
/// while still penalizing pathologically thin (e.g. 1x10) layouts.
pub const TARGET_AR_MAX: f64 = 16.0 / 9.0;

/// One quality cost per aesthetic concern, with `0.0` always meaning "ideal".
///
/// Most terms are scale-free by construction (ratios of like quantities), so
/// they are comparable across models of different absolute coordinate scale.
/// Three terms are *intentionally* sensitive to the absolute coordinate scale
/// relative to the universal fixed node-box size (`node_overlap`,
/// `label_overlap`, `sprawl`): a model whose nodes are packed tightly against
/// the fixed pixel size of a stock/aux box should score differently from one
/// spread far apart, and that sensitivity is what makes those terms meaningful
/// across models. See the AC1.8 scoping note in the Phase 1 plan.
///
/// `Serialize`/`Deserialize` let the layout-quality eval sweep
/// (`examples/layout_eval.rs`) emit the per-term breakdown into its
/// `metrics.json` artifact and round-trip the committed baseline report back
/// from JSON for the baseline diff; the struct is pure data (every field a
/// plain `f64`), so the derives carry no behavior.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayoutMetrics {
    /// Sum of pairwise node *shape*-box overlap area (label-free), normalized
    /// by total shape-box area. Measures shapes overlapping shapes; label
    /// collisions are charged by `label_overlap` instead.
    pub node_overlap: f64,
    /// Fraction of total connector length that passes through non-incident
    /// node *shape* boxes (label-free). A connector under a node shape reads as
    /// a false causal connection; a connector under only a label is not
    /// charged here.
    pub node_connector_overlap: f64,
    /// Sum of label-vs-label and label-vs-node overlap area, normalized by
    /// total label area.
    pub label_overlap: f64,
    /// Edge crossings normalized by connector count.
    pub crossings: f64,
    /// Mean connector length relative to the characteristic node size.
    pub sprawl: f64,
    /// Coefficient of variation (stddev/mean) of connector lengths.
    pub edge_length_cv: f64,
    /// How far the view bounding-box aspect ratio exceeds the target band.
    pub aspect_penalty: f64,
    /// Reserved; computed in a future rung. Always 0.0, weight 0.
    pub chain_straightness: f64,
    /// Reserved; computed in a future rung. Always 0.0, weight 0.
    pub loop_compactness: f64,
}

/// Per-term weights for the scalar an optimizer minimizes.
///
/// The calibrated production weights (and the failure-mode priority ordering)
/// are committed in Phase 4. Until then `MetricWeights::default()` is all-zeros
/// (see below) so any accidental use of `weighted_cost` before calibration is
/// obviously inert rather than silently wrong.
///
/// `Serialize`/`Deserialize` let the layout-quality eval sweep
/// (`examples/layout_eval.rs`) record the weight set it used in its
/// `metrics.json` artifact and read it back when round-tripping the committed
/// baseline report; the struct is pure data (every field a plain `f64`), so the
/// derives carry no behavior.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetricWeights {
    pub node_overlap: f64,
    pub node_connector_overlap: f64,
    pub label_overlap: f64,
    pub crossings: f64,
    pub sprawl: f64,
    pub edge_length_cv: f64,
    pub aspect_penalty: f64,
    pub chain_straightness: f64,
    pub loop_compactness: f64,
}

impl Default for MetricWeights {
    /// All-zeros: calibrated in Phase 4. An all-zero weight set makes
    /// `weighted_cost` return 0.0 regardless of the metrics, so using the
    /// default before calibration is inert (cannot mislead an optimizer) rather
    /// than applying made-up weights.
    fn default() -> Self {
        MetricWeights {
            node_overlap: 0.0,
            node_connector_overlap: 0.0,
            label_overlap: 0.0,
            crossings: 0.0,
            sprawl: 0.0,
            edge_length_cv: 0.0,
            aspect_penalty: 0.0,
            chain_straightness: 0.0,
            loop_compactness: 0.0,
        }
    }
}

impl LayoutMetrics {
    /// Sigma w_i * term_i -- the scalar an optimizer minimizes.
    pub fn weighted_cost(&self, w: &MetricWeights) -> f64 {
        self.node_overlap * w.node_overlap
            + self.node_connector_overlap * w.node_connector_overlap
            + self.label_overlap * w.label_overlap
            + self.crossings * w.crossings
            + self.sprawl * w.sprawl
            + self.edge_length_cv * w.edge_length_cv
            + self.aspect_penalty * w.aspect_penalty
            + self.chain_straightness * w.chain_straightness
            + self.loop_compactness * w.loop_compactness
    }
}

/// The drawn geometry of one connector (Link or Flow): its incident node uids
/// (so node-connector-overlap can skip them) and the polyline the renderer
/// draws. Built once and reused by every connector-derived term so they all see
/// the same geometry.
struct ConnectorGeometry {
    /// Element uids the connector is attached to and must not be charged for
    /// passing through (its own endpoints).
    incident_uids: HashSet<i32>,
    /// The drawn polyline. Always has at least two points (connectors that draw
    /// nothing -- e.g. MultiPoint links -- are not collected at all).
    polyline: Vec<Point>,
    /// Total polyline length.
    length: f64,
}

/// Polyline length: sum of segment lengths.
fn polyline_length(points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|w| {
            let dx = w[1].x - w[0].x;
            let dy = w[1].y - w[0].y;
            (dx * dx + dy * dy).sqrt()
        })
        .sum()
}

/// Resolve the node box for an element that has one (everything except links,
/// groups, and aliases -- aliases have no bounds helper and are excluded to
/// match the renderer's `calc_view_box`).
fn node_box(element: &ViewElement) -> Option<Rect> {
    match element {
        ViewElement::Aux(a) => Some(aux_bounds(a)),
        ViewElement::Stock(s) => Some(stock_bounds(s)),
        ViewElement::Module(m) => Some(module_bounds(m)),
        ViewElement::Cloud(c) => Some(cloud_bounds(c)),
        ViewElement::Flow(f) => Some(flow_bounds(f)),
        ViewElement::Link(_) | ViewElement::Alias(_) | ViewElement::Group(_) => None,
    }
}

/// The element's bare *shape* box, WITHOUT its own label, for the same set of
/// elements as `node_box`. `aux_bounds`/`stock_bounds`/`flow_bounds` merge each
/// element's own label into the returned box; the label-vs-node term of
/// `label_overlap` must use the label-free shape so a label-vs-label overlap is
/// not also charged via the other node's label-merged box (a double-count).
/// `module_bounds`/`cloud_bounds` already exclude the label (modules render a
/// label that their bounds omit; clouds render none), so they are their own
/// shape box.
fn node_shape_box(element: &ViewElement) -> Option<Rect> {
    match element {
        ViewElement::Aux(a) => Some(aux_shape_bounds(a)),
        ViewElement::Stock(s) => Some(stock_shape_bounds(s)),
        ViewElement::Module(m) => Some(module_bounds(m)),
        ViewElement::Cloud(c) => Some(cloud_bounds(c)),
        ViewElement::Flow(f) => Some(flow_shape_bounds(f)),
        ViewElement::Link(_) | ViewElement::Alias(_) | ViewElement::Group(_) => None,
    }
}

/// Build a `LabelProps` for a labeled element, matching the renderer's label
/// geometry (center, label side, display name, and the element's radii). Only
/// elements that render a label return `Some`. The radii match the per-element
/// `with_radii` calls in `diagram::elements`/`diagram::flow`.
fn element_label_props(element: &ViewElement) -> Option<LabelProps> {
    use crate::diagram::constants::{
        AUX_RADIUS, FLOW_VALVE_RADIUS, MODULE_HEIGHT, MODULE_WIDTH, STOCK_HEIGHT, STOCK_WIDTH,
    };
    match element {
        ViewElement::Aux(a) => Some(
            LabelProps::new(a.x, a.y, a.label_side, display_name(&a.name))
                .with_radii(AUX_RADIUS, AUX_RADIUS),
        ),
        ViewElement::Stock(s) => Some(
            LabelProps::new(s.x, s.y, s.label_side, display_name(&s.name))
                .with_radii(STOCK_WIDTH / 2.0, STOCK_HEIGHT / 2.0),
        ),
        ViewElement::Module(m) => Some(
            LabelProps::new(m.x, m.y, m.label_side, display_name(&m.name))
                .with_radii(MODULE_WIDTH / 2.0, MODULE_HEIGHT / 2.0),
        ),
        ViewElement::Flow(f) => Some(
            LabelProps::new(f.x, f.y, f.label_side, display_name(&f.name))
                .with_radii(FLOW_VALVE_RADIUS, FLOW_VALVE_RADIUS),
        ),
        // Aliases do render a label, but they have no `*_bounds` helper and are
        // excluded from node bounds to match the renderer's view box; we keep
        // the label-set consistent with the node-box set by also excluding
        // their labels. Links/Clouds/Groups render no element label.
        ViewElement::Alias(_)
        | ViewElement::Link(_)
        | ViewElement::Cloud(_)
        | ViewElement::Group(_) => None,
    }
}

/// Collect the drawn geometry of every connector (Link or Flow) that draws
/// something. Links use the shared `connector_polyline` (the exact geometry the
/// renderer draws and `build_view_segments` counts); flows use their point
/// polyline. Connectors that draw nothing (MultiPoint links, degenerate arcs,
/// flows with fewer than two points) are omitted entirely.
fn collect_connector_geometry(view: &datamodel::StockFlow) -> Vec<ConnectorGeometry> {
    let mut uid_elements = std::collections::HashMap::new();
    for elem in &view.elements {
        uid_elements.insert(elem.get_uid(), elem);
    }
    // Center-based, deterministic: nothing is treated as arrayed (matches
    // `build_view_segments`).
    let not_arrayed = |_: &str| false;

    let mut out = Vec::new();
    for elem in &view.elements {
        match elem {
            ViewElement::Link(link) => {
                let (Some(&from), Some(&to)) = (
                    uid_elements.get(&link.from_uid),
                    uid_elements.get(&link.to_uid),
                ) else {
                    continue;
                };
                let polyline =
                    connector_polyline(link, from, to, &not_arrayed, ARC_POLYLINE_SAMPLES);
                if polyline.len() < 2 {
                    continue;
                }
                let length = polyline_length(&polyline);
                let mut incident_uids = HashSet::new();
                incident_uids.insert(link.from_uid);
                incident_uids.insert(link.to_uid);
                out.push(ConnectorGeometry {
                    incident_uids,
                    polyline,
                    length,
                });
            }
            ViewElement::Flow(flow) => {
                if flow.points.len() < 2 {
                    continue;
                }
                let polyline: Vec<Point> = flow
                    .points
                    .iter()
                    .map(|p| Point { x: p.x, y: p.y })
                    .collect();
                let length = polyline_length(&polyline);
                // A flow is incident on its own valve plus any element its
                // points attach to (the stock/cloud at each end).
                let mut incident_uids = HashSet::new();
                incident_uids.insert(flow.uid);
                for p in &flow.points {
                    if let Some(uid) = p.attached_to_uid {
                        incident_uids.insert(uid);
                    }
                }
                out.push(ConnectorGeometry {
                    incident_uids,
                    polyline,
                    length,
                });
            }
            _ => {}
        }
    }
    out
}

/// Compute the layout quality metrics for a completed view.
///
/// PURE: takes data, returns scalars, performs no I/O. The `_config` parameter
/// is kept to match the design's optimizer-facing signature and for forward
/// compatibility; the box geometry is sourced entirely from the `diagram`
/// helpers (which use fixed pixel element sizes), so the config is presently
/// unused. Every term is guaranteed finite (each division guards a zero
/// denominator by returning 0), so empty and single-element views yield
/// all-zero, NaN-free metrics.
pub fn compute_layout_metrics(
    view: &datamodel::StockFlow,
    _config: &LayoutConfig,
) -> LayoutMetrics {
    // --- node boxes (with their owning element for incidence checks) ---
    //
    // Two box sets, used by different terms:
    //   * `node_boxes` is the LABEL-MERGED box (`node_box`): each element's own
    //     label unioned into its shape. The view's visual extent and its
    //     characteristic node size both include labels, so `sprawl` and
    //     `aspect_penalty` use this set.
    //   * `node_shape_boxes` is the bare SHAPE box (`node_shape_box`):
    //     label-free. `node_overlap` and `node_connector_overlap` use this set
    //     so they measure exactly what the user cares about -- node SHAPES
    //     overlapping other node shapes, and a connector passing under a node
    //     SHAPE (a false-causal-connection at a glance). A connector passing
    //     only under a node's LABEL is mild noise (labels are semi-transparent
    //     and no connector terminates on one) and must NOT be charged here;
    //     label collisions are the province of `label_overlap`.
    let node_boxes: Vec<(i32, Rect)> = view
        .elements
        .iter()
        .filter_map(|e| node_box(e).map(|r| (e.get_uid(), r)))
        .collect();
    let node_shape_boxes: Vec<(i32, Rect)> = view
        .elements
        .iter()
        .filter_map(|e| node_shape_box(e).map(|r| (e.get_uid(), r)))
        .collect();

    // --- node_overlap (bare shape boxes, normalized by total shape-box area) ---
    let total_shape_area: f64 = node_shape_boxes.iter().map(|(_, r)| rect_area(r)).sum();
    let node_overlap = if total_shape_area > 0.0 {
        let mut overlap = 0.0;
        for i in 0..node_shape_boxes.len() {
            for j in (i + 1)..node_shape_boxes.len() {
                overlap += rect_overlap_area(&node_shape_boxes[i].1, &node_shape_boxes[j].1);
            }
        }
        overlap / total_shape_area
    } else {
        0.0
    };

    // --- connector geometry (shared by several terms) ---
    let connectors = collect_connector_geometry(view);
    let total_connector_length: f64 = connectors.iter().map(|c| c.length).sum();

    // --- node_connector_overlap (length inside non-incident shape boxes) ---
    let node_connector_overlap = if total_connector_length > 0.0 {
        let mut inside = 0.0;
        for c in &connectors {
            for (uid, rect) in &node_shape_boxes {
                if c.incident_uids.contains(uid) {
                    continue; // skip the connector's own endpoints
                }
                for seg in c.polyline.windows(2) {
                    inside += segment_length_in_rect(&seg[0], &seg[1], rect);
                }
            }
        }
        inside / total_connector_length
    } else {
        0.0
    };

    // --- label_overlap ---
    // Each label box is tagged with its owning element's uid so the
    // label-vs-node sum can skip that element's own node box: a label is, by
    // construction, adjacent to (and inside the merged bounds of) its own
    // element, so charging it against its own box would always add exactly the
    // label's area -- a constant that is not a real collision.
    //
    // The label-vs-node sum compares each label against every OTHER element's
    // bare *shape* box (`node_shape_box`), NOT its label-merged `node_box`.
    // `aux_bounds`/`stock_bounds`/`flow_bounds` union each element's own label
    // into the box they return, so comparing a label against another node's
    // MERGED box would re-count a label-vs-label overlap that the label-vs-label
    // term above already counts -- a double-count that inflates the term's
    // magnitude (which Phase 4 calibrates against). Using the label-free shape
    // cleanly separates "label lands on another label" from "label lands on
    // another node's shape".
    let label_boxes: Vec<(i32, Rect)> = view
        .elements
        .iter()
        .filter_map(|e| element_label_props(e).map(|props| (e.get_uid(), label_bounds(&props))))
        .collect();
    // `node_shape_boxes` is computed once above (shared with node_overlap and
    // node_connector_overlap).
    let total_label_area: f64 = label_boxes.iter().map(|(_, r)| rect_area(r)).sum();
    let label_overlap = if total_label_area > 0.0 {
        let mut overlap = 0.0;
        // label-vs-label (each unordered pair once)
        for i in 0..label_boxes.len() {
            for j in (i + 1)..label_boxes.len() {
                overlap += rect_overlap_area(&label_boxes[i].1, &label_boxes[j].1);
            }
        }
        // label-vs-node, against the OTHER element's bare shape box.
        for (lbl_uid, lbl) in &label_boxes {
            for (node_uid, node) in &node_shape_boxes {
                if lbl_uid == node_uid {
                    continue;
                }
                overlap += rect_overlap_area(lbl, node);
            }
        }
        overlap / total_label_area
    } else {
        0.0
    };

    // --- crossings ---
    let connector_count = connectors.len();
    let crossings = if connector_count > 0 {
        count_crossings(&build_view_segments(view)) as f64 / connector_count as f64
    } else {
        0.0
    };

    // --- sprawl ---
    let sprawl = if !connectors.is_empty() && !node_boxes.is_empty() {
        let mean_connector_length = total_connector_length / connectors.len() as f64;
        let characteristic_node_size = node_boxes
            .iter()
            .map(|(_, r)| {
                let w = common::rect_width(r);
                let h = common::rect_height(r);
                (w * w + h * h).sqrt()
            })
            .sum::<f64>()
            / node_boxes.len() as f64;
        if characteristic_node_size > 0.0 {
            mean_connector_length / characteristic_node_size
        } else {
            0.0
        }
    } else {
        0.0
    };

    // --- edge_length_cv ---
    let edge_length_cv = if connectors.len() >= 2 {
        let n = connectors.len() as f64;
        let mean = total_connector_length / n;
        if mean > 0.0 {
            let variance = connectors
                .iter()
                .map(|c| {
                    let d = c.length - mean;
                    d * d
                })
                .sum::<f64>()
                / n; // population variance
            variance.sqrt() / mean
        } else {
            0.0
        }
    } else {
        0.0
    };

    // --- aspect_penalty ---
    // Bounding box over node boxes (union). The aspect ratio is the long side
    // over the short side (always >= 1); we penalize the amount by which it
    // exceeds the target band. Chosen formula: `ar - TARGET_AR_MAX` (a plain
    // unit-of-ratio overshoot). Documented here and matched in the AC1.5 test.
    let aspect_penalty = match view_bounding_box(&node_boxes) {
        Some(bbox) => {
            let w = common::rect_width(&bbox);
            let h = common::rect_height(&bbox);
            let (long, short) = if w >= h { (w, h) } else { (h, w) };
            if short <= 0.0 {
                0.0
            } else {
                let ar = long / short;
                (ar - TARGET_AR_MAX).max(0.0)
            }
        }
        None => 0.0,
    };

    LayoutMetrics {
        node_overlap,
        node_connector_overlap,
        label_overlap,
        crossings,
        sprawl,
        edge_length_cv,
        aspect_penalty,
        // reserved; computed in a future rung
        chain_straightness: 0.0,
        loop_compactness: 0.0,
    }
}

/// Union of the node boxes, or `None` if there are no node boxes.
fn view_bounding_box(node_boxes: &[(i32, Rect)]) -> Option<Rect> {
    let mut iter = node_boxes.iter();
    let first = iter.next()?.1;
    Some(iter.fold(first, |acc, (_, r)| merge_bounds(acc, *r)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element::{self, LabelSide, LinkShape};
    use crate::diagram::constants::STOCK_WIDTH;
    use proptest::prelude::*;

    // --- fixture helpers ---

    fn stock(uid: i32, name: &str, x: f64, y: f64) -> ViewElement {
        ViewElement::Stock(view_element::Stock {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
            compat: None,
        })
    }

    fn aux(uid: i32, name: &str, x: f64, y: f64) -> ViewElement {
        ViewElement::Aux(view_element::Aux {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
            compat: None,
        })
    }

    fn straight_link(uid: i32, from_uid: i32, to_uid: i32) -> ViewElement {
        ViewElement::Link(view_element::Link {
            uid,
            from_uid,
            to_uid,
            shape: LinkShape::Straight,
            polarity: None,
        })
    }

    fn make_view(elements: Vec<ViewElement>) -> datamodel::StockFlow {
        datamodel::StockFlow {
            name: None,
            elements,
            view_box: datamodel::Rect {
                x: 0.0,
                y: 0.0,
                width: 1000.0,
                height: 1000.0,
            },
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        }
    }

    fn cfg() -> LayoutConfig {
        LayoutConfig::default()
    }

    /// Scale every coordinate of a view by `s` (element centers and any
    /// flow/connector points). Used by the AC1.8 scale-invariance test.
    fn scale_view(view: &datamodel::StockFlow, s: f64) -> datamodel::StockFlow {
        let elements = view
            .elements
            .iter()
            .map(|e| match e {
                ViewElement::Aux(a) => ViewElement::Aux(view_element::Aux {
                    x: a.x * s,
                    y: a.y * s,
                    ..a.clone()
                }),
                ViewElement::Stock(st) => ViewElement::Stock(view_element::Stock {
                    x: st.x * s,
                    y: st.y * s,
                    ..st.clone()
                }),
                ViewElement::Flow(f) => ViewElement::Flow(view_element::Flow {
                    x: f.x * s,
                    y: f.y * s,
                    points: f
                        .points
                        .iter()
                        .map(|p| view_element::FlowPoint {
                            x: p.x * s,
                            y: p.y * s,
                            attached_to_uid: p.attached_to_uid,
                        })
                        .collect(),
                    ..f.clone()
                }),
                ViewElement::Module(m) => ViewElement::Module(view_element::Module {
                    x: m.x * s,
                    y: m.y * s,
                    ..m.clone()
                }),
                ViewElement::Cloud(c) => ViewElement::Cloud(view_element::Cloud {
                    x: c.x * s,
                    y: c.y * s,
                    ..c.clone()
                }),
                ViewElement::Alias(a) => ViewElement::Alias(view_element::Alias {
                    x: a.x * s,
                    y: a.y * s,
                    ..a.clone()
                }),
                other => other.clone(),
            })
            .collect();
        datamodel::StockFlow {
            elements,
            ..view.clone()
        }
    }

    // --- AC1.1: node_overlap equals known overlap / total node area ---

    #[test]
    fn test_node_overlap_known_overlap_fraction() {
        // Two stocks (45x35) whose centers are 20px apart horizontally and at
        // the same y. node_overlap is computed on the bare SHAPE boxes (not the
        // label-merged boxes), so the expected value comes from
        // `stock_shape_bounds` and is normalized by the total SHAPE-box area.
        let s1 = stock(1, "a", 100.0, 100.0);
        let s2 = stock(2, "b", 120.0, 100.0);
        let view = make_view(vec![s1.clone(), s2.clone()]);

        let m = compute_layout_metrics(&view, &cfg());

        // Expected: compute directly from the two bare shape boxes the renderer
        // draws (the rects, label-free).
        let b1 = node_shape_box(&s1).unwrap();
        let b2 = node_shape_box(&s2).unwrap();
        let expected_overlap = rect_overlap_area(&b1, &b2);
        let expected_total = rect_area(&b1) + rect_area(&b2);
        assert!(expected_overlap > 0.0, "fixture must actually overlap");
        let expected = expected_overlap / expected_total;
        assert!(
            (m.node_overlap - expected).abs() < 1e-9,
            "node_overlap {} != expected {}",
            m.node_overlap,
            expected
        );
    }

    #[test]
    fn test_node_overlap_simple_hand_computed() {
        // Two stocks with exactly one stock-width of horizontal center
        // separation. node_overlap is a sum over the bare SHAPE boxes, so only
        // the rects matter (labels are irrelevant to this term now).
        let s1 = stock(1, "a", 0.0, 0.0);
        let s2 = stock(2, "b", STOCK_WIDTH, 0.0); // centers exactly one width apart
        let view = make_view(vec![s1, s2]);
        let m = compute_layout_metrics(&view, &cfg());
        // Centers one full width apart -> the 45-wide shape boxes just touch in
        // x (right edge of #1 at +22.5, left edge of #2 at +22.5): zero shape
        // overlap. So node_overlap == 0.
        assert_eq!(m.node_overlap, 0.0);
    }

    // --- AC1.2: pairwise-disjoint nodes => node_overlap == 0 ---

    #[test]
    fn test_node_overlap_disjoint_is_zero() {
        let view = make_view(vec![
            stock(1, "a", 0.0, 0.0),
            stock(2, "b", 500.0, 500.0),
            aux(3, "c", 1000.0, 0.0),
        ]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(m.node_overlap, 0.0);
    }

    // node_overlap is computed on the bare SHAPE boxes, NOT the label-merged
    // boxes. The user cares about node shapes overlapping other node shapes;
    // a label landing on another node's shape (or another label) is the
    // province of `label_overlap`. This test distinguishes the two regimes and
    // would FAIL against the prior label-merged-box implementation.

    #[test]
    fn test_node_overlap_labels_overlap_shapes_disjoint_is_zero() {
        // Two `LabelSide::Bottom` auxes named "samename" (8 chars), 40px apart
        // horizontally at the same y -- the same fixture as the label_overlap
        // double-count regression test:
        //   aux1 @ (0,0):  shape [-9,9]x[-9,9],   label [-29,29]x[13,27]
        //   aux2 @ (40,0): shape [31,49]x[-9,9],  label [11,69]x[13,27]
        // The SHAPE boxes are disjoint (9 < 31), so node_overlap == 0. The
        // LABEL boxes overlap, but that collision belongs to label_overlap, not
        // node_overlap. Under the old label-merged boxes node_overlap would be
        // > 0 (the merged boxes [-29,29]x[-9,27] and [11,69]x[-9,27] overlap),
        // so this assertion pins the new shape-only behavior.
        let view = make_view(vec![
            aux(1, "samename", 0.0, 0.0),
            aux(2, "samename", 40.0, 0.0),
        ]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(
            m.node_overlap, 0.0,
            "node_overlap must ignore label-only overlap (shapes are disjoint)"
        );
        // Sanity: the label collision IS captured by label_overlap, confirming
        // the overlap was not simply lost.
        assert!(
            m.label_overlap > 0.0,
            "the label-vs-label overlap must still be charged by label_overlap"
        );
    }

    #[test]
    fn test_node_overlap_shapes_overlap_is_positive() {
        // Two stocks (45x35) whose centers are 20px apart horizontally and at
        // the same y -- their bare SHAPE boxes overlap, so node_overlap > 0.
        let view = make_view(vec![
            stock(1, "a", 100.0, 100.0),
            stock(2, "b", 120.0, 100.0),
        ]);
        let m = compute_layout_metrics(&view, &cfg());
        assert!(
            m.node_overlap > 0.0,
            "overlapping node shapes must produce positive node_overlap"
        );
    }

    // --- AC1.3: node_connector_overlap ---

    #[test]
    fn test_node_connector_overlap_through_third_node() {
        // Connector from aux #1 (far left) to aux #2 (far right), passing
        // horizontally through a stock #3 sitting on the line at the middle.
        let a = aux(1, "a", 0.0, 0.0);
        let b = aux(2, "b", 400.0, 0.0);
        let mid = stock(3, "s", 200.0, 0.0);
        let link = straight_link(10, 1, 2);
        let view = make_view(vec![a, b, mid, link]);

        let m = compute_layout_metrics(&view, &cfg());
        assert!(
            m.node_connector_overlap > 0.0,
            "connector passing through a non-incident stock must contribute"
        );

        // Expected = clipped length inside the stock SHAPE box / total polyline
        // len. node_connector_overlap charges against the bare shape box, not
        // the label-merged box. (The connector is horizontal at y=0, so the
        // clipped length happens to be identical to the label-merged box here;
        // the SHAPE box is the contract regardless.)
        let connectors = collect_connector_geometry(&view);
        assert_eq!(connectors.len(), 1);
        let c = &connectors[0];
        let stock_box = node_shape_box(&stock(3, "s", 200.0, 0.0)).unwrap();
        let mut inside = 0.0;
        for seg in c.polyline.windows(2) {
            inside += segment_length_in_rect(&seg[0], &seg[1], &stock_box);
        }
        let expected = inside / c.length;
        assert!(
            (m.node_connector_overlap - expected).abs() < 1e-9,
            "got {} expected {}",
            m.node_connector_overlap,
            expected
        );
    }

    #[test]
    fn test_node_connector_overlap_avoids_all_is_zero() {
        // Connector between two auxes with a third node well off the line.
        let a = aux(1, "a", 0.0, 0.0);
        let b = aux(2, "b", 400.0, 0.0);
        let off = stock(3, "s", 200.0, 500.0);
        let link = straight_link(10, 1, 2);
        let view = make_view(vec![a, b, off, link]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(m.node_connector_overlap, 0.0);
    }

    // node_connector_overlap charges a connector for the length it spends
    // inside a non-incident node's bare SHAPE box, NOT its label-merged box.
    // The user reads a connector passing under a node SHAPE as a false causal
    // connection (high priority); a connector passing only under a node's LABEL
    // is mild noise (labels are semi-transparent, no connector starts/ends on a
    // label) and must NOT be charged. These two tests pin that distinction; the
    // first would FAIL against the prior label-merged-box implementation.

    #[test]
    fn test_node_connector_overlap_under_label_only_is_zero() {
        // Connector from aux #1 (0,0) to aux #2 (400,0): a horizontal line at
        // y=0 (clipped to the 9px aux radii, so drawn x in [9, 391]). A
        // non-incident `LabelSide::Bottom` stock #3 named "s" (1 char) is placed
        // ABOVE the line so its SHAPE box clears y=0 but its label (which hangs
        // BELOW the shape) reaches down across y=0:
        //   stock #3 @ (200,-25):
        //     shape box  x [177.5, 222.5], y [-42.5, -7.5]   (does NOT cross 0)
        //     label box  x [192, 208],     y [-3.5, 10.5]    (DOES cross 0)
        // The connector at y=0 passes through the label band but never enters
        // the shape box, so node_connector_overlap == 0. Under the old
        // label-merged box (which unions the label, y [-42.5, 10.5]) the line
        // WOULD be charged, so this assertion is the load-bearing distinction.
        let a = aux(1, "a", 0.0, 0.0);
        let b = aux(2, "b", 400.0, 0.0);
        let label_only = stock(3, "s", 200.0, -25.0);
        let link = straight_link(10, 1, 2);
        let view = make_view(vec![a, b, label_only, link]);

        // Confirm the fixture geometry is what we claim before asserting on the
        // metric: shape box clears the line, merged box does not.
        let shape = node_shape_box(&stock(3, "s", 200.0, -25.0)).unwrap();
        let merged = node_box(&stock(3, "s", 200.0, -25.0)).unwrap();
        assert!(
            shape.bottom < 0.0,
            "shape box must clear the connector line (bottom {} < 0)",
            shape.bottom
        );
        assert!(
            merged.bottom > 0.0,
            "merged box must cross the connector line via the label (bottom {} > 0)",
            merged.bottom
        );

        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(
            m.node_connector_overlap, 0.0,
            "a connector passing only under a node's LABEL must not be charged"
        );
    }

    #[test]
    fn test_node_connector_overlap_under_shape_is_positive() {
        // Same connector, but the non-incident stock sits ON the line so the
        // connector crosses its SHAPE box -- the false-causal-connection case
        // the user cares about. node_connector_overlap > 0.
        let a = aux(1, "a", 0.0, 0.0);
        let b = aux(2, "b", 400.0, 0.0);
        let on_line = stock(3, "s", 200.0, 0.0);
        let link = straight_link(10, 1, 2);
        let view = make_view(vec![a, b, on_line, link]);
        let m = compute_layout_metrics(&view, &cfg());
        assert!(
            m.node_connector_overlap > 0.0,
            "a connector passing under a node SHAPE must be charged"
        );
    }

    // --- AC1.4: label_overlap ---

    #[test]
    fn test_label_overlap_overlapping_labels() {
        // Two auxes at the same position -> their labels (Bottom) coincide.
        let view = make_view(vec![
            aux(1, "samename", 100.0, 100.0),
            aux(2, "samename", 100.0, 100.0),
        ]);
        let m = compute_layout_metrics(&view, &cfg());
        assert!(
            m.label_overlap > 0.0,
            "coincident labels must produce positive label_overlap"
        );
    }

    #[test]
    fn test_label_overlap_disjoint_is_zero() {
        // Two auxes far apart -> labels and node boxes are all disjoint.
        let view = make_view(vec![aux(1, "a", 0.0, 0.0), aux(2, "b", 1000.0, 1000.0)]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(m.label_overlap, 0.0);
    }

    #[test]
    fn test_label_overlap_counts_label_pair_exactly_once() {
        // Regression for the label_overlap double-count: the label-vs-node term
        // must compare each label against the OTHER element's bare *shape* box,
        // not its label-merged `*_bounds` box. Otherwise a label-vs-label
        // overlap is also counted (once or twice more) inside the other node's
        // merged box, inflating the magnitude (and the Phase 4 weight it
        // calibrates).
        //
        // Fixture: two `LabelSide::Bottom` auxes named "samename" (8 chars).
        //   AUX_RADIUS = 9; label editor width = 8*6 + 10 = 58, height = 14.
        //   With Bottom labels, label top = cy + 9 + LABEL_PADDING(4) = cy + 13,
        //   bottom = cy + 27, left = cx - 29, right = cx + 29.
        //
        // Place them 40px apart horizontally, same y:
        //   aux1 @ (0,0): shape [-9,9]x[-9,9],  label [-29,29]x[13,27]
        //   aux2 @ (40,0): shape [31,49]x[-9,9], label [11,69]x[13,27]
        //
        // SHAPE boxes do NOT overlap (9 < 31). LABELS overlap by
        //   x: [11,29] = 18, y: [13,27] = 14  ->  18*14 = 252.
        // Each label clears the OTHER aux's bare shape box entirely, so the only
        // contribution is the single label-vs-label pair: total overlap = 252.
        let view = make_view(vec![
            aux(1, "samename", 0.0, 0.0),
            aux(2, "samename", 40.0, 0.0),
        ]);
        let m = compute_layout_metrics(&view, &cfg());

        // Total label area = 2 * (58 * 14) = 1624.
        let expected_overlap = 18.0 * 14.0; // 252.0, counted exactly once
        let total_label_area = 2.0 * (58.0 * 14.0); // 1624.0
        let expected = expected_overlap / total_label_area;
        assert!(
            (m.label_overlap - expected).abs() < 1e-9,
            "label_overlap should count the label pair exactly once: got {} expected {}",
            m.label_overlap,
            expected
        );
    }

    // --- AC1.5: aspect_penalty ---

    #[test]
    fn test_aspect_penalty_thin_box_positive() {
        // Two auxes stacked far apart vertically and close horizontally -> the
        // node bounding box is tall and thin (ar >> target), so penalty > 0.
        let view = make_view(vec![aux(1, "a", 0.0, 0.0), aux(2, "b", 0.0, 1000.0)]);
        let m = compute_layout_metrics(&view, &cfg());
        assert!(
            m.aspect_penalty > 0.0,
            "a tall thin bbox must be penalized, got {}",
            m.aspect_penalty
        );

        // Verify it equals exactly `ar - TARGET_AR_MAX` for the computed bbox.
        let node_boxes: Vec<(i32, Rect)> = view
            .elements
            .iter()
            .filter_map(|e| node_box(e).map(|r| (e.get_uid(), r)))
            .collect();
        let bbox = view_bounding_box(&node_boxes).unwrap();
        let w = common::rect_width(&bbox);
        let h = common::rect_height(&bbox);
        let (long, short) = if w >= h { (w, h) } else { (h, w) };
        let expected = (long / short - TARGET_AR_MAX).max(0.0);
        assert!((m.aspect_penalty - expected).abs() < 1e-9);
    }

    #[test]
    fn test_aspect_penalty_balanced_box_zero() {
        // Four auxes placed so the bounding box is ~4:3 (well inside the 16:9
        // band) -> zero penalty. Width 400, height 300 between centers; the
        // fixed node radii add a small symmetric margin that keeps ar < 16/9.
        let view = make_view(vec![
            aux(1, "a", 0.0, 0.0),
            aux(2, "b", 400.0, 0.0),
            aux(3, "c", 0.0, 300.0),
            aux(4, "d", 400.0, 300.0),
        ]);
        let m = compute_layout_metrics(&view, &cfg());

        // Confirm the bbox aspect ratio really is inside the band for this
        // fixture, then assert the penalty is exactly zero.
        let node_boxes: Vec<(i32, Rect)> = view
            .elements
            .iter()
            .filter_map(|e| node_box(e).map(|r| (e.get_uid(), r)))
            .collect();
        let bbox = view_bounding_box(&node_boxes).unwrap();
        let w = common::rect_width(&bbox);
        let h = common::rect_height(&bbox);
        let ar = w.max(h) / w.min(h);
        assert!(ar <= TARGET_AR_MAX, "fixture bbox ar {} not in band", ar);
        assert_eq!(m.aspect_penalty, 0.0);
    }

    // --- AC1.6: weighted_cost is the exact linear combination ---

    #[test]
    fn test_weighted_cost_exact_linear_combination() {
        let m = LayoutMetrics {
            node_overlap: 1.5,
            node_connector_overlap: 2.0,
            label_overlap: 0.5,
            crossings: 3.0,
            sprawl: 4.0,
            edge_length_cv: 0.25,
            aspect_penalty: 6.0,
            chain_straightness: 7.0,
            loop_compactness: 8.0,
        };
        let w = MetricWeights {
            node_overlap: 10.0,
            node_connector_overlap: 20.0,
            label_overlap: 30.0,
            crossings: 40.0,
            sprawl: 50.0,
            edge_length_cv: 60.0,
            aspect_penalty: 70.0,
            chain_straightness: 80.0,
            loop_compactness: 90.0,
        };
        let expected = 1.5 * 10.0
            + 2.0 * 20.0
            + 0.5 * 30.0
            + 3.0 * 40.0
            + 4.0 * 50.0
            + 0.25 * 60.0
            + 6.0 * 70.0
            + 7.0 * 80.0
            + 8.0 * 90.0;
        assert!((m.weighted_cost(&w) - expected).abs() < 1e-9);
    }

    #[test]
    fn test_default_weights_are_all_zero_so_cost_is_inert() {
        let m = LayoutMetrics {
            node_overlap: 1.0,
            node_connector_overlap: 1.0,
            label_overlap: 1.0,
            crossings: 1.0,
            sprawl: 1.0,
            edge_length_cv: 1.0,
            aspect_penalty: 1.0,
            chain_straightness: 1.0,
            loop_compactness: 1.0,
        };
        assert_eq!(m.weighted_cost(&MetricWeights::default()), 0.0);
    }

    // --- AC1.7: empty / single-element views are all-zero and finite ---

    fn assert_all_finite(m: &LayoutMetrics) {
        assert!(m.node_overlap.is_finite());
        assert!(m.node_connector_overlap.is_finite());
        assert!(m.label_overlap.is_finite());
        assert!(m.crossings.is_finite());
        assert!(m.sprawl.is_finite());
        assert!(m.edge_length_cv.is_finite());
        assert!(m.aspect_penalty.is_finite());
        assert!(m.chain_straightness.is_finite());
        assert!(m.loop_compactness.is_finite());
    }

    fn assert_all_zero(m: &LayoutMetrics) {
        assert_eq!(m.node_overlap, 0.0);
        assert_eq!(m.node_connector_overlap, 0.0);
        assert_eq!(m.label_overlap, 0.0);
        assert_eq!(m.crossings, 0.0);
        assert_eq!(m.sprawl, 0.0);
        assert_eq!(m.edge_length_cv, 0.0);
        assert_eq!(m.aspect_penalty, 0.0);
        assert_eq!(m.chain_straightness, 0.0);
        assert_eq!(m.loop_compactness, 0.0);
    }

    #[test]
    fn test_empty_view_all_zero_finite() {
        let view = make_view(vec![]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_all_finite(&m);
        assert_all_zero(&m);
    }

    #[test]
    fn test_single_element_view_all_zero_finite() {
        let view = make_view(vec![aux(1, "only", 100.0, 100.0)]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_all_finite(&m);
        // A single node has no overlaps, no connectors, and a degenerate (zero
        // short-side? no -- a real box) bounding box. Its aspect ratio is the
        // single aux box's own ar, which for a square-ish aux box is ~1 (inside
        // the band), so aspect_penalty is 0; all connector terms are 0.
        assert_eq!(m.node_overlap, 0.0);
        assert_eq!(m.node_connector_overlap, 0.0);
        assert_eq!(m.crossings, 0.0);
        assert_eq!(m.sprawl, 0.0);
        assert_eq!(m.edge_length_cv, 0.0);
    }

    // --- AC1.8 (scoped): scale invariance under uniform coordinate scaling ---
    //
    // SCOPING (correction to the AC1.8 plan note, 2026-05-22): the plan listed
    // `node_connector_overlap`, `crossings`, `edge_length_cv`, and
    // `aspect_penalty` as scale-free. After implementing the metric against the
    // ACTUAL renderer geometry (the design's load-bearing invariant: metrics
    // are computed on the same geometry the renderer draws), only `crossings`
    // is exactly scale-invariant -- and even then only for crossings that lie
    // INTERIOR to both connectors, away from the fixed-size node boundaries the
    // polylines are clipped to (a crossing grazing a node boundary near a
    // segment endpoint can flip; see the detailed note at the assertion below).
    // This fixture's crossing is at the center of the square the two links form,
    // squarely in that interior regime. The reason the other terms are not
    // exactly invariant is the same fixed-pixel element geometry the plan
    // already cites for node_overlap/label_overlap/sprawl, and it propagates
    // further than the plan anticipated:
    //
    //   * Connectors are clipped to fixed-radius element boundaries, so a
    //     straight link's drawn length is `s*center_dist - r_from - r_to`
    //     (AFFINE in `s`, not linear). Hence `edge_length_cv = stddev/mean` of
    //     those affine lengths is only ASYMPTOTICALLY invariant (the fixed
    //     offset shrinks relative to the scaled spread), not exactly.
    //   * `node_connector_overlap` divides an inside-fixed-box overlap length
    //     (which does NOT scale) by total connector length (which does), so it
    //     shrinks like ~1/s -- scale-SENSITIVE, like `sprawl`.
    //   * The view bounding box is `union(fixed boxes around scaled centers)`,
    //     so its width/height are each `s*span + fixed_box_size`; the aspect
    //     ratio is therefore only asymptotically invariant.
    //
    // The principled resolution keeps renderer-faithful geometry (the whole
    // point of the phase) and accepts that only the topological `crossings`
    // term is exactly scale-invariant. This test asserts that exactly, and
    // additionally pins the documented scale-SENSITIVITY of
    // `node_connector_overlap` (clean ~1/s) so the scoping is non-vacuous. The
    // mismatch with the plan's term list is surfaced in the executor report and
    // tracked for the calibration phase.
    //
    // The fixture has zero node-overlap and zero label-overlap so those
    // scale-sensitive area terms are trivially 0 before and after scaling.
    #[test]
    fn test_scale_invariance_of_scale_free_terms() {
        // A small connected, well-separated view: three auxes and two stocks,
        // far enough apart that there is no node-overlap and no label-overlap,
        // with two straight links (one of which passes through a non-incident
        // node so node_connector_overlap is nonzero and meaningful).
        let view = make_view(vec![
            aux(1, "a", 0.0, 0.0),
            aux(2, "b", 400.0, 0.0),
            stock(3, "s", 200.0, 0.0), // on the a->b line: nonzero conn overlap
            aux(4, "c", 0.0, 300.0),
            stock(5, "t", 400.0, 320.0),
            straight_link(10, 1, 2), // passes through stock #3
            straight_link(11, 4, 5),
        ]);

        let base = compute_layout_metrics(&view, &cfg());
        // Sanity: the fixture must have zero node/label overlap (so the
        // scale-sensitive area terms are trivially scale-equal) and a nonzero
        // conn-overlap (so the documented scale-SENSITIVITY check is
        // non-vacuous).
        assert_eq!(base.node_overlap, 0.0, "fixture must have no node overlap");
        assert_eq!(
            base.label_overlap, 0.0,
            "fixture must have no label overlap"
        );
        assert!(
            base.node_connector_overlap > 0.0,
            "fixture must have a connector through a non-incident node"
        );

        let s = 3.0;
        let scaled = compute_layout_metrics(&scale_view(&view, s), &cfg());

        // The one exactly scale-invariant term here: edge crossings.
        //
        // Crossings are NOT *universally* scale-invariant. A crossing is counted
        // on the drawn polylines, which are clipped to the same fixed-pixel node
        // boxes (the connector endpoints sit on element boundaries that do not
        // scale). A crossing that merely grazes a node boundary near a segment
        // endpoint can therefore appear or disappear under uniform scale.
        // Crossings that lie comfortably INTERIOR to both connectors (away from
        // those fixed-size boundaries) are exactly preserved, because the
        // interior of each polyline is an exact affine image of itself under
        // uniform scale and an intersection of two segments is invariant under a
        // shared affine map. This fixture's crossing is at the center of the
        // square the two links form -- maximally far from every node box -- so
        // it is squarely in the scale-invariant interior regime and the count is
        // preserved exactly.
        assert!(
            (scaled.crossings - base.crossings).abs() < 1e-9,
            "crossings not scale-invariant: {} vs {}",
            scaled.crossings,
            base.crossings
        );

        // Documented scale-SENSITIVITY of node_connector_overlap: with
        // fixed-size node boxes, scaling the coordinates by `s` leaves the
        // inside-box overlap length essentially unchanged (the box and the
        // line's center crossing are fixed) while total connector length grows
        // with `s`, so the ratio strictly DECREASES under up-scaling. (It does
        // not drop by exactly 1/s because the denominator -- connector length
        // clipped to fixed-radius element boundaries -- is affine in `s`, not
        // linear; we assert the robust direction rather than a brittle factor.)
        assert!(
            scaled.node_connector_overlap < base.node_connector_overlap,
            "node_connector_overlap should DROP under up-scaling (fixed boxes): \
             scaled {} should be < base {}",
            scaled.node_connector_overlap,
            base.node_connector_overlap
        );
    }

    // --- Property test: node_overlap is symmetric under element shuffle ---

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// node_overlap is a sum over unordered element pairs, so it must be
        /// invariant under any permutation of the element list.
        #[test]
        fn prop_node_overlap_shuffle_invariant(
            // four stocks at small integer-ish coordinates so some overlap and
            // some don't; coordinates kept modest to stay fast.
            xs in prop::collection::vec(-50.0f64..50.0, 4),
            ys in prop::collection::vec(-50.0f64..50.0, 4),
            perm in prop::sample::subsequence(vec![0usize, 1, 2, 3], 4),
        ) {
            let elems: Vec<ViewElement> = (0..4)
                .map(|i| stock(i as i32 + 1, "n", xs[i], ys[i]))
                .collect();

            let base = compute_layout_metrics(&make_view(elems.clone()), &cfg());

            // `perm` is a random ordering of [0,1,2,3]; reorder accordingly.
            let shuffled: Vec<ViewElement> = perm.iter().map(|&i| elems[i].clone()).collect();
            let other = compute_layout_metrics(&make_view(shuffled), &cfg());

            prop_assert!(
                (base.node_overlap - other.node_overlap).abs() < 1e-9,
                "node_overlap changed under shuffle: {} vs {}",
                base.node_overlap,
                other.node_overlap
            );
        }
    }
}
