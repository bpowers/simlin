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

use crate::datamodel::ViewElement;
use crate::editing::test_support::{
    aux, cloud, flow, labeled, link, load, project_of, stock, view_of,
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
