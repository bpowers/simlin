// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Every arm of `normalize_flow_geometry`: `EndFix::{Keep, Face, Leg, Jog}` on
//! both axes and both ends, the line constraints that make a segment
//! unsolvable, the slide's interior-neighbour guard, straightening, valve
//! projection, and cloud recentering.
//!
//! These build datamodel views by hand. That is the pass's contract: it takes
//! any datamodel view, whatever produced it. What the importers actually hand
//! it is pinned separately, through the production readers on corpus files
//! (`xmile::views::flow_geometry_tests`).

use super::*;
use crate::datamodel::view_element::{Cloud, LabelSide, Stock};

fn stock(uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Stock(Stock {
        name: format!("s{uid}"),
        uid,
        x,
        y,
        label_side: LabelSide::Top,
        compat: None,
    })
}

fn cloud(uid: i32, flow_uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Cloud(Cloud {
        uid,
        flow_uid,
        x,
        y,
        compat: None,
    })
}

fn pt(x: f64, y: f64, attached: Option<i32>) -> FlowPoint {
    FlowPoint {
        x,
        y,
        attached_to_uid: attached,
    }
}

fn flow(uid: i32, valve: (f64, f64), points: Vec<FlowPoint>) -> ViewElement {
    ViewElement::Flow(Flow {
        name: format!("f{uid}"),
        uid,
        x: valve.0,
        y: valve.1,
        label_side: LabelSide::Bottom,
        points,
        compat: None,
        label_compat: None,
    })
}

fn the_flow(elements: &[ViewElement]) -> &Flow {
    elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) => Some(f),
            _ => None,
        })
        .unwrap()
}

fn coords(f: &Flow) -> Vec<(f64, f64)> {
    f.points.iter().map(|p| (p.x, p.y)).collect()
}

fn normalized(mut elements: Vec<ViewElement>) -> Vec<ViewElement> {
    normalize_flow_geometry(&mut elements);
    assert_eq!(
        flow_invariant_violations(&elements),
        Vec::<String>::new(),
        "normalization must establish the invariants"
    );
    elements
}

#[test]
fn keep_leaves_valid_endpoints_on_every_face_untouched() {
    // Off-center slots on all four faces of a stock at (100, 100).
    let cases = [
        vec![pt(122.5, 90.0, Some(1)), pt(200.0, 90.0, Some(3))],
        vec![pt(20.0, 108.0, Some(3)), pt(77.5, 108.0, Some(1))],
        vec![pt(88.0, 82.5, Some(1)), pt(88.0, 20.0, Some(3))],
        vec![pt(110.0, 200.0, Some(3)), pt(110.0, 117.5, Some(1))],
    ];
    for points in cases {
        let (a, b) = (&points[0], &points[1]);
        let valve = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let cloud_end = if a.attached_to_uid == Some(3) { a } else { b };
        let elements = normalized(vec![
            stock(1, 100.0, 100.0),
            cloud(3, 2, cloud_end.x, cloud_end.y),
            flow(2, valve, points.clone()),
        ]);
        let f = the_flow(&elements);
        assert_eq!(
            coords(f),
            points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()
        );
        assert_eq!((f.x, f.y), valve);
    }
}

#[test]
fn face_snaps_an_endpoint_inside_the_clearance_span_along_its_line() {
    // Horizontal: an endpoint at the stock's center column (a producer that
    // anchors endpoints to centers) moves out to the face the pipe approaches.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 250.0, 108.0),
        flow(
            2,
            (180.0, 108.0),
            vec![pt(100.0, 108.0, Some(1)), pt(250.0, 108.0, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(122.5, 108.0), (250.0, 108.0)]
    );

    // Vertical, pipe leaving upward from beyond the face.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 95.0, 10.0),
        flow(
            2,
            (95.0, 50.0),
            vec![pt(95.0, 10.0, Some(3)), pt(95.0, 60.0, Some(1))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(95.0, 10.0), (95.0, 82.5)]
    );
}

#[test]
fn face_slides_a_line_in_the_corner_zone_and_carries_valve_and_cloud() {
    // Line 16 below center: past the 14.5 clearance span but within
    // MIN_SEGMENT_LENGTH of the 17.5 face line, so the pipe enters the side
    // face: it slides up by 1.5, and the valve and the cloud move with it.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 250.0, 116.0),
        flow(
            2,
            (180.0, 116.0),
            vec![pt(122.5, 116.0, Some(1)), pt(250.0, 116.0, Some(3))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(coords(f), vec![(122.5, 114.5), (250.0, 114.5)]);
    assert_eq!((f.x, f.y), (180.0, 114.5));

    // The vertical axis: a pipe on the bottom face 22 right of center (0.5
    // from the corner) slides left to 19.5.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 122.0, 250.0),
        flow(
            2,
            (122.0, 200.0),
            vec![pt(122.0, 117.5, Some(1)), pt(122.0, 250.0, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(119.5, 117.5), (119.5, 250.0)]
    );
}

#[test]
fn leg_keeps_a_far_line_and_adds_a_perpendicular_leg_into_the_passed_face() {
    // Horizontal line 55 above the center, endpoint anchored at the center
    // column: the endpoint becomes a bend and a leg drops into the top face.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 300.0, 45.0),
        flow(
            2,
            (200.0, 45.0),
            vec![pt(100.0, 45.0, Some(1)), pt(300.0, 45.0, Some(3))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(coords(f), vec![(100.0, 82.5), (100.0, 45.0), (300.0, 45.0)]);
    assert_eq!(f.points[0].attached_to_uid, Some(1));
    assert_eq!(f.points[1].attached_to_uid, None);

    // Vertical line far left of the stock (a sketch whose pipe corner sits
    // off to the side): the bend row is clamped into the face span and a
    // horizontal leg runs into the left face.
    let elements = normalized(vec![
        stock(1, 600.0, 400.0),
        cloud(3, 2, 30.0, 100.0),
        flow(
            2,
            (30.0, 200.0),
            vec![pt(30.0, 100.0, Some(3)), pt(30.0, 440.0, Some(1))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(30.0, 100.0), (30.0, 414.5), (577.5, 414.5)]
    );
}

#[test]
fn leg_at_both_ends_routes_over_two_stocks() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 100.0),
        flow(
            2,
            (250.0, 45.0),
            vec![pt(100.0, 45.0, Some(1)), pt(400.0, 45.0, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(100.0, 82.5), (100.0, 45.0), (400.0, 45.0), (400.0, 82.5)]
    );
}

#[test]
fn the_face_rule_takes_the_line_within_min_segment_length_of_the_face() {
    // 20 below center: less than 17.5 + MIN_SEGMENT_LENGTH, so Face (slide to
    // 14.5), not a 2.5px leg.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 250.0, 120.0),
        flow(
            2,
            (180.0, 120.0),
            vec![pt(100.0, 120.0, Some(1)), pt(250.0, 120.0, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(122.5, 114.5), (250.0, 114.5)]
    );

    // 20.5 below center: exactly 17.5 + MIN_SEGMENT_LENGTH, so a 3px leg.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 250.0, 120.5),
        flow(
            2,
            (180.0, 120.5),
            vec![pt(100.0, 120.5, Some(1)), pt(250.0, 120.5, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(100.0, 117.5), (100.0, 120.5), (250.0, 120.5)]
    );
}

#[test]
fn two_stock_ends_share_one_line() {
    // Both ends Face (each anchored at its stock's center column) with
    // overlapping spans, [85.5, 114.5] and [95.5, 124.5]: the line is clamped
    // into both.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 110.0),
        flow(
            2,
            (250.0, 118.0),
            vec![pt(100.0, 118.0, Some(1)), pt(400.0, 118.0, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(122.5, 114.5), (377.5, 114.5)]
    );
}

/// A valid end pins the line; a Face end whose span excludes that line jogs
/// to its own line just short of its face, rather than moving the modeler's
/// valid slot. The step sits between the face and the valve.
#[test]
fn a_face_end_jogs_to_its_own_line_when_a_valid_slot_pins_the_other() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 128.0),
        flow(
            2,
            (250.0, 110.0),
            vec![pt(122.5, 110.0, Some(1)), pt(377.5, 110.0, Some(3))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(
        coords(f),
        vec![
            (122.5, 110.0),
            (367.5, 110.0),
            (367.5, 113.5),
            (377.5, 113.5)
        ]
    );
    assert_eq!((f.x, f.y), (250.0, 110.0));

    // Two Face ends with disjoint spans ([85.5, 114.5] and [120.5, 149.5]):
    // the ends are tried source first, so the line takes the sink's span
    // (116 -> 120.5, valve carried along) and the source jogs.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 135.0),
        flow(
            2,
            (250.0, 116.0),
            vec![pt(100.0, 116.0, Some(1)), pt(400.0, 116.0, Some(3))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(
        coords(f),
        vec![
            (122.5, 114.5),
            (132.5, 114.5),
            (132.5, 120.5),
            (377.5, 120.5)
        ]
    );
    assert_eq!((f.x, f.y), (250.0, 120.5));
}

/// A valid end pins the line; a Face end whose own line is within
/// MIN_SEGMENT_LENGTH of it cannot jog either (the step would be degenerate),
/// so the segment is left as it was rather than moving the valid slot.
#[test]
fn a_pinned_line_the_other_end_cannot_reach_is_left_unchanged() {
    let original = vec![pt(122.5, 110.0, Some(1)), pt(377.5, 110.0, Some(3))];
    let mut elements = vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 127.0),
        flow(2, (250.0, 110.0), original.clone()),
    ];
    normalize_flow_geometry(&mut elements);
    assert_eq!(
        coords(the_flow(&elements)),
        original.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()
    );
}

#[test]
fn a_slide_moves_an_interior_neighbour_only_along_its_perpendicular_segment() {
    // Source on the bottom face 0.5 from the corner: the first segment slides
    // 2.5 left into the clearance span; its neighbour's horizontal segment
    // lengthens and the rest of the pipe is untouched.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 300.0, 160.0),
        flow(
            2,
            (200.0, 160.0),
            vec![
                pt(122.0, 117.5, Some(1)),
                pt(122.0, 160.0, None),
                pt(300.0, 160.0, Some(3)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(119.5, 117.5), (119.5, 160.0), (300.0, 160.0)]
    );

    // A collinear neighbour would be pulled off its axis by the slide, so
    // the segment stays as it was.
    let original = vec![
        pt(122.0, 117.5, Some(1)),
        pt(122.0, 160.0, None),
        pt(122.0, 200.0, None),
        pt(300.0, 200.0, Some(3)),
    ];
    let mut elements = vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 300.0, 200.0),
        flow(2, (200.0, 200.0), original.clone()),
    ];
    normalize_flow_geometry(&mut elements);
    assert_eq!(
        coords(the_flow(&elements)),
        original.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()
    );
}

#[test]
fn straightening_averages_the_dominant_axis_and_carries_the_valve() {
    let elements = normalized(vec![
        cloud(3, 2, 10.0, 100.0),
        cloud(4, 2, 110.0, 106.0),
        flow(
            2,
            (60.0, 101.0),
            vec![pt(10.0, 100.0, Some(3)), pt(110.0, 106.0, Some(4))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(coords(f), vec![(10.0, 103.0), (110.0, 103.0)]);
    assert_eq!((f.x, f.y), (60.0, 103.0));

    let elements = normalized(vec![
        cloud(3, 2, 50.0, 10.0),
        cloud(4, 2, 54.0, 110.0),
        flow(
            2,
            (51.0, 60.0),
            vec![pt(50.0, 10.0, Some(3)), pt(54.0, 110.0, Some(4))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(coords(f), vec![(52.0, 10.0), (52.0, 110.0)]);
    assert_eq!((f.x, f.y), (52.0, 60.0));
}

#[test]
fn an_off_pipe_valve_is_projected_with_a_margin() {
    // Off the pipe and beyond its end: projected onto the segment and kept
    // VALVE_MARGIN from the end.
    let elements = normalized(vec![
        cloud(3, 2, 0.0, 100.0),
        cloud(4, 2, 100.0, 100.0),
        flow(
            2,
            (130.0, 80.0),
            vec![pt(0.0, 100.0, Some(3)), pt(100.0, 100.0, Some(4))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!((f.x, f.y), (90.0, 100.0));

    // Already on the pipe, even close to an end: not moved.
    let elements = normalized(vec![
        cloud(3, 2, 0.0, 100.0),
        cloud(4, 2, 100.0, 100.0),
        flow(
            2,
            (3.0, 100.0),
            vec![pt(0.0, 100.0, Some(3)), pt(100.0, 100.0, Some(4))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!((f.x, f.y), (3.0, 100.0));
}

#[test]
fn clouds_move_to_their_endpoints_and_nothing_else_moves_them() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 240.0, 93.0),
        flow(
            2,
            (180.0, 100.0),
            vec![pt(122.5, 100.0, Some(1)), pt(250.0, 100.0, Some(3))],
        ),
    ]);
    let c = elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Cloud(c) => Some(c),
            _ => None,
        })
        .unwrap();
    assert_eq!((c.x, c.y), (250.0, 100.0));
}

#[test]
fn clamp_to_face_span_keeps_corner_clearance() {
    assert_eq!(clamp_to_face_span(100.0, 100.0, 22.5), 100.0);
    assert_eq!(clamp_to_face_span(130.0, 100.0, 22.5), 119.5);
    assert_eq!(clamp_to_face_span(70.0, 100.0, 22.5), 80.5);
    assert_eq!(clamp_to_face_span(117.5, 100.0, 17.5), 114.5);
}
