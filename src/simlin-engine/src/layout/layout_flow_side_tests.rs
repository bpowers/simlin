// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Which stock face each flow attaches to (`classify_flow_sides`) and the
//! geometry fresh layouts draw from it.

use super::*;
use crate::datamodel;

// ---------------------------------------------------------------------------
// classify_flow_sides tests
// ---------------------------------------------------------------------------

/// Helper: build a ComputedMetadata with the given stock/flow topology.
fn metadata_with_flows(
    stock_outflows: &[(&str, &[&str])],
    stock_inflows: &[(&str, &[&str])],
    flow_to_stocks: &[(&str, Option<&str>, Option<&str>)],
) -> ComputedMetadata {
    let mut meta = ComputedMetadata::new_empty();
    for &(stock, outflows) in stock_outflows {
        meta.stock_to_outflows.insert(
            stock.to_string(),
            outflows.iter().map(|s| s.to_string()).collect(),
        );
    }
    for &(stock, inflows) in stock_inflows {
        meta.stock_to_inflows.insert(
            stock.to_string(),
            inflows.iter().map(|s| s.to_string()).collect(),
        );
    }
    for &(flow, from, to) in flow_to_stocks {
        meta.flow_to_stocks.insert(
            flow.to_string(),
            (from.map(|s| s.to_string()), to.map(|s| s.to_string())),
        );
    }
    meta
}

#[test]
fn test_classify_sides_single_outflow_no_chain() {
    // Stock with one outflow to cloud (no chain flow) -> stays Right
    let meta = metadata_with_flows(&[("a", &["f1"])], &[], &[("f1", Some("a"), None)]);
    let sides = classify_flow_sides("a", &meta, &HashMap::new());
    let att = sides.get("f1").expect("f1 should have attachment");
    assert_eq!(att.side, StockAttachSide::Right);
    assert!((att.offset - 0.5).abs() < f64::EPSILON);
}

#[test]
fn test_classify_sides_chain_plus_side_outflow() {
    // Stock with chain outflow (a->b) + side outflow (a->cloud)
    let meta = metadata_with_flows(
        &[("a", &["chain_flow", "waste_flow"])],
        &[],
        &[
            ("chain_flow", Some("a"), Some("b")),
            ("waste_flow", Some("a"), None),
        ],
    );
    let sides = classify_flow_sides("a", &meta, &HashMap::new());

    let chain = sides.get("chain_flow").expect("chain_flow attachment");
    assert_eq!(chain.side, StockAttachSide::Right);
    assert!((chain.offset - 0.5).abs() < f64::EPSILON);

    let waste = sides.get("waste_flow").expect("waste_flow attachment");
    assert_eq!(waste.side, StockAttachSide::Bottom);
    assert!((waste.offset - 0.5).abs() < f64::EPSILON);
}

#[test]
fn test_classify_sides_chain_plus_two_side_outflows() {
    // Chain + two side outflows: the chain holds the right face, so the side
    // outflows each take a face of their own -- below, then above -- rather
    // than stacking their valves on one face.
    let meta = metadata_with_flows(
        &[("a", &["chain_flow", "waste_a", "waste_b"])],
        &[],
        &[
            ("chain_flow", Some("a"), Some("b")),
            ("waste_a", Some("a"), None),
            ("waste_b", Some("a"), None),
        ],
    );
    let sides = classify_flow_sides("a", &meta, &HashMap::new());

    let chain = sides.get("chain_flow").expect("chain_flow");
    assert_eq!(chain.side, StockAttachSide::Right);

    // waste_a and waste_b sorted alphabetically -> waste_a first
    let wa = sides.get("waste_a").expect("waste_a");
    let wb = sides.get("waste_b").expect("waste_b");
    assert_eq!(wa.side, StockAttachSide::Bottom);
    assert_eq!(wb.side, StockAttachSide::Top);
    assert!((wa.offset - 0.5).abs() < 1e-10);
    assert!((wb.offset - 0.5).abs() < 1e-10);
}

#[test]
fn test_classify_sides_chain_inflow_plus_side_inflow() {
    // Stock with chain inflow (from stock) + side inflow (from cloud)
    let meta = metadata_with_flows(
        &[],
        &[("b", &["chain_in", "side_in"])],
        &[
            ("chain_in", Some("a"), Some("b")),
            ("side_in", None, Some("b")),
        ],
    );
    let sides = classify_flow_sides("b", &meta, &HashMap::new());

    let chain = sides.get("chain_in").expect("chain_in");
    assert_eq!(chain.side, StockAttachSide::Left);
    assert!((chain.offset - 0.5).abs() < f64::EPSILON);

    let side = sides.get("side_in").expect("side_in");
    assert_eq!(side.side, StockAttachSide::Top);
    assert!((side.offset - 0.5).abs() < f64::EPSILON);
}

#[test]
fn test_classify_sides_only_nonchain_outflows() {
    // Three side outflows spread over the three outflow faces in preference
    // order (sorted by ident): right, bottom, top.
    let meta = metadata_with_flows(
        &[("a", &["f1", "f2", "f3"])],
        &[],
        &[
            ("f1", Some("a"), None),
            ("f2", Some("a"), None),
            ("f3", Some("a"), None),
        ],
    );
    let sides = classify_flow_sides("a", &meta, &HashMap::new());

    assert_eq!(sides["f1"].side, StockAttachSide::Right);
    assert_eq!(sides["f2"].side, StockAttachSide::Bottom);
    assert_eq!(sides["f3"].side, StockAttachSide::Top);
    for name in ["f1", "f2", "f3"] {
        assert!((sides[name].offset - 0.5).abs() < 1e-10, "{name}");
    }
}

#[test]
fn test_classify_sides_multiple_chain_outflows() {
    // Stock with two chain outflows (feeds two stocks) -> both Right, 1/3 and 2/3
    let meta = metadata_with_flows(
        &[("a", &["f1", "f2"])],
        &[],
        &[("f1", Some("a"), Some("b")), ("f2", Some("a"), Some("c"))],
    );
    let sides = classify_flow_sides("a", &meta, &HashMap::new());

    let a1 = sides.get("f1").expect("f1");
    let a2 = sides.get("f2").expect("f2");
    assert_eq!(a1.side, StockAttachSide::Right);
    assert_eq!(a2.side, StockAttachSide::Right);
    assert!((a1.offset - 1.0 / 3.0).abs() < 1e-10);
    assert!((a2.offset - 2.0 / 3.0).abs() < 1e-10);
}

#[test]
fn test_classify_sides_existing_faces_are_kept() {
    // One row per arm of how an already-drawn side flow is seated. Stock `a`;
    // `chain` (when present) runs a -> b and holds the right face.
    type Row = (
        &'static str,
        bool,
        &'static [&'static str],
        &'static [(&'static str, StockAttachSide)],
        &'static [(&'static str, StockAttachSide)],
    );
    let rows: &[Row] = &[
        (
            "a flow drawn on a face a chain now holds is re-placed like a new one",
            true,
            &["w"],
            &[("w", StockAttachSide::Right)],
            &[("w", StockAttachSide::Bottom)],
        ),
        (
            "flows on their faces stay; a new flow takes a free face, even when it sorts first",
            false,
            &["w0", "w1", "w2"],
            &[
                ("w1", StockAttachSide::Right),
                ("w2", StockAttachSide::Bottom),
            ],
            &[
                ("w0", StockAttachSide::Top),
                ("w1", StockAttachSide::Right),
                ("w2", StockAttachSide::Bottom),
            ],
        ),
        (
            "a flow off its preferred face returns once that face comes free",
            false,
            &["w"],
            &[("w", StockAttachSide::Bottom)],
            &[("w", StockAttachSide::Right)],
        ),
        (
            "a flow off its preferred face, with that face held, stays even on a shared face",
            true,
            &["w1", "w2"],
            &[
                ("w1", StockAttachSide::Bottom),
                ("w2", StockAttachSide::Bottom),
            ],
            &[
                ("w1", StockAttachSide::Bottom),
                ("w2", StockAttachSide::Bottom),
            ],
        ),
        (
            "a hand-placed flow on a face outside its direction's preferences stays",
            true,
            &["w"],
            &[("w", StockAttachSide::Left)],
            &[("w", StockAttachSide::Left)],
        ),
    ];
    for &(label, with_chain, side_outflows, existing, expected) in rows {
        let mut outflows: Vec<&str> = side_outflows.to_vec();
        let mut flow_stocks: Vec<(&str, Option<&str>, Option<&str>)> = side_outflows
            .iter()
            .map(|f| (*f, Some("a"), None))
            .collect();
        if with_chain {
            outflows.push("chain");
            flow_stocks.push(("chain", Some("a"), Some("b")));
        }
        let meta = metadata_with_flows(&[("a", &outflows)], &[], &flow_stocks);
        let existing: HashMap<String, StockAttachSide> = existing
            .iter()
            .map(|(f, side)| (f.to_string(), *side))
            .collect();
        let sides = classify_flow_sides("a", &meta, &existing);
        for &(flow, side) in expected {
            assert_eq!(sides[flow].side, side, "{label}: {flow}");
        }
    }
}

// ---------------------------------------------------------------------------
// Layout behavior tests for perpendicular side flows
// ---------------------------------------------------------------------------

/// Build a model with stock A -> chain_flow -> stock B, plus stock A -> waste_flow -> cloud.
fn chain_with_waste_model() -> datamodel::Model {
    datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["chain_flow".to_string(), "waste_flow".to_string()],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "b".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["chain_flow".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "chain_flow".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "waste_flow".to_string(),
                equation: datamodel::Equation::Scalar("5".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    }
}

#[test]
fn test_layout_side_flow_below_stock() {
    let project = test_project(chain_with_waste_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let stock_a = find_stock(&result, "a").expect("stock a should exist");
    let chain_flow = find_flow(&result, "chain_flow").expect("chain_flow should exist");
    let waste_flow = find_flow(&result, "waste_flow").expect("waste_flow should exist");

    // Chain flow should be horizontal (same y as stock)
    assert!(
        (chain_flow.y - stock_a.y).abs() < 1.0,
        "chain_flow y ({}) should be near stock a y ({})",
        chain_flow.y,
        stock_a.y,
    );

    // Waste flow should be below stock
    assert!(
        waste_flow.y > stock_a.y + 10.0,
        "waste_flow y ({}) should be well below stock a y ({})",
        waste_flow.y,
        stock_a.y,
    );

    // Waste flow should have vertical flow points (same x, different y)
    assert!(
        waste_flow.points.len() >= 2,
        "waste_flow should have at least 2 points"
    );
    let first = &waste_flow.points[0];
    let last = &waste_flow.points[waste_flow.points.len() - 1];
    assert!(
        (first.x - last.x).abs() < 1.0,
        "waste_flow points should be vertically aligned: first.x={}, last.x={}",
        first.x,
        last.x,
    );
    assert!(
        (first.y - last.y).abs() > 10.0,
        "waste_flow points should have vertical separation: first.y={}, last.y={}",
        first.y,
        last.y,
    );
}

#[test]
fn test_layout_side_flows_no_overlap() {
    let project = test_project(chain_with_waste_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let chain_flow = find_flow(&result, "chain_flow").expect("chain_flow");
    let waste_flow = find_flow(&result, "waste_flow").expect("waste_flow");

    // Flows must not overlap: either x or y must differ significantly
    let dist =
        ((chain_flow.x - waste_flow.x).powi(2) + (chain_flow.y - waste_flow.y).powi(2)).sqrt();
    assert!(
        dist > 5.0,
        "chain_flow ({}, {}) and waste_flow ({}, {}) should not overlap (dist={})",
        chain_flow.x,
        chain_flow.y,
        waste_flow.x,
        waste_flow.y,
        dist,
    );
}

/// Model with chain + 2 waste flows to test spacing.
fn chain_with_two_waste_model() -> datamodel::Model {
    datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![
                    "chain_flow".to_string(),
                    "waste_a".to_string(),
                    "waste_b".to_string(),
                ],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "b".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["chain_flow".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "chain_flow".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "waste_a".to_string(),
                equation: datamodel::Equation::Scalar("3".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "waste_b".to_string(),
                equation: datamodel::Equation::Scalar("2".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    }
}

#[test]
fn test_layout_multiple_side_flows_spaced() {
    let project = test_project(chain_with_two_waste_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let waste_a = find_flow(&result, "waste_a").expect("waste_a");
    let waste_b = find_flow(&result, "waste_b").expect("waste_b");
    let stock_a = find_stock(&result, "a").expect("stock a");

    // The chain holds the right face, so one waste flow drops out of the
    // bottom face and the other out of the top face.
    assert!(waste_a.y > stock_a.y, "waste_a should be below stock a");
    assert!(waste_b.y < stock_a.y, "waste_b should be above stock a");

    let valve_gap = ((waste_a.x - waste_b.x).powi(2) + (waste_a.y - waste_b.y).powi(2)).sqrt();
    assert!(
        valve_gap > 2.0 * crate::diagram::constants::AUX_RADIUS,
        "waste_a ({}, {}) and waste_b ({}, {}) valves should not overlap",
        waste_a.x,
        waste_a.y,
        waste_b.x,
        waste_b.y,
    );
}

#[test]
fn test_layout_side_flows_take_separate_faces() {
    // Every combination of a chain outflow, a chain inflow, and up to three
    // side outflows and two side inflows on stock `a`. Whenever there are
    // enough free faces for each side flow to have its own -- outflows use
    // right/bottom/top, inflows left/top/bottom, a chain's face is never
    // shared -- the side flows' valves must not overlap. And no side flow
    // ever attaches to a face a chain flow holds.
    let config = LayoutConfig::default();
    for with_chain_out in [false, true] {
        for with_chain_in in [false, true] {
            for n_out in 0..=3 {
                for n_in in 0..=2 {
                    let side_out: Vec<String> = (0..n_out).map(|i| format!("out_{i}")).collect();
                    let side_in: Vec<String> = (0..n_in).map(|i| format!("in_{i}")).collect();
                    let mut a_out: Vec<&str> = side_out.iter().map(String::as_str).collect();
                    let mut a_in: Vec<&str> = side_in.iter().map(String::as_str).collect();
                    let mut vars = Vec::new();
                    if with_chain_out {
                        a_out.push("chain_out");
                        vars.push(stock_var("b", &["chain_out"], &[]));
                        vars.push(flow_var("chain_out"));
                    }
                    if with_chain_in {
                        a_in.push("chain_in");
                        vars.push(stock_var("c", &[], &["chain_in"]));
                        vars.push(flow_var("chain_in"));
                    }
                    vars.push(stock_var("a", &a_in, &a_out));
                    for f in side_out.iter().chain(&side_in) {
                        vars.push(flow_var(f));
                    }
                    let model = datamodel::Model {
                        name: TEST_MODEL.to_string(),
                        sim_specs: None,
                        variables: vars,
                        views: Vec::new(),
                        loop_metadata: Vec::new(),
                        groups: Vec::new(),
                        macro_spec: None,
                    };
                    let row = format!(
                        "chain_out={with_chain_out} chain_in={with_chain_in} \
                         side_out={n_out} side_in={n_in}"
                    );
                    let view = generate_layout(&test_project(model), TEST_MODEL, None)
                        .unwrap_or_else(|e| panic!("{row}: {e:?}"));
                    let stock = find_stock(&view, "a").expect("stock a");

                    let face_of = |flow: &view_element::Flow| {
                        let pt = flow
                            .points
                            .iter()
                            .find(|pt| pt.attached_to_uid == Some(stock.uid))
                            .expect("side flow attached to a");
                        let (dx, dy) = (pt.x - stock.x, pt.y - stock.y);
                        if (dx - config.stock_width / 2.0).abs() < 0.5 {
                            StockAttachSide::Right
                        } else if (dx + config.stock_width / 2.0).abs() < 0.5 {
                            StockAttachSide::Left
                        } else if dy > 0.0 {
                            StockAttachSide::Bottom
                        } else {
                            StockAttachSide::Top
                        }
                    };
                    let side_flows: Vec<&view_element::Flow> = side_out
                        .iter()
                        .chain(&side_in)
                        .map(|f| find_flow(&view, f).expect("side flow"))
                        .collect();
                    for flow in &side_flows {
                        let face = face_of(flow);
                        assert!(
                            !(with_chain_out && face == StockAttachSide::Right)
                                && !(with_chain_in && face == StockAttachSide::Left),
                            "{row}: {} sits on a chain face ({face:?})",
                            flow.name
                        );
                    }

                    let free_faces = 4 - usize::from(with_chain_out) - usize::from(with_chain_in);
                    let out_faces = 3 - usize::from(with_chain_out);
                    let in_faces = 3 - usize::from(with_chain_in);
                    if n_out > out_faces || n_in > in_faces || n_out + n_in > free_faces {
                        continue;
                    }
                    for (i, f1) in side_flows.iter().enumerate() {
                        for f2 in &side_flows[i + 1..] {
                            let gap = ((f1.x - f2.x).powi(2) + (f1.y - f2.y).powi(2)).sqrt();
                            assert!(
                                gap > 2.0 * crate::diagram::constants::AUX_RADIUS,
                                "{row}: valves of {} and {} overlap (gap {gap})",
                                f1.name,
                                f2.name
                            );
                        }
                    }
                }
            }
        }
    }
}

fn stock_var(ident: &str, inflows: &[&str], outflows: &[&str]) -> datamodel::Variable {
    datamodel::Variable::Stock(datamodel::Stock {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar("100".to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|s| s.to_string()).collect(),
        outflows: outflows.iter().map(|s| s.to_string()).collect(),
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

fn flow_var(ident: &str) -> datamodel::Variable {
    datamodel::Variable::Flow(datamodel::Flow {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar("1".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

#[test]
fn test_layout_single_outflow_still_horizontal() {
    // Stock with only one outflow to cloud (no chain) -> should go right
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["f1".to_string()],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "f1".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    };
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let stock_a = find_stock(&result, "a").expect("stock a");
    let flow = find_flow(&result, "f1").expect("flow f1");

    // Flow should be at same y as stock (horizontal)
    assert!(
        (flow.y - stock_a.y).abs() < 1.0,
        "single outflow should be horizontal: flow.y={}, stock.y={}",
        flow.y,
        stock_a.y,
    );

    // Flow should be to the right of the stock
    assert!(
        flow.x > stock_a.x,
        "single outflow should be to the right: flow.x={}, stock.x={}",
        flow.x,
        stock_a.x,
    );
}
