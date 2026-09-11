// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// The layout quality core. Every term is computed purely from a
// `datamodel::StockFlow`, over the SCENE the renderer draws: node shapes at
// their drawn size (a flow valve is the 9px circle `render_flow` draws, not
// its smaller bounds box), flow pipes as 4px-thick segments, connectors as the
// exact polylines `diagram::connector` draws, and labels at the boxes
// `diagram::label` measures. A layout's score therefore can never disagree
// with what the picture shows.
//
// Every defect term is a RATE -- a mean over the elements, labels, or
// connectors it concerns -- so a model's cost does not grow with its size, the
// trade-off between terms is the same for a 10-variable model as for a
// 300-variable one, and the corpus aggregate is not dominated by the largest
// models. `analyze_layout` additionally reports each defect's location, so the
// eval harness can draw what the metric sees over the rendered diagram.
//
// There is NO I/O in this module: it takes data, computes scalars, returns
// them. That makes every term testable with hand-computed expected values (see
// `metrics_tests.rs`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::datamodel::{self, ViewElement};
use crate::diagram::common::{
    self, Point, Rect, display_name, merge_bounds, rect_area, rect_overlap_area,
    segment_clip_interval_in_rect,
};
use crate::diagram::connector::{ARC_POLYLINE_SAMPLES, connector_polyline, get_visual_center};
use crate::diagram::elements::{
    aux_shape_bounds, cloud_bounds, module_shape_bounds, stock_shape_bounds,
};
use crate::diagram::label::{LabelProps, label_bounds};

use super::annealing::segment_intersection;
use super::build_view_segments;
use super::config::LayoutConfig;

/// Upper bound of the target aspect-ratio band. A view whose bounding-box
/// aspect ratio (long side / short side, always >= 1) is at or below this value
/// is "well-proportioned" and incurs no `aspect_penalty`.
pub const TARGET_AR_MAX: f64 = 16.0 / 9.0;

/// Half the drawn width of a flow pipe: the renderer strokes the pipe's outer
/// path 4px wide.
pub(crate) const PIPE_HALF_WIDTH: f64 = 2.0;

/// The gap between two element footprints (shape or label boxes) below which
/// they read as jammed together: enough air that two labels, or a label and a
/// neighbor's shape, read as separate marks. Hand-drawn diagrams routinely
/// leave less than a text line between neighbors, so the threshold sits well
/// under one line's height and the deficit is squared, charging marks that
/// nearly touch far more than ones that are merely snug.
pub(crate) const COMFORTABLE_CLEARANCE: f64 = 8.0;

/// A link whose drawn length outside its two endpoint shapes is below this
/// cannot show its arrowhead's direction and reads as the nodes touching.
const MIN_VISIBLE_LINK: f64 = 20.0;

/// A link longer than this many times the view's median link length reads as
/// a line across the diagram rather than a local connection.
const LONG_CONNECTOR_FACTOR: f64 = 3.0;

/// How far inside a label box a connector must pass to be charged as crossing
/// the text: `label_bounds` pads the text horizontally, and a line grazing
/// that padding does not obscure anything.
const LABEL_INSET: f64 = 2.0;

/// How much of a line through a name counts when the line is the name's own
/// node's link (see `label_connector_overlap`).
const OWN_LINK_STRIKE_FACTOR: f64 = 0.5;

/// Two node centers within this distance on one axis share a row or column.
const ALIGN_TOLERANCE: f64 = 3.0;

/// How far apart two nodes may be and still count as aligned with each other:
/// alignment is a local reading aid, not a property of distant nodes that
/// happen to share a coordinate.
const ALIGN_REACH: f64 = 300.0;

/// One quality cost per aesthetic concern, with `0.0` always meaning "ideal".
///
/// The defect terms are rates (means over the things they concern), so they
/// are comparable across models of different size. Several terms are
/// intentionally sensitive to absolute coordinate scale relative to the fixed
/// pixel size of shapes and labels (`node_overlap`, `label_overlap`,
/// `crowding`, `sprawl`): packing nodes tightly against those fixed sizes is
/// exactly what they measure.
///
/// `Serialize`/`Deserialize` let the layout-quality eval sweep
/// (`examples/layout_eval/`) emit the per-term breakdown into its artifacts
/// and read a stored report back for comparison. Terms added after a report
/// was written deserialize as `0.0`.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayoutMetrics {
    /// Mean over nodes of the fraction of each node's drawn shape covered by
    /// other nodes' shapes (capped at 1 per node).
    pub node_overlap: f64,
    /// Fraction of total connector length that passes under a non-incident
    /// node shape or flow pipe: a connector under a shape reads as a false
    /// causal connection.
    pub node_connector_overlap: f64,
    /// Mean over labels of the fraction of each label box covered by other
    /// labels and other nodes' shapes (capped at 1).
    pub label_overlap: f64,
    /// Mean over labels of how much connector -- link or flow pipe -- passes
    /// through the label's text box, relative to the box's smaller side (capped
    /// at 1): a line through a name strikes it out, however thin. The label's
    /// own node's links count at `OWN_LINK_STRIKE_FACTOR`; a flow's own pipe
    /// never strikes its name.
    #[serde(default)]
    pub label_connector_overlap: f64,
    /// Edge crossings per connector.
    pub crossings: f64,
    /// Mean over nodes of the clearance deficit to their neighbors -- for each
    /// pair of nodes whose footprints (shape and label boxes) come closer than
    /// `COMFORTABLE_CLEARANCE`, `(1 - gap/clearance)^2` -- plus the mean over
    /// links of the same deficit for links whose visible length is below
    /// `MIN_VISIBLE_LINK`. The counterweight to `sprawl`: without it, the
    /// cheapest layout is the most crowded one that does not quite overlap.
    #[serde(default)]
    pub crowding: f64,
    /// Mean connector length relative to the characteristic node size.
    pub sprawl: f64,
    /// Mean over links of how far each exceeds `LONG_CONNECTOR_FACTOR` times
    /// the median link length, in multiples of that threshold: a parameter
    /// parked across the diagram from its consumer.
    #[serde(default)]
    pub long_connectors: f64,
    /// Coefficient of variation (stddev/mean) of connector lengths.
    pub edge_length_cv: f64,
    /// How far the view bounding-box aspect ratio exceeds the target band.
    pub aspect_penalty: f64,
    /// Fraction of nodes that share neither a row nor a column (within
    /// `ALIGN_TOLERANCE`) with any node within `ALIGN_REACH`.
    #[serde(default)]
    pub misalignment: f64,
    /// Mean isoperimetric penalty `1 - Q` over the view's feedback cycles
    /// (`Q = 4*PI*Area / Perimeter^2` of each loop's node-center polygon,
    /// clamped to [0,1]). 0.0 = clean, well-spread loops (circles); higher =
    /// collapsed/collinear loops. 0.0 when the view has no cycle of >= 3 nodes.
    pub loop_compactness: f64,
    /// Mean number of right-angle bends per flow pipe (a straight pipe has 0, an
    /// `L` has 1, a `Z` has 2). Flows are orthogonalized before scoring, so this
    /// rewards placements where the two stocks a flow connects are naturally
    /// aligned over diagonally-offset stocks that require an `L`/`Z` detour.
    #[serde(default)]
    pub flow_bends: f64,
    /// Mean bow shortfall over the causal connectors that participate in a
    /// feedback loop: 0.0 = every loop connector is drawn with at least the
    /// target curvature (the loop reads as a visible circle), 1.0 = loop
    /// connectors are straight (the loop collapses to a zig-zag).
    #[serde(default)]
    pub loop_straightness: f64,
}

/// Per-term weights for the scalar an optimizer minimizes.
///
/// `MetricWeights::default()` holds the calibrated production weights (see the
/// rationale on the `Default` impl). Weights a stored report predates
/// deserialize as `0.0`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetricWeights {
    pub node_overlap: f64,
    pub node_connector_overlap: f64,
    pub label_overlap: f64,
    #[serde(default)]
    pub label_connector_overlap: f64,
    pub crossings: f64,
    #[serde(default)]
    pub crowding: f64,
    pub sprawl: f64,
    #[serde(default)]
    pub long_connectors: f64,
    pub edge_length_cv: f64,
    pub aspect_penalty: f64,
    #[serde(default)]
    pub misalignment: f64,
    pub loop_compactness: f64,
    #[serde(default)]
    pub flow_bends: f64,
    #[serde(default)]
    pub loop_straightness: f64,
}

impl Default for MetricWeights {
    /// The calibrated production weights.
    ///
    /// Every defect term is a rate, so a weight is the cost of that defect
    /// affecting EVERY element it concerns; a single defect in a model of `n`
    /// elements costs `weight / n`. The weights encode how much one instance of
    /// each defect hurts relative to the others:
    ///   * Illegibility dominates. A node covering a node, a label covered by
    ///     something, a connector under a shape (a false causal link), and a
    ///     line through a name each destroy information outright.
    ///   * A crossing costs less than an obscured label (the reader can follow
    ///     a line across another) but more than mild crowding.
    ///   * `crowding` and `sprawl` pull in opposite directions and together set
    ///     the finite optimum spacing: spread until neighbors have air, no
    ///     further.
    ///   * `long_connectors` charges the one parameter parked across the
    ///     diagram, which the mean-length `sprawl` barely registers.
    ///   * Loop and flow conventions (`loop_compactness`, `flow_bends`,
    ///     `loop_straightness`) and alignment (`misalignment`) are gentle
    ///     nudges toward how modelers draw.
    ///   * `edge_length_cv` and `aspect_penalty` are reported for diagnosis and
    ///     carry no weight.
    ///
    /// Calibrated against the eval harness's judged pairs (every taste-battery
    /// degradation of the corpus references and production layouts, plus
    /// visual reference-vs-production judgments): a log-space fit anchored at
    /// `crossings = 1` moved these by under 15%, so the values are rounded
    /// priors the data confirms rather than a fit to a handful of models.
    fn default() -> Self {
        MetricWeights {
            node_overlap: 3.5,
            node_connector_overlap: 2.0,
            label_overlap: 3.5,
            label_connector_overlap: 1.5,
            crossings: 1.0,
            crowding: 1.0,
            sprawl: 0.25,
            long_connectors: 0.5,
            edge_length_cv: 0.0,
            aspect_penalty: 0.0,
            misalignment: 0.1,
            loop_compactness: 0.4,
            flow_bends: 0.15,
            loop_straightness: 0.1,
        }
    }
}

impl MetricWeights {
    /// Every weight zero: the base for isolating one or a few terms
    /// (`MetricWeights { crossings: 1.0, ..MetricWeights::zero() }`).
    pub const fn zero() -> Self {
        MetricWeights {
            node_overlap: 0.0,
            node_connector_overlap: 0.0,
            label_overlap: 0.0,
            label_connector_overlap: 0.0,
            crossings: 0.0,
            crowding: 0.0,
            sprawl: 0.0,
            long_connectors: 0.0,
            edge_length_cv: 0.0,
            aspect_penalty: 0.0,
            misalignment: 0.0,
            loop_compactness: 0.0,
            flow_bends: 0.0,
            loop_straightness: 0.0,
        }
    }
}

impl LayoutMetrics {
    /// Sigma w_i * term_i -- the scalar an optimizer minimizes.
    pub fn weighted_cost(&self, w: &MetricWeights) -> f64 {
        self.node_overlap * w.node_overlap
            + self.node_connector_overlap * w.node_connector_overlap
            + self.label_overlap * w.label_overlap
            + self.label_connector_overlap * w.label_connector_overlap
            + self.crossings * w.crossings
            + self.crowding * w.crowding
            + self.sprawl * w.sprawl
            + self.long_connectors * w.long_connectors
            + self.edge_length_cv * w.edge_length_cv
            + self.aspect_penalty * w.aspect_penalty
            + self.misalignment * w.misalignment
            + self.loop_compactness * w.loop_compactness
            + self.flow_bends * w.flow_bends
            + self.loop_straightness * w.loop_straightness
    }

    /// `(name, value)` for every term, in a stable display order.
    pub fn terms(&self) -> [(&'static str, f64); 14] {
        [
            ("node_overlap", self.node_overlap),
            ("node_connector_overlap", self.node_connector_overlap),
            ("label_overlap", self.label_overlap),
            ("label_connector_overlap", self.label_connector_overlap),
            ("crossings", self.crossings),
            ("crowding", self.crowding),
            ("sprawl", self.sprawl),
            ("long_connectors", self.long_connectors),
            ("edge_length_cv", self.edge_length_cv),
            ("aspect_penalty", self.aspect_penalty),
            ("misalignment", self.misalignment),
            ("loop_compactness", self.loop_compactness),
            ("flow_bends", self.flow_bends),
            ("loop_straightness", self.loop_straightness),
        ]
    }
}

impl MetricWeights {
    /// `(name, weight)` for every weight, in the same order as
    /// [`LayoutMetrics::terms`].
    pub fn terms(&self) -> [(&'static str, f64); 14] {
        [
            ("node_overlap", self.node_overlap),
            ("node_connector_overlap", self.node_connector_overlap),
            ("label_overlap", self.label_overlap),
            ("label_connector_overlap", self.label_connector_overlap),
            ("crossings", self.crossings),
            ("crowding", self.crowding),
            ("sprawl", self.sprawl),
            ("long_connectors", self.long_connectors),
            ("edge_length_cv", self.edge_length_cv),
            ("aspect_penalty", self.aspect_penalty),
            ("misalignment", self.misalignment),
            ("loop_compactness", self.loop_compactness),
            ("flow_bends", self.flow_bends),
            ("loop_straightness", self.loop_straightness),
        ]
    }
}

/// What kind of defect a [`Defect`] marks, one per defect term.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefectKind {
    NodeOverlap,
    ConnectorThroughNode,
    LabelObscured,
    LabelCrossed,
    Crossing,
    Crowded,
    LongConnector,
}

/// One defect the metric charged, located on the diagram: the region it
/// concerns (`[left, top, right, bottom]`; a point is a zero-size region) and
/// its severity in the term's own units (a covered fraction, a clearance
/// deficit, an excess ratio).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Defect {
    pub kind: DefectKind,
    pub region: [f64; 4],
    pub severity: f64,
}

/// The metrics of a view together with every defect behind them.
pub struct LayoutAnalysis {
    pub metrics: LayoutMetrics,
    pub defects: Vec<Defect>,
}

/// Where defects go while the terms are computed: nowhere (the hot path an
/// optimizer runs) or into a list (the eval harness's overlays). One code path
/// computes both, so the overlay can never disagree with the score.
struct DefectSink {
    defects: Option<Vec<Defect>>,
}

impl DefectSink {
    fn push(&mut self, kind: DefectKind, region: Rect, severity: f64) {
        if let Some(d) = &mut self.defects {
            d.push(Defect {
                kind,
                region: [region.left, region.top, region.right, region.bottom],
                severity,
            });
        }
    }

    fn push_point(&mut self, kind: DefectKind, p: Point, severity: f64) {
        self.push(
            kind,
            Rect {
                left: p.x,
                top: p.y,
                right: p.x,
                bottom: p.y,
            },
            severity,
        );
    }
}

// --- the drawn scene -----------------------------------------------------------

/// The element's primary drawn *shape* box, WITHOUT its label: the circle or
/// rectangle the renderer draws for it. A flow's shape is its valve circle
/// (`render_flow` draws radius `AUX_RADIUS`); its pipe is separate geometry
/// ([`pipe_rects`]). An alias draws an aux-sized circle. Links and groups have
/// no shape.
pub(crate) fn node_shape_box(element: &ViewElement) -> Option<Rect> {
    use crate::diagram::constants::AUX_RADIUS;
    match element {
        ViewElement::Aux(a) => Some(aux_shape_bounds(a)),
        ViewElement::Stock(s) => Some(stock_shape_bounds(s)),
        ViewElement::Module(m) => Some(module_shape_bounds(m)),
        ViewElement::Cloud(c) => Some(cloud_bounds(c)),
        ViewElement::Flow(f) => Some(circle_box(f.x, f.y, AUX_RADIUS)),
        ViewElement::Alias(a) => Some(alias_shape_box(a)),
        ViewElement::Link(_) | ViewElement::Group(_) => None,
    }
}

fn circle_box(cx: f64, cy: f64, r: f64) -> Rect {
    Rect {
        left: cx - r,
        right: cx + r,
        top: cy - r,
        bottom: cy + r,
    }
}

/// A flow's pipe as drawn: one box per segment, inflated by the stroke's half
/// width, so the pipe covers what it visibly covers. An axis-aligned segment
/// (the orthogonalized pipes the layout produces) is covered exactly.
pub(crate) fn pipe_rects(flow: &datamodel::view_element::Flow) -> Vec<Rect> {
    flow.points
        .windows(2)
        .map(|w| Rect {
            left: w[0].x.min(w[1].x) - PIPE_HALF_WIDTH,
            right: w[0].x.max(w[1].x) + PIPE_HALF_WIDTH,
            top: w[0].y.min(w[1].y) - PIPE_HALF_WIDTH,
            bottom: w[0].y.max(w[1].y) + PIPE_HALF_WIDTH,
        })
        .collect()
}

/// The bare shape box of an alias: the aux-radius circle `render_alias` draws,
/// centered on the alias position.
pub(crate) fn alias_shape_box(alias: &crate::datamodel::view_element::Alias) -> Rect {
    use crate::diagram::constants::AUX_RADIUS;
    circle_box(alias.x, alias.y, AUX_RADIUS)
}

/// The label an alias renders: its SOURCE element's display name (resolved
/// through `alias_of_uid`), positioned like an aux label.
pub(crate) fn alias_label_props_for(
    alias: &crate::datamodel::view_element::Alias,
    source_name: &str,
    side: crate::datamodel::view_element::LabelSide,
) -> LabelProps {
    use crate::diagram::constants::AUX_RADIUS;
    LabelProps::new(alias.x, alias.y, side, display_name(source_name))
        .with_radii(AUX_RADIUS, AUX_RADIUS)
}

/// Map each alias uid in `elements` to its source element's name. Aliases whose
/// `alias_of_uid` does not resolve to a named element are omitted (dangling).
pub(crate) fn alias_source_names(elements: &[ViewElement]) -> HashMap<i32, String> {
    let names: HashMap<i32, &str> = elements
        .iter()
        .filter_map(|e| e.get_name().map(|n| (e.get_uid(), n)))
        .collect();
    elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Alias(a) => names
                .get(&a.alias_of_uid)
                .map(|n| (a.uid, (*n).to_string())),
            _ => None,
        })
        .collect()
}

/// Build a `LabelProps` for a labeled element placed on `side`, matching the
/// renderer's label geometry (center, display name, and the radii the element
/// renders its label with). Only elements that render their own name return
/// `Some`; an alias's label needs its source's name (see
/// [`alias_label_props_for`]).
///
/// Exposed `pub(crate)` so the declutter pass (`layout::declutter`) can probe
/// the label box an element *would* occupy on an alternative side, scoring
/// against the identical geometry this metric uses.
pub(crate) fn element_label_props_for(
    element: &ViewElement,
    side: crate::datamodel::view_element::LabelSide,
) -> Option<LabelProps> {
    use crate::diagram::constants::{
        AUX_RADIUS, MODULE_HEIGHT, MODULE_WIDTH, STOCK_HEIGHT, STOCK_WIDTH,
    };
    match element {
        ViewElement::Aux(a) => Some(
            LabelProps::new(a.x, a.y, side, display_name(&a.name))
                .with_radii(AUX_RADIUS, AUX_RADIUS),
        ),
        ViewElement::Stock(s) => Some(
            LabelProps::new(s.x, s.y, side, display_name(&s.name))
                .with_radii(STOCK_WIDTH / 2.0, STOCK_HEIGHT / 2.0),
        ),
        ViewElement::Module(m) => Some(
            LabelProps::new(m.x, m.y, side, display_name(&m.name))
                .with_radii(MODULE_WIDTH / 2.0, MODULE_HEIGHT / 2.0),
        ),
        // `render_flow` places a flow's label around the valve's drawn radius.
        ViewElement::Flow(f) => Some(
            LabelProps::new(f.x, f.y, side, display_name(&f.name))
                .with_radii(AUX_RADIUS, AUX_RADIUS),
        ),
        ViewElement::Alias(_)
        | ViewElement::Link(_)
        | ViewElement::Cloud(_)
        | ViewElement::Group(_) => None,
    }
}

/// The element's own current label side, or `None` for kinds that render no
/// label of their own name (the set `element_label_props_for` returns `Some`
/// for, plus aliases).
fn element_label_side(element: &ViewElement) -> Option<crate::datamodel::view_element::LabelSide> {
    match element {
        ViewElement::Aux(a) => Some(a.label_side),
        ViewElement::Stock(s) => Some(s.label_side),
        ViewElement::Module(m) => Some(m.label_side),
        ViewElement::Flow(f) => Some(f.label_side),
        ViewElement::Alias(a) => Some(a.label_side),
        ViewElement::Link(_) | ViewElement::Cloud(_) | ViewElement::Group(_) => None,
    }
}

/// One node of the drawn scene.
struct SceneNode {
    uid: i32,
    shape: Rect,
    label: Option<Rect>,
    /// A flow's pipe boxes; empty for every other node.
    pipe: Vec<Rect>,
    /// The uids a flow's pipe attaches to (stocks, clouds); empty otherwise.
    attached: Vec<i32>,
    /// The renderer's visual center.
    center: Point,
    /// A cloud is a flow's decorative source or sink: a light mark whose
    /// proximity to anything is not crowding (landing ON something is still
    /// an overlap).
    is_cloud: bool,
}

impl SceneNode {
    /// Whether this node and `other` are joined by construction (a flow and the
    /// stock or cloud its pipe attaches to), so their adjacency is not a
    /// layout defect.
    fn attached_to(&self, other: &SceneNode) -> bool {
        self.attached.contains(&other.uid) || other.attached.contains(&self.uid)
    }

    /// The label-merged box: shape and label together.
    fn footprint_box(&self) -> Rect {
        match self.label {
            Some(l) => merge_bounds(self.shape, l),
            None => self.shape,
        }
    }
}

fn build_scene_nodes(elements: &[ViewElement]) -> Vec<SceneNode> {
    let alias_names = alias_source_names(elements);
    let not_arrayed = |_: &str| false;
    elements
        .iter()
        .filter_map(|e| {
            let shape = node_shape_box(e)?;
            let label = match e {
                ViewElement::Alias(a) => alias_names
                    .get(&a.uid)
                    .map(|name| label_bounds(&alias_label_props_for(a, name, a.label_side))),
                _ => element_label_side(e)
                    .and_then(|side| element_label_props_for(e, side))
                    .map(|props| label_bounds(&props)),
            };
            let (pipe, attached) = match e {
                ViewElement::Flow(f) => (
                    pipe_rects(f),
                    f.points.iter().filter_map(|p| p.attached_to_uid).collect(),
                ),
                _ => (Vec::new(), Vec::new()),
            };
            let (cx, cy) = get_visual_center(e, &not_arrayed);
            Some(SceneNode {
                uid: e.get_uid(),
                shape,
                label,
                pipe,
                attached,
                center: Point { x: cx, y: cy },
                is_cloud: matches!(e, ViewElement::Cloud(_)),
            })
        })
        .collect()
}

/// Whether a connector is a causal link or a flow pipe.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectorKind {
    Link,
    Pipe,
}

/// The drawn geometry of one connector (Link or Flow pipe): its incident node
/// uids (so overlap terms skip them) and the polyline the renderer draws.
struct ConnectorGeometry {
    kind: ConnectorKind,
    /// A link's two endpoints; a pipe's flow and the stocks and clouds it
    /// attaches to.
    incident_uids: HashSet<i32>,
    /// The flow a pipe belongs to; `None` for a link.
    flow_uid: Option<i32>,
    /// Always at least two points (connectors that draw nothing are omitted).
    polyline: Vec<Point>,
    length: f64,
}

/// Total length of the UNION of parameter intervals `[t0, t1]` (each `t` in
/// [0,1]), counting each covered sub-length once. Mutates `intervals` (sorts in
/// place); empty input yields 0.0.
fn merged_interval_length(intervals: &mut [(f64, f64)]) -> f64 {
    if intervals.is_empty() {
        return 0.0;
    }
    intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut total = 0.0;
    let mut cur = intervals[0];
    for &(t0, t1) in &intervals[1..] {
        if t0 <= cur.1 {
            cur.1 = cur.1.max(t1);
        } else {
            total += cur.1 - cur.0;
            cur = (t0, t1);
        }
    }
    total += cur.1 - cur.0;
    total
}

/// Polyline length: sum of segment lengths.
fn polyline_length(points: &[Point]) -> f64 {
    points
        .windows(2)
        .map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt())
        .sum()
}

/// Collect the drawn geometry of every connector that draws something. Links
/// use the shared `connector_polyline` (the exact geometry the renderer draws
/// and `build_view_segments` counts); flows use their point polyline.
fn collect_connector_geometry(elements: &[ViewElement]) -> Vec<ConnectorGeometry> {
    let uid_elements: HashMap<i32, &ViewElement> =
        elements.iter().map(|e| (e.get_uid(), e)).collect();
    let not_arrayed = |_: &str| false;

    let mut out = Vec::new();
    for elem in elements {
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
                out.push(ConnectorGeometry {
                    kind: ConnectorKind::Link,
                    incident_uids: HashSet::from([link.from_uid, link.to_uid]),
                    flow_uid: None,
                    length: polyline_length(&polyline),
                    polyline,
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
                let mut incident_uids = HashSet::from([flow.uid]);
                incident_uids.extend(flow.points.iter().filter_map(|p| p.attached_to_uid));
                out.push(ConnectorGeometry {
                    kind: ConnectorKind::Pipe,
                    incident_uids,
                    flow_uid: Some(flow.uid),
                    length: polyline_length(&polyline),
                    polyline,
                });
            }
            _ => {}
        }
    }
    out
}

/// Separation distance between two rects (0 when they touch or overlap).
fn rect_gap(a: &Rect, b: &Rect) -> f64 {
    let dx = (a.left - b.right).max(b.left - a.right).max(0.0);
    let dy = (a.top - b.bottom).max(b.top - a.bottom).max(0.0);
    (dx * dx + dy * dy).sqrt()
}

fn rect_intersection(a: &Rect, b: &Rect) -> Rect {
    Rect {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    }
}

fn inset(r: &Rect, d: f64) -> Rect {
    Rect {
        left: r.left + d,
        top: r.top + d,
        right: r.right - d,
        bottom: r.bottom - d,
    }
}

// --- the defect terms ------------------------------------------------------------

/// `node_overlap`: mean covered fraction of each node's shape.
fn node_overlap_term(nodes: &[SceneNode], sink: &mut DefectSink) -> f64 {
    if nodes.is_empty() {
        return 0.0;
    }
    let mut total = 0.0;
    for (i, a) in nodes.iter().enumerate() {
        let area = rect_area(&a.shape);
        if area <= 0.0 {
            continue;
        }
        let mut covered = 0.0;
        for (j, b) in nodes.iter().enumerate() {
            if i == j {
                continue;
            }
            let o = rect_overlap_area(&a.shape, &b.shape);
            if o > 0.0 {
                covered += o;
                if i < j {
                    sink.push(
                        DefectKind::NodeOverlap,
                        rect_intersection(&a.shape, &b.shape),
                        o / area.min(rect_area(&b.shape)).max(1e-9),
                    );
                }
            }
        }
        total += covered.min(area) / area;
    }
    total / nodes.len() as f64
}

/// `node_connector_overlap`: fraction of connector length under non-incident
/// node shapes and pipes, each covered sub-length counted once.
fn node_connector_overlap_term(
    nodes: &[SceneNode],
    connectors: &[ConnectorGeometry],
    sink: &mut DefectSink,
) -> f64 {
    let total_length: f64 = connectors.iter().map(|c| c.length).sum();
    if total_length <= 0.0 {
        return 0.0;
    }
    // Obstacles: every node's shape, and every flow's pipe boxes (a link run
    // along a pipe is as misleading as one under a shape). Each obstacle is
    // owned by a node uid for the incidence check.
    let mut obstacles: Vec<(i32, Rect)> = Vec::new();
    for n in nodes {
        obstacles.push((n.uid, n.shape));
        obstacles.extend(n.pipe.iter().map(|r| (n.uid, *r)));
    }
    let mut inside = 0.0;
    for c in connectors {
        // Length of this connector under each obstacle, for the defect report.
        let mut per_obstacle: BTreeMap<usize, f64> = BTreeMap::new();
        for seg in c.polyline.windows(2) {
            let seg_len = ((seg[1].x - seg[0].x).powi(2) + (seg[1].y - seg[0].y).powi(2)).sqrt();
            if seg_len == 0.0 {
                continue;
            }
            let mut intervals: Vec<(f64, f64)> = Vec::new();
            for (k, (uid, rect)) in obstacles.iter().enumerate() {
                if c.incident_uids.contains(uid) {
                    continue;
                }
                if let Some(iv) = segment_clip_interval_in_rect(&seg[0], &seg[1], rect) {
                    intervals.push(iv);
                    *per_obstacle.entry(k).or_default() += (iv.1 - iv.0) * seg_len;
                }
            }
            inside += merged_interval_length(&mut intervals) * seg_len;
        }
        for (k, length) in per_obstacle {
            sink.push(DefectKind::ConnectorThroughNode, obstacles[k].1, length);
        }
    }
    inside / total_length
}

/// `label_overlap`: mean covered fraction of each label box by other labels
/// and other nodes' shapes. A pipe through a label is not coverage but a line
/// through the name, charged by `label_connector_overlap`: counting its 4px
/// band by area would make a pipe through a name several times cheaper than a
/// hairline link through it.
fn label_overlap_term(nodes: &[SceneNode], sink: &mut DefectSink) -> f64 {
    let labeled: Vec<&SceneNode> = nodes.iter().filter(|n| n.label.is_some()).collect();
    if labeled.is_empty() {
        return 0.0;
    }
    let mut total = 0.0;
    for a in &labeled {
        let lbl = a.label.expect("filtered to labeled nodes");
        let area = rect_area(&lbl);
        if area <= 0.0 {
            continue;
        }
        let mut covered = 0.0;
        for b in nodes {
            if b.uid == a.uid {
                continue;
            }
            covered += rect_overlap_area(&lbl, &b.shape);
            if let Some(other) = b.label {
                covered += rect_overlap_area(&lbl, &other);
            }
        }
        let fraction = covered.min(area) / area;
        if fraction > 0.0 {
            sink.push(DefectKind::LabelObscured, lbl, fraction);
        }
        total += fraction;
    }
    total / labeled.len() as f64
}

/// `label_connector_overlap`: mean over labels of the connector length (links
/// and pipes) through the label's text box relative to the box's smaller side.
fn label_connector_overlap_term(
    nodes: &[SceneNode],
    connectors: &[ConnectorGeometry],
    sink: &mut DefectSink,
) -> f64 {
    let labels: Vec<(i32, Rect)> = nodes
        .iter()
        .filter_map(|n| n.label.map(|l| (n.uid, l)))
        .collect();
    if labels.is_empty() {
        return 0.0;
    }
    let mut total = 0.0;
    for (owner, lbl) in &labels {
        let fraction = label_strike_fraction(*owner, lbl, connectors);
        if fraction > 0.0 {
            sink.push(DefectKind::LabelCrossed, *lbl, fraction);
        }
        total += fraction;
    }
    total / labels.len() as f64
}

/// How struck out the label box `lbl` of node `owner` is: the connector length
/// through its (inset) text box relative to the box's smaller side, capped at
/// 1.
fn label_strike_fraction<'a>(
    owner: i32,
    lbl: &Rect,
    connectors: impl IntoIterator<Item = &'a ConnectorGeometry>,
) -> f64 {
    let text = inset(lbl, LABEL_INSET);
    let side = common::rect_width(&text).min(common::rect_height(&text));
    if side <= 0.0 {
        return 0.0;
    }
    let mut through = 0.0;
    for c in connectors {
        let factor = match c.kind {
            // A link into or out of the labeled node at least points at (or
            // leaves from) that name, the way a modeler draws an arrow to a
            // variable, so it strikes the name out half as badly as a line
            // passing through on its way somewhere else.
            ConnectorKind::Link if c.incident_uids.contains(&owner) => OWN_LINK_STRIKE_FACTOR,
            ConnectorKind::Link => 1.0,
            // A flow's name sits beside its own pipe. Every other pipe through
            // a name -- one entering the named stock through the face the name
            // sits on included -- writes over it.
            ConnectorKind::Pipe if c.flow_uid == Some(owner) => continue,
            ConnectorKind::Pipe => 1.0,
        };
        for seg in c.polyline.windows(2) {
            if let Some((t0, t1)) = segment_clip_interval_in_rect(&seg[0], &seg[1], &text) {
                let seg_len =
                    ((seg[1].x - seg[0].x).powi(2) + (seg[1].y - seg[0].y).powi(2)).sqrt();
                through += factor * (t1 - t0) * seg_len;
            }
        }
    }
    (through / side).min(1.0)
}

/// The drawn scene of a view, for a label-side chooser that must charge a
/// candidate label box as the metric would. Shapes and connectors stay put
/// while sides are chosen; the other labels' boxes are whatever the chooser
/// has picked so far, so they are supplied per call.
pub(crate) struct LabelScene {
    nodes: Vec<SceneNode>,
    connectors: Vec<ConnectorGeometry>,
    /// `labels / nodes`: converts a per-node crowding deficit into the same
    /// per-label units the label terms are charged in.
    crowding_scale: f64,
    index_of_uid: HashMap<i32, usize>,
    /// Each node by the region it can reach: its shape, grown by its label's
    /// size on every side (a label may take any side) and the crowding
    /// clearance.
    node_grid: SceneGrid,
    /// Each connector by its polyline's bounding box.
    connector_grid: SceneGrid,
}

impl LabelScene {
    pub(crate) fn new(elements: &[ViewElement]) -> Self {
        let nodes = build_scene_nodes(elements);
        let connectors = collect_connector_geometry(elements);
        let labels = nodes.iter().filter(|n| n.label.is_some()).count();
        let crowding_scale = if nodes.is_empty() {
            0.0
        } else {
            labels as f64 / nodes.len() as f64
        };
        let mut node_grid = SceneGrid::default();
        for (i, n) in nodes.iter().enumerate() {
            let (w, h) = n.label.map_or((0.0, 0.0), |l| {
                (common::rect_width(&l), common::rect_height(&l))
            });
            let reach = w.max(h) + REACH_PAD;
            node_grid.insert(i, &grown(&n.shape, reach));
        }
        let mut connector_grid = SceneGrid::default();
        for (i, c) in connectors.iter().enumerate() {
            let bounds = c
                .polyline
                .iter()
                .fold(None, |acc: Option<Rect>, p| {
                    let point = Rect {
                        left: p.x,
                        top: p.y,
                        right: p.x,
                        bottom: p.y,
                    };
                    Some(acc.map_or(point, |r| merge_bounds(r, point)))
                })
                .expect("a connector has at least two points");
            connector_grid.insert(i, &bounds);
        }
        LabelScene {
            index_of_uid: nodes.iter().enumerate().map(|(i, n)| (n.uid, i)).collect(),
            nodes,
            connectors,
            crowding_scale,
            node_grid,
            connector_grid,
        }
    }

    /// What the metric charges node `owner` for wearing the label box `lbl`,
    /// in per-label units: `w.label_overlap` times the fraction of the box
    /// covered by other nodes' shapes and labels, `w.label_connector_overlap`
    /// times its strike fraction, and `w.crowding` times the clearance deficit
    /// of every pair `owner` forms with another node. `label_of(uid)` is
    /// another node's current label box. The part of the cost that does not
    /// depend on `lbl` is the same for every side, so only differences
    /// between sides mean anything.
    pub(crate) fn label_cost(
        &self,
        owner: i32,
        lbl: &Rect,
        label_of: impl Fn(i32) -> Option<Rect>,
        w: &MetricWeights,
    ) -> f64 {
        let area = rect_area(lbl);
        if area <= 0.0 {
            return 0.0;
        }
        let Some(&own_index) = self.index_of_uid.get(&owner) else {
            return 0.0;
        };
        let own = &self.nodes[own_index];
        // Only nodes whose reach meets this label or the owner's own shape
        // (every pair the crowding term can charge involves one of the two)
        // can contribute; the rest add exact zeros. Visiting the candidates in
        // index order keeps every sum bit-identical to a full scan.
        let query = grown(&merge_bounds(*lbl, own.shape), COMFORTABLE_CLEARANCE);
        let mut covered = 0.0;
        let mut crowding = 0.0;
        for other in self
            .node_grid
            .query(&query)
            .into_iter()
            .map(|i| &self.nodes[i])
            .filter(|n| n.uid != owner)
        {
            let other_label = label_of(other.uid);
            covered += rect_overlap_area(lbl, &other.shape);
            if let Some(ol) = &other_label {
                covered += rect_overlap_area(lbl, ol);
            }
            if own.is_cloud || other.is_cloud {
                continue;
            }
            let (gap, _) = footprint_gap(own, Some(*lbl), other, other_label);
            if gap < COMFORTABLE_CLEARANCE {
                crowding += (1.0 - gap / COMFORTABLE_CLEARANCE).powi(2);
            }
        }
        let text = inset(lbl, LABEL_INSET);
        let struck = self
            .connector_grid
            .query(&text)
            .into_iter()
            .map(|i| &self.connectors[i]);
        w.label_overlap * covered.min(area) / area
            + w.label_connector_overlap * label_strike_fraction(owner, lbl, struck)
            + w.crowding * self.crowding_scale * crowding
    }
}

/// How far past a node's shape its reach extends beyond its label's size: the
/// crowding clearance plus room for the label's offset from the shape.
const REACH_PAD: f64 = 2.0 * COMFORTABLE_CLEARANCE;

/// Cell size of [`SceneGrid`]: about a node with its label.
const SCENE_GRID_CELL: f64 = 96.0;

/// A uniform grid of item indices by the cells their rects cover.
#[derive(Default)]
struct SceneGrid {
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl SceneGrid {
    fn cell_range(r: &Rect) -> (std::ops::RangeInclusive<i64>, std::ops::RangeInclusive<i64>) {
        let cell = |v: f64| (v / SCENE_GRID_CELL).floor() as i64;
        (cell(r.left)..=cell(r.right), cell(r.top)..=cell(r.bottom))
    }

    fn insert(&mut self, index: usize, r: &Rect) {
        let (xs, ys) = Self::cell_range(r);
        for x in xs {
            for y in ys.clone() {
                self.cells.entry((x, y)).or_default().push(index);
            }
        }
    }

    /// Every item whose rect's cells meet `r`'s, each once, in index order.
    fn query(&self, r: &Rect) -> Vec<usize> {
        let (xs, ys) = Self::cell_range(r);
        let mut out = Vec::new();
        for x in xs {
            for y in ys.clone() {
                if let Some(items) = self.cells.get(&(x, y)) {
                    out.extend_from_slice(items);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

fn grown(r: &Rect, d: f64) -> Rect {
    inset(r, -d)
}

/// `crossings`: crossings per connector, on the drawn polylines.
fn crossings_term(
    view: &datamodel::StockFlow,
    connector_count: usize,
    sink: &mut DefectSink,
) -> f64 {
    if connector_count == 0 {
        return 0.0;
    }
    let segments = build_view_segments(view);
    let mut count = 0usize;
    for i in 0..segments.len() {
        for j in (i + 1)..segments.len() {
            if let Some(p) = segment_intersection(&segments[i], &segments[j]) {
                count += 1;
                sink.push_point(DefectKind::Crossing, Point { x: p.x, y: p.y }, 1.0);
            }
        }
    }
    count as f64 / connector_count as f64
}

/// `crowding`: the mean clearance deficit per node over pairs of non-cloud
/// nodes whose footprints come closer than `COMFORTABLE_CLEARANCE`, plus the
/// mean deficit per link over links whose visible length (outside both
/// endpoint shapes) falls below `MIN_VISIBLE_LINK`.
fn crowding_term(
    nodes: &[SceneNode],
    connectors: &[ConnectorGeometry],
    sink: &mut DefectSink,
) -> f64 {
    if nodes.len() < 2 {
        return 0.0;
    }
    let shapes: HashMap<i32, Rect> = nodes.iter().map(|n| (n.uid, n.shape)).collect();
    let links: Vec<&ConnectorGeometry> = connectors
        .iter()
        .filter(|c| c.kind == ConnectorKind::Link)
        .collect();
    let mut short = 0.0;
    for c in &links {
        let hidden: f64 = c
            .incident_uids
            .iter()
            .filter_map(|uid| shapes.get(uid))
            .map(|shape| {
                c.polyline
                    .windows(2)
                    .filter_map(|seg| {
                        segment_clip_interval_in_rect(&seg[0], &seg[1], shape).map(|(t0, t1)| {
                            (t1 - t0)
                                * ((seg[1].x - seg[0].x).powi(2) + (seg[1].y - seg[0].y).powi(2))
                                    .sqrt()
                        })
                    })
                    .sum::<f64>()
            })
            .sum();
        let visible = (c.length - hidden).max(0.0);
        if visible < MIN_VISIBLE_LINK {
            let deficit = (1.0 - visible / MIN_VISIBLE_LINK).powi(2);
            short += deficit;
            let mid = c.polyline[c.polyline.len() / 2];
            sink.push_point(DefectKind::Crowded, mid, deficit);
        }
    }
    let short_rate = if links.is_empty() {
        0.0
    } else {
        short / links.len() as f64
    };
    let boxes: Vec<Rect> = nodes.iter().map(SceneNode::footprint_box).collect();
    let mut total = 0.0;
    for i in 0..nodes.len() {
        for j in (i + 1)..nodes.len() {
            if nodes[i].is_cloud || nodes[j].is_cloud {
                continue;
            }
            // Cheap reject: the merged boxes are already comfortably apart.
            if rect_gap(&boxes[i], &boxes[j]) >= COMFORTABLE_CLEARANCE {
                continue;
            }
            let (gap, closest) =
                footprint_gap(&nodes[i], nodes[i].label, &nodes[j], nodes[j].label);
            if gap < COMFORTABLE_CLEARANCE {
                let deficit = (1.0 - gap / COMFORTABLE_CLEARANCE).powi(2);
                total += deficit;
                sink.push(
                    DefectKind::Crowded,
                    merge_bounds(closest.0, closest.1),
                    deficit,
                );
            }
        }
    }
    total / nodes.len() as f64 + short_rate
}

/// The clearance between two nodes' footprints -- each one's shape and its
/// label box `*_label` -- as `crowding` measures it, with the two rects that
/// realize it. A flow's valve sits a fixed short pipe away from the stock or
/// cloud it attaches to by construction: their SHAPES being close is structure,
/// but either one's label crowding the other is not.
fn footprint_gap(
    a: &SceneNode,
    a_label: Option<Rect>,
    b: &SceneNode,
    b_label: Option<Rect>,
) -> (f64, (Rect, Rect)) {
    let attached = a.attached_to(b);
    let a_rects = [Some((false, a.shape)), a_label.map(|l| (true, l))];
    let b_rects = [Some((false, b.shape)), b_label.map(|l| (true, l))];
    let mut gap = f64::INFINITY;
    let mut closest = (a.shape, b.shape);
    for &(a_is_label, ra) in a_rects.iter().flatten() {
        for &(b_is_label, rb) in b_rects.iter().flatten() {
            if attached && !a_is_label && !b_is_label {
                continue;
            }
            let g = rect_gap(&ra, &rb);
            if g < gap {
                gap = g;
                closest = (ra, rb);
            }
        }
    }
    (gap, closest)
}

/// `long_connectors`: mean excess of each link over `LONG_CONNECTOR_FACTOR`
/// times the median link length.
fn long_connectors_term(connectors: &[ConnectorGeometry], sink: &mut DefectSink) -> f64 {
    let links: Vec<&ConnectorGeometry> = connectors
        .iter()
        .filter(|c| c.kind == ConnectorKind::Link)
        .collect();
    if links.len() < 2 {
        return 0.0;
    }
    let mut lengths: Vec<f64> = links.iter().map(|c| c.length).collect();
    lengths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = lengths.len() / 2;
    let median = if lengths.len().is_multiple_of(2) {
        (lengths[mid - 1] + lengths[mid]) / 2.0
    } else {
        lengths[mid]
    };
    let threshold = LONG_CONNECTOR_FACTOR * median.max(1.0);
    let mut total = 0.0;
    for c in &links {
        let excess = (c.length / threshold - 1.0).max(0.0);
        if excess > 0.0 {
            total += excess;
            let mid_point = c.polyline[c.polyline.len() / 2];
            sink.push_point(DefectKind::LongConnector, mid_point, excess);
        }
    }
    total / links.len() as f64
}

/// `misalignment`: fraction of nodes sharing no row or column with a nearby
/// node.
fn misalignment_term(nodes: &[SceneNode]) -> f64 {
    if nodes.len() < 2 {
        return 0.0;
    }
    let aligned = nodes
        .iter()
        .filter(|a| {
            nodes.iter().any(|b| {
                if a.uid == b.uid {
                    return false;
                }
                let dx = (a.center.x - b.center.x).abs();
                let dy = (a.center.y - b.center.y).abs();
                (dx <= ALIGN_TOLERANCE && dy <= ALIGN_REACH)
                    || (dy <= ALIGN_TOLERANCE && dx <= ALIGN_REACH)
            })
        })
        .count();
    1.0 - aligned as f64 / nodes.len() as f64
}

/// Union of rects, or `None` for an empty set.
fn view_bounding_box(boxes: &[Rect]) -> Option<Rect> {
    let mut iter = boxes.iter();
    let first = *iter.next()?;
    Some(iter.fold(first, |acc, r| merge_bounds(acc, *r)))
}

/// Compute the layout quality metrics for a completed view.
///
/// PURE: takes data, returns scalars, performs no I/O. The `_config` parameter
/// is kept for the optimizer-facing signature; all geometry comes from the
/// `diagram` helpers (fixed pixel element sizes). Every term is finite: each
/// division guards a zero denominator by returning 0.
pub fn compute_layout_metrics(
    view: &datamodel::StockFlow,
    _config: &LayoutConfig,
) -> LayoutMetrics {
    analyze(view, &mut DefectSink { defects: None })
}

/// The metrics of `view` plus every defect behind them, located on the diagram.
/// Computed by the same code as [`compute_layout_metrics`].
pub fn analyze_layout(view: &datamodel::StockFlow) -> LayoutAnalysis {
    let mut sink = DefectSink {
        defects: Some(Vec::new()),
    };
    let metrics = analyze(view, &mut sink);
    LayoutAnalysis {
        metrics,
        defects: sink.defects.unwrap_or_default(),
    }
}

fn analyze(view: &datamodel::StockFlow, sink: &mut DefectSink) -> LayoutMetrics {
    let nodes = build_scene_nodes(&view.elements);
    let connectors = collect_connector_geometry(&view.elements);

    let node_overlap = node_overlap_term(&nodes, sink);
    let node_connector_overlap = node_connector_overlap_term(&nodes, &connectors, sink);
    let label_overlap = label_overlap_term(&nodes, sink);
    let label_connector_overlap = label_connector_overlap_term(&nodes, &connectors, sink);
    let crossings = crossings_term(view, connectors.len(), sink);
    let crowding = crowding_term(&nodes, &connectors, sink);
    let long_connectors = long_connectors_term(&connectors, sink);
    let misalignment = misalignment_term(&nodes);

    let footprints: Vec<Rect> = nodes.iter().map(SceneNode::footprint_box).collect();
    let total_connector_length: f64 = connectors.iter().map(|c| c.length).sum();

    // --- sprawl ---
    let sprawl = if !connectors.is_empty() && !footprints.is_empty() {
        let mean_connector_length = total_connector_length / connectors.len() as f64;
        let characteristic_node_size = footprints
            .iter()
            .map(|r| {
                let w = common::rect_width(r);
                let h = common::rect_height(r);
                (w * w + h * h).sqrt()
            })
            .sum::<f64>()
            / footprints.len() as f64;
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
                .map(|c| (c.length - mean).powi(2))
                .sum::<f64>()
                / n;
            variance.sqrt() / mean
        } else {
            0.0
        }
    } else {
        0.0
    };

    // --- aspect_penalty: long side over short side, beyond the target band ---
    let aspect_penalty = match view_bounding_box(&footprints) {
        Some(bbox) => {
            let w = common::rect_width(&bbox);
            let h = common::rect_height(&bbox);
            let (long, short) = if w >= h { (w, h) } else { (h, w) };
            if short <= 0.0 {
                0.0
            } else {
                (long / short - TARGET_AR_MAX).max(0.0)
            }
        }
        None => 0.0,
    };

    let loop_compactness = compute_loop_compactness(view);
    let loop_straightness = compute_loop_straightness(view);

    // --- flow_bends (mean right-angle bends per flow pipe) ---
    let flow_bends = {
        let mut total_bends = 0usize;
        let mut flow_count = 0usize;
        for e in &view.elements {
            if let ViewElement::Flow(f) = e {
                flow_count += 1;
                total_bends += crate::layout::orthogonal::flow_bend_count(&f.points);
            }
        }
        if flow_count > 0 {
            total_bends as f64 / flow_count as f64
        } else {
            0.0
        }
    };

    LayoutMetrics {
        node_overlap,
        node_connector_overlap,
        label_overlap,
        label_connector_overlap,
        crossings,
        crowding,
        sprawl,
        long_connectors,
        edge_length_cv,
        aspect_penalty,
        misalignment,
        loop_compactness,
        flow_bends,
        loop_straightness,
    }
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

// --- loop_straightness (are feedback-loop connectors drawn as visible curves) -
//
// loop_compactness above scores how circular a loop's NODE arrangement is, but
// not whether the connectors between those nodes are actually drawn as arcs. A
// loop can have well-spread node centers yet still read as a zig-zag if its
// causal connectors are straight chords. This term measures exactly that: the
// shortfall of each loop connector's drawn curvature below a target bow. It is
// primarily a Goodhart guard -- `apply_loop_curvature` curves loop connectors
// deterministically, so a healthy layout scores ~0; if any future change flattens
// loop connectors, this term (and the metric) rises, so the optimizer can never
// trade away the curvature that makes a loop legible. Flow pipes in a loop are
// exempt: they are orthogonal by convention, never arced.

/// Target bow ratio (max perpendicular deviation / chord length) for a loop's
/// causal connectors. A quarter-circle arc -- a clearly visible loop curve --
/// has a bow of ~0.21; 0.15 treats moderate curvature as "enough" so the term
/// only fires on connectors drawn (near-)straight.
const LOOP_LINK_TARGET_BOW: f64 = 0.15;

/// Maximum perpendicular deviation of a polyline from its straight chord
/// (first -> last point), divided by the chord length. 0 for a straight two-point
/// line; ~0.21 for a quarter-circle arc. Returns 0 for a degenerate (near-zero)
/// chord so the ratio is always finite.
fn polyline_bow_ratio(polyline: &[Point]) -> f64 {
    if polyline.len() < 3 {
        return 0.0;
    }
    let a = polyline[0];
    let b = polyline[polyline.len() - 1];
    let cx = b.x - a.x;
    let cy = b.y - a.y;
    let chord = (cx * cx + cy * cy).sqrt();
    if chord < 1e-9 {
        return 0.0;
    }
    let mut max_perp = 0.0_f64;
    for p in &polyline[1..polyline.len() - 1] {
        // Perpendicular distance from p to the infinite line through a,b.
        let perp = (cx * (a.y - p.y) - cy * (a.x - p.x)).abs() / chord;
        max_perp = max_perp.max(perp);
    }
    max_perp / chord
}

/// Map each directed causal connector (Link) `from_uid -> to_uid` to the polyline
/// the renderer draws for it, so loop-straightness can look up the drawn
/// curvature of a loop edge. Flows are not included (loop edges through a flow
/// valve have no Link and are correctly skipped).
fn link_polylines(
    view: &datamodel::StockFlow,
) -> std::collections::HashMap<(i32, i32), Vec<Point>> {
    let mut uid_elements: std::collections::HashMap<i32, &ViewElement> =
        std::collections::HashMap::new();
    for elem in &view.elements {
        uid_elements.insert(elem.get_uid(), elem);
    }
    let not_arrayed = |_: &str| false;
    let mut out: std::collections::HashMap<(i32, i32), Vec<Point>> =
        std::collections::HashMap::new();
    for elem in &view.elements {
        if let ViewElement::Link(link) = elem
            && let (Some(&from), Some(&to)) = (
                uid_elements.get(&link.from_uid),
                uid_elements.get(&link.to_uid),
            )
        {
            let polyline = connector_polyline(link, from, to, &not_arrayed, ARC_POLYLINE_SAMPLES);
            if polyline.len() >= 2 {
                out.insert((link.from_uid, link.to_uid), polyline);
            }
        }
    }
    out
}

/// `loop_straightness`: mean bow shortfall over the causal connectors that
/// participate in a feedback loop. 0.0 = every loop connector is drawn with at
/// least the target curvature (the loop reads as a visible circle); 1.0 = loop
/// connectors are straight (the loop collapses to a zig-zag). 0.0 when the view
/// has no loop with a causal connector. Deterministic and PURE; reuses the same
/// loop graph / cycle enumeration as loop_compactness.
fn compute_loop_straightness(view: &datamodel::StockFlow) -> f64 {
    let graph = build_loop_graph(view);
    let cycles = enumerate_simple_cycles(&graph);
    if cycles.is_empty() {
        return 0.0;
    }
    let polys = link_polylines(view);
    let mut seen: HashSet<(i32, i32)> = HashSet::new();
    let mut total = 0.0;
    let mut count = 0usize;
    for cycle in &cycles {
        let n = cycle.len();
        for k in 0..n {
            let edge = (cycle[k], cycle[(k + 1) % n]);
            let Some(poly) = polys.get(&edge) else {
                continue; // a flow-pipe edge (no Link): exempt
            };
            if !seen.insert(edge) {
                continue; // count each loop connector once
            }
            let bow = polyline_bow_ratio(poly);
            let shortfall = (LOOP_LINK_TARGET_BOW - bow).max(0.0) / LOOP_LINK_TARGET_BOW;
            total += shortfall;
            count += 1;
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
