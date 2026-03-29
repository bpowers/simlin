// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;

fn test_project(model: datamodel::Model) -> datamodel::Project {
    let name = model.name.clone();
    datamodel::Project {
        name: name.clone(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: Vec::new(),
        units: Vec::new(),
        models: vec![model],
        source: None,
        ai_information: None,
    }
}

/// Name used for test models -- matches the name in `simple_model()` and
/// inline test models so that `project.get_model(TEST_MODEL)` finds them.
const TEST_MODEL: &str = "test";

fn simple_model() -> datamodel::Model {
    datamodel::Model {
        name: "test".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["births".to_string()],
                outflows: vec!["deaths".to_string()],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "births".to_string(),
                equation: datamodel::Equation::Scalar("population * birth_rate".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "deaths".to_string(),
                equation: datamodel::Equation::Scalar("population * death_rate".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(3),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "birth_rate".to_string(),
                equation: datamodel::Equation::Scalar("0.03".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(4),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "death_rate".to_string(),
                equation: datamodel::Equation::Scalar("0.01".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(5),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    }
}

#[test]
fn test_generate_layout_empty() {
    let project = test_project(datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: Vec::new(),
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    });
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();
    assert!(result.elements.is_empty());
    assert_eq!(result.zoom, 1.0);
}

#[test]
fn test_generate_layout_single_chain() {
    let project = test_project(simple_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    assert!(!result.elements.is_empty());
    assert_eq!(result.zoom, 1.0);

    // Should have stocks, flows, auxes, clouds, and links
    let stock_count = result
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Stock(_)))
        .count();
    let flow_count = result
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Flow(_)))
        .count();
    let aux_count = result
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Aux(_)))
        .count();

    assert_eq!(stock_count, 1); // population
    assert_eq!(flow_count, 2); // births, deaths
    assert_eq!(aux_count, 2); // birth_rate, death_rate
}

#[test]
fn test_generate_layout_completeness() {
    let project = test_project(simple_model());
    let model = project.get_model(TEST_MODEL).unwrap();
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    // Every model variable should have a view element
    let element_names: HashSet<String> = result
        .elements
        .iter()
        .filter_map(|e| e.get_name().map(|n| canonicalize(n).into_owned()))
        .collect();

    for var in &model.variables {
        let ident = canonicalize(var.get_ident()).into_owned();
        assert!(
            element_names.contains(&ident),
            "missing view element for {}",
            ident
        );
    }
}

#[test]
fn test_coordinates_positive() {
    let project = test_project(simple_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    for elem in &result.elements {
        match elem {
            ViewElement::Stock(s) => {
                assert!(s.x >= 0.0, "stock {} has negative x: {}", s.name, s.x);
                assert!(s.y >= 0.0, "stock {} has negative y: {}", s.name, s.y);
            }
            ViewElement::Flow(f) => {
                assert!(f.x >= 0.0, "flow {} has negative x: {}", f.name, f.x);
                assert!(f.y >= 0.0, "flow {} has negative y: {}", f.name, f.y);
            }
            ViewElement::Aux(a) => {
                assert!(a.x >= 0.0, "aux {} has negative x: {}", a.name, a.x);
                assert!(a.y >= 0.0, "aux {} has negative y: {}", a.name, a.y);
            }
            ViewElement::Cloud(c) => {
                assert!(c.x >= 0.0, "cloud {} has negative x: {}", c.uid, c.x);
                assert!(c.y >= 0.0, "cloud {} has negative y: {}", c.uid, c.y);
            }
            _ => {}
        }
    }
}

#[test]
fn test_no_duplicate_uids() {
    let project = test_project(simple_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let mut uids: HashSet<i32> = HashSet::new();
    for elem in &result.elements {
        let uid = elem.get_uid();
        assert!(uids.insert(uid), "duplicate UID: {}", uid);
    }
}

#[test]
fn test_viewbox_encompasses_elements() {
    let project = test_project(simple_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let vb = &result.view_box;
    assert!(vb.width > 0.0);
    assert!(vb.height > 0.0);

    for elem in &result.elements {
        match elem {
            ViewElement::Stock(s) => {
                assert!(
                    s.x <= vb.x + vb.width,
                    "stock x {} exceeds viewbox width {}",
                    s.x,
                    vb.width
                );
                assert!(
                    s.y <= vb.y + vb.height,
                    "stock y {} exceeds viewbox height {}",
                    s.y,
                    vb.height
                );
            }
            ViewElement::Flow(f) => {
                assert!(f.x <= vb.x + vb.width);
                assert!(f.y <= vb.y + vb.height);
            }
            ViewElement::Aux(a) => {
                assert!(a.x <= vb.x + vb.width);
                assert!(a.y <= vb.y + vb.height);
            }
            _ => {}
        }
    }
}

#[test]
fn test_zoom_default() {
    let project = test_project(simple_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();
    assert_eq!(result.zoom, 1.0);
}

#[test]
fn test_flow_points_attached() {
    let project = test_project(simple_model());
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    for elem in &result.elements {
        if let ViewElement::Flow(flow) = elem {
            assert!(
                flow.points.len() >= 2,
                "flow {} has too few points",
                flow.name
            );
            // At least one endpoint should be attached
            let has_attachment = flow.points.iter().any(|p| p.attached_to_uid.is_some());
            assert!(
                has_attachment,
                "flow {} has no attached endpoints",
                flow.name
            );
        }
    }
}

#[test]
fn test_compute_metadata_chains() {
    let project = test_project(simple_model());
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    // Should detect one chain: population + births + deaths
    assert_eq!(metadata.chains.len(), 1);
    assert_eq!(metadata.chains[0].stocks.len(), 1);
    assert!(
        metadata.chains[0]
            .stocks
            .contains(&"population".to_string())
    );
    assert_eq!(metadata.chains[0].flows.len(), 2);
}

#[test]
fn test_compute_metadata_dep_graph() {
    let project = test_project(simple_model());
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    // births depends on population and birth_rate
    let births_deps = metadata.dep_graph.get("births").unwrap();
    assert!(births_deps.contains("population"));
    assert!(births_deps.contains("birth_rate"));
}

#[test]
fn test_compute_metadata_constants() {
    let project = test_project(simple_model());
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    // birth_rate and death_rate are constants (scalar equations with no variable references)
    assert!(metadata.is_constant("birth_rate"));
    assert!(metadata.is_constant("death_rate"));
}

#[test]
fn test_detect_chains_multiple() {
    let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut all_flows: BTreeSet<String> = BTreeSet::new();

    // Chain 1: A -> f1 -> B
    stock_to_inflows.insert("b".into(), vec!["f1".into()]);
    stock_to_outflows.insert("a".into(), vec!["f1".into()]);
    flow_to_stocks.insert("f1".into(), (Some("a".into()), Some("b".into())));
    all_flows.insert("f1".into());

    // Chain 2: C (isolated stock)
    stock_to_inflows.insert("c".into(), vec![]);
    stock_to_outflows.insert("c".into(), vec![]);

    let chains = detect_chains(
        &stock_to_inflows,
        &stock_to_outflows,
        &flow_to_stocks,
        &all_flows,
    );
    assert_eq!(chains.len(), 2);
}

#[test]
fn test_is_structural_stock_flow_matches_dep_graph_direction() {
    // dep_graph stores stock -> flow (stock depends on its inflows/outflows).
    // is_structural_stock_flow(from=stock, to=flow) should return true.
    let stock_inflows: HashMap<String, HashSet<String>> =
        HashMap::from([("population".into(), HashSet::from(["births".into()]))]);
    let stock_outflows: HashMap<String, HashSet<String>> =
        HashMap::from([("population".into(), HashSet::from(["deaths".into()]))]);

    assert!(is_structural_stock_flow(
        "population",
        "births",
        &stock_inflows,
        &stock_outflows,
    ));
    assert!(is_structural_stock_flow(
        "population",
        "deaths",
        &stock_inflows,
        &stock_outflows,
    ));
    // Reversed direction should NOT match
    assert!(!is_structural_stock_flow(
        "births",
        "population",
        &stock_inflows,
        &stock_outflows,
    ));
    // Unrelated pair should not match
    assert!(!is_structural_stock_flow(
        "birth_rate",
        "births",
        &stock_inflows,
        &stock_outflows,
    ));
}

#[test]
fn test_is_structural_flow_stock_matches_connector_direction() {
    // Connectors render as dependency -> dependent. For structural
    // stock-flow deps, the connector goes from flow -> stock.
    // is_structural_flow_stock(from=flow, to=stock) should return true.
    let stock_inflows: HashMap<String, HashSet<String>> =
        HashMap::from([("population".into(), HashSet::from(["births".into()]))]);
    let stock_outflows: HashMap<String, HashSet<String>> =
        HashMap::from([("population".into(), HashSet::from(["deaths".into()]))]);

    assert!(is_structural_flow_stock(
        "births",
        "population",
        &stock_inflows,
        &stock_outflows,
    ));
    assert!(is_structural_flow_stock(
        "deaths",
        "population",
        &stock_inflows,
        &stock_outflows,
    ));
    // Reversed direction should NOT match
    assert!(!is_structural_flow_stock(
        "population",
        "births",
        &stock_inflows,
        &stock_outflows,
    ));
}

#[test]
fn test_contains_ident_word_boundary() {
    assert!(contains_ident("a + b * c", "b"));
    assert!(!contains_ident("abc", "b"));
    assert!(contains_ident("birth_rate * population", "birth_rate"));
    assert!(!contains_ident("high_birth_rate * x", "birth_rate"));
}

fn make_aux(ident: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
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

#[test]
fn test_extract_equation_deps_simple() {
    let var = make_aux("births", "population * birth_rate");
    let idents: HashSet<String> = ["population", "birth_rate", "births", "death_rate"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut deps = extract_equation_deps(&var, &idents);
    deps.sort();
    assert_eq!(deps, vec!["birth_rate", "population"]);
}

#[test]
fn test_extract_equation_deps_excludes_self() {
    let var = make_aux("x", "x + y");
    let idents: HashSet<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
    let deps = extract_equation_deps(&var, &idents);
    assert_eq!(deps, vec!["y"]);
}

#[test]
fn test_extract_equation_deps_builtin_function() {
    let var = make_aux("result", "MAX(a, b)");
    let idents: HashSet<String> = ["a", "b", "result"].iter().map(|s| s.to_string()).collect();
    let mut deps = extract_equation_deps(&var, &idents);
    deps.sort();
    assert_eq!(deps, vec!["a", "b"]);
}

#[test]
fn test_extract_equation_deps_if_then_else() {
    let var = make_aux("output", "IF THEN ELSE(flag > 0, alpha, beta)");
    let idents: HashSet<String> = ["flag", "alpha", "beta", "output"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut deps = extract_equation_deps(&var, &idents);
    deps.sort();
    assert_eq!(deps, vec!["alpha", "beta", "flag"]);
}

#[test]
fn test_extract_equation_deps_no_equation() {
    let var = datamodel::Variable::Stock(datamodel::Stock {
        ident: "stock".to_string(),
        equation: datamodel::Equation::Scalar(String::new()),
        documentation: String::new(),
        units: None,
        inflows: vec![],
        outflows: vec![],
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
        ai_state: None,
        uid: None,
    });
    let idents: HashSet<String> = ["stock", "x"].iter().map(|s| s.to_string()).collect();
    let deps = extract_equation_deps(&var, &idents);
    assert!(deps.is_empty());
}

#[test]
fn test_extract_equation_deps_arrayed_uses_all_entries() {
    let var = datamodel::Variable::Aux(datamodel::Aux {
        ident: "arr".to_string(),
        equation: datamodel::Equation::Arrayed(
            vec!["dim".to_string()],
            vec![
                ("a".to_string(), "foo".to_string(), None, None),
                ("b".to_string(), "bar".to_string(), None, None),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
        ai_state: None,
        uid: None,
    });
    let idents: HashSet<String> = ["arr", "foo", "bar"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    let mut deps = extract_equation_deps(&var, &idents);
    deps.sort();
    assert_eq!(deps, vec!["bar", "foo"]);
}

#[test]
fn test_select_best_layout_fewest_crossings() {
    let results = vec![
        Ok(LayoutResult {
            view: datamodel::StockFlow {
                name: None,
                elements: vec![ViewElement::Aux(view_element::Aux {
                    name: "from_5_crossings".to_string(),
                    uid: 1,
                    x: 0.0,
                    y: 0.0,
                    label_side: LabelSide::Bottom,
                    compat: None,
                })],
                view_box: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 100.0,
                },
                zoom: 1.0,
                use_lettered_polarity: false,
                font: None,
                sketch_compat: None,
            },
            crossings: 5,
            seed: 42,
        }),
        Ok(LayoutResult {
            view: datamodel::StockFlow {
                name: None,
                elements: vec![ViewElement::Aux(view_element::Aux {
                    name: "from_2_crossings".to_string(),
                    uid: 2,
                    x: 0.0,
                    y: 0.0,
                    label_side: LabelSide::Bottom,
                    compat: None,
                })],
                view_box: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 100.0,
                },
                zoom: 1.0,
                use_lettered_polarity: false,
                font: None,
                sketch_compat: None,
            },
            crossings: 2,
            seed: 123,
        }),
    ];
    let best = select_best_layout(results).unwrap();
    // Should pick the one with 2 crossings (fewer is better)
    assert_eq!(best.elements.len(), 1);
    if let ViewElement::Aux(aux) = &best.elements[0] {
        assert_eq!(aux.name, "from_2_crossings");
    } else {
        unreachable!("expected Aux element");
    }
}

#[test]
fn test_select_best_layout_lowest_seed_on_tie() {
    let results = vec![
        Ok(LayoutResult {
            view: datamodel::StockFlow {
                name: None,
                elements: vec![ViewElement::Aux(view_element::Aux {
                    name: "from_seed_123".to_string(),
                    uid: 1,
                    x: 0.0,
                    y: 0.0,
                    label_side: LabelSide::Bottom,
                    compat: None,
                })],
                view_box: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 100.0,
                },
                zoom: 1.0,
                use_lettered_polarity: false,
                font: None,
                sketch_compat: None,
            },
            crossings: 3,
            seed: 123,
        }),
        Ok(LayoutResult {
            view: datamodel::StockFlow {
                name: None,
                elements: vec![ViewElement::Aux(view_element::Aux {
                    name: "from_seed_42".to_string(),
                    uid: 2,
                    x: 0.0,
                    y: 0.0,
                    label_side: LabelSide::Bottom,
                    compat: None,
                })],
                view_box: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height: 100.0,
                },
                zoom: 1.0,
                use_lettered_polarity: false,
                font: None,
                sketch_compat: None,
            },
            crossings: 3,
            seed: 42,
        }),
    ];
    let best = select_best_layout(results).unwrap();
    // Should pick seed 42 (lower seed wins on tie)
    assert_eq!(best.elements.len(), 1);
    if let ViewElement::Aux(aux) = &best.elements[0] {
        assert_eq!(aux.name, "from_seed_42");
    } else {
        unreachable!("expected Aux element");
    }
}

#[test]
fn test_generate_layout_aux_only() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "rate".to_string(),
                equation: datamodel::Equation::Scalar("0.5".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "factor".to_string(),
                equation: datamodel::Equation::Scalar("rate * 2".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();
    assert_eq!(
        result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Aux(_)))
            .count(),
        2
    );
}

#[test]
fn test_generate_layout_single_aux() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "x".to_string(),
            equation: datamodel::Equation::Scalar("42".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat {
                visibility: datamodel::Visibility::Public,
                ..datamodel::Compat::default()
            },
            ai_state: None,
            uid: Some(1),
        })],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();
    assert_eq!(result.elements.len(), 1);
}

#[test]
fn test_generate_layout_disconnected_stocks() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("200".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();
    let stocks: Vec<_> = result
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Stock(_)))
        .collect();
    assert_eq!(stocks.len(), 2);
}

#[test]
fn test_generate_layout_disconnected_chains_do_not_explode_apart() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_c".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(3),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_d".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(4),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let stock_positions: Vec<(f64, f64)> = result
        .elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Stock(s) = e {
                Some((s.x, s.y))
            } else {
                None
            }
        })
        .collect();

    assert_eq!(stock_positions.len(), 4);

    let min_x = stock_positions
        .iter()
        .map(|(x, _)| *x)
        .fold(f64::INFINITY, f64::min);
    let max_x = stock_positions
        .iter()
        .map(|(x, _)| *x)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = stock_positions
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::INFINITY, f64::min);
    let max_y = stock_positions
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::NEG_INFINITY, f64::max);

    // Disconnected chains should remain in a reasonable neighborhood, not
    // be flung thousands of units apart by force configuration.
    assert!(
        max_x - min_x < 10_000.0,
        "x span too large: {}",
        max_x - min_x
    );
    assert!(
        max_y - min_y < 10_000.0,
        "y span too large: {}",
        max_y - min_y
    );
}

#[test]
fn test_connector_direction_dependency_to_dependent() {
    let project = test_project(simple_model());
    let model = project.get_model(TEST_MODEL).unwrap();
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();

    let uid_to_ident: HashMap<i32, String> = model
        .variables
        .iter()
        .filter_map(|var| match var {
            datamodel::Variable::Stock(s) => {
                s.uid.map(|uid| (uid, canonicalize(&s.ident).into_owned()))
            }
            datamodel::Variable::Flow(f) => {
                f.uid.map(|uid| (uid, canonicalize(&f.ident).into_owned()))
            }
            datamodel::Variable::Aux(a) => {
                a.uid.map(|uid| (uid, canonicalize(&a.ident).into_owned()))
            }
            datamodel::Variable::Module(_) => None,
        })
        .collect();

    let link_pairs: HashSet<(String, String)> = result
        .elements
        .iter()
        .filter_map(|elem| {
            if let ViewElement::Link(link) = elem {
                let from = uid_to_ident.get(&link.from_uid)?.clone();
                let to = uid_to_ident.get(&link.to_uid)?.clone();
                Some((from, to))
            } else {
                None
            }
        })
        .collect();

    assert!(
        link_pairs.contains(&("birth_rate".to_string(), "births".to_string())),
        "expected dependency link birth_rate -> births"
    );
    assert!(
        !link_pairs.contains(&("births".to_string(), "birth_rate".to_string())),
        "did not expect reversed dependency link births -> birth_rate"
    );
}

#[test]
fn test_compute_metadata_includes_isolated_flows_when_stocks_exist() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["connected_flow".to_string()],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "connected_flow".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "isolated_flow".to_string(),
                equation: datamodel::Equation::Scalar("5".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let project = test_project(model);
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    assert!(
        metadata
            .chains
            .iter()
            .any(|chain| chain.flows.contains(&"isolated_flow".to_string())),
        "expected isolated_flow to be represented in some chain"
    );
}

#[test]
fn test_generate_layout_includes_isolated_flows_when_stocks_exist() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["connected_flow".to_string()],
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "connected_flow".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "isolated_flow".to_string(),
                equation: datamodel::Equation::Scalar("5".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();
    let flow_count = result
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Flow(_)))
        .count();
    assert_eq!(flow_count, 2, "expected both flows to be laid out");
}

#[test]
fn test_generate_layout_includes_module_elements_and_connectors() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Module(datamodel::Module {
                ident: "m".to_string(),
                model_name: "submodel".to_string(),
                documentation: String::new(),
                units: None,
                references: Vec::new(),
                ai_state: None,
                uid: Some(2),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "y".to_string(),
                equation: datamodel::Equation::Scalar("x + m".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None).unwrap();
    let element_uids: HashSet<i32> = result.elements.iter().map(|e| e.get_uid()).collect();

    // All link endpoints should reference rendered elements
    for elem in &result.elements {
        if let ViewElement::Link(link) = elem {
            assert!(
                element_uids.contains(&link.from_uid),
                "link from_uid {} should reference a rendered element",
                link.from_uid
            );
            assert!(
                element_uids.contains(&link.to_uid),
                "link to_uid {} should reference a rendered element",
                link.to_uid
            );
        }
    }

    // Module should produce a ViewElement::Module
    let module_count = result
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Module(_)))
        .count();
    assert_eq!(module_count, 1, "module 'm' should be rendered");

    // Auxiliaries should still be present
    let aux_count = result
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Aux(_)))
        .count();
    assert_eq!(aux_count, 2, "both auxiliaries should be rendered");

    // Module should have finite coordinates from SFDP
    for elem in &result.elements {
        if let ViewElement::Module(m) = elem {
            assert!(
                m.x.is_finite() && m.y.is_finite(),
                "module '{}' should have finite coordinates",
                m.name
            );
        }
    }
}

#[test]
fn test_count_view_crossings_shared_endpoint_bidirectional_links() {
    let view = datamodel::StockFlow {
        name: None,
        elements: vec![
            ViewElement::Aux(view_element::Aux {
                name: "a".to_string(),
                uid: 1,
                x: 0.0,
                y: 0.0,
                label_side: LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Aux(view_element::Aux {
                name: "b".to_string(),
                uid: 2,
                x: 10.0,
                y: 10.0,
                label_side: LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Aux(view_element::Aux {
                name: "c".to_string(),
                uid: 3,
                x: 10.0,
                y: -10.0,
                label_side: LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Link(view_element::Link {
                uid: 4,
                from_uid: 1,
                to_uid: 2,
                shape: LinkShape::Straight,
                polarity: None,
            }),
            ViewElement::Link(view_element::Link {
                uid: 5,
                from_uid: 3,
                to_uid: 1,
                shape: LinkShape::Straight,
                polarity: None,
            }),
        ],
        view_box: Rect {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
        },
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    };

    assert_eq!(count_view_crossings(&view), 0);
}

#[test]
fn test_compute_metadata_populates_feedback_loops_from_model_metadata() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("y".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "y".to_string(),
                equation: datamodel::Equation::Scalar("x".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: vec![datamodel::LoopMetadata {
            uids: vec![1, 2],
            deleted: false,
            name: "R1".to_string(),
            description: String::new(),
        }],
        groups: Vec::new(),
    };

    let project = test_project(model);
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();
    assert_eq!(metadata.feedback_loops.len(), 1);
    assert_eq!(metadata.feedback_loops[0].name, "R1");
    assert_eq!(
        metadata.feedback_loops[0].causal_chain(),
        &["x".to_string(), "y".to_string(), "x".to_string()]
    );
}

#[test]
fn test_chain_importance_formula() {
    let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut all_flows: BTreeSet<String> = BTreeSet::new();

    // Chain: s1 -> f1 -> s2 -> f2 -> (none), f3 -> s2
    // 2 stocks + 3 flows = 2*10 + 3*5 = 35
    stock_to_outflows.insert("s1".into(), vec!["f1".into()]);
    stock_to_inflows.insert("s2".into(), vec!["f1".into(), "f3".into()]);
    stock_to_outflows.insert("s2".into(), vec!["f2".into()]);
    stock_to_inflows.insert("s1".into(), vec![]);
    flow_to_stocks.insert("f1".into(), (Some("s1".into()), Some("s2".into())));
    flow_to_stocks.insert("f2".into(), (Some("s2".into()), None));
    flow_to_stocks.insert("f3".into(), (None, Some("s2".into())));
    all_flows.extend(["f1".into(), "f2".into(), "f3".into()]);

    let chains = detect_chains(
        &stock_to_inflows,
        &stock_to_outflows,
        &flow_to_stocks,
        &all_flows,
    );
    assert_eq!(chains.len(), 1);
    assert!((chains[0].importance - 35.0).abs() < f64::EPSILON);
}

#[test]
fn test_chains_sorted_descending() {
    let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut all_flows: BTreeSet<String> = BTreeSet::new();

    // Chain 1: 1 stock + 1 flow = 15
    stock_to_outflows.insert("a".into(), vec!["fa".into()]);
    stock_to_inflows.insert("a".into(), vec![]);
    flow_to_stocks.insert("fa".into(), (Some("a".into()), None));
    all_flows.insert("fa".into());

    // Chain 2: 2 stocks + 1 flow = 25
    stock_to_outflows.insert("b".into(), vec!["fb".into()]);
    stock_to_inflows.insert("b".into(), vec![]);
    stock_to_inflows.insert("c".into(), vec!["fb".into()]);
    stock_to_outflows.insert("c".into(), vec![]);
    flow_to_stocks.insert("fb".into(), (Some("b".into()), Some("c".into())));
    all_flows.insert("fb".into());

    let chains = detect_chains(
        &stock_to_inflows,
        &stock_to_outflows,
        &flow_to_stocks,
        &all_flows,
    );
    assert_eq!(chains.len(), 2);
    assert!(
        chains[0].importance >= chains[1].importance,
        "chains should be sorted descending by importance: {} vs {}",
        chains[0].importance,
        chains[1].importance
    );
    assert!((chains[0].importance - 25.0).abs() < f64::EPSILON);
    assert!((chains[1].importance - 15.0).abs() < f64::EPSILON);
}

#[test]
fn test_isolated_flow_importance_is_5() {
    let stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
    let stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
    let flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut all_flows: BTreeSet<String> = BTreeSet::new();
    all_flows.insert("lonely_flow".into());

    let chains = detect_chains(
        &stock_to_inflows,
        &stock_to_outflows,
        &flow_to_stocks,
        &all_flows,
    );
    assert_eq!(chains.len(), 1);
    assert!((chains[0].importance - 5.0).abs() < f64::EPSILON);
}

#[test]
fn test_ast_deps_exclude_builtins() {
    // A variable referencing TIME (a builtin) should not produce a connector
    // to TIME since TIME is not a model variable.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "rate".to_string(),
                equation: datamodel::Equation::Scalar("0.1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(1),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "output".to_string(),
                equation: datamodel::Equation::Scalar("rate * TIME".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(2),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    let output_deps = metadata.dep_graph.get("output").unwrap();
    assert!(output_deps.contains("rate"), "output should depend on rate");
    assert!(
        !output_deps.contains("time"),
        "output should NOT depend on builtin TIME"
    );
}

#[test]
fn test_ast_deps_no_false_positives() {
    // String heuristic would falsely match "birth" inside "birthday".
    // AST-based extraction should not.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "birth".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(1),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "birthday".to_string(),
                equation: datamodel::Equation::Scalar("365".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(2),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "output".to_string(),
                equation: datamodel::Equation::Scalar("birthday + 1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(3),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    let output_deps = metadata.dep_graph.get("output").unwrap();
    assert!(
        output_deps.contains("birthday"),
        "output should depend on birthday"
    );
    assert!(
        !output_deps.contains("birth"),
        "output should NOT falsely depend on birth (substring match)"
    );
}

#[test]
fn test_deps_fallback_on_compile_error() {
    // A model with a module referencing a nonexistent submodel should
    // gracefully fall back to string heuristic for dep extraction.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(1),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Module(datamodel::Module {
                ident: "m".to_string(),
                model_name: "nonexistent_model".to_string(),
                documentation: String::new(),
                units: None,
                references: Vec::new(),
                ai_state: None,
                uid: Some(2),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "y".to_string(),
                equation: datamodel::Equation::Scalar("x + 1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(3),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let result = generate_layout(&project, TEST_MODEL, None);
    assert!(
        result.is_ok(),
        "layout should succeed despite compile error, via fallback"
    );
}

#[test]
fn test_ltm_fallback_on_sim_error() {
    // A model with loop_metadata but that can't be simulated should
    // fall back to persisted loop_metadata UIDs for feedback loops.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("y".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(1),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "y".to_string(),
                equation: datamodel::Equation::Scalar("x".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(2),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
        ],
        views: Vec::new(),
        loop_metadata: vec![datamodel::LoopMetadata {
            uids: vec![1, 2],
            deleted: false,
            name: "R1".to_string(),
            description: String::new(),
        }],
        groups: Vec::new(),
    };
    let project = test_project(model);
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    // Should have fallen back to persisted metadata since this minimal
    // model doesn't have sim_specs and can't simulate.
    assert_eq!(metadata.feedback_loops.len(), 1);
    assert_eq!(metadata.feedback_loops[0].name, "R1");
}

#[test]
fn test_compute_metadata_returns_none_for_unknown_model() {
    let project = test_project(simple_model());
    assert!(compute_metadata(&project, "nonexistent", None).is_none());
}

#[test]
fn test_compute_metadata_falls_back_for_invalid_equation() {
    // A variable with an unparseable equation should still get
    // string-heuristic dependencies rather than being treated as a
    // constant.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["births".to_string()],
                outflows: vec![],
                ai_state: None,
                uid: Some(1),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "births".to_string(),
                equation: datamodel::Equation::Scalar(
                    "population *** totally_broken_syntax".to_string(),
                ),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(2),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    let births_deps = metadata.dep_graph.get("births").unwrap();
    assert!(
        births_deps.contains("population"),
        "births should depend on population via string-heuristic fallback, got: {:?}",
        births_deps,
    );
    assert!(
        !metadata.constants.contains("births"),
        "births should not be classified as a constant",
    );

    // population should also depend on births via structural inflows
    let pop_deps = metadata.dep_graph.get("population").unwrap();
    assert!(
        pop_deps.contains("births"),
        "population should depend on births via structural inflows, got: {:?}",
        pop_deps,
    );
}

#[test]
fn test_compute_metadata_excludes_non_model_deps() {
    // Equation text that mentions an identifier not in the model
    // (simulating a module output reference like "m·out" that
    // resolve_non_private_dependencies might pass through).
    // The dep graph should only contain identifiers for actual
    // rendered model variables.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("phantom + 1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(1),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "y".to_string(),
                equation: datamodel::Equation::Scalar("x * 2".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(2),
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..Default::default()
                },
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

    // "phantom" is not a model variable so should not appear in the dep graph
    let all_graph_nodes: BTreeSet<&String> = metadata
        .dep_graph
        .keys()
        .chain(metadata.dep_graph.values().flat_map(|deps| deps.iter()))
        .collect();
    assert!(
        !all_graph_nodes.contains(&"phantom".to_string()),
        "dep graph should not contain non-model identifiers, got: {:?}",
        all_graph_nodes,
    );

    // y should still depend on x (a real model variable)
    let y_deps = metadata.dep_graph.get("y").unwrap();
    assert!(
        y_deps.contains("x"),
        "y should depend on x, got: {:?}",
        y_deps,
    );

    // x should be a constant since its only dep (phantom) was filtered
    assert!(
        metadata.constants.contains("x"),
        "x should be a constant after filtering non-model deps",
    );
}

#[test]
fn test_resolve_model_name_returns_actual_name_for_main_alias() {
    let model = datamodel::Model {
        name: String::new(),
        sim_specs: None,
        variables: Vec::new(),
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let project = test_project(model);
    // "main" should resolve to the empty string (the actual model name)
    assert_eq!(resolve_model_name(&project, "main"), "");
}

#[test]
fn test_resolve_model_name_passthrough_for_named_model() {
    let project = test_project(simple_model());
    assert_eq!(resolve_model_name(&project, TEST_MODEL), TEST_MODEL);
}

#[test]
fn test_resolve_model_name_passthrough_for_unknown_model() {
    let project = test_project(simple_model());
    assert_eq!(resolve_model_name(&project, "nonexistent"), "nonexistent");
}

#[test]
fn test_layout_chain() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["births".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "births".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let config = LayoutConfig::default();

    let mut metadata = ComputedMetadata::new_empty();
    metadata
        .flow_to_stocks
        .insert("births".to_string(), (None, Some("population".to_string())));
    metadata
        .stock_to_inflows
        .insert("population".to_string(), vec!["births".to_string()]);

    let mut state = LayoutState::new(&model);
    let stocks = vec!["population".to_string()];
    let flows = vec!["births".to_string()];

    layout_chain(
        &mut state,
        &config,
        &metadata,
        &stocks,
        &flows,
        Position::new(100.0, 100.0),
    )
    .unwrap();

    // 1 stock + 1 flow + 1 cloud (births has no from_stock, so a source cloud is created)
    let stock_count = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Stock(_)))
        .count();
    let flow_count = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Flow(_)))
        .count();
    let cloud_count = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(_)))
        .count();
    assert_eq!(stock_count, 1);
    assert_eq!(flow_count, 1);
    assert_eq!(
        cloud_count, 1,
        "births has no from_stock, so one cloud expected"
    );

    // Stock and flow are present by name
    let element_names: HashSet<String> = state
        .elements
        .iter()
        .filter_map(|e| e.get_name().map(|n| canonicalize(n).into_owned()))
        .collect();
    assert!(
        element_names.contains("population"),
        "stock 'population' missing"
    );
    assert!(element_names.contains("births"), "flow 'births' missing");

    // All elements have finite coordinates (normalization happens
    // later in the pipeline, so pre-normalization clouds at unconnected
    // endpoints may have negative coordinates).
    for elem in &state.elements {
        match elem {
            ViewElement::Stock(s) => {
                assert!(s.x.is_finite(), "stock {} has non-finite x", s.name);
                assert!(s.y.is_finite(), "stock {} has non-finite y", s.name);
            }
            ViewElement::Flow(f) => {
                assert!(f.x.is_finite(), "flow {} has non-finite x", f.name);
                assert!(f.y.is_finite(), "flow {} has non-finite y", f.name);
            }
            ViewElement::Cloud(c) => {
                assert!(c.x.is_finite(), "cloud {} has non-finite x", c.uid);
                assert!(c.y.is_finite(), "cloud {} has non-finite y", c.uid);
            }
            _ => {}
        }
    }

    // UIDs are unique
    let mut uids: HashSet<i32> = HashSet::new();
    for elem in &state.elements {
        let uid = elem.get_uid();
        assert!(uids.insert(uid), "duplicate UID: {}", uid);
    }

    // Flow is positioned between the stock and the cloud endpoint.
    // With (None, Some("population")), the flow should be offset left
    // of the stock position (inflow from cloud).
    let stock_elem = state.elements.iter().find_map(|e| {
        if let ViewElement::Stock(s) = e {
            Some(s)
        } else {
            None
        }
    });
    let flow_elem = state.elements.iter().find_map(|e| {
        if let ViewElement::Flow(f) = e {
            Some(f)
        } else {
            None
        }
    });
    let stock_elem = stock_elem.expect("stock element should exist");
    let flow_elem = flow_elem.expect("flow element should exist");

    // The flow's x should differ from the stock's x (not stacked on top)
    assert!(
        (flow_elem.x - stock_elem.x).abs() > 1.0,
        "flow should be offset from stock, got flow.x={} stock.x={}",
        flow_elem.x,
        stock_elem.x
    );

    // Flow points should reference the stock via attached_to_uid
    let attached_uids: Vec<Option<i32>> = flow_elem
        .points
        .iter()
        .map(|pt| pt.attached_to_uid)
        .collect();
    assert!(
        attached_uids.contains(&Some(stock_elem.uid)),
        "at least one flow point should be attached to the stock uid, got {:?}",
        attached_uids
    );
}

#[test]
fn test_build_clouds() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["births".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "births".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let mut metadata = ComputedMetadata::new_empty();
    // births flows into population; no source stock, so a source cloud is needed
    metadata
        .flow_to_stocks
        .insert("births".to_string(), (None, Some("population".to_string())));
    metadata
        .stock_to_inflows
        .insert("population".to_string(), vec!["births".to_string()]);

    let mut state = LayoutState::new(&model);

    let flow_uid = state.get_or_alloc_uid("births");
    let stock_uid = state.get_or_alloc_uid("population");

    // Simulate post-layout_chain state: a flow element already exists
    // with points at known positions.  The first point (source end) has
    // no attached stock; the second point (sink end) is attached to the
    // population stock.
    let mut flow_elem = view_element::Flow {
        name: "births".to_string(),
        uid: flow_uid,
        x: 75.0,
        y: 100.0,
        label_side: view_element::LabelSide::Top,
        points: vec![
            view_element::FlowPoint {
                x: 50.0,
                y: 100.0,
                attached_to_uid: None,
            },
            view_element::FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: Some(stock_uid),
            },
        ],
        compat: None,
        label_compat: None,
    };

    let clouds_before = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(_)))
        .count();
    assert_eq!(
        clouds_before, 0,
        "no clouds before calling build_clouds_for_flow"
    );

    build_clouds_for_flow(&mut state, &metadata, "births", &mut flow_elem);

    // Exactly one cloud should be created (source end only; sink is connected)
    let clouds: Vec<&view_element::Cloud> = state
        .elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Cloud(c) = e {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        clouds.len(),
        1,
        "expected 1 cloud for the missing source endpoint"
    );

    let cloud = clouds[0];
    // The cloud's flow_uid must reference the flow
    assert_eq!(
        cloud.flow_uid, flow_uid,
        "cloud's flow_uid should match the flow"
    );

    // The cloud should be positioned at the first flow point (source end)
    assert!(
        (cloud.x - 50.0).abs() < f64::EPSILON,
        "cloud x should match source flow point"
    );
    assert!(
        (cloud.y - 100.0).abs() < f64::EPSILON,
        "cloud y should match source flow point"
    );

    // The first flow point (source) should now be attached to the cloud
    assert_eq!(
        flow_elem.points[0].attached_to_uid,
        Some(cloud.uid),
        "source flow point should be attached to the new cloud"
    );

    // The second flow point (sink) should remain attached to the stock
    assert_eq!(
        flow_elem.points[1].attached_to_uid,
        Some(stock_uid),
        "sink flow point should still be attached to the stock"
    );

    // Cloud UID must be unique (different from flow and stock UIDs)
    let mut uids: HashSet<i32> = HashSet::new();
    uids.insert(flow_uid);
    uids.insert(stock_uid);
    assert!(
        uids.insert(cloud.uid),
        "cloud UID {} should be unique",
        cloud.uid
    );
}

#[test]
fn test_build_clouds_no_cloud_for_connected_endpoint() {
    // When both endpoints are connected to stocks, no clouds should be
    // created at all.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "source".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["transfer".to_string()],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "sink".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["transfer".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "transfer".to_string(),
                equation: datamodel::Equation::Scalar("5".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let mut metadata = ComputedMetadata::new_empty();
    metadata.flow_to_stocks.insert(
        "transfer".to_string(),
        (Some("source".to_string()), Some("sink".to_string())),
    );

    let mut state = LayoutState::new(&model);
    let flow_uid = state.get_or_alloc_uid("transfer");
    let source_uid = state.get_or_alloc_uid("source");
    let sink_uid = state.get_or_alloc_uid("sink");

    let mut flow_elem = view_element::Flow {
        name: "transfer".to_string(),
        uid: flow_uid,
        x: 75.0,
        y: 100.0,
        label_side: view_element::LabelSide::Top,
        points: vec![
            view_element::FlowPoint {
                x: 50.0,
                y: 100.0,
                attached_to_uid: Some(source_uid),
            },
            view_element::FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: Some(sink_uid),
            },
        ],
        compat: None,
        label_compat: None,
    };

    build_clouds_for_flow(&mut state, &metadata, "transfer", &mut flow_elem);

    let cloud_count = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(_)))
        .count();
    assert_eq!(
        cloud_count, 0,
        "no clouds when both endpoints are connected to stocks"
    );
}

#[test]
fn test_build_clouds_sink_cloud() {
    // When the flow has a source stock but no sink stock, a sink cloud
    // should be created at the last flow point.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["deaths".to_string()],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "deaths".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let mut metadata = ComputedMetadata::new_empty();
    // deaths flows out of population; no sink stock
    metadata
        .flow_to_stocks
        .insert("deaths".to_string(), (Some("population".to_string()), None));
    metadata
        .stock_to_outflows
        .insert("population".to_string(), vec!["deaths".to_string()]);

    let mut state = LayoutState::new(&model);
    let flow_uid = state.get_or_alloc_uid("deaths");
    let stock_uid = state.get_or_alloc_uid("population");

    let mut flow_elem = view_element::Flow {
        name: "deaths".to_string(),
        uid: flow_uid,
        x: 125.0,
        y: 100.0,
        label_side: view_element::LabelSide::Top,
        points: vec![
            view_element::FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: Some(stock_uid),
            },
            view_element::FlowPoint {
                x: 150.0,
                y: 100.0,
                attached_to_uid: None,
            },
        ],
        compat: None,
        label_compat: None,
    };

    build_clouds_for_flow(&mut state, &metadata, "deaths", &mut flow_elem);

    let clouds: Vec<&view_element::Cloud> = state
        .elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Cloud(c) = e {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(clouds.len(), 1, "expected 1 cloud for the missing sink");

    let cloud = clouds[0];
    assert_eq!(cloud.flow_uid, flow_uid);

    // Cloud should be at the last (sink) flow point
    assert!(
        (cloud.x - 150.0).abs() < f64::EPSILON,
        "cloud x should match sink flow point"
    );
    assert!(
        (cloud.y - 100.0).abs() < f64::EPSILON,
        "cloud y should match sink flow point"
    );

    // Source flow point should remain attached to the stock
    assert_eq!(flow_elem.points[0].attached_to_uid, Some(stock_uid));

    // Sink flow point should now be attached to the cloud
    assert_eq!(flow_elem.points[1].attached_to_uid, Some(cloud.uid));
}

#[test]
fn test_build_connectors() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["births".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "births".to_string(),
                equation: datamodel::Equation::Scalar("population * birth_rate".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "birth_rate".to_string(),
                equation: datamodel::Equation::Scalar("0.03".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let mut metadata = ComputedMetadata::new_empty();
    // births depends on birth_rate and population
    metadata.dep_graph.insert(
        "births".to_string(),
        vec!["birth_rate".to_string(), "population".to_string()]
            .into_iter()
            .collect(),
    );
    // population depends on births (structural inflow)
    metadata.dep_graph.insert(
        "population".to_string(),
        vec!["births".to_string()].into_iter().collect(),
    );
    metadata
        .stock_to_inflows
        .insert("population".to_string(), vec!["births".to_string()]);
    metadata
        .flow_to_stocks
        .insert("births".to_string(), (None, Some("population".to_string())));

    let mut state = LayoutState::new(&model);

    // Pre-populate positioned view elements (simulating post-layout state)
    let pop_uid = state.get_or_alloc_uid("population");
    let births_uid = state.get_or_alloc_uid("births");
    let br_uid = state.get_or_alloc_uid("birth_rate");

    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "population".to_string(),
        uid: pop_uid,
        x: 200.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(pop_uid, Position::new(200.0, 100.0));

    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "births".to_string(),
        uid: births_uid,
        x: 150.0,
        y: 100.0,
        label_side: view_element::LabelSide::Top,
        points: vec![
            FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: None,
            },
            FlowPoint {
                x: 200.0,
                y: 100.0,
                attached_to_uid: Some(pop_uid),
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state
        .positions
        .insert(births_uid, Position::new(150.0, 100.0));

    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "birth_rate".to_string(),
        uid: br_uid,
        x: 150.0,
        y: 50.0,
        label_side: view_element::LabelSide::Top,
        compat: None,
    }));
    state.positions.insert(br_uid, Position::new(150.0, 50.0));

    build_connectors(&mut state, &model, &metadata).unwrap();

    // Collect all Link elements
    let links: Vec<&view_element::Link> = state
        .elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Link(l) = e {
                Some(l)
            } else {
                None
            }
        })
        .collect();

    // Non-structural edge: birth_rate -> births should produce a link
    let br_to_births = links
        .iter()
        .find(|l| l.from_uid == br_uid && l.to_uid == births_uid);
    assert!(
        br_to_births.is_some(),
        "expected a link from birth_rate -> births"
    );

    // Structural stock-flow edge: births -> population (inflow
    // relationship) should NOT produce a link (already represented by
    // the flow pipe).
    let births_to_pop = links
        .iter()
        .find(|l| l.from_uid == births_uid && l.to_uid == pop_uid);
    assert!(
        births_to_pop.is_none(),
        "structural flow->stock edge should not produce a link"
    );

    // The stock->flow structural edge (population -> births) should
    // create an arc link, not be skipped.  Verify it exists with an
    // arc shape.
    let pop_to_births = links
        .iter()
        .find(|l| l.from_uid == pop_uid && l.to_uid == births_uid);
    assert!(
        pop_to_births.is_some(),
        "stock->flow structural edge should produce an arc link"
    );
    assert!(
        matches!(pop_to_births.unwrap().shape, LinkShape::Arc(_)),
        "stock->flow link should have arc shape"
    );

    // Every link's from_uid and to_uid should reference existing
    // element UIDs in the state.
    let all_uids: HashSet<i32> = state.elements.iter().map(|e| e.get_uid()).collect();
    for link in &links {
        assert!(
            all_uids.contains(&link.from_uid),
            "link from_uid {} not found in elements",
            link.from_uid
        );
        assert!(
            all_uids.contains(&link.to_uid),
            "link to_uid {} not found in elements",
            link.to_uid
        );
    }

    // No duplicate links (each (from_uid, to_uid) pair appears at most once)
    let link_pairs: HashSet<(i32, i32)> = links.iter().map(|l| (l.from_uid, l.to_uid)).collect();
    assert_eq!(link_pairs.len(), links.len(), "duplicate links detected");
}

#[test]
fn test_optimize_labels() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "aux_a".to_string(),
                equation: datamodel::Equation::Scalar("aux_b".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "aux_b".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let mut metadata = ComputedMetadata::new_empty();
    // aux_a depends on aux_b
    metadata.dep_graph.insert(
        "aux_a".to_string(),
        vec!["aux_b".to_string()].into_iter().collect(),
    );
    // reverse: aux_b is used by aux_a
    metadata.reverse_dep_graph.insert(
        "aux_b".to_string(),
        vec!["aux_a".to_string()].into_iter().collect(),
    );

    let mut state = LayoutState::new(&model);
    let a_uid = state.get_or_alloc_uid("aux_a");
    let b_uid = state.get_or_alloc_uid("aux_b");

    // aux_a below aux_b: the connection runs downward from aux_b and
    // upward into aux_a.  From each variable's perspective the connector
    // leaves toward the bottom, making Bottom a poor label position and
    // causing the optimizer to prefer Top.
    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "aux_a".to_string(),
        uid: a_uid,
        x: 100.0,
        y: 200.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(a_uid, Position::new(100.0, 200.0));

    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "aux_b".to_string(),
        uid: b_uid,
        x: 100.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(b_uid, Position::new(100.0, 100.0));

    let orig_a = (100.0_f64, 200.0_f64);
    let orig_b = (100.0_f64, 100.0_f64);

    optimize_labels(&mut state, &model, &metadata);

    // At least one element's label_side should have moved away from
    // Bottom, because each aux has a connection in the downward or
    // upward direction that makes Bottom suboptimal.
    let sides: Vec<view_element::LabelSide> = state
        .elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Aux(a) = e {
                Some(a.label_side)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(sides.len(), 2, "should still have 2 aux elements");
    let any_changed = sides.iter().any(|&s| s != view_element::LabelSide::Bottom);
    assert!(
        any_changed,
        "optimize_labels should adjust at least one label_side from the default; got {:?}",
        sides
    );

    // Element positions must not change -- optimize_labels moves
    // labels, not the elements themselves.
    for elem in &state.elements {
        if let ViewElement::Aux(a) = elem {
            if a.name == "aux_a" {
                assert_eq!(a.x, orig_a.0, "aux_a x should be unchanged");
                assert_eq!(a.y, orig_a.1, "aux_a y should be unchanged");
            } else if a.name == "aux_b" {
                assert_eq!(a.x, orig_b.0, "aux_b x should be unchanged");
                assert_eq!(a.y, orig_b.1, "aux_b y should be unchanged");
            }
        }
    }
}

#[test]
fn test_place_auxiliaries() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["births".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "births".to_string(),
                equation: datamodel::Equation::Scalar("population * birth_rate".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "birth_rate".to_string(),
                equation: datamodel::Equation::Scalar("0.03".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let config = LayoutConfig::default();

    let mut metadata = ComputedMetadata::new_empty();
    // births depends on birth_rate and population
    metadata.dep_graph.insert(
        "births".to_string(),
        vec!["birth_rate".to_string(), "population".to_string()]
            .into_iter()
            .collect(),
    );
    // population depends on births (structural inflow)
    metadata.dep_graph.insert(
        "population".to_string(),
        vec!["births".to_string()].into_iter().collect(),
    );
    metadata
        .stock_to_inflows
        .insert("population".to_string(), vec!["births".to_string()]);
    metadata
        .flow_to_stocks
        .insert("births".to_string(), (None, Some("population".to_string())));

    let mut state = LayoutState::new(&model);

    // Pre-populate stock and flow view elements (simulating
    // post-layout_chain state).
    let pop_uid = state.get_or_alloc_uid("population");
    let births_uid = state.get_or_alloc_uid("births");

    let stock_x = 200.0_f64;
    let stock_y = 100.0_f64;
    let flow_x = 150.0_f64;
    let flow_y = 100.0_f64;

    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "population".to_string(),
        uid: pop_uid,
        x: stock_x,
        y: stock_y,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    }));
    state
        .positions
        .insert(pop_uid, Position::new(stock_x, stock_y));

    // A cloud at the source end of the flow (births has no from_stock)
    let cloud_uid = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: cloud_uid,
        flow_uid: births_uid,
        x: 100.0,
        y: 100.0,
        compat: None,
    }));
    state
        .positions
        .insert(cloud_uid, Position::new(100.0, 100.0));

    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "births".to_string(),
        uid: births_uid,
        x: flow_x,
        y: flow_y,
        label_side: view_element::LabelSide::Top,
        points: vec![
            FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: Some(cloud_uid),
            },
            FlowPoint {
                x: 200.0,
                y: 100.0,
                attached_to_uid: Some(pop_uid),
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state
        .positions
        .insert(births_uid, Position::new(flow_x, flow_y));

    let chains_data = vec![(
        vec!["population".to_string()],
        vec!["births".to_string()],
        vec!["population".to_string(), "births".to_string()],
    )];

    place_auxiliaries(&mut state, &config, &model, &metadata, &chains_data).unwrap();

    // An aux view element should be created for birth_rate
    let aux_elems: Vec<&view_element::Aux> = state
        .elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Aux(a) = e {
                Some(a)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        aux_elems.len(),
        1,
        "expected exactly one aux element for birth_rate"
    );
    let aux = aux_elems[0];
    assert!(
        canonicalize(&aux.name).contains("birth_rate"),
        "aux element should be named birth_rate, got '{}'",
        aux.name
    );

    // The rigid group constraint means chain elements move together
    // as a unit.  Verify their relative spacing is preserved even if
    // SFDP shifts the group slightly.
    let post_stock = state
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Stock(s) = e {
                (s.uid == pop_uid).then_some((s.x, s.y))
            } else {
                None
            }
        })
        .expect("stock should exist");
    let post_flow = state
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Flow(f) = e {
                (f.uid == births_uid).then_some((f.x, f.y))
            } else {
                None
            }
        })
        .expect("flow should exist");

    let orig_dx = stock_x - flow_x;
    let orig_dy = stock_y - flow_y;
    let post_dx = post_stock.0 - post_flow.0;
    let post_dy = post_stock.1 - post_flow.1;
    assert!(
        (orig_dx - post_dx).abs() < 1.0 && (orig_dy - post_dy).abs() < 1.0,
        "chain relative spacing should be preserved: \
         orig delta ({}, {}), post delta ({}, {})",
        orig_dx,
        orig_dy,
        post_dx,
        post_dy
    );

    // The aux element should have positive coordinates
    assert!(aux.x > 0.0, "aux x should be positive, got {}", aux.x);
    assert!(aux.y > 0.0, "aux y should be positive, got {}", aux.y);

    // The aux element's UID should be unique
    let all_uids: Vec<i32> = state.elements.iter().map(|e| e.get_uid()).collect();
    let unique_uids: HashSet<i32> = all_uids.iter().copied().collect();
    assert_eq!(
        all_uids.len(),
        unique_uids.len(),
        "all UIDs should be unique: {:?}",
        all_uids
    );
    assert!(
        unique_uids.contains(&aux.uid),
        "aux UID {} should be in the element set",
        aux.uid
    );
    assert_ne!(aux.uid, pop_uid, "aux UID should differ from stock UID");
    assert_ne!(aux.uid, births_uid, "aux UID should differ from flow UID");
}

#[test]
fn test_compute_metadata_with_main_alias() {
    let mut model = simple_model();
    model.name = String::new();
    let project = test_project(model);
    let metadata = compute_metadata(&project, "main", None);
    assert!(
        metadata.is_some(),
        "compute_metadata should work with 'main' alias for unnamed models"
    );
    let metadata = metadata.unwrap();
    assert!(!metadata.dep_graph.is_empty());
}

/// Build a LayoutState with known elements for deletion tests.
/// Layout: stock(uid=1) --flow(uid=2)--> with aux(uid=3) feeding
/// into the flow via a link, plus clouds on the flow endpoints,
/// and a link from aux to flow.
fn make_deletion_state() -> LayoutState {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    // Register named elements
    state.uid_manager.add(1, "population");
    state.uid_manager.add(2, "births");
    state.uid_manager.add(3, "birth_rate");
    // Unnamed elements (clouds and links) get sequential UIDs
    state.uid_manager.add(10, "");
    state.uid_manager.add(11, "");
    state.uid_manager.add(20, "");

    state
        .display_names
        .insert("population".into(), "population".into());
    state.display_names.insert("births".into(), "births".into());
    state
        .display_names
        .insert("birth_rate".into(), "birth_rate".into());

    // Stock
    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "population".into(),
        uid: 1,
        x: 100.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(1, Position::new(100.0, 100.0));

    // Flow
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "births".into(),
        uid: 2,
        x: 50.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        points: vec![
            FlowPoint {
                x: 0.0,
                y: 100.0,
                attached_to_uid: Some(10),
            },
            FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: Some(1),
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(2, Position::new(50.0, 100.0));

    // Aux
    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "birth_rate".into(),
        uid: 3,
        x: 50.0,
        y: 50.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(3, Position::new(50.0, 50.0));

    // Source cloud for the flow
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: 10,
        flow_uid: 2,
        x: 0.0,
        y: 100.0,
        compat: None,
    }));
    state.positions.insert(10, Position::new(0.0, 100.0));
    state.cloud_ident_to_uid.insert("__cloud_10".into(), 10);
    state
        .cloud_ident_to_flow_ident
        .insert("__cloud_10".into(), "births".into());
    state
        .flow_ident_to_clouds
        .entry("births".into())
        .or_default()
        .push("__cloud_10".into());

    // Sink cloud for the flow
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: 11,
        flow_uid: 2,
        x: 100.0,
        y: 100.0,
        compat: None,
    }));
    state.positions.insert(11, Position::new(100.0, 100.0));
    state.cloud_ident_to_uid.insert("__cloud_11".into(), 11);
    state
        .cloud_ident_to_flow_ident
        .insert("__cloud_11".into(), "births".into());
    state
        .flow_ident_to_clouds
        .entry("births".into())
        .or_default()
        .push("__cloud_11".into());

    // Link from birth_rate(3) to births(2)
    state.elements.push(ViewElement::Link(view_element::Link {
        uid: 20,
        from_uid: 3,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: None,
    }));

    state
}

#[test]
fn test_apply_deletion_removes_aux_element() {
    let mut state = make_deletion_state();

    let had_aux = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Aux(a) if a.uid == 3));
    assert!(had_aux, "precondition: aux should exist before deletion");

    state.apply_deletion("birth_rate");

    let has_aux = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Aux(a) if a.uid == 3));
    assert!(!has_aux, "aux element should be removed after deletion");
    assert!(
        !state.positions.contains_key(&3),
        "position should be removed"
    );
}

#[test]
fn test_apply_deletion_removes_links_referencing_deleted_uid() {
    let mut state = make_deletion_state();

    let link_count_before = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Link(_)))
        .count();
    assert_eq!(link_count_before, 1, "precondition: one link");

    // Delete the aux that is the source of the link
    state.apply_deletion("birth_rate");

    let link_count_after = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Link(_)))
        .count();
    assert_eq!(
        link_count_after, 0,
        "link from deleted element should be removed"
    );
}

#[test]
fn test_apply_deletion_removes_links_to_deleted_uid() {
    let mut state = make_deletion_state();

    // Delete the flow that is the target of the link
    state.apply_deletion("births");

    let link_count = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Link(_)))
        .count();
    assert_eq!(link_count, 0, "link to deleted element should be removed");
}

#[test]
fn test_apply_deletion_removes_clouds_for_deleted_flow() {
    let mut state = make_deletion_state();

    let cloud_count_before = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(_)))
        .count();
    assert_eq!(cloud_count_before, 2, "precondition: two clouds");

    state.apply_deletion("births");

    let cloud_count_after = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(_)))
        .count();
    assert_eq!(
        cloud_count_after, 0,
        "clouds for deleted flow should be removed"
    );
    assert!(
        state.flow_ident_to_clouds.is_empty(),
        "flow_ident_to_clouds should be cleaned up"
    );
    assert!(
        state.cloud_ident_to_uid.is_empty(),
        "cloud_ident_to_uid should be cleaned up"
    );
    assert!(
        state.cloud_ident_to_flow_ident.is_empty(),
        "cloud_ident_to_flow_ident should be cleaned up"
    );
}

#[test]
fn test_apply_deletion_noop_for_unknown_ident() {
    let mut state = make_deletion_state();
    let elem_count_before = state.elements.len();

    state.apply_deletion("nonexistent_var");

    assert_eq!(
        state.elements.len(),
        elem_count_before,
        "deleting unknown ident should be a no-op"
    );
}

#[test]
fn test_apply_deletion_all_chain_elements() {
    let mut state = make_deletion_state();

    // Delete everything: aux, flow, stock -- order matters for
    // verifying cascading cleanup
    state.apply_deletion("birth_rate");
    state.apply_deletion("births");
    state.apply_deletion("population");

    assert!(
        state.elements.is_empty(),
        "all elements should be removed after deleting entire chain; remaining: {:?}",
        state
            .elements
            .iter()
            .map(|e| e.get_uid())
            .collect::<Vec<_>>()
    );
    assert!(
        state.positions.is_empty(),
        "all positions should be removed"
    );
    assert!(
        state.cloud_ident_to_uid.is_empty(),
        "cloud maps should be empty"
    );
    assert!(
        state.cloud_ident_to_flow_ident.is_empty(),
        "cloud flow maps should be empty"
    );
    assert!(
        state.flow_ident_to_clouds.is_empty(),
        "flow cloud maps should be empty"
    );
}

fn make_rename_state() -> LayoutState {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(5, "old_name");
    state
        .display_names
        .insert("old_name".into(), "Old Name".into());

    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "Old Name".into(),
        uid: 5,
        x: 150.0,
        y: 200.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(5, Position::new(150.0, 200.0));

    state
}

#[test]
fn test_apply_rename_updates_element_name() {
    let mut state = make_rename_state();

    state.apply_rename("old_name", "new_name", "New Name");

    let aux = state
        .elements
        .iter()
        .find(|e| e.get_uid() == 5)
        .expect("element should still exist");
    assert_eq!(
        aux.get_name(),
        Some(format_label_with_line_breaks("New Name").as_str())
    );
}

#[test]
fn test_apply_rename_preserves_position_and_uid() {
    let mut state = make_rename_state();

    state.apply_rename("old_name", "new_name", "New Name");

    let pos = state
        .positions
        .get(&5)
        .expect("position should be preserved");
    assert_eq!(pos.x, 150.0);
    assert_eq!(pos.y, 200.0);

    let elem = state.elements.iter().find(|e| e.get_uid() == 5);
    assert!(
        elem.is_some(),
        "element with original UID should still exist"
    );
}

#[test]
fn test_apply_rename_updates_uid_manager() {
    let mut state = make_rename_state();

    state.apply_rename("old_name", "new_name", "New Name");

    assert_eq!(state.uid_manager.get_uid("new_name"), Some(5));
    assert_eq!(state.uid_manager.get_uid("old_name"), None);
}

#[test]
fn test_apply_rename_updates_display_names() {
    let mut state = make_rename_state();

    state.apply_rename("old_name", "new_name", "New Name");

    assert_eq!(
        state.display_names.get("new_name"),
        Some(&"New Name".to_string())
    );
    assert_eq!(state.display_names.get("old_name"), None);
}

#[test]
fn test_apply_rename_noop_for_unknown_ident() {
    let mut state = make_rename_state();
    let elem_count_before = state.elements.len();

    state.apply_rename("nonexistent", "something", "Something");

    assert_eq!(state.elements.len(), elem_count_before);
    // Original element should be unchanged
    let aux = state
        .elements
        .iter()
        .find(|e| e.get_uid() == 5)
        .expect("original element should still exist");
    assert_eq!(aux.get_name(), Some("Old Name"));
}

#[test]
fn test_apply_rename_stock() {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(1, "population");
    state
        .display_names
        .insert("population".into(), "Population".into());
    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "Population".into(),
        uid: 1,
        x: 100.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(1, Position::new(100.0, 100.0));

    state.apply_rename("population", "people", "People");

    let stock = state
        .elements
        .iter()
        .find(|e| e.get_uid() == 1)
        .expect("stock should still exist");
    assert_eq!(stock.get_name(), Some("People"));
    assert_eq!(state.uid_manager.get_uid("people"), Some(1));
    assert_eq!(state.uid_manager.get_uid("population"), None);
    let pos = state.positions.get(&1).unwrap();
    assert_eq!(pos.x, 100.0);
    assert_eq!(pos.y, 100.0);
}

#[test]
fn test_apply_rename_flow() {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(2, "births");
    state.display_names.insert("births".into(), "Births".into());
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "Births".into(),
        uid: 2,
        x: 50.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        points: vec![],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(2, Position::new(50.0, 100.0));

    state.apply_rename("births", "arrivals", "Arrivals");

    let flow = state
        .elements
        .iter()
        .find(|e| e.get_uid() == 2)
        .expect("flow should still exist");
    assert_eq!(flow.get_name(), Some("Arrivals"));
    assert_eq!(state.uid_manager.get_uid("arrivals"), Some(2));
    assert_eq!(state.uid_manager.get_uid("births"), None);
}

// --- diff_connectors tests ---

/// Build a LayoutState and ComputedMetadata for connector diff tests.
///
/// Model:  birth_rate(3) -> births(2) -> population(1)
///                          death_rate(5) -> deaths(4) -> population(1)
///
/// Links: birth_rate->births (uid=20, Arc(45.0)),
///        death_rate->deaths (uid=21, Straight)
///
/// Stock-flow structural edges (population<->births, population<->deaths)
/// are NOT represented as links.
fn make_connector_diff_state() -> (LayoutState, ComputedMetadata) {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(1, "population");
    state.uid_manager.add(2, "births");
    state.uid_manager.add(3, "birth_rate");
    state.uid_manager.add(4, "deaths");
    state.uid_manager.add(5, "death_rate");
    state.uid_manager.add(20, "");
    state.uid_manager.add(21, "");

    state
        .display_names
        .insert("population".into(), "population".into());
    state.display_names.insert("births".into(), "births".into());
    state
        .display_names
        .insert("birth_rate".into(), "birth_rate".into());
    state.display_names.insert("deaths".into(), "deaths".into());
    state
        .display_names
        .insert("death_rate".into(), "death_rate".into());

    // Stock
    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "population".into(),
        uid: 1,
        x: 200.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(1, Position::new(200.0, 100.0));

    // Flow births
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "births".into(),
        uid: 2,
        x: 100.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        points: vec![
            FlowPoint {
                x: 50.0,
                y: 100.0,
                attached_to_uid: None,
            },
            FlowPoint {
                x: 200.0,
                y: 100.0,
                attached_to_uid: Some(1),
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(2, Position::new(100.0, 100.0));

    // Aux birth_rate
    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "birth_rate".into(),
        uid: 3,
        x: 100.0,
        y: 50.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(3, Position::new(100.0, 50.0));

    // Flow deaths
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "deaths".into(),
        uid: 4,
        x: 300.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        points: vec![
            FlowPoint {
                x: 200.0,
                y: 100.0,
                attached_to_uid: Some(1),
            },
            FlowPoint {
                x: 400.0,
                y: 100.0,
                attached_to_uid: None,
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(4, Position::new(300.0, 100.0));

    // Aux death_rate
    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "death_rate".into(),
        uid: 5,
        x: 300.0,
        y: 50.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(5, Position::new(300.0, 50.0));

    // Link: birth_rate -> births with custom Arc shape
    state.elements.push(ViewElement::Link(view_element::Link {
        uid: 20,
        from_uid: 3,
        to_uid: 2,
        shape: LinkShape::Arc(45.0),
        polarity: Some(view_element::LinkPolarity::Positive),
    }));

    // Link: death_rate -> deaths (straight)
    state.elements.push(ViewElement::Link(view_element::Link {
        uid: 21,
        from_uid: 5,
        to_uid: 4,
        shape: LinkShape::Straight,
        polarity: None,
    }));

    // Structural stock->flow links (population->births, population->deaths)
    // These use Arc shapes in the standard layout.
    state.uid_manager.add(22, "");
    state.uid_manager.add(23, "");
    state.elements.push(ViewElement::Link(view_element::Link {
        uid: 22,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Arc(-45.0),
        polarity: None,
    }));
    state.elements.push(ViewElement::Link(view_element::Link {
        uid: 23,
        from_uid: 1,
        to_uid: 4,
        shape: LinkShape::Arc(-45.0),
        polarity: None,
    }));

    // Build matching metadata
    let mut dep_graph = BTreeMap::new();
    // births depends on population and birth_rate
    dep_graph.insert(
        "births".into(),
        BTreeSet::from(["population".into(), "birth_rate".into()]),
    );
    // deaths depends on population and death_rate
    dep_graph.insert(
        "deaths".into(),
        BTreeSet::from(["population".into(), "death_rate".into()]),
    );
    // population depends on births and deaths (structural stock-flow edges)
    dep_graph.insert(
        "population".into(),
        BTreeSet::from(["births".into(), "deaths".into()]),
    );

    let metadata = ComputedMetadata {
        chains: Vec::new(),
        feedback_loops: Vec::new(),
        dominant_periods: Vec::new(),
        dep_graph,
        reverse_dep_graph: BTreeMap::new(),
        constants: BTreeSet::new(),
        stock_to_inflows: HashMap::from([("population".into(), vec!["births".into()])]),
        stock_to_outflows: HashMap::from([("population".into(), vec!["deaths".into()])]),
        flow_to_stocks: HashMap::from([
            ("births".into(), (None, Some("population".to_string()))),
            ("deaths".into(), (Some("population".to_string()), None)),
        ]),
    };

    (state, metadata)
}

#[test]
fn test_diff_connectors_preserves_existing_links() {
    let (mut state, metadata) = make_connector_diff_state();

    diff_connectors(&mut state, &metadata);

    // birth_rate(3)->births(2) should still exist with Arc(45.0) and Positive polarity
    let link_br = state
        .elements
        .iter()
        .find(|e| matches!(e, ViewElement::Link(l) if l.from_uid == 3 && l.to_uid == 2));
    assert!(
        link_br.is_some(),
        "birth_rate->births link should be preserved"
    );
    if let Some(ViewElement::Link(l)) = link_br {
        assert_eq!(
            l.shape,
            LinkShape::Arc(45.0),
            "arc shape should be preserved"
        );
        assert_eq!(
            l.polarity,
            Some(view_element::LinkPolarity::Positive),
            "polarity should be preserved"
        );
    }

    // death_rate(5)->deaths(4) should still exist with Straight shape
    let link_dr = state
        .elements
        .iter()
        .find(|e| matches!(e, ViewElement::Link(l) if l.from_uid == 5 && l.to_uid == 4));
    assert!(
        link_dr.is_some(),
        "death_rate->deaths link should be preserved"
    );
    if let Some(ViewElement::Link(l)) = link_dr {
        assert_eq!(
            l.shape,
            LinkShape::Straight,
            "straight shape should be preserved"
        );
        assert_eq!(l.polarity, None, "None polarity should be preserved");
    }
}

#[test]
fn test_diff_connectors_removes_stale_links() {
    let (mut state, mut metadata) = make_connector_diff_state();

    // Remove death_rate from the dep_graph for deaths, so the
    // death_rate->deaths link should be removed
    if let Some(deps) = metadata.dep_graph.get_mut("deaths") {
        deps.remove("death_rate");
    }

    diff_connectors(&mut state, &metadata);

    // death_rate->deaths link should no longer exist
    let link_dr = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Link(l) if l.from_uid == 5 && l.to_uid == 4));
    assert!(!link_dr, "death_rate->deaths link should be removed");

    // birth_rate->births should still be preserved
    let link_br = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Link(l) if l.from_uid == 3 && l.to_uid == 2));
    assert!(link_br, "birth_rate->births link should be preserved");
}

#[test]
fn test_diff_connectors_adds_new_links() {
    let (mut state, mut metadata) = make_connector_diff_state();

    // Add a new dependency: births also depends on death_rate (contrived)
    if let Some(deps) = metadata.dep_graph.get_mut("births") {
        deps.insert("death_rate".into());
    }

    let link_count_before = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Link(_)))
        .count();
    assert_eq!(link_count_before, 4, "precondition: four links");

    diff_connectors(&mut state, &metadata);

    // New link: death_rate(5)->births(2)
    let new_link = state
        .elements
        .iter()
        .find(|e| matches!(e, ViewElement::Link(l) if l.from_uid == 5 && l.to_uid == 2));
    assert!(
        new_link.is_some(),
        "new death_rate->births link should be created"
    );
    if let Some(ViewElement::Link(l)) = new_link {
        assert_eq!(
            l.shape,
            LinkShape::Straight,
            "new non-structural link should be Straight"
        );
    }

    // Total should now be 5 links (4 existing + 1 new)
    let link_count_after = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Link(_)))
        .count();
    assert_eq!(link_count_after, 5, "should have five links after addition");
}

#[test]
fn test_diff_connectors_noop_same_dep_graph() {
    let (mut state, metadata) = make_connector_diff_state();

    // Snapshot element UIDs and link details before diff
    let links_before: Vec<(i32, i32, LinkShape)> = state
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Link(l) => Some((l.from_uid, l.to_uid, l.shape.clone())),
            _ => None,
        })
        .collect();
    let total_before = state.elements.len();

    diff_connectors(&mut state, &metadata);

    let links_after: Vec<(i32, i32, LinkShape)> = state
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Link(l) => Some((l.from_uid, l.to_uid, l.shape.clone())),
            _ => None,
        })
        .collect();
    let total_after = state.elements.len();

    assert_eq!(total_before, total_after, "element count should not change");

    // Every link from before should still be present with the same shape
    for (from, to, shape) in &links_before {
        let found = links_after
            .iter()
            .any(|(f, t, s)| f == from && t == to && s == shape);
        assert!(
            found,
            "link ({} -> {}) with shape {:?} should be preserved in no-op diff",
            from, to, shape
        );
    }
}

#[test]
fn test_diff_connectors_structural_flow_stock_skipped() {
    let (mut state, metadata) = make_connector_diff_state();

    diff_connectors(&mut state, &metadata);

    // births(2)->population(1) is structural flow->stock. The dep_graph
    // has this as population depending on births, which would be
    // from=births(2), to=population(1). This direction is filtered by
    // is_structural_flow_stock, so no link should exist.
    let flow_to_stock_link = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Link(l) if l.from_uid == 2 && l.to_uid == 1));
    assert!(
        !flow_to_stock_link,
        "structural flow->stock edge should not be represented as a link"
    );

    // But population(1)->births(2) is structural stock->flow. This
    // SHOULD have an Arc link, matching build_connectors behavior.
    let stock_to_flow_link = state
        .elements
        .iter()
        .find(|e| matches!(e, ViewElement::Link(l) if l.from_uid == 1 && l.to_uid == 2));
    assert!(
        stock_to_flow_link.is_some(),
        "structural stock->flow edge should have an Arc link"
    );
    if let Some(ViewElement::Link(l)) = stock_to_flow_link {
        assert!(
            matches!(l.shape, LinkShape::Arc(_)),
            "structural stock->flow link should have Arc shape, got {:?}",
            l.shape
        );
    }
}

// --- diff_clouds tests ---

#[test]
fn test_diff_clouds_preserves_needed_clouds() {
    let (mut state, metadata) = make_connector_diff_state();

    // Add clouds for births flow (source cloud, no from_stock)
    let cloud_uid = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: cloud_uid,
        flow_uid: 2,
        x: 50.0,
        y: 100.0,
        compat: None,
    }));
    state
        .positions
        .insert(cloud_uid, Position::new(50.0, 100.0));

    diff_clouds(&mut state, &metadata);

    // The source cloud for births should still exist (births has no from_stock)
    let has_births_cloud = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Cloud(c) if c.flow_uid == 2));
    assert!(
        has_births_cloud,
        "source cloud for births should be preserved"
    );

    // Position of preserved cloud should be maintained
    let preserved_cloud = state.elements.iter().find_map(|e| match e {
        ViewElement::Cloud(c) if c.flow_uid == 2 => Some(c),
        _ => None,
    });
    if let Some(c) = preserved_cloud {
        assert!(
            state.positions.contains_key(&c.uid),
            "preserved cloud position should exist"
        );
    }
}

#[test]
fn test_diff_clouds_removes_unneeded_clouds() {
    let (mut state, mut metadata) = make_connector_diff_state();

    // Add source cloud for births
    let cloud_uid = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: cloud_uid,
        flow_uid: 2,
        x: 50.0,
        y: 100.0,
        compat: None,
    }));
    state
        .positions
        .insert(cloud_uid, Position::new(50.0, 100.0));

    // Now change metadata so births has BOTH endpoints connected
    metadata.flow_to_stocks.insert(
        "births".into(),
        (
            Some("other_stock".to_string()),
            Some("population".to_string()),
        ),
    );

    diff_clouds(&mut state, &metadata);

    // Cloud for births should be removed since both endpoints are connected
    let has_births_cloud = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Cloud(c) if c.flow_uid == 2));
    assert!(
        !has_births_cloud,
        "cloud for births should be removed when both endpoints are connected"
    );
}

#[test]
fn test_diff_clouds_creates_new_clouds() {
    let (mut state, mut metadata) = make_connector_diff_state();

    // Set deaths to have no to_stock (sink cloud needed) and no existing cloud
    metadata
        .flow_to_stocks
        .insert("deaths".into(), (Some("population".to_string()), None));

    // Verify no cloud for deaths exists yet
    let has_deaths_cloud_before = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Cloud(c) if c.flow_uid == 4));
    assert!(!has_deaths_cloud_before, "precondition: no deaths cloud");

    diff_clouds(&mut state, &metadata);

    // A sink cloud for deaths should now exist
    let has_deaths_cloud = state
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Cloud(c) if c.flow_uid == 4));
    assert!(has_deaths_cloud, "sink cloud for deaths should be created");
}

#[test]
fn test_diff_clouds_noop_when_unchanged() {
    let (mut state, metadata) = make_connector_diff_state();

    // Add source cloud for births (matching metadata: births has no from_stock)
    let cloud_uid_births = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: cloud_uid_births,
        flow_uid: 2,
        x: 50.0,
        y: 100.0,
        compat: None,
    }));
    state
        .positions
        .insert(cloud_uid_births, Position::new(50.0, 100.0));

    // Add sink cloud for deaths (matching metadata: deaths has no to_stock)
    let cloud_uid_deaths = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: cloud_uid_deaths,
        flow_uid: 4,
        x: 400.0,
        y: 100.0,
        compat: None,
    }));
    state
        .positions
        .insert(cloud_uid_deaths, Position::new(400.0, 100.0));

    let cloud_count_before = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(_)))
        .count();

    diff_clouds(&mut state, &metadata);

    let cloud_count_after = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(_)))
        .count();

    assert_eq!(
        cloud_count_before, cloud_count_after,
        "cloud count should not change when metadata is unchanged"
    );
}

// -- diff_clouds role-based preservation tests --

/// When a flow transitions from 2 clouds to 1 (source end gets connected to a
/// stock), the preserved cloud should be the one at the *sink* end — not
/// whichever happens to appear first in the element list.
#[test]
fn test_diff_clouds_preserves_correct_role_on_reduction() {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(1, "stock_a");
    state.uid_manager.add(2, "my_flow");

    // Stock at x=400 (will be connected to flow's sink end)
    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "stock_a".into(),
        uid: 1,
        x: 400.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(1, Position::new(400.0, 100.0));

    // Flow from (50,100) to (350,100)
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "my_flow".into(),
        uid: 2,
        x: 200.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        points: vec![
            FlowPoint {
                x: 50.0,
                y: 100.0,
                attached_to_uid: None,
            },
            FlowPoint {
                x: 350.0,
                y: 100.0,
                attached_to_uid: None,
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(2, Position::new(200.0, 100.0));

    // Source cloud at source end (50, 100) — pushed first
    let source_cloud_uid = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: source_cloud_uid,
        flow_uid: 2,
        x: 50.0,
        y: 100.0,
        compat: None,
    }));
    state
        .positions
        .insert(source_cloud_uid, Position::new(50.0, 100.0));

    // Sink cloud at sink end (350, 100) — pushed second
    let sink_cloud_uid = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: sink_cloud_uid,
        flow_uid: 2,
        x: 350.0,
        y: 100.0,
        compat: None,
    }));
    state
        .positions
        .insert(sink_cloud_uid, Position::new(350.0, 100.0));

    // Metadata: source end is now connected to stock_a, only sink cloud needed
    let metadata = ComputedMetadata {
        chains: Vec::new(),
        feedback_loops: Vec::new(),
        dominant_periods: Vec::new(),
        dep_graph: BTreeMap::new(),
        reverse_dep_graph: BTreeMap::new(),
        constants: BTreeSet::new(),
        stock_to_inflows: HashMap::new(),
        stock_to_outflows: HashMap::from([("stock_a".into(), vec!["my_flow".into()])]),
        flow_to_stocks: HashMap::from([("my_flow".into(), (Some("stock_a".to_string()), None))]),
    };

    diff_clouds(&mut state, &metadata);

    // Should have exactly 1 cloud remaining
    let clouds: Vec<_> = state
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Cloud(c) if c.flow_uid == 2 => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(
        clouds.len(),
        1,
        "should have exactly 1 cloud after reduction"
    );

    // The preserved cloud should be near the sink end (350, 100), not the source end (50, 100)
    let cloud = clouds[0];
    assert!(
        (cloud.x - 350.0).abs() < 1.0,
        "preserved cloud should be at sink position (350), not source (50); got x={}",
        cloud.x
    );
}

// -- resnap_flow_endpoints tests --

/// A horizontal flow with its source endpoint at the stock's right edge
/// should remain at the right edge after re-snap, not shift to the center.
#[test]
fn test_resnap_preserves_stock_edge_position() {
    let config = LayoutConfig::default();
    let half_w = config.stock_width / 2.0;

    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(1, "stock_a");
    state.uid_manager.add(2, "my_flow");

    // Stock at (200, 100)
    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "stock_a".into(),
        uid: 1,
        x: 200.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(1, Position::new(200.0, 100.0));

    // Flow with source endpoint already at stock's right edge
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "my_flow".into(),
        uid: 2,
        x: 300.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        points: vec![
            FlowPoint {
                x: 200.0 + half_w,
                y: 100.0,
                attached_to_uid: Some(1),
            },
            FlowPoint {
                x: 400.0,
                y: 100.0,
                attached_to_uid: None,
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(2, Position::new(300.0, 100.0));

    resnap_flow_endpoints(&mut state, &config);

    let flow = state
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if f.uid == 2 => Some(f),
            _ => None,
        })
        .unwrap();

    let pt = &flow.points[0];
    assert!(
        (pt.x - (200.0 + half_w)).abs() < 0.01,
        "source endpoint should stay at stock right edge ({}), got {}",
        200.0 + half_w,
        pt.x
    );
    assert!(
        (pt.y - 100.0).abs() < 0.01,
        "source endpoint y should be at stock center y (100), got {}",
        pt.y
    );
}

/// A flow whose valve is to the left of the stock should have its
/// endpoint snapped to the stock's left edge.
#[test]
fn test_resnap_snaps_to_correct_face() {
    let config = LayoutConfig::default();
    let half_w = config.stock_width / 2.0;

    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(1, "stock_a");
    state.uid_manager.add(2, "inflow");

    // Stock at (300, 100)
    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "stock_a".into(),
        uid: 1,
        x: 300.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(1, Position::new(300.0, 100.0));

    // Flow valve at (200, 100) — to the left of stock
    // Sink endpoint at stock center (simulating the old buggy re-snap)
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "inflow".into(),
        uid: 2,
        x: 200.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        points: vec![
            FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: None,
            },
            FlowPoint {
                x: 300.0,
                y: 100.0,
                attached_to_uid: Some(1),
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(2, Position::new(200.0, 100.0));

    resnap_flow_endpoints(&mut state, &config);

    let flow = state
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if f.uid == 2 => Some(f),
            _ => None,
        })
        .unwrap();

    // Sink endpoint should be at the stock's LEFT edge (valve is to the left)
    let pt = &flow.points[1];
    assert!(
        (pt.x - (300.0 - half_w)).abs() < 0.01,
        "sink endpoint should be at stock left edge ({}), got {}",
        300.0 - half_w,
        pt.x
    );
    assert!(
        (pt.y - 100.0).abs() < 0.01,
        "sink endpoint y should be at stock center y (100), got {}",
        pt.y
    );
}

/// Vertical flow approaching from below should snap to the stock's bottom edge.
#[test]
fn test_resnap_vertical_flow_snaps_to_bottom_edge() {
    let config = LayoutConfig::default();
    let half_h = config.stock_height / 2.0;

    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(1, "stock_a");
    state.uid_manager.add(2, "vert_flow");

    // Stock at (200, 100)
    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "stock_a".into(),
        uid: 1,
        x: 200.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(1, Position::new(200.0, 100.0));

    // Vertical flow: valve at (200, 250), below the stock
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "vert_flow".into(),
        uid: 2,
        x: 200.0,
        y: 250.0,
        label_side: LabelSide::Left,
        points: vec![
            FlowPoint {
                x: 200.0,
                y: 200.0,
                attached_to_uid: Some(1),
            },
            FlowPoint {
                x: 200.0,
                y: 400.0,
                attached_to_uid: None,
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(2, Position::new(200.0, 250.0));

    resnap_flow_endpoints(&mut state, &config);

    let flow = state
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if f.uid == 2 => Some(f),
            _ => None,
        })
        .unwrap();

    // Source endpoint should be at the stock's bottom edge
    let pt = &flow.points[0];
    assert!(
        (pt.x - 200.0).abs() < 0.01,
        "endpoint x should be at stock center x (200), got {}",
        pt.x
    );
    assert!(
        (pt.y - (100.0 + half_h)).abs() < 0.01,
        "endpoint y should be at stock bottom edge ({}), got {}",
        100.0 + half_h,
        pt.y
    );
}

// -- identify_new_elements tests --

/// Build a LayoutState with elements for stocks S1/S2 and aux A1,
/// then check that a model adding A2 and F1 identifies them correctly.
#[test]
fn test_identify_new_elements_partial_overlap() {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    // Register existing elements: stocks s1, s2 and aux a1
    state.uid_manager.add(1, "s1");
    state.uid_manager.add(2, "s2");
    state.uid_manager.add(3, "a1");

    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "s1".into(),
        uid: 1,
        x: 100.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "s2".into(),
        uid: 2,
        x: 200.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "a1".into(),
        uid: 3,
        x: 150.0,
        y: 50.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));

    // Model has s1, s2, a1 (existing) + a2 and f1 (new)
    let model = datamodel::Model {
        name: "test".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "s1".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["f1".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "s2".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "a1".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(3),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "a2".to_string(),
                equation: datamodel::Equation::Scalar("2".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "f1".to_string(),
                equation: datamodel::Equation::Scalar("a2".to_string()),
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
    };

    let result = state.identify_new_elements(&model);

    assert!(
        result.new_stocks.is_empty(),
        "s1 and s2 already exist, no new stocks"
    );
    assert_eq!(result.new_auxes, vec!["a2"]);
    assert_eq!(result.new_flows, vec!["f1"]);
    assert!(result.new_modules.is_empty());
}

/// When all model variables already have elements in state,
/// identify_new_elements should return empty lists.
#[test]
fn test_identify_new_elements_all_present() {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    state.uid_manager.add(1, "population");
    state.uid_manager.add(2, "growth");

    state.elements.push(ViewElement::Stock(view_element::Stock {
        name: "population".into(),
        uid: 1,
        x: 100.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "growth".into(),
        uid: 2,
        x: 50.0,
        y: 50.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));

    let model = datamodel::Model {
        name: "test".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "population".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "growth".to_string(),
                equation: datamodel::Equation::Scalar("0.05".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let result = state.identify_new_elements(&model);
    assert!(result.is_empty(), "all variables already present");
}

/// When the LayoutState is empty, every model variable should be new.
#[test]
fn test_identify_new_elements_empty_state() {
    let state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    let model = datamodel::Model {
        name: "test".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "susceptible".to_string(),
                equation: datamodel::Equation::Scalar("1000".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["infection".to_string()],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "infection".to_string(),
                equation: datamodel::Equation::Scalar("susceptible * contact_rate".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "contact_rate".to_string(),
                equation: datamodel::Equation::Scalar("0.05".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Module(datamodel::Module {
                ident: "vaccination".to_string(),
                model_name: "vaccination_model".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let result = state.identify_new_elements(&model);

    assert_eq!(result.new_stocks, vec!["susceptible"]);
    assert_eq!(result.new_flows, vec!["infection"]);
    assert_eq!(result.new_auxes, vec!["contact_rate"]);
    assert_eq!(result.new_modules, vec!["vaccination"]);
}

/// When a UID exists in uid_manager but no element in elements has that UID,
/// the variable should be treated as new.
#[test]
fn test_identify_new_elements_uid_exists_but_no_element() {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    // UID is registered but there is no corresponding ViewElement
    state.uid_manager.add(10, "orphan_aux");

    let model = datamodel::Model {
        name: "test".to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "orphan_aux".to_string(),
            equation: datamodel::Equation::Scalar("1".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(10),
        })],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let result = state.identify_new_elements(&model);
    assert_eq!(
        result.new_auxes,
        vec!["orphan_aux"],
        "should be new when UID exists but no element"
    );
}

// -- compute_new_element_positions tests --

/// Helper: build a LayoutState with positioned existing elements and a
/// ComputedMetadata with the given dep_graph.  Returns (state, metadata).
fn make_positioning_state(
    existing: &[(&str, f64, f64, &str)], // (ident, x, y, type: "stock"|"aux"|"flow")
    dep_graph: &[(&str, &[&str])],       // (var, deps_it_depends_on)
) -> (LayoutState, ComputedMetadata) {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    let mut next_uid = 1;
    for &(ident, x, y, kind) in existing {
        let uid = next_uid;
        next_uid += 1;
        state.uid_manager.add(uid, ident);
        state.positions.insert(uid, Position::new(x, y));

        let elem = match kind {
            "stock" => ViewElement::Stock(view_element::Stock {
                name: ident.into(),
                uid,
                x,
                y,
                label_side: LabelSide::Bottom,
                compat: None,
            }),
            "flow" => ViewElement::Flow(view_element::Flow {
                name: ident.into(),
                uid,
                x,
                y,
                label_side: LabelSide::Bottom,
                points: vec![
                    FlowPoint {
                        x: x - 50.0,
                        y,
                        attached_to_uid: None,
                    },
                    FlowPoint {
                        x: x + 50.0,
                        y,
                        attached_to_uid: None,
                    },
                ],
                compat: None,
                label_compat: None,
            }),
            _ => ViewElement::Aux(view_element::Aux {
                name: ident.into(),
                uid,
                x,
                y,
                label_side: LabelSide::Bottom,
                compat: None,
            }),
        };
        state.elements.push(elem);
    }

    let mut dg = BTreeMap::new();
    let mut rdg = BTreeMap::new();
    for &(var, deps) in dep_graph {
        let dep_set: BTreeSet<String> = deps.iter().map(|s| s.to_string()).collect();
        dg.insert(var.to_string(), dep_set);
        for dep in deps {
            rdg.entry(dep.to_string())
                .or_insert_with(BTreeSet::new)
                .insert(var.to_string());
        }
    }

    let metadata = ComputedMetadata {
        chains: Vec::new(),
        feedback_loops: Vec::new(),
        dominant_periods: Vec::new(),
        dep_graph: dg,
        reverse_dep_graph: rdg,
        constants: BTreeSet::new(),
        stock_to_inflows: HashMap::new(),
        stock_to_outflows: HashMap::new(),
        flow_to_stocks: HashMap::new(),
    };

    (state, metadata)
}

/// AC4.1: A new aux connected to two existing elements at (100,100)
/// and (200,200) is placed near (150,150).
#[test]
fn test_new_element_positioning_aux_near_connected() {
    let (state, metadata) = make_positioning_state(
        &[("a", 100.0, 100.0, "aux"), ("b", 200.0, 200.0, "aux")],
        // new_aux depends on a and b
        &[("new_aux", &["a", "b"])],
    );

    let new_elements = NewElements {
        new_stocks: Vec::new(),
        new_flows: Vec::new(),
        new_auxes: vec!["new_aux".to_string()],
        new_modules: Vec::new(),
    };

    let positions = compute_new_element_positions(&state, &metadata, &new_elements);
    let pos = positions
        .get("new_aux")
        .expect("new_aux should have a position");

    // Should be within a reasonable radius of centroid (150, 150)
    let dx = pos.x - 150.0;
    let dy = pos.y - 150.0;
    let dist = (dx * dx + dy * dy).sqrt();
    assert!(
        dist < 100.0,
        "new_aux should be near centroid of a and b, got ({}, {}), distance {}",
        pos.x,
        pos.y,
        dist
    );
}

/// AC4.2: A new stock-flow chain connected to existing structure is
/// positioned near the connected elements' centroid.
#[test]
fn test_new_element_positioning_chain_near_connected() {
    let (state, metadata) = make_positioning_state(
        &[("existing_stock", 300.0, 200.0, "stock")],
        // new_flow depends on existing_stock
        &[("new_flow", &["existing_stock"])],
    );

    let new_elements = NewElements {
        new_stocks: vec!["new_stock".to_string()],
        new_flows: vec!["new_flow".to_string()],
        new_auxes: Vec::new(),
        new_modules: Vec::new(),
    };

    let positions = compute_new_element_positions(&state, &metadata, &new_elements);

    // At least the new_flow should have a position near existing_stock
    let flow_pos = positions
        .get("new_flow")
        .expect("new_flow should have a position");
    let dx = flow_pos.x - 300.0;
    let dy = flow_pos.y - 200.0;
    let dist = (dx * dx + dy * dy).sqrt();
    assert!(
        dist < 300.0,
        "new_flow should be near existing_stock, got ({}, {}), distance {}",
        flow_pos.x,
        flow_pos.y,
        dist
    );
}

/// AC4.3: A new chain with no connections to existing structure is
/// placed at the diagram periphery (beyond the bounding box of
/// existing elements).
#[test]
fn test_new_element_positioning_chain_at_periphery() {
    let (state, metadata) = make_positioning_state(
        &[("s1", 100.0, 100.0, "stock"), ("s2", 300.0, 100.0, "stock")],
        // new_stock has no dependency edges to existing elements
        &[],
    );

    let new_elements = NewElements {
        new_stocks: vec!["new_stock".to_string()],
        new_flows: Vec::new(),
        new_auxes: Vec::new(),
        new_modules: Vec::new(),
    };

    let positions = compute_new_element_positions(&state, &metadata, &new_elements);

    let pos = positions
        .get("new_stock")
        .expect("new_stock should have a position");

    // Should be placed outside the bounding box of existing elements
    // Existing elements span x=[100,300], y=[100,100]
    // The new element should be beyond max_x (300)
    assert!(
        pos.x > 300.0 || pos.y > 100.0 || pos.x < 100.0 || pos.y < 100.0,
        "new_stock at ({}, {}) should be outside existing bounding box [100..300, 100..100]",
        pos.x,
        pos.y
    );
}

/// AC4.4: Two new auxes connected to the same existing element are
/// spread apart (not stacked at the same position).
#[test]
fn test_new_element_positioning_auxes_spread_apart() {
    let (state, metadata) = make_positioning_state(
        &[("target", 200.0, 200.0, "aux")],
        // Both new auxes depend on the same existing element
        &[("aux_a", &["target"]), ("aux_b", &["target"])],
    );

    let new_elements = NewElements {
        new_stocks: Vec::new(),
        new_flows: Vec::new(),
        new_auxes: vec!["aux_a".to_string(), "aux_b".to_string()],
        new_modules: Vec::new(),
    };

    let positions = compute_new_element_positions(&state, &metadata, &new_elements);

    let pos_a = positions
        .get("aux_a")
        .expect("aux_a should have a position");
    let pos_b = positions
        .get("aux_b")
        .expect("aux_b should have a position");

    let dx = pos_a.x - pos_b.x;
    let dy = pos_a.y - pos_b.y;
    let dist = (dx * dx + dy * dy).sqrt();
    assert!(
        dist > 10.0,
        "aux_a ({}, {}) and aux_b ({}, {}) should be spread apart, but distance is {}",
        pos_a.x,
        pos_a.y,
        pos_b.x,
        pos_b.y,
        dist
    );
}

/// New modules connected to existing elements are placed near them.
#[test]
fn test_new_element_positioning_module_near_connected() {
    let (state, metadata) = make_positioning_state(
        &[("existing_aux", 150.0, 150.0, "aux")],
        // new_module depends on existing_aux
        &[("new_module", &["existing_aux"])],
    );

    let new_elements = NewElements {
        new_stocks: Vec::new(),
        new_flows: Vec::new(),
        new_auxes: Vec::new(),
        new_modules: vec!["new_module".to_string()],
    };

    let positions = compute_new_element_positions(&state, &metadata, &new_elements);

    let pos = positions
        .get("new_module")
        .expect("new_module should have a position");
    let dx = pos.x - 150.0;
    let dy = pos.y - 150.0;
    let dist = (dx * dx + dy * dy).sqrt();
    assert!(
        dist < 200.0,
        "new_module should be near existing_aux, got ({}, {}), distance {}",
        pos.x,
        pos.y,
        dist
    );
}

/// Build a LayoutState and model suitable for testing settle_new_elements.
/// `existing` elements are present in both the state and model; `new_idents`
/// are in the model but not in the state.
fn make_settlement_scenario(
    existing: &[(&str, f64, f64, &str)], // (ident, x, y, "stock"|"aux"|"flow")
    new_vars: &[(&str, &str)],           // (ident, "stock"|"aux"|"flow")
    dep_graph: &[(&str, &[&str])],       // (var, deps_it_depends_on)
) -> (LayoutState, datamodel::Model, ComputedMetadata) {
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    let mut variables: Vec<datamodel::Variable> = Vec::new();
    let mut next_uid = 1;

    // Add existing elements to state and model
    for &(ident, x, y, kind) in existing {
        let uid = next_uid;
        next_uid += 1;
        state.uid_manager.add(uid, ident);
        state.positions.insert(uid, Position::new(x, y));
        state
            .display_names
            .insert(ident.to_string(), ident.to_string());

        let elem = match kind {
            "stock" => ViewElement::Stock(view_element::Stock {
                name: ident.into(),
                uid,
                x,
                y,
                label_side: LabelSide::Bottom,
                compat: None,
            }),
            "flow" => ViewElement::Flow(view_element::Flow {
                name: ident.into(),
                uid,
                x,
                y,
                label_side: LabelSide::Bottom,
                points: vec![
                    FlowPoint {
                        x: x - 50.0,
                        y,
                        attached_to_uid: None,
                    },
                    FlowPoint {
                        x: x + 50.0,
                        y,
                        attached_to_uid: None,
                    },
                ],
                compat: None,
                label_compat: None,
            }),
            _ => ViewElement::Aux(view_element::Aux {
                name: ident.into(),
                uid,
                x,
                y,
                label_side: LabelSide::Bottom,
                compat: None,
            }),
        };
        state.elements.push(elem);

        let var = match kind {
            "stock" => datamodel::Variable::Stock(datamodel::Stock {
                ident: ident.to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: Vec::new(),
                outflows: Vec::new(),
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(uid),
            }),
            "flow" => datamodel::Variable::Flow(datamodel::Flow {
                ident: ident.to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(uid),
            }),
            _ => datamodel::Variable::Aux(datamodel::Aux {
                ident: ident.to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(uid),
            }),
        };
        variables.push(var);
    }

    // Add new variables to model only (not to state)
    for &(ident, kind) in new_vars {
        let uid = next_uid;
        next_uid += 1;
        state
            .display_names
            .insert(ident.to_string(), ident.to_string());

        let var = match kind {
            "stock" => datamodel::Variable::Stock(datamodel::Stock {
                ident: ident.to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: Vec::new(),
                outflows: Vec::new(),
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(uid),
            }),
            "flow" => datamodel::Variable::Flow(datamodel::Flow {
                ident: ident.to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(uid),
            }),
            _ => datamodel::Variable::Aux(datamodel::Aux {
                ident: ident.to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(uid),
            }),
        };
        variables.push(var);
    }

    let model = datamodel::Model {
        name: "test".to_string(),
        sim_specs: None,
        variables,
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let mut dg = BTreeMap::new();
    let mut rdg = BTreeMap::new();
    for &(var, deps) in dep_graph {
        let dep_set: BTreeSet<String> = deps.iter().map(|s| s.to_string()).collect();
        dg.insert(var.to_string(), dep_set);
        for dep in deps {
            rdg.entry(dep.to_string())
                .or_insert_with(BTreeSet::new)
                .insert(var.to_string());
        }
    }

    let metadata = ComputedMetadata {
        chains: Vec::new(),
        feedback_loops: Vec::new(),
        dominant_periods: Vec::new(),
        dep_graph: dg,
        reverse_dep_graph: rdg,
        constants: BTreeSet::new(),
        stock_to_inflows: HashMap::new(),
        stock_to_outflows: HashMap::new(),
        flow_to_stocks: HashMap::new(),
    };

    (state, model, metadata)
}

/// AC1.2: After settlement, all pre-existing elements have identical
/// (x, y) coordinates to their pre-settlement positions.
#[test]
fn test_pinned_settlement_preserves_existing_positions() {
    let (mut state, model, metadata) = make_settlement_scenario(
        &[
            ("population", 200.0, 200.0, "stock"),
            ("birth_rate", 300.0, 100.0, "aux"),
            ("death_rate", 100.0, 300.0, "aux"),
        ],
        &[("vaccination_rate", "aux")],
        &[
            ("birth_rate", &["population"]),
            ("death_rate", &["population"]),
            ("vaccination_rate", &["population"]),
        ],
    );

    let new_elements = state.identify_new_elements(&model);
    assert_eq!(new_elements.new_auxes, vec!["vaccination_rate"]);

    // Compute initial positions for new elements and seed them into state
    let initial_positions = compute_new_element_positions(&state, &metadata, &new_elements);
    for (ident, pos) in &initial_positions {
        let uid = state.uid_manager.alloc(ident);
        state.positions.insert(uid, *pos);
    }

    // Snapshot existing element positions before settlement
    let existing_idents = ["population", "birth_rate", "death_rate"];
    let pre_positions: HashMap<String, Position> = existing_idents
        .iter()
        .filter_map(|&ident| {
            let uid = state.uid_manager.get_uid(ident)?;
            let pos = state.positions.get(&uid)?;
            Some((ident.to_string(), *pos))
        })
        .collect();

    let config = LayoutConfig::default();
    let chains_data: Vec<(Vec<String>, Vec<String>, Vec<String>)> = Vec::new();

    settle_new_elements(
        &mut state,
        &config,
        &model,
        &metadata,
        &new_elements,
        &chains_data,
    )
    .expect("settle_new_elements should succeed");

    // Verify all existing element positions are preserved exactly
    for (ident, pre_pos) in &pre_positions {
        let uid = state
            .uid_manager
            .get_uid(ident)
            .expect("existing element should have uid");
        let post_pos = state
            .positions
            .get(&uid)
            .expect("existing element should still have position");
        assert!(
            (post_pos.x - pre_pos.x).abs() < 1e-10 && (post_pos.y - pre_pos.y).abs() < 1e-10,
            "existing element '{}' position changed: ({}, {}) -> ({}, {})",
            ident,
            pre_pos.x,
            pre_pos.y,
            post_pos.x,
            post_pos.y,
        );
    }
}

/// AC5.4: New elements have settled positions that differ from their
/// raw initial placement (SFDP/annealing moved them).
///
/// We place the new element at a position intentionally far from its
/// equilibrium (near connected existing elements) so the force-directed
/// algorithm must move it during settlement.
#[test]
fn test_pinned_settlement_moves_new_elements() {
    let (mut state, model, metadata) = make_settlement_scenario(
        &[
            ("population", 200.0, 200.0, "stock"),
            ("birth_rate", 300.0, 100.0, "aux"),
            ("death_rate", 100.0, 300.0, "aux"),
        ],
        &[("vaccination_rate", "aux")],
        &[
            ("birth_rate", &["population"]),
            ("death_rate", &["population"]),
            ("vaccination_rate", &["population", "birth_rate"]),
        ],
    );

    let new_elements = state.identify_new_elements(&model);

    // Seed the new element at a position far from its equilibrium:
    // its connections are at (200,200) and (300,100), centroid ~ (250, 150),
    // but we deliberately place it far away so SFDP forces will pull it.
    let far_initial = Position::new(600.0, 600.0);
    let vr_uid = state.uid_manager.alloc("vaccination_rate");
    state.positions.insert(vr_uid, far_initial);

    let config = LayoutConfig::default();
    let chains_data: Vec<(Vec<String>, Vec<String>, Vec<String>)> = Vec::new();

    settle_new_elements(
        &mut state,
        &config,
        &model,
        &metadata,
        &new_elements,
        &chains_data,
    )
    .expect("settle_new_elements should succeed");

    let final_pos = state
        .positions
        .get(&vr_uid)
        .expect("vaccination_rate should have final position");

    assert!(
        final_pos.x.is_finite() && final_pos.y.is_finite(),
        "vaccination_rate should have finite position, got ({}, {})",
        final_pos.x,
        final_pos.y,
    );

    // SFDP should have moved the new element significantly toward its
    // connected nodes, away from the far initial placement at (600, 600).
    let moved =
        (final_pos.x - far_initial.x).abs() > 1.0 || (final_pos.y - far_initial.y).abs() > 1.0;
    assert!(
        moved,
        "vaccination_rate should have moved from initial ({}, {}) to ({}, {})",
        far_initial.x, far_initial.y, final_pos.x, final_pos.y,
    );
}

/// Settlement with no new elements is a no-op.
#[test]
fn test_pinned_settlement_noop_when_no_new_elements() {
    let (mut state, model, metadata) = make_settlement_scenario(
        &[
            ("population", 200.0, 200.0, "stock"),
            ("birth_rate", 300.0, 100.0, "aux"),
        ],
        &[],
        &[("birth_rate", &["population"])],
    );

    let new_elements = NewElements {
        new_stocks: Vec::new(),
        new_flows: Vec::new(),
        new_auxes: Vec::new(),
        new_modules: Vec::new(),
    };

    let pre_positions = state.positions.clone();
    let config = LayoutConfig::default();
    let chains_data: Vec<(Vec<String>, Vec<String>, Vec<String>)> = Vec::new();

    settle_new_elements(
        &mut state,
        &config,
        &model,
        &metadata,
        &new_elements,
        &chains_data,
    )
    .expect("settle_new_elements should succeed");

    assert_eq!(
        state.positions, pre_positions,
        "positions should be unchanged when there are no new elements"
    );
}

/// Settlement with multiple new elements preserves existing and settles all new.
#[test]
fn test_pinned_settlement_multiple_new_elements() {
    let (mut state, model, metadata) = make_settlement_scenario(
        &[("center", 200.0, 200.0, "aux")],
        &[("new_a", "aux"), ("new_b", "aux")],
        &[("new_a", &["center"]), ("new_b", &["center"])],
    );

    let new_elements = state.identify_new_elements(&model);
    assert_eq!(new_elements.new_auxes.len(), 2);

    let initial_positions = compute_new_element_positions(&state, &metadata, &new_elements);
    for (ident, pos) in &initial_positions {
        let uid = state.uid_manager.alloc(ident);
        state.positions.insert(uid, *pos);
    }

    let pre_center_pos = *state
        .positions
        .get(&state.uid_manager.get_uid("center").unwrap())
        .unwrap();

    let config = LayoutConfig::default();
    let chains_data: Vec<(Vec<String>, Vec<String>, Vec<String>)> = Vec::new();

    settle_new_elements(
        &mut state,
        &config,
        &model,
        &metadata,
        &new_elements,
        &chains_data,
    )
    .expect("settle_new_elements should succeed");

    // Existing position preserved
    let post_center_pos = *state
        .positions
        .get(&state.uid_manager.get_uid("center").unwrap())
        .unwrap();
    assert!(
        (post_center_pos.x - pre_center_pos.x).abs() < 1e-10
            && (post_center_pos.y - pre_center_pos.y).abs() < 1e-10,
        "existing center position changed"
    );

    // Both new elements have valid positions
    for ident in &["new_a", "new_b"] {
        let uid = state.uid_manager.get_uid(ident).expect("should have uid");
        let pos = state
            .positions
            .get(&uid)
            .expect("should have final position");
        assert!(
            pos.x.is_finite() && pos.y.is_finite(),
            "{} should have finite position",
            ident
        );
    }
}

#[path = "layout_review_tests.rs"]
mod review_tests;
