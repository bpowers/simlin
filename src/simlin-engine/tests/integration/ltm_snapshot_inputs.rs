// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A snapshot expression is an input of the ceteris-paribus evaluation.
//! `PREVIOUS(g)` changes when g's history advances; its value at the previous
//! step, including its initial fallback, belongs in a changed-first partial.
//! `INIT(g)` is constant. These tests derive all inputs through the production
//! parser, dependency extraction, augmentation, and VM.

use simlin_engine::test_common::TestProject;

use crate::test_helpers::{ltm_run, ltm_series};

fn score_key(from: &str, to: &str) -> String {
    format!("$⁚ltm⁚link_score⁚{from}→{to}")
}

fn snapshot_project(memory: &str, coefficient: f64) -> simlin_engine::datamodel::Project {
    TestProject::new("snapshot_partial")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("x", "10", &["z"], &[], None)
        .flow("z", &format!("10 + {coefficient} * x + {memory}"), None)
        .stock("g", "30", &["growing"], &[], None)
        .flow("growing", "3", None)
        .build_datamodel()
}

/// For z = c*x + memory, only the current occurrence of x changes in the
/// partial. The selected-source temporal policy has a separate test below.
fn assert_additive_snapshot_score(memory: &str, coefficient: f64) {
    let project = snapshot_project(memory, coefficient);
    for discovery in [false, true] {
        let run = ltm_run(&project, discovery);
        assert!(run.diagnostics.is_empty(), "{:?}", run.diagnostics);
        let x = ltm_series(&run.results, "x", 0);
        let z = ltm_series(&run.results, "z", 0);
        let scores = ltm_series(&run.results, &score_key("x", "z"), 0);
        assert_eq!(scores[0], 0.0);
        for step in 1..scores.len() {
            let dx = x[step] - x[step - 1];
            let dz = z[step] - z[step - 1];
            let expected = if dx == 0.0 || dz == 0.0 {
                0.0
            } else {
                coefficient * dx.abs() / dz.abs()
            };
            assert!(
                (scores[step] - expected).abs() < 1e-12,
                "{memory}, coefficient {coefficient}, discovery={discovery}, step {step}: \
                 got {}, want {expected}; x={x:?}, z={z:?}",
                scores[step]
            );
        }
    }
}

#[test]
fn scoring_snapshots_preserves_the_ordinary_trajectory() {
    use simlin_engine::db::{
        LtmOverlay, SimlinDb, compile_project_incremental, sync_from_datamodel_incremental,
    };

    for method in [
        simlin_engine::datamodel::SimMethod::Euler,
        simlin_engine::datamodel::SimMethod::RungeKutta2,
        simlin_engine::datamodel::SimMethod::RungeKutta4,
    ] {
        let mut project = snapshot_project("PREVIOUS(PREVIOUS(g, 17), x)", 0.1);
        project.sim_specs.sim_method = method;
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let compiled = compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off)
            .expect("ordinary compilation");
        let mut vm = simlin_engine::Vm::new(compiled).expect("ordinary VM");
        vm.run_to_end().expect("ordinary simulation");
        let ordinary = vm.into_results();
        let scored = ltm_run(&project, true);
        for name in ["x", "z", "g", "growing"] {
            assert_eq!(
                ltm_series(&ordinary, name, 0),
                ltm_series(&scored.results, name, 0),
                "{method:?}: {name} must have the same values with LTM enabled"
            );
        }
    }
}

#[test]
fn lagged_input_motion_is_not_attributed_to_an_inert_source() {
    assert_additive_snapshot_score("PREVIOUS(g, 17)", 0.0);
}

#[test]
fn snapshots_are_frozen_at_their_previous_evaluation() {
    // Both snapshot builtins, nested combinations, and both PREVIOUS arities
    // cover the co-input temporal arms. Selected-source snapshots and
    // reducer-body freezing have their own production tests below.
    for memory in [
        "PREVIOUS(g)",
        "PREVIOUS(g, 17)",
        "PREVIOUS(g, x)",
        "PREVIOUS(PREVIOUS(g, 17), 23)",
        "INIT(g)",
        "INIT(PREVIOUS(g, 17))",
        "PREVIOUS(INIT(g), 17)",
    ] {
        assert_additive_snapshot_score(memory, 0.1);
    }
}

#[test]
fn selected_source_snapshots_keep_the_existing_temporal_policy() {
    // A Previous read still forms a causal edge. LTM groups current and past
    // reads of the selected source into that edge; freezing those reads would
    // silently erase temporal feedback. The increasing source has a decreasing
    // first lagged value (fallback 17 -> initial x=10), and the resulting -1
    // documents the unresolved source-window polarity limitation.
    for equation in ["10 + PREVIOUS(x, 17)", "10 + 0.1*x + PREVIOUS(x, 17)"] {
        let project = TestProject::new("selected_snapshot")
            .with_sim_time(0.0, 4.0, 1.0)
            .stock("x", "10", &["z"], &[], None)
            .flow("z", equation, None)
            .build_datamodel();
        let run = ltm_run(&project, true);
        let scores = ltm_series(&run.results, &score_key("x", "z"), 0);
        assert_eq!(scores, vec![0.0, -1.0, 1.0, 1.0, 1.0], "{equation}");
    }
}

#[test]
fn lagged_reducer_input_does_not_credit_a_row_that_never_wins() {
    let mut project = TestProject::new("snapshot_min")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("Region", &["a", "b"])
        .array_stock("x[Region]", "10", &["growth"], &[], None)
        .array_flow("growth[Region]", "1 + 0.01 * total", None)
        .aux("total", "MIN(x[*] + PREVIOUS(g, 17))", None)
        .stock("g", "30", &["growing"], &[], None)
        .flow("growing", "3", None)
        .build_datamodel();
    for var in &mut project.models[0].variables {
        if let simlin_engine::datamodel::Variable::Stock(stock) = var
            && stock.ident == "x"
        {
            stock.equation = simlin_engine::datamodel::Equation::Arrayed(
                vec!["Region".to_owned()],
                vec![
                    ("a".to_owned(), "10".to_owned(), None, None),
                    ("b".to_owned(), "100".to_owned(), None, None),
                ],
                None,
                false,
            );
        }
    }
    let run = ltm_run(&project, true);
    let scores = ltm_series(&run.results, &score_key("x[b]", "total"), 0);
    assert_eq!(scores, vec![0.0; 5], "the other row is always the minimum");
}

#[test]
fn proportional_loop_scores_are_invariant_to_the_units_scale() {
    for method in [
        simlin_engine::datamodel::SimMethod::Euler,
        simlin_engine::datamodel::SimMethod::RungeKutta2,
        simlin_engine::datamodel::SimMethod::RungeKutta4,
    ] {
        for scale in [1.0, 1e-18] {
            let project = TestProject::new("scaled_loop")
                .with_sim_time(0.0, 3.0, 1.0)
                .with_sim_method(method)
                .stock("s", &scale.to_string(), &["growth"], &[], None)
                .flow("growth", "0.1 * s", None)
                .build_datamodel();
            let run = ltm_run(&project, false);
            for (source, target) in [("s", "growth"), ("growth", "s")] {
                assert_eq!(
                    ltm_series(&run.results, &score_key(source, target), 0),
                    vec![0.0, 1.0, 1.0, 1.0],
                    "{method:?}, scale {scale}: {source} -> {target}"
                );
            }
        }
    }
}

#[test]
fn fixed_index_snapshot_co_inputs_keep_each_arrayed_targets_history() {
    let project = TestProject::new("arrayed_snapshot")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("Region", &["a", "b"])
        .array_stock("x[Region]", "10", &["z"], &[], None)
        .array_stock("g[Region]", "30", &["growing"], &[], None)
        .array_flow("growing[Region]", "3", None)
        .array_flow_with_ranges(
            "z[Region]",
            vec![
                ("a", "10 + 0.1 * x[a] + PREVIOUS(g[b], 17)"),
                ("b", "10 + 0.1 * x[b] + PREVIOUS(g[a], 23)"),
            ],
        )
        .build_datamodel();
    let run = ltm_run(&project, true);
    for (element, slot) in [("a", 0), ("b", 1)] {
        let source = format!("x[{element}]");
        let x = ltm_series(&run.results, &source, 0);
        let z = ltm_series(&run.results, &format!("z[{element}]"), 0);
        let scores = ltm_series(&run.results, &score_key(&source, "z"), slot);
        for step in 1..scores.len() {
            let expected = 0.1 * (x[step] - x[step - 1]).abs() / (z[step] - z[step - 1]).abs();
            assert!(
                (scores[step] - expected).abs() < 1e-12,
                "{element}, step {step}: {} != {expected}",
                scores[step]
            );
        }
    }
}

#[test]
fn element_reducer_scores_are_invariant_to_the_units_scale() {
    for scale in [1.0, 1e-18] {
        let project = TestProject::new("scaled_reducer")
            .with_sim_time(0.0, 3.0, 1.0)
            .named_dimension("Region", &["a", "b"])
            .array_stock("x[Region]", &scale.to_string(), &["growth"], &[], None)
            .array_flow("growth[Region]", "0.1 * total", None)
            .aux("total", "SUM(x[*])", None)
            .build_datamodel();
        let run = ltm_run(&project, true);
        for element in ["a", "b"] {
            let scores = ltm_series(
                &run.results,
                &score_key(&format!("x[{element}]"), "total"),
                0,
            );
            assert_eq!(scores[0], 0.0);
            assert!(scores[1..].iter().all(|s| (*s - 0.5).abs() < 1e-12));
        }
    }
}

/// `main`: `s -> m -> growth -> s`, where `m`'s output ignores its input
/// (`out = scale * (1 + TIME)`). A module with an input->output pathway
/// exposes a composite for the port instead, so only a pathway-less module
/// reaches the magnitude-1 black-box unit transfer for `s -> m`: the third
/// link-score generator beside the ceteris-paribus guard form and the element
/// reducer (the two tests above).
fn black_box_loop_project(scale: f64) -> simlin_engine::datamodel::Project {
    use simlin_engine::datamodel;

    let aux = |ident: &str, equation: &str, can_be_module_input: bool| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: datamodel::Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input,
                ..datamodel::Compat::default()
            },
        })
    };
    let mut project = TestProject::new("scaled_black_box")
        .with_sim_time(0.0, 3.0, 1.0)
        .stock("s", &scale.to_string(), &["growth"], &[], None)
        .flow("growth", "0.1 * m.out", None)
        .build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
            ident: "m".to_string(),
            model_name: "independent".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "s".to_string(),
                dst: "m.input_val".to_string(),
            }],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        }));
    project.models.push(datamodel::Model {
        name: "independent".to_string(),
        sim_specs: None,
        variables: vec![
            aux("input_val", "0", true),
            aux("out", &format!("{scale} * (1 + TIME)"), false),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project
}

#[test]
fn black_box_module_scores_are_invariant_to_the_units_scale() {
    use simlin_engine::db::{SimlinDb, model_ltm_variables, sync_from_datamodel_incremental};

    for scale in [1.0, 1e-18] {
        let project = black_box_loop_project(scale);
        // The value assertions below cannot tell the black box from a
        // composite that also scores 1, so pin which generator ran.
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let ltm = model_ltm_variables(&db, sync.models["main"].source_model, sync.project);
        let black_box = ltm
            .vars
            .iter()
            .find(|v| v.name == score_key("s", "m"))
            .expect("s -> m must be scored");
        let text = black_box.equation.source_text();
        assert!(
            text.contains("SIGN(") && !text.contains("SAFEDIV") && !text.contains("composite"),
            "s -> m must be the black-box unit transfer: {text}"
        );

        let run = ltm_run(&project, false);
        assert!(run.diagnostics.is_empty(), "{:?}", run.diagnostics);
        for (source, target) in [("s", "m"), ("m", "growth"), ("growth", "s")] {
            assert_eq!(
                ltm_series(&run.results, &score_key(source, target), 0),
                vec![0.0, 1.0, 1.0, 1.0],
                "scale {scale}: {source} -> {target}"
            );
        }
        let loop_id = run.loop_through("growth");
        assert_eq!(
            ltm_series(
                &run.results,
                &format!("$\u{205A}ltm\u{205A}loop_score\u{205A}{loop_id}"),
                0
            ),
            vec![0.0, 1.0, 1.0, 1.0],
            "scale {scale}: the loop through the black box"
        );
    }
}

/// The first-step guard compares the clock with the start time exactly, so a
/// step is scored however close its time is to the start. Two time scales
/// straddle both arms of equation equality's tolerance: times within
/// `f64::EPSILON` of a zero start, and times a few ULPs past a large start.
/// Every generator shares one guard builder
/// (`ltm_augment::link_score_guard`), so the ordinary loop pins it for all.
#[test]
fn first_step_guard_is_invariant_to_the_time_scale() {
    // Powers of two keep every accumulated TIME exact.
    for (start, dt) in [(0.0, 2f64.powi(-53)), (2f64.powi(30), 2f64.powi(-22))] {
        let project = TestProject::new("time_scaled_loop")
            .with_sim_time(start, start + 5.0 * dt, dt)
            .stock("s", "1", &["growth"], &[], None)
            .flow("growth", &format!("0.1 * s / {}", dt), None)
            .build_datamodel();
        let run = ltm_run(&project, false);
        assert_eq!(ltm_series(&run.results, "time", 0)[1], start + dt);
        for (source, target) in [("s", "growth"), ("growth", "s")] {
            assert_eq!(
                ltm_series(&run.results, &score_key(source, target), 0),
                vec![0.0, 1.0, 1.0, 1.0, 1.0, 1.0],
                "start {start}, dt {dt}: {source} -> {target}"
            );
        }
    }
}
