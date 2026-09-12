// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The flow invariants (`diagram::flow_geometry`) on the geometry the layout
//! produces: where endpoints land on stock faces, what the finishing pass
//! settles, and what a fresh layout hands back. Mounted from `layout_tests.rs`,
//! whose helpers it reuses.

use super::*;
use crate::diagram::flow_geometry::flow_invariant_violations;

/// `resnap_flow_endpoints` keeps an endpoint's position along the face it
/// snaps to, but never within `CORNER_CLEARANCE` of a corner, on either
/// approach: a pipe drawn into a corner reads as attached to two faces.
#[test]
fn test_resnap_keeps_corner_clearance_on_both_approaches() {
    let config = LayoutConfig::default();
    let half_w = config.stock_width / 2.0;
    let half_h = config.stock_height / 2.0;
    let clearance = crate::diagram::flow_geometry::CORNER_CLEARANCE;
    // (valve, endpoint before resnap, expected endpoint): the preserved
    // coordinate lies beyond the stock's span on each row.
    let rows = [
        (
            "horizontal approach",
            (400.0, 100.0),
            (222.5, 140.0),
            (200.0 + half_w, 100.0 + half_h - clearance),
        ),
        (
            "vertical approach",
            (200.0, 400.0),
            (160.0, 117.5),
            (200.0 - half_w + clearance, 100.0 + half_h),
        ),
    ];
    for (label, valve, before, expected) in rows {
        let model = simple_model();
        let mut state = LayoutState::new(&model);
        state.elements.push(ViewElement::Stock(view_element::Stock {
            name: "stock_a".into(),
            uid: 1,
            x: 200.0,
            y: 100.0,
            label_side: LabelSide::Bottom,
            compat: None,
        }));
        state.elements.push(ViewElement::Flow(view_element::Flow {
            name: "my_flow".into(),
            uid: 2,
            x: valve.0,
            y: valve.1,
            label_side: LabelSide::Bottom,
            points: vec![
                FlowPoint {
                    x: before.0,
                    y: before.1,
                    attached_to_uid: Some(1),
                },
                FlowPoint {
                    x: valve.0,
                    y: valve.1,
                    attached_to_uid: None,
                },
            ],
            compat: None,
            label_compat: None,
        }));
        resnap_flow_endpoints(&mut state, &config, |_| true);
        let ViewElement::Flow(f) = &state.elements[1] else {
            unreachable!()
        };
        assert!(
            (f.points[0].x - expected.0).abs() < 1e-9 && (f.points[0].y - expected.1).abs() < 1e-9,
            "{label}: endpoint ({}, {}) expected {expected:?}",
            f.points[0].x,
            f.points[0].y
        );
    }
}

/// `finish_flow_geometry` on a pipe the orthogonalizer alone leaves invalid:
/// the valve and the cloud sit on a row beyond the stock's face span, so the
/// endpoint was clamped into the span and the pipe leaves the stock
/// diagonally. The orthogonalizer routes an `L` between the two attached ends
/// that passes through neither the valve nor its row; the valve has to be
/// brought onto the route.
///
/// The input is `generate_layout`'s own geometry for `dr` in
/// `test/metasd/scientific-revolution/scirev7.mdl` as it reaches the
/// finishing pass, translated so the stock sits at (100, 100) and rounded to
/// the half pixel. That model's placement alone takes over three seconds on a
/// debug build, past the per-test budget, so the capture stands in for it; the
/// corpus measurement covers the whole model.
#[test]
fn test_finish_flow_geometry_brings_a_valve_off_the_route_onto_it() {
    let mut elements = vec![
        ViewElement::Stock(view_element::Stock {
            name: "p".into(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: LabelSide::Bottom,
            compat: None,
        }),
        ViewElement::Flow(view_element::Flow {
            name: "dr".into(),
            uid: 2,
            x: 450.0,
            y: 128.0,
            label_side: LabelSide::Bottom,
            points: vec![
                FlowPoint {
                    x: 122.5,
                    y: 114.5,
                    attached_to_uid: Some(1),
                },
                FlowPoint {
                    x: 691.5,
                    y: 128.0,
                    attached_to_uid: Some(3),
                },
            ],
            compat: None,
            label_compat: None,
        }),
        ViewElement::Cloud(view_element::Cloud {
            uid: 3,
            flow_uid: 2,
            x: 691.5,
            y: 128.0,
            compat: None,
        }),
    ];
    finish_flow_geometry(&mut elements, |_| true);
    assert_eq!(flow_invariant_violations(&elements), Vec::<String>::new());
}

/// `finish_flow_geometry` on a pipe whose orthogonal route leaves a stub
/// under `MIN_SEGMENT`: `aeei install_rate` in
/// `test/metasd/FREE/FREE6/FREE6-original/free 6.mdl`, as `generate_best_layout`
/// hands it to the finishing pass (seed 456, the seed that wins), captured
/// unrounded. The cloud sits on a row 2.16px above the sink's clamped face
/// slot, so the orthogonalizer's L rises 2.16px at the cloud before running
/// into the stock; the stub collapses and the pipe runs straight. That model's
/// placement takes tens of seconds on a debug build, so the capture stands in
/// for it; the corpus measurement covers the whole model.
#[test]
fn test_finish_flow_geometry_collapses_a_stub_under_the_minimum() {
    let (cloud_x, cloud_y) = (1208.8755575407895, 1768.5582441059819);
    let sink = (1536.2478075407898, 1770.7188274393143);
    let mut elements = vec![
        ViewElement::Stock(view_element::Stock {
            name: "embodied\\naeei".into(),
            uid: 15,
            x: 1558.7478075407898,
            y: 1785.2188274393143,
            label_side: LabelSide::Bottom,
            compat: None,
        }),
        ViewElement::Flow(view_element::Flow {
            name: "aeei\\ninstall_rate".into(),
            uid: 16,
            x: 1351.6805575407893,
            y: cloud_y,
            label_side: LabelSide::Top,
            points: vec![
                FlowPoint {
                    x: cloud_x,
                    y: cloud_y,
                    attached_to_uid: Some(17),
                },
                FlowPoint {
                    x: sink.0,
                    y: sink.1,
                    attached_to_uid: Some(15),
                },
            ],
            compat: None,
            label_compat: None,
        }),
        ViewElement::Cloud(view_element::Cloud {
            uid: 17,
            flow_uid: 16,
            x: cloud_x,
            y: cloud_y,
            compat: None,
        }),
    ];
    finish_flow_geometry(&mut elements, |_| true);
    assert_eq!(flow_invariant_violations(&elements), Vec::<String>::new());
    let ViewElement::Flow(f) = &elements[1] else {
        unreachable!()
    };
    assert_eq!(
        f.points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
        vec![(cloud_x, sink.1), sink]
    );
}

/// Engine auto-layout holds the flow invariants end to end on a model whose
/// layout puts stocks off each other's rows and columns: `leaky_conveyor.xmile`,
/// whose leak flows ended on stock corners. This row pins the corner arm of
/// endpoint placement. The finishing pass's valve arm is pinned by
/// `test_finish_flow_geometry_brings_a_valve_off_the_route_onto_it`, the
/// resnap clearance by `test_resnap_keeps_corner_clearance_on_both_approaches`,
/// and the normalization's own arms by `diagram::flow_geometry_tests`; the
/// larger models that reach them through a whole layout (scirev7, thyroid,
/// C-LEARN) are measured corpus-wide rather than run here, to keep this test
/// within the time budget.
#[test]
fn test_fresh_layout_holds_the_flow_invariants() {
    const LEAKY: &str = include_str!("../../../../test/conveyors/leaky_conveyor.xmile");
    let project = crate::compat::open_xmile(&mut std::io::BufReader::new(LEAKY.as_bytes()))
        .expect("leaky_conveyor imports");
    let model_name = project.models[0].name.clone();
    let view = generate_layout(&project, &model_name, None).expect("layout");
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new()
    );
}
