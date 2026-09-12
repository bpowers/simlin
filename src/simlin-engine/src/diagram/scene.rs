// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The diagram scene: a resolution-independent display list of one model's
//! stock-and-flow view, for renderers that draw natively instead of from SVG.
//! `docs/design/diagram-scene.md` is the contract.
//!
//! The scene is the SVG renderer's twin. [`build_scene`] walks the same
//! `resolve_view` result `render_svg` walks and reads every number from the
//! same per-element geometry functions (`aux_geometry`, `flow_geometry`,
//! `connector_geometry`, `label_lines`, ...), so a consumer drawing the scene
//! draws what the SVG draws -- and that SVG is byte-identical to the web
//! editor's static renderer -- without implementing any geometry itself. What
//! the scene adds is representation only: SVG arcs become cubic Béziers and
//! SVG transforms are applied to the points (`diagram::path`).

use std::sync::OnceLock;

use serde::Serialize;

use crate::common::canonicalize;
use crate::datamodel;
use crate::diagram::arrowhead::{
    ARROWHEAD_BACK_LARGE_ARC, ARROWHEAD_BACK_SWEEP, ArrowheadGeometry, arrowhead_geometry,
};
use crate::diagram::common::{Circle, Frame, Point, Rect, merge_bounds, rad_to_deg, rotate_about};
use crate::diagram::connector::{
    ConnectorGeometry, connector_geometry, connector_is_dashed, get_visual_center,
    intersect_element_straight,
};
use crate::diagram::constants::{
    ARROWHEAD_RADIUS, GROUP_LABEL_FONT_WEIGHT, LABEL_FONT_SIZE, LABEL_FONT_WEIGHT,
};
use crate::diagram::elements::{
    CLOUD_PATH, alias_geometry, aux_geometry, cloud_transform, group_geometry, module_geometry,
    stock_geometry,
};
use crate::diagram::flow::flow_geometry;
use crate::diagram::label::{LabelProps, TextAnchor, label_bounds, label_lines};
use crate::diagram::path::{PathBuilder, control_point_bounds, parse_absolute_svg_path};
use crate::diagram::resolve::{LINK_LAYER, ResolvedElement, resolve_view};

/// The contract version a consumer checks before reading a scene.
pub const SCENE_VERSION: u32 = 1;

/// Half the widest stroke any paint draws (the 4-unit outer pipe). Padding
/// every shape's box by it keeps element bounds conservative without restating
/// the paint table the consumers own.
const STROKE_MARGIN: f64 = 2.0;

/// A model's diagram as a display list.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    /// The contract version, [`SCENE_VERSION`].
    pub version: u32,
    pub model_name: String,
    /// The fit-to-content box: the union of the element boxes `render_svg`
    /// folds into its viewBox, before the SVG's padding. `None` when no
    /// element contributes one.
    pub content_bounds: Option<SceneBounds>,
    /// The drawn elements, in draw order.
    pub elements: Vec<SceneElement>,
}

impl Scene {
    /// The scene as compact JSON, the form `simlin_project_render_scene`
    /// returns.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|err| format!("failed to serialize scene: {err}"))
    }
}

/// An axis-aligned box in canvas units.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Serialize)]
pub struct SceneBounds {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl From<Rect> for SceneBounds {
    fn from(r: Rect) -> Self {
        SceneBounds {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

/// One drawn element: its shapes, then its sparkline, then its label.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneElement {
    pub uid: i32,
    pub kind: SceneElementKind,
    pub layer: u8,
    /// The canonical identifier of the variable whose series the element
    /// displays: the element's own variable, or an alias's target.
    pub ident: Option<String>,
    pub is_arrayed: bool,
    /// A conservative visual box, for culling and hit testing.
    pub bounds: SceneBounds,
    pub shapes: Vec<SceneShape>,
    pub sparkline: Option<SparklineSlot>,
    pub label: Option<SceneLabel>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SceneElementKind {
    Group,
    Link,
    Flow,
    Stock,
    Cloud,
    Module,
    Aux,
    Alias,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SceneShape {
    Rect(SceneRectangle),
    Circle(SceneCircle),
    Path(ScenePath),
}

impl SceneShape {
    /// Whether every coordinate is finite. A non-finite one (degenerate
    /// geometry the SVG prints as `NaN` and draws nothing for) has no drawing
    /// to hand a consumer.
    fn is_finite(&self) -> bool {
        match self {
            SceneShape::Rect(r) => [r.x, r.y, r.width, r.height, r.corner_radius]
                .iter()
                .all(|v| v.is_finite()),
            SceneShape::Circle(c) => [c.cx, c.cy, c.r].iter().all(|v| v.is_finite()),
            SceneShape::Path(p) => p.d.iter().all(|v| v.is_finite()),
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneRectangle {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub corner_radius: f64,
    pub paint: ScenePaint,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneCircle {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
    pub paint: ScenePaint,
}

/// A path as a flat array of opcodes and operands (`diagram::path`).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenePath {
    pub d: Vec<f64>,
    pub paint: ScenePaint,
}

/// The semantic style of a shape; consumers resolve it through their theme.
/// Each paint is one CSS class of the SVG renderer's stylesheet.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScenePaint {
    Stock,
    Aux,
    Module,
    Alias,
    Valve,
    FlowPipeOuter,
    FlowPipeInner,
    ArrowheadFlow,
    Cloud,
    Connector,
    ConnectorDashed,
    ArrowheadLink,
    Group,
}

/// The box a sparkline is drawn into.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Serialize)]
pub struct SparklineSlot {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<Frame> for SparklineSlot {
    fn from(f: Frame) -> Self {
        SparklineSlot {
            x: f.x,
            y: f.y,
            width: f.width,
            height: f.height,
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneLabel {
    pub paint: LabelPaint,
    pub anchor: SceneTextAnchor,
    pub baseline: TextBaseline,
    pub font_size: f64,
    pub font_weight: u32,
    /// Whether the label draws the SVG's `labelBackground` halo.
    pub halo: bool,
    pub lines: Vec<SceneLabelLine>,
}

impl SceneLabel {
    fn is_finite(&self) -> bool {
        self.lines
            .iter()
            .all(|l| l.x.is_finite() && l.y.is_finite())
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LabelPaint {
    Label,
    GroupLabel,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SceneTextAnchor {
    Start,
    Middle,
    End,
}

impl From<TextAnchor> for SceneTextAnchor {
    fn from(anchor: TextAnchor) -> Self {
        match anchor {
            TextAnchor::Start => SceneTextAnchor::Start,
            TextAnchor::Middle => SceneTextAnchor::Middle,
            TextAnchor::End => SceneTextAnchor::End,
        }
    }
}

/// What a label line's `y` names: the alphabetic baseline, or the top of the
/// line (`dominant-baseline="hanging"`).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TextBaseline {
    Alphabetic,
    Hanging,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneLabelLine {
    pub text: String,
    pub x: f64,
    pub y: f64,
}

/// Builds the scene for `model_name`'s first stock-and-flow view.
///
/// Fails exactly where `render_svg` fails: a missing model, or a model with no
/// stock-and-flow view.
pub fn build_scene(project: &datamodel::Project, model_name: &str) -> Result<Scene, String> {
    let view = resolve_view(project, model_name)?;
    let is_arrayed_fn = |name: &str| -> bool { view.is_arrayed(name) };
    let elements = view
        .elements
        .iter()
        .filter_map(|element| scene_element(element, &is_arrayed_fn))
        .collect();
    Ok(Scene {
        version: SCENE_VERSION,
        model_name: view.model.name.clone(),
        content_bounds: view.content_bounds.map(SceneBounds::from),
        elements,
    })
}

/// One element's parts, before its bounds are folded and undrawable numbers
/// are dropped.
struct ElementParts {
    uid: i32,
    kind: SceneElementKind,
    ident: Option<String>,
    is_arrayed: bool,
    shapes: Vec<SceneShape>,
    sparkline: Option<Frame>,
    label: Option<SceneLabel>,
    /// The label's estimated box (`label_bounds`), folded into the element's
    /// visual bounds.
    label_box: Option<Rect>,
}

/// The scene element of one resolved element, `None` where the SVG draws
/// nothing.
pub(crate) fn scene_element(
    element: &ResolvedElement<'_>,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> Option<SceneElement> {
    let layer = element.layer();
    let parts = match element {
        ResolvedElement::Group(group) => {
            let g = group_geometry(group);
            ElementParts {
                uid: group.uid,
                kind: SceneElementKind::Group,
                ident: None,
                is_arrayed: false,
                shapes: vec![rect_shape(&g.rect, g.corner_radius, ScenePaint::Group)],
                sparkline: None,
                label: Some(SceneLabel {
                    paint: LabelPaint::GroupLabel,
                    // The stylesheet's `.simlin-group text { text-anchor: start }`
                    // and the SVG's `dominant-baseline="hanging"`.
                    anchor: SceneTextAnchor::Start,
                    baseline: TextBaseline::Hanging,
                    font_size: LABEL_FONT_SIZE,
                    font_weight: GROUP_LABEL_FONT_WEIGHT,
                    halo: false,
                    lines: vec![SceneLabelLine {
                        text: single_line(&g.label_text),
                        x: g.label_anchor.x,
                        y: g.label_anchor.y,
                    }],
                }),
                label_box: None,
            }
        }
        ResolvedElement::Link { link, from, to } => {
            let line_paint = if connector_is_dashed(to) {
                ScenePaint::ConnectorDashed
            } else {
                ScenePaint::Connector
            };
            let mut line = PathBuilder::new();
            let arrowhead = match connector_geometry(link, from, to, is_arrayed_fn) {
                ConnectorGeometry::Straight(g) => {
                    line.move_to(g.start);
                    line.line_to(g.end);
                    g.arrowhead()
                }
                ConnectorGeometry::Arc(g) => {
                    line.move_to(g.start);
                    line.svg_arc_to(g.circ.r, g.circ.r, 0.0, g.sweep, g.inv, g.arc_end);
                    g.arrowhead()
                }
                // The SVG prints an empty group: nothing is drawn.
                ConnectorGeometry::Undrawable => return None,
            };
            ElementParts {
                uid: link.uid,
                kind: SceneElementKind::Link,
                ident: None,
                is_arrayed: false,
                shapes: vec![
                    path_shape(line.into_d(), line_paint),
                    path_shape(arrowhead_path(&arrowhead), ScenePaint::ArrowheadLink),
                ],
                sparkline: None,
                label: None,
                label_box: None,
            }
        }
        ResolvedElement::Flow {
            flow,
            sink,
            is_arrayed,
        } => {
            let g = flow_geometry(flow, sink, *is_arrayed)?;
            let pipe = polyline_path(&g.pipe);
            let mut shapes = vec![
                path_shape(pipe.clone(), ScenePaint::FlowPipeOuter),
                path_shape(arrowhead_path(&g.arrowhead), ScenePaint::ArrowheadFlow),
                path_shape(pipe, ScenePaint::FlowPipeInner),
            ];
            shapes.extend(
                g.valves
                    .iter()
                    .map(|valve| circle_shape(valve, ScenePaint::Valve)),
            );
            ElementParts {
                uid: flow.uid,
                kind: SceneElementKind::Flow,
                ident: Some(canonical_ident(&flow.name)),
                is_arrayed: *is_arrayed,
                shapes,
                sparkline: Some(g.sparkline),
                label: Some(element_label(&g.label)),
                label_box: Some(label_bounds(&g.label)),
            }
        }
        ResolvedElement::Stock { stock, is_arrayed } => {
            let g = stock_geometry(stock, *is_arrayed);
            ElementParts {
                uid: stock.uid,
                kind: SceneElementKind::Stock,
                ident: Some(canonical_ident(&stock.name)),
                is_arrayed: *is_arrayed,
                shapes: g
                    .rects
                    .iter()
                    .map(|r| rect_shape(r, 0.0, ScenePaint::Stock))
                    .collect(),
                sparkline: Some(g.sparkline),
                label: Some(element_label(&g.label)),
                label_box: Some(label_bounds(&g.label)),
            }
        }
        ResolvedElement::Cloud(cloud) => {
            let transform = cloud_transform(cloud);
            let mut outline = cloud_outline().clone();
            outline.map_points(|p| transform.apply(p));
            ElementParts {
                uid: cloud.uid,
                kind: SceneElementKind::Cloud,
                ident: None,
                is_arrayed: false,
                shapes: vec![path_shape(outline.into_d(), ScenePaint::Cloud)],
                sparkline: None,
                label: None,
                label_box: None,
            }
        }
        ResolvedElement::Module(module) => {
            let g = module_geometry(module);
            ElementParts {
                uid: module.uid,
                kind: SceneElementKind::Module,
                ident: Some(canonical_ident(&module.name)),
                is_arrayed: false,
                shapes: vec![rect_shape(&g.rect, g.corner_radius, ScenePaint::Module)],
                sparkline: None,
                label: Some(element_label(&g.label)),
                label_box: Some(label_bounds(&g.label)),
            }
        }
        ResolvedElement::Aux { aux, is_arrayed } => {
            let g = aux_geometry(aux, *is_arrayed);
            ElementParts {
                uid: aux.uid,
                kind: SceneElementKind::Aux,
                ident: Some(canonical_ident(&aux.name)),
                is_arrayed: *is_arrayed,
                shapes: g
                    .circles
                    .iter()
                    .map(|c| circle_shape(c, ScenePaint::Aux))
                    .collect(),
                sparkline: Some(g.sparkline),
                label: Some(element_label(&g.label)),
                label_box: Some(label_bounds(&g.label)),
            }
        }
        ResolvedElement::Alias {
            alias,
            alias_of_name,
        } => {
            let g = alias_geometry(alias, *alias_of_name);
            ElementParts {
                uid: alias.uid,
                kind: SceneElementKind::Alias,
                ident: alias_of_name.map(canonical_ident),
                is_arrayed: false,
                shapes: vec![circle_shape(&g.circle, ScenePaint::Alias)],
                // An alias shows its target's series; with no target there is
                // nothing to show.
                sparkline: alias_of_name.map(|_| g.sparkline),
                label: Some(element_label(&g.label)),
                label_box: Some(label_bounds(&g.label)),
            }
        }
    };
    finish(parts, layer)
}

/// A link drawn from `from` to a point where no element is: a link being drawn
/// or reattached while its end is over no valid target. A straight connector
/// from the source's boundary along the bearing of the point, its arrowhead at
/// the point, drawn as `straight_geometry` draws a link's start and arrowhead.
pub(crate) fn dangling_link_scene_element(
    uid: i32,
    from: &datamodel::ViewElement,
    to: Point,
    is_arrayed_fn: &dyn Fn(&str) -> bool,
) -> Option<SceneElement> {
    let (fx, fy) = get_visual_center(from, is_arrayed_fn);
    let theta = (to.y - fy).atan2(to.x - fx);
    let start = intersect_element_straight(from, theta, is_arrayed_fn);
    let arrowhead = arrowhead_geometry(to.x, to.y, rad_to_deg(theta), ARROWHEAD_RADIUS);
    let mut line = PathBuilder::new();
    line.move_to(start);
    line.line_to(to);
    finish(
        ElementParts {
            uid,
            kind: SceneElementKind::Link,
            ident: None,
            is_arrayed: false,
            shapes: vec![
                path_shape(line.into_d(), ScenePaint::Connector),
                path_shape(arrowhead_path(&arrowhead), ScenePaint::ArrowheadLink),
            ],
            sparkline: None,
            label: None,
            label_box: None,
        },
        LINK_LAYER,
    )
}

fn finish(parts: ElementParts, layer: u8) -> Option<SceneElement> {
    let shapes: Vec<SceneShape> = parts
        .shapes
        .into_iter()
        .filter(SceneShape::is_finite)
        .collect();
    let mut bounds = shapes
        .iter()
        .filter_map(shape_bounds)
        .reduce(merge_bounds)?;
    if let Some(label_box) = parts.label_box {
        bounds = merge_bounds(bounds, label_box);
    }
    Some(SceneElement {
        uid: parts.uid,
        kind: parts.kind,
        layer,
        ident: parts.ident,
        is_arrayed: parts.is_arrayed,
        bounds: bounds.into(),
        shapes,
        sparkline: parts
            .sparkline
            .filter(|f| [f.x, f.y, f.width, f.height].iter().all(|v| v.is_finite()))
            .map(SparklineSlot::from),
        label: parts.label.filter(SceneLabel::is_finite),
    })
}

fn canonical_ident(name: &str) -> String {
    canonicalize(name).into_owned()
}

/// An element label, with the halo the SVG's `labelBackground` filter draws.
fn element_label(props: &LabelProps) -> SceneLabel {
    let (anchor, lines) = label_lines(props);
    SceneLabel {
        paint: LabelPaint::Label,
        anchor: anchor.into(),
        baseline: TextBaseline::Alphabetic,
        font_size: LABEL_FONT_SIZE,
        font_weight: LABEL_FONT_WEIGHT,
        halo: true,
        lines: lines
            .into_iter()
            .map(|line| SceneLabelLine {
                text: line.text,
                x: line.x,
                y: line.y,
            })
            .collect(),
    }
}

/// A group label's text as the SVG shows it. The group name is printed as a
/// `<text>` with no `<tspan>`s, and the canvas styles text
/// `white-space: nowrap`, under which a newline renders as a space.
fn single_line(text: &str) -> String {
    text.replace('\n', " ")
}

/// [`CLOUD_PATH`] parsed once.
fn cloud_outline() -> &'static PathBuilder {
    static OUTLINE: OnceLock<PathBuilder> = OnceLock::new();
    OUTLINE.get_or_init(|| match parse_absolute_svg_path(CLOUD_PATH) {
        Ok(path) => path,
        // `scene_tests::the_cloud_outline_is_the_transformed_cloud_path` pins
        // that the constant parses.
        Err(err) => unreachable!("CLOUD_PATH is not an absolute M/L/C/Z path: {err}"),
    })
}

/// An arrowhead's outline with its rotation applied: the tip, a line to the
/// back edge, the back edge's bowing arc, closed.
fn arrowhead_path(g: &ArrowheadGeometry) -> Vec<f64> {
    let mut path = PathBuilder::new();
    path.move_to(g.tip);
    path.line_to(g.back_start);
    path.svg_arc_to(
        g.back_radius,
        g.back_radius,
        0.0,
        ARROWHEAD_BACK_LARGE_ARC,
        ARROWHEAD_BACK_SWEEP,
        g.back_end,
    );
    path.close();
    let (tip, degrees) = (g.tip, g.rotation_deg);
    path.map_points(|p| rotate_about(p, tip, degrees));
    path.into_d()
}

fn polyline_path(points: &[Point]) -> Vec<f64> {
    let mut path = PathBuilder::new();
    if let Some((first, rest)) = points.split_first() {
        path.move_to(*first);
        for p in rest {
            path.line_to(*p);
        }
    }
    path.into_d()
}

fn rect_shape(frame: &Frame, corner_radius: f64, paint: ScenePaint) -> SceneShape {
    SceneShape::Rect(SceneRectangle {
        x: frame.x,
        y: frame.y,
        width: frame.width,
        height: frame.height,
        corner_radius,
        paint,
    })
}

fn circle_shape(circle: &Circle, paint: ScenePaint) -> SceneShape {
    SceneShape::Circle(SceneCircle {
        cx: circle.x,
        cy: circle.y,
        r: circle.r,
        paint,
    })
}

fn path_shape(d: Vec<f64>, paint: ScenePaint) -> SceneShape {
    SceneShape::Path(ScenePath { d, paint })
}

/// A shape's box padded by [`STROKE_MARGIN`]; a path's box holds its control
/// points, which contain the drawn curve.
fn shape_bounds(shape: &SceneShape) -> Option<Rect> {
    let raw = match shape {
        SceneShape::Rect(r) => Some(Rect {
            top: r.y,
            left: r.x,
            right: r.x + r.width,
            bottom: r.y + r.height,
        }),
        SceneShape::Circle(c) => Some(Rect {
            top: c.cy - c.r,
            left: c.cx - c.r,
            right: c.cx + c.r,
            bottom: c.cy + c.r,
        }),
        SceneShape::Path(p) => control_point_bounds(&p.d),
    }?;
    Some(Rect {
        top: raw.top - STROKE_MARGIN,
        left: raw.left - STROKE_MARGIN,
        right: raw.right + STROKE_MARGIN,
        bottom: raw.bottom + STROKE_MARGIN,
    })
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod scene_tests;
