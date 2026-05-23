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

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::datamodel::{self, ViewElement};
use crate::diagram::common::{
    self, Point, Rect, display_name, merge_bounds, rect_area, rect_overlap_area,
    segment_length_in_rect,
};
use crate::diagram::connector::{ARC_POLYLINE_SAMPLES, connector_polyline, get_visual_center};
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
    /// Sum over labeled elements of each label's *obscured fraction*: the area
    /// of the label box covered by any other label box or any other element's
    /// bare shape box, capped at the label's own area and divided by it (so each
    /// term is in [0,1]). 0 = no label obscured. Per-label so a small overlap
    /// registers at its true obscuration fraction rather than being diluted by
    /// the corpus's total label area.
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
    /// Mean isoperimetric penalty `1 - Q` over the view's feedback cycles
    /// (`Q = 4*PI*Area / Perimeter^2` of each loop's node-center polygon,
    /// clamped to [0,1]). 0.0 = clean, well-spread loops (circles); higher =
    /// collapsed/collinear loops. 0.0 when the view has no cycle of >= 3 nodes.
    /// Computed and reported now; weight stays 0 until Phase 4 calibration.
    pub loop_compactness: f64,
}

/// Per-term weights for the scalar an optimizer minimizes.
///
/// `MetricWeights::default()` holds the calibrated production weights committed
/// in Phase 4 (see the failure-mode rationale on the `Default` impl below).
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
    /// The calibrated production weights, from the Phase 3 contact-sheet
    /// calibration with explicit user sign-off (2026-05-23).
    ///
    /// Failure-mode rationale -- readability >> compactness:
    ///   * The dominant concerns all carry weight 1.0: node-shape overlap
    ///     (`node_overlap`), connectors passing under node shapes
    ///     (`node_connector_overlap`), obscured labels (`label_overlap`), and
    ///     edge `crossings`. These are the things that make a diagram unreadable
    ///     or assert false causal connections, so they dominate the cost.
    ///   * `sprawl`, `edge_length_cv`, and `aspect_penalty` are intentionally
    ///     0.0: compactness and aspect ratio are NOT goals. Spreading nodes out
    ///     to keep labels legible and feedback loops visible is GOOD, not
    ///     something to penalize, so these terms must not pull against
    ///     readability.
    ///   * `loop_compactness` is a low 0.25: it gently REWARDS drawing feedback
    ///     loops as visible circles (a readability aid), but must never dominate
    ///     the overlap/crossings family, so it stays well below 1.0.
    ///   * `chain_straightness` stays 0.0: it is reserved (not yet computed), so
    ///     it carries no weight.
    fn default() -> Self {
        MetricWeights {
            node_overlap: 1.0,
            node_connector_overlap: 1.0,
            label_overlap: 1.0,
            crossings: 1.0,
            sprawl: 0.0,
            edge_length_cv: 0.0,
            aspect_penalty: 0.0,
            chain_straightness: 0.0,
            loop_compactness: 0.25,
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

// --- loop_compactness (isoperimetric feedback-loop quality) -----------------
//
// What it measures: how cleanly the view draws its feedback loops as visible
// circles. For each simple directed cycle of >= 3 positioned nodes we take the
// node-box centers in cycle order and form a polygon. Its isoperimetric
// quotient Q = 4*PI*Area / Perimeter^2 is 1 for a perfect circle and tends to 0
// as the polygon collapses toward a line (the area vanishes while the perimeter
// stays large). The per-cycle penalty is `1 - Q` (0 = ideal clean loop, ~1 =
// squished/collinear), and `loop_compactness` is the mean penalty over all
// qualifying cycles (0.0 when the view has no cycle of >= 3 nodes). It thus
// REWARDS well-spread loops and PENALIZES collapsed ones.
//
// Bounds (SD diagrams are small, so this stays O(small) and total): a simple
// cycle is enumerated only up to `MAX_CYCLE_LEN` nodes, and at most
// `MAX_CYCLES` cycles are scored; enumeration stops once the cap is hit. The
// graph is built over positioned node-box elements (aux/stock/flow/module/cloud
// -- the same set as `node_box`); links and flows supply the directed edges.
//
// Determinism: layout is deterministic per seed, but this term is additionally
// independent of element ordering. Adjacency targets are sorted, the DFS starts
// from each node in sorted uid order, and every enumerated cycle is canonicalized
// (rotated so its smallest uid is first) and de-duplicated, so the mean is the
// same regardless of how the elements are listed in the view.

/// Maximum number of nodes in an enumerated simple cycle. SD feedback loops are
/// short; a longer "cycle" is almost always an artifact of many overlapping
/// smaller loops and is not worth the combinatorial cost.
const MAX_CYCLE_LEN: usize = 12;

/// Maximum number of distinct simple cycles scored. Bounds the work on dense
/// graphs; the mean penalty over the first `MAX_CYCLES` cycles is a faithful
/// proxy for the whole (SD diagrams rarely approach this).
const MAX_CYCLES: usize = 64;

/// Directed adjacency over positioned node-box elements, keyed by uid with
/// sorted successor lists. Each node's loop vertex is the renderer's VISUAL
/// center (`diagram::connector::get_visual_center`) -- for a flow that is its
/// VALVE `(flow.x, flow.y)`, NOT the pipe-extent center of `flow_shape_bounds`
/// (which unions the valve box with every pipe point and so drifts off the valve
/// when the pipe is bent or the valve is dragged off-center); for an
/// aux/stock/module/cloud it is the element center, which already equals the
/// symmetric shape-box midpoint. Using the same visual center the SVG renderer
/// draws keeps the loop polygon faithful to the drawn diagram.
struct LoopGraph {
    /// uid -> sorted, de-duplicated successor uids.
    adj: BTreeMap<i32, Vec<i32>>,
    /// uid -> node visual-center point (the valve for flows; the element center
    /// for aux/stock/module/cloud).
    centers: BTreeMap<i32, Point>,
}

/// Build the directed loop graph from the view. Nodes are exactly the elements
/// with a node box (`node_shape_box` -- aux/stock/module/cloud/flow; links,
/// aliases, and groups are excluded). Each node's loop vertex is the renderer's
/// VISUAL center (`get_visual_center`), so a flow's vertex is its VALVE
/// `(flow.x, flow.y)`, NOT the pipe-extent center of `flow_shape_bounds` (the
/// valve box unioned with every pipe point), which drifts off the valve when the
/// pipe is bent or the valve is dragged off-center. For aux/stock/module/cloud
/// the visual center is the element center, which already equals the symmetric
/// shape-box midpoint, so those vertices are unchanged. Edges to/from uids that
/// are not positioned nodes are dropped. Edges come from:
///   * each Link: `from_uid -> to_uid`;
///   * each Flow: for consecutive attached points, `source_attached -> flow.uid`
///     and `flow.uid -> dest_attached`, so a stock--flow--stock feedback path is
///     part of the graph (the flow's own valve is the intermediate node).
fn build_loop_graph(view: &datamodel::StockFlow) -> LoopGraph {
    // The node-membership gate stays `node_shape_box` (it defines which elements
    // are loop nodes), but the loop VERTEX is the renderer's visual center, which
    // is correct for every gated kind: the valve for a flow, the element center
    // for aux/stock/module/cloud. `not_arrayed` matches `collect_connector_geometry`
    // / `build_view_segments` (offset 0, deterministic).
    let not_arrayed = |_: &str| false;
    let mut centers: BTreeMap<i32, Point> = BTreeMap::new();
    for e in &view.elements {
        if node_shape_box(e).is_some() {
            let (cx, cy) = get_visual_center(e, &not_arrayed);
            centers.insert(e.get_uid(), Point { x: cx, y: cy });
        }
    }

    // Collect edges into sorted sets per source so the adjacency is canonical
    // (sorted, de-duplicated) and the cycle search is order-independent.
    let mut edge_sets: BTreeMap<i32, BTreeSet<i32>> = BTreeMap::new();
    let mut add_edge = |from: i32, to: i32, centers: &BTreeMap<i32, Point>| {
        // Both endpoints must be positioned nodes, and we never record a
        // self-loop (a single-node "cycle" forms no polygon).
        if from != to && centers.contains_key(&from) && centers.contains_key(&to) {
            edge_sets.entry(from).or_default().insert(to);
        }
    };

    for e in &view.elements {
        match e {
            ViewElement::Link(link) => {
                add_edge(link.from_uid, link.to_uid, &centers);
            }
            ViewElement::Flow(flow) => {
                // Consecutive attached points define stock->flow and flow->stock
                // edges through the flow's own valve uid.
                let attached: Vec<i32> = flow
                    .points
                    .iter()
                    .filter_map(|p| p.attached_to_uid)
                    .collect();
                for w in attached.windows(2) {
                    add_edge(w[0], flow.uid, &centers);
                    add_edge(flow.uid, w[1], &centers);
                }
            }
            _ => {}
        }
    }

    let adj: BTreeMap<i32, Vec<i32>> = edge_sets
        .into_iter()
        .map(|(k, set)| (k, set.into_iter().collect()))
        .collect();
    LoopGraph { adj, centers }
}

/// Enumerate simple directed cycles (each >= 2 nodes), bounded by
/// `MAX_CYCLE_LEN` and `MAX_CYCLES`, canonicalized and de-duplicated so the same
/// directed cycle is returned exactly once regardless of where the search
/// started. A bounded DFS suffices: SD diagrams are tiny, and the caps keep it
/// O(small) on the rare dense graph.
///
/// Each returned cycle is a `Vec<i32>` of uids in traversal order, rotated so
/// its smallest uid is first (canonical form), and the set of returned cycles is
/// itself sorted for a fully deterministic result.
fn enumerate_simple_cycles(graph: &LoopGraph) -> Vec<Vec<i32>> {
    let mut found: BTreeSet<Vec<i32>> = BTreeSet::new();
    // Start a DFS from each node in sorted uid order. To avoid re-finding the
    // same cycle from each of its members we still canonicalize+dedup, but we
    // also restrict each search to cycles whose minimum node is the start node,
    // which prunes the bulk of the duplicate work.
    let starts: Vec<i32> = graph.adj.keys().copied().collect();
    let mut path: Vec<i32> = Vec::new();
    let mut on_path: HashSet<i32> = HashSet::new();
    for &start in &starts {
        path.clear();
        on_path.clear();
        dfs_cycles(graph, start, start, &mut path, &mut on_path, &mut found);
        if found.len() >= MAX_CYCLES {
            break;
        }
    }
    found.into_iter().take(MAX_CYCLES).collect()
}

/// Depth-first walk that records every simple cycle returning to `start` and
/// composed only of nodes whose uid is >= `start` (so each cycle is discovered
/// from its smallest member). `path`/`on_path` track the current simple path.
fn dfs_cycles(
    graph: &LoopGraph,
    start: i32,
    current: i32,
    path: &mut Vec<i32>,
    on_path: &mut HashSet<i32>,
    found: &mut BTreeSet<Vec<i32>>,
) {
    if found.len() >= MAX_CYCLES {
        return;
    }
    path.push(current);
    on_path.insert(current);

    if let Some(succs) = graph.adj.get(&current) {
        for &next in succs {
            if next == start {
                // Closed a cycle back to the start. Record it (>= 2 nodes by
                // construction; self-loops were never added as edges).
                if path.len() >= 2 {
                    found.insert(canonicalize_cycle(path));
                    if found.len() >= MAX_CYCLES {
                        break;
                    }
                }
                continue;
            }
            // Only extend through nodes strictly greater than the start (so the
            // start is the minimum), not already on the path, within the length
            // cap.
            if next > start && !on_path.contains(&next) && path.len() < MAX_CYCLE_LEN {
                dfs_cycles(graph, start, next, path, on_path, found);
                if found.len() >= MAX_CYCLES {
                    break;
                }
            }
        }
    }

    on_path.remove(&current);
    path.pop();
}

/// Rotate a cycle so its smallest uid is first, preserving traversal direction.
/// The DFS already guarantees the start (= minimum) is element 0, but rotating
/// defensively keeps the canonical form correct for any caller.
///
/// Note: this canonicalizes rotation (start at min uid) but NOT traversal
/// direction, so a directed cycle and its reverse canonicalize to distinct
/// entries. That is harmless: a reverse-direction duplicate (essentially never
/// present for directed SD feedback loops, which would require both directed
/// edge sets in the graph) would compute the same isoperimetric penalty because
/// the shoelace polygon area in `cycle_penalty` is direction-invariant.
fn canonicalize_cycle(cycle: &[i32]) -> Vec<i32> {
    if cycle.is_empty() {
        return Vec::new();
    }
    let min_idx = cycle
        .iter()
        .enumerate()
        .min_by_key(|&(_, v)| *v)
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut out = Vec::with_capacity(cycle.len());
    for k in 0..cycle.len() {
        out.push(cycle[(min_idx + k) % cycle.len()]);
    }
    out
}

/// Isoperimetric penalty `1 - Q` for one cycle's node-box centers, or `None` if
/// the cycle does not qualify (fewer than 3 distinct positioned nodes, or a
/// degenerate zero-perimeter polygon). `Q = 4*PI*Area / Perimeter^2` is clamped
/// to [0, 1]; `Area` is the shoelace area (absolute value) and `Perimeter` the
/// summed edge length over the closed polygon.
fn cycle_penalty(cycle: &[i32], centers: &BTreeMap<i32, Point>) -> Option<f64> {
    // Distinct positioned nodes only: a polygon needs >= 3 vertices.
    let distinct: BTreeSet<i32> = cycle.iter().copied().collect();
    if distinct.len() < 3 {
        return None;
    }
    let pts: Vec<Point> = cycle
        .iter()
        .filter_map(|uid| centers.get(uid).copied())
        .collect();
    if pts.len() < 3 {
        return None;
    }

    let n = pts.len();
    let mut area2 = 0.0;
    let mut perimeter = 0.0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        area2 += a.x * b.y - b.x * a.y;
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        perimeter += (dx * dx + dy * dy).sqrt();
    }
    if perimeter <= 0.0 {
        // All centers coincide: no polygon. Guarded so the division below is
        // never NaN; such a degenerate cycle simply does not contribute.
        return None;
    }
    let area = area2.abs() / 2.0;
    let q = (4.0 * std::f64::consts::PI * area / (perimeter * perimeter)).clamp(0.0, 1.0);
    Some(1.0 - q)
}

/// `loop_compactness`: mean isoperimetric penalty `1 - Q` over the view's
/// bounded simple directed cycles of >= 3 positioned nodes. 0.0 when there is no
/// qualifying cycle. Deterministic for a given view regardless of element order
/// (see the module comment above). PURE.
fn compute_loop_compactness(view: &datamodel::StockFlow) -> f64 {
    let graph = build_loop_graph(view);
    let cycles = enumerate_simple_cycles(&graph);
    let penalties: Vec<f64> = cycles
        .iter()
        .filter_map(|c| cycle_penalty(c, &graph.centers))
        .collect();
    if penalties.is_empty() {
        0.0
    } else {
        penalties.iter().sum::<f64>() / penalties.len() as f64
    }
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

    // --- label_overlap (per-label obscuration) ---
    //
    // For each labeled element L, measure how much of its label box B_L is
    // covered (obscured) by OTHER drawn geometry, then SUM each label's obscured
    // fraction. This is per-label rather than a single corpus-wide ratio: a
    // small-but-readability-killing overlap (e.g. a node circle clipping the last
    // two characters of a short label) registers at its true obscuration
    // fraction instead of being diluted to ~0 by the corpus's total label area
    // (the prior `sum_of_overlaps / total_label_area` definition under-counted
    // exactly this case).
    //
    // The coverers of B_L are (a) any OTHER label box and (b) any OTHER element's
    // bare *shape* box (`node_shape_box`, NOT the label-merged `node_box`):
    //   * A label is never charged against its OWN element's shape box. By
    //     construction a label sits adjacent to (and within the merged bounds of)
    //     its own element, so charging it there would always add a constant that
    //     is not a real collision.
    //   * Comparing against the bare shape box (not the label-merged box) keeps
    //     "label lands on another label" and "label lands on another node's
    //     shape" cleanly separate -- the merged box unions that node's own label,
    //     which would re-count the label-vs-label coverage already captured by
    //     the label-box term.
    //
    // A pixel-exact union of all coverers is unnecessary: the covered area is
    // approximated by the SUM of individual overlap areas, capped at area(B_L) so
    // a label's obscured fraction stays in [0,1] even when coverers overlap each
    // other. This is a monotone proxy (more/larger overlaps never decrease the
    // fraction). A mutual label-label collision is charged from BOTH labels'
    // perspectives -- intended, since both are unreadable. Guards area(B_L) == 0
    // (degenerate label) by skipping it, so the term is always finite.
    let label_boxes: Vec<(i32, Rect)> = view
        .elements
        .iter()
        .filter_map(|e| element_label_props(e).map(|props| (e.get_uid(), label_bounds(&props))))
        .collect();
    // `node_shape_boxes` is computed once above (shared with node_overlap and
    // node_connector_overlap).
    let mut label_overlap = 0.0;
    for (lbl_uid, lbl) in &label_boxes {
        let lbl_area = rect_area(lbl);
        if lbl_area <= 0.0 {
            continue; // degenerate label box: no NaN, contributes nothing
        }
        let mut covered = 0.0;
        // Covered by every OTHER label box.
        for (other_uid, other) in &label_boxes {
            if other_uid == lbl_uid {
                continue;
            }
            covered += rect_overlap_area(lbl, other);
        }
        // Covered by every OTHER element's bare shape box.
        for (node_uid, node) in &node_shape_boxes {
            if node_uid == lbl_uid {
                continue;
            }
            covered += rect_overlap_area(lbl, node);
        }
        // Cap the (possibly over-counted) covered area at the label's own area
        // so the obscured fraction is in [0,1].
        let obscured_fraction = (covered.min(lbl_area)) / lbl_area;
        label_overlap += obscured_fraction;
    }

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

    // --- loop_compactness (isoperimetric feedback-loop quality) ---
    let loop_compactness = compute_loop_compactness(view);

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
        loop_compactness,
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

    /// A cloud at `(x, y)`. A cloud is a positioned node with a bare shape box
    /// (`cloud_bounds`, a 27x27 square: CLOUD_RADIUS = 13.5) and NO rendered
    /// label, so it is the cleanest "obscuring shape" fixture for label_overlap.
    fn cloud(uid: i32, x: f64, y: f64) -> ViewElement {
        ViewElement::Cloud(view_element::Cloud {
            uid,
            flow_uid: -1,
            x,
            y,
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

    /// A flow valve at `(x, y)` with a two-point polyline whose endpoints attach
    /// to `from_uid` and `to_uid` (a stock--flow--stock segment). The point
    /// coordinates are irrelevant to `loop_compactness` (which uses node-box
    /// centers, not flow points), so they are placed at the valve.
    fn flow_between(
        uid: i32,
        name: &str,
        x: f64,
        y: f64,
        from_uid: i32,
        to_uid: i32,
    ) -> ViewElement {
        ViewElement::Flow(view_element::Flow {
            name: name.to_string(),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
            points: vec![
                view_element::FlowPoint {
                    x,
                    y,
                    attached_to_uid: Some(from_uid),
                },
                view_element::FlowPoint {
                    x,
                    y,
                    attached_to_uid: Some(to_uid),
                },
            ],
            compat: None,
            label_compat: None,
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

    // --- AC1.4: label_overlap (per-label obscuration) ---
    //
    // label_overlap is the SUM over labeled elements of each label's obscured
    // fraction: the area of the label box covered by any OTHER label box or any
    // OTHER element's bare shape box, capped at the label's own area and divided
    // by it (so each term is in [0,1]). 0 = no label obscured. A small overlap
    // registers at its true per-label obscuration fraction rather than being
    // diluted by the corpus's total label area (the old area/total definition's
    // under-counting; see `test_label_overlap_small_clip_is_sensitive`).

    #[test]
    fn test_label_overlap_overlapping_labels() {
        // Two auxes at the same position -> their labels (Bottom) coincide
        // exactly. Each label is fully covered by the other (capped at its own
        // area), so each obscured fraction is 1.0 and the sum is 2.0.
        let view = make_view(vec![
            aux(1, "samename", 100.0, 100.0),
            aux(2, "samename", 100.0, 100.0),
        ]);
        let m = compute_layout_metrics(&view, &cfg());
        assert!(
            (m.label_overlap - 2.0).abs() < 1e-9,
            "two coincident labels are each fully obscured: expected 2.0, got {}",
            m.label_overlap
        );
    }

    #[test]
    fn test_label_overlap_disjoint_is_zero() {
        // Two auxes far apart -> no label is covered by anything. Sum of
        // obscured fractions is 0.0.
        let view = make_view(vec![aux(1, "a", 0.0, 0.0), aux(2, "b", 1000.0, 1000.0)]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(m.label_overlap, 0.0);
    }

    #[test]
    fn test_label_overlap_counts_label_pair_exactly_once() {
        // The Phase-1 double-count guard, restated for per-label obscuration: a
        // label is never charged against its OWN element's shape box, and a
        // label-vs-label collision is counted from each label's own perspective
        // (both labels are unreadable -- that is intended), not via the other
        // node's label-merged bounds.
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
        // SHAPE boxes do NOT overlap (9 < 31), and each label clears the OTHER
        // aux's bare shape box entirely (label y [13,27] vs shape y [-9,9]). The
        // LABELS overlap by x:[11,29]=18, y:[13,27]=14 -> 252. Each label box has
        // area 58*14 = 812 and is covered only by the other label (252 < 812, no
        // cap), so each obscured fraction is 252/812 and the sum is 504/812.
        let view = make_view(vec![
            aux(1, "samename", 0.0, 0.0),
            aux(2, "samename", 40.0, 0.0),
        ]);
        let m = compute_layout_metrics(&view, &cfg());

        let label_area = 58.0 * 14.0; // 812.0
        let overlap = 18.0 * 14.0; // 252.0, the single label-label intersection
        let expected = (overlap / label_area) + (overlap / label_area); // 504/812
        assert!(
            (m.label_overlap - expected).abs() < 1e-9,
            "per-label obscuration should sum each label's fraction once: got {} expected {}",
            m.label_overlap,
            expected
        );
    }

    #[test]
    fn test_label_overlap_never_charged_against_own_shape() {
        // A single labeled aux: its Bottom label sits adjacent to (and partly
        // within the merged bounds of) its OWN shape. A label is never charged
        // against its own element's shape, and there is no other element, so the
        // obscured fraction is 0 and label_overlap is exactly 0.0.
        let view = make_view(vec![aux(1, "samename", 0.0, 0.0)]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(
            m.label_overlap, 0.0,
            "a label must never be charged against its own element's shape box"
        );
    }

    #[test]
    fn test_label_overlap_small_clip_is_sensitive() {
        // A small node SHAPE clipping a few characters of a short label must
        // register at its true per-label obscuration fraction, NOT be diluted to
        // ~0 by the corpus's total label area (the old area/total under-count).
        //
        // L: aux "ab" (2 chars) @ (0,0), Bottom label.
        //   editor_width = 2*6 + 10 = 22, height 14 -> label area 308.
        //   label box: left -11, right 11, top 13, bottom 27.
        // O: a cloud (no label) @ (18, 20). cloud_bounds (CLOUD_RADIUS 13.5):
        //   x [4.5, 31.5], y [6.5, 33.5].
        //   Overlap with L's label: x [4.5,11]=6.5, y [13,27]=14 -> 91.
        //   obscured_fraction(L) = 91/308 ~= 0.2955; the cloud has no label, so
        //   the sum is exactly 91/308.
        // Plus 15 far-apart auxes with long (20-char) labels: each label area
        //   20*6+10 = 130 wide * 14 = 1820, none overlapping anything. They add
        //   nothing to the per-label SUM (obscured fraction 0 each) but bloat the
        //   OLD denominator (total label area), so the OLD area/total score for
        //   the same clip collapses to ~0.003 -- the under-count this fixes.
        let mut elements = vec![aux(1, "ab", 0.0, 0.0), cloud(2, 18.0, 20.0)];
        for k in 0..15 {
            // Far apart on a 1000px grid so nothing overlaps; 20-char names.
            elements.push(aux(
                100 + k,
                "abcdefghijklmnopqrst",
                3000.0 + f64::from(k) * 1000.0,
                3000.0,
            ));
        }
        let view = make_view(elements);
        let m = compute_layout_metrics(&view, &cfg());

        let label_area = 22.0 * 14.0; // 308.0
        let clip_area = 6.5 * 14.0; // 91.0
        let expected = clip_area / label_area; // ~0.2955
        assert!(
            (m.label_overlap - expected).abs() < 1e-9,
            "small clip must score its per-label obscuration fraction: got {} expected {}",
            m.label_overlap,
            expected
        );
        assert!(
            m.label_overlap > 0.1,
            "a readability-killing clip must register clearly (> 0.1), got {}",
            m.label_overlap
        );

        // Confirm the OLD area/total definition would have under-counted this to
        // near-zero: the same clip area divided by the corpus total label area.
        let total_label_area = label_area + 15.0 * (130.0 * 14.0); // 308 + 27300
        let old_score = clip_area / total_label_area; // ~0.0033
        assert!(
            old_score < 0.01,
            "fixture must demonstrate the old under-count (< 0.01), got {}",
            old_score
        );
        assert!(
            m.label_overlap > old_score * 50.0,
            "new per-label score {} must be far larger than the old {}",
            m.label_overlap,
            old_score
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

    // --- AC5.1: the committed calibrated default expresses readability dominance ---
    //
    // The Phase-1 placeholder default was all-zeros (so a pre-calibration
    // `weighted_cost` was inert). Phase 4 commits real, user-signed-off weights
    // (2026-05-23), so the default is no longer all-zeros and `weighted_cost`
    // under it is now meaningful. This test pins the DOMINANCE ORDERING the
    // committed weights encode -- relationships rather than magic numbers, so it
    // documents the intent and survives minor retuning -- and re-confirms that
    // `weighted_cost` applies the default exactly as Σ wᵢ·termᵢ. It replaces the
    // old "default is all-zeros so cost is inert" assertion, which is no longer
    // true by design.

    #[test]
    fn test_default_weights_readability_dominant_ordering() {
        let w = MetricWeights::default();

        // The dominant "overlap + crossings" family: each term that hurts
        // readability (shapes overlapping shapes, connectors under shapes, labels
        // obscured, edges crossing) must outweigh every compactness/aspect term.
        let dominant = [
            w.node_overlap,
            w.node_connector_overlap,
            w.label_overlap,
            w.crossings,
        ];
        let compactness = [w.sprawl, w.edge_length_cv, w.aspect_penalty];
        for &d in &dominant {
            for &c in &compactness {
                assert!(
                    d > c,
                    "every readability term ({d}) must strictly exceed every \
                     compactness/aspect term ({c})"
                );
            }
        }

        // Compactness/aspect are intentionally zero: spreading out to keep labels
        // legible and feedback loops visible is good, not penalized.
        assert_eq!(w.sprawl, 0.0, "sprawl is not a goal");
        assert_eq!(
            w.edge_length_cv, 0.0,
            "edge-length uniformity is not a goal"
        );
        assert_eq!(w.aspect_penalty, 0.0, "aspect ratio is not a goal");

        // chain_straightness is reserved (not yet computed), so it carries no
        // weight.
        assert_eq!(
            w.chain_straightness, 0.0,
            "chain_straightness is reserved and must stay zero"
        );

        // loop_compactness rewards visible feedback-loop circles, but only as a
        // gentle nudge: a low, non-dominant weight strictly between zero and the
        // dominant family.
        assert!(
            w.loop_compactness > 0.0,
            "loop_compactness should gently reward visible loops, got {}",
            w.loop_compactness
        );
        assert!(
            w.loop_compactness < w.node_overlap,
            "loop_compactness ({}) must stay below the dominant node_overlap ({})",
            w.loop_compactness,
            w.node_overlap
        );

        // `weighted_cost` under the default is still the exact linear combination
        // (the default is now meaningful, not inert): verify against an explicit
        // Σ wᵢ·termᵢ over a hand-set metrics value.
        let m = LayoutMetrics {
            node_overlap: 0.3,
            node_connector_overlap: 0.1,
            label_overlap: 0.7,
            crossings: 2.0,
            sprawl: 5.0,
            edge_length_cv: 0.4,
            aspect_penalty: 1.5,
            chain_straightness: 0.0,
            loop_compactness: 0.8,
        };
        let expected = m.node_overlap * w.node_overlap
            + m.node_connector_overlap * w.node_connector_overlap
            + m.label_overlap * w.label_overlap
            + m.crossings * w.crossings
            + m.sprawl * w.sprawl
            + m.edge_length_cv * w.edge_length_cv
            + m.aspect_penalty * w.aspect_penalty
            + m.chain_straightness * w.chain_straightness
            + m.loop_compactness * w.loop_compactness;
        assert!(
            (m.weighted_cost(&w) - expected).abs() < 1e-12,
            "weighted_cost under the default must equal Σ wᵢ·termᵢ: got {} expected {}",
            m.weighted_cost(&w),
            expected
        );
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

    // --- loop_compactness (isoperimetric loop quality) ---

    /// The center of a node's bare shape box (which is symmetric about the
    /// element position, so this is the element center). Mirrors the centers the
    /// metric uses to build each loop polygon.
    fn shape_center(e: &ViewElement) -> Point {
        let r = node_shape_box(e).unwrap();
        Point {
            x: (r.left + r.right) / 2.0,
            y: (r.top + r.bottom) / 2.0,
        }
    }

    /// Hand-computed isoperimetric penalty `1 - Q` for a polygon over the given
    /// centers in order (shoelace area, summed-edge perimeter, Q clamped to
    /// [0,1]). The test's independent oracle for `loop_compactness`.
    fn expected_loop_penalty(centers: &[Point]) -> f64 {
        let n = centers.len();
        let mut area2 = 0.0;
        let mut perim = 0.0;
        for i in 0..n {
            let a = centers[i];
            let b = centers[(i + 1) % n];
            area2 += a.x * b.y - b.x * a.y;
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            perim += (dx * dx + dy * dy).sqrt();
        }
        let area = area2.abs() / 2.0;
        let q = (4.0 * std::f64::consts::PI * area / (perim * perim)).clamp(0.0, 1.0);
        1.0 - q
    }

    #[test]
    fn test_loop_compactness_circle_loop_near_zero() {
        // Eight stocks placed on a circle of radius 300, wired into a directed
        // 8-cycle by links 1->2->...->8->1. A well-spread loop reads as a clean
        // circle, so its isoperimetric quotient Q is close to 1 and the penalty
        // (1 - Q) is small.
        let n: i32 = 8;
        let radius = 300.0;
        let mut elements: Vec<ViewElement> = Vec::new();
        let mut centers: Vec<Point> = Vec::new();
        for i in 0..n {
            let theta = 2.0 * std::f64::consts::PI * f64::from(i) / f64::from(n);
            let x = radius * theta.cos();
            let y = radius * theta.sin();
            let e = stock(i + 1, "n", x, y);
            centers.push(shape_center(&e));
            elements.push(e);
        }
        for i in 0..n {
            let from = i + 1;
            let to = (i + 1) % n + 1;
            elements.push(straight_link(100 + i, from, to));
        }
        let view = make_view(elements);
        let m = compute_layout_metrics(&view, &cfg());

        let expected = expected_loop_penalty(&centers);
        assert!(
            (m.loop_compactness - expected).abs() < 1e-9,
            "loop_compactness {} != hand-computed penalty {}",
            m.loop_compactness,
            expected
        );
        // A regular octagon's penalty is ~0.05 -- "near 0" (a clean circle).
        assert!(
            m.loop_compactness < 0.1,
            "a well-spread circular loop should score near 0, got {}",
            m.loop_compactness
        );
    }

    #[test]
    fn test_loop_compactness_collapsed_loop_higher() {
        // The SAME directed 8-cycle, but the nodes are squished onto a nearly
        // straight line (a collapsed/collinear loop). The polygon area shrinks
        // toward zero while the perimeter stays large, so Q -> 0 and the penalty
        // (1 - Q) -> 1: clearly higher than the circular placement.
        let n: i32 = 8;
        let mut elements: Vec<ViewElement> = Vec::new();
        let mut centers: Vec<Point> = Vec::new();
        for i in 0..n {
            // Spread along x, with a tiny alternating y wobble so the polygon is
            // non-degenerate (nonzero perimeter) but nearly collinear.
            let x = f64::from(i) * 100.0;
            let y = if i % 2 == 0 { 0.0 } else { 1.0 };
            let e = stock(i + 1, "n", x, y);
            centers.push(shape_center(&e));
            elements.push(e);
        }
        for i in 0..n {
            let from = i + 1;
            let to = (i + 1) % n + 1;
            elements.push(straight_link(100 + i, from, to));
        }
        let view = make_view(elements);
        let m = compute_layout_metrics(&view, &cfg());

        let expected = expected_loop_penalty(&centers);
        assert!(
            (m.loop_compactness - expected).abs() < 1e-9,
            "loop_compactness {} != hand-computed penalty {}",
            m.loop_compactness,
            expected
        );
        // A nearly-collinear loop scores near 1 (squished).
        assert!(
            m.loop_compactness > 0.9,
            "a collapsed/collinear loop should score near 1, got {}",
            m.loop_compactness
        );
    }

    #[test]
    fn test_loop_compactness_no_cycle_is_zero() {
        // A pure chain a -> b -> c (no feedback) has no directed cycle, so there
        // is nothing to score: loop_compactness == 0.0.
        let view = make_view(vec![
            aux(1, "a", 0.0, 0.0),
            aux(2, "b", 200.0, 0.0),
            aux(3, "c", 400.0, 0.0),
            straight_link(10, 1, 2),
            straight_link(11, 2, 3),
        ]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(m.loop_compactness, 0.0);
    }

    #[test]
    fn test_loop_compactness_two_node_mutual_pair_is_zero() {
        // A 2-node mutual pair (a -> b -> a) is a cycle, but two points form no
        // polygon (fewer than 3 distinct nodes), so it contributes nothing.
        let view = make_view(vec![
            aux(1, "a", 0.0, 0.0),
            aux(2, "b", 200.0, 0.0),
            straight_link(10, 1, 2),
            straight_link(11, 2, 1),
        ]);
        let m = compute_layout_metrics(&view, &cfg());
        assert_eq!(m.loop_compactness, 0.0);
    }

    #[test]
    fn test_loop_compactness_flow_feedback_path_is_a_cycle() {
        // A stock--flow--stock feedback path must enter the loop graph: stock #1
        // and stock #2 connected by flow #3 (so #1 -> #3 -> #2), plus a link
        // #2 -> #1 closing the loop. The cycle is {#1, #3, #2}: three distinct
        // positioned nodes -> a real polygon -> a positive penalty.
        let s1 = stock(1, "a", 0.0, 0.0);
        let s2 = stock(2, "b", 300.0, 0.0);
        let f = flow_between(3, "f", 150.0, 200.0, 1, 2);
        let link = straight_link(10, 2, 1);
        let view = make_view(vec![s1, s2, f, link]);
        let m = compute_layout_metrics(&view, &cfg());
        assert!(
            m.loop_compactness > 0.0,
            "a stock--flow--stock feedback path must form a scored loop, got {}",
            m.loop_compactness
        );
    }

    /// A stock--flow--stock loop whose flow has an extra pipe point placed far
    /// from the valve, plus a closing link. The flow valve sits at `valve`; an
    /// interior pipe point at `bend` (between the two attached endpoints) bends
    /// the drawn pipe. `loop_compactness` must score the loop on the flow's
    /// VALVE (its visual center), NOT on `flow_shape_bounds`' pipe-extent bbox
    /// center, so the result must depend only on `valve` -- never on `bend`.
    fn bent_flow_loop_view(valve: Point, bend: Point) -> datamodel::StockFlow {
        let s1 = stock(1, "a", 0.0, 0.0);
        let s2 = stock(2, "b", 300.0, 0.0);
        let f = ViewElement::Flow(view_element::Flow {
            name: "f".to_string(),
            uid: 3,
            x: valve.x,
            y: valve.y,
            label_side: LabelSide::Bottom,
            points: vec![
                view_element::FlowPoint {
                    x: 0.0,
                    y: 0.0,
                    attached_to_uid: Some(1),
                },
                // An interior pipe point that bends the drawn pipe and stretches
                // `flow_shape_bounds`' bbox, but is NOT the valve.
                view_element::FlowPoint {
                    x: bend.x,
                    y: bend.y,
                    attached_to_uid: None,
                },
                view_element::FlowPoint {
                    x: 300.0,
                    y: 0.0,
                    attached_to_uid: Some(2),
                },
            ],
            compat: None,
            label_compat: None,
        });
        let link = straight_link(10, 2, 1);
        make_view(vec![s1, s2, f, link])
    }

    #[test]
    fn test_loop_compactness_scored_on_flow_valve_not_pipe_extent() {
        // The loop vertex for a flow must be its VALVE (the renderer's visual
        // center), not the center of `flow_shape_bounds` (which unions the valve
        // box with every pipe point). Extending the pipe with a far interior
        // point moves the pipe-extent bbox center but leaves the valve fixed, so
        // `loop_compactness` -- which scores the feedback-loop polygon -- must be
        // UNCHANGED. On the buggy (shape-box-midpoint) implementation it changes.
        let valve = Point { x: 150.0, y: 200.0 };

        // A pipe bend near the valve vs. one stretched far away. The valve is
        // identical in both, so the loop polygon (stock--valve--stock) is too.
        let near = compute_layout_metrics(
            &bent_flow_loop_view(valve, Point { x: 150.0, y: 210.0 }),
            &cfg(),
        );
        let far = compute_layout_metrics(
            &bent_flow_loop_view(
                valve,
                Point {
                    x: 150.0,
                    y: 2000.0,
                },
            ),
            &cfg(),
        );

        assert!(
            near.loop_compactness > 0.0,
            "fixture must form a real (positive-penalty) loop, got {}",
            near.loop_compactness
        );
        assert!(
            (near.loop_compactness - far.loop_compactness).abs() < 1e-12,
            "loop_compactness must score the flow VALVE, not the pipe-extent bbox \
             center: stretching the pipe changed it from {} to {}",
            near.loop_compactness,
            far.loop_compactness
        );

        // Non-vacuous guard: MOVING the valve (with the same pipe bend) DOES
        // change the loop polygon, so the metric is not trivially constant.
        let moved_valve = compute_layout_metrics(
            &bent_flow_loop_view(Point { x: 150.0, y: 400.0 }, Point { x: 150.0, y: 210.0 }),
            &cfg(),
        );
        assert!(
            (near.loop_compactness - moved_valve.loop_compactness).abs() > 1e-9,
            "moving the valve must change loop_compactness (test is not trivially \
             constant): {} vs {}",
            near.loop_compactness,
            moved_valve.loop_compactness
        );
    }

    #[test]
    fn test_loop_compactness_deterministic_under_shuffle() {
        // loop_compactness is a mean over cycles, each computed from node-box
        // centers in cycle order. It must be invariant to the order elements
        // appear in the view's element list.
        let n: i32 = 6;
        let radius = 250.0;
        let mut elements: Vec<ViewElement> = Vec::new();
        for i in 0..n {
            let theta = 2.0 * std::f64::consts::PI * f64::from(i) / f64::from(n);
            elements.push(stock(
                i + 1,
                "n",
                radius * theta.cos(),
                radius * theta.sin(),
            ));
        }
        for i in 0..n {
            let from = i + 1;
            let to = (i + 1) % n + 1;
            elements.push(straight_link(100 + i, from, to));
        }
        let base = compute_layout_metrics(&make_view(elements.clone()), &cfg());

        // Reverse the element order (links before nodes, nodes reversed); the
        // graph and its cycles are unchanged.
        let mut shuffled = elements.clone();
        shuffled.reverse();
        let other = compute_layout_metrics(&make_view(shuffled), &cfg());

        assert!(
            (base.loop_compactness - other.loop_compactness).abs() < 1e-12,
            "loop_compactness changed under element shuffle: {} vs {}",
            base.loop_compactness,
            other.loop_compactness
        );
        assert!(base.loop_compactness > 0.0);
    }

    // --- AC5.2: human-vs-auto reference-pair ordering under the committed weights ---
    //
    // The committed `MetricWeights::default()` must agree with the user's visual
    // taste: on the agreed reference pairs the SHIPPED, hand-authored ("human")
    // layout must score a lower `weighted_cost` than a machine-generated
    // ("auto") layout of the SAME model. This is the objective validation of the
    // calibration (Phase 4, AC5.2): if the metric and the weights did not agree
    // with human taste on an obvious pair, the metric or the pair would be wrong.
    //
    // Construction (b) -- "human view vs generated layout" (design glossary): the
    // four `default_projects` models each ship a hand-authored main view. We
    // score that as-loaded view (human) and a fixed-seed `generate_layout_with_config`
    // layout (auto) of the same model, and assert `human < auto`.
    //
    // Determinism + budget: layout is deterministic per seed (fix #633), so ONE
    // fixed seed (not `generate_best_layout`'s multi-seed search) makes the test
    // reproducible AND fast. The four default_projects are small (<= 42
    // elements), so a single layout generation each is well under the per-test
    // budget.
    //
    // Anchors: reliability, fishbanks, population, dp(logistic-growth). These all
    // flip the right way under the committed weights (verified during
    // calibration). `sir` is deliberately NOT a human<auto anchor -- its shipped
    // reference genuinely obscures more labels than the auto layout, so the
    // metric correctly prefers the auto; that direction is pinned separately by
    // `test_sir_auto_beats_reference_under_default_weights` so the asymmetry is
    // documented rather than silently dropped.

    /// A fixed annealing seed for the auto layout. Any single fixed seed makes the
    /// test deterministic; 42 matches the convention used elsewhere in the layout
    /// config.
    const REF_PAIR_SEED: u64 = 42;

    /// Load a `default_projects` XMILE model by directory name, resolving the path
    /// against `CARGO_MANIFEST_DIR` (= `src/simlin-engine`) like the layout
    /// integration tests. Panics with a clear message on any I/O or parse failure
    /// (a missing fixture is a test-environment bug, not a metric result).
    fn load_default_project(dir: &str) -> datamodel::Project {
        let path = format!(
            "{}/../../default_projects/{}/model.xmile",
            env!("CARGO_MANIFEST_DIR"),
            dir
        );
        let file =
            std::fs::File::open(&path).unwrap_or_else(|e| panic!("failed to open {path}: {e}"));
        let mut reader = std::io::BufReader::new(file);
        crate::compat::open_xmile(&mut reader)
            .unwrap_or_else(|e| panic!("failed to parse {path}: {e:?}"))
    }

    /// The model's as-loaded, hand-authored main `StockFlow` view (the "human"
    /// reference). Panics if the model has no such view -- every chosen anchor
    /// ships one, so its absence is a fixture regression.
    fn human_view(project: &datamodel::Project) -> datamodel::StockFlow {
        let model = project
            .get_model("main")
            .expect("anchor model must have a 'main' model");
        match model.views.first() {
            Some(datamodel::View::StockFlow(sf)) if !sf.elements.is_empty() => sf.clone(),
            _ => panic!("anchor model must ship a non-empty hand-authored main view"),
        }
    }

    /// `weighted_cost` of the shipped human layout under the committed default
    /// weights.
    fn human_cost(project: &datamodel::Project) -> f64 {
        let view = human_view(project);
        compute_layout_metrics(&view, &LayoutConfig::default())
            .weighted_cost(&MetricWeights::default())
    }

    /// `weighted_cost` of a single fixed-seed generated layout under the committed
    /// default weights. Deterministic per seed, so the score is reproducible.
    fn auto_cost(project: &datamodel::Project) -> f64 {
        let cfg = LayoutConfig {
            annealing_random_seed: REF_PAIR_SEED,
            ..LayoutConfig::default()
        };
        let view = crate::layout::generate_layout_with_config(project, "main", cfg.clone(), None)
            .expect("auto layout generation must succeed for the anchor model");
        compute_layout_metrics(&view, &cfg).weighted_cost(&MetricWeights::default())
    }

    /// Assert the human reference beats the auto layout for one anchor model,
    /// naming the model and both costs on failure (so a calibration regression is
    /// immediately legible).
    fn assert_human_beats_auto(dir: &str) {
        let project = load_default_project(dir);
        let human = human_cost(&project);
        let auto = auto_cost(&project);
        assert!(
            human < auto,
            "reference pair {dir}: expected human_cost ({human}) < auto_cost ({auto}) \
             under MetricWeights::default()"
        );
    }

    #[test]
    fn test_reference_pair_reliability_human_beats_auto() {
        assert_human_beats_auto("reliability");
    }

    #[test]
    fn test_reference_pair_fishbanks_human_beats_auto() {
        assert_human_beats_auto("fishbanks");
    }

    // Population is a MARGINAL taste anchor: under the committed default weights
    // its human cost (~0.0521) beats auto (~0.0533) by only ~2.3%, far thinner
    // than the other anchors (reliability ~8.5%, fishbanks ~12%,
    // logistic-growth ~58%). The layout is deterministic per seed, so the
    // assertion is not flaky -- but if it ever fails it should be read as
    // "population sits near the boundary" rather than necessarily a real metric
    // regression. The robust signal lives in reliability/fishbanks/logistic-growth.
    #[test]
    fn test_reference_pair_population_human_beats_auto() {
        assert_human_beats_auto("population");
    }

    #[test]
    fn test_reference_pair_dp_logistic_growth_human_beats_auto() {
        assert_human_beats_auto("logistic-growth");
    }

    #[test]
    fn test_sir_auto_beats_reference_under_default_weights() {
        // The documented NON-anchor: SIR's shipped reference obscures more labels
        // than the auto layout, so the metric correctly prefers the auto. This
        // pins that direction so the asymmetry (why SIR is excluded from the
        // human<auto anchors) is recorded rather than silently assumed.
        let path = format!(
            "{}/../../test/test-models/samples/SIR/SIR.stmx",
            env!("CARGO_MANIFEST_DIR")
        );
        let file =
            std::fs::File::open(&path).unwrap_or_else(|e| panic!("failed to open {path}: {e}"));
        let mut reader = std::io::BufReader::new(file);
        let project = crate::compat::open_xmile(&mut reader)
            .unwrap_or_else(|e| panic!("failed to parse {path}: {e:?}"));

        let human = human_cost(&project);
        let auto = auto_cost(&project);
        assert!(
            auto < human,
            "sir is a documented non-anchor: expected auto_cost ({auto}) < human_cost ({human}) \
             under MetricWeights::default() (its reference obscures more labels than the auto)"
        );
    }
}
