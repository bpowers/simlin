// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;

use simlin_engine::common::canonicalize;
use simlin_engine::datamodel::ViewElement;
use simlin_engine::db::{SimlinDb, sync_from_datamodel_incremental};
use simlin_engine::layout::LayoutState;
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
            ViewElement::Module(m) => Some(normalize(&m.name)),
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
            ViewElement::Module(m) => {
                assert!(
                    m.x > 0.0,
                    "[{}] module '{}' has non-positive x: {}",
                    label,
                    m.name,
                    m.x
                );
                assert!(
                    m.y > 0.0,
                    "[{}] module '{}' has non-positive y: {}",
                    label,
                    m.name,
                    m.y
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
            ViewElement::Link(_) | ViewElement::Alias(_) | ViewElement::Group(_) => {}
        }
    }

    // ViewBox should encompass all elements
    let vb = &view.view_box;
    for elem in &view.elements {
        let (x, y) = match elem {
            ViewElement::Stock(s) => (s.x, s.y),
            ViewElement::Flow(f) => (f.x, f.y),
            ViewElement::Aux(a) => (a.x, a.y),
            ViewElement::Module(m) => (m.x, m.y),
            ViewElement::Cloud(c) => (c.x, c.y),
            ViewElement::Link(_) | ViewElement::Alias(_) | ViewElement::Group(_) => continue,
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
    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");
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
    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");
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
    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");
    verify_layout(&view, model, "logistic_growth");
}

#[test]
fn test_layout_arms_race() {
    let project = load_project("test/arms_race_3party/arms_race.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");
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
    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");
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
    let view1 = generate_layout(&project, MAIN_MODEL, None).expect("first layout should succeed");
    let view2 = generate_layout(&project, MAIN_MODEL, None).expect("second layout should succeed");

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
    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");

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
    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");

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
    let view = generate_best_layout(&project, MAIN_MODEL, None)
        .expect("best layout generation should succeed");
    verify_layout(&view, model, "SIR_best");
}

#[test]
fn test_layout_link_uids_reference_existing_elements() {
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");

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
    let view = generate_layout_with_config(&project, MAIN_MODEL, config, None)
        .expect("layout should succeed with zero config");
    verify_layout(&view, model, "zero_reheat");
}

#[test]
fn test_ltm_populates_loop_importance() {
    use simlin_engine::layout::compute_metadata;

    // The logistic growth model has known feedback loops; LTM should
    // populate importance_series for at least one.
    let project = load_project("test/logistic_growth_ltm/logistic_growth.stmx");
    let metadata = compute_metadata(&project, MAIN_MODEL, None).unwrap();

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
    let metadata = compute_metadata(&project, MAIN_MODEL, None).unwrap();

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
    let metadata = compute_metadata(&project, MAIN_MODEL, None).unwrap();

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
    let metadata = compute_layout_metadata(&project, MAIN_MODEL, None).unwrap();

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
    let metadata = compute_layout_metadata(&project, MAIN_MODEL, None).unwrap();

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
    let metadata = compute_layout_metadata(&project, MAIN_MODEL, None).unwrap();

    assert!(
        !metadata.dep_graph.is_empty(),
        "SIR should have a non-empty dependency graph"
    );
    assert!(
        !metadata.reverse_dep_graph.is_empty(),
        "SIR should have a non-empty reverse dependency graph"
    );
}

/// Regression test: when save_step < dt, dominant period timestamps must still
/// use the effective save cadence (dt, not save_step). Without this fix, period
/// timestamps would be compressed (e.g. 0..2.5 instead of 0..10) because the
/// code used the raw save_step as the series spacing.
#[test]
fn test_dominant_period_timestamps_respect_effective_save_cadence() {
    use simlin_engine::layout::compute_metadata;

    // save_step (0.25) < dt (1), so effective cadence should be dt (1.0).
    let xmile = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><vendor>test</vendor><product version="1.0">test</product></header>
  <sim_specs>
    <start>0</start><stop>10</stop><dt>1</dt>
    <save_step>0.25</save_step>
  </sim_specs>
  <model>
    <variables>
      <stock name="population">
        <eqn>100</eqn>
        <inflow>births</inflow>
      </stock>
      <flow name="births">
        <eqn>population * growth_rate</eqn>
      </flow>
      <aux name="growth_rate">
        <eqn>0.1</eqn>
      </aux>
    </variables>
  </model>
</xmile>"#;

    let mut reader = std::io::BufReader::new(xmile.as_bytes());
    let project = simlin_engine::open_xmile(&mut reader).unwrap();
    let metadata = compute_metadata(&project, MAIN_MODEL, None).unwrap();

    let sim_stop = 10.0_f64;
    for period in &metadata.dominant_periods {
        assert!(
            period.end <= sim_stop,
            "dominant period end ({}) should not exceed simulation stop ({}); \
             save_step < dt should use dt as effective cadence",
            period.end,
            sim_stop,
        );
    }
}

/// Regression test: AST dependency extraction must not produce self-edges.
/// A stock whose init expression references itself (e.g. `population * 0.5`)
/// would create a self-loop in the dep graph without the self-reference filter.
#[test]
fn test_dep_graph_excludes_self_references() {
    use simlin_engine::layout::compute_layout_metadata;
    use std::io::BufReader;

    let xmile = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><vendor>test</vendor><product version="1.0">test</product></header>
  <sim_specs><start>0</start><stop>10</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <stock name="population">
        <eqn>population * 0.5</eqn>
        <inflow>births</inflow>
      </stock>
      <flow name="births">
        <eqn>population * growth_rate</eqn>
      </flow>
      <aux name="growth_rate">
        <eqn>0.1</eqn>
      </aux>
    </variables>
  </model>
</xmile>"#;

    let mut reader = BufReader::new(xmile.as_bytes());
    let project = simlin_engine::open_xmile(&mut reader).unwrap();
    let metadata = compute_layout_metadata(&project, MAIN_MODEL, None).unwrap();

    for (var, deps) in &metadata.dep_graph {
        assert!(
            !deps.contains(var),
            "dep_graph has self-edge: '{}' -> '{}'",
            var,
            var,
        );
    }

    // The stock's init references itself, but the dep graph should still
    // contain the stock with its flow dependencies (from structural edges).
    assert!(
        metadata.dep_graph.contains_key("population"),
        "population should be in dep_graph"
    );
    let pop_deps = &metadata.dep_graph["population"];
    assert!(
        pop_deps.contains("births"),
        "population should depend on its inflow 'births'"
    );
}

/// Constants must have zero deps in the dep graph when the salsa path is used.
/// Previously the heuristic fallback would run whenever salsa deps were empty,
/// potentially adding spurious edges for constant variables.
#[test]
fn test_constant_variable_has_no_deps_with_salsa() {
    use simlin_engine::layout::compute_layout_metadata;
    use std::io::BufReader;

    // "capacity" is the constant name, and "cap" appears as a substring
    // in variable name "capacity_factor". The heuristic fallback might
    // match tokens from the equation text against variable names, but the
    // salsa path should correctly return empty deps for a constant.
    let xmile = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><vendor>test</vendor><product version="1.0">test</product></header>
  <sim_specs><start>0</start><stop>10</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <aux name="capacity">
        <eqn>100</eqn>
      </aux>
      <aux name="growth_rate">
        <eqn>0.05</eqn>
      </aux>
      <stock name="population">
        <eqn>10</eqn>
        <inflow>births</inflow>
      </stock>
      <flow name="births">
        <eqn>population * growth_rate * (1 - population / capacity)</eqn>
      </flow>
    </variables>
  </model>
</xmile>"#;

    let mut reader = BufReader::new(xmile.as_bytes());
    let project = simlin_engine::open_xmile(&mut reader).unwrap();

    let mut db = SimlinDb::default();
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let source_project = state.to_sync_result().project;

    let metadata =
        compute_layout_metadata(&project, MAIN_MODEL, Some((&mut db, source_project))).unwrap();

    // Constants should have no equation deps (structural edges may still exist).
    let capacity_deps = metadata
        .dep_graph
        .get("capacity")
        .cloned()
        .unwrap_or_default();
    assert!(
        capacity_deps.is_empty(),
        "constant 'capacity' should have no deps, got: {:?}",
        capacity_deps
    );
    let growth_rate_deps = metadata
        .dep_graph
        .get("growth_rate")
        .cloned()
        .unwrap_or_default();
    assert!(
        growth_rate_deps.is_empty(),
        "constant 'growth_rate' should have no deps, got: {:?}",
        growth_rate_deps
    );
}

/// Verify that ltm_enabled is reset to false after compute_metadata
/// completes, even when the incremental LTM path encounters failures
/// (e.g., compilation or simulation errors).
#[test]
fn test_ltm_enabled_reset_after_incremental_metadata() {
    use simlin_engine::layout::compute_metadata;

    let project = load_project("test/logistic_growth_ltm/logistic_growth.stmx");
    let mut db = SimlinDb::default();
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let source_project = state.to_sync_result().project;

    // Call compute_metadata with the incremental salsa path.
    let metadata = compute_metadata(&project, MAIN_MODEL, Some((&mut db, source_project)));
    assert!(
        metadata.is_some(),
        "metadata should be returned for valid project"
    );

    // ltm_enabled should be reset to false after the call.
    assert!(
        !source_project.ltm_enabled(&db),
        "ltm_enabled should be false after compute_metadata completes"
    );
}

/// Codex review regression (PR #472): every detected loop's
/// `importance_series` must have length exactly `results.step_count`,
/// regardless of the partition stride that
/// `compute_rel_loop_scores_per_element` happens to write its output
/// at.  Pre-fix, the layout divided `series.len()` by the loop's own
/// `n_slots` to derive `n_steps`, which silently produced
/// `step_count * stride`-long importance_series whenever the
/// helper's stride exceeded the loop's own slot count.
///
/// Note on coverage: at the time of writing, the engine's partition
/// logic uses *element-level* stock SCCs in `model_element_cycle_partitions`
/// while A2A loops carry *variable-level* stock names from
/// `find_stocks_in_loop`, so `partition_for_loop` returns `None` for
/// every A2A loop and they end up in the unkeyed partition by
/// themselves.  As a result, no current engine pipeline configuration
/// produces a mixed-stride partition (one with both `n=1` scalar and
/// `n>1` arrayed loops); the bug only manifests if/when that engine
/// quirk is fixed.  The aggregation helper's unit tests in
/// `ltm_post::tests` cover the algorithm directly with hand-crafted
/// inputs that DO simulate mixed-stride partitions, which is the
/// authoritative regression coverage.
///
/// This test stands as forward-looking belt-and-suspenders: it
/// verifies the layout's importance_series length contract end-to-end
/// against a model whose SCC structure is the closest current-engine
/// approximation to a mixed partition (arrayed and scalar loops
/// connected through a scalar buffer stock).  If the engine partition
/// logic is later unified, this fixture should immediately start
/// producing mixed-stride partitions and the test will keep the
/// layout aggregation honest.
#[test]
fn test_compute_metadata_importance_series_length_matches_step_count() {
    use simlin_engine::Vm;
    use simlin_engine::db::{compile_project_incremental, set_project_ltm_enabled};
    use simlin_engine::layout::compute_metadata;
    use simlin_engine::test_common::TestProject;

    let project = TestProject::new("mixed_partition_proxy")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_with_ranges(
            "birth_rate[Region]",
            vec![("NYC", "0.05"), ("Boston", "0.20")],
        )
        .array_stock("population[Region]", "100", &["adjusted_births"], &[], None)
        .array_aux("region_births[Region]", "population * birth_rate")
        .scalar_aux("total_pop", "SUM(population[*])")
        .scalar_aux("buffer_inflow", "total_pop * 0.001")
        .stock("buffer", "0", &["buffer_inflow"], &["buffer_outflow"], None)
        .scalar_aux("buffer_outflow", "buffer * 0.005")
        .array_flow(
            "adjusted_births[Region]",
            "region_births - buffer * 0.0001",
            None,
        )
        .build_datamodel();

    // Independently determine step_count so the assertion below stays
    // honest even if compute_metadata's own bookkeeping drifts.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, MAIN_MODEL).unwrap();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let step_count = vm.into_results().step_count;

    let metadata = compute_metadata(&project, MAIN_MODEL, None)
        .expect("compute_metadata should succeed for the proxy fixture");
    assert!(
        !metadata.feedback_loops.is_empty(),
        "fixture should produce at least one detected loop"
    );

    for fl in &metadata.feedback_loops {
        assert_eq!(
            fl.importance_series.len(),
            step_count,
            "loop {} importance_series.len() = {} but step_count = {}; \
             pre-fix mixed-stride partitions produced step_count * stride here",
            fl.name,
            fl.importance_series.len(),
            step_count
        );
        for (t, v) in fl.importance_series.iter().enumerate() {
            assert!(
                v.is_finite(),
                "loop {} importance_series[{}] = {} is non-finite",
                fl.name,
                t,
                v
            );
        }
    }
}

/// Issue #463 contract test: for arrayed loops, the layout's
/// `importance_series` is computed by aggregating per-element relative
/// loop scores via signed argmax-abs across slots.
///
/// Verifies two properties:
///   1. Contract: importance_series equals the manually-computed
///      argmax-abs aggregation of the per-element rel scores.
///   2. Bite: at least one arrayed loop's importance_series differs
///      from a hypothetical slot-0-only collapse at some step.  This
///      proves the layout actually picks the dominant element rather
///      than reading slot 0 -- if the implementation regressed to
///      slot-0-only, this would fail because slot 0 ≠ argmax-abs for
///      the heterogeneous-rate fixture.
#[test]
fn test_arrayed_loop_importance_matches_argmax_abs_aggregation() {
    use simlin_engine::Vm;
    use simlin_engine::db::{
        compile_project_incremental, model_ltm_variables, project_datamodel_dims,
        set_project_ltm_enabled,
    };
    use simlin_engine::layout::compute_metadata;
    use simlin_engine::ltm_post;
    use std::collections::HashMap;

    let project = load_project("test/arrayed_population_ltm/arrayed_population.stmx");

    let metadata = compute_metadata(&project, MAIN_MODEL, None)
        .expect("compute_metadata should return Some for a valid project");
    assert!(
        !metadata.feedback_loops.is_empty(),
        "fixture should yield detected feedback loops"
    );

    // Parallel pipeline: compute the per-element rel scores ourselves and
    // aggregate via the same argmax-abs rule the layout is contracted to use.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, MAIN_MODEL).unwrap();
    let source_model = sync.models[MAIN_MODEL].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    let loop_partitions = ltm_vars.loop_partitions.clone();

    let dm_dims = project_datamodel_dims(&db, sync.project);
    let dim_size: HashMap<String, usize> = dm_dims
        .iter()
        .map(|d| (d.name().to_string(), d.len()))
        .collect();
    let prefix = "$\u{205A}ltm\u{205A}loop_score\u{205A}";
    let n_slots_by_loop: HashMap<String, usize> = ltm_vars
        .vars
        .iter()
        .filter_map(|v| {
            let id = v.name.strip_prefix(prefix)?;
            let n = if v.dimensions.is_empty() {
                1
            } else {
                v.dimensions
                    .iter()
                    .map(|d| dim_size.get(d).copied().unwrap_or(1))
                    .product()
            };
            Some((id.to_string(), n))
        })
        .collect();

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    // `compute_rel_loop_scores_per_element` derives each loop's slot count
    // from `loop_partitions[id].len()`; `n_slots_by_loop` is still used below
    // (and by `aggregate_per_element_argmax_abs` inside `compute_metadata`).
    let per_elem = ltm_post::compute_rel_loop_scores_per_element(&results, &loop_partitions);
    let slot0_only = ltm_post::compute_rel_loop_scores(&results, &loop_partitions);

    // (1) Contract: for every detected loop, importance_series must equal
    //     the argmax-abs aggregation of the per-element series.
    for fl in &metadata.feedback_loops {
        let n = n_slots_by_loop.get(&fl.name).copied().unwrap_or(1).max(1);
        let series = per_elem.get(&fl.name).cloned().unwrap_or_default();
        if series.is_empty() {
            assert!(
                fl.importance_series.is_empty(),
                "loop {} has no per-element series but importance_series is non-empty",
                fl.name
            );
            continue;
        }
        let n_steps = series.len() / n;
        let expected: Vec<f64> = (0..n_steps)
            .map(|t| {
                let mut best = 0.0_f64;
                let mut best_abs = -1.0_f64;
                for k in 0..n {
                    let v = series[t * n + k];
                    if v.abs() > best_abs {
                        best_abs = v.abs();
                        best = v;
                    }
                }
                if best.is_finite() { best } else { 0.0 }
            })
            .collect();

        assert_eq!(
            fl.importance_series.len(),
            expected.len(),
            "loop {} importance_series length mismatch",
            fl.name
        );
        for (t, (actual, exp)) in fl.importance_series.iter().zip(&expected).enumerate() {
            assert!(
                (actual - exp).abs() < 1e-9 || (actual.is_nan() && exp.is_nan()),
                "loop {} step {}: importance_series {} != argmax-abs {}",
                fl.name,
                t,
                actual,
                exp
            );
        }
    }

    // (2) Bite: at least one arrayed loop's importance_series must differ
    //     from a slot-0-only collapse at some step.  If the layout still
    //     read slot 0, this would fail because slot 0 and argmax-abs would
    //     be identical for every loop -- which is precisely what the
    //     pre-tech-debt-#34 engine bug used to make true accidentally.
    let any_arrayed = metadata
        .feedback_loops
        .iter()
        .any(|fl| n_slots_by_loop.get(&fl.name).copied().unwrap_or(1) > 1);
    assert!(
        any_arrayed,
        "fixture must produce at least one arrayed feedback loop"
    );
    let any_diff = metadata.feedback_loops.iter().any(|fl| {
        if n_slots_by_loop.get(&fl.name).copied().unwrap_or(1) <= 1 {
            return false;
        }
        let slot0 = slot0_only.get(&fl.name).cloned().unwrap_or_default();
        fl.importance_series
            .iter()
            .zip(&slot0)
            .any(|(a, s)| (a - s).abs() > 1e-9)
    });
    assert!(
        any_diff,
        "expected at least one arrayed loop's importance_series to differ from its slot-0 collapse, \
         which proves the layout uses argmax-abs aggregation rather than reading slot 0"
    );
}

#[test]
fn test_from_existing_view_sir_round_trip() {
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let generated_view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");

    let state = LayoutState::from_existing_view(&generated_view, model);

    // Element count must match
    assert_eq!(
        state.elements.len(),
        generated_view.elements.len(),
        "from_existing_view should preserve all elements"
    );

    // Every element UID and position must match
    for (orig, seeded) in generated_view.elements.iter().zip(state.elements.iter()) {
        assert_eq!(orig.get_uid(), seeded.get_uid(), "UIDs must match");

        // Check positions for elements with coordinates
        let orig_pos = get_element_position(orig);
        let seeded_pos = get_element_position(seeded);
        assert_eq!(
            orig_pos,
            seeded_pos,
            "positions must match for uid={}",
            orig.get_uid()
        );
    }

    // Positions map must contain entries for all elements with coordinates
    for elem in &generated_view.elements {
        if let Some((x, y)) = get_element_position(elem) {
            let pos = state.positions.get(&elem.get_uid());
            assert!(
                pos.is_some(),
                "positions map should contain uid={}",
                elem.get_uid()
            );
            let pos = pos.unwrap();
            assert!(
                (pos.x - x).abs() < f64::EPSILON && (pos.y - y).abs() < f64::EPSILON,
                "position mismatch for uid={}: ({}, {}) vs ({}, {})",
                elem.get_uid(),
                pos.x,
                pos.y,
                x,
                y
            );
        }
    }
}

#[test]
fn test_from_existing_view_teacup_round_trip() {
    let project = load_project("test/test-models/samples/teacup/teacup.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let generated_view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");

    let state = LayoutState::from_existing_view(&generated_view, model);

    assert_eq!(
        state.elements.len(),
        generated_view.elements.len(),
        "from_existing_view should preserve all elements"
    );

    for (orig, seeded) in generated_view.elements.iter().zip(state.elements.iter()) {
        assert_eq!(orig.get_uid(), seeded.get_uid(), "UIDs must match");
    }

    // Positions map must contain entries for all elements with coordinates
    for elem in &generated_view.elements {
        if let Some((x, y)) = get_element_position(elem) {
            let pos = state.positions.get(&elem.get_uid());
            assert!(
                pos.is_some(),
                "positions map should contain uid={}",
                elem.get_uid()
            );
            let pos = pos.unwrap();
            assert!(
                (pos.x - x).abs() < f64::EPSILON && (pos.y - y).abs() < f64::EPSILON,
                "position mismatch for uid={}",
                elem.get_uid()
            );
        }
    }
}

/// Extract (x, y) position from a view element, if it has coordinates.
fn get_element_position(elem: &ViewElement) -> Option<(f64, f64)> {
    match elem {
        ViewElement::Aux(a) => Some((a.x, a.y)),
        ViewElement::Stock(s) => Some((s.x, s.y)),
        ViewElement::Flow(f) => Some((f.x, f.y)),
        ViewElement::Module(m) => Some((m.x, m.y)),
        ViewElement::Cloud(c) => Some((c.x, c.y)),
        ViewElement::Alias(a) => Some((a.x, a.y)),
        ViewElement::Group(g) => Some((g.x, g.y)),
        ViewElement::Link(_) => None,
    }
}

#[test]
fn test_from_existing_view_flow_templates() {
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let model = project.get_model(MAIN_MODEL).unwrap();
    let generated_view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");

    let state = LayoutState::from_existing_view(&generated_view, model);

    // SIR has 2 flows, so flow_templates should be non-empty
    assert!(
        !state.flow_templates.is_empty(),
        "flow_templates should be non-empty for a model with flows"
    );

    // Collect flow elements from the generated view for comparison
    let flow_elems: Vec<_> = generated_view
        .elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Flow(f) = e {
                Some(f)
            } else {
                None
            }
        })
        .collect();

    assert_eq!(
        state.flow_templates.len(),
        flow_elems.iter().filter(|f| f.points.len() >= 2).count(),
        "should have a template for each flow with >= 2 points"
    );

    // Verify each flow template has correct offsets relative to valve center
    for flow_elem in &flow_elems {
        if flow_elem.points.len() < 2 {
            continue;
        }

        let flow_ident = canonicalize(&flow_elem.name).into_owned();
        let template = state.flow_templates.get(&flow_ident);
        assert!(
            template.is_some(),
            "flow_templates should contain entry for '{}'",
            flow_ident
        );
        let template = template.unwrap();

        assert_eq!(
            template.offsets.len(),
            flow_elem.points.len(),
            "template offset count should match flow point count for '{}'",
            flow_ident
        );

        // Each offset should be the flow point position minus the valve center
        for (i, (offset, pt)) in template
            .offsets
            .iter()
            .zip(flow_elem.points.iter())
            .enumerate()
        {
            let expected_dx = pt.x - flow_elem.x;
            let expected_dy = pt.y - flow_elem.y;
            assert!(
                (offset.x - expected_dx).abs() < f64::EPSILON,
                "offset[{}].x for '{}': expected {}, got {}",
                i,
                flow_ident,
                expected_dx,
                offset.x
            );
            assert!(
                (offset.y - expected_dy).abs() < f64::EPSILON,
                "offset[{}].y for '{}': expected {}, got {}",
                i,
                flow_ident,
                expected_dy,
                offset.y
            );
        }
    }
}

/// Verify that a systems-format model with modules produces a complete,
/// renderable diagram with ViewElement::Module for each Variable::Module.
#[test]
fn test_systems_format_layout_with_modules() {
    use simlin_engine::datamodel;

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("could not determine repo root");
    let file_path = repo_root.join("test/systems-format/hiring.txt");
    let contents = std::fs::read_to_string(&file_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", file_path.display(), e));

    let systems_model = simlin_engine::systems::parse(&contents).unwrap();
    let project = simlin_engine::systems::translate::translate(&systems_model, 5).unwrap();

    let view = generate_layout(&project, MAIN_MODEL, None)
        .expect("layout generation should succeed for systems format model");
    let model = project.get_model(MAIN_MODEL).unwrap();

    // Count expected module elements
    let module_count = model
        .variables
        .iter()
        .filter(|v| matches!(v, datamodel::Variable::Module(_)))
        .count();
    let view_module_count = view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Module(_)))
        .count();
    assert_eq!(
        module_count, view_module_count,
        "every Variable::Module should have a ViewElement::Module"
    );

    // Systems format models use stdlib modules for flows, so there
    // should be a non-trivial number of modules.
    assert!(
        module_count > 0,
        "hiring model should have at least one module (got {})",
        module_count
    );

    // Verify all modules have valid, finite positions
    for elem in &view.elements {
        if let ViewElement::Module(m) = elem {
            assert!(
                m.x.is_finite() && m.y.is_finite(),
                "module '{}' should have finite coordinates ({}, {})",
                m.name,
                m.x,
                m.y
            );
        }
    }

    // Use the shared verify_layout helper which now includes Module coverage
    verify_layout(&view, model, "systems_hiring");
}

/// Helper: extract element position by canonical ident from a StockFlow view.
fn find_element_position(
    view: &simlin_engine::datamodel::StockFlow,
    canonical_ident: &str,
) -> Option<(f64, f64)> {
    let normalize = |s: &str| -> String { canonicalize(&s.replace('\n', "_")).into_owned() };
    for elem in &view.elements {
        match elem {
            ViewElement::Stock(s) if normalize(&s.name) == canonical_ident => {
                return Some((s.x, s.y));
            }
            ViewElement::Flow(f) if normalize(&f.name) == canonical_ident => {
                return Some((f.x, f.y));
            }
            ViewElement::Aux(a) if normalize(&a.name) == canonical_ident => {
                return Some((a.x, a.y));
            }
            ViewElement::Module(m) if normalize(&m.name) == canonical_ident => {
                return Some((m.x, m.y));
            }
            _ => {}
        }
    }
    None
}

/// Collect all variable element positions from a view as a map of
/// canonical ident -> (x, y).
fn collect_element_positions(
    view: &simlin_engine::datamodel::StockFlow,
) -> std::collections::HashMap<String, (f64, f64)> {
    let normalize = |s: &str| -> String { canonicalize(&s.replace('\n', "_")).into_owned() };
    let mut positions = std::collections::HashMap::new();
    for elem in &view.elements {
        match elem {
            ViewElement::Stock(s) => {
                positions.insert(normalize(&s.name), (s.x, s.y));
            }
            ViewElement::Flow(f) => {
                positions.insert(normalize(&f.name), (f.x, f.y));
            }
            ViewElement::Aux(a) => {
                positions.insert(normalize(&a.name), (a.x, a.y));
            }
            ViewElement::Module(m) => {
                positions.insert(normalize(&m.name), (m.x, m.y));
            }
            _ => {}
        }
    }
    positions
}

#[test]
fn test_incremental_add_aux() {
    use simlin_engine::datamodel;
    use simlin_engine::layout::incremental_layout;
    use simlin_engine::{ModelOperation, ModelPatch};

    // Load SIR model and generate initial layout
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let old_view =
        generate_layout(&project, MAIN_MODEL, None).expect("initial layout should succeed");
    let original_positions = collect_element_positions(&old_view);

    // Build patched project: add vaccination_rate aux that depends on susceptible
    let mut patched_project = project.clone();
    let model = patched_project.get_model_mut(MAIN_MODEL).unwrap();
    model
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "vaccination_rate".to_string(),
            equation: datamodel::Equation::Scalar("susceptible * 0.01".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Default::default(),
        }));

    let patch = ModelPatch {
        name: String::new(),
        ops: vec![ModelOperation::UpsertAux(datamodel::Aux {
            ident: "vaccination_rate".to_string(),
            equation: datamodel::Equation::Scalar("susceptible * 0.01".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Default::default(),
        })],
    };

    let new_view = incremental_layout(&old_view, &patched_project, MAIN_MODEL, &patch, None)
        .expect("incremental layout should succeed");

    // AC1.2: All original element positions should be preserved
    let new_positions = collect_element_positions(&new_view);
    for (ident, &(orig_x, orig_y)) in &original_positions {
        let (new_x, new_y) = new_positions
            .get(ident)
            .unwrap_or_else(|| panic!("original element '{}' should still exist", ident));
        assert!(
            (orig_x - new_x).abs() < 0.01 && (orig_y - new_y).abs() < 0.01,
            "element '{}' moved from ({}, {}) to ({}, {})",
            ident,
            orig_x,
            orig_y,
            new_x,
            new_y,
        );
    }

    // AC4.1: vaccination_rate should have a view element
    let vr_pos = find_element_position(&new_view, "vaccination_rate")
        .expect("vaccination_rate should have a view element");
    assert!(
        vr_pos.0.is_finite() && vr_pos.1.is_finite(),
        "vaccination_rate should have finite coordinates"
    );

    // AC4.1: vaccination_rate should be near susceptible (its dependency)
    let susc_pos = new_positions
        .get("susceptible")
        .expect("susceptible should exist");
    let distance = ((vr_pos.0 - susc_pos.0).powi(2) + (vr_pos.1 - susc_pos.1).powi(2)).sqrt();
    assert!(
        distance < 500.0,
        "vaccination_rate should be within 500px of susceptible, got {}",
        distance,
    );

    // Basic integrity: all UIDs should be unique
    let mut uids = HashSet::new();
    for elem in &new_view.elements {
        let uid = elem.get_uid();
        assert!(uids.insert(uid), "duplicate UID {} found", uid);
    }
}

/// Helper: collect all Link elements as (from_uid, to_uid) pairs.
fn collect_links(view: &simlin_engine::datamodel::StockFlow) -> HashSet<(i32, i32)> {
    view.elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Link(l) => Some((l.from_uid, l.to_uid)),
            _ => None,
        })
        .collect()
}

/// Helper: find the UID of a named view element.
fn find_element_uid(
    view: &simlin_engine::datamodel::StockFlow,
    canonical_ident: &str,
) -> Option<i32> {
    let normalize = |s: &str| -> String { canonicalize(&s.replace('\n', "_")).into_owned() };
    for elem in &view.elements {
        match elem {
            ViewElement::Stock(s) if normalize(&s.name) == canonical_ident => return Some(s.uid),
            ViewElement::Flow(f) if normalize(&f.name) == canonical_ident => return Some(f.uid),
            ViewElement::Aux(a) if normalize(&a.name) == canonical_ident => return Some(a.uid),
            ViewElement::Module(m) if normalize(&m.name) == canonical_ident => return Some(m.uid),
            _ => {}
        }
    }
    None
}

#[test]
fn test_incremental_combined_ops() {
    use simlin_engine::datamodel;
    use simlin_engine::layout::incremental_layout;
    use simlin_engine::{ModelOperation, ModelPatch};

    // SIR model variables:
    //   stocks: susceptible, infectious, recovered
    //   flows: succumbing, recovering
    //   auxes: total_population, duration, contact_infectivity
    //
    // Dependency edges (non-structural):
    //   total_population -> susceptible (init)
    //   contact_infectivity -> succumbing
    //   susceptible -> succumbing, infectious -> succumbing
    //   total_population -> succumbing
    //   duration -> recovering, infectious -> recovering
    //
    // Patch:
    //   1. Delete contact_infectivity
    //   2. Rename total_population -> total_pop
    //   3. Add immunity_rate = 1/duration, change recovering = infectious * immunity_rate
    //      This inserts immunity_rate between duration and recovering:
    //      old: duration -> recovering
    //      new: duration -> immunity_rate -> recovering

    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let old_view =
        generate_layout(&project, MAIN_MODEL, None).expect("initial layout should succeed");
    let original_positions = collect_element_positions(&old_view);

    // Snapshot the position of total_population before rename
    let tp_position = original_positions
        .get("total_population")
        .expect("total_population should exist");

    // Build post-patch model manually
    let mut patched_project = project.clone();
    let model = patched_project.get_model_mut(MAIN_MODEL).unwrap();

    // Delete contact_infectivity
    model
        .variables
        .retain(|v| canonicalize(v.get_ident()).as_ref() != "contact_infectivity");

    // Rename total_population -> total_pop
    for var in &mut model.variables {
        if canonicalize(var.get_ident()).as_ref() == "total_population"
            && let datamodel::Variable::Aux(a) = var
        {
            a.ident = "total_pop".to_string();
        }
    }

    // Update succumbing equation to remove contact_infectivity reference
    for var in &mut model.variables {
        if canonicalize(var.get_ident()).as_ref() == "succumbing"
            && let datamodel::Variable::Flow(f) = var
        {
            f.equation =
                datamodel::Equation::Scalar("susceptible*infectious/total_pop".to_string());
        }
    }

    // Update susceptible init to reference total_pop
    for var in &mut model.variables {
        if canonicalize(var.get_ident()).as_ref() == "susceptible"
            && let datamodel::Variable::Stock(s) = var
        {
            s.equation = datamodel::Equation::Scalar("total_pop".to_string());
        }
    }

    // Change recovering equation to use immunity_rate instead of duration
    for var in &mut model.variables {
        if canonicalize(var.get_ident()).as_ref() == "recovering"
            && let datamodel::Variable::Flow(f) = var
        {
            f.equation = datamodel::Equation::Scalar("infectious * immunity_rate".to_string());
        }
    }

    // Add immunity_rate aux
    model
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "immunity_rate".to_string(),
            equation: datamodel::Equation::Scalar("1 / duration".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Default::default(),
        }));

    // Build the patch
    let patch = ModelPatch {
        name: String::new(),
        ops: vec![
            ModelOperation::DeleteVariable {
                ident: "contact_infectivity".to_string(),
            },
            ModelOperation::RenameVariable {
                from: "total_population".to_string(),
                to: "total_pop".to_string(),
            },
            ModelOperation::UpsertAux(datamodel::Aux {
                ident: "immunity_rate".to_string(),
                equation: datamodel::Equation::Scalar("1 / duration".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: Default::default(),
            }),
        ],
    };

    let new_view = incremental_layout(&old_view, &patched_project, MAIN_MODEL, &patch, None)
        .expect("incremental layout with combined ops should succeed");
    let new_positions = collect_element_positions(&new_view);

    // Deleted variable should have no view element
    assert!(
        find_element_position(&new_view, "contact_infectivity").is_none(),
        "contact_infectivity should not have a view element after deletion"
    );

    // Renamed variable should exist with new name and same position
    let tp_new_pos = find_element_position(&new_view, "total_pop")
        .expect("total_pop should have a view element after rename");
    assert!(
        (tp_position.0 - tp_new_pos.0).abs() < 0.01 && (tp_position.1 - tp_new_pos.1).abs() < 0.01,
        "total_pop should have the same position as total_population: ({}, {}) vs ({}, {})",
        tp_position.0,
        tp_position.1,
        tp_new_pos.0,
        tp_new_pos.1,
    );

    // New variable should have a view element
    let ir_pos = find_element_position(&new_view, "immunity_rate")
        .expect("immunity_rate should have a view element");
    assert!(
        ir_pos.0.is_finite() && ir_pos.1.is_finite(),
        "immunity_rate should have finite coordinates"
    );

    // All other original elements (excluding deleted/renamed) should preserve position
    for (ident, &(orig_x, orig_y)) in &original_positions {
        if ident == "contact_infectivity" || ident == "total_population" {
            continue;
        }
        let (new_x, new_y) = new_positions
            .get(ident)
            .unwrap_or_else(|| panic!("element '{}' should still exist", ident));
        assert!(
            (orig_x - new_x).abs() < 0.01 && (orig_y - new_y).abs() < 0.01,
            "element '{}' moved from ({}, {}) to ({}, {})",
            ident,
            orig_x,
            orig_y,
            new_x,
            new_y,
        );
    }

    // AC5.5: Verify connector updates for the intermediate auxiliary.
    // In the original model: duration -> recovering (direct link)
    // In the patched model: duration -> immunity_rate -> recovering
    let duration_uid = find_element_uid(&new_view, "duration").expect("duration should have a UID");
    let recovering_uid =
        find_element_uid(&new_view, "recovering").expect("recovering should have a UID");
    let immunity_rate_uid =
        find_element_uid(&new_view, "immunity_rate").expect("immunity_rate should have a UID");

    let links = collect_links(&new_view);

    // Old direct link should be gone
    assert!(
        !links.contains(&(duration_uid, recovering_uid)),
        "direct link duration -> recovering should be removed"
    );

    // New links through the intermediate should exist
    assert!(
        links.contains(&(duration_uid, immunity_rate_uid)),
        "link duration -> immunity_rate should exist"
    );
    assert!(
        links.contains(&(immunity_rate_uid, recovering_uid)),
        "link immunity_rate -> recovering should exist"
    );

    // No dangling link references: every link's from_uid and to_uid
    // should reference an element that exists in the view.
    let all_uids: HashSet<i32> = new_view.elements.iter().map(|e| e.get_uid()).collect();
    for elem in &new_view.elements {
        if let ViewElement::Link(l) = elem {
            assert!(
                all_uids.contains(&l.from_uid),
                "link from_uid {} references non-existent element",
                l.from_uid,
            );
            assert!(
                all_uids.contains(&l.to_uid),
                "link to_uid {} references non-existent element",
                l.to_uid,
            );
        }
    }

    // All UIDs should be unique
    let mut uid_set = HashSet::new();
    for elem in &new_view.elements {
        let uid = elem.get_uid();
        assert!(uid_set.insert(uid), "duplicate UID {} found", uid);
    }
}

#[test]
fn test_incremental_fallback_to_full_layout() {
    use simlin_engine::ModelPatch;
    use simlin_engine::datamodel;
    use simlin_engine::layout::incremental_layout;

    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let model = project.get_model(MAIN_MODEL).expect("model should exist");

    let empty_view = datamodel::StockFlow {
        name: None,
        elements: Vec::new(),
        view_box: datamodel::Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    };

    let empty_patch = ModelPatch {
        name: String::new(),
        ops: Vec::new(),
    };

    let incremental_result =
        incremental_layout(&empty_view, &project, MAIN_MODEL, &empty_patch, None)
            .expect("incremental layout with empty view should succeed");

    let full_result = generate_best_layout(&project, MAIN_MODEL, None)
        .expect("generate_best_layout should succeed");

    // Both should produce views covering all model variables
    verify_layout(&incremental_result, model, "incremental fallback");
    verify_layout(&full_result, model, "full layout");

    assert_eq!(
        incremental_result.elements.len(),
        full_result.elements.len(),
        "fallback should produce the same number of elements as full layout"
    );
}

/// Verify that the hiring model's waste flows do not overlap chain flows.
///
/// The hiring model is an aging chain with conversion flows that produce
/// both a chain flow (stock-to-stock) and a waste flow (stock-to-cloud).
/// Waste flows must exit from the bottom of their source stock, not from
/// the right where they would overlap the chain flow.
#[test]
fn test_layout_hiring_no_flow_overlap() {
    use std::collections::HashMap;

    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("could not determine repo root");
    let file_path = repo_root.join("test/systems-format/hiring.txt");
    let contents = std::fs::read_to_string(&file_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", file_path.display(), e));

    let systems_model = simlin_engine::systems::parse(&contents).unwrap();
    let project = simlin_engine::systems::translate::translate(&systems_model, 10).unwrap();

    let view =
        generate_layout(&project, MAIN_MODEL, None).expect("layout generation should succeed");
    let model = project.get_model(MAIN_MODEL).unwrap();

    // Standard invariants
    verify_layout(&view, model, "hiring_no_overlap");

    // Build lookup tables
    let normalize = |s: &str| -> String { canonicalize(&s.replace('\n', "_")).into_owned() };
    let mut flow_positions: HashMap<String, (f64, f64)> = HashMap::new();
    let mut stock_positions: HashMap<String, (f64, f64)> = HashMap::new();

    for elem in &view.elements {
        match elem {
            ViewElement::Flow(f) => {
                flow_positions.insert(normalize(&f.name), (f.x, f.y));
            }
            ViewElement::Stock(s) => {
                stock_positions.insert(normalize(&s.name), (s.x, s.y));
            }
            _ => {}
        }
    }

    // Each conversion stock (phonescreens, onsites, offers, hires, departures)
    // has a chain flow and a waste flow. The waste flow should have a different
    // y than the chain flow.
    let waste_chain_pairs = [
        (
            "phonescreens_to_onsites",
            "phonescreens_to_onsites_waste",
            "phonescreens",
        ),
        ("onsites_to_offers", "onsites_to_offers_waste", "onsites"),
        ("offers_to_hires", "offers_to_hires_waste", "offers"),
        ("hires_to_employees", "hires_to_employees_waste", "hires"),
        (
            "departures_to_departed",
            "departures_to_departed_waste",
            "departures",
        ),
    ];

    for (chain_name, waste_name, stock_name) in &waste_chain_pairs {
        let chain_pos = flow_positions
            .get(*chain_name)
            .unwrap_or_else(|| panic!("missing flow: {chain_name}"));
        let waste_pos = flow_positions
            .get(*waste_name)
            .unwrap_or_else(|| panic!("missing flow: {waste_name}"));
        let stock_pos = stock_positions
            .get(*stock_name)
            .unwrap_or_else(|| panic!("missing stock: {stock_name}"));

        // Chain flow should be at the same y as its source stock (horizontal)
        assert!(
            (chain_pos.1 - stock_pos.1).abs() < 1.0,
            "{chain_name} y ({}) should be near {stock_name} y ({})",
            chain_pos.1,
            stock_pos.1,
        );

        // Waste flow should be below the stock (perpendicular exit)
        assert!(
            waste_pos.1 > stock_pos.1 + 5.0,
            "{waste_name} y ({}) should be below {stock_name} y ({})",
            waste_pos.1,
            stock_pos.1,
        );

        // Chain and waste flows must not overlap
        let dist =
            ((chain_pos.0 - waste_pos.0).powi(2) + (chain_pos.1 - waste_pos.1).powi(2)).sqrt();
        assert!(
            dist > 5.0,
            "{chain_name} ({}, {}) and {waste_name} ({}, {}) should not overlap (dist={dist})",
            chain_pos.0,
            chain_pos.1,
            waste_pos.0,
            waste_pos.1,
        );
    }

    // No two flow elements should share the exact same position
    let flow_coords: Vec<(String, f64, f64)> = view
        .elements
        .iter()
        .filter_map(|e| {
            if let ViewElement::Flow(f) = e {
                Some((normalize(&f.name), f.x, f.y))
            } else {
                None
            }
        })
        .collect();

    for i in 0..flow_coords.len() {
        for j in (i + 1)..flow_coords.len() {
            let (ref name_i, xi, yi) = flow_coords[i];
            let (ref name_j, xj, yj) = flow_coords[j];
            let dist = ((xi - xj).powi(2) + (yi - yj).powi(2)).sqrt();
            assert!(
                dist > 1.0,
                "flows '{name_i}' ({xi}, {yi}) and '{name_j}' ({xj}, {yj}) overlap (dist={dist})"
            );
        }
    }
}

/// P1: Incrementally adding a waste flow to a stock with an existing chain
/// flow should position the waste flow below the stock (not overlapping).
#[test]
fn test_incremental_add_waste_flow_goes_below() {
    use simlin_engine::datamodel;
    use simlin_engine::layout::incremental_layout;
    use simlin_engine::{ModelOperation, ModelPatch};

    // Start with a simple chain: stock_a -> chain_flow -> stock_b
    let initial_model = datamodel::Model {
        name: "main".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["chain_flow".to_string()],
                compat: Default::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["chain_flow".to_string()],
                outflows: vec![],
                compat: Default::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "chain_flow".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: Default::default(),
                ai_state: None,
                uid: None,
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    };
    let initial_project = datamodel::Project {
        name: "main".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: Vec::new(),
        units: Vec::new(),
        models: vec![initial_model],
        source: None,
        ai_information: None,
    };

    let old_view = generate_layout(&initial_project, MAIN_MODEL, None).expect("initial layout");

    // Now add waste_flow as a new outflow from stock_a (stock-to-cloud)
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(MAIN_MODEL).unwrap();

    // Add waste_flow to stock_a's outflows
    for var in &mut model.variables {
        if let datamodel::Variable::Stock(s) = var
            && s.ident == "stock_a"
        {
            s.outflows.push("waste_flow".to_string());
        }
    }
    model
        .variables
        .push(datamodel::Variable::Flow(datamodel::Flow {
            ident: "waste_flow".to_string(),
            equation: datamodel::Equation::Scalar("5".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: Default::default(),
            ai_state: None,
            uid: None,
        }));

    let patch = ModelPatch {
        name: String::new(),
        ops: vec![
            ModelOperation::UpsertFlow(datamodel::Flow {
                ident: "waste_flow".to_string(),
                equation: datamodel::Equation::Scalar("5".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: Default::default(),
                ai_state: None,
                uid: None,
            }),
            ModelOperation::UpdateStockFlows {
                ident: "stock_a".to_string(),
                inflows: vec![],
                outflows: vec!["chain_flow".to_string(), "waste_flow".to_string()],
            },
        ],
    };

    let new_view = incremental_layout(&old_view, &patched_project, MAIN_MODEL, &patch, None)
        .expect("incremental layout should succeed");

    let normalize = |s: &str| -> String { canonicalize(&s.replace('\n', "_")).into_owned() };
    let stock_a_pos = new_view
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Stock(s) = e
                && normalize(&s.name) == "stock_a"
            {
                Some((s.x, s.y))
            } else {
                None
            }
        })
        .expect("stock_a should exist");

    let waste_pos = new_view
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Flow(f) = e
                && normalize(&f.name) == "waste_flow"
            {
                Some((f.x, f.y))
            } else {
                None
            }
        })
        .expect("waste_flow should exist");

    let chain_pos = new_view
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Flow(f) = e
                && normalize(&f.name) == "chain_flow"
            {
                Some((f.x, f.y))
            } else {
                None
            }
        })
        .expect("chain_flow should exist");

    // Waste flow should be below the stock, not at the same y as chain flow
    assert!(
        waste_pos.1 > stock_a_pos.1 + 5.0,
        "waste_flow y ({}) should be below stock_a y ({})",
        waste_pos.1,
        stock_a_pos.1,
    );

    // Chain and waste should not overlap
    let dist = ((chain_pos.0 - waste_pos.0).powi(2) + (chain_pos.1 - waste_pos.1).powi(2)).sqrt();
    assert!(
        dist > 5.0,
        "chain_flow and waste_flow should not overlap (dist={dist})"
    );
}

/// P2: When a chain flow is incrementally added to a stock that already has
/// a cloud outflow on the right, the existing cloud flow should be rebuilt
/// to exit from the bottom.
#[test]
fn test_incremental_add_chain_rebuilds_existing_cloud_flow() {
    use simlin_engine::datamodel;
    use simlin_engine::layout::incremental_layout;
    use simlin_engine::{ModelOperation, ModelPatch};

    // Start with stock_a -> waste_flow -> cloud (only outflow, goes right)
    let initial_model = datamodel::Model {
        name: "main".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["waste_flow".to_string()],
                compat: Default::default(),
                ai_state: None,
                uid: None,
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "waste_flow".to_string(),
                equation: datamodel::Equation::Scalar("5".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: Default::default(),
                ai_state: None,
                uid: None,
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    };
    let initial_project = datamodel::Project {
        name: "main".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: Vec::new(),
        units: Vec::new(),
        models: vec![initial_model],
        source: None,
        ai_information: None,
    };

    let old_view = generate_layout(&initial_project, MAIN_MODEL, None).expect("initial layout");

    // Verify waste_flow starts on the right (horizontal, same y as stock)
    let normalize = |s: &str| -> String { canonicalize(&s.replace('\n', "_")).into_owned() };
    let old_stock_pos = old_view
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Stock(s) = e
                && normalize(&s.name) == "stock_a"
            {
                Some((s.x, s.y))
            } else {
                None
            }
        })
        .expect("stock_a in old view");
    let old_waste_pos = old_view
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Flow(f) = e
                && normalize(&f.name) == "waste_flow"
            {
                Some((f.x, f.y))
            } else {
                None
            }
        })
        .expect("waste_flow in old view");
    assert!(
        (old_waste_pos.1 - old_stock_pos.1).abs() < 1.0,
        "waste_flow should start horizontal (same y as stock)"
    );

    // Now add stock_b and chain_flow: stock_a -> chain_flow -> stock_b
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(MAIN_MODEL).unwrap();

    // Update stock_a outflows to include chain_flow
    for var in &mut model.variables {
        if let datamodel::Variable::Stock(s) = var
            && s.ident == "stock_a"
        {
            s.outflows.push("chain_flow".to_string());
        }
    }
    model
        .variables
        .push(datamodel::Variable::Stock(datamodel::Stock {
            ident: "stock_b".to_string(),
            equation: datamodel::Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec!["chain_flow".to_string()],
            outflows: vec![],
            compat: Default::default(),
            ai_state: None,
            uid: None,
        }));
    model
        .variables
        .push(datamodel::Variable::Flow(datamodel::Flow {
            ident: "chain_flow".to_string(),
            equation: datamodel::Equation::Scalar("10".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: Default::default(),
            ai_state: None,
            uid: None,
        }));

    let patch = ModelPatch {
        name: String::new(),
        ops: vec![
            ModelOperation::UpsertStock(datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["chain_flow".to_string()],
                outflows: vec![],
                compat: Default::default(),
                ai_state: None,
                uid: None,
            }),
            ModelOperation::UpsertFlow(datamodel::Flow {
                ident: "chain_flow".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: Default::default(),
                ai_state: None,
                uid: None,
            }),
            ModelOperation::UpdateStockFlows {
                ident: "stock_a".to_string(),
                inflows: vec![],
                outflows: vec!["waste_flow".to_string(), "chain_flow".to_string()],
            },
        ],
    };

    let new_view = incremental_layout(&old_view, &patched_project, MAIN_MODEL, &patch, None)
        .expect("incremental layout should succeed");

    let new_stock_pos = new_view
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Stock(s) = e
                && normalize(&s.name) == "stock_a"
            {
                Some((s.x, s.y))
            } else {
                None
            }
        })
        .expect("stock_a in new view");

    let new_waste_pos = new_view
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Flow(f) = e
                && normalize(&f.name) == "waste_flow"
            {
                Some((f.x, f.y))
            } else {
                None
            }
        })
        .expect("waste_flow in new view");

    let new_chain_pos = new_view
        .elements
        .iter()
        .find_map(|e| {
            if let ViewElement::Flow(f) = e
                && normalize(&f.name) == "chain_flow"
            {
                Some((f.x, f.y))
            } else {
                None
            }
        })
        .expect("chain_flow in new view");

    // Now waste_flow should have moved to the bottom (different y from stock)
    assert!(
        new_waste_pos.1 > new_stock_pos.1 + 5.0,
        "waste_flow y ({}) should now be below stock_a y ({}) after chain added",
        new_waste_pos.1,
        new_stock_pos.1,
    );

    // Chain and waste should not overlap
    let dist = ((new_chain_pos.0 - new_waste_pos.0).powi(2)
        + (new_chain_pos.1 - new_waste_pos.1).powi(2))
    .sqrt();
    assert!(
        dist > 5.0,
        "chain_flow and waste_flow should not overlap after incremental add (dist={dist})"
    );
}

/// Count how many elements differ between two views generated for the same
/// model.  Element ordering is structurally stable (see
/// `test_layout_structural_consistency`), so a positional comparison can be
/// done index-by-index; `ViewElement` derives `PartialEq` over its f64
/// coordinates (and flow `points`), giving an exact byte-for-byte comparison.
/// Returns `(differing, total)`.
fn count_layout_differences(
    a: &simlin_engine::datamodel::StockFlow,
    b: &simlin_engine::datamodel::StockFlow,
) -> (usize, usize) {
    assert_eq!(
        a.elements.len(),
        b.elements.len(),
        "layouts must have the same number of elements to compare"
    );
    let differing = a
        .elements
        .iter()
        .zip(b.elements.iter())
        .filter(|(ea, eb)| ea != eb)
        .count();
    (differing, a.elements.len())
}

/// A layout produced for a fixed (model, annealing_random_seed) must be
/// bit-identical across repeated serial calls in one process (issue #633).
/// The RNG is already seeded deterministically; the only remaining source of
/// run-to-run drift was per-instance-random `HashMap` iteration order inside
/// `run_sfdp_with_rigid_chains` (centroid float accumulation and aux initial
/// placement).  SIR has auxiliaries, so it exercises the aux-placement loop.
#[test]
fn test_layout_deterministic_per_seed() {
    let project = load_project("test/test-models/samples/SIR/SIR.stmx");

    let config = LayoutConfig {
        annealing_random_seed: 42,
        ..Default::default()
    };

    let view1 = generate_layout_with_config(&project, MAIN_MODEL, config.clone(), None)
        .expect("first layout should succeed");
    let view2 = generate_layout_with_config(&project, MAIN_MODEL, config, None)
        .expect("second layout should succeed");

    let (differing, total) = count_layout_differences(&view1, &view2);
    assert_eq!(
        differing, 0,
        "layout for a fixed seed must be deterministic: {differing}/{total} elements differ \
         between two serial calls"
    );
}

/// The incremental layout path (`incremental_layout` ->
/// `compute_new_element_positions`) must also be deterministic for a fixed
/// model + patch.  This guards against the same class of HashMap-iteration
/// nondeterminism in the incremental code paths.
#[test]
fn test_incremental_layout_deterministic() {
    use simlin_engine::datamodel;
    use simlin_engine::layout::incremental_layout;
    use simlin_engine::{ModelOperation, ModelPatch};

    let project = load_project("test/test-models/samples/SIR/SIR.stmx");
    let old_view =
        generate_layout(&project, MAIN_MODEL, None).expect("initial layout should succeed");

    let mut patched_project = project.clone();
    let model = patched_project.get_model_mut(MAIN_MODEL).unwrap();
    model
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "vaccination_rate".to_string(),
            equation: datamodel::Equation::Scalar("susceptible * 0.01".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Default::default(),
        }));

    let make_patch = || ModelPatch {
        name: String::new(),
        ops: vec![ModelOperation::UpsertAux(datamodel::Aux {
            ident: "vaccination_rate".to_string(),
            equation: datamodel::Equation::Scalar("susceptible * 0.01".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Default::default(),
        })],
    };

    let new_view1 =
        incremental_layout(&old_view, &patched_project, MAIN_MODEL, &make_patch(), None)
            .expect("first incremental layout should succeed");
    let new_view2 =
        incremental_layout(&old_view, &patched_project, MAIN_MODEL, &make_patch(), None)
            .expect("second incremental layout should succeed");

    let (differing, total) = count_layout_differences(&new_view1, &new_view2);
    assert_eq!(
        differing, 0,
        "incremental layout must be deterministic: {differing}/{total} elements differ \
         between two serial calls"
    );
}
