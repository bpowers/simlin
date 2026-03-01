// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::compile_ltm_equation_fragment;
use crate::datamodel;
use crate::db::{SimlinDb, sync_from_datamodel};

fn phase_has_sym_load_prev(phase: &Option<crate::compiler::symbolic::PerVarBytecodes>) -> bool {
    phase.as_ref().is_some_and(|bc| {
        bc.symbolic.code.iter().any(|op| {
            matches!(
                op,
                crate::compiler::symbolic::SymbolicOpcode::SymLoadPrev { .. }
            )
        })
    })
}

#[test]
fn test_ltm_previous_module_var_uses_module_expansion() {
    let project = datamodel::Project {
        name: "ltm_prev_module_regression".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "producer".to_string(),
                    model_name: "producer".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
                    equation: datamodel::Equation::Scalar("TIME".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
        ],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    let fragment = compile_ltm_equation_fragment(
        &db,
        "$⁚ltm⁚test_prev_module",
        "PREVIOUS(producer)",
        source_model,
        sync.project,
    )
    .expect("LTM equation should compile");

    assert!(
        !phase_has_sym_load_prev(&fragment.fragment.initial_bytecodes),
        "initial phase should not use SymLoadPrev for PREVIOUS(module_var)",
    );
    assert!(
        !phase_has_sym_load_prev(&fragment.fragment.flow_bytecodes),
        "flow phase should not use SymLoadPrev for PREVIOUS(module_var)",
    );
    assert!(
        !phase_has_sym_load_prev(&fragment.fragment.stock_bytecodes),
        "stock phase should not use SymLoadPrev for PREVIOUS(module_var)",
    );
}
