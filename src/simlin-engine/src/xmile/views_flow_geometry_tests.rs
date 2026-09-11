// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The XMILE importer's flow geometry, read through the production reader
//! (`project_from_reader`) on corpus files and on small documents shaped the
//! way producers write them. The invariants are `diagram::flow_geometry`'s.

use std::io::BufReader;

use crate::datamodel::{self, ViewElement, view_element};
use crate::diagram::flow_geometry::flow_invariant_violations;
use crate::xmile::project_from_reader;

fn import(xml: &str) -> datamodel::Project {
    project_from_reader(&mut BufReader::new(xml.as_bytes())).expect("must parse")
}

fn main_view(project: &datamodel::Project) -> &datamodel::StockFlow {
    let model = project
        .models
        .iter()
        .find(|m| m.name == "main")
        .expect("main model");
    let datamodel::View::StockFlow(sf) = &model.views[0];
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

fn stock<'a>(view: &'a datamodel::StockFlow, name: &str) -> &'a view_element::Stock {
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if s.name == name => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| panic!("stock {name:?} not in view"))
}

fn cloud(view: &datamodel::StockFlow, uid: i32) -> &view_element::Cloud {
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Cloud(c) if c.uid == uid => Some(c),
            _ => None,
        })
        .unwrap_or_else(|| panic!("cloud {uid} not in view"))
}

/// Violations restricted to the named flows, so a corpus file's unrelated
/// flows do not decide a test about specific ones.
fn violations_for(view: &datamodel::StockFlow, names: &[&str]) -> Vec<String> {
    flow_invariant_violations(&view.elements)
        .into_iter()
        .filter(|v| names.iter().any(|n| v.starts_with(&format!("{n}:"))))
        .collect()
}

fn document(view_body: &str, variables: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>isee systems, inc.</vendor><product version="1">Stella</product></header>
  <sim_specs><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
  <model>
    <variables>{variables}</variables>
    <views><view type="stock_flow">{view_body}</view></views>
  </model>
</xmile>"#
    )
}

/// A stock written with `width`/`height` carries its TOP-LEFT corner in
/// `x`/`y` (the importer converts it to a center). A flow drawn into that
/// stock's left edge must import onto the 45x35 box's left face at the
/// stock's CENTER, not relative to the top-left corner.
#[test]
fn sized_stock_endpoint_lands_on_the_centered_face() {
    // presumed_infected: top-left (450, 1011.07), 45x35 -> center (472.5, 1028.57).
    const COVID: &str = include_str!("../../../../test/conveyors/covid19_severity.stmx");
    let project = import(COVID);
    let view = main_view(&project);

    let infected = stock(view, "presumed\\ninfected");
    let presumed = flow(view, "presumed\\nnew infections");
    let sink = presumed.points.last().unwrap();
    assert_eq!(sink.attached_to_uid, Some(infected.uid));
    assert!(
        (sink.x - (infected.x - 22.5)).abs() < 1e-9,
        "sink x {} must be on the left face {}",
        sink.x,
        infected.x - 22.5
    );

    // Uninfected at risk: 93.75x65 in Stella, drawn 45x35 here; the pipe
    // leaves its right edge, which is on the right face of the drawn box.
    let at_risk = stock(view, "Uninfected\\nat risk");
    let by_severity = flow(view, "new infections\\nby severity");
    let source = &by_severity.points[0];
    assert_eq!(source.attached_to_uid, Some(at_risk.uid));
    assert!(
        (source.x - (at_risk.x + 22.5)).abs() < 1e-9,
        "source x {} must be on the right face {}",
        source.x,
        at_risk.x + 22.5
    );

    assert_eq!(
        violations_for(
            view,
            &["presumed\\nnew infections", "new infections\\nby severity"]
        ),
        Vec::<String>::new()
    );
}

/// Both takeoff directions and both axes for a sized stock (top-left `x`/`y`):
/// a flow leaving each of the four faces imports onto that face of the
/// centered box. Pins every arm of the face choice, not just the covid one.
#[test]
fn sized_stock_endpoints_on_every_face() {
    // Stock top-left (100, 100), 45x35 -> center (122.5, 117.5).
    let view_body = r#"
      <stock name="s" x="100" y="100" width="45" height="35"/>
      <flow name="right_out" x="200" y="117.5"><pts><pt x="145" y="117.5"/><pt x="260" y="117.5"/></pts></flow>
      <flow name="left_in" x="40" y="117.5"><pts><pt x="-20" y="117.5"/><pt x="100" y="117.5"/></pts></flow>
      <flow name="top_in" x="122.5" y="40"><pts><pt x="122.5" y="-20"/><pt x="122.5" y="100"/></pts></flow>
      <flow name="bottom_out" x="122.5" y="200"><pts><pt x="122.5" y="135"/><pt x="122.5" y="260"/></pts></flow>
    "#;
    let variables = r#"
      <stock name="s"><eqn>1</eqn><inflow>left_in</inflow><inflow>top_in</inflow><outflow>right_out</outflow><outflow>bottom_out</outflow></stock>
      <flow name="right_out"><eqn>1</eqn></flow>
      <flow name="left_in"><eqn>1</eqn></flow>
      <flow name="top_in"><eqn>1</eqn></flow>
      <flow name="bottom_out"><eqn>1</eqn></flow>
    "#;
    let project = import(&document(view_body, variables));
    let view = main_view(&project);
    let s = stock(view, "s");
    assert_eq!((s.x, s.y), (122.5, 117.5));

    let expect = [
        ("right_out", 0, (145.0, 117.5)),
        ("left_in", 1, (100.0, 117.5)),
        ("top_in", 1, (122.5, 100.0)),
        ("bottom_out", 0, (122.5, 135.0)),
    ];
    for (name, end, (x, y)) in expect {
        let p = &flow(view, name).points[end];
        assert_eq!(p.attached_to_uid, Some(s.uid), "{name}");
        assert!(
            (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9,
            "{name}: endpoint ({}, {}) expected ({x}, {y})",
            p.x,
            p.y
        );
    }
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new()
    );
}

/// A flow already satisfying every invariant -- including a deliberately
/// off-center slot on a face -- imports byte for byte.
#[test]
fn valid_geometry_including_off_center_slots_is_not_moved() {
    let view_body = r#"
      <stock name="a" x="100" y="100"/>
      <stock name="b" x="300" y="115"/>
      <flow name="ab" x="200" y="110"><pts><pt x="122.5" y="110"/><pt x="277.5" y="110"/></pts></flow>
      <flow name="drain" x="65" y="160"><pts><pt x="90" y="117.5"/><pt x="90" y="160"/><pt x="40" y="160"/></pts></flow>
    "#;
    let variables = r#"
      <stock name="a"><eqn>1</eqn><outflow>ab</outflow><outflow>drain</outflow></stock>
      <stock name="b"><eqn>1</eqn><inflow>ab</inflow></stock>
      <flow name="ab"><eqn>1</eqn></flow>
      <flow name="drain"><eqn>1</eqn></flow>
    "#;
    let project = import(&document(view_body, variables));
    let view = main_view(&project);
    let ab: Vec<(f64, f64)> = flow(view, "ab").points.iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(ab, vec![(122.5, 110.0), (277.5, 110.0)]);
    let drain = flow(view, "drain");
    let pts: Vec<(f64, f64)> = drain.points.iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(pts, vec![(90.0, 117.5), (90.0, 160.0), (40.0, 160.0)]);
    assert_eq!(
        (drain.x, drain.y),
        (65.0, 160.0),
        "valve already on the pipe"
    );
    let ab = flow(view, "ab");
    assert_eq!((ab.x, ab.y), (200.0, 110.0), "valve already on the pipe");
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new()
    );
}

/// `land_model.stmx` (Stella): corner and off-stock endpoints are brought
/// onto faces, and `forest to agriculture`'s valid off-center source slot
/// stays exactly where Stella put it while its corner sink jogs 3px into the
/// sink's clearance span.
#[test]
fn land_model_corner_endpoints_are_fixed_without_moving_valid_slots() {
    const LAND: &str = include_str!("../../../../test/land_model/land_model.stmx");
    let project = import(LAND);
    let view = main_view(&project);
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new()
    );

    let f = flow(view, "forest to agriculture");
    let pts: Vec<(f64, f64)> = f.points.iter().map(|p| (p.x, p.y)).collect();
    assert_eq!(
        pts,
        vec![
            (618.5, 2055.5),
            (817.5, 2055.5),
            (817.5, 2052.5),
            (827.5, 2052.5)
        ]
    );
    assert_eq!((f.x, f.y), (723.0, 2055.5));
}

/// A cloud is created at a 2-point flow's raw endpoint, and the importer then
/// straightens the pipe (xmutil writes pipes a pixel or two off axis). The
/// cloud must end up centered on the STRAIGHTENED endpoint, for both the
/// source and the sink cloud, horizontal and vertical.
#[test]
fn clouds_are_centered_on_straightened_endpoints() {
    let view_body = r#"
      <stock name="s" x="300" y="100"/>
      <flow name="h_in" x="200" y="100"><pts><pt x="120" y="93"/><pt x="277.5" y="107"/></pts></flow>
      <flow name="v_out" x="300" y="200"><pts><pt x="296" y="117.5"/><pt x="304" y="280"/></pts></flow>
    "#;
    let variables = r#"
      <stock name="s"><eqn>1</eqn><inflow>h_in</inflow><outflow>v_out</outflow></stock>
      <flow name="h_in"><eqn>1</eqn></flow>
      <flow name="v_out"><eqn>1</eqn></flow>
    "#;
    let project = import(&document(view_body, variables));
    let view = main_view(&project);
    for (name, cloud_end) in [("h_in", 0usize), ("v_out", 1usize)] {
        let f = flow(view, name);
        let p = &f.points[cloud_end];
        let c = cloud(view, p.attached_to_uid.expect("cloud end attached"));
        assert_eq!(c.flow_uid, f.uid, "{name}");
        assert_eq!((c.x, c.y), (p.x, p.y), "{name}: cloud off its endpoint");
    }
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new()
    );
}

/// The corpus flows the cloud defect was measured on: xmutil-converted
/// models whose raw pipes are slightly diagonal.
#[test]
fn xmutil_corpus_clouds_sit_on_their_endpoints() {
    const ABS: &str = include_str!("../../../../test/test-models/tests/abs/test_abs.xmile");
    const CHAINED: &str = include_str!(
        "../../../../test/test-models/tests/chained_initialization/test_chained_initialization.xmile"
    );
    for (label, text) in [("test_abs", ABS), ("chained_initialization", CHAINED)] {
        let project = import(text);
        let view = main_view(&project);
        let problems: Vec<String> = flow_invariant_violations(&view.elements)
            .into_iter()
            .filter(|v| v.contains("cloud"))
            .collect();
        assert_eq!(problems, Vec::<String>::new(), "{label}");
    }
}
