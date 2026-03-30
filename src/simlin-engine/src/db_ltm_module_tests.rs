// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for LTM compilation with models containing modules
//! (stdlib SMOOTH/DELAY and user-defined passthrough modules).

use super::*;
use crate::datamodel;
use crate::testutils::{x_aux, x_flow, x_model, x_module, x_stock};

/// AC1.1: A model with SMTH1 in a feedback loop generates LTM synthetic
/// variables including link_score entries when LTM is enabled, and the
/// layout allocates extra slots for them.
#[test]
fn test_ltm_smooth_model_compiles_with_ltm() {
    use salsa::Setter;

    let project = datamodel::Project {
        name: "smooth_feedback".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![
                x_aux("goal", "100", None),
                x_stock("level", "50", &["adjustment"], &[], None),
                x_aux("smoothed_level", "SMTH1(level, 3)", None),
                x_aux("gap", "goal - smoothed_level", None),
                x_flow("adjustment", "gap / 5", None),
            ],
        )],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, source_model, source_project);
    assert!(
        !ltm_vars.vars.is_empty(),
        "root model should have LTM synthetic variables for its feedback loop"
    );

    let has_link_score_var = ltm_vars.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(
        has_link_score_var,
        "LTM vars should include at least one link_score variable"
    );

    let n_slots_ltm = compute_layout(&db, source_model, source_project, true).n_slots;
    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_no_ltm = compute_layout(&db, source_model, source_project, true).n_slots;
    assert!(
        n_slots_ltm > n_slots_no_ltm,
        "LTM-enabled layout should have more slots: ltm={n_slots_ltm}, no_ltm={n_slots_no_ltm}"
    );
}

/// AC1.2: A model with DELAY1 in a feedback loop produces LTM synthetic
/// variables including link_score entries when LTM is enabled.
#[test]
fn test_ltm_delay_model_compiles() {
    use salsa::Setter;

    let project = datamodel::Project {
        name: "delay_feedback".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![
                x_aux("goal", "100", None),
                x_stock("level", "50", &["adjustment"], &[], None),
                x_aux("delayed_level", "DELAY1(level, 3)", None),
                x_aux("gap", "goal - delayed_level", None),
                x_flow("adjustment", "gap / 5", None),
            ],
        )],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, source_model, source_project);
    assert!(
        !ltm_vars.vars.is_empty(),
        "root model should have LTM synthetic variables for its DELAY1 feedback loop"
    );

    let has_link_score_var = ltm_vars.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(
        has_link_score_var,
        "LTM vars should include at least one link_score variable"
    );

    let n_slots_ltm = compute_layout(&db, source_model, source_project, true).n_slots;
    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_no_ltm = compute_layout(&db, source_model, source_project, true).n_slots;
    assert!(
        n_slots_ltm > n_slots_no_ltm,
        "LTM-enabled layout should have more slots: ltm={n_slots_ltm}, no_ltm={n_slots_no_ltm}"
    );
}

/// AC1.7: A model with a passthrough module (no internal stocks) compiles
/// with LTM enabled without errors. The module itself generates no LTM vars
/// because it has no feedback loops.
#[test]
fn test_ltm_passthrough_module_compiles() {
    use salsa::Setter;

    let project = datamodel::Project {
        name: "passthrough_module".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            x_model(
                "main",
                vec![
                    x_stock("level", "50", &["inflow"], &[], None),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "inflow".to_string(),
                        equation: datamodel::Equation::Scalar("scaler.scaled_output".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    x_aux("raw_input", "level * 0.1", None),
                    x_module("scaler", &[("raw_input", "input_val")], None),
                ],
            ),
            x_model(
                "scaler",
                vec![
                    x_aux("input_val", "0", None),
                    x_aux("scaled_output", "input_val * 2", None),
                ],
            ),
        ],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let source_project = {
        let sync = sync_from_datamodel(&db, &project);
        sync.project
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let compiled = compile_project_incremental(&db, source_project, "main")
        .expect("passthrough module model should compile with LTM enabled");

    // The main model has a feedback loop (level -> raw_input -> scaler -> inflow -> level)
    let has_ltm_offset = compiled.offsets.keys().any(|k| k.as_str().starts_with('$'));
    assert!(
        has_ltm_offset,
        "main model should have LTM variable offsets for its feedback loop"
    );
}

/// AC1.8: A model with two SMOOTH instances on different variables
/// generates independent LTM synthetic variables for each feedback path
/// when LTM is enabled.
#[test]
fn test_ltm_multiple_smooth_instances_compile() {
    use salsa::Setter;

    let project = datamodel::Project {
        name: "multi_smooth".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(0.5),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![
                x_stock("level_a", "50", &["adj_a"], &[], None),
                x_aux("smoothed_a", "SMTH1(level_a, 3)", None),
                x_aux("gap_a", "100 - smoothed_a", None),
                x_flow("adj_a", "gap_a / 5", None),
                x_stock("level_b", "30", &["adj_b"], &[], None),
                x_aux("smoothed_b", "SMTH1(level_b, 2)", None),
                x_aux("gap_b", "80 - smoothed_b", None),
                x_flow("adj_b", "gap_b / 3", None),
            ],
        )],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, source_model, source_project);
    let link_score_count = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.contains("link_score"))
        .count();
    assert!(
        link_score_count >= 2,
        "should have link_score vars for multiple feedback paths, got {link_score_count}"
    );

    let n_slots_ltm = compute_layout(&db, source_model, source_project, true).n_slots;

    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_no_ltm = compute_layout(&db, source_model, source_project, true).n_slots;

    assert!(
        n_slots_ltm > n_slots_no_ltm,
        "LTM-enabled layout should have more slots: ltm={n_slots_ltm}, no_ltm={n_slots_no_ltm}"
    );
}
