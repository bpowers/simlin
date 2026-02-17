// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;

use simlin_engine::common::canonicalize;
use simlin_engine::datamodel::ViewElement;
use simlin_engine::layout::generate_layout;
use simlin_engine::open_xmile;

fn load_model(path: &str) -> simlin_engine::datamodel::Model {
    let file_path = format!("../../{}", path);
    let file =
        File::open(&file_path).unwrap_or_else(|e| panic!("failed to open {}: {}", file_path, e));
    let mut reader = BufReader::new(file);
    let project = open_xmile(&mut reader).unwrap_or_else(|e| {
        panic!("failed to parse {}: {:?}", path, e);
    });
    project
        .models
        .into_iter()
        .next()
        .expect("project should have at least one model")
}

/// Shared verification for all layout results.
fn verify_layout(
    view: &simlin_engine::datamodel::StockFlow,
    model: &simlin_engine::datamodel::Model,
    label: &str,
) {
    // Every model variable should have a corresponding view element.
    // View element names may be line-break-formatted, so canonicalize both
    // sides (replace newlines/underscores/spaces and lowercase) for matching.
    let normalize = |s: &str| -> String { canonicalize(&s.replace('\n', "_")).into_owned() };

    let view_names: HashSet<String> = view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some(normalize(&s.name)),
            ViewElement::Flow(f) => Some(normalize(&f.name)),
            ViewElement::Aux(a) => Some(normalize(&a.name)),
            _ => None,
        })
        .collect();

    for var in &model.variables {
        let canonical = canonicalize(var.get_ident()).into_owned();
        assert!(
            view_names.contains(&canonical),
            "[{}] variable '{}' (canonical: '{}') missing from layout view elements",
            label,
            var.get_ident(),
            canonical,
        );
    }

    // Element count should be >= variable count (variables + links + clouds)
    assert!(
        view.elements.len() >= model.variables.len(),
        "[{}] expected at least {} elements, got {}",
        label,
        model.variables.len(),
        view.elements.len(),
    );

    // All UIDs should be unique
    let mut uids = HashSet::new();
    for elem in &view.elements {
        let uid = elem.get_uid();
        assert!(uids.insert(uid), "[{}] duplicate UID {} found", label, uid,);
    }

    // All coordinates should be positive (after normalization)
    for elem in &view.elements {
        match elem {
            ViewElement::Stock(s) => {
                assert!(
                    s.x > 0.0,
                    "[{}] stock '{}' has non-positive x: {}",
                    label,
                    s.name,
                    s.x
                );
                assert!(
                    s.y > 0.0,
                    "[{}] stock '{}' has non-positive y: {}",
                    label,
                    s.name,
                    s.y
                );
            }
            ViewElement::Flow(f) => {
                assert!(
                    f.x > 0.0,
                    "[{}] flow '{}' has non-positive x: {}",
                    label,
                    f.name,
                    f.x
                );
                assert!(
                    f.y > 0.0,
                    "[{}] flow '{}' has non-positive y: {}",
                    label,
                    f.name,
                    f.y
                );
            }
            ViewElement::Aux(a) => {
                assert!(
                    a.x > 0.0,
                    "[{}] aux '{}' has non-positive x: {}",
                    label,
                    a.name,
                    a.x
                );
                assert!(
                    a.y > 0.0,
                    "[{}] aux '{}' has non-positive y: {}",
                    label,
                    a.name,
                    a.y
                );
            }
            ViewElement::Cloud(c) => {
                assert!(
                    c.x > 0.0,
                    "[{}] cloud uid={} has non-positive x: {}",
                    label,
                    c.uid,
                    c.x
                );
                assert!(
                    c.y > 0.0,
                    "[{}] cloud uid={} has non-positive y: {}",
                    label,
                    c.uid,
                    c.y
                );
            }
            ViewElement::Link(_)
            | ViewElement::Module(_)
            | ViewElement::Alias(_)
            | ViewElement::Group(_) => {}
        }
    }

    // ViewBox should encompass all elements
    let vb = &view.view_box;
    for elem in &view.elements {
        let (x, y) = match elem {
            ViewElement::Stock(s) => (s.x, s.y),
            ViewElement::Flow(f) => (f.x, f.y),
            ViewElement::Aux(a) => (a.x, a.y),
            ViewElement::Cloud(c) => (c.x, c.y),
            ViewElement::Link(_)
            | ViewElement::Module(_)
            | ViewElement::Alias(_)
            | ViewElement::Group(_) => continue,
        };
        assert!(
            x >= vb.x && x <= vb.x + vb.width,
            "[{}] element at x={} outside viewbox ({}, {})",
            label,
            x,
            vb.x,
            vb.x + vb.width,
        );
        assert!(
            y >= vb.y && y <= vb.y + vb.height,
            "[{}] element at y={} outside viewbox ({}, {})",
            label,
            y,
            vb.y,
            vb.y + vb.height,
        );
    }

    // Zoom should be 1.0
    assert!(
        (view.zoom - 1.0).abs() < f64::EPSILON,
        "[{}] expected zoom 1.0, got {}",
        label,
        view.zoom,
    );
}

#[test]
fn test_layout_sir() {
    let model = load_model("test/test-models/samples/SIR/SIR.stmx");
    let view = generate_layout(&model).expect("layout generation should succeed");
    verify_layout(&view, &model, "SIR");

    // SIR should have 3 stocks, 2 flows, 3 auxes = 8 variables minimum
    let stock_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Stock(_)))
        .count();
    let flow_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Flow(_)))
        .count();
    assert_eq!(stock_count, 3, "SIR should have 3 stocks");
    assert_eq!(flow_count, 2, "SIR should have 2 flows");
}

#[test]
fn test_layout_teacup() {
    let model = load_model("test/test-models/samples/teacup/teacup.stmx");
    let view = generate_layout(&model).expect("layout generation should succeed");
    verify_layout(&view, &model, "teacup");

    let stock_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Stock(_)))
        .count();
    assert_eq!(stock_count, 1, "teacup should have 1 stock");
}

#[test]
fn test_layout_logistic_growth() {
    let model = load_model("test/logistic_growth_ltm/logistic_growth.stmx");
    let view = generate_layout(&model).expect("layout generation should succeed");
    verify_layout(&view, &model, "logistic_growth");
}

#[test]
fn test_layout_arms_race() {
    let model = load_model("test/arms_race_3party/arms_race.stmx");
    let view = generate_layout(&model).expect("layout generation should succeed");
    verify_layout(&view, &model, "arms_race");

    // Should have 3 stocks and 3 flows
    let stock_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Stock(_)))
        .count();
    let flow_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Flow(_)))
        .count();
    assert_eq!(stock_count, 3, "arms_race should have 3 stocks");
    assert_eq!(flow_count, 3, "arms_race should have 3 flows");
}

#[test]
fn test_layout_decoupled_stocks() {
    let model = load_model("test/decoupled_stocks/decoupled.stmx");
    let view = generate_layout(&model).expect("layout generation should succeed");
    verify_layout(&view, &model, "decoupled");

    // Should have 2 stocks in separate chains
    let stock_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Stock(_)))
        .count();
    assert_eq!(stock_count, 2, "decoupled should have 2 stocks");
}

#[test]
fn test_layout_structural_consistency() {
    let model = load_model("test/test-models/samples/SIR/SIR.stmx");
    let view1 = generate_layout(&model).expect("first layout should succeed");
    let view2 = generate_layout(&model).expect("second layout should succeed");

    assert_eq!(
        view1.elements.len(),
        view2.elements.len(),
        "repeated layout should produce same element count"
    );

    // Verify same element types in same order
    let types1: Vec<&str> = view1
        .elements
        .iter()
        .map(|e| match e {
            ViewElement::Stock(_) => "stock",
            ViewElement::Flow(_) => "flow",
            ViewElement::Aux(_) => "aux",
            ViewElement::Cloud(_) => "cloud",
            ViewElement::Link(_) => "link",
            _ => "other",
        })
        .collect();
    let types2: Vec<&str> = view2
        .elements
        .iter()
        .map(|e| match e {
            ViewElement::Stock(_) => "stock",
            ViewElement::Flow(_) => "flow",
            ViewElement::Aux(_) => "aux",
            ViewElement::Cloud(_) => "cloud",
            ViewElement::Link(_) => "link",
            _ => "other",
        })
        .collect();
    assert_eq!(types1, types2, "element type ordering should be consistent");
}

#[test]
fn test_layout_flow_points_have_cloud_attachment() {
    let model = load_model("test/test-models/samples/teacup/teacup.stmx");
    let view = generate_layout(&model).expect("layout generation should succeed");

    // Teacup has 1 flow with 1 stock attached. The other end should have a cloud.
    let clouds: Vec<_> = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(_)))
        .collect();
    assert!(
        !clouds.is_empty(),
        "teacup should have at least one cloud (flow endpoint without stock)"
    );

    // Flow endpoints should have attached_to_uid set for at least one point
    for elem in &view.elements {
        if let ViewElement::Flow(flow) = elem {
            let has_attachment = flow.points.iter().any(|pt| pt.attached_to_uid.is_some());
            assert!(
                has_attachment,
                "flow '{}' should have at least one point attached to a stock or cloud",
                flow.name,
            );
        }
    }
}

#[test]
fn test_layout_connectors_present() {
    let model = load_model("test/test-models/samples/SIR/SIR.stmx");
    let view = generate_layout(&model).expect("layout generation should succeed");

    let link_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Link(_)))
        .count();
    assert!(
        link_count > 0,
        "SIR model should have dependency connectors (links)"
    );
}

#[test]
fn test_layout_link_uids_reference_existing_elements() {
    let model = load_model("test/test-models/samples/SIR/SIR.stmx");
    let view = generate_layout(&model).expect("layout generation should succeed");

    let all_uids: HashSet<i32> = view.elements.iter().map(|e| e.get_uid()).collect();

    for elem in &view.elements {
        if let ViewElement::Link(link) = elem {
            assert!(
                all_uids.contains(&link.from_uid),
                "link from_uid {} not found in element UIDs",
                link.from_uid,
            );
            assert!(
                all_uids.contains(&link.to_uid),
                "link to_uid {} not found in element UIDs",
                link.to_uid,
            );
        }
    }
}
