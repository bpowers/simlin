// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests of hit testing.
//!
//! Rows are derived from `HitPart::ALL`: each part has a point that lands on
//! it, read off the diagram's own geometry rather than restated. The ranking
//! rows pin each tier of `hit_test`'s precedence: a body firmly holding the
//! point beats an end handle beneath it; an end handle beats the body whose edge
//! it touches (the `Arrowhead` part row) and a label drawn above it; a label
//! drawn over a body beats the body; the nearest drawing within the tolerance
//! wins otherwise, nothing beyond the tolerance is hit, and a group's interior
//! belongs to what it holds. No row puts an end handle above a firm body (a
//! flow's end over a later flow's valve): that arm is the same top-down walk
//! the label-over-body test pins.
//!
//! A hit lands only on what the scene draws. Rows are derived from the kinds of
//! element, each holding a coordinate that is not finite beside a drawn aux,
//! and at every point of a grid over the scene the element and the part a hit
//! lands on must be drawn by the production scene (`build_scene`).

use crate::datamodel::ViewElement;
use crate::diagram::{ScenePaint, SceneShape, build_scene};
use crate::editing::test_support::{
    alias, aux, cloud, flow, labeled, link, load, project_of, stock, view_of,
};
use crate::json;

use super::*;

const TOLERANCE: f64 = 10.0;

/// Two stocks joined by a flow inside a group, a cloud-to-cloud flow, and an
/// aux linked to that flow.
fn scene() -> datamodel::Project {
    project_of(load(vec![
        json::ViewElement::Group(json::GroupViewElement {
            uid: 10,
            name: "g".to_string(),
            x: 40.0,
            y: 40.0,
            width: 440.0,
            height: 120.0,
            is_mdl_view_marker: false,
        }),
        labeled(stock(1, 100.0, 100.0), "bottom"),
        labeled(stock(2, 400.0, 100.0), "bottom"),
        labeled(
            flow(3, (250.0, 100.0), &[(122.5, 100.0, 1), (377.5, 100.0, 2)]),
            "bottom",
        ),
        labeled(
            flow(4, (200.0, 300.0), &[(100.0, 300.0, 5), (300.0, 300.0, 6)]),
            "bottom",
        ),
        cloud(5, 4, 100.0, 300.0),
        cloud(6, 4, 300.0, 300.0),
        labeled(aux(7, 250.0, 450.0), "bottom"),
        link(8, 7, 4, None),
    ]))
}

fn element(project: &datamodel::Project, uid: i32) -> &ViewElement {
    view_of(project)
        .iter()
        .find(|e| e.get_uid() == uid)
        .unwrap_or_else(|| panic!("no element {uid}"))
}

/// A point that lands on `part`, and the hit it lands on. Exhaustive, so a new
/// part does not compile until it has a row.
fn part_row(part: HitPart, project: &datamodel::Project) -> (Point, Hit) {
    let hit = |uid| Hit { uid, part };
    match part {
        HitPart::Body => (Point::new(400.0, 100.0), hit(2)),
        // The tip touches the sink stock's face, which draws above the flow.
        HitPart::Arrowhead => {
            let ViewElement::Flow(f) = element(project, 3) else {
                panic!("3 is a flow");
            };
            let g = flow_geometry(f, element(project, 2), false).expect("a drawable flow");
            (Point::new(g.arrowhead.tip.x, g.arrowhead.tip.y), hit(3))
        }
        // Just past the source stock's face, on the pipe.
        HitPart::Source => (Point::new(126.0, 100.0), hit(3)),
        HitPart::Label => {
            let ViewElement::Aux(a) = element(project, 7) else {
                panic!("7 is an aux");
            };
            let r = label_bounds(&aux_geometry(a, false).label);
            (
                Point::new((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0),
                hit(7),
            )
        }
    }
}

#[test]
fn each_part_has_a_point_that_lands_on_it() {
    let project = scene();
    let mut failures = Vec::new();
    for part in HitPart::ALL {
        let (point, want) = part_row(part, &project);
        let got = hit_test(&project, "main", point, TOLERANCE).expect("main has a view");
        if got != Some(want) {
            failures.push(format!("{part:?}: {got:?}, want {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_tiers_rank_bodies_handles_labels_and_nearness() {
    let project = scene();
    let link_end = {
        let ViewElement::Link(l) = element(&project, 8) else {
            panic!("8 is a link");
        };
        match connector_geometry(
            l,
            element(&project, l.from_uid),
            element(&project, l.to_uid),
            &|_| false,
        ) {
            ConnectorGeometry::Straight(g) => Point::new(g.end.x, g.end.y),
            ConnectorGeometry::Arc(g) => Point::new(g.end.x, g.end.y),
            ConnectorGeometry::Undrawable => panic!("link 8 draws nothing"),
        }
    };
    /// A name, a point, the tolerance, and the element and part the point lands on.
    type Row = (&'static str, Point, f64, Option<(i32, HitPart)>);
    let rows: [Row; 8] = [
        (
            "an aux holding the point beats the link starting at its edge",
            Point::new(249.0, 443.0),
            TOLERANCE,
            Some((7, HitPart::Body)),
        ),
        (
            "a point firmly inside a stock is the stock, though a flow's end touches its face",
            Point::new(381.0, 100.0),
            TOLERANCE,
            Some((2, HitPart::Body)),
        ),
        (
            "a link's arrowhead at the valve of the flow it points at is the arrowhead",
            link_end,
            TOLERANCE,
            Some((8, HitPart::Arrowhead)),
        ),
        (
            "the nearest drawing within the tolerance when nothing holds the point",
            Point::new(432.5, 100.0),
            12.0,
            Some((2, HitPart::Body)),
        ),
        (
            "nothing beyond the tolerance",
            Point::new(700.0, 700.0),
            TOLERANCE,
            None,
        ),
        (
            "a group's interior belongs to what it holds, here nothing",
            Point::new(250.0, 70.0),
            TOLERANCE,
            None,
        ),
        (
            "a group's outline is the group",
            Point::new(250.0, 45.0),
            TOLERANCE,
            Some((10, HitPart::Body)),
        ),
        (
            "a non-finite point hits nothing",
            Point::new(f64::NAN, 0.0),
            TOLERANCE,
            None,
        ),
    ];
    let mut failures = Vec::new();
    for (name, point, tolerance, want) in rows {
        let got = hit_test(&project, "main", point, tolerance).expect("main has a view");
        if got.map(|h| (h.uid, h.part)) != want {
            failures.push(format!("{name}: {got:?}, want {want:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_label_drawn_over_a_body_is_the_label() {
    // An aux just above a stock, its label hanging over the stock's top edge.
    let project = project_of(load(vec![
        labeled(stock(1, 100.0, 100.0), "bottom"),
        labeled(aux(2, 100.0, 70.0), "bottom"),
    ]));
    let ViewElement::Stock(s) = element(&project, 1) else {
        panic!("1 is a stock");
    };
    let ViewElement::Aux(a) = element(&project, 2) else {
        panic!("2 is an aux");
    };
    let body = &stock_geometry(s, false).rects[0];
    let label = label_bounds(&aux_geometry(a, false).label);
    let (left, right) = (label.left.max(body.x), label.right.min(body.x + body.width));
    let (top, bottom) = (
        label.top.max(body.y),
        label.bottom.min(body.y + body.height),
    );
    assert!(
        right - left > 2.0 * FIRM_INSET && bottom - top > 2.0 * FIRM_INSET,
        "the label must overlap the stock's body firmly"
    );
    let point = Point::new((left + right) / 2.0, (top + bottom) / 2.0);
    let got = hit_test(&project, "main", point, TOLERANCE).expect("main has a view");
    assert_eq!(
        got,
        Some(Hit {
            uid: 2,
            part: HitPart::Label
        })
    );
}

#[test]
fn a_missing_model_is_an_error() {
    assert!(hit_test(&scene(), "no such model", Point::new(0.0, 0.0), TOLERANCE).is_err());
}

fn shape_paint(shape: &SceneShape) -> ScenePaint {
    match shape {
        SceneShape::Rect(r) => r.paint,
        SceneShape::Circle(c) => c.paint,
        SceneShape::Path(p) => p.paint,
    }
}

/// One kind of element holding a coordinate that is not finite, beside a drawn
/// aux (uid 1) at (100, 100).
fn with_undrawn_coordinate(row: &str) -> Vec<json::ViewElement> {
    let nan = f64::NAN;
    let mut elements = vec![labeled(aux(1, 100.0, 100.0), "bottom")];
    elements.extend(match row {
        "group" => vec![json::ViewElement::Group(json::GroupViewElement {
            uid: 2,
            name: "g".to_string(),
            x: nan,
            y: 300.0,
            width: 120.0,
            height: 80.0,
            is_mdl_view_marker: false,
        })],
        "link" => vec![aux(3, nan, 300.0), link(2, 3, 1, None)],
        "flow valve" => vec![
            flow(2, (nan, 300.0), &[(250.0, 300.0, 3), (400.0, 300.0, 4)]),
            cloud(3, 2, 250.0, 300.0),
            cloud(4, 2, 400.0, 300.0),
        ],
        "flow source" => vec![
            flow(2, (325.0, 300.0), &[(nan, 300.0, 3), (400.0, 300.0, 4)]),
            cloud(3, 2, 250.0, 300.0),
            cloud(4, 2, 400.0, 300.0),
        ],
        "flow sink" => vec![
            flow(2, (325.0, 300.0), &[(250.0, 300.0, 3), (nan, 300.0, 4)]),
            cloud(3, 2, 250.0, 300.0),
            cloud(4, 2, 400.0, 300.0),
        ],
        "stock" => vec![stock(2, nan, 300.0)],
        "cloud" => vec![
            flow(3, (325.0, 300.0), &[(250.0, 300.0, 2), (400.0, 300.0, 4)]),
            cloud(2, 3, nan, 300.0),
            cloud(4, 3, 400.0, 300.0),
        ],
        "module" => vec![json::ViewElement::Module(json::ModuleViewElement {
            uid: 2,
            name: "m".to_string(),
            x: nan,
            y: 300.0,
            label_side: String::new(),
        })],
        "aux" => vec![aux(2, nan, 300.0)],
        "alias" => vec![alias(2, 1, nan, 300.0)],
        other => panic!("no row for {other}"),
    });
    elements
}

/// Every kind of element, a flow once per part a coordinate can break.
const UNDRAWN_ROWS: [&str; 10] = [
    "group",
    "link",
    "flow valve",
    "flow source",
    "flow sink",
    "stock",
    "cloud",
    "module",
    "aux",
    "alias",
];

#[test]
fn a_point_lands_only_on_what_the_scene_draws() {
    let mut failures = Vec::new();
    for row in UNDRAWN_ROWS {
        let project = project_of(load(with_undrawn_coordinate(row)));
        let scene = build_scene(&project, "main").expect("main has a view");
        let index = HitIndex::new(&project, "main").expect("main has a view");
        for i in 0..24 {
            for j in 0..16 {
                let point = Point::new(-50.0 + 25.0 * i as f64, -50.0 + 25.0 * j as f64);
                let Some(hit) = index.hit(point, TOLERANCE) else {
                    continue;
                };
                let at = format!(
                    "{row}: ({}, {}) hits {:?} of {}",
                    point.x, point.y, hit.part, hit.uid
                );
                let Some(drawn) = scene.elements.iter().find(|e| e.uid == hit.uid) else {
                    failures.push(format!("{at}, which the scene does not draw"));
                    continue;
                };
                // A hit is within the tolerance of what the scene draws of the
                // element, and the part it lands on is drawn: a label, or a
                // shape (the pipe a source end starts, the arrowhead a sink end
                // or a link's end is).
                let b = &drawn.bounds;
                let dx = (b.left - point.x).max(0.0).max(point.x - b.right);
                let dy = (b.top - point.y).max(0.0).max(point.y - b.bottom);
                if dx.hypot(dy) > TOLERANCE {
                    failures.push(format!("{at}, beyond what the scene draws of it"));
                }
                let paints: Vec<ScenePaint> = drawn.shapes.iter().map(shape_paint).collect();
                let part_drawn = match hit.part {
                    HitPart::Label => drawn.label.is_some(),
                    HitPart::Source => paints.contains(&ScenePaint::FlowPipeOuter),
                    HitPart::Arrowhead => paints.iter().any(|p| {
                        matches!(p, ScenePaint::ArrowheadFlow | ScenePaint::ArrowheadLink)
                    }),
                    HitPart::Body => !paints.is_empty(),
                };
                if !part_drawn {
                    failures.push(format!("{at}, a part the scene does not draw"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
