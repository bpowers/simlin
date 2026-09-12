// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The MDL importer's flow geometry, read through `open_vensim` on corpus
//! files. A Vensim sketch anchors a pipe's endpoints to the centers of the
//! elements it connects and draws stocks at whatever size the modeler chose,
//! while every Simlin renderer draws a stock as a 45x35 box; the imported
//! geometry must satisfy `diagram::flow_geometry`'s invariants on that box.

use crate::compat::open_vensim;
use crate::datamodel::{self, ViewElement, view_element};
use crate::diagram::flow_geometry::flow_invariant_violations;

fn main_view(project: &datamodel::Project) -> &datamodel::StockFlow {
    let datamodel::View::StockFlow(sf) = &project.models[0].views[0];
    sf
}

fn flow<'a>(view: &'a datamodel::StockFlow, name: &str) -> &'a view_element::Flow {
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if f.name == name => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("flow {name:?} not in view"))
}

fn coords(f: &view_element::Flow) -> Vec<(f64, f64)> {
    f.points.iter().map(|p| (p.x, p.y)).collect()
}

fn violations_for(view: &datamodel::StockFlow, names: &[&str]) -> Vec<String> {
    flow_invariant_violations(&view.elements)
        .into_iter()
        .filter(|v| names.iter().any(|n| v.starts_with(&format!("{n}:"))))
        .collect()
}

fn cloud_at(view: &datamodel::StockFlow, uid: i32) -> (f64, f64) {
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Cloud(c) if c.uid == uid => Some((c.x, c.y)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("cloud {uid} not in view"))
}

/// `mark2.mdl`: Vensim's `risk taking behavior` box is 90x65, and the pipe
/// into it runs 19 below its center -- past the 45x35 box's bottom face but
/// within reach of the side face it approaches. The pipe slides up into the
/// right face's clearance span, carrying its valve and its cloud.
#[test]
fn a_pipe_just_past_a_big_stocks_face_slides_onto_the_45x35_face() {
    const MARK2: &str = include_str!("../../../../../test/bobby/vdf/econ/mark2.mdl");
    let project = open_vensim(MARK2).expect("mark2 imports");
    let view = main_view(&project);
    let f = flow(view, "change_in_risk_taking_behavior");
    assert_eq!(coords(f), vec![(1087.0, 1478.5), (887.5, 1478.5)]);
    assert_eq!((f.x, f.y), (1012.0, 1478.5));
    assert_eq!(
        cloud_at(view, f.points[0].attached_to_uid.unwrap()),
        (1087.0, 1478.5)
    );
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new()
    );
}

/// `scirev7.mdl`: a pipe whose line passes far from its stock keeps its line
/// and gains the perpendicular leg the 45x35 box needs. `PSR`'s pipe drops
/// from `PUA` and ends 571 left of `SP`'s center on `SP`'s center row, so a
/// horizontal leg runs into `SP`'s left face; `ARESR`'s pipe passes 210 above
/// `SP`'s center, so a vertical leg drops into its top face.
#[test]
fn a_pipe_far_from_its_stock_gains_a_leg_into_a_face() {
    const SCIREV7: &str =
        include_str!("../../../../../test/metasd/scientific-revolution/scirev7.mdl");
    let project = open_vensim(SCIREV7).expect("scirev7 imports");
    let view = main_view(&project);
    assert_eq!(
        coords(flow(view, "PSR")),
        vec![(621.0, 227.5), (621.0, 420.0), (1169.5, 420.0)]
    );
    assert_eq!(
        coords(flow(view, "ARESR")),
        vec![(989.5, 210.0), (1192.0, 210.0), (1192.0, 402.5)]
    );
    assert_eq!(
        violations_for(view, &["PSR", "ARESR"]),
        Vec::<String>::new()
    );
}

/// `Covid19US v8.mdl`: `Fatalities` leaves `Infected sympto` 134 above its
/// center (a leg into the top face) and enters `Dead from COVID` on a valid
/// off-center slot, which stays exactly where it is.
#[test]
fn a_leg_at_one_end_leaves_a_valid_slot_at_the_other_untouched() {
    const COVID: &str =
        include_str!("../../../../../test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl");
    let project = open_vensim(COVID).expect("Covid19US v8 imports");
    let view = main_view(&project);
    assert_eq!(
        coords(flow(view, "Fatalities")),
        vec![(949.0, 481.5), (949.0, 365.0), (1155.5, 365.0)]
    );
    assert_eq!(violations_for(view, &["Fatalities"]), Vec::<String>::new());
}

/// Endpoints on a top or bottom face within 3px of a corner slide into the
/// clearance span: `flow7` (0.5 from the corner, the other end a cloud) and
/// `Flow_k63` (2.5 from the corner, the other end a valid slot 4.5 from its
/// own corner, which gives up the least it can -- 0.5 -- and stays valid).
#[test]
fn corner_zone_endpoints_slide_into_the_clearance_span() {
    const ZEROLED: &str = include_str!(
        "../../../../../test/test-models/tests/zeroled_decimals/test_zeroled_decimals.mdl"
    );
    let project = open_vensim(ZEROLED).expect("zeroled_decimals imports");
    let view = main_view(&project);
    let f = flow(view, "flow7");
    assert_eq!(coords(f), vec![(800.5, 308.5), (800.5, 405.0)]);
    assert_eq!((f.x, f.y), (800.5, 354.0));
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new()
    );

    const THYROID: &str =
        include_str!("../../../../../test/metasd/thyroid-dynamics/thyroid-2008-d.mdl");
    let project = open_vensim(THYROID).expect("thyroid imports");
    let view = main_view(&project);
    assert_eq!(
        coords(flow(view, "Flow_k63")),
        vec![(305.5, 612.5), (305.5, 927.5)]
    );
    // Flow_k31 and its siblings run 50-66 above or below both stocks'
    // centers: legs at both ends.
    assert_eq!(
        coords(flow(view, "Flow_k31")),
        vec![
            (610.0, 577.5),
            (610.0, 540.0),
            (325.0, 540.0),
            (325.0, 577.5)
        ]
    );
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new()
    );
}

/// `integration3.mdl`: a Vensim cloud comment sits a few pixels off the
/// pipe's end; the cloud moves onto the endpoint.
#[test]
fn a_cloud_comment_off_the_pipe_end_is_centered_on_it() {
    const INTEGRATION3: &str =
        include_str!("../../../../../test/metasd/bathtub-statistics/integration3.mdl");
    let project = open_vensim(INTEGRATION3).expect("integration3 imports");
    let view = main_view(&project);
    let f = flow(view, "Noise");
    let source = &f.points[0];
    assert_eq!(
        cloud_at(view, source.attached_to_uid.unwrap()),
        (source.x, source.y)
    );
    assert_eq!(violations_for(view, &["Noise"]), Vec::<String>::new());
}
