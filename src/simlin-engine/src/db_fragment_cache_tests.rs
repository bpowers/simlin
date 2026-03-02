// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use salsa::plumbing::AsId;

use crate::datamodel;
use crate::db::{SimlinDb, compile_var_fragment, sync_from_datamodel_incremental};

#[test]
fn test_compile_var_fragment_caching() {
    // AC1.1: Changing one variable's equation (same deps) should only
    // recompile that variable. Other variables' fragments should remain cached.
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "cache_test".to_string(),
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
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "alpha".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "beta".to_string(),
                    equation: datamodel::Equation::Scalar("20".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    };
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

    // Prime the cache and capture a stable pointer for beta's fragment query.
    let (model_id_before, project_id_before, beta_var_id_before, beta_frag1, beta_ptr_before) = {
        let sync1 = state1.to_sync_result();
        let model = sync1.models["main"].source;
        let alpha_var = sync1.models["main"].variables["alpha"].source;
        let beta_var = sync1.models["main"].variables["beta"].source;

        let alpha_result1 =
            compile_var_fragment(&db, alpha_var, model, sync1.project, true, vec![]);
        let beta_result1 = compile_var_fragment(&db, beta_var, model, sync1.project, true, vec![]);
        assert!(alpha_result1.is_some());
        assert!(beta_result1.is_some());

        (
            model.as_id(),
            sync1.project.as_id(),
            beta_var.as_id(),
            beta_result1.as_ref().unwrap().fragment.clone(),
            beta_result1 as *const _,
        )
    };

    // Change only alpha.
    let mut project2 = project.clone();
    project2.models[0].variables[0] = datamodel::Variable::Aux(datamodel::Aux {
        ident: "alpha".to_string(),
        equation: datamodel::Equation::Scalar("20".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let model2 = sync2.models["main"].source;
    let beta_var2 = sync2.models["main"].variables["beta"].source;

    assert_eq!(
        project_id_before,
        sync2.project.as_id(),
        "project handle should remain stable for equation-only edits"
    );
    assert_eq!(
        model_id_before,
        model2.as_id(),
        "model handle should remain stable for equation-only edits"
    );
    assert_eq!(
        beta_var_id_before,
        beta_var2.as_id(),
        "unchanged variable handle should remain stable across sync"
    );

    let alpha_var2 = sync2.models["main"].variables["alpha"].source;
    let alpha_result2 = compile_var_fragment(&db, alpha_var2, model2, sync2.project, true, vec![]);
    assert!(alpha_result2.is_some());

    let beta_result2 = compile_var_fragment(&db, beta_var2, model2, sync2.project, true, vec![]);
    assert!(beta_result2.is_some());
    let beta_ptr_after = beta_result2 as *const _;
    assert_eq!(
        beta_frag1,
        beta_result2.as_ref().unwrap().fragment,
        "beta fragment should be unchanged when only alpha's equation changes"
    );
    assert_eq!(
        beta_ptr_before, beta_ptr_after,
        "beta fragment query should be a cache hit (pointer-equal) when only alpha changes"
    );
}
