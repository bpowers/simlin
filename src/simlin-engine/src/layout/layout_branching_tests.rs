// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for chain layout of BRANCHING stock-flow topologies: a stock whose
//! flows connect it to two or more other stocks (compartment models, SIR-style
//! splits). The chain BFS lays stocks out along a horizontal line; without a
//! position-collision check every branch lands on the same spot, stacking
//! stocks exactly on top of each other (the thyroid corpus regression: 6 stocks
//! collapsed onto 2 positions).

use super::*;
use crate::datamodel;

/// Name used for test models, matching `project.get_model(TEST_MODEL)`.
const TEST_MODEL: &str = "test";

fn test_project(model: datamodel::Model) -> datamodel::Project {
    datamodel::Project {
        name: model.name.clone(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: Vec::new(),
        units: Vec::new(),
        models: vec![model],
        source: None,
        ai_information: None,
    }
}

fn make_stock(ident: &str, inflows: &[&str], outflows: &[&str]) -> datamodel::Variable {
    datamodel::Variable::Stock(datamodel::Stock {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar("100".to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|s| s.to_string()).collect(),
        outflows: outflows.iter().map(|s| s.to_string()).collect(),
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
        ai_state: None,
        uid: None,
    })
}

fn make_flow(ident: &str) -> datamodel::Variable {
    datamodel::Variable::Flow(datamodel::Flow {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar("1".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
        ai_state: None,
        uid: None,
    })
}

fn make_model(variables: Vec<datamodel::Variable>) -> datamodel::Model {
    datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables,
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    }
}

/// Every pair of stocks must have non-overlapping shape boxes. A generated
/// layout that places two stocks on (or nearly on) the same spot is broken: the
/// declutter cannot separate them (stocks are not movable there) and the
/// annealing cannot either (stocks are not perturbable), so stacking is
/// permanent.
fn assert_no_stock_overlap(view: &datamodel::StockFlow) {
    use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};
    let stocks: Vec<(&str, f64, f64)> = view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.name.as_str(), s.x, s.y)),
            _ => None,
        })
        .collect();
    for i in 0..stocks.len() {
        for j in (i + 1)..stocks.len() {
            let dx = (stocks[i].1 - stocks[j].1).abs();
            let dy = (stocks[i].2 - stocks[j].2).abs();
            assert!(
                dx >= STOCK_WIDTH || dy >= STOCK_HEIGHT,
                "stocks '{}' ({}, {}) and '{}' ({}, {}) overlap",
                stocks[i].0,
                stocks[i].1,
                stocks[i].2,
                stocks[j].0,
                stocks[j].1,
                stocks[j].2,
            );
        }
    }
}

/// Every stock-attached flow endpoint must lie ON that stock's boundary
/// rectangle (within a small tolerance). Mirrors the corpus-level tripwire test
/// in `tests/integration/layout.rs`; repeated here so the unit tests catch a detached
/// branch flow without needing the corpus models.
fn assert_flow_endpoints_attached(view: &datamodel::StockFlow) {
    use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};
    const TOLERANCE: f64 = 1.0;

    let stock_pos: HashMap<i32, (f64, f64)> = view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y))),
            _ => None,
        })
        .collect();

    for elem in &view.elements {
        let ViewElement::Flow(f) = elem else { continue };
        for pt in &f.points {
            let Some(uid) = pt.attached_to_uid else {
                continue;
            };
            let Some(&(sx, sy)) = stock_pos.get(&uid) else {
                continue; // attached to a cloud or non-stock
            };
            let half_w = STOCK_WIDTH / 2.0;
            let half_h = STOCK_HEIGHT / 2.0;
            let (left, right) = (sx - half_w, sx + half_w);
            let (top, bottom) = (sy - half_h, sy + half_h);
            let on_vertical_edge = ((pt.x - left).abs() <= TOLERANCE
                || (pt.x - right).abs() <= TOLERANCE)
                && pt.y >= top - TOLERANCE
                && pt.y <= bottom + TOLERANCE;
            let on_horizontal_edge = ((pt.y - top).abs() <= TOLERANCE
                || (pt.y - bottom).abs() <= TOLERANCE)
                && pt.x >= left - TOLERANCE
                && pt.x <= right + TOLERANCE;
            assert!(
                on_vertical_edge || on_horizontal_edge,
                "flow '{}' endpoint ({}, {}) is not on the boundary of its stock at ({}, {})",
                f.name,
                pt.x,
                pt.y,
                sx,
                sy,
            );
        }
    }
}

/// One source stock with two outflows feeding two DIFFERENT sink stocks: the
/// SIR-style split (infected -> recovered, infected -> dead). Both sinks must
/// get distinct positions.
#[test]
fn test_branching_outflows_stocks_not_stacked() {
    let model = make_model(vec![
        make_stock("source", &[], &["to_a", "to_b"]),
        make_stock("branch_a", &["to_a"], &[]),
        make_stock("branch_b", &["to_b"], &[]),
        make_flow("to_a"),
        make_flow("to_b"),
    ]);
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    assert_no_stock_overlap(&result);
    assert_flow_endpoints_attached(&result);
}

/// Two source stocks both feeding one sink stock (the merge mirror of the
/// branch case). Both sources must get distinct positions.
#[test]
fn test_merging_inflows_stocks_not_stacked() {
    let model = make_model(vec![
        make_stock("source_a", &[], &["from_a"]),
        make_stock("source_b", &[], &["from_b"]),
        make_stock("sink", &["from_a", "from_b"], &[]),
        make_flow("from_a"),
        make_flow("from_b"),
    ]);
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    assert_no_stock_overlap(&result);
    assert_flow_endpoints_attached(&result);
}

/// Thyroid-style compartment fan: one central stock exchanging with three
/// peripheral stocks. The central stock has flows to/from every peripheral
/// stock; without collision-aware placement all three peripherals stack.
#[test]
fn test_compartment_fan_stocks_not_stacked() {
    let model = make_model(vec![
        make_stock(
            "plasma",
            &["into_plasma_a", "into_plasma_b"],
            &["to_a", "to_b", "to_c"],
        ),
        make_stock("fast", &["to_a"], &["into_plasma_a"]),
        make_stock("slow", &["to_b"], &["into_plasma_b"]),
        make_stock("excreted", &["to_c"], &[]),
        make_flow("to_a"),
        make_flow("to_b"),
        make_flow("to_c"),
        make_flow("into_plasma_a"),
        make_flow("into_plasma_b"),
    ]);
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    assert_no_stock_overlap(&result);
    assert_flow_endpoints_attached(&result);
}

/// Two flows in opposite directions between the same stock pair (compartment
/// exchange, like thyroid's plasma <-> fast flows): both valves would land on
/// the exact midpoint of the two stocks, stacking the valve circles and their
/// labels on top of each other. They must instead draw as parallel pipes with
/// visually distinct valves.
#[test]
fn test_bidirectional_flows_get_distinct_valves() {
    let model = make_model(vec![
        make_stock("compartment_a", &["b_to_a"], &["a_to_b"]),
        make_stock("compartment_b", &["a_to_b"], &["b_to_a"]),
        make_flow("a_to_b"),
        make_flow("b_to_a"),
    ]);
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let valves: Vec<(String, f64, f64)> = result
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Flow(f) => Some((f.name.clone(), f.x, f.y)),
            _ => None,
        })
        .collect();
    assert_eq!(valves.len(), 2, "both flows must have view elements");
    let d = ((valves[0].1 - valves[1].1).powi(2) + (valves[0].2 - valves[1].2).powi(2)).sqrt();
    assert!(
        d >= 20.0,
        "bidirectional flow valves must be visually distinct (>= 20 units apart), \
         got {d:.1}: '{}' at ({}, {}) vs '{}' at ({}, {})",
        valves[0].0,
        valves[0].1,
        valves[0].2,
        valves[1].0,
        valves[1].1,
        valves[1].2,
    );
    // The parallel pipes must still be attached to both stocks.
    assert_flow_endpoints_attached(&result);
    assert_no_stock_overlap(&result);
}

/// Layouts of branching models must remain deterministic per seed (#633).
#[test]
fn test_branching_layout_deterministic() {
    let model = make_model(vec![
        make_stock("source", &[], &["to_a", "to_b"]),
        make_stock("branch_a", &["to_a"], &[]),
        make_stock("branch_b", &["to_b"], &[]),
        make_flow("to_a"),
        make_flow("to_b"),
    ]);
    let project = test_project(model);

    let a = generate_layout(&project, TEST_MODEL, None).unwrap();
    let b = generate_layout(&project, TEST_MODEL, None).unwrap();
    assert_eq!(a, b, "branching layout must be deterministic per seed");
}
