// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a scene is checked by: the SVG `render_svg` prints for the same project,
//! read back. Here rather than in the tests because the corpus models' scenes
//! are checked from simlin-tests, which can open them.

use super::*;

use crate::path::{OP_CLOSE, OP_CUBIC, OP_LINE, OP_MOVE};
use simlin_base::datamodel::{View, ViewElement};

/// How far a scene number may sit from its printed SVG twin. The SVG quantizes
/// every coordinate to six decimals (`js_format_number`), which moves a number
/// by at most 5e-7, and a point rebuilt from printed numbers (an arc from a
/// quantized radius, the cloud outline under a quantized scale) by a few times
/// that.
pub(super) const TOLERANCE: f64 = 1e-5;

pub(super) fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE
}

// --- Reading the SVG ---

/// One tag of the SVG's drawn body (everything after `</defs>`).
pub(super) struct Tag {
    pub(super) name: String,
    pub(super) attrs: Vec<(String, String)>,
    /// The text between this tag and the next.
    pub(super) text: String,
    /// The class of the latest `simlin-*` element group opened before this
    /// tag: which element a `<rect>` or `<circle>` belongs to.
    pub(super) element_class: String,
}

impl Tag {
    pub(super) fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub(super) fn num(&self, key: &str) -> f64 {
        let value = self
            .attr(key)
            .unwrap_or_else(|| panic!("<{}> has no {key}", self.name));
        value
            .parse()
            .unwrap_or_else(|e| panic!("<{}> {key}=\"{value}\": {e}", self.name))
    }

    pub(super) fn class(&self) -> &str {
        self.attr("class").unwrap_or("")
    }
}

pub(super) fn body_tags(svg: &str) -> Vec<Tag> {
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

pub(super) fn attributes(mut text: &str) -> Vec<(String, String)> {
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

pub(super) fn tags_of<'a>(tags: &'a [Tag], name: &str, class: &str) -> Vec<&'a Tag> {
    tags.iter()
        .filter(|t| t.name == name && t.class() == class)
        .collect()
}

/// Every number in an SVG path, transform or viewBox, in order.
/// `js_format_number` prints no exponents, so digits, the point and the sign
/// are a number's only characters.
pub(super) fn numbers(text: &str) -> Vec<f64> {
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
pub(super) fn svg_path_as_scene(d: &str, rotation: Option<&str>) -> Vec<f64> {
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
pub(super) fn path_rows(d: &[f64]) -> Vec<(f64, Vec<Point>)> {
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

pub(super) fn opcodes(d: &[f64]) -> Vec<f64> {
    path_rows(d).into_iter().map(|(op, _)| op).collect()
}

pub(super) fn assert_paths_close(scene: &[f64], svg: &[f64], what: &str) {
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
pub(super) struct SvgLabel {
    pub(super) anchor: String,
    pub(super) font_size: f64,
    pub(super) font_weight: u32,
    pub(super) halo: bool,
    pub(super) lines: Vec<(String, f64, f64)>,
}

pub(super) fn style_declaration<'a>(style: &'a str, key: &str) -> Option<&'a str> {
    style
        .split(';')
        .find_map(|declaration| declaration.strip_prefix(key)?.strip_prefix(':'))
}

pub(super) fn svg_element_labels(tags: &[Tag]) -> Vec<SvgLabel> {
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
pub fn assert_scene_draws_the_svg(scene: &Scene, svg: &str, context: &str) {
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
pub(super) fn assert_bounds_hold_the_element(element: &SceneElement, context: &str) {
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
pub fn assert_idents_name_the_displayed_variable(scene: &Scene, project: &datamodel::Project) {
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
