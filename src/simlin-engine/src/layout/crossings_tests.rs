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

fn cv_stock(uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Stock(view_element::Stock {
        name: format!("s{uid}"),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

fn cv_cloud(uid: i32, flow_uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Cloud(view_element::Cloud {
        uid,
        flow_uid,
        x,
        y,
        compat: None,
    })
}

/// A horizontal flow whose valve sits at (`x`, `y`), with its source end
/// attached to `from_uid` (a cloud or stock to the left) and its sink end
/// attached to `to_uid` (a stock to the right). The valve lies on the pipe,
/// mid-span between the two attached endpoints.
fn cv_flow(uid: i32, x: f64, y: f64, from_uid: i32, to_uid: i32) -> ViewElement {
    cv_flow_pts(
        uid,
        x,
        y,
        (x - 60.0, y, Some(from_uid)),
        (x + 60.0, y, Some(to_uid)),
    )
}

/// A two-point flow with the valve at (`x`, `y`) and explicitly positioned
/// source/sink points, each carrying an optional `attached_to_uid`. Lets a
/// test reproduce a real reference geometry where the valve does not sit at the
/// midpoint of the two points.
fn cv_flow_pts(
    uid: i32,
    x: f64,
    y: f64,
    from: (f64, f64, Option<i32>),
    to: (f64, f64, Option<i32>),
) -> ViewElement {
    ViewElement::Flow(view_element::Flow {
        name: format!("f{uid}"),
        uid,
        x,
        y,
        label_side: LabelSide::Top,
        points: vec![
            view_element::FlowPoint {
                x: from.0,
                y: from.1,
                attached_to_uid: from.2,
            },
            view_element::FlowPoint {
                x: to.0,
                y: to.1,
                attached_to_uid: to.2,
            },
        ],
        compat: None,
        label_compat: None,
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

/// A link that TERMINATES at a flow's valve must not be counted as crossing the
/// flow pipe at that shared connection point. This is the exact
/// dp_logistic_growth reference geometry: the horizontal `net birth rate` flow
/// (cloud -> valve -> Population stock) plus the `fractional growth rate ->
/// net birth rate` link, whose drawn arc curves up to the valve from below and
/// grazes the pipe at the connection point. The link's endpoint (`elem_2`, the
/// flow's own element uid) and the pipe share the flow's element at the valve,
/// so that graze is not a real crossing.
#[test]
fn test_count_view_crossings_link_to_flow_valve_no_crossing() {
    let flow_uid = 2;
    let view = cv_view(vec![
        cv_stock(1, 602.4000244140625, 259.8000183105469),
        cv_flow_pts(
            flow_uid,
            518.2726610523725,
            258.60003662109375,
            // source end attached to the cloud, sink end to the stock
            (456.79998779296875, 258.60003662109375, Some(3)),
            (579.9000244140625, 258.60003662109375, Some(1)),
        ),
        cv_cloud(3, flow_uid, 456.79998779296875, 258.60003662109375),
        cv_aux(4, 498.0, 344.20001220703125),
        // fractional growth rate -> net birth rate (to_uid == flow.uid): the
        // drawn arc bulges up to graze the pipe at the valve connection point.
        cv_link(10, 4, flow_uid, LinkShape::Arc(118.82198603295677)),
    ]);

    assert_eq!(
        count_view_crossings(&view),
        0,
        "a link terminating at a flow valve must not count as crossing the pipe"
    );
}

/// The flow-segment naming contract that the suppression relies on: a flow
/// point attached to a stock/cloud names its pipe vertex `elem_{attached_uid}`
/// (so a link incident on that stock/cloud, which uses the same name, is
/// suppressed at the shared connection point), the valve is injected as an
/// `elem_{flow.uid}` vertex on the pipe (so a link incident on the valve is
/// suppressed there), and a free point keeps the per-flow `flow_{uid}#{i}`
/// name (so a genuine mid-span crossing is still counted). This is the
/// node-name contract; the end-to-end suppression is exercised by the valve and
/// mid-span tests, since for an attached stock/cloud the link endpoint clips to
/// the element boundary and only grazes the pipe through the shared vertex.
#[test]
fn test_build_view_segments_flow_vertex_naming() {
    let flow_uid = 2;
    let stock_uid = 1;
    let cloud_uid = 3;
    let view = cv_view(vec![
        cv_stock(stock_uid, 602.4000244140625, 259.8000183105469),
        cv_flow_pts(
            flow_uid,
            518.2726610523725,
            258.60003662109375,
            (456.79998779296875, 258.60003662109375, Some(cloud_uid)),
            (579.9000244140625, 258.60003662109375, Some(stock_uid)),
        ),
        cv_cloud(cloud_uid, flow_uid, 456.79998779296875, 258.60003662109375),
    ]);

    let segs = build_view_segments(&view);
    // The pipe splits at the valve into two sub-segments:
    //   elem_3 (cloud) -> elem_2 (valve)  and  elem_2 (valve) -> elem_1 (stock)
    let names: Vec<(String, String)> = segs
        .iter()
        .map(|s| (s.from_node.clone(), s.to_node.clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("elem_3".to_string(), "elem_2".to_string()),
            ("elem_2".to_string(), "elem_1".to_string()),
        ],
        "flow pipe must name attached endpoints elem_<attached> and split at the valve as elem_<flow>"
    );

    // A free (unattached) interior point keeps the per-flow name.
    let free_view = cv_view(vec![cv_flow_pts(
        flow_uid,
        518.2726610523725,
        258.60003662109375,
        (456.79998779296875, 258.60003662109375, None),
        (579.9000244140625, 258.60003662109375, None),
    )]);
    let free_segs = build_view_segments(&free_view);
    let free_names: Vec<(String, String)> = free_segs
        .iter()
        .map(|s| (s.from_node.clone(), s.to_node.clone()))
        .collect();
    assert_eq!(
        free_names,
        vec![
            (format!("flow_{flow_uid}#0"), format!("elem_{flow_uid}")),
            (format!("elem_{flow_uid}"), format!("flow_{flow_uid}#1")),
        ],
        "an unattached flow point keeps its per-flow name; only the valve is elem_<flow>"
    );
}

/// A GENUINE mid-span crossing of a flow pipe -- a link that crosses the pipe
/// away from any element the flow shares -- must STILL be counted. This guards
/// against the valve/attachment suppression over-suppressing real crossings.
#[test]
fn test_count_view_crossings_link_crosses_flow_pipe_midspan_counted() {
    // Flow valve at (100, 100), pipe from x=40 to x=160 at y=100. A straight
    // link runs vertically through x=70 (between the cloud end and the valve,
    // so it does NOT touch the valve, the cloud, or the stock), crossing the
    // pipe once.
    let flow_uid = 20;
    let view = cv_view(vec![
        cv_cloud(1, flow_uid, 40.0, 100.0),
        cv_stock(2, 200.0, 100.0),
        cv_aux(3, 70.0, 50.0),
        cv_aux(4, 70.0, 150.0),
        cv_flow(flow_uid, 100.0, 100.0, 1, 2),
        // Link from a3 (above the pipe) to a4 (below the pipe), crossing the
        // pipe at x=70 -- nowhere near the valve or either attached element.
        cv_link(30, 3, 4, LinkShape::Straight),
    ]);

    assert_eq!(
        count_view_crossings(&view),
        1,
        "a genuine mid-span crossing of the flow pipe must still be counted"
    );
}
