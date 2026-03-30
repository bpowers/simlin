// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;
use crate::testutils::{feedback_loop_project, x_aux, x_model};

#[test]
fn test_model_ltm_variables_generates_scores() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let ltm = model_ltm_variables(&db, model, result.project);

    assert!(!ltm.vars.is_empty(), "should generate LTM variables");

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
    assert!(has_loop_score, "should have loop score variables");

    for var in &ltm.vars {
        assert!(
            !var.equation.is_empty(),
            "var {} should have non-empty equation",
            var.name
        );
    }
}

#[test]
fn test_model_ltm_variables_stdlib_module() {
    let db = SimlinDb::default();
    let stdlib_model = crate::stdlib::get("smth1").expect("smth1 stdlib model should exist");

    let project = datamodel::Project {
        name: "smth1_ltm_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![stdlib_model],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["stdlib\u{205A}smth1"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_pathway = ltm.vars.iter().any(|v| v.name.contains("path"));
    assert!(has_pathway, "should have pathway variables");

    let has_composite = ltm.vars.iter().any(|v| v.name.contains("composite"));
    assert!(has_composite, "should have composite variables");

    let has_ilink = ltm.vars.iter().any(|v| v.name.contains("ilink"));
    assert!(
        !has_ilink,
        "no var name should contain 'ilink': {:?}",
        ltm.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

#[test]
fn test_model_ltm_variables_passthrough_module() {
    let db = SimlinDb::default();

    let project = datamodel::Project {
        name: "passthrough_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "passthrough",
            vec![
                x_aux("input", "0", None),
                x_aux("output", "input * 2", None),
            ],
        )],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["passthrough"].source;

    let ltm = model_ltm_variables(&db, model, sync.project);
    assert!(
        ltm.vars.is_empty(),
        "passthrough module with no stocks should produce no LTM vars"
    );
}

#[test]
fn test_model_ltm_variables_discovery_mode() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, model) = {
        let result = sync_from_datamodel(&db, &project);
        (result.project, result.models["main"].source)
    };

    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm = model_ltm_variables(&db, model, source_project);

    assert!(!ltm.vars.is_empty(), "should generate link score variables");

    let has_link_score = ltm.vars.iter().any(|v| v.name.contains("link_score"));
    assert!(has_link_score, "should have link score variables");

    let has_loop_score = ltm.vars.iter().any(|v| v.name.contains("loop_score"));
    assert!(
        !has_loop_score,
        "discovery mode should not have loop scores"
    );
}
