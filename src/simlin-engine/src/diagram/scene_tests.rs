// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the diagram scene.
//!
//! The oracle for every number here is the SVG `render_svg` prints for the
//! same project. The scene describes exactly the drawing that SVG describes,
//! and the SVG is byte-identical to the web editor's static renderer, so a
//! scene number is checked against the SVG's printed number by
//! [`assert_scene_draws_the_svg`], never against a second statement of the
//! geometry.

use super::*;

use crate::datamodel::view_element::{self, FlowPoint, LabelSide, LinkShape};
use crate::datamodel::{self, StockFlow, View, ViewElement};
use crate::diagram::constants::{
    CLOUD_RADIUS, GROUP_RADIUS, MODULE_HEIGHT, MODULE_RADIUS, MODULE_WIDTH,
};
use crate::diagram::elements::cloud_bounds;
use crate::diagram::path::{OP_CLOSE, OP_CUBIC, OP_LINE, OP_MOVE};
use crate::diagram::render_svg;
use crate::test_common::TestProject;

/// How far a scene number may sit from its printed SVG twin. The SVG quantizes
/// every coordinate to six decimals (`js_format_number`), which moves a number
/// by at most 5e-7, and a point rebuilt from printed numbers (an arc from a
/// quantized radius, the cloud outline under a quantized scale) by a few times
/// that.
const TOLERANCE: f64 = 1e-5;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE
}

// --- Fixtures ---

/// `builder`'s project with `elements` as the `main` model's view.
///
/// `TestProject` builds no views, so the view is attached here. The diagram
/// reads a variable only to ask whether its equation is arrayed, so fixtures
/// declare the variables their stocks, flows and auxes name, and leave out a
/// module's model, which the diagram never reads.
fn with_view(builder: TestProject, elements: Vec<ViewElement>) -> datamodel::Project {
    let mut project = builder.build_datamodel();
    project.models[0].views = vec![View::StockFlow(StockFlow {
        name: None,
        elements,
        view_box: datamodel::Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    })];
    project
}

fn aux(name: &str, uid: i32, x: f64, y: f64) -> ViewElement {
    aux_on(name, uid, x, y, LabelSide::Bottom)
}

fn aux_on(name: &str, uid: i32, x: f64, y: f64, label_side: LabelSide) -> ViewElement {
    ViewElement::Aux(view_element::Aux {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side,
        compat: None,
    })
}

fn stock(name: &str, uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Stock(view_element::Stock {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

fn cloud(uid: i32, flow_uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Cloud(view_element::Cloud {
        uid,
        flow_uid,
        x,
        y,
        compat: None,
    })
}

fn flow(name: &str, uid: i32, x: f64, y: f64, points: &[(f64, f64, Option<i32>)]) -> ViewElement {
    ViewElement::Flow(view_element::Flow {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        points: points
            .iter()
            .map(|&(x, y, attached_to_uid)| FlowPoint {
                x,
                y,
                attached_to_uid,
            })
            .collect(),
        compat: None,
        label_compat: None,
    })
}

fn link(uid: i32, from_uid: i32, to_uid: i32, shape: LinkShape) -> ViewElement {
    ViewElement::Link(view_element::Link {
        uid,
        from_uid,
        to_uid,
        shape,
        polarity: None,
    })
}

fn module(name: &str, uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Module(view_element::Module {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
    })
}

fn alias(uid: i32, alias_of_uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Alias(view_element::Alias {
        uid,
        alias_of_uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

fn group(name: &str, uid: i32, x: f64, y: f64, width: f64, height: f64) -> ViewElement {
    ViewElement::Group(view_element::Group {
        uid,
        name: name.to_string(),
        x,
        y,
        width,
        height,
        is_mdl_view_marker: false,
    })
}

/// One element of every kind: a flow from a cloud into a stock, an aux, a
/// link from the aux to the flow, a module, an alias of the aux, and a group
/// around them.
fn every_kind_project() -> datamodel::Project {
    with_view(
        TestProject::new("every-kind")
            .aux("growth_rate", "0.1", None)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * growth_rate", None),
        vec![
            group("sector", 1, 200.0, 150.0, 400.0, 300.0),
            aux("growth_rate", 2, 225.0, 40.0),
            stock("population", 3, 300.0, 100.0),
            cloud(4, 5, 150.0, 100.0),
            flow(
                "births",
                5,
                225.0,
                100.0,
                &[(150.0, 100.0, Some(4)), (277.5, 100.0, Some(3))],
            ),
            link(6, 2, 5, LinkShape::Straight),
            module("sub", 7, 100.0, 250.0),
            alias(8, 2, 300.0, 250.0),
        ],
    )
}

fn open_corpus_model(relative: &str) -> datamodel::Project {
    let path = format!("{}/../../test/{relative}", env!("CARGO_MANIFEST_DIR"));
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("failed to open {path}: {e}"));
    crate::compat::open_xmile(&mut std::io::BufReader::new(file))
        .unwrap_or_else(|e| panic!("failed to parse {path}: {e}"))
}

fn build(project: &datamodel::Project) -> (Scene, String) {
    (
        build_scene(project, "main").expect("the scene builds"),
        render_svg(project, "main").expect("the SVG renders"),
    )
}

fn element_by_uid(scene: &Scene, uid: i32) -> &SceneElement {
    scene
        .elements
        .iter()
        .find(|e| e.uid == uid)
        .unwrap_or_else(|| panic!("the scene has no element {uid}"))
}

fn shape_paint(shape: &SceneShape) -> ScenePaint {
    match shape {
        SceneShape::Rect(r) => r.paint,
        SceneShape::Circle(c) => c.paint,
        SceneShape::Path(p) => p.paint,
    }
}

fn paints(element: &SceneElement) -> Vec<ScenePaint> {
    element.shapes.iter().map(shape_paint).collect()
}

const ALL_KINDS: [SceneElementKind; 8] = [
    SceneElementKind::Group,
    SceneElementKind::Link,
    SceneElementKind::Flow,
    SceneElementKind::Stock,
    SceneElementKind::Cloud,
    SceneElementKind::Module,
    SceneElementKind::Aux,
    SceneElementKind::Alias,
];

/// The contract's per-kind row (`SCENE.md`, "Elements"): the layer, whether
/// an ident is carried, whether a sparkline slot is, and the label's paint.
/// The match has no wildcard arm, so a new kind does not compile without one.
/// An alias carries its ident and slot only when its target is in the view;
/// `an_alias_shows_its_targets_ident_and_sparkline_and_a_missing_target_neither`
/// covers the missing-target arm.
fn contract_row(kind: SceneElementKind) -> (u8, bool, bool, Option<LabelPaint>) {
    match kind {
        SceneElementKind::Group => (0, false, false, Some(LabelPaint::GroupLabel)),
        SceneElementKind::Link => (2, false, false, None),
        SceneElementKind::Flow => (3, true, true, Some(LabelPaint::Label)),
        SceneElementKind::Stock => (4, true, true, Some(LabelPaint::Label)),
        SceneElementKind::Cloud => (4, false, false, None),
        SceneElementKind::Module => (4, true, false, Some(LabelPaint::Label)),
        SceneElementKind::Aux => (5, true, true, Some(LabelPaint::Label)),
        SceneElementKind::Alias => (5, true, true, Some(LabelPaint::Label)),
    }
}

// --- Reading the SVG ---

/// One tag of the SVG's drawn body (everything after `</defs>`).
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
    /// The text between this tag and the next.
    text: String,
    /// The class of the latest `simlin-*` element group opened before this
    /// tag: which element a `<rect>` or `<circle>` belongs to.
    element_class: String,
}

impl Tag {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn num(&self, key: &str) -> f64 {
        let value = self
            .attr(key)
            .unwrap_or_else(|| panic!("<{}> has no {key}", self.name));
        value
            .parse()
            .unwrap_or_else(|e| panic!("<{}> {key}=\"{value}\": {e}", self.name))
    }

    fn class(&self) -> &str {
        self.attr("class").unwrap_or("")
    }
}

fn body_tags(svg: &str) -> Vec<Tag> {
    let body_start = svg.find("</defs>").expect("the SVG has a defs block") + "</defs>".len();
    let mut rest = &svg[body_start..];
    let mut element_class = String::new();
    let mut tags = Vec::new();
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        let tag_end = after.find('>').expect("every tag closes");
        let inner = &after[..tag_end];
        rest = &after[tag_end + 1..];
        if inner.starts_with('/') {
            continue;
        }
        let name_end = inner.find(' ').unwrap_or(inner.len());
        let mut tag = Tag {
            name: inner[..name_end].to_string(),
            attrs: attributes(&inner[name_end..]),
            text: rest[..rest.find('<').unwrap_or(rest.len())].to_string(),
            element_class: element_class.clone(),
        };
        if tag.name == "g" && tag.class().starts_with("simlin-") {
            element_class = tag.class().to_string();
            tag.element_class = element_class.clone();
        }
        tags.push(tag);
    }
    tags
}

fn attributes(mut text: &str) -> Vec<(String, String)> {
    let mut attrs = Vec::new();
    while let Some(eq) = text.find("=\"") {
        let key = text[..eq].trim().to_string();
        let value_start = eq + 2;
        let value_end = value_start
            + text[value_start..]
                .find('"')
                .expect("every attribute value closes");
        attrs.push((key, text[value_start..value_end].to_string()));
        text = &text[value_end + 1..];
    }
    attrs
}

fn tags_of<'a>(tags: &'a [Tag], name: &str, class: &str) -> Vec<&'a Tag> {
    tags.iter()
        .filter(|t| t.name == name && t.class() == class)
        .collect()
}

/// Every number in an SVG path, transform or viewBox, in order.
/// `js_format_number` prints no exponents, so digits, the point and the sign
/// are a number's only characters.
fn numbers(text: &str) -> Vec<f64> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter(|t| !t.is_empty())
        .map(|t| {
            t.parse()
                .unwrap_or_else(|e| panic!("bad number '{t}' in '{text}': {e}"))
        })
        .collect()
}

/// The scene path for an absolute SVG path of `M`, `L`, `A` and `z` commands
/// -- every form `render_svg` prints for pipes, connectors and arrowheads --
/// built from the SVG's printed numbers, with the SVG `rotate(deg, cx, cy)`
/// transform applied when given. That the arc conversion is exact is
/// `diagram::path`'s tests; here it only carries the SVG's numbers over.
fn svg_path_as_scene(d: &str, rotation: Option<&str>) -> Vec<f64> {
    let commands: Vec<(usize, char)> = d
        .char_indices()
        .filter(|(_, c)| c.is_ascii_alphabetic())
        .collect();
    let mut path = PathBuilder::new();
    for (i, &(start, command)) in commands.iter().enumerate() {
        let end = commands.get(i + 1).map_or(d.len(), |&(next, _)| next);
        let n = numbers(&d[start + 1..end]);
        let point = |at: usize| Point {
            x: n[at],
            y: n[at + 1],
        };
        match command {
            'M' => path.move_to(point(0)),
            'L' => path.line_to(point(0)),
            'A' => path.svg_arc_to(n[0], n[1], n[2], n[3] != 0.0, n[4] != 0.0, point(5)),
            'z' => path.close(),
            other => panic!("render_svg prints no '{other}' command: {d}"),
        }
    }
    if let Some(transform) = rotation {
        let r = numbers(transform);
        let center = Point { x: r[1], y: r[2] };
        path.map_points(|p| rotate_about(p, center, r[0]));
    }
    path.into_d()
}

/// Each command of a flat scene path, with the points it carries.
fn path_rows(d: &[f64]) -> Vec<(f64, Vec<Point>)> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < d.len() {
        let op = d[i];
        let point_count = if op == OP_MOVE || op == OP_LINE {
            1
        } else if op == OP_CUBIC {
            3
        } else {
            assert_eq!(op, OP_CLOSE, "a scene path holds only the four opcodes");
            0
        };
        let points = (0..point_count)
            .map(|k| Point {
                x: d[i + 1 + 2 * k],
                y: d[i + 2 + 2 * k],
            })
            .collect();
        rows.push((op, points));
        i += 1 + 2 * point_count;
    }
    rows
}

fn opcodes(d: &[f64]) -> Vec<f64> {
    path_rows(d).into_iter().map(|(op, _)| op).collect()
}

fn assert_paths_close(scene: &[f64], svg: &[f64], what: &str) {
    assert_eq!(opcodes(scene), opcodes(svg), "{what}: the commands");
    for (a, b) in scene.iter().zip(svg) {
        assert!(
            close(*a, *b),
            "{what}: the scene has {a} where the SVG draws {b}"
        );
    }
}

/// A label as the SVG prints it: its `<text>`'s style, and each `<tspan>`'s
/// text at the `<text>`'s `y` advanced by every `dy` so far.
struct SvgLabel {
    anchor: String,
    font_size: f64,
    font_weight: u32,
    halo: bool,
    lines: Vec<(String, f64, f64)>,
}

fn style_declaration<'a>(style: &'a str, key: &str) -> Option<&'a str> {
    style
        .split(';')
        .find_map(|declaration| declaration.strip_prefix(key)?.strip_prefix(':'))
}

fn svg_element_labels(tags: &[Tag]) -> Vec<SvgLabel> {
    let mut labels: Vec<SvgLabel> = Vec::new();
    let mut y = 0.0;
    for tag in tags {
        match tag.name.as_str() {
            "text" => {
                let Some(style) = tag.attr("style") else {
                    // A group's label: no tspans, read by the group check.
                    continue;
                };
                let declared = |key: &str| {
                    style_declaration(style, key)
                        .unwrap_or_else(|| panic!("the label style has no {key}: {style}"))
                        .to_string()
                };
                labels.push(SvgLabel {
                    anchor: declared("text-anchor"),
                    font_size: declared("font-size")
                        .trim_end_matches("px")
                        .parse()
                        .unwrap(),
                    font_weight: declared("font-weight").parse().unwrap(),
                    halo: style_declaration(style, "filter") == Some("url(#labelBackground)"),
                    lines: Vec::new(),
                });
                y = tag.num("y");
            }
            "tspan" => {
                let label = labels.last_mut().expect("a tspan belongs to a label");
                let dy = tag.attr("dy").expect("a tspan advances by dy");
                y += if dy == "1em" {
                    label.font_size
                } else {
                    dy.trim_end_matches("px").parse::<f64>().unwrap()
                };
                label.lines.push((tag.text.clone(), tag.num("x"), y));
            }
            _ => {}
        }
    }
    labels
}

// --- The parity oracle ---

/// Asserts the scene draws what the SVG draws: the same elements in the same
/// order; every rectangle, circle, path and label line at the SVG's printed
/// numbers, with the paint its SVG class names; and content bounds the SVG's
/// viewBox pads. Also asserts each element's own promises: it has shapes, and
/// its bounds hold them and its label.
fn assert_scene_draws_the_svg(scene: &Scene, svg: &str, context: &str) {
    let tags = body_tags(svg);

    let svg_kinds: Vec<SceneElementKind> = tags
        .iter()
        .filter_map(|t| match (t.name.as_str(), t.class()) {
            ("g", "simlin-group") => Some(SceneElementKind::Group),
            ("path", "simlin-connector-bg") => Some(SceneElementKind::Link),
            ("g", "simlin-flow") => Some(SceneElementKind::Flow),
            ("g", "simlin-stock") => Some(SceneElementKind::Stock),
            ("path", "simlin-cloud") => Some(SceneElementKind::Cloud),
            ("g", "simlin-module") => Some(SceneElementKind::Module),
            ("g", "simlin-aux") => Some(SceneElementKind::Aux),
            ("g", "simlin-alias") => Some(SceneElementKind::Alias),
            _ => None,
        })
        .collect();
    let scene_kinds: Vec<SceneElementKind> = scene.elements.iter().map(|e| e.kind).collect();
    assert_eq!(
        scene_kinds, svg_kinds,
        "{context}: the elements, in draw order"
    );

    let shapes: Vec<&SceneShape> = scene.elements.iter().flat_map(|e| &e.shapes).collect();

    let svg_rects: Vec<&Tag> = tags.iter().filter(|t| t.name == "rect").collect();
    let rects: Vec<&SceneRectangle> = shapes
        .iter()
        .filter_map(|s| match s {
            SceneShape::Rect(r) => Some(r),
            _ => None,
        })
        .collect();
    assert_eq!(rects.len(), svg_rects.len(), "{context}: rectangles");
    for (rect, tag) in rects.iter().zip(&svg_rects) {
        let paint = match tag.element_class.as_str() {
            "simlin-stock" => ScenePaint::Stock,
            "simlin-module" => ScenePaint::Module,
            "simlin-group" => ScenePaint::Group,
            other => panic!("{context}: a <rect> inside {other}"),
        };
        assert_eq!(rect.paint, paint, "{context}: rectangle paint");
        for (value, key) in [
            (rect.x, "x"),
            (rect.y, "y"),
            (rect.width, "width"),
            (rect.height, "height"),
        ] {
            assert!(
                close(value, tag.num(key)),
                "{context}: rect {key} {value}, the SVG prints {}",
                tag.num(key)
            );
        }
        // SVG's `rx` defaults to 0, and the one corner radius is both axes'.
        assert_eq!(tag.attr("ry"), tag.attr("rx"), "{context}: rx = ry");
        let rx = tag.attr("rx").map_or(0.0, |_| tag.num("rx"));
        assert!(close(rect.corner_radius, rx), "{context}: corner radius");
    }

    let svg_circles: Vec<&Tag> = tags.iter().filter(|t| t.name == "circle").collect();
    let circles: Vec<&SceneCircle> = shapes
        .iter()
        .filter_map(|s| match s {
            SceneShape::Circle(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(circles.len(), svg_circles.len(), "{context}: circles");
    for (circle, tag) in circles.iter().zip(&svg_circles) {
        let paint = match tag.element_class.as_str() {
            "simlin-aux" => ScenePaint::Aux,
            "simlin-alias" => ScenePaint::Alias,
            "simlin-flow" => ScenePaint::Valve,
            other => panic!("{context}: a <circle> inside {other}"),
        };
        assert_eq!(circle.paint, paint, "{context}: circle paint");
        for (value, key) in [(circle.cx, "cx"), (circle.cy, "cy"), (circle.r, "r")] {
            assert!(
                close(value, tag.num(key)),
                "{context}: circle {key} {value}, the SVG prints {}",
                tag.num(key)
            );
        }
    }

    // Paths, sorted by paint into the SVG classes that print them. The match
    // has no wildcard arm, so every paint a path can carry is checked.
    let mut outer = Vec::new();
    let mut inner = Vec::new();
    let mut flow_heads = Vec::new();
    let mut link_heads = Vec::new();
    let mut connectors = Vec::new();
    let mut clouds = Vec::new();
    for shape in &shapes {
        let SceneShape::Path(path) = shape else {
            continue;
        };
        match path.paint {
            ScenePaint::FlowPipeOuter => outer.push(path),
            ScenePaint::FlowPipeInner => inner.push(path),
            ScenePaint::ArrowheadFlow => flow_heads.push(path),
            ScenePaint::ArrowheadLink => link_heads.push(path),
            ScenePaint::Connector | ScenePaint::ConnectorDashed => connectors.push(path),
            ScenePaint::Cloud => clouds.push(path),
            ScenePaint::Stock
            | ScenePaint::Aux
            | ScenePaint::Module
            | ScenePaint::Alias
            | ScenePaint::Valve
            | ScenePaint::Group => panic!("{context}: {:?} is never a path", path.paint),
        }
    }

    for (paths, class) in [(&outer, "simlin-outer"), (&inner, "simlin-inner")] {
        let svg_paths = tags_of(&tags, "path", class);
        assert_eq!(paths.len(), svg_paths.len(), "{context}: {class} paths");
        for (path, tag) in paths.iter().zip(&svg_paths) {
            let oracle = svg_path_as_scene(tag.attr("d").unwrap(), None);
            assert_paths_close(&path.d, &oracle, &format!("{context}: {class}"));
        }
    }

    for (heads, class) in [
        (&flow_heads, "simlin-arrowhead-flow"),
        (&link_heads, "simlin-arrowhead-link"),
    ] {
        let svg_heads = tags_of(&tags, "path", class);
        assert_eq!(heads.len(), svg_heads.len(), "{context}: {class} paths");
        for (head, tag) in heads.iter().zip(&svg_heads) {
            let oracle = svg_path_as_scene(tag.attr("d").unwrap(), tag.attr("transform"));
            assert_paths_close(&head.d, &oracle, &format!("{context}: {class}"));
        }
    }

    let svg_connectors: Vec<&Tag> = tags
        .iter()
        .filter(|t| {
            t.name == "path"
                && matches!(
                    t.class(),
                    "simlin-connector" | "simlin-connector simlin-connector-dashed"
                )
        })
        .collect();
    assert_eq!(
        connectors.len(),
        svg_connectors.len(),
        "{context}: connector paths"
    );
    for (path, tag) in connectors.iter().zip(&svg_connectors) {
        let dashed = tag.class().ends_with("simlin-connector-dashed");
        assert_eq!(
            path.paint == ScenePaint::ConnectorDashed,
            dashed,
            "{context}: a connector the SVG dashes is painted dashed"
        );
        let oracle = svg_path_as_scene(tag.attr("d").unwrap(), None);
        assert_paths_close(&path.d, &oracle, &format!("{context}: connector"));
    }

    let svg_clouds = tags_of(&tags, "path", "simlin-cloud");
    assert_eq!(clouds.len(), svg_clouds.len(), "{context}: cloud paths");
    for (path, tag) in clouds.iter().zip(&svg_clouds) {
        let m = numbers(tag.attr("transform").unwrap());
        let mut outline = parse_absolute_svg_path(tag.attr("d").unwrap())
            .expect("the SVG prints the cloud outline");
        outline.map_points(|p| Point {
            x: m[0] * p.x + m[2] * p.y + m[4],
            y: m[1] * p.x + m[3] * p.y + m[5],
        });
        assert_paths_close(&path.d, &outline.into_d(), &format!("{context}: cloud"));
    }

    let svg_labels = svg_element_labels(&tags);
    let labels: Vec<&SceneLabel> = scene
        .elements
        .iter()
        .filter_map(|e| e.label.as_ref())
        .filter(|l| l.paint == LabelPaint::Label)
        .collect();
    assert_eq!(labels.len(), svg_labels.len(), "{context}: element labels");
    for (label, svg_label) in labels.iter().zip(&svg_labels) {
        assert_eq!(
            serde_json::to_value(label.anchor).unwrap(),
            svg_label.anchor.as_str(),
            "{context}: label anchor"
        );
        assert_eq!(label.baseline, TextBaseline::Alphabetic, "{context}");
        assert_eq!(label.font_size, svg_label.font_size, "{context}: font size");
        assert_eq!(
            label.font_weight, svg_label.font_weight,
            "{context}: font weight"
        );
        assert_eq!(label.halo, svg_label.halo, "{context}: halo");
        assert_eq!(
            label.lines.len(),
            svg_label.lines.len(),
            "{context}: label lines"
        );
        for (line, (text, x, y)) in label.lines.iter().zip(&svg_label.lines) {
            assert_eq!(&line.text, text, "{context}: line text");
            assert!(
                close(line.x, *x) && close(line.y, *y),
                "{context}: line '{text}' at ({}, {}), the SVG places it at ({x}, {y})",
                line.x,
                line.y
            );
        }
    }

    let svg_group_labels: Vec<&Tag> = tags
        .iter()
        .filter(|t| t.name == "text" && t.attr("dominant-baseline") == Some("hanging"))
        .collect();
    let group_labels: Vec<&SceneLabel> = scene
        .elements
        .iter()
        .filter_map(|e| e.label.as_ref())
        .filter(|l| l.paint == LabelPaint::GroupLabel)
        .collect();
    assert_eq!(
        group_labels.len(),
        svg_group_labels.len(),
        "{context}: group labels"
    );
    for (label, tag) in group_labels.iter().zip(&svg_group_labels) {
        assert_eq!(label.baseline, TextBaseline::Hanging, "{context}");
        // The stylesheet's `.simlin-group text { text-anchor: start }`.
        assert_eq!(label.anchor, SceneTextAnchor::Start, "{context}");
        assert_eq!(label.font_weight, GROUP_LABEL_FONT_WEIGHT, "{context}");
        assert!(!label.halo, "{context}: a group label has no halo filter");
        assert_eq!(
            label.lines.len(),
            1,
            "{context}: the SVG prints a group name as one text run"
        );
        let line = &label.lines[0];
        assert_eq!(line.text, tag.text.replace('\n', " "), "{context}");
        assert!(
            close(line.x, tag.num("x")) && close(line.y, tag.num("y")),
            "{context}: group label anchor"
        );
    }

    let root = &svg[..svg.find('>').expect("the SVG has a root tag")];
    let view_box = attributes(root)
        .into_iter()
        .find(|(k, _)| k == "viewBox")
        .map(|(_, v)| numbers(&v))
        .expect("the SVG has a viewBox");
    let expected_view_box = match scene.content_bounds {
        Some(b) => {
            let (left, top) = (b.left.floor() - 10.0, b.top.floor() - 10.0);
            vec![
                left,
                top,
                (b.right - left).ceil() + 10.0,
                (b.bottom - top).ceil() + 10.0,
            ]
        }
        None => vec![0.0, 0.0, 100.0, 100.0],
    };
    assert_eq!(
        view_box, expected_view_box,
        "{context}: the viewBox pads the scene's content bounds"
    );

    for element in &scene.elements {
        assert!(
            !element.shapes.is_empty(),
            "{context}: element {} carries no shapes",
            element.uid
        );
        assert_bounds_hold_the_element(element, context);
    }
}

/// Asserts an element's `bounds` holds every point of its shapes and every
/// label line's anchor.
fn assert_bounds_hold_the_element(element: &SceneElement, context: &str) {
    let mut points: Vec<Point> = Vec::new();
    for shape in &element.shapes {
        match shape {
            SceneShape::Rect(r) => points.extend([
                Point { x: r.x, y: r.y },
                Point {
                    x: r.x + r.width,
                    y: r.y + r.height,
                },
            ]),
            SceneShape::Circle(c) => points.extend([
                Point {
                    x: c.cx - c.r,
                    y: c.cy - c.r,
                },
                Point {
                    x: c.cx + c.r,
                    y: c.cy + c.r,
                },
            ]),
            SceneShape::Path(p) => {
                points.extend(path_rows(&p.d).into_iter().flat_map(|(_, ps)| ps))
            }
        }
    }
    if let Some(label) = &element.label {
        points.extend(label.lines.iter().map(|l| Point { x: l.x, y: l.y }));
    }
    let b = element.bounds;
    for p in points {
        assert!(
            b.left <= p.x && p.x <= b.right && b.top <= p.y && p.y <= b.bottom,
            "{context}: element {}'s bounds {b:?} miss ({}, {})",
            element.uid,
            p.x,
            p.y
        );
    }
}

/// Asserts every element's ident is the canonical name of the variable it
/// displays: its own for a stock, flow, aux or module, its target's for an
/// alias, and none for the rest (`SCENE.md`, "Elements").
fn assert_idents_name_the_displayed_variable(scene: &Scene, project: &datamodel::Project) {
    let View::StockFlow(view) = &project.get_model("main").unwrap().views[0];
    let by_uid = |uid: i32| view.elements.iter().find(|e| e.get_uid() == uid);
    for element in &scene.elements {
        let view_element = by_uid(element.uid).expect("a scene element is a view element");
        let displayed = match view_element {
            ViewElement::Stock(_)
            | ViewElement::Flow(_)
            | ViewElement::Aux(_)
            | ViewElement::Module(_) => view_element.get_name(),
            ViewElement::Alias(alias) => by_uid(alias.alias_of_uid).and_then(|e| e.get_name()),
            ViewElement::Group(_) | ViewElement::Link(_) | ViewElement::Cloud(_) => None,
        };
        assert_eq!(
            element.ident.as_deref(),
            displayed.map(canonicalize).as_deref(),
            "element {}",
            element.uid
        );
    }
}

// --- Tests ---

#[test]
fn the_scene_draws_what_the_svg_draws_for_corpus_models() {
    for model in [
        "test-models/samples/teacup/teacup_w_diagram.xmile",
        "test-models/samples/SIR/SIR.xmile",
        "alias1/alias1.stmx",
        "test-models/samples/bpowers-hares_and_lynxes_modules/model.stmx",
    ] {
        let project = open_corpus_model(model);
        let (scene, svg) = build(&project);
        assert!(!scene.elements.is_empty(), "{model} draws something");
        assert_scene_draws_the_svg(&scene, &svg, model);
        assert_idents_name_the_displayed_variable(&scene, &project);
    }
}

#[test]
fn every_kind_is_drawn_with_its_contract_row() {
    let project = every_kind_project();
    let (scene, svg) = build(&project);
    assert_scene_draws_the_svg(&scene, &svg, "every kind");
    assert_idents_name_the_displayed_variable(&scene, &project);
    for kind in ALL_KINDS {
        let (layer, has_ident, has_sparkline, label_paint) = contract_row(kind);
        let of_kind: Vec<&SceneElement> =
            scene.elements.iter().filter(|e| e.kind == kind).collect();
        assert_eq!(of_kind.len(), 1, "{kind:?}: one element of each kind");
        let element = of_kind[0];
        assert_eq!(element.layer, layer, "{kind:?}: layer");
        assert_eq!(element.ident.is_some(), has_ident, "{kind:?}: ident");
        assert_eq!(
            element.sparkline.is_some(),
            has_sparkline,
            "{kind:?}: sparkline slot"
        );
        assert_eq!(
            element.label.as_ref().map(|l| l.paint),
            label_paint,
            "{kind:?}: label"
        );
    }
    assert_eq!(scene.elements.len(), ALL_KINDS.len());
}

#[test]
fn stocks_auxes_and_flows_stack_three_copies_when_arrayed() {
    // The rows are the equation's two shapes. The kinds checked in each are
    // the three whose drawing asks `is_arrayed` (`resolve_view`): an alias
    // never stacks (covered by the alias test), and a module, cloud, link or
    // group has no equation of its own.
    for arrayed in [false, true] {
        let builder = TestProject::new("arrayed").named_dimension("region", &["north", "south"]);
        let builder = if arrayed {
            builder
                .array_aux("growth_rate[region]", "0.1")
                .array_stock("population[region]", "100", &["births"], &[], None)
                .array_flow("births[region]", "population * growth_rate", None)
        } else {
            builder
                .aux("growth_rate", "0.1", None)
                .stock("population", "100", &["births"], &[], None)
                .flow("births", "population * growth_rate", None)
        };
        let project = with_view(
            builder,
            vec![
                aux("growth_rate", 1, 225.0, 40.0),
                stock("population", 2, 300.0, 100.0),
                cloud(3, 4, 150.0, 100.0),
                flow(
                    "births",
                    4,
                    225.0,
                    100.0,
                    &[(150.0, 100.0, Some(3)), (277.5, 100.0, Some(2))],
                ),
            ],
        );
        let (scene, svg) = build(&project);
        let row = if arrayed { "arrayed" } else { "scalar" };
        assert_scene_draws_the_svg(&scene, &svg, row);

        let copies = if arrayed { 3 } else { 1 };
        for (uid, paint) in [
            (1, ScenePaint::Aux),
            (2, ScenePaint::Stock),
            (4, ScenePaint::Valve),
        ] {
            let element = element_by_uid(&scene, uid);
            let kind = element.kind;
            assert_eq!(element.is_arrayed, arrayed, "{row} {kind:?}");
            let stacked: Vec<&SceneShape> = element
                .shapes
                .iter()
                .filter(|s| shape_paint(s) == paint)
                .collect();
            assert_eq!(stacked.len(), copies, "{row} {kind:?}: copies");
            // `Stock.tsx`, `Auxiliary.tsx` and `Flow.tsx` translate the
            // sparkline to `cx - arrayedOffset + 1 - w / 2` with width `w - 2`
            // (`r` for a circle): inside the front copy, the one drawn last.
            let slot = element.sparkline.expect("a stock, aux or flow has a slot");
            let expected = match stacked.last() {
                Some(SceneShape::Rect(front)) => SparklineSlot {
                    x: front.x + 1.0,
                    y: front.y + 1.0,
                    width: front.width - 2.0,
                    height: front.height - 2.0,
                },
                Some(SceneShape::Circle(front)) => SparklineSlot {
                    x: front.cx + 1.0 - front.r / 2.0,
                    y: front.cy + 1.0 - front.r / 2.0,
                    width: front.r - 2.0,
                    height: front.r - 2.0,
                },
                other => panic!("{row} {kind:?}: no front copy: {other:?}"),
            };
            for (a, b) in [
                (slot.x, expected.x),
                (slot.y, expected.y),
                (slot.width, expected.width),
                (slot.height, expected.height),
            ] {
                assert!(
                    close(a, b),
                    "{row} {kind:?}: the slot {slot:?} is not in the front copy"
                );
            }
        }
    }
}

#[test]
fn a_repeated_ident_draws_as_its_first_variable() {
    // Rows: which of two variables sharing one canonical ident is arrayed. The
    // first decides, the variable `Model::get_variable` finds, whatever the
    // second's equation.
    for first_arrayed in [false, true] {
        let builder = TestProject::new("repeated").named_dimension("region", &["north", "south"]);
        let builder = if first_arrayed {
            builder
                .array_aux("growth_rate[region]", "0.1")
                .aux("Growth Rate", "0.1", None)
        } else {
            builder
                .aux("growth_rate", "0.1", None)
                .array_aux("Growth Rate[region]", "0.1")
        };
        let project = with_view(builder, vec![aux("growth_rate", 1, 100.0, 100.0)]);
        let (scene, svg) = build(&project);
        let row = if first_arrayed {
            "arrayed first"
        } else {
            "scalar first"
        };
        assert_scene_draws_the_svg(&scene, &svg, row);
        let element = element_by_uid(&scene, 1);
        assert_eq!(element.is_arrayed, first_arrayed, "{row}");
        let copies = if first_arrayed { 3 } else { 1 };
        assert_eq!(paints(element), vec![ScenePaint::Aux; copies], "{row}");
    }
}

#[test]
fn an_alias_shows_its_targets_ident_and_sparkline_and_a_missing_target_neither() {
    // Rows: the target is a scalar aux in the view, an arrayed aux in the
    // view, or absent from the view.
    for (row, target_arrayed, alias_of_uid) in [
        ("scalar target", false, 1),
        ("arrayed target", true, 1),
        ("missing target", false, 99),
    ] {
        let builder = TestProject::new("alias").named_dimension("region", &["north", "south"]);
        let builder = if target_arrayed {
            builder.array_aux("growth_rate[region]", "0.1")
        } else {
            builder.aux("growth_rate", "0.1", None)
        };
        let project = with_view(
            builder,
            vec![
                aux("growth_rate", 1, 100.0, 100.0),
                alias(2, alias_of_uid, 200.0, 100.0),
            ],
        );
        let (scene, svg) = build(&project);
        assert_scene_draws_the_svg(&scene, &svg, row);

        let shown = element_by_uid(&scene, 2);
        let target_present = alias_of_uid == 1;
        assert_eq!(
            shown.ident.as_deref(),
            target_present.then_some("growth_rate"),
            "{row}: ident"
        );
        assert_eq!(
            shown.sparkline.is_some(),
            target_present,
            "{row}: sparkline slot"
        );
        // `Alias.tsx` draws one circle, `isArrayed = false`, whatever the
        // target's equation.
        assert!(!shown.is_arrayed, "{row}");
        assert_eq!(paints(shown), vec![ScenePaint::Alias], "{row}");
        let label = shown.label.as_ref().expect("an alias is labeled");
        let text = if target_present {
            "growth rate"
        } else {
            "unknown alias"
        };
        assert_eq!(
            label
                .lines
                .iter()
                .map(|l| l.text.as_str())
                .collect::<Vec<_>>(),
            vec![text],
            "{row}: label"
        );
    }
}

#[test]
fn a_flow_into_a_cloud_stops_at_the_clouds_edge_and_into_a_stock_at_its_last_point() {
    for sink_is_cloud in [true, false] {
        let row = if sink_is_cloud {
            "cloud sink"
        } else {
            "stock sink"
        };
        let sink = if sink_is_cloud {
            cloud(5, 4, 250.0, 100.0)
        } else {
            stock("population", 5, 272.5, 100.0)
        };
        let project = with_view(
            TestProject::new("flow-sink")
                .stock("population", "100", &["births"], &[], None)
                .flow("births", "1", None),
            vec![
                cloud(3, 4, 100.0, 100.0),
                flow(
                    "births",
                    4,
                    175.0,
                    100.0,
                    &[(100.0, 100.0, Some(3)), (250.0, 100.0, Some(5))],
                ),
                sink,
            ],
        );
        let (scene, svg) = build(&project);
        assert_scene_draws_the_svg(&scene, &svg, row);

        let drawn = element_by_uid(&scene, 4);
        assert_eq!(
            paints(drawn),
            vec![
                ScenePaint::FlowPipeOuter,
                ScenePaint::ArrowheadFlow,
                ScenePaint::FlowPipeInner,
                ScenePaint::Valve,
            ],
            "{row}: the SVG group's order"
        );
        let SceneShape::Path(head) = &drawn.shapes[1] else {
            panic!("{row}: the arrowhead is a path");
        };
        // A cloud sink pulls the arrowhead back along the pipe to the cloud's
        // edge; a stock sink leaves it on the flow's last point.
        let tip_x = if sink_is_cloud {
            250.0 - CLOUD_RADIUS
        } else {
            250.0
        };
        assert_eq!(
            (head.d[0], head.d[1], head.d[2]),
            (OP_MOVE, tip_x, 100.0),
            "{row}: the arrowhead's tip"
        );
    }
}

#[test]
fn a_link_is_straight_or_arced_and_dashed_into_a_stock() {
    for shape in [LinkShape::Straight, LinkShape::Arc(30.0)] {
        for into_stock in [false, true] {
            let arced = matches!(shape, LinkShape::Arc(_));
            let row = format!("arced={arced} into_stock={into_stock}");
            let builder = TestProject::new("links").aux("source", "1", None);
            let (builder, target) = if into_stock {
                (
                    builder.stock("target", "source", &[], &[], None),
                    stock("target", 2, 200.0, 200.0),
                )
            } else {
                (
                    builder.aux("target", "source", None),
                    aux("target", 2, 200.0, 200.0),
                )
            };
            let project = with_view(
                builder,
                vec![
                    aux("source", 1, 100.0, 100.0),
                    target,
                    link(3, 1, 2, shape.clone()),
                ],
            );
            let (scene, svg) = build(&project);
            assert_scene_draws_the_svg(&scene, &svg, &row);

            let drawn = element_by_uid(&scene, 3);
            let line_paint = if into_stock {
                ScenePaint::ConnectorDashed
            } else {
                ScenePaint::Connector
            };
            assert_eq!(
                paints(drawn),
                vec![line_paint, ScenePaint::ArrowheadLink],
                "{row}"
            );
            let SceneShape::Path(line) = &drawn.shapes[0] else {
                panic!("{row}: the line is a path");
            };
            let commands = opcodes(&line.d);
            if arced {
                assert_eq!(commands[0], OP_MOVE, "{row}");
                assert!(
                    commands.len() > 1 && commands[1..].iter().all(|&op| op == OP_CUBIC),
                    "{row}: an arc is drawn as cubics: {commands:?}"
                );
            } else {
                assert_eq!(commands, vec![OP_MOVE, OP_LINE], "{row}");
            }
        }
    }
}

#[test]
fn a_module_rect_is_snapped_up_to_whole_units() {
    let project = with_view(
        TestProject::new("module"),
        vec![module("sub", 1, 100.25, 200.75)],
    );
    let (scene, svg) = build(&project);
    assert_scene_draws_the_svg(&scene, &svg, "module");

    let drawn = element_by_uid(&scene, 1);
    let SceneShape::Rect(rect) = &drawn.shapes[0] else {
        panic!("a module is a rectangle");
    };
    // The web canvas rounds a module's corner up: 100.25 - 55 / 2 = 72.75
    // draws at 73, and 200.75 - 45 / 2 = 178.25 at 179.
    assert_eq!((rect.x, rect.y), (73.0, 179.0));
    assert_eq!(
        (rect.width, rect.height, rect.corner_radius),
        (MODULE_WIDTH, MODULE_HEIGHT, MODULE_RADIUS)
    );
    assert_eq!(drawn.ident.as_deref(), Some("sub"));
}

#[test]
fn a_group_is_a_box_with_a_hanging_start_anchored_label() {
    // Rows: a one-line name, and a name holding the stored line-break escape,
    // which the SVG prints inside one text run.
    for (name, text) in [
        ("climate_sector", "climate sector"),
        ("two\\nlines", "two lines"),
    ] {
        let project = with_view(
            TestProject::new("group").aux("inside", "1", None),
            vec![
                group(name, 1, 200.0, 150.0, 300.0, 200.0),
                aux("inside", 2, 200.0, 150.0),
            ],
        );
        let (scene, svg) = build(&project);
        assert_scene_draws_the_svg(&scene, &svg, name);

        let drawn = element_by_uid(&scene, 1);
        assert_eq!(paints(drawn), vec![ScenePaint::Group], "{name}");
        let SceneShape::Rect(rect) = &drawn.shapes[0] else {
            panic!("{name}: a group is a rectangle");
        };
        assert_eq!(
            (rect.x, rect.y, rect.width, rect.height, rect.corner_radius),
            (50.0, 50.0, 300.0, 200.0, GROUP_RADIUS),
            "{name}"
        );
        let label = drawn.label.as_ref().expect("a group is labeled");
        assert_eq!(
            label.lines,
            vec![SceneLabelLine {
                text: text.to_string(),
                x: 58.0,
                y: 58.0,
            }],
            "{name}"
        );
    }
}

#[test]
fn an_element_the_svg_does_not_draw_is_not_in_the_scene() {
    let base = vec![
        aux("a", 1, 100.0, 100.0),
        aux("b", 2, 200.0, 200.0),
        cloud(3, 50, 100.0, 300.0),
        stock("s", 5, 300.0, 300.0),
    ];
    let builder = || {
        TestProject::new("skips")
            .aux("a", "1", None)
            .aux("b", "a", None)
            .stock("s", "0", &["f"], &[], None)
            .flow("f", "1", None)
    };
    let without = build_scene(&with_view(builder(), base.clone()), "main").unwrap();
    let flow_between = |source: Option<i32>, sink: Option<i32>| {
        flow(
            "f",
            50,
            200.0,
            300.0,
            &[(100.0, 300.0, source), (277.5, 300.0, sink)],
        )
    };
    // The rows are the arms by which `resolve_view` leaves an element out,
    // plus the connector `connector_geometry` cannot draw, each beside a
    // control that the same fixture draws.
    let rows = [
        (
            "control: a link between two drawn elements",
            link(50, 1, 2, LinkShape::Straight),
            true,
        ),
        (
            "a link whose source is not in the view",
            link(50, 99, 2, LinkShape::Straight),
            false,
        ),
        (
            "a link whose target is not in the view",
            link(50, 1, 99, LinkShape::Straight),
            false,
        ),
        (
            "a multi-point link, which the SVG prints as an empty group",
            link(50, 1, 2, LinkShape::MultiPoint(vec![])),
            false,
        ),
        (
            "control: a flow between a drawn cloud and stock",
            flow_between(Some(3), Some(5)),
            true,
        ),
        (
            "a flow with one point",
            flow("f", 50, 200.0, 300.0, &[(100.0, 300.0, Some(3))]),
            false,
        ),
        (
            "a flow whose first point is attached to nothing",
            flow_between(None, Some(5)),
            false,
        ),
        (
            "a flow whose source is not in the view",
            flow_between(Some(99), Some(5)),
            false,
        ),
        (
            "a flow whose last point is attached to nothing",
            flow_between(Some(3), None),
            false,
        ),
        (
            "a flow whose sink is not in the view",
            flow_between(Some(3), Some(99)),
            false,
        ),
    ];
    for (row, extra, drawn) in rows {
        let mut elements = base.clone();
        elements.push(extra);
        let project = with_view(builder(), elements);
        let (scene, svg) = build(&project);
        assert_scene_draws_the_svg(&scene, &svg, row);
        assert_eq!(scene.elements.iter().any(|e| e.uid == 50), drawn, "{row}");
        if !drawn {
            assert_eq!(scene, without, "{row}: the scene is the scene without it");
        }
    }
}

#[test]
fn elements_draw_in_layer_order_and_in_view_order_within_a_layer() {
    let project = with_view(
        TestProject::new("order")
            .aux("a", "1", None)
            .aux("b", "1", None)
            .stock("s", "0", &[], &[], None)
            .stock("t", "0", &["f"], &[], None)
            .flow("f", "1", None),
        vec![
            aux("a", 1, 100.0, 100.0),
            stock("s", 2, 300.0, 100.0),
            link(3, 1, 2, LinkShape::Straight),
            group("g", 4, 200.0, 200.0, 400.0, 400.0),
            aux("b", 5, 100.0, 200.0),
            cloud(6, 8, 150.0, 300.0),
            module("m", 7, 400.0, 300.0),
            flow(
                "f",
                8,
                200.0,
                300.0,
                &[(150.0, 300.0, Some(6)), (277.5, 300.0, Some(10))],
            ),
            alias(9, 1, 100.0, 350.0),
            stock("t", 10, 300.0, 300.0),
        ],
    );
    let (scene, svg) = build(&project);
    assert_scene_draws_the_svg(&scene, &svg, "order");
    assert_eq!(
        scene.elements.iter().map(|e| e.uid).collect::<Vec<_>>(),
        vec![4, 3, 8, 2, 6, 7, 10, 1, 5, 9],
        "groups, links, flows, then stocks, clouds and modules, then auxes and aliases"
    );
    assert!(
        scene
            .elements
            .windows(2)
            .all(|pair| pair[0].layer <= pair[1].layer)
    );
}

/// Every label side's name. The match has no wildcard arm, so a new side does
/// not compile until it has a name, and so a row, here.
fn side_name(side: LabelSide) -> &'static str {
    match side {
        LabelSide::Top => "top",
        LabelSide::Left => "left",
        LabelSide::Center => "center",
        LabelSide::Bottom => "bottom",
        LabelSide::Right => "right",
    }
}

#[test]
fn label_lines_sit_where_the_svg_places_them_on_every_side() {
    for side in [
        LabelSide::Top,
        LabelSide::Left,
        LabelSide::Center,
        LabelSide::Bottom,
        LabelSide::Right,
    ] {
        for (name, lines) in [
            ("alpha", vec!["alpha"]),
            ("alpha\\nbeta", vec!["alpha", "beta"]),
        ] {
            let row = format!("{} with {} line(s)", side_name(side), lines.len());
            let project = with_view(
                TestProject::new("labels").aux(name, "1", None),
                vec![aux_on(name, 1, 100.0, 200.0, side)],
            );
            let (scene, svg) = build(&project);
            assert_scene_draws_the_svg(&scene, &svg, &row);
            let label = element_by_uid(&scene, 1)
                .label
                .as_ref()
                .expect("an aux is labeled");
            assert_eq!(
                label
                    .lines
                    .iter()
                    .map(|l| l.text.as_str())
                    .collect::<Vec<_>>(),
                lines,
                "{row}"
            );
        }
    }
}

#[test]
fn a_cloud_is_the_shared_outline_placed_on_its_circle() {
    let element = view_element::Cloud {
        uid: 1,
        flow_uid: 2,
        x: 100.0,
        y: 200.0,
        compat: None,
    };
    let project = with_view(
        TestProject::new("cloud"),
        vec![ViewElement::Cloud(element.clone())],
    );
    let (scene, svg) = build(&project);
    assert_scene_draws_the_svg(&scene, &svg, "cloud");

    let drawn = element_by_uid(&scene, 1);
    let SceneShape::Path(outline) = &drawn.shapes[0] else {
        panic!("a cloud is a path");
    };
    // The outline's design box maps onto the cloud's circle, so its control
    // points lie inside the box fit-to-content folds in for the cloud.
    let points = control_point_bounds(&outline.d).expect("the outline has points");
    let circle = cloud_bounds(&element);
    assert!(
        points.left >= circle.left
            && points.right <= circle.right
            && points.top >= circle.top
            && points.bottom <= circle.bottom,
        "the outline leaves the cloud's circle"
    );
    assert_eq!(scene.content_bounds, Some(SceneBounds::from(circle)));
}

#[test]
fn the_json_is_the_contracts_shape() {
    fn keys(value: &serde_json::Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    let scene = build_scene(&every_kind_project(), "main").unwrap();
    let json: serde_json::Value = serde_json::from_str(&scene.to_json().unwrap()).unwrap();
    assert_eq!(
        keys(&json),
        ["contentBounds", "elements", "modelName", "version"]
    );
    assert_eq!(json["version"], 1);
    assert_eq!(json["modelName"], "main");
    assert_eq!(
        keys(&json["contentBounds"]),
        ["bottom", "left", "right", "top"]
    );

    let mut shape_types = Vec::new();
    for element in json["elements"].as_array().unwrap() {
        assert_eq!(
            keys(element),
            [
                "bounds",
                "ident",
                "isArrayed",
                "kind",
                "label",
                "layer",
                "shapes",
                "sparkline",
                "uid"
            ]
        );
        assert_eq!(keys(&element["bounds"]), ["bottom", "left", "right", "top"]);
        if !element["sparkline"].is_null() {
            assert_eq!(keys(&element["sparkline"]), ["height", "width", "x", "y"]);
        }
        if !element["label"].is_null() {
            assert_eq!(
                keys(&element["label"]),
                [
                    "anchor",
                    "baseline",
                    "fontSize",
                    "fontWeight",
                    "halo",
                    "lines",
                    "paint"
                ]
            );
            for line in element["label"]["lines"].as_array().unwrap() {
                assert_eq!(keys(line), ["text", "x", "y"]);
            }
        }
        for shape in element["shapes"].as_array().unwrap() {
            let shape_type = shape["type"].as_str().unwrap().to_string();
            match shape_type.as_str() {
                "rect" => assert_eq!(
                    keys(shape),
                    ["cornerRadius", "height", "paint", "type", "width", "x", "y"]
                ),
                "circle" => assert_eq!(keys(shape), ["cx", "cy", "paint", "r", "type"]),
                "path" => {
                    assert_eq!(keys(shape), ["d", "paint", "type"]);
                    // A flat number array, opening with a move.
                    assert_eq!(shape["d"][0], OP_MOVE);
                    assert!(
                        shape["d"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .all(serde_json::Value::is_number)
                    );
                }
                other => panic!("the contract has no '{other}' shape"),
            }
            shape_types.push(shape_type);
        }
    }
    for shape_type in ["rect", "circle", "path"] {
        assert!(
            shape_types.iter().any(|t| t == shape_type),
            "the fixture draws a {shape_type}, so its arm above ran"
        );
    }
}

#[test]
fn every_enumerated_value_serializes_as_the_contract_names_it() {
    // Each row list is its enum's variants; each match has no wildcard arm,
    // so a new variant does not compile without a contract name.
    for paint in [
        ScenePaint::Stock,
        ScenePaint::Aux,
        ScenePaint::Module,
        ScenePaint::Alias,
        ScenePaint::Valve,
        ScenePaint::FlowPipeOuter,
        ScenePaint::FlowPipeInner,
        ScenePaint::ArrowheadFlow,
        ScenePaint::Cloud,
        ScenePaint::Connector,
        ScenePaint::ConnectorDashed,
        ScenePaint::ArrowheadLink,
        ScenePaint::Group,
    ] {
        let name = match paint {
            ScenePaint::Stock => "stock",
            ScenePaint::Aux => "aux",
            ScenePaint::Module => "module",
            ScenePaint::Alias => "alias",
            ScenePaint::Valve => "valve",
            ScenePaint::FlowPipeOuter => "flowPipeOuter",
            ScenePaint::FlowPipeInner => "flowPipeInner",
            ScenePaint::ArrowheadFlow => "arrowheadFlow",
            ScenePaint::Cloud => "cloud",
            ScenePaint::Connector => "connector",
            ScenePaint::ConnectorDashed => "connectorDashed",
            ScenePaint::ArrowheadLink => "arrowheadLink",
            ScenePaint::Group => "group",
        };
        assert_eq!(serde_json::to_value(paint).unwrap(), name);
    }
    for kind in ALL_KINDS {
        let name = match kind {
            SceneElementKind::Group => "group",
            SceneElementKind::Link => "link",
            SceneElementKind::Flow => "flow",
            SceneElementKind::Stock => "stock",
            SceneElementKind::Cloud => "cloud",
            SceneElementKind::Module => "module",
            SceneElementKind::Aux => "aux",
            SceneElementKind::Alias => "alias",
        };
        assert_eq!(serde_json::to_value(kind).unwrap(), name);
    }
    for paint in [LabelPaint::Label, LabelPaint::GroupLabel] {
        let name = match paint {
            LabelPaint::Label => "label",
            LabelPaint::GroupLabel => "groupLabel",
        };
        assert_eq!(serde_json::to_value(paint).unwrap(), name);
    }
    for anchor in [
        SceneTextAnchor::Start,
        SceneTextAnchor::Middle,
        SceneTextAnchor::End,
    ] {
        let name = match anchor {
            SceneTextAnchor::Start => "start",
            SceneTextAnchor::Middle => "middle",
            SceneTextAnchor::End => "end",
        };
        assert_eq!(serde_json::to_value(anchor).unwrap(), name);
    }
    for baseline in [TextBaseline::Alphabetic, TextBaseline::Hanging] {
        let name = match baseline {
            TextBaseline::Alphabetic => "alphabetic",
            TextBaseline::Hanging => "hanging",
        };
        assert_eq!(serde_json::to_value(baseline).unwrap(), name);
    }
    assert_eq!(
        [OP_MOVE, OP_LINE, OP_CUBIC, OP_CLOSE],
        [0.0, 1.0, 2.0, 3.0],
        "the contract's opcodes"
    );
    assert_eq!(SCENE_VERSION, 1);
}

#[test]
fn an_empty_view_has_nothing_to_fit() {
    let project = with_view(TestProject::new("empty"), vec![]);
    let (scene, svg) = build(&project);
    assert_scene_draws_the_svg(&scene, &svg, "empty");
    assert!(scene.elements.is_empty());
    assert_eq!(scene.content_bounds, None);
    assert!(scene.to_json().unwrap().contains("\"contentBounds\":null"));
}

#[test]
fn build_scene_refuses_what_render_svg_refuses() {
    let viewless = TestProject::new("viewless")
        .aux("a", "1", None)
        .build_datamodel();
    let with_a_view = with_view(TestProject::new("absent"), vec![]);
    for (row, project, model) in [
        ("a model the project does not hold", &with_a_view, "absent"),
        ("a model with no stock-and-flow view", &viewless, "main"),
    ] {
        let scene_err = build_scene(project, model).expect_err(row);
        let svg_err = render_svg(project, model).expect_err(row);
        assert_eq!(scene_err, svg_err, "{row}");
    }
}

#[test]
fn an_element_whose_geometry_is_not_finite_is_not_drawn() {
    // A view element's coordinates are whatever its file held. The SVG prints
    // a NaN coordinate as `NaN`, which draws nothing; this is the arm of
    // `finish` that leaves such an element out rather than handing a consumer
    // NaN.
    let project = with_view(
        TestProject::new("nan")
            .aux("finite", "1", None)
            .aux("broken", "1", None),
        vec![
            aux("finite", 1, 100.0, 100.0),
            aux("broken", 2, f64::NAN, 100.0),
        ],
    );
    let (scene, svg) = build(&project);
    assert!(
        svg.contains("cx=\"NaN\""),
        "the SVG prints the broken aux at NaN"
    );
    assert_eq!(
        scene.elements.iter().map(|e| e.uid).collect::<Vec<_>>(),
        vec![1]
    );
}
