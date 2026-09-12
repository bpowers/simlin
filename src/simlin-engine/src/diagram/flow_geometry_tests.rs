// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Every arm of `normalize_flow_geometry`: `EndFix::{Keep, Face, Leg, Jog}` on
//! both axes and both ends, the line constraints that make a segment
//! unsolvable, a slide's interior neighbours, `simplify` and the simplified
//! pipe each pass starts from, the collapse of a segment under the minima
//! (with room, and while crowded ends are invalid), the rule that no step adds
//! a segment through a terminal stock's body, straightening and its undo,
//! valve projection by arc length, cloud recentering, idempotence and identity
//! on a view the checker accepts; and the checker's segment minima and valve
//! margin.
//!
//! These build datamodel views by hand. That is the pass's contract: it takes
//! any datamodel view, whatever produced it. What the importers actually hand
//! it is pinned separately, through the production readers on corpus files
//! (`xmile::views::flow_geometry_tests`, `mdl::view::convert::flow_geometry_tests`).

use super::*;
use crate::datamodel::view_element::{Cloud, Flow, LabelSide, Stock};

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
    // Line 16 below center: past the 14.5 clearance span but within a stub
    // (MIN_SEGMENT) of the 17.5 face line, so the pipe enters the side face:
    // it slides up by 1.5, and the valve and the cloud move with it.
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
fn the_face_rule_takes_the_line_within_a_stub_of_the_face() {
    // 27 below center: less than 17.5 + MIN_SEGMENT, so Face (slide to 14.5),
    // not a leg shorter than a stub.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 250.0, 127.0),
        flow(
            2,
            (180.0, 127.0),
            vec![pt(100.0, 127.0, Some(1)), pt(250.0, 127.0, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(122.5, 114.5), (250.0, 114.5)]
    );

    // 27.5 below center: exactly 17.5 + MIN_SEGMENT, so a stub-long leg.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 250.0, 127.5),
        flow(
            2,
            (180.0, 127.5),
            vec![pt(100.0, 127.5, Some(1)), pt(250.0, 127.5, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(100.0, 117.5), (100.0, 127.5), (250.0, 127.5)]
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
/// to its own line, stepping the end's minimum segment short of its face (a
/// sink's for the sink, a stub's for the source), rather than moving the
/// modeler's valid slot. The riser is at least `MIN_SEGMENT`.
#[test]
fn a_face_end_jogs_to_its_own_line_when_a_valid_slot_pins_the_other() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 140.0),
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
            (362.0, 110.0),
            (362.0, 125.5),
            (377.5, 125.5)
        ]
    );
    assert_eq!((f.x, f.y), (250.0, 110.0));

    // Two Face ends with disjoint spans ([85.5, 114.5] and [130.5, 159.5]):
    // the ends are tried source first, so the line takes the sink's span
    // (116 -> 130.5, valve carried along) and the source jogs a stub from its
    // face.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 145.0),
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
            (132.5, 130.5),
            (377.5, 130.5)
        ]
    );
    assert_eq!((f.x, f.y), (250.0, 130.5));
}

/// A valid end pins the line, and the Face end's own line is within a riser
/// (MIN_SEGMENT) of it, so no jog is possible. As the last resort the
/// valid slot slides within its own clearance span by the least amount that
/// lets the other end in, and stays valid.
#[test]
fn a_valid_slot_gives_up_the_least_it_can_when_no_jog_fits() {
    // Source valid 4.5 from its right corner (dx = 15); the sink sits 2.5
    // from a corner and needs x >= 305.5 on a segment pinned at 305.
    let elements = normalized(vec![
        stock(1, 320.0, 945.0),
        stock(3, 325.0, 595.0),
        flow(
            2,
            (305.0, 755.0),
            vec![pt(305.0, 927.5, Some(1)), pt(305.0, 612.5, Some(3))],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(coords(f), vec![(305.5, 927.5), (305.5, 612.5)]);
    assert_eq!((f.x, f.y), (305.5, 755.0));
}

/// Two Face ends whose clearance spans are disjoint by less than
/// a riser, MIN_SEGMENT ([85.5, 114.5] and [116, 145]): no shared line, no jog
/// long enough, and no valid slot to relax, so nothing moves.
#[test]
fn an_unsolvable_segment_is_left_unchanged() {
    let original = vec![pt(100.0, 115.0, Some(1)), pt(400.0, 115.0, Some(3))];
    let mut elements = vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 130.5),
        flow(2, (250.0, 115.0), original.clone()),
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

    // A collinear interior point is dropped before the slide (a pass starts
    // from the simplified pipe), so the neighbour the slide moves is the
    // merged segment's far point and nothing is pulled off its axis.
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 300.0, 200.0),
        flow(
            2,
            (200.0, 200.0),
            vec![
                pt(122.0, 117.5, Some(1)),
                pt(122.0, 160.0, None),
                pt(122.0, 200.0, None),
                pt(300.0, 200.0, Some(3)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(119.5, 117.5), (119.5, 200.0), (300.0, 200.0)]
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

/// Rows: off the pipe beyond an end; on the pipe within the margin of an end;
/// on the pipe outside the margin; across a bend (the margin is arc length
/// from the path's ends, not from a segment's); a path too short for the
/// margin.
#[test]
fn an_off_pipe_valve_is_projected_with_a_margin() {
    let straight = vec![pt(0.0, 100.0, Some(3)), pt(100.0, 100.0, Some(4))];
    let bent = vec![
        pt(0.0, 0.0, Some(3)),
        pt(0.0, 100.0, None),
        pt(30.0, 100.0, Some(4)),
    ];
    let short = vec![pt(0.0, 100.0, Some(3)), pt(15.0, 100.0, Some(4))];
    /// (label, path, valve before, valve after).
    type Row<'a> = (&'a str, &'a [FlowPoint], (f64, f64), (f64, f64));
    let rows: [Row; 6] = [
        (
            "off the pipe beyond an end",
            &straight,
            (130.0, 80.0),
            (90.0, 100.0),
        ),
        (
            "on the pipe within the margin",
            &straight,
            (3.0, 100.0),
            (10.0, 100.0),
        ),
        (
            "on the pipe outside the margin",
            &straight,
            (30.0, 100.0),
            (30.0, 100.0),
        ),
        (
            "near a bend, far from the ends",
            &bent,
            (0.0, 97.0),
            (0.0, 97.0),
        ),
        (
            "near the far end of a bent path",
            &bent,
            (27.0, 100.0),
            (20.0, 100.0),
        ),
        (
            "a path shorter than two margins",
            &short,
            (2.0, 100.0),
            (2.0, 100.0),
        ),
    ];
    for (label, points, valve, expected) in rows {
        let mut v = valve;
        project_valve_onto_pipe(points, &mut v);
        assert_eq!(v, expected, "{label}");
    }
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

/// Rows, one per arm of `collapse_short_segment`: a short stub at a cloud end
/// (the end moves to the bend); a short stub at a stock end (the end moves to
/// the bend and the Face arm brings it onto the face); a short riser whose
/// later run can move onto the earlier run's line; the same riser with the
/// pipe reversed, where moving the later run would move a valid stock slot and
/// the earlier (free) run moves instead.
#[test]
fn a_short_segment_collapses_where_the_pipe_can_give_it_up() {
    let elements = normalized(vec![
        stock(1, 300.0, 116.0),
        cloud(3, 2, 0.0, 100.0),
        flow(
            2,
            (150.0, 102.0),
            vec![
                pt(0.0, 100.0, Some(3)),
                pt(0.0, 102.0, None),
                pt(277.5, 102.0, Some(1)),
            ],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(coords(f), vec![(0.0, 102.0), (277.5, 102.0)], "cloud stub");
    assert_eq!((f.x, f.y), (150.0, 102.0), "cloud stub");

    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 300.0, 120.0),
        flow(
            2,
            (200.0, 120.0),
            vec![
                pt(100.0, 117.5, Some(1)),
                pt(100.0, 120.0, None),
                pt(300.0, 120.0, Some(3)),
            ],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(
        coords(f),
        vec![(122.5, 114.5), (300.0, 114.5)],
        "stock stub"
    );
    assert_eq!((f.x, f.y), (200.0, 114.5), "stock stub");

    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 400.0, 100.0),
        flow(
            2,
            (250.0, 101.0),
            vec![
                pt(122.5, 100.0, Some(1)),
                pt(250.0, 100.0, None),
                pt(250.0, 102.0, None),
                pt(377.5, 102.0, Some(3)),
            ],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(
        coords(f),
        vec![(122.5, 100.0), (377.5, 100.0)],
        "riser, later run moves"
    );
    assert_eq!((f.x, f.y), (250.0, 100.0), "riser, later run moves");

    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(3, 2, 400.0, 102.0),
        flow(
            2,
            (300.0, 102.0),
            vec![
                pt(400.0, 102.0, Some(3)),
                pt(250.0, 102.0, None),
                pt(250.0, 100.0, None),
                pt(122.5, 100.0, Some(1)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(400.0, 100.0), (122.5, 100.0)],
        "riser, earlier run moves to keep the valid slot"
    );
}

/// Stocks at (100, 100) and (300, 130): the straightened line (115) is outside
/// both clearance spans by 0.5, no jog is a riser long, and no valid slot can
/// give way, so the attach step cannot bring the ends on. The straightening is
/// not committed either.
#[test]
fn a_straightening_the_attach_step_rejects_is_undone() {
    let original = vec![pt(122.5, 108.0, Some(1)), pt(277.5, 122.0, Some(3))];
    let mut elements = vec![
        stock(1, 100.0, 100.0),
        stock(3, 300.0, 130.0),
        flow(2, (200.0, 115.0), original.clone()),
    ];
    normalize_flow_geometry(&mut elements);
    let f = the_flow(&elements);
    assert_eq!(
        coords(f),
        original.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()
    );
    assert_eq!((f.x, f.y), (200.0, 115.0));
}

/// Rows, one per rule the checker adds beyond attachment: a zero-length
/// segment, collinear segments, a short first, interior and final segment
/// (with room), no minimum when the terminals crowd, and the valve's arc
/// margin.
#[test]
fn the_checker_reports_segment_minima_and_the_valve_margin() {
    let clouds_at =
        |a: (f64, f64), b: (f64, f64)| vec![cloud(3, 2, a.0, a.1), cloud(4, 2, b.0, b.1)];
    /// (label, points, valve, the message the checker must report).
    type Row = (&'static str, Vec<FlowPoint>, (f64, f64), &'static str);
    let rows: Vec<Row> = vec![
        (
            "zero length",
            vec![
                pt(0.0, 0.0, Some(3)),
                pt(0.0, 0.0, None),
                pt(100.0, 0.0, Some(4)),
            ],
            (50.0, 0.0),
            "segment 0 has zero length",
        ),
        (
            "collinear",
            vec![
                pt(0.0, 0.0, Some(3)),
                pt(50.0, 0.0, None),
                pt(100.0, 0.0, Some(4)),
            ],
            (25.0, 0.0),
            "segments 0 and 1 are collinear",
        ),
        (
            "short first segment",
            vec![
                pt(0.0, 0.0, Some(3)),
                pt(0.0, 5.0, None),
                pt(100.0, 5.0, Some(4)),
            ],
            (50.0, 5.0),
            "first segment 0 is 5.00",
        ),
        (
            "short interior segment",
            vec![
                pt(0.0, 0.0, Some(3)),
                pt(50.0, 0.0, None),
                pt(50.0, 5.0, None),
                pt(100.0, 5.0, Some(4)),
            ],
            (25.0, 0.0),
            "interior segment 1 is 5.00",
        ),
        (
            "short final segment",
            vec![
                pt(0.0, 0.0, Some(3)),
                pt(100.0, 0.0, None),
                pt(100.0, 12.0, Some(4)),
            ],
            (50.0, 0.0),
            "final segment 1 is 12.00",
        ),
        (
            "valve margin",
            vec![pt(0.0, 0.0, Some(3)), pt(100.0, 0.0, Some(4))],
            (4.0, 0.0),
            "valve 4.00 from an end of the path",
        ),
    ];
    for (label, points, valve, message) in rows {
        let last = points.last().map(|p| (p.x, p.y)).unwrap();
        let mut elements = clouds_at((points[0].x, points[0].y), last);
        elements.push(flow(2, valve, points));
        let violations = flow_invariant_violations(&elements);
        assert!(
            violations.iter().any(|v| v.contains(message)),
            "{label}: expected {message:?} in {violations:?}"
        );
    }

    // Crowded terminals: two clouds 20 apart leave no room, so a 20px pipe
    // with a 5px stub is not held to the minima.
    let mut elements = clouds_at((0.0, 0.0), (15.0, 5.0));
    elements.push(flow(
        2,
        (10.0, 5.0),
        vec![
            pt(0.0, 0.0, Some(3)),
            pt(0.0, 5.0, None),
            pt(15.0, 5.0, Some(4)),
        ],
    ));
    assert!(
        !flow_invariant_violations(&elements)
            .iter()
            .any(|v| v.contains("segment") && v.contains("under")),
        "crowded terminals are not held to the minima"
    );
}

/// `normalize_flow_geometry` is idempotent -- a second pass changes nothing --
/// and it leaves a view the checker accepts exactly as it is. Rows: a Z
/// between two stocks whose 3px riser collapses and whose merged line then
/// needs a slide into both clearance spans, so the geometry settles only over
/// several passes; and a deterministic sweep over views of one or two stocks
/// and one flow of each shape the producers hand the pass -- two-point
/// stock-to-cloud and stock-to-stock pipes, an L into a cloud, a Z between
/// stocks -- with endpoints on, near, inside and far from their stocks.
#[test]
fn normalization_is_idempotent() {
    fn twice_equals_once(elements: Vec<ViewElement>, label: &str) {
        let valid = flow_invariant_violations(&elements).is_empty();
        let mut once = elements.clone();
        normalize_flow_geometry(&mut once);
        assert!(
            !valid || once == elements,
            "{label}: a view the checker accepts moved from {elements:?} to {once:?}"
        );
        let mut twice = once.clone();
        normalize_flow_geometry(&mut twice);
        assert!(
            once == twice,
            "{label}: a second pass moved {once:?} to {twice:?}"
        );
    }

    twice_equals_once(
        vec![
            stock(1, 100.0, 100.0),
            stock(3, 300.0, 96.0),
            flow(
                2,
                (160.0, 116.0),
                vec![
                    pt(122.5, 116.0, Some(1)),
                    pt(200.0, 116.0, None),
                    pt(200.0, 113.0, None),
                    pt(277.5, 113.0, Some(3)),
                ],
            ),
        ],
        "a Z settled over several passes",
    );

    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let half = |lo: f64, hi: f64, next: &mut dyn FnMut() -> u64| {
        let unit = (next() % 1_000_000) as f64 / 1_000_000.0;
        ((lo + unit * (hi - lo)) * 2.0).round() / 2.0
    };
    for case in 0..3000 {
        let s1 = (100.0, 100.0);
        let s2 = if next() % 4 == 0 {
            (half(90.0, 140.0, &mut next), half(80.0, 130.0, &mut next))
        } else {
            (
                half(-300.0, 500.0, &mut next),
                half(-300.0, 500.0, &mut next),
            )
        };
        let near = |s: (f64, f64), next: &mut dyn FnMut() -> u64| -> (f64, f64) {
            let pick = next() % 4;
            let mut h = |lo: f64, hi: f64| {
                let unit = (next() % 1_000_000) as f64 / 1_000_000.0;
                ((lo + unit * (hi - lo)) * 2.0).round() / 2.0
            };
            match pick {
                0 => (s.0 + 22.5, h(s.1 - 17.5, s.1 + 17.5)),
                1 => (h(s.0 - 22.5, s.0 + 22.5), s.1 + 17.5),
                2 => (h(s.0 - 45.0, s.0 + 45.0), h(s.1 - 35.0, s.1 + 35.0)),
                _ => (h(s.0 - 150.0, s.0 + 150.0), h(s.1 - 150.0, s.1 + 150.0)),
            }
        };
        let p = near(s1, &mut next);
        let (points, uses_s2) = match next() % 4 {
            0 => {
                let d = half(-250.0, 250.0, &mut next);
                (
                    vec![pt(p.0, p.1, Some(1)), pt(p.0 + d, p.1, Some(11))],
                    false,
                )
            }
            1 => {
                let q = near(s2, &mut next);
                (vec![pt(p.0, p.1, Some(1)), pt(q.0, p.1, Some(3))], true)
            }
            2 => {
                let bx = half(-300.0, 500.0, &mut next);
                let cy = half(-300.0, 500.0, &mut next);
                (
                    vec![
                        pt(p.0, p.1, Some(1)),
                        pt(bx, p.1, None),
                        pt(bx, cy, Some(11)),
                    ],
                    false,
                )
            }
            _ => {
                let q = near(s2, &mut next);
                let mx = half(-300.0, 500.0, &mut next);
                (
                    vec![
                        pt(p.0, p.1, Some(1)),
                        pt(mx, p.1, None),
                        pt(mx, q.1, None),
                        pt(q.0, q.1, Some(3)),
                    ],
                    true,
                )
            }
        };
        let valve = (
            half(-300.0, 500.0, &mut next),
            half(-300.0, 500.0, &mut next),
        );
        let mut elements = vec![stock(1, s1.0, s1.1)];
        if uses_s2 {
            elements.push(stock(3, s2.0, s2.1));
        } else {
            let end = points.last().unwrap();
            elements.push(cloud(11, 2, end.x, end.y));
        }
        elements.push(flow(2, valve, points));
        twice_equals_once(elements, &format!("case {case}"));
    }
}

/// A stock-to-cloud L whose first segment runs along the stock's bottom face:
/// the Face arm needs the line 3px up, onto the cloud end's own row, where the
/// slide leaves the L's riser of no length. The next pass starts from the
/// simplified pipe, which drops the repeated point, and the straight pipe
/// comes onto the right face.
#[test]
fn a_slide_that_empties_a_neighbour_settles_on_the_next_pass() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        cloud(11, 2, 197.0, 114.5),
        flow(
            2,
            (-4.0, 417.5),
            vec![
                pt(118.0, 117.5, Some(1)),
                pt(197.0, 117.5, None),
                pt(197.0, 114.5, Some(11)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(122.5, 114.5), (197.0, 114.5)]
    );
}

/// Where the terminal bodies, each inflated by `MIN_SEGMENT`, overlap, the
/// design plan leaves G6 best effort but still demands G1-G5, so no step is
/// refused for running a segment through the other stock. Rows: a diagonal
/// pipe between s1 and a stock 30px above it, both ends off their faces.
/// Each settles with every end on its face; only crossings may remain.
#[test]
fn between_overlapping_stocks_the_ends_attach_whatever_they_cross() {
    /// (label, the second stock, the pipe, the valve).
    type Row = (&'static str, (f64, f64), Vec<FlowPoint>, (f64, f64));
    let rows: Vec<Row> = vec![(
        "a diagonal pipe",
        (101.5, 69.5),
        vec![pt(77.5, 82.5, Some(1)), pt(-41.5, 190.5, Some(3))],
        (14.75, 139.5),
    )];
    for (label, s3, points, valve) in rows {
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            stock(3, s3.0, s3.1),
            flow(2, valve, points),
        ];
        normalize_flow_geometry(&mut elements);
        let violations = flow_invariant_violations(&elements);
        assert!(
            violations
                .iter()
                .all(|v| v.contains("runs through an endpoint stock")),
            "{label}: G1-G5 must hold: {violations:?} at {:?}",
            coords(the_flow(&elements))
        );
    }
}

/// Rows, one per thing `simplify` drops or keeps: a repeated interior point;
/// a repeated endpoint, whose attachment survives; an interior point that
/// continues its segment onward; the tip of a spur, where the pipe turns back
/// along its own line; a removal that exposes the next removal (a spur whose
/// retraction leaves a repeated point, then a collinear one); and an attached
/// interior point, which is not the pass's to drop.
#[test]
fn simplify_drops_what_draws_no_route() {
    /// (label, points, the simplified points).
    type Row = (&'static str, Vec<FlowPoint>, Vec<(f64, f64, Option<i32>)>);
    let rows: Vec<Row> = vec![
        (
            "repeated interior point",
            vec![
                pt(0.0, 0.0, Some(1)),
                pt(0.0, 50.0, None),
                pt(0.0, 50.0, None),
                pt(40.0, 50.0, Some(2)),
            ],
            vec![
                (0.0, 0.0, Some(1)),
                (0.0, 50.0, None),
                (40.0, 50.0, Some(2)),
            ],
        ),
        (
            "repeated endpoint keeps its attachment",
            vec![
                pt(0.0, 0.0, Some(1)),
                pt(0.0, 50.0, None),
                pt(40.0, 50.0, None),
                pt(40.0, 50.0, Some(2)),
            ],
            vec![
                (0.0, 0.0, Some(1)),
                (0.0, 50.0, None),
                (40.0, 50.0, Some(2)),
            ],
        ),
        (
            "onward collinear point",
            vec![
                pt(0.0, 0.0, Some(1)),
                pt(0.0, 20.0, None),
                pt(0.0, 50.0, None),
                pt(40.0, 50.0, Some(2)),
            ],
            vec![
                (0.0, 0.0, Some(1)),
                (0.0, 50.0, None),
                (40.0, 50.0, Some(2)),
            ],
        ),
        (
            "a spur's tip",
            vec![
                pt(0.0, 0.0, Some(1)),
                pt(0.0, 50.0, None),
                pt(40.0, 50.0, None),
                pt(20.0, 50.0, Some(2)),
            ],
            vec![
                (0.0, 0.0, Some(1)),
                (0.0, 50.0, None),
                (20.0, 50.0, Some(2)),
            ],
        ),
        (
            "a removal exposes the next",
            vec![
                pt(0.0, 0.0, Some(1)),
                pt(0.0, 50.0, None),
                pt(40.0, 50.0, None),
                pt(40.0, 60.0, None),
                pt(40.0, 50.0, None),
                pt(80.0, 50.0, Some(2)),
            ],
            vec![
                (0.0, 0.0, Some(1)),
                (0.0, 50.0, None),
                (80.0, 50.0, Some(2)),
            ],
        ),
        (
            "an attached interior point stays",
            vec![
                pt(0.0, 0.0, Some(1)),
                pt(0.0, 20.0, Some(9)),
                pt(0.0, 50.0, Some(2)),
            ],
            vec![
                (0.0, 0.0, Some(1)),
                (0.0, 20.0, Some(9)),
                (0.0, 50.0, Some(2)),
            ],
        ),
    ];
    for (label, mut points, expected) in rows {
        simplify(&mut points);
        let got: Vec<(f64, f64, Option<i32>)> = points
            .iter()
            .map(|p| (p.x, p.y, p.attached_to_uid))
            .collect();
        assert_eq!(got, expected, "{label}");
    }
}

/// A U-turn whose 2px riser is under the minimum: collapsing the riser folds
/// the pipe back along its own line, and the fold's tip is a spur drawn over
/// itself, so the pipe settles without it -- a straight run from the source
/// into an L to the sink -- rather than keeping two collinear segments.
#[test]
fn a_pipe_turning_back_along_its_own_line_settles_without_the_spur() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 413.5, 450.0),
        flow(
            2,
            (14.0, 178.5),
            vec![
                pt(122.5, 105.0, Some(1)),
                pt(122.5, 404.0, None),
                pt(-81.0, 404.0, None),
                pt(-81.0, 406.0, None),
                pt(413.5, 406.0, Some(3)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![
            (119.5, 117.5),
            (119.5, 404.0),
            (413.5, 404.0),
            (413.5, 432.5)
        ]
    );
}

/// A pass starts from the simplified pipe. A repeated interior point on a
/// pipe that runs back over itself reads, unsimplified, as a zero-length
/// segment between two collinear runs, and no arm can bring the ends on;
/// simplified, it is a straight pipe whose ends enter the two facing faces.
#[test]
fn a_pass_starts_from_the_pipe_without_repeated_or_collinear_points() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 489.0, 112.0),
        flow(
            2,
            (33.5, 100.0),
            vec![
                pt(52.5, 100.0, Some(1)),
                pt(-121.0, 100.0, None),
                pt(-121.0, 100.0, None),
                pt(463.5, 100.0, Some(3)),
            ],
        ),
    ]);
    let f = the_flow(&elements);
    assert_eq!(coords(f), vec![(122.5, 100.0), (466.5, 100.0)]);
    assert_eq!((f.x, f.y), (132.5, 100.0));
}

/// Neither step commits a segment through the body of a stock the pipe ends
/// on where the pipe had none. Rows: a collapse whose candidate would run the
/// merged line through the sink's body, where the other candidate settles
/// the pipe; an attach whose leg into the source would cross the sink, where
/// the pipe settles through a longer route; a Z whose leg into its sink
/// would drop through the source's body, which comes back with no crossing
/// and no more violations than it had; a pipe the producer drew up through its
/// own source, whose steps keep that crossing on the way to a valid route, so
/// the rule refuses only crossings a step adds; and a pipe already crossing its
/// source's body, whose collapse would run two more segments through bodies,
/// so a pipe that crosses may still not gain crossings.
#[test]
fn a_collapse_or_attach_never_adds_a_body_crossing() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 179.5, -264.5),
        flow(
            2,
            (-114.0, 268.5),
            vec![
                pt(111.5, 100.0, Some(1)),
                pt(-12.5, 100.0, None),
                pt(-12.5, 112.5, None),
                pt(24.5, 112.5, None),
                pt(24.5, 118.0, None),
                pt(179.5, 118.0, Some(3)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![
            (77.5, 100.0),
            (-12.5, 100.0),
            (-12.5, 118.0),
            (179.5, 118.0),
            (179.5, -247.0)
        ],
        "collapse"
    );

    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 141.0, -22.5),
        flow(
            2,
            (196.0, -56.25),
            vec![
                pt(118.5, -29.0, Some(3)),
                pt(424.0, -29.0, None),
                pt(424.0, -67.5, None),
                pt(196.0, -67.5, None),
                pt(196.0, -47.5, None),
                pt(202.5, -47.5, Some(1)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![
            (163.5, -29.0),
            (424.0, -29.0),
            (424.0, -67.5),
            (196.0, -67.5),
            (196.0, 85.5),
            (122.5, 85.5)
        ],
        "attach"
    );

    let mut elements = vec![
        stock(1, 100.0, 100.0),
        stock(3, 16.0, 99.5),
        flow(
            2,
            (79.0, 165.5),
            vec![
                pt(85.5, 117.5, Some(1)),
                pt(85.5, 144.0, None),
                pt(79.0, 144.0, None),
                pt(79.0, 194.0, Some(3)),
            ],
        ),
    ];
    let before = flow_invariant_violations(&elements);
    normalize_flow_geometry(&mut elements);
    let after = flow_invariant_violations(&elements);
    assert!(
        !after
            .iter()
            .any(|v| v.contains("runs through an endpoint stock")),
        "a Z into its sink: {after:?}"
    );
    assert!(
        after.len() <= before.len(),
        "a Z into its sink: {before:?} -> {after:?}"
    );

    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, -291.0, -131.5),
        flow(
            2,
            (-241.5, -115.0),
            vec![
                pt(121.5, 100.0, Some(1)),
                pt(121.5, -115.0, None),
                pt(-331.0, -115.0, Some(3)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(119.5, 82.5), (119.5, -117.0), (-268.5, -117.0)],
        "a pipe drawn through its own source"
    );

    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 244.5, 109.5),
        flow(
            2,
            (108.5, 111.75),
            vec![
                pt(122.5, 106.0, Some(1)),
                pt(119.0, 106.0, None),
                pt(119.0, 101.5, None),
                pt(108.5, 101.5, None),
                pt(108.5, 112.5, Some(3)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(122.5, 101.5), (222.0, 101.5)],
        "a pipe already crossing its source"
    );
}

/// A collapse candidate need not leave every stock end valid. The pipe climbs
/// from its source, doubles back over the sink's body, and drops 9.5px, under
/// a riser, to a sink end at the stock's center. The candidate that merges
/// that riser still leaves the sink end at the center; the next passes bring
/// it onto the bottom face and settle the pipe into an L. Requiring every end
/// valid would refuse the candidate and keep the riser and the segments
/// through the sink's body.
#[test]
fn a_collapse_may_leave_an_end_for_the_next_pass_to_attach() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 200.0, 179.0),
        flow(
            2,
            (71.5, 100.0),
            vec![
                pt(84.5, 179.0, Some(3)),
                pt(84.5, 90.5, None),
                pt(41.0, 90.5, None),
                pt(41.0, 100.0, None),
                pt(100.0, 100.0, Some(1)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(177.5, 179.0), (84.5, 179.0), (84.5, 117.5)]
    );
}

/// When the terminals crowd each other the checker demands no segment
/// minimum, so a pipe it accepts is not moved for a short segment. Rows: an L
/// from a stock's right face into a cloud 0.5 above the pipe, and a cloud
/// stub 4.5 long into a 0.5px final segment on a stock's face.
#[test]
fn a_crowded_pipe_the_checker_accepts_is_left_as_it_is() {
    /// (label, the view).
    type Row = (&'static str, Vec<ViewElement>);
    let rows: Vec<Row> = vec![
        (
            "an L into a cloud",
            vec![
                stock(1, 100.0, 100.0),
                stock(4, 148.5, 133.0),
                cloud(3, 2, 136.0, 94.0),
                flow(
                    2,
                    (130.0, 94.5),
                    vec![
                        pt(122.5, 94.5, Some(1)),
                        pt(136.0, 94.5, None),
                        pt(136.0, 94.0, Some(3)),
                    ],
                ),
            ],
        ),
        (
            "a final segment of half a pixel",
            vec![
                stock(1, 100.0, 100.0),
                stock(4, 62.0, 134.5),
                cloud(3, 2, 123.0, 103.5),
                flow(
                    2,
                    (123.0, 101.0),
                    vec![
                        pt(123.0, 103.5, Some(3)),
                        pt(123.0, 99.0, None),
                        pt(122.5, 99.0, Some(1)),
                    ],
                ),
            ],
        ),
    ];
    for (label, original) in rows {
        assert_eq!(
            flow_invariant_violations(&original),
            Vec::<String>::new(),
            "{label}: the checker accepts the input"
        );
        let mut elements = original.clone();
        normalize_flow_geometry(&mut elements);
        assert!(elements == original, "{label}: moved to {elements:?}");
    }
}

/// With crowded terminals a segment shorter than `CROWDED_MIN_SEGMENT` is
/// still collapsed while it keeps a stock end off its face. A 0.5px riser
/// sits between the sink's run and a stub that ends at the stock's center;
/// merging it lets the attach step bring that end onto the bottom face.
#[test]
fn a_crowded_segment_is_collapsed_to_bring_an_end_onto_its_face() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 87.0, 135.0),
        flow(
            2,
            (112.25, 115.0),
            vec![
                pt(112.5, 135.0, Some(3)),
                pt(112.5, 115.0, None),
                pt(112.0, 115.0, None),
                pt(112.0, 100.0, Some(1)),
            ],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(109.5, 135.0), (112.5, 135.0), (112.5, 117.5)]
    );
}

/// The Face arm holds the segment an end leaves to that end's own minimum. A
/// two-point pipe runs along s1's top face and past the top of s3, far below:
/// s3's end drops a leg into its top face, and because the sink's arm runs
/// first, the segment s1's end leaves as it enters its right face is a stub,
/// 11.5px, held to `MIN_SEGMENT` rather than to the sink segment's
/// `MIN_SINK_SEGMENT`.
#[test]
fn a_face_end_needs_only_the_minimum_of_the_segment_it_leaves() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 124.0, 282.0),
        flow(
            2,
            (116.5, 5.5),
            vec![pt(100.5, 82.5, Some(1)), pt(134.0, 82.5, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(122.5, 85.5), (134.0, 85.5), (134.0, 264.5)]
    );
}

/// A jog is not refused for where the valve sits. The diagonal pipe
/// straightens onto a line inside s3's clearance span but under s1's, and s3's
/// end is a valid slot that pins it, so s1's end jogs to its own line. The
/// valve lies on the shared line behind s1's face; the jog's step is placed
/// all the same, and the valve is projected onto the settled pipe.
#[test]
fn a_jog_does_not_step_around_the_valve() {
    let elements = normalized(vec![
        stock(1, 100.0, 100.0),
        stock(3, 156.5, 94.0),
        flow(
            2,
            (93.75, 82.5),
            vec![pt(56.0, 71.0, Some(1)), pt(134.0, 94.5, Some(3))],
        ),
    ]);
    assert_eq!(
        coords(the_flow(&elements)),
        vec![(122.5, 85.5), (123.5, 85.5), (123.5, 82.75), (134.0, 82.75)]
    );
}

/// A jog whose step would pass the adjacent point is refused. Two overlapping
/// stocks and a vertical pipe through both: s1's end takes the line inside
/// s1's clearance span, and s2's end, whose span excludes that line, would jog
/// to its own line, stepping a stub short of s2's top face. That step lies
/// beyond s1's end, so the route would pass it and double back over itself.
/// No invariant names such a route, so the pipe is left as the producer drew
/// it.
#[test]
fn a_jog_whose_step_would_double_back_is_refused() {
    let original = vec![pt(122.5, 131.0, Some(2)), pt(122.5, 88.5, Some(1))];
    let mut elements = vec![
        stock(1, 100.0, 100.0),
        stock(2, 140.0, 118.5),
        flow(10, (-132.0, -140.0), original.clone()),
    ];
    normalize_flow_geometry(&mut elements);
    assert_eq!(
        coords(the_flow(&elements)),
        original.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>()
    );
}

/// Rows, one per arm of `settle_laid_out_valve`: a valve already
/// `VALVE_CLAMP_MARGIN` inside its segment is not moved; a valve within the
/// margin of its segment's end is clamped along that segment, as the editor
/// clamps a dragged valve; a valve on a segment shorter than twice the margin
/// moves to the nearest position of a longer segment; a pipe whose every
/// segment is that short leaves the valve where it is; a valve on a sibling's
/// pipe moves to the nearest position clear of both the sibling and its
/// segment's ends (the earlier segment on a tie); a valve clear of the sibling
/// is not moved; and where a sibling runs within the margin of every position,
/// the valve keeps the segment margin alone -- clamped along its own segment,
/// or at the longest segment's middle when its own is short.
#[test]
fn a_laid_out_valve_keeps_the_margin_from_its_segments_ends() {
    let z = vec![
        pt(277.5, 115.25, Some(3)),
        pt(200.0, 115.25, None),
        pt(200.0, 95.0, None),
        pt(122.5, 95.0, Some(1)),
    ];
    let z_short_riser = vec![
        pt(277.5, 110.0, Some(3)),
        pt(200.0, 110.0, None),
        pt(200.0, 95.0, None),
        pt(122.5, 95.0, Some(1)),
    ];
    let tiny = vec![
        pt(0.0, 0.0, Some(3)),
        pt(0.0, 8.0, None),
        pt(6.0, 8.0, Some(4)),
    ];
    // A Z between offset stocks crossing a sibling straight between them at
    // y = 110, and a long pipe with a sibling running 5px beside it end to end.
    let crossing_z = vec![
        pt(277.5, 122.25, Some(3)),
        pt(200.0, 122.25, None),
        pt(200.0, 97.75, None),
        pt(122.5, 97.75, Some(1)),
    ];
    let sibling: &[FlowPoint] = &[pt(122.5, 110.0, Some(1)), pt(277.5, 110.0, Some(3))];
    let straight = vec![pt(0.0, 0.0, Some(3)), pt(0.0, 100.0, Some(4))];
    let beside: &[FlowPoint] = &[pt(5.0, -50.0, Some(5)), pt(5.0, 150.0, Some(6))];
    let between_runs: &[FlowPoint] = &[pt(100.0, 103.0, Some(5)), pt(300.0, 103.0, Some(6))];
    /// (label, path, other pipes, valve before, valve after).
    type Row<'a> = (
        &'a str,
        &'a [FlowPoint],
        Vec<&'a [FlowPoint]>,
        (f64, f64),
        (f64, f64),
    );
    let rows: [Row; 8] = [
        (
            "inside the margin",
            &z,
            vec![],
            (240.0, 115.25),
            (240.0, 115.25),
        ),
        (
            "within the margin of a bend",
            &z,
            vec![],
            (200.0, 105.5),
            (200.0, 105.25),
        ),
        (
            "on a riser shorter than two margins: the nearest position of a longer segment",
            &z_short_riser,
            vec![],
            (200.0, 102.0),
            (190.0, 95.0),
        ),
        (
            "on a short riser, every longer segment beside a sibling: the longest one's middle",
            &z_short_riser,
            vec![between_runs],
            (200.0, 102.0),
            (238.75, 110.0),
        ),
        ("every segment short", &tiny, vec![], (0.0, 4.0), (0.0, 4.0)),
        (
            "on a sibling's pipe: the nearest position clear of it",
            &crossing_z,
            vec![sibling],
            (200.0, 110.0),
            (210.0, 122.25),
        ),
        (
            "clear of the sibling and the segment's ends",
            &crossing_z,
            vec![sibling],
            (240.0, 122.25),
            (240.0, 122.25),
        ),
        (
            "no position clear of the sibling: the segment margin alone",
            &straight,
            vec![beside],
            (0.0, 3.0),
            (0.0, 10.0),
        ),
    ];
    for (label, points, others, valve, expected) in rows {
        let mut v = valve;
        settle_laid_out_valve(points, &mut v, &others);
        assert_eq!(v, expected, "{label}");
    }
}

#[test]
fn clamp_to_face_span_keeps_corner_clearance() {
    assert_eq!(clamp_to_face_span(100.0, 100.0, 22.5), 100.0);
    assert_eq!(clamp_to_face_span(130.0, 100.0, 22.5), 119.5);
    assert_eq!(clamp_to_face_span(70.0, 100.0, 22.5), 80.5);
    assert_eq!(clamp_to_face_span(117.5, 100.0, 17.5), 114.5);
}
