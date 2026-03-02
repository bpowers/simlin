// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use salsa::plumbing::AsId;

use crate::datamodel;
use crate::db::{
    SimlinDb, compile_var_fragment, model_dependency_graph, sync_from_datamodel,
    sync_from_datamodel_incremental,
};
use crate::test_common::TestProject;

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

#[test]
fn test_previous_lagged_feedback_does_not_create_cycle() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "prev_lag_cycle".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 3.0,
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
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("PREVIOUS(b)".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "b".to_string(),
                    equation: datamodel::Equation::Scalar("a + 1".to_string()),
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

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);
    assert!(
        !dep_graph.has_cycle,
        "PREVIOUS(b) should be treated as a lagged dependency, not a same-step cycle"
    );
}

#[test]
fn test_previous_plus_current_keeps_current_step_dependency() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "prev_plus_current_dep".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("PREVIOUS(b) + b".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "b".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
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

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    assert!(
        dep_graph.dt_dependencies["a"].contains("b"),
        "a = PREVIOUS(b) + b must keep b as a same-step dependency"
    );
}

#[test]
fn test_previous_lagged_feedback_interpreter_path_is_acyclic() {
    let tp = TestProject::new("prev_lag_interp")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("a", "PREVIOUS(b)", None)
        .aux("b", "a + 1", None);

    let results = tp
        .run_interpreter()
        .expect("interpreter path should compile/run lagged PREVIOUS feedback without cycles");
    let a = results.get("a").expect("missing a results");
    let b = results.get("b").expect("missing b results");

    assert_eq!(a[0], 0.0);
    assert_eq!(b[0], 1.0);
    assert_eq!(a[1], 1.0);
    assert_eq!(b[1], 2.0);
}

#[test]
fn test_active_initial_previous_is_lagged_in_initial_graph() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "prev_active_initial_lag".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        active_initial: Some("PREVIOUS(b)".to_string()),
                        ..datamodel::Compat::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "b".to_string(),
                    equation: datamodel::Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        active_initial: Some("a + 1".to_string()),
                        ..datamodel::Compat::default()
                    },
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    assert!(
        !dep_graph.has_cycle,
        "active_initial PREVIOUS references should be lagged and not induce initial-step cycles"
    );
    assert!(
        !dep_graph.initial_dependencies["a"].contains("b"),
        "a.active_initial = PREVIOUS(b) must not keep b as a same-step initial dependency"
    );
    assert!(
        dep_graph.initial_dependencies["b"].contains("a"),
        "control check: b.active_initial should still depend on a in the initial graph"
    );
}

#[test]
fn test_previous_module_output_is_pruned_from_dt_dependencies() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "prev_module_output_lag".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "source".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "sub".to_string(),
                        model_name: "producer".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "source".to_string(),
                            dst: "sub.input".to_string(),
                        }],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "a".to_string(),
                        equation: datamodel::Equation::Scalar("PREVIOUS(sub.output)".to_string()),
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
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "input".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "output".to_string(),
                        equation: datamodel::Equation::Scalar("input".to_string()),
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
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    assert!(
        !dep_graph.dt_dependencies["a"].contains("sub"),
        "a = PREVIOUS(sub.output) should not keep sub as a same-step dependency"
    );
}

#[test]
fn test_previous_module_output_keeps_non_lagged_same_module_dependency() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "prev_module_mixed_outputs".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "source".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "sub".to_string(),
                        model_name: "producer".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "source".to_string(),
                            dst: "sub.input".to_string(),
                        }],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "a".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "PREVIOUS(sub.out1) + sub.out2".to_string(),
                        ),
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
            },
            datamodel::Model {
                name: "producer".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "input".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            ..datamodel::Compat::default()
                        },
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "out1".to_string(),
                        equation: datamodel::Equation::Scalar("input".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "out2".to_string(),
                        equation: datamodel::Equation::Scalar("input * 2".to_string()),
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
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    assert!(
        dep_graph.dt_dependencies["a"].contains("sub"),
        "a = PREVIOUS(sub.out1) + sub.out2 must keep sub as a same-step dependency"
    );
}

#[test]
fn test_init_feedback_does_not_create_dt_cycle() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "init_lag_cycle".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("INIT(b)".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "b".to_string(),
                    equation: datamodel::Equation::Scalar("a + 1".to_string()),
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

    let sync = sync_from_datamodel(&db, &project);
    let a_var = sync.models["main"].variables["a"].source;
    let deps = crate::db::variable_direct_dependencies(&db, a_var, sync.project);

    assert!(
        !deps.dt_deps.contains("b"),
        "INIT(b) should not keep b as a same-step dt dependency"
    );
    assert!(
        deps.init_referenced_vars.contains("b"),
        "INIT(b) should still track b for initials runlist seeding"
    );
}

#[test]
fn test_init_plus_current_keeps_current_step_dependency() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "init_plus_current_dep".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("INIT(b) + b".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "b".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
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

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    assert!(
        dep_graph.dt_dependencies["a"].contains("b"),
        "a = INIT(b) + b must keep b as a same-step dependency"
    );
}
