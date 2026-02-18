// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;

use simlin_engine::common::canonicalize;
use simlin_engine::datamodel::ViewElement;
use simlin_engine::layout::config::LayoutConfig;
use simlin_engine::layout::{generate_best_layout, generate_layout, generate_layout_with_config};
use simlin_engine::open_xmile;

/// The main model name in single-model XMILE files is the empty string;
/// `Project::get_model` maps "main" to the first model when the name is empty.
const MAIN_MODEL: &str = "main";

fn load_project(path: &str) -> simlin_engine::datamodel::Project {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("could not determine repo root");
    let file_path = repo_root.join(path);
    let file = File::open(&file_path)
        .unwrap_or_else(|e| panic!("failed to open {}: {}", file_path.display(), e));
    let mut reader = BufReader::new(file);
    open_xmile(&mut reader).unwrap_or_else(|e| {
        panic!("failed to parse {}: {:?}", path, e);
    })
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
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let view = generate_layout(&project, MAIN_MODEL).expect("layout generation should succeed");
    verify_layout(&view, model, "SIR");

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
    let project = load_project("test/test-models/samples/teacup/teacup.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let view = generate_layout(&project, MAIN_MODEL).expect("layout generation should succeed");
    verify_layout(&view, model, "teacup");

    let stock_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Stock(_)))
        .count();
    assert_eq!(stock_count, 1, "teacup should have 1 stock");
}

#[test]
fn test_layout_logistic_growth() {
    let project = load_project("test/logistic_growth_ltm/logistic_growth.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let view = generate_layout(&project, MAIN_MODEL).expect("layout generation should succeed");
    verify_layout(&view, model, "logistic_growth");
}

#[test]
fn test_layout_arms_race() {
    let project = load_project("test/arms_race_3party/arms_race.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let view = generate_layout(&project, MAIN_MODEL).expect("layout generation should succeed");
    verify_layout(&view, model, "arms_race");

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
    let project = load_project("test/decoupled_stocks/decoupled.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let view = generate_layout(&project, MAIN_MODEL).expect("layout generation should succeed");
    verify_layout(&view, model, "decoupled");

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
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let view1 = generate_layout(&project, MAIN_MODEL).expect("first layout should succeed");
    let view2 = generate_layout(&project, MAIN_MODEL).expect("second layout should succeed");

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
    let project = load_project("test/test-models/samples/teacup/teacup.stmx");
    let view = generate_layout(&project, MAIN_MODEL).expect("layout generation should succeed");

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
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let view = generate_layout(&project, MAIN_MODEL).expect("layout generation should succeed");

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
fn test_best_layout_sir() {
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let view =
        generate_best_layout(&project, MAIN_MODEL).expect("best layout generation should succeed");
    verify_layout(&view, model, "SIR_best");
}

#[test]
fn test_layout_link_uids_reference_existing_elements() {
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let view = generate_layout(&project, MAIN_MODEL).expect("layout generation should succeed");

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

#[test]
fn test_generate_layout_with_zero_reheat_does_not_panic() {
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let config = LayoutConfig {
        annealing_reheat_period: 0,
        annealing_iterations: 0,
        annealing_interval: 0,
        annealing_max_rounds: 0,
        ..LayoutConfig::default()
    };
    let view = generate_layout_with_config(&project, MAIN_MODEL, config)
        .expect("layout should succeed with zero config");
    verify_layout(&view, model, "zero_reheat");
}

#[test]
fn test_ltm_populates_loop_importance() {
    use simlin_engine::layout::compute_metadata;

    // The logistic growth model has known feedback loops; LTM should
    // populate importance_series for at least one.
    let project = load_project("test/logistic_growth_ltm/logistic_growth.stmx");
    let metadata = compute_metadata(&project, MAIN_MODEL).unwrap();

    assert!(
        !metadata.feedback_loops.is_empty(),
        "logistic growth model should have at least one detected feedback loop"
    );

    let has_importance = metadata
        .feedback_loops
        .iter()
        .any(|fl| !fl.importance_series.is_empty());
    assert!(
        has_importance,
        "at least one feedback loop should have a non-empty importance series"
    );
}

#[test]
fn test_ltm_detects_polarity() {
    use simlin_engine::layout::compute_metadata;
    use simlin_engine::layout::metadata::LoopPolarity;

    let project = load_project("test/logistic_growth_ltm/logistic_growth.stmx");
    let metadata = compute_metadata(&project, MAIN_MODEL).unwrap();

    // The logistic growth model should have both reinforcing and balancing loops.
    let has_reinforcing = metadata
        .feedback_loops
        .iter()
        .any(|fl| fl.polarity == LoopPolarity::Reinforcing);
    let has_balancing = metadata
        .feedback_loops
        .iter()
        .any(|fl| fl.polarity == LoopPolarity::Balancing);
    assert!(
        has_reinforcing || has_balancing,
        "LTM should detect at least one loop with a definite polarity"
    );
}

#[test]
fn test_loops_sorted_by_average_importance() {
    use simlin_engine::layout::compute_metadata;

    let project = load_project("test/logistic_growth_ltm/logistic_growth.stmx");
    let metadata = compute_metadata(&project, MAIN_MODEL).unwrap();

    // Verify descending sort by average importance.
    for pair in metadata.feedback_loops.windows(2) {
        assert!(
            pair[0].average_importance() >= pair[1].average_importance(),
            "feedback loops should be sorted descending by average importance: {} >= {}",
            pair[0].average_importance(),
            pair[1].average_importance(),
        );
    }
}

#[test]
fn test_compute_layout_metadata_has_dominant_periods() {
    use simlin_engine::layout::compute_layout_metadata;

    let project = load_project("test/logistic_growth_ltm/logistic_growth.stmx");
    let metadata = compute_layout_metadata(&project, MAIN_MODEL).unwrap();

    assert!(
        !metadata.feedback_loops.is_empty(),
        "logistic growth should have feedback loops"
    );
    assert!(
        !metadata.dominant_periods.is_empty(),
        "logistic growth should have dominant periods computed from LTM"
    );

    // Each dominant period should have valid time bounds
    for period in &metadata.dominant_periods {
        assert!(
            period.end >= period.start,
            "dominant period end ({}) should be >= start ({})",
            period.end,
            period.start,
        );
        assert!(
            !period.dominant_loops.is_empty(),
            "dominant period should have at least one dominant loop"
        );
        assert!(
            period.combined_score > 0.0,
            "dominant period combined_score should be positive"
        );
    }
}

#[test]
fn test_compute_layout_metadata_chains_sorted() {
    use simlin_engine::layout::compute_layout_metadata;

    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let metadata = compute_layout_metadata(&project, MAIN_MODEL).unwrap();

    assert!(
        !metadata.chains.is_empty(),
        "SIR should have stock-flow chains"
    );

    // Chains should be sorted by importance descending
    for pair in metadata.chains.windows(2) {
        assert!(
            pair[0].importance >= pair[1].importance,
            "chains should be sorted descending by importance: {} >= {}",
            pair[0].importance,
            pair[1].importance,
        );
    }
}

#[test]
fn test_compute_layout_metadata_dep_graph() {
    use simlin_engine::layout::compute_layout_metadata;

    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let metadata = compute_layout_metadata(&project, MAIN_MODEL).unwrap();

    assert!(
        !metadata.dep_graph.is_empty(),
        "SIR should have a non-empty dependency graph"
    );
    assert!(
        !metadata.reverse_dep_graph.is_empty(),
        "SIR should have a non-empty reverse dependency graph"
    );
}
