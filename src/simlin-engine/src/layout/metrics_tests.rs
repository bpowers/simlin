// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel::view_element::{self, LabelSide, LinkShape};
use crate::diagram::common::segment_length_in_rect;
use crate::diagram::constants::{AUX_RADIUS, STOCK_WIDTH};
use crate::layout::taste::{Degradation, degrade};
use proptest::prelude::*;

// --- fixture helpers ---

fn stock(uid: i32, name: &str, x: f64, y: f64) -> ViewElement {
    ViewElement::Stock(view_element::Stock {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

fn aux(uid: i32, name: &str, x: f64, y: f64) -> ViewElement {
    aux_side(uid, name, x, y, LabelSide::Bottom)
}

fn aux_side(uid: i32, name: &str, x: f64, y: f64, side: LabelSide) -> ViewElement {
    ViewElement::Aux(view_element::Aux {
        name: name.to_string(),
        uid,
        x,
        y,
        label_side: side,
        compat: None,
    })
}

/// A cloud at `(x, y)`: a 27x27 shape box and NO label, the cleanest
/// "obscuring shape" fixture for label terms.
fn cloud(uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Cloud(view_element::Cloud {
        uid,
        flow_uid: -1,
        x,
        y,
        compat: None,
    })
}

fn straight_link(uid: i32, from_uid: i32, to_uid: i32) -> ViewElement {
    ViewElement::Link(view_element::Link {
        uid,
        from_uid,
        to_uid,
        shape: LinkShape::Straight,
        polarity: None,
    })
}

fn arc_link(uid: i32, from_uid: i32, to_uid: i32, angle: f64) -> ViewElement {
    ViewElement::Link(view_element::Link {
        uid,
        from_uid,
        to_uid,
        shape: LinkShape::Arc(angle),
        polarity: None,
    })
}

/// A flow valve at `(x, y)` with a two-point polyline through the valve whose
/// endpoints attach to `from_uid` and `to_uid`.
fn flow_between(uid: i32, name: &str, x: f64, y: f64, from_uid: i32, to_uid: i32) -> ViewElement {
    flow_with_points(
        uid,
        name,
        (x, y),
        vec![(x, y, Some(from_uid)), (x, y, Some(to_uid))],
    )
}

fn flow_with_points(
    uid: i32,
    name: &str,
    valve: (f64, f64),
    points: Vec<(f64, f64, Option<i32>)>,
) -> ViewElement {
    ViewElement::Flow(view_element::Flow {
        name: name.to_string(),
        uid,
        x: valve.0,
        y: valve.1,
        label_side: LabelSide::Bottom,
        points: points
            .into_iter()
            .map(|(x, y, attached_to_uid)| view_element::FlowPoint {
                x,
                y,
                attached_to_uid,
            })
            .collect(),
        compat: None,
        label_compat: None,
    })
}

fn make_view(elements: Vec<ViewElement>) -> datamodel::StockFlow {
    datamodel::StockFlow {
        name: None,
        elements,
        view_box: datamodel::Rect {
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

fn cfg() -> LayoutConfig {
    LayoutConfig::default()
}

/// An alias (ghost) of the element with uid `alias_of_uid`, at `(x, y)`.
fn alias_of(uid: i32, alias_of_uid: i32, x: f64, y: f64) -> ViewElement {
    ViewElement::Alias(view_element::Alias {
        uid,
        alias_of_uid,
        x,
        y,
        label_side: LabelSide::Bottom,
        compat: None,
    })
}

/// The label box an element's own name occupies, measured exactly as the
/// metric measures it.
fn label_box(e: &ViewElement) -> Rect {
    let side = element_label_side(e).expect("labeled element");
    label_bounds(&element_label_props_for(e, side).expect("labeled element"))
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

// --- the drawn scene ---

#[test]
fn test_flow_valve_is_scored_at_its_drawn_radius() {
    // `render_flow` draws the valve circle at AUX_RADIUS (9px). A stock whose
    // edge sits 7px from the valve center overlaps the drawn circle; scoring the
    // valve at the 6px bounds radius would miss it.
    let valve_x = 100.0;
    let stock_center_x = valve_x + 7.0 + STOCK_WIDTH / 2.0;
    let view = make_view(vec![
        flow_with_points(
            1,
            "f",
            (valve_x, 100.0),
            vec![(40.0, 100.0, None), (valve_x - 20.0, 100.0, None)],
        ),
        stock(2, "s", stock_center_x, 100.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(
        m.node_overlap > 0.0,
        "a stock 7px from the valve center covers the drawn 9px valve circle"
    );
    let shape = node_shape_box(&view.elements[0]).unwrap();
    assert!(close(common::rect_width(&shape), 2.0 * AUX_RADIUS));
}

#[test]
fn test_pipe_through_a_label_strikes_it_but_own_pipe_does_not() {
    // A horizontal pipe (flow #1, valve far to the right) runs straight through
    // aux #2's Bottom label: a line through the name, struck out exactly as a
    // link through it would be, not a thin band of covered area. The flow's own
    // label is never charged against its own pipe.
    let a = aux(2, "a fairly long name", 200.0, 100.0);
    let lbl = label_box(&a);
    let pipe_y = (lbl.top + lbl.bottom) / 2.0;
    let view = make_view(vec![
        flow_with_points(
            1,
            "f",
            (600.0, pipe_y),
            vec![(0.0, pipe_y, None), (700.0, pipe_y, None)],
        ),
        a,
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    // Two labels in the view: the aux's (fully struck: the run through the
    // text far exceeds its height) and the flow's own (clear of anything).
    assert!(
        close(m.label_connector_overlap, 1.0 / 2.0),
        "label_connector_overlap {}",
        m.label_connector_overlap
    );
    assert_eq!(
        m.label_overlap, 0.0,
        "a pipe strikes a name; it covers no area"
    );
}

#[test]
fn test_a_pipe_into_a_stock_through_the_stocks_name_strikes_it() {
    // An inflow arrives from above into stock #1 whose name sits on top: the
    // pipe runs down through the name. Unlike a node's own link, which at
    // least points at the name, a pipe along the face's normal simply writes
    // over it, so it counts in full. Two labels -> a rate of 1/2.
    let s = ViewElement::Stock(view_element::Stock {
        name: "a long stock name".to_string(),
        uid: 1,
        x: 200.0,
        y: 200.0,
        label_side: LabelSide::Top,
        compat: None,
    });
    let top = 200.0 - crate::diagram::constants::STOCK_HEIGHT / 2.0;
    let view = make_view(vec![
        s,
        flow_with_points(
            2,
            "f",
            (500.0, 60.0),
            vec![(200.0, 0.0, None), (200.0, top, Some(1))],
        ),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(
        close(m.label_connector_overlap, 1.0 / 2.0),
        "label_connector_overlap {}",
        m.label_connector_overlap
    );
}

// --- alias scoring ---

#[test]
fn test_alias_node_overlap_charged() {
    // An alias stacked exactly on an aux vs the same alias far away: the
    // stacked layout must score strictly worse on node_overlap.
    let stacked = make_view(vec![
        aux(1, "source variable", 100.0, 100.0),
        aux(2, "another aux", 300.0, 100.0),
        alias_of(3, 1, 300.0, 100.0),
    ]);
    let apart = make_view(vec![
        aux(1, "source variable", 100.0, 100.0),
        aux(2, "another aux", 300.0, 100.0),
        alias_of(3, 1, 600.0, 100.0),
    ]);
    let m_stacked = compute_layout_metrics(&stacked, &cfg());
    let m_apart = compute_layout_metrics(&apart, &cfg());
    assert!(m_stacked.node_overlap > m_apart.node_overlap);
    assert!(m_apart.node_overlap.abs() < 1e-9);
}

#[test]
fn test_alias_label_sized_by_source_name() {
    // The alias's label box is the SOURCE element's name: a long source name
    // collides with a nearby aux's label where a short one does not.
    let dx = 80.0;
    let long_name = make_view(vec![
        aux(1, "an extremely long variable name here", 100.0, 600.0),
        aux(2, "consumer", 300.0, 100.0),
        alias_of(3, 1, 300.0 + dx, 100.0),
    ]);
    let short_name = make_view(vec![
        aux(1, "x", 100.0, 600.0),
        aux(2, "consumer", 300.0, 100.0),
        alias_of(3, 1, 300.0 + dx, 100.0),
    ]);
    let m_long = compute_layout_metrics(&long_name, &cfg());
    let m_short = compute_layout_metrics(&short_name, &cfg());
    assert!(m_long.label_overlap > m_short.label_overlap);
}

#[test]
fn test_alias_with_dangling_source_is_ignored() {
    // An alias whose source resolves to nothing still draws its circle (so it
    // can overlap) but has no derivable label; nothing panics.
    let view = make_view(vec![
        aux(1, "real aux", 100.0, 100.0),
        alias_of(2, 999, 100.0, 100.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(m.node_overlap > 0.0);
    assert!(m.label_overlap.is_finite());
}

#[test]
fn test_alias_extends_view_bounding_box() {
    // A far-flung alias is a drawn element: it widens the bounding box.
    let compact = make_view(vec![
        aux(1, "a", 100.0, 100.0),
        aux(2, "b", 300.0, 100.0),
        straight_link(10, 1, 2),
    ]);
    let with_far_alias = make_view(vec![
        aux(1, "a", 100.0, 100.0),
        aux(2, "b", 300.0, 100.0),
        straight_link(10, 1, 2),
        alias_of(3, 1, 2000.0, 100.0),
    ]);
    let m_compact = compute_layout_metrics(&compact, &cfg());
    let m_far = compute_layout_metrics(&with_far_alias, &cfg());
    assert!(m_far.aspect_penalty > m_compact.aspect_penalty);
}

/// Scale every coordinate of a view by `s` (element centers and flow points).
fn scale_view(view: &datamodel::StockFlow, s: f64) -> datamodel::StockFlow {
    let elements = view
        .elements
        .iter()
        .map(|e| match e {
            ViewElement::Aux(a) => ViewElement::Aux(view_element::Aux {
                x: a.x * s,
                y: a.y * s,
                ..a.clone()
            }),
            ViewElement::Stock(st) => ViewElement::Stock(view_element::Stock {
                x: st.x * s,
                y: st.y * s,
                ..st.clone()
            }),
            ViewElement::Flow(f) => ViewElement::Flow(view_element::Flow {
                x: f.x * s,
                y: f.y * s,
                points: f
                    .points
                    .iter()
                    .map(|p| view_element::FlowPoint {
                        x: p.x * s,
                        y: p.y * s,
                        attached_to_uid: p.attached_to_uid,
                    })
                    .collect(),
                ..f.clone()
            }),
            ViewElement::Module(m) => ViewElement::Module(view_element::Module {
                x: m.x * s,
                y: m.y * s,
                ..m.clone()
            }),
            ViewElement::Cloud(c) => ViewElement::Cloud(view_element::Cloud {
                x: c.x * s,
                y: c.y * s,
                ..c.clone()
            }),
            ViewElement::Alias(a) => ViewElement::Alias(view_element::Alias {
                x: a.x * s,
                y: a.y * s,
                ..a.clone()
            }),
            other => other.clone(),
        })
        .collect();
    datamodel::StockFlow {
        elements,
        ..view.clone()
    }
}

// --- node_overlap: mean covered fraction of each node's shape ---

#[test]
fn test_node_overlap_known_overlap_fraction() {
    // Two stocks whose centers are 20px apart: each shape is covered over a
    // 25x35 band of its 45x35 area, so both nodes are 25/45 covered and the
    // mean over the two nodes is 25/45.
    let view = make_view(vec![
        stock(1, "a", 100.0, 100.0),
        stock(2, "b", 120.0, 100.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    let expected = 25.0 / 45.0;
    assert!(
        close(m.node_overlap, expected),
        "node_overlap {} != {expected}",
        m.node_overlap
    );
}

#[test]
fn test_node_overlap_is_a_rate_over_nodes() {
    // The same stacked pair plus eight far-away stocks: the two covered nodes
    // are now two of ten, so the rate drops to a fifth of the pair's.
    let pair = make_view(vec![
        stock(1, "a", 100.0, 100.0),
        stock(2, "b", 120.0, 100.0),
    ]);
    let mut elements = vec![stock(1, "a", 100.0, 100.0), stock(2, "b", 120.0, 100.0)];
    for k in 0..8 {
        elements.push(stock(10 + k, "far", 1000.0 + f64::from(k) * 200.0, 1000.0));
    }
    let diluted = make_view(elements);
    let m_pair = compute_layout_metrics(&pair, &cfg());
    let m_diluted = compute_layout_metrics(&diluted, &cfg());
    assert!(close(m_diluted.node_overlap, m_pair.node_overlap / 5.0));
}

#[test]
fn test_node_overlap_touching_shapes_is_zero() {
    let view = make_view(vec![
        stock(1, "a", 0.0, 0.0),
        stock(2, "b", STOCK_WIDTH, 0.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.node_overlap, 0.0);
}

#[test]
fn test_node_overlap_disjoint_is_zero() {
    let view = make_view(vec![
        stock(1, "a", 0.0, 0.0),
        stock(2, "b", 500.0, 500.0),
        aux(3, "c", 1000.0, 0.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.node_overlap, 0.0);
}

#[test]
fn test_node_overlap_labels_overlap_shapes_disjoint_is_zero() {
    // Two Bottom-labeled auxes 40px apart: shapes disjoint, labels overlapping.
    // node_overlap ignores labels; label_overlap charges them.
    let view = make_view(vec![
        aux(1, "samename", 0.0, 0.0),
        aux(2, "samename", 40.0, 0.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.node_overlap, 0.0);
    assert!(m.label_overlap > 0.0);
}

// --- node_connector_overlap ---

#[test]
fn test_node_connector_overlap_through_third_node() {
    // A link between two far-apart auxes passes horizontally through a stock.
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 400.0, 0.0),
        stock(3, "s", 200.0, 0.0),
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    let connectors = collect_connector_geometry(&view.elements);
    assert_eq!(connectors.len(), 1);
    let c = &connectors[0];
    let stock_box = node_shape_box(&stock(3, "s", 200.0, 0.0)).unwrap();
    let inside: f64 = c
        .polyline
        .windows(2)
        .map(|seg| segment_length_in_rect(&seg[0], &seg[1], &stock_box))
        .sum();
    assert!(close(m.node_connector_overlap, inside / c.length));
}

#[test]
fn test_node_connector_overlap_avoids_all_is_zero() {
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 400.0, 0.0),
        stock(3, "s", 200.0, 500.0),
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.node_connector_overlap, 0.0);
}

#[test]
fn test_node_connector_overlap_under_label_only_is_zero() {
    // The link at y=0 passes under stock #3's label (which hangs below its
    // shape) but never under the shape itself: not charged here (the label
    // term charges it).
    let label_only = stock(3, "s", 200.0, -25.0);
    let shape = node_shape_box(&label_only).unwrap();
    let lbl = label_box(&label_only);
    assert!(shape.bottom < 0.0, "fixture: the shape clears the line");
    assert!(
        lbl.bottom > 0.0 && lbl.top < 0.0,
        "fixture: the label spans the line"
    );
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 400.0, 0.0),
        label_only,
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.node_connector_overlap, 0.0);
    assert!(m.label_connector_overlap > 0.0);
}

#[test]
fn test_link_along_a_foreign_pipe_is_charged() {
    // A link runs along a pipe it has nothing to do with: it reads as part of
    // the flow.
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 400.0, 0.0),
        flow_with_points(
            3,
            "f",
            (200.0, 300.0),
            vec![(100.0, 0.0, None), (300.0, 0.0, None)],
        ),
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(m.node_connector_overlap > 0.0);
}

/// Length of segment p0->p1 covered by the UNION of `rects`, for horizontal
/// segments only: the independent oracle for the union tests.
fn union_segment_length_in_rects(p0: &Point, p1: &Point, rects: &[Rect]) -> f64 {
    let seg_len = ((p1.x - p0.x).powi(2) + (p1.y - p0.y).powi(2)).sqrt();
    if seg_len == 0.0 {
        return 0.0;
    }
    let mut intervals: Vec<(f64, f64)> = Vec::new();
    for r in rects {
        if segment_length_in_rect(p0, p1, r) <= 0.0 {
            continue;
        }
        let (xa, xb) = (p0.x.min(p1.x), p0.x.max(p1.x));
        let span = p1.x - p0.x;
        let t_lo = ((xa.max(r.left) - p0.x) / span).clamp(0.0, 1.0);
        let t_hi = ((xb.min(r.right) - p0.x) / span).clamp(0.0, 1.0);
        intervals.push((t_lo.min(t_hi), t_lo.max(t_hi)));
    }
    merged_interval_length(&mut intervals) * seg_len
}

#[test]
fn test_node_connector_overlap_union_of_overlapping_boxes() {
    // Two overlapping non-incident stocks straddle the link: the covered length
    // is their UNION, not the sum.
    let s3 = stock(3, "s3", 200.0, 0.0);
    let s4 = stock(4, "s4", 210.0, 0.0);
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 400.0, 0.0),
        s3.clone(),
        s4.clone(),
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    let connectors = collect_connector_geometry(&view.elements);
    let c = &connectors[0];
    let boxes = [node_shape_box(&s3).unwrap(), node_shape_box(&s4).unwrap()];
    let union_len: f64 = c
        .polyline
        .windows(2)
        .map(|seg| union_segment_length_in_rects(&seg[0], &seg[1], &boxes))
        .sum();
    assert!(close(m.node_connector_overlap, union_len / c.length));
    assert!(m.node_connector_overlap <= 1.0);
}

#[test]
fn test_node_connector_overlap_coincident_boxes_counted_once() {
    // A short link fully inside two coincident stocks: the fraction is exactly
    // 1.0, never the 2.0 a per-box sum would report.
    let view = make_view(vec![
        aux(1, "a", 180.0, 0.0),
        aux(2, "b", 220.0, 0.0),
        stock(3, "s3", 200.0, 0.0),
        stock(4, "s4", 200.0, 0.0),
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(
        close(m.node_connector_overlap, 1.0),
        "{}",
        m.node_connector_overlap
    );
}

// --- label_overlap: mean covered fraction of each label ---

#[test]
fn test_label_overlap_coincident_labels_are_fully_obscured() {
    let view = make_view(vec![
        aux(1, "samename", 100.0, 100.0),
        aux(2, "samename", 100.0, 100.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(close(m.label_overlap, 1.0), "{}", m.label_overlap);
}

#[test]
fn test_label_overlap_disjoint_is_zero() {
    let view = make_view(vec![aux(1, "a", 0.0, 0.0), aux(2, "b", 1000.0, 1000.0)]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.label_overlap, 0.0);
}

#[test]
fn test_label_overlap_hand_computed_pair() {
    // Two Bottom-labeled "samename" auxes 40px apart. Each label box is 58x14
    // (8*6+10 wide), the labels overlap over 18x14, and neither label reaches
    // the other aux's shape. Each label is 252/812 covered: the mean over the
    // two labels is 252/812.
    let view = make_view(vec![
        aux(1, "samename", 0.0, 0.0),
        aux(2, "samename", 40.0, 0.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(close(m.label_overlap, 252.0 / 812.0), "{}", m.label_overlap);
}

#[test]
fn test_label_overlap_never_charged_against_own_shape() {
    let view = make_view(vec![aux(1, "samename", 0.0, 0.0)]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.label_overlap, 0.0);
}

#[test]
fn test_label_overlap_normalizes_by_label_count_not_area() {
    // A cloud clips 6.5x14 of a 22x14 label (91/308 covered). Fifteen far-away
    // labels dilute the rate by COUNT -- identically whether their names are
    // long or short, so a big label elsewhere never hides a small collision.
    let build = |filler: &str| {
        let mut elements = vec![aux(1, "ab", 0.0, 0.0), cloud(2, 18.0, 20.0)];
        for k in 0..15 {
            elements.push(aux(100 + k, filler, 3000.0 + f64::from(k) * 1000.0, 3000.0));
        }
        make_view(elements)
    };
    let expected = (91.0 / 308.0) / 16.0;
    for filler in ["abcdefghijklmnopqrst", "x"] {
        let m = compute_layout_metrics(&build(filler), &cfg());
        assert!(
            close(m.label_overlap, expected),
            "filler {filler:?}: {} expected {expected}",
            m.label_overlap
        );
    }
}

// --- label_connector_overlap: lines through names ---

#[test]
fn test_link_through_a_label_strikes_it_out() {
    // A horizontal link through the middle of aux #3's single-line label: the
    // run inside the (inset) text box far exceeds the box's height, so that
    // label is fully struck (1.0). Three labels -> a rate of 1/3.
    let target = aux(3, "a long label name", 200.0, -30.0);
    let lbl = label_box(&target);
    let y = (lbl.top + lbl.bottom) / 2.0;
    let view = make_view(vec![
        aux(1, "a", 0.0, y),
        aux(2, "b", 400.0, y),
        target,
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(
        close(m.label_connector_overlap, 1.0 / 3.0),
        "{}",
        m.label_connector_overlap
    );
}

#[test]
fn test_a_link_into_its_own_node_through_the_name_counts_at_the_own_link_factor() {
    // An arrow from below into aux #1 passes vertically through #1's Bottom
    // label on the way in. The run inside the inset text box is the box's
    // height, so a foreign line would strike the label fully; the node's own
    // link counts at OWN_LINK_STRIKE_FACTOR. Two labels -> a rate of half that.
    let view = make_view(vec![
        aux(1, "a long label name", 200.0, 0.0),
        aux(2, "source", 200.0, 300.0),
        straight_link(10, 2, 1),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(
        close(m.label_connector_overlap, OWN_LINK_STRIKE_FACTOR / 2.0),
        "{}",
        m.label_connector_overlap
    );
}

#[test]
fn test_link_grazing_label_padding_is_not_charged() {
    // A link just inside the label box's top edge (within LABEL_INSET) passes
    // through padding, not text.
    let target = aux(3, "a long label name", 200.0, -30.0);
    let lbl = label_box(&target);
    let y = lbl.top + LABEL_INSET / 2.0;
    let view = make_view(vec![
        aux(1, "a", 0.0, y),
        aux(2, "b", 400.0, y),
        target,
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.label_connector_overlap, 0.0);
}

// --- crowding: clearance deficits ---

/// Two Right-labeled auxes on one row, `gap` px between the first's label box
/// and the second's shape.
fn crowded_pair(gap: f64) -> datamodel::StockFlow {
    let first = aux_side(1, "name", 0.0, 0.0, LabelSide::Right);
    let lbl = label_box(&first);
    let second_x = lbl.right + gap + AUX_RADIUS;
    make_view(vec![
        first,
        aux_side(2, "name", second_x, 0.0, LabelSide::Right),
    ])
}

#[test]
fn test_crowding_charges_the_squared_clearance_deficit() {
    let gap = 3.0;
    let m = compute_layout_metrics(&crowded_pair(gap), &cfg());
    let deficit = (1.0 - gap / COMFORTABLE_CLEARANCE).powi(2);
    // One crowded pair over two nodes.
    assert!(
        close(m.crowding, deficit / 2.0),
        "{} vs {}",
        m.crowding,
        deficit / 2.0
    );
}

#[test]
fn test_crowding_is_zero_beyond_the_clearance_and_monotone_within_it() {
    assert_eq!(
        compute_layout_metrics(&crowded_pair(COMFORTABLE_CLEARANCE + 1.0), &cfg()).crowding,
        0.0
    );
    let gaps = [0.0, 1.5, 3.0, 5.0, 7.0];
    let costs: Vec<f64> = gaps
        .iter()
        .map(|&g| compute_layout_metrics(&crowded_pair(g), &cfg()).crowding)
        .collect();
    assert!(
        costs.windows(2).all(|w| w[0] > w[1]),
        "crowding must fall as the gap grows: {costs:?}"
    );
}

#[test]
fn test_a_link_too_short_to_show_its_arrow_is_crowding() {
    // Two auxes whose circles are 8px apart: the straight link between them is
    // drawn 8px long, below MIN_VISIBLE_LINK, so crowding includes that link's
    // deficit (one short link over one link) on top of the pair's clearance
    // deficit.
    let view = make_view(vec![
        aux_side(1, "a", 0.0, 0.0, LabelSide::Top),
        aux_side(2, "b", 2.0 * AUX_RADIUS + 8.0, 0.0, LabelSide::Bottom),
        straight_link(10, 1, 2),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    let connectors = collect_connector_geometry(&view.elements);
    let visible = connectors[0].length;
    assert!(
        visible < MIN_VISIBLE_LINK,
        "fixture link must be short: {visible}"
    );
    let far = make_view(vec![
        aux_side(1, "a", 0.0, 0.0, LabelSide::Top),
        aux_side(2, "b", 300.0, 0.0, LabelSide::Bottom),
        straight_link(10, 1, 2),
    ]);
    let m_far = compute_layout_metrics(&far, &cfg());
    assert_eq!(m_far.crowding, 0.0);
    assert!(
        m.crowding >= (1.0 - visible / MIN_VISIBLE_LINK).powi(2) - 1e-9,
        "the short link's deficit must be charged: {}",
        m.crowding
    );
}

/// A stock with a flow leaving its right face, the valve `valve_offset` px
/// past the face; labels on the given sides.
fn stock_with_outflow(
    valve_offset: f64,
    stock_side: LabelSide,
    flow_side: LabelSide,
) -> datamodel::StockFlow {
    let edge = 100.0 + STOCK_WIDTH / 2.0;
    let mut s = stock(1, "stock", 100.0, 100.0);
    if let ViewElement::Stock(st) = &mut s {
        st.label_side = stock_side;
    }
    let mut f = flow_with_points(
        2,
        "f",
        (edge + valve_offset, 100.0),
        vec![(edge, 100.0, Some(1)), (edge + 300.0, 100.0, None)],
    );
    if let ViewElement::Flow(fl) = &mut f {
        fl.label_side = flow_side;
    }
    make_view(vec![s, f])
}

#[test]
fn test_a_flow_is_not_crowded_by_the_stock_its_pipe_attaches_to() {
    // The valve circle sits 16px past the stock face (7px of clearance, less
    // than COMFORTABLE_CLEARANCE): joined by construction, not jammed together.
    // Labels on opposite sides stay clear of each other and of both shapes.
    let m = compute_layout_metrics(
        &stock_with_outflow(16.0, LabelSide::Top, LabelSide::Right),
        &cfg(),
    );
    assert_eq!(m.crowding, 0.0);
}

#[test]
fn test_labels_of_an_attached_flow_and_stock_still_crowd() {
    // The same pair with both labels Bottom: the flow's name runs into the
    // stock's. Attachment excuses the shapes, never the labels.
    let m = compute_layout_metrics(
        &stock_with_outflow(12.0, LabelSide::Bottom, LabelSide::Bottom),
        &cfg(),
    );
    assert!(m.crowding > 0.0);
}

// --- long_connectors ---

#[test]
fn test_one_parameter_across_the_diagram_is_a_long_connector() {
    // Five short links (drawn length 100: centers 118 apart, minus both 9px
    // radii) and one drawn 1182 long. Median 100, threshold 300: the long link
    // exceeds it by 1182/300 - 1. Mean over six links.
    let mut elements = Vec::new();
    for k in 0..5 {
        let base = f64::from(k) * 1000.0;
        elements.push(aux(10 + k, "x", base, 0.0));
        elements.push(aux(20 + k, "y", base + 118.0, 0.0));
        elements.push(straight_link(30 + k, 10 + k, 20 + k));
    }
    elements.push(aux(40, "far", 0.0, 1000.0));
    elements.push(aux(41, "consumer", 1200.0, 1000.0));
    elements.push(straight_link(42, 40, 41));
    let m = compute_layout_metrics(&make_view(elements), &cfg());
    let expected = (1182.0 / 300.0 - 1.0) / 6.0;
    assert!(
        (m.long_connectors - expected).abs() < 1e-6,
        "{} expected {expected}",
        m.long_connectors
    );
}

#[test]
fn test_uniformly_scaled_links_are_not_long() {
    // Every link the same length: nothing stands out, whatever the scale.
    let mut elements = Vec::new();
    for k in 0..4 {
        let base = f64::from(k) * 2000.0;
        elements.push(aux(10 + k, "x", base, 0.0));
        elements.push(aux(20 + k, "y", base + 900.0, 0.0));
        elements.push(straight_link(30 + k, 10 + k, 20 + k));
    }
    let m = compute_layout_metrics(&make_view(elements), &cfg());
    assert_eq!(m.long_connectors, 0.0);
}

// --- misalignment ---

#[test]
fn test_misalignment_counts_nodes_sharing_no_row_or_column() {
    // Three auxes on one row are aligned; a fourth off every row and column
    // within reach is not.
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 100.0, 0.0),
        aux(3, "c", 200.0, 0.0),
        aux(4, "d", 50.0, 70.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(close(m.misalignment, 0.25), "{}", m.misalignment);
}

// --- aspect_penalty ---

#[test]
fn test_aspect_penalty_thin_box_positive() {
    let view = make_view(vec![aux(1, "a", 0.0, 0.0), aux(2, "b", 0.0, 1000.0)]);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(m.aspect_penalty > 0.0);
    let boxes: Vec<Rect> = build_scene_nodes(&view.elements)
        .iter()
        .map(SceneNode::footprint_box)
        .collect();
    let bbox = view_bounding_box(&boxes).unwrap();
    let (w, h) = (common::rect_width(&bbox), common::rect_height(&bbox));
    let expected = (w.max(h) / w.min(h) - TARGET_AR_MAX).max(0.0);
    assert!(close(m.aspect_penalty, expected));
}

#[test]
fn test_aspect_penalty_balanced_box_zero() {
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 400.0, 0.0),
        aux(3, "c", 0.0, 300.0),
        aux(4, "d", 400.0, 300.0),
    ]);
    let m = compute_layout_metrics(&view, &cfg());
    assert_eq!(m.aspect_penalty, 0.0);
}

// --- weighted_cost ---

#[test]
fn test_weighted_cost_exact_linear_combination() {
    let m = LayoutMetrics {
        node_overlap: 1.5,
        node_connector_overlap: 2.0,
        label_overlap: 0.5,
        label_connector_overlap: 0.75,
        crossings: 3.0,
        crowding: 0.3,
        sprawl: 4.0,
        long_connectors: 0.2,
        edge_length_cv: 0.25,
        aspect_penalty: 6.0,
        misalignment: 0.4,
        loop_compactness: 8.0,
        flow_bends: 9.0,
        loop_straightness: 11.0,
    };
    let w = MetricWeights {
        node_overlap: 10.0,
        node_connector_overlap: 20.0,
        label_overlap: 30.0,
        label_connector_overlap: 35.0,
        crossings: 40.0,
        crowding: 45.0,
        sprawl: 50.0,
        long_connectors: 55.0,
        edge_length_cv: 60.0,
        aspect_penalty: 70.0,
        misalignment: 75.0,
        loop_compactness: 90.0,
        flow_bends: 100.0,
        loop_straightness: 110.0,
    };
    let expected: f64 = m
        .terms()
        .iter()
        .zip(w.terms().iter())
        .map(|((name_m, v), (name_w, wt))| {
            assert_eq!(name_m, name_w, "terms() and weights terms() must align");
            v * wt
        })
        .sum();
    assert!(close(m.weighted_cost(&w), expected));
    // And spelled out, so `terms()` itself is checked against the fields.
    let spelled = 1.5 * 10.0
        + 2.0 * 20.0
        + 0.5 * 30.0
        + 0.75 * 35.0
        + 3.0 * 40.0
        + 0.3 * 45.0
        + 4.0 * 50.0
        + 0.2 * 55.0
        + 0.25 * 60.0
        + 6.0 * 70.0
        + 0.4 * 75.0
        + 8.0 * 90.0
        + 9.0 * 100.0
        + 11.0 * 110.0;
    assert!(close(m.weighted_cost(&w), spelled));
}

#[test]
fn test_default_weights_encode_illegibility_dominance() {
    let w = MetricWeights::default();
    // Information-destroying defects outweigh crossings and spacing.
    for illegible in [w.node_overlap, w.label_overlap] {
        assert!(illegible > w.crossings);
        assert!(illegible > w.crowding);
    }
    // Spacing has a finite optimum: both directions carry weight, gently.
    assert!(w.crowding > 0.0 && w.sprawl > 0.0);
    assert!(w.sprawl < w.crowding);
    // Diagnostics carry none.
    assert_eq!(w.edge_length_cv, 0.0);
    assert_eq!(w.aspect_penalty, 0.0);
    // Conventions are nudges below every defect weight.
    for convention in [
        w.loop_compactness,
        w.flow_bends,
        w.loop_straightness,
        w.misalignment,
    ] {
        assert!(convention > 0.0 && convention < w.crossings);
    }
}

// --- degenerate views ---

fn assert_all_finite(m: &LayoutMetrics) {
    for (name, v) in m.terms() {
        assert!(v.is_finite(), "{name} is not finite: {v}");
    }
}

#[test]
fn test_empty_view_all_zero_finite() {
    let m = compute_layout_metrics(&make_view(vec![]), &cfg());
    assert_all_finite(&m);
    for (name, v) in m.terms() {
        assert_eq!(v, 0.0, "{name}");
    }
}

#[test]
fn test_single_element_view_all_zero_finite() {
    let m = compute_layout_metrics(&make_view(vec![aux(1, "only", 100.0, 100.0)]), &cfg());
    assert_all_finite(&m);
    for (name, v) in m.terms() {
        assert_eq!(v, 0.0, "{name}");
    }
}

// --- scale behavior ---

#[test]
fn test_scale_invariance_of_scale_free_terms() {
    // Crossings interior to both connectors are exactly scale-invariant; the
    // fraction of connector length under a fixed-size shape DROPS as the view
    // is scaled up (the shape does not scale, the connector does).
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 400.0, 0.0),
        stock(3, "s", 200.0, 0.0),
        aux(4, "c", 0.0, 300.0),
        stock(5, "t", 400.0, 320.0),
        straight_link(10, 1, 2),
        straight_link(11, 4, 5),
    ]);
    let base = compute_layout_metrics(&view, &cfg());
    assert_eq!(base.node_overlap, 0.0);
    assert!(base.node_connector_overlap > 0.0);
    let scaled = compute_layout_metrics(&scale_view(&view, 3.0), &cfg());
    assert!(close(scaled.crossings, base.crossings));
    assert!(scaled.node_connector_overlap < base.node_connector_overlap);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Every term is a function of the view's geometry, not the order its
    /// elements are listed in.
    #[test]
    fn prop_metrics_shuffle_invariant(
        xs in prop::collection::vec(-80.0f64..80.0, 4),
        ys in prop::collection::vec(-80.0f64..80.0, 4),
        perm in prop::sample::subsequence(vec![0usize, 1, 2, 3], 4),
    ) {
        let elems: Vec<ViewElement> = (0..4)
            .map(|i| if i % 2 == 0 {
                stock(i as i32 + 1, "n", xs[i], ys[i])
            } else {
                aux(i as i32 + 1, "an aux", xs[i], ys[i])
            })
            .collect();
        let base = compute_layout_metrics(&make_view(elems.clone()), &cfg());
        let shuffled: Vec<ViewElement> = perm.iter().map(|&i| elems[i].clone()).collect();
        let other = compute_layout_metrics(&make_view(shuffled), &cfg());
        for ((name, a), (_, b)) in base.terms().iter().zip(other.terms().iter()) {
            prop_assert!((a - b).abs() < 1e-9, "{} changed under shuffle: {} vs {}", name, a, b);
        }
    }
}

// --- analyze_layout: defects agree with the score ---

#[test]
fn test_analyze_layout_metrics_equal_compute_layout_metrics() {
    let view = make_view(vec![
        aux(1, "samename", 0.0, 0.0),
        aux(2, "samename", 40.0, 0.0),
        aux(3, "c", 0.0, 300.0),
        stock(4, "t", 400.0, 320.0),
        straight_link(10, 1, 4),
        straight_link(11, 3, 2),
    ]);
    let analysis = analyze_layout(&view);
    assert_eq!(analysis.metrics, compute_layout_metrics(&view, &cfg()));
}

#[test]
fn test_analyze_layout_locates_a_crossing_at_the_intersection() {
    // Two links crossing at the center of the square their endpoints form.
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 200.0, 200.0),
        aux(3, "c", 200.0, 0.0),
        aux(4, "d", 0.0, 200.0),
        straight_link(10, 1, 2),
        straight_link(11, 3, 4),
    ]);
    let analysis = analyze_layout(&view);
    let crossings: Vec<&Defect> = analysis
        .defects
        .iter()
        .filter(|d| d.kind == DefectKind::Crossing)
        .collect();
    assert_eq!(crossings.len(), 1);
    let [l, t, r, b] = crossings[0].region;
    assert!((l - 100.0).abs() < 1e-6 && (t - 100.0).abs() < 1e-6);
    assert!(close(l, r) && close(t, b), "a crossing is a point");
    assert!(
        close(analysis.metrics.crossings, 0.5),
        "one crossing over two connectors"
    );
}

#[test]
fn test_analyze_layout_reports_each_obscured_label_with_its_fraction() {
    let view = make_view(vec![
        aux(1, "samename", 0.0, 0.0),
        aux(2, "samename", 40.0, 0.0),
    ]);
    let analysis = analyze_layout(&view);
    let obscured: Vec<&Defect> = analysis
        .defects
        .iter()
        .filter(|d| d.kind == DefectKind::LabelObscured)
        .collect();
    assert_eq!(obscured.len(), 2);
    for d in obscured {
        assert!(close(d.severity, 252.0 / 812.0));
    }
}

// --- loop_compactness (isoperimetric loop quality) ---

fn shape_center(e: &ViewElement) -> Point {
    let r = node_shape_box(e).unwrap();
    Point {
        x: (r.left + r.right) / 2.0,
        y: (r.top + r.bottom) / 2.0,
    }
}

/// Hand-computed isoperimetric penalty `1 - Q` for a polygon over `centers`.
fn expected_loop_penalty(centers: &[Point]) -> f64 {
    let n = centers.len();
    let mut area2 = 0.0;
    let mut perim = 0.0;
    for i in 0..n {
        let a = centers[i];
        let b = centers[(i + 1) % n];
        area2 += a.x * b.y - b.x * a.y;
        perim += ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
    }
    let area = area2.abs() / 2.0;
    1.0 - (4.0 * std::f64::consts::PI * area / (perim * perim)).clamp(0.0, 1.0)
}

fn cycle_view(positions: &[(f64, f64)]) -> (datamodel::StockFlow, Vec<Point>) {
    let n = positions.len() as i32;
    let mut elements: Vec<ViewElement> = Vec::new();
    let mut centers = Vec::new();
    for (i, &(x, y)) in positions.iter().enumerate() {
        let e = stock(i as i32 + 1, "n", x, y);
        centers.push(shape_center(&e));
        elements.push(e);
    }
    for i in 0..n {
        elements.push(straight_link(100 + i, i + 1, (i + 1) % n + 1));
    }
    (make_view(elements), centers)
}

#[test]
fn test_loop_compactness_circle_loop_near_zero() {
    let positions: Vec<(f64, f64)> = (0..8)
        .map(|i| {
            let theta = 2.0 * std::f64::consts::PI * f64::from(i) / 8.0;
            (300.0 * theta.cos(), 300.0 * theta.sin())
        })
        .collect();
    let (view, centers) = cycle_view(&positions);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(close(m.loop_compactness, expected_loop_penalty(&centers)));
    assert!(m.loop_compactness < 0.1);
}

#[test]
fn test_loop_compactness_collapsed_loop_higher() {
    let positions: Vec<(f64, f64)> = (0..8)
        .map(|i| (f64::from(i) * 100.0, if i % 2 == 0 { 0.0 } else { 1.0 }))
        .collect();
    let (view, centers) = cycle_view(&positions);
    let m = compute_layout_metrics(&view, &cfg());
    assert!(close(m.loop_compactness, expected_loop_penalty(&centers)));
    assert!(m.loop_compactness > 0.9);
}

#[test]
fn test_loop_compactness_no_cycle_is_zero() {
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 200.0, 0.0),
        aux(3, "c", 400.0, 0.0),
        straight_link(10, 1, 2),
        straight_link(11, 2, 3),
    ]);
    assert_eq!(compute_layout_metrics(&view, &cfg()).loop_compactness, 0.0);
}

#[test]
fn test_loop_straightness_straight_loop_is_high() {
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 300.0, 0.0),
        aux(3, "c", 300.0, 300.0),
        aux(4, "d", 0.0, 300.0),
        straight_link(11, 1, 2),
        straight_link(12, 2, 3),
        straight_link(13, 3, 4),
        straight_link(14, 4, 1),
    ]);
    assert!(compute_layout_metrics(&view, &cfg()).loop_straightness > 0.9);
}

#[test]
fn test_loop_straightness_curved_loop_is_low() {
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 300.0, 0.0),
        aux(3, "c", 300.0, 300.0),
        aux(4, "d", 0.0, 300.0),
        arc_link(11, 1, 2, 45.0),
        arc_link(12, 2, 3, 45.0),
        arc_link(13, 3, 4, 45.0),
        arc_link(14, 4, 1, 45.0),
    ]);
    assert!(compute_layout_metrics(&view, &cfg()).loop_straightness < 0.5);
}

#[test]
fn test_loop_straightness_no_loop_is_zero() {
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 200.0, 0.0),
        aux(3, "c", 400.0, 0.0),
        straight_link(10, 1, 2),
        straight_link(11, 2, 3),
    ]);
    assert_eq!(compute_layout_metrics(&view, &cfg()).loop_straightness, 0.0);
}

#[test]
fn test_loop_compactness_two_node_mutual_pair_is_zero() {
    let view = make_view(vec![
        aux(1, "a", 0.0, 0.0),
        aux(2, "b", 200.0, 0.0),
        straight_link(10, 1, 2),
        straight_link(11, 2, 1),
    ]);
    assert_eq!(compute_layout_metrics(&view, &cfg()).loop_compactness, 0.0);
}

#[test]
fn test_loop_compactness_flow_feedback_path_is_a_cycle() {
    let view = make_view(vec![
        stock(1, "a", 0.0, 0.0),
        stock(2, "b", 300.0, 0.0),
        flow_between(3, "f", 150.0, 200.0, 1, 2),
        straight_link(10, 2, 1),
    ]);
    assert!(compute_layout_metrics(&view, &cfg()).loop_compactness > 0.0);
}

fn bent_flow_loop_view(valve: Point, bend: Point) -> datamodel::StockFlow {
    make_view(vec![
        stock(1, "a", 0.0, 0.0),
        stock(2, "b", 300.0, 0.0),
        flow_with_points(
            3,
            "f",
            (valve.x, valve.y),
            vec![
                (0.0, 0.0, Some(1)),
                (bend.x, bend.y, None),
                (300.0, 0.0, Some(2)),
            ],
        ),
        straight_link(10, 2, 1),
    ])
}

#[test]
fn test_loop_compactness_scored_on_flow_valve_not_pipe_extent() {
    // The loop vertex for a flow is its valve: stretching the pipe with a far
    // interior point must not change the loop polygon; moving the valve must.
    let valve = Point { x: 150.0, y: 200.0 };
    let near = compute_layout_metrics(
        &bent_flow_loop_view(valve, Point { x: 150.0, y: 210.0 }),
        &cfg(),
    );
    let far = compute_layout_metrics(
        &bent_flow_loop_view(
            valve,
            Point {
                x: 150.0,
                y: 2000.0,
            },
        ),
        &cfg(),
    );
    assert!(near.loop_compactness > 0.0);
    assert!((near.loop_compactness - far.loop_compactness).abs() < 1e-12);
    let moved = compute_layout_metrics(
        &bent_flow_loop_view(Point { x: 150.0, y: 400.0 }, Point { x: 150.0, y: 210.0 }),
        &cfg(),
    );
    assert!((near.loop_compactness - moved.loop_compactness).abs() > 1e-9);
}

// --- metric validity: visibly worse diagrams must cost more ---
//
// The shipped default projects are hand-drawn, clean diagrams. Every
// degradation in the taste battery is an edit a modeler would call a
// regression; the committed metric must penalize each one on each of these
// exemplars, or an optimizer driving the metric is free to produce that very
// defect. `StraightenLinks` is excluded: flattening a non-loop connector is a
// matter of style, and only loops care (`loop_straightness`, a gentle nudge).
//
// The eval harness runs the same battery over the whole corpus, references
// and generated layouts alike; this test pins the exemplars a regression can
// never be allowed to reach.

/// A shipped default project's hand-drawn main view.
fn default_project_view(dir: &str) -> datamodel::StockFlow {
    let path = format!(
        "{}/../../default_projects/{}/model.xmile",
        env!("CARGO_MANIFEST_DIR"),
        dir
    );
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    let project = crate::compat::open_xmile(&mut std::io::BufReader::new(file))
        .unwrap_or_else(|e| panic!("parse {path}: {e:?}"));
    match project.get_model("main").and_then(|m| m.views.first()) {
        Some(datamodel::View::StockFlow(sf)) => sf.clone(),
        _ => panic!("{dir} ships no main view"),
    }
}

#[test]
fn test_metric_penalizes_every_degradation_of_the_exemplars() {
    let weights = MetricWeights::default();
    let mut misses = Vec::new();
    for dir in ["logistic-growth", "population", "fishbanks", "reliability"] {
        let view = default_project_view(dir);
        let base_metrics = compute_layout_metrics(&view, &cfg());
        let base = base_metrics.weighted_cost(&weights);
        for degradation in Degradation::battery() {
            if degradation == Degradation::StraightenLinks {
                continue;
            }
            let Some(degraded) = degrade(&view, degradation) else {
                continue;
            };
            let metrics = compute_layout_metrics(&degraded, &cfg());
            let cost = metrics.weighted_cost(&weights);
            if cost <= base * 1.01 {
                // Name the weighted terms that moved, so a miss is diagnosable.
                let moved: Vec<String> = metrics
                    .terms()
                    .iter()
                    .zip(base_metrics.terms().iter())
                    .zip(weights.terms().iter())
                    .filter(|((_, _), (_, w))| *w > 0.0)
                    .filter(|(((_, after), (_, before)), _)| (after - before).abs() > 1e-6)
                    .map(|(((name, after), (_, before)), (_, w))| {
                        format!("{name} {:+.4}", (after - before) * w)
                    })
                    .collect();
                misses.push(format!(
                    "{dir}/{}: {base:.4} -> {cost:.4} [{}]",
                    degradation.name(),
                    moved.join(", ")
                ));
            }
        }
    }
    assert!(
        misses.is_empty(),
        "the metric failed to penalize visibly worse diagrams:\n  {}",
        misses.join("\n  ")
    );
}

// --- human-vs-auto reference pairs under the committed weights ---
//
// On the shipped default projects, the hand-authored ("human") layout must
// score a lower cost than a fixed-seed generated ("auto") layout of the same
// model: if the metric preferred the generator's output over these exemplars,
// the metric -- not the exemplar -- would be wrong.

const REF_PAIR_SEED: u64 = 42;

fn human_cost(dir: &str) -> f64 {
    compute_layout_metrics(&default_project_view(dir), &cfg())
        .weighted_cost(&MetricWeights::default())
}

fn auto_cost(dir: &str) -> f64 {
    let path = format!(
        "{}/../../default_projects/{}/model.xmile",
        env!("CARGO_MANIFEST_DIR"),
        dir
    );
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    let project = crate::compat::open_xmile(&mut std::io::BufReader::new(file))
        .unwrap_or_else(|e| panic!("parse {path}: {e:?}"));
    let config = LayoutConfig {
        annealing_random_seed: REF_PAIR_SEED,
        ..LayoutConfig::default()
    };
    let view = crate::layout::generate_layout_with_config(&project, "main", config.clone(), None)
        .expect("auto layout generation must succeed for the anchor model");
    compute_layout_metrics(&view, &config).weighted_cost(&MetricWeights::default())
}

fn assert_human_beats_auto(dir: &str) {
    let human = human_cost(dir);
    let auto = auto_cost(dir);
    assert!(
        human < auto,
        "reference pair {dir}: expected human_cost ({human}) < auto_cost ({auto})"
    );
}

#[test]
fn test_reference_pair_reliability_human_beats_auto() {
    assert_human_beats_auto("reliability");
}

#[test]
fn test_reference_pair_fishbanks_human_beats_auto() {
    assert_human_beats_auto("fishbanks");
}

#[test]
fn test_reference_pair_population_human_beats_auto() {
    assert_human_beats_auto("population");
}

#[test]
fn test_reference_pair_logistic_growth_human_beats_auto() {
    assert_human_beats_auto("logistic-growth");
}
