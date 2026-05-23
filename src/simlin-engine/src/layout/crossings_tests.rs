// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the polyline-based `count_view_crossings` / `build_view_segments`
//! (Phase 1, Task 4 of the layout quality eval). Kept in their own file so the
//! `layout_tests.rs` integration suite stays under the per-file line cap.

use super::*;

fn cv_aux(uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Aux(view_element::Aux {
        name: format!("a{uid}"),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

fn cv_module(uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Module(view_element::Module {
        name: format!("m{uid}"),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
    })
}

fn cv_link(uid: i32, from_uid: i32, to_uid: i32, shape: LinkShape) -> ViewElement {
    ViewElement::Link(view_element::Link {
        uid,
        from_uid,
        to_uid,
        shape,
        polarity: None,
    })
}

fn cv_view(elements: Vec<ViewElement>) -> datamodel::StockFlow {
    datamodel::StockFlow {
        name: None,
        elements,
        view_box: Rect {
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

/// AC2.1: two straight links that cross once yield a crossing count of 1.
#[test]
fn test_count_view_crossings_two_straight_links_cross_once() {
    // Link 1: a1(0,0) -> a2(100,100). Link 2: a3(0,100) -> a4(100,0).
    // The two diagonals of a square cross exactly once at the center.
    let view = cv_view(vec![
        cv_aux(1, 0.0, 0.0),
        cv_aux(2, 100.0, 100.0),
        cv_aux(3, 0.0, 100.0),
        cv_aux(4, 100.0, 0.0),
        cv_link(10, 1, 2, LinkShape::Straight),
        cv_link(11, 3, 4, LinkShape::Straight),
    ]);

    assert_eq!(count_view_crossings(&view), 1);
}

/// AC2.1: two links sharing an endpoint element yield 0 crossings.
#[test]
fn test_count_view_crossings_shared_endpoint_no_crossing() {
    // Both links start at a1; sharing the `elem_1` vertex suppresses any
    // intersection at the shared endpoint.
    let view = cv_view(vec![
        cv_aux(1, 50.0, 50.0),
        cv_aux(2, 100.0, 0.0),
        cv_aux(3, 100.0, 100.0),
        cv_link(10, 1, 2, LinkShape::Straight),
        cv_link(11, 1, 3, LinkShape::Straight),
    ]);

    assert_eq!(count_view_crossings(&view), 0);
}

/// AC2.2: an Arc connector that visually crosses another edge is counted via
/// polyline sampling, on a case where the straight-chord approximation does
/// not count it. The arc from a1(0,0) to a2(200,0) bulges down to a peak near
/// (100, 57.7); a horizontal straight link c-d at y=50 (from x=40 to x=160)
/// passes through the bulge, crossing the curve twice (near x=58 and x=142),
/// while the arc's straight chord (the line y=0) stays well clear of it. So the
/// old chord-based count is 0 and the new polyline-based count is >= 1.
#[test]
fn test_count_view_crossings_arc_curve_crosses_when_chord_does_not() {
    let view = cv_view(vec![
        cv_aux(1, 0.0, 0.0),
        cv_aux(2, 200.0, 0.0),
        cv_aux(3, 40.0, 50.0),
        cv_aux(4, 160.0, 50.0),
        // Wide arc: large take-off angle so the curve bulges well below the
        // straight chord between the two endpoints.
        cv_link(10, 1, 2, LinkShape::Arc(60.0)),
        cv_link(11, 3, 4, LinkShape::Straight),
    ]);

    // The straight-chord approximation (centers, ignoring shape) does NOT
    // count this crossing: build those chord segments inline and confirm 0.
    let p1 = Position::new(0.0, 0.0);
    let p2 = Position::new(200.0, 0.0);
    let p3 = Position::new(40.0, 50.0);
    let p4 = Position::new(160.0, 50.0);
    let chord_segments = vec![
        LineSegment {
            start: p1,
            end: p2,
            from_node: "elem_1".to_string(),
            to_node: "elem_2".to_string(),
        },
        LineSegment {
            start: p3,
            end: p4,
            from_node: "elem_3".to_string(),
            to_node: "elem_4".to_string(),
        },
    ];
    assert_eq!(
        annealing::count_crossings(&chord_segments),
        0,
        "chord approximation must not see this crossing"
    );

    // The polyline (sampled arc) DOES count it.
    assert!(
        count_view_crossings(&view) >= 1,
        "sampled arc curve must cross the straight link"
    );
}

/// AC2.3: the crossing count is invariant under translation and rotation of
/// the whole view.
#[test]
fn test_count_view_crossings_translation_rotation_invariant() {
    let base = vec![
        cv_aux(1, 0.0, 0.0),
        cv_aux(2, 100.0, 100.0),
        cv_aux(3, 0.0, 100.0),
        cv_aux(4, 100.0, 0.0),
        cv_link(10, 1, 2, LinkShape::Arc(25.0)),
        cv_link(11, 3, 4, LinkShape::Straight),
    ];
    let base_count = count_view_crossings(&cv_view(base.clone()));

    // Translate every coordinate by a fixed offset.
    let translated: Vec<ViewElement> = base
        .iter()
        .map(|e| transform_element(e, |x, y| (x + 137.0, y - 89.0)))
        .collect();
    assert_eq!(
        count_view_crossings(&cv_view(translated)),
        base_count,
        "translation must preserve crossing count"
    );

    // Rotate every coordinate about the origin by a fixed angle.
    let theta = 0.7_f64; // radians
    let (s, c) = theta.sin_cos();
    let rotated: Vec<ViewElement> = base
        .iter()
        .map(|e| transform_element(e, |x, y| (x * c - y * s, x * s + y * c)))
        .collect();
    assert_eq!(
        count_view_crossings(&cv_view(rotated)),
        base_count,
        "rotation must preserve crossing count"
    );
}

/// Apply a coordinate transform to the (x, y) of a positioned view element.
/// Links carry no coordinates of their own and pass through unchanged.
fn transform_element(e: &ViewElement, f: impl Fn(f64, f64) -> (f64, f64)) -> ViewElement {
    match e {
        ViewElement::Aux(a) => {
            let (x, y) = f(a.x, a.y);
            ViewElement::Aux(view_element::Aux { x, y, ..a.clone() })
        }
        ViewElement::Module(m) => {
            let (x, y) = f(m.x, m.y);
            ViewElement::Module(view_element::Module { x, y, ..m.clone() })
        }
        other => other.clone(),
    }
}

/// Module/Alias undercount fix: a link from an Aux to a Module that crosses
/// another link is now counted. Previously Module-incident links were dropped
/// from the segment set entirely, so this crossing was invisible.
#[test]
fn test_count_view_crossings_module_incident_link_participates() {
    // Link 1: a1(0,0) -> m2(100,100) (a Module endpoint).
    // Link 2: a3(0,100) -> a4(100,0). The two diagonals cross once.
    let view = cv_view(vec![
        cv_aux(1, 0.0, 0.0),
        cv_module(2, 100.0, 100.0),
        cv_aux(3, 0.0, 100.0),
        cv_aux(4, 100.0, 0.0),
        cv_link(10, 1, 2, LinkShape::Straight),
        cv_link(11, 3, 4, LinkShape::Straight),
    ]);

    assert_eq!(
        count_view_crossings(&view),
        1,
        "a Module-incident link must participate in crossing detection"
    );
}
