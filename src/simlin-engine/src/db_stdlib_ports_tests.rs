// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;
use crate::testutils::{x_aux, x_model};

#[test]
fn test_stdlib_composite_ports_include_dynamic_module_inputs() {
    let ports = get_stdlib_composite_ports();

    let smth1_ports = ports
        .get(&Ident::new("stdlib⁚smth1"))
        .expect("smth1 should have cached composite ports");
    assert!(
        smth1_ports.contains(&Ident::new("input")),
        "smth1 input should have a causal pathway to output: {smth1_ports:?}"
    );
    assert!(
        !smth1_ports.contains(&Ident::new("flow")),
        "intermediate variables must not appear as composite ports: {smth1_ports:?}"
    );

    for model_name in ["stdlib⁚delay1", "stdlib⁚delay3", "stdlib⁚trend"] {
        assert!(
            ports.contains_key(&Ident::new(model_name)),
            "{model_name} should be present in the stdlib composite-port cache"
        );
    }
    assert!(
        !ports.contains_key(&Ident::new("stdlib⁚previous")),
        "PREVIOUS is infrastructure and must not get composite ports"
    );
}

#[test]
fn test_link_score_equation_text_uses_composite_for_smth1_module_input() {
    let project = datamodel::Project {
        name: "smth1_composite_link".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![x_aux("x", "10", None), x_aux("s", "SMTH1(x, 5)", None)],
        )],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let edges = model_causal_edges(&db, source_model, sync.project);
    let module_name = edges
        .edges
        .get("x")
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.contains("smth1") && !target.contains('·'))
        })
        .cloned()
        .expect("SMTH1 expansion should create a module node reachable from x");

    let to_var = reconstruct_single_variable(&db, source_model, sync.project, &module_name)
        .expect("module instance should reconstruct");
    let composite_ports = get_stdlib_composite_ports();
    let (input_source, model_name, port_name) = match &to_var {
        crate::variable::Variable::Module {
            model_name, inputs, ..
        } => (
            inputs
                .iter()
                .find(|input| input.dst == Ident::new("input"))
                .map(|input| input.src.clone())
                .unwrap_or_else(|| {
                    panic!("module instance inputs did not include 'input': {inputs:?}")
                }),
            model_name.clone(),
            inputs
                .iter()
                .find(|input| input.dst == Ident::new("input"))
                .map(|input| input.dst.clone())
                .unwrap_or_else(|| {
                    panic!("module instance inputs did not include 'input': {inputs:?}")
                }),
        ),
        _ => panic!("expected {module_name} to reconstruct as a module variable"),
    };
    let link_id = LtmLinkId::new(&db, input_source.to_string(), module_name.clone());
    assert!(
        composite_ports.contains_key(&model_name),
        "composite-port cache should contain reconstructed module model {model_name:?}"
    );
    assert!(
        composite_ports
            .get(&model_name)
            .is_some_and(|ports| ports.contains(&port_name)),
        "composite-port cache should contain {port_name:?} for {model_name:?}"
    );

    let lsv = link_score_equation_text(&db, link_id, source_model, sync.project)
        .as_ref()
        .expect("link score should be generated for the module input link");

    assert_eq!(
        lsv.equation,
        format!("\"{module_name}·$⁚ltm⁚composite⁚input\""),
        "dynamic module inputs should use cached composite references"
    );
}
