// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::sync::Arc;

use salsa::plumbing::AsId;

use crate::datamodel;
use crate::db::{
    DiagnosticSeverity, ModuleInputSet, SimlinDb, assemble_module, assemble_simulation,
    collect_all_diagnostics, compile_project_incremental, compile_var_fragment, compute_layout,
    model_all_diagnostics, model_dependency_graph, sync_from_datamodel,
    sync_from_datamodel_incremental,
};
use crate::test_common::TestProject;

/// Build a minimal `aux` variable with a scalar equation (keeps the test
/// bodies below readable -- the datamodel `Aux` literal is verbose).
fn scalar_aux(ident: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

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
            macro_spec: None,
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

        let alpha_result1 = compile_var_fragment(
            &db,
            alpha_var,
            model,
            sync1.project,
            ModuleInputSet::empty(&db),
        );
        let beta_result1 = compile_var_fragment(
            &db,
            beta_var,
            model,
            sync1.project,
            ModuleInputSet::empty(&db),
        );
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
    let alpha_result2 = compile_var_fragment(
        &db,
        alpha_var2,
        model2,
        sync2.project,
        ModuleInputSet::empty(&db),
    );
    assert!(alpha_result2.is_some());

    let beta_result2 = compile_var_fragment(
        &db,
        beta_var2,
        model2,
        sync2.project,
        ModuleInputSet::empty(&db),
    );
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));

    assert!(
        dep_graph.dt_dependencies["a"].contains("b"),
        "a = PREVIOUS(b) + b must keep b as a same-step dependency"
    );
}

#[test]
fn test_previous_lagged_feedback_is_acyclic() {
    let tp = TestProject::new("prev_lag_vm")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("a", "PREVIOUS(b)", None)
        .aux("b", "a + 1", None);

    let results = tp
        .run_vm()
        .expect("VM should compile/run lagged PREVIOUS feedback without cycles");
    let a = results.get("a").expect("missing a results");
    let b = results.get("b").expect("missing b results");

    // a = PREVIOUS(b), b = a + 1
    // t=0: a=0 (PREVIOUS returns 0 at initial step), b=0+1=1
    // t=1: a=PREVIOUS(b)=1 (previous step's b), b=1+1=2
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));

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
                macro_spec: None,
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
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));

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
                macro_spec: None,
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
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));

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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let a_var = sync.models["main"].variables["a"].source;
    let deps = crate::db::variable_direct_dependencies(
        &db,
        a_var,
        sync.project,
        crate::db::ModuleIdentContext::new(&db, vec![]),
        ModuleInputSet::empty(&db),
    );
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));

    assert!(
        deps.dt_deps.contains("b"),
        "INIT(b) should remain in direct deps for fragment compilation context"
    );
    assert!(
        deps.init_referenced_vars.contains("b"),
        "INIT(b) should still track b for initials runlist seeding"
    );
    assert!(
        !dep_graph.dt_dependencies["a"].contains("b"),
        "INIT(b) should be excluded from same-step dt ordering edges"
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));

    assert!(
        dep_graph.dt_dependencies["a"].contains("b"),
        "a = INIT(b) + b must keep b as a same-step dependency"
    );
}

#[test]
fn test_previous_plus_init_does_not_keep_current_step_dependency() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "prev_plus_init_dep".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("PREVIOUS(b) + INIT(b)".to_string()),
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
                    compat: datamodel::Compat {
                        active_initial: Some("1".to_string()),
                        ..datamodel::Compat::default()
                    },
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project, ModuleInputSet::empty(&db));

    assert!(
        !dep_graph.has_cycle,
        "PREVIOUS+INIT lagged/snapshot refs should not create dt cycles when initials are acyclic"
    );
    let a_dt = dep_graph
        .dt_dependencies
        .get("a")
        .expect("missing dt deps for a");
    assert!(
        !a_dt.contains("b"),
        "a = PREVIOUS(b) + INIT(b) should not keep b as a same-step dependency"
    );
}

#[test]
fn test_compile_fragment_init_expression_temp_arg_compiles() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "init_expr_fragment".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "frozen".to_string(),
                    equation: datamodel::Equation::Scalar("INIT(y + 1)".to_string()),
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
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let frozen_var = sync.models["main"].variables["frozen"].source;
    let fragment = compile_var_fragment(
        &db,
        frozen_var,
        model,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    assert!(
        fragment.is_some(),
        "INIT(expr) should compile in fragment mode with generated temp-arg metadata"
    );
}

#[test]
fn test_compile_fragment_init_dep_kept_for_active_initial_override() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "init_active_initial_fragment".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("INIT(y)".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        active_initial: Some("0".to_string()),
                        ..datamodel::Compat::default()
                    },
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let x_var = sync.models["main"].variables["x"].source;
    let fragment =
        compile_var_fragment(&db, x_var, model, sync.project, ModuleInputSet::empty(&db));
    assert!(
        fragment.is_some(),
        "INIT(y) with active_initial override should still compile in fragment mode"
    );
}

#[test]
fn test_init_feedback_path_is_acyclic() {
    let project = datamodel::Project {
        name: "init_lag".to_string(),
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
                    compat: datamodel::Compat {
                        active_initial: Some("1".to_string()),
                        ..datamodel::Compat::default()
                    },
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let diags = collect_all_diagnostics(&db, sync.project);
    let errors: Vec<_> = diags
        .iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "should have no errors for INIT feedback with active_initial; got: {errors:?}"
    );
}

#[test]
fn test_module_input_branch_prunes_previous_only_dt_dep() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "module_input_prev_branch".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "src".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "m".to_string(),
                        model_name: "submodel".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "src".to_string(),
                            dst: "m.input".to_string(),
                        }],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
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
                        ident: "y".to_string(),
                        equation: datamodel::Equation::Scalar("2".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "z".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "if isModuleInput(input) then PREVIOUS(y) else y".to_string(),
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
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let sub_model = sync.models["submodel"].source;

    let active = model_dependency_graph(
        &db,
        sub_model,
        sync.project,
        ModuleInputSet::from_names(&db, &["input".to_string()]),
    );
    assert!(
        !active.dt_dependencies["z"].contains("y"),
        "active isModuleInput branch uses PREVIOUS(y), so z should not have same-step dt dep on y"
    );

    let inactive = model_dependency_graph(&db, sub_model, sync.project, ModuleInputSet::empty(&db));
    assert!(
        inactive.dt_dependencies["z"].contains("y"),
        "inactive branch falls back to y, so z should depend on y"
    );
}

#[test]
fn test_module_input_branch_prunes_init_only_dt_dep() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "module_input_init_branch".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "src".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "m".to_string(),
                        model_name: "submodel".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "src".to_string(),
                            dst: "m.input".to_string(),
                        }],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
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
                        ident: "y".to_string(),
                        equation: datamodel::Equation::Scalar("2".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "z".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "if isModuleInput(input) then INIT(y) else y".to_string(),
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
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let sync = sync_from_datamodel(&db, &project);
    let sub_model = sync.models["submodel"].source;

    let active = model_dependency_graph(
        &db,
        sub_model,
        sync.project,
        ModuleInputSet::from_names(&db, &["input".to_string()]),
    );
    assert!(
        !active.dt_dependencies["z"].contains("y"),
        "active isModuleInput branch uses INIT(y), so z should not have same-step dt dep on y"
    );

    let inactive = model_dependency_graph(&db, sub_model, sync.project, ModuleInputSet::empty(&db));
    assert!(
        inactive.dt_dependencies["z"].contains("y"),
        "inactive branch falls back to y, so z should depend on y"
    );
}

/// A no-op recompile (no input changes) must be a pure salsa cache hit for
/// `assemble_simulation`: zero re-assembly work. Proof technique mirrors
/// `test_compile_var_fragment_caching` -- the tracked fn returns its memoized
/// value cloned out, and because the success payload is an `Arc`, a cache hit
/// hands back the SAME allocation (pointer-equal). A re-execution would mint a
/// fresh `Arc`, so `Arc::ptr_eq` failing loudly proves assembly re-ran.
#[test]
fn test_assemble_simulation_noop_recompile_is_cache_hit() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "assemble_noop_cache".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
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
            variables: vec![scalar_aux("alpha", "10"), scalar_aux("beta", "alpha + 1")],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    // First sync + assemble primes the cache.
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let project1 = state1.to_sync_result().project;
    let sim1 = assemble_simulation(&db, project1, "main".to_string())
        .expect("first assemble_simulation should succeed");

    // Re-sync the IDENTICAL datamodel: handles are reused, no input field
    // value changes, so every transitively-read input is bit-identical.
    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));
    let project2 = state2.to_sync_result().project;
    let sim2 = assemble_simulation(&db, project2, "main".to_string())
        .expect("second assemble_simulation should succeed");

    assert!(
        Arc::ptr_eq(&sim1, &sim2),
        "no-op recompile must be a salsa cache hit for assemble_simulation \
         (the Arc payload must be pointer-equal); assembly re-ran instead"
    );

    // Structural equality sanity check: the cached simulation is byte-for-byte
    // what `compile_project_incremental` returns (its public owned form).
    let owned = compile_project_incremental(&db, project2, "main")
        .expect("compile_project_incremental should succeed");
    assert_eq!(
        owned, *sim2,
        "compile_project_incremental's CompiledSimulation must equal the cached one"
    );
}

/// The db-owned `sync` API must preserve incrementality without the caller
/// threading `prev_state`: a no-op `db.sync` of the SAME datamodel reuses the
/// db's internal handles, so the next assemble is a salsa cache hit (the
/// `Arc<CompiledSimulation>` payload is pointer-equal). This is the
/// footgun-proof counterpart to `test_assemble_simulation_noop_recompile_is_cache_hit`.
#[test]
fn test_db_sync_noop_recompile_is_cache_hit() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "db_sync_noop_cache".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
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
            variables: vec![scalar_aux("alpha", "10"), scalar_aux("beta", "alpha + 1")],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    // First sync primes the cache; the db retains the handles internally.
    let project1 = db.sync(&project);
    let sim1 = assemble_simulation(&db, project1, "main".to_string())
        .expect("first assemble_simulation should succeed");

    // Re-sync the IDENTICAL datamodel WITHOUT threading any prior state by
    // hand -- the db reuses its own `sync_state`, so no input field value
    // changes and the assemble is a cache hit.
    let project2 = db.sync(&project);
    let sim2 = assemble_simulation(&db, project2, "main".to_string())
        .expect("second assemble_simulation should succeed");

    assert!(
        Arc::ptr_eq(&sim1, &sim2),
        "no-op db.sync recompile must be a salsa cache hit (the Arc payload must \
         be pointer-equal); the db-owned sync state failed to preserve incrementality"
    );

    // `current_source_project` returns the handle from the most recent sync.
    assert!(
        db.current_source_project() == Some(project2),
        "current_source_project must reflect the latest db.sync"
    );
}

/// Editing ONE variable in the root model re-assembles the root module but
/// cache-hits an UNCHANGED submodule: `assemble_module` is memoized per
/// `(model, project, is_root, module_inputs)`, and an equation-only edit to a
/// root variable that the submodel never reads leaves the submodel's assembled
/// `Arc<CompiledModule>` pointer-stable while the root's changes.
#[test]
fn test_assemble_module_unchanged_submodule_is_cache_hit() {
    let mut db = SimlinDb::default();
    // `main` holds an independent aux `driver` plus a module `sub` wiring its
    // `src` into `producer.input`; `producer` is the submodel we expect to
    // cache-hit when only `driver` changes.
    let make_project = |driver_eq: &str| datamodel::Project {
        name: "assemble_submodule_cache".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 5.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    scalar_aux("driver", driver_eq),
                    scalar_aux("src", "1"),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "sub".to_string(),
                        model_name: "producer".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "src".to_string(),
                            dst: "sub.input".to_string(),
                        }],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
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
                    scalar_aux("output", "input * 2"),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let project_a = make_project("100");
    let state1 = sync_from_datamodel_incremental(&mut db, &project_a, None);
    let (main1, producer1, project1) = {
        let sync1 = state1.to_sync_result();
        (
            sync1.models["main"].source,
            sync1.models["producer"].source,
            sync1.project,
        )
    };

    // `sub` wires `src -> producer.input`, so producer's instance input set is
    // `{input}` -- assemble it directly with that key (mirroring how
    // assemble_simulation builds the module-input set). The interned
    // `ModuleInputSet` borrows `db`, so it is built fresh at each use site
    // around the later `&mut db` sync; interning dedups it to the same id.
    let root_main1 = assemble_module(&db, main1, project1, true, ModuleInputSet::empty(&db))
        .expect("root assemble_module should succeed");
    let sub_producer1 = assemble_module(
        &db,
        producer1,
        project1,
        false,
        ModuleInputSet::from_names(&db, &["input".to_string()]),
    )
    .expect("submodule assemble_module should succeed");

    // Change ONLY `main.driver`'s equation. `producer` never reads `driver`,
    // so producer's assembly inputs are bit-identical across the edit.
    let project_b = make_project("200");
    let state2 = sync_from_datamodel_incremental(&mut db, &project_b, Some(&state1));
    let (main2, producer2, project2) = {
        let sync2 = state2.to_sync_result();
        (
            sync2.models["main"].source,
            sync2.models["producer"].source,
            sync2.project,
        )
    };

    let root_main2 = assemble_module(&db, main2, project2, true, ModuleInputSet::empty(&db))
        .expect("root assemble_module should succeed after edit");
    let sub_producer2 = assemble_module(
        &db,
        producer2,
        project2,
        false,
        ModuleInputSet::from_names(&db, &["input".to_string()]),
    )
    .expect("submodule assemble_module should succeed after edit");

    assert!(
        Arc::ptr_eq(&sub_producer1, &sub_producer2),
        "the UNCHANGED submodule (producer) must be a salsa cache hit \
         (pointer-equal Arc) when only a root-model variable changes; it re-assembled"
    );
    assert!(
        !Arc::ptr_eq(&root_main1, &root_main2),
        "the root module (main), whose `driver` variable changed, must re-assemble \
         (a fresh Arc), not cache-hit"
    );
}

/// Observe that dropping `is_root` makes the diagnostic-pass and assembly
/// fragments for a submodule variable share ONE salsa cache entry.
///
/// Before the change, the diagnostic pass compiled every model's variables
/// with `is_root = true` while assembly compiled a submodule with
/// `is_root = false`, so a submodule variable was keyed twice on the
/// differing flag and produced two cache entries. Now `compile_var_fragment`
/// takes no `is_root`, so for the same `(var, model, project, inputs)` there
/// is exactly one cache entry and one fragment value, regardless of caller
/// role. We prove it by checking the fragment a submodule variable gets
/// during the (root-context) diagnostic pass is byte-identical to the one
/// the (submodule-context) assembly path uses.
#[test]
fn test_submodule_fragment_shared_between_diagnostics_and_assembly() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "shared_fragment".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        source: None,
        ai_information: None,
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "sub".to_string(),
                    model_name: "submodel".to_string(),
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
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: vec![scalar_aux("output", "time * 3")],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
    };

    let sync = sync_from_datamodel(&db, &project);
    let submodel = sync.models["submodel"].source;
    let output_var = sync.models["submodel"].variables["output"].source;

    // The diagnostic pass compiles every model's variables (including the
    // submodel's) via `compile_var_fragment` with the empty input set -- the
    // SAME call assembly makes for this submodule (it has no module inputs).
    model_all_diagnostics(&db, submodel, sync.project);

    // The submodule is assembled as a sub-model (the role that was previously
    // `is_root = false`). Its `output` fragment is produced by the same
    // `compile_var_fragment` query the diagnostic pass already populated.
    let from_diag_pass = compile_var_fragment(
        &db,
        output_var,
        submodel,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    let from_assembly = compile_var_fragment(
        &db,
        output_var,
        submodel,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    let frag_diag = from_diag_pass
        .as_ref()
        .expect("submodule output fragment should compile");
    let frag_asm = from_assembly
        .as_ref()
        .expect("submodule output fragment should compile");

    // Byte-identical: there is one cache entry, role-independent. (If the two
    // roles still produced different fragments the symbolic stream would
    // differ -- the regression this guards against.)
    assert_eq!(
        frag_diag.fragment, frag_asm.fragment,
        "the submodule variable's fragment must be byte-identical for the \
         diagnostic-pass and assembly callers -- they now share one cache entry"
    );

    // And the whole project still assembles and runs through the root path.
    assemble_simulation(&db, sync.project, "main".to_string())
        .expect("project with a submodule should assemble");
}

/// Migration guard for dropping `is_root` from the tracked layout/compile
/// queries: the root +`IMPLICIT_VAR_COUNT` shift now lives in two separate
/// machineries -- `assemble_module`'s root path (via
/// `VariableLayout::root_shifted`) and `calc_flattened_offsets_incremental`'s
/// own root reservation. They MUST stay in perfect lockstep; a divergence (an
/// off-by-IMPLICIT_VAR_COUNT, a different ordering, a missing global) would
/// corrupt every result slot. This test fails loudly if they ever diverge.
///
/// It uses a model with a submodule so the nested module-decl `off`
/// relocation is exercised: the submodule is assembled with the unshifted
/// body layout and the parent relocates it via the (root-shifted) module-decl
/// offset. It also uses a SMOOTH builtin so the SMOOTH/DELAY implicit-var
/// section is cross-checked: that is the ONE section
/// `calc_flattened_offsets_incremental` computes INDEPENDENTLY (via its
/// running `i`-arithmetic) rather than reading `root_shifted()` directly, so
/// it is the divergence-prone section the entry-for-entry loop must cover.
/// (The body section is also covered by the loop; the LTM section is
/// tautologically consistent because both sides read `root_shifted()`.)
#[test]
fn test_is_root_shift_machineries_in_lockstep() {
    let db = SimlinDb::default();
    let project = datamodel::Project {
        name: "lockstep".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        source: None,
        ai_information: None,
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "aaa".to_string(),
                        equation: datamodel::Equation::Scalar("time * 2".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "zzz".to_string(),
                        equation: datamodel::Equation::Scalar("aaa + 1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    // SMTH3 synthesizes implicit module/helper variables
                    // (`$⁚smoothed⁚…`), exercising the SMOOTH/DELAY implicit-var
                    // section that `calc_flattened_offsets_incremental` lays out
                    // with its own independent `i`-arithmetic.
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "smoothed".to_string(),
                        equation: datamodel::Equation::Scalar("SMTH3(aaa, 5)".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "sub".to_string(),
                        model_name: "submodel".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "submodel".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
                    equation: datamodel::Equation::Scalar("7".to_string()),
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
                macro_spec: None,
            },
        ],
    };

    let sync = sync_from_datamodel(&db, &project);
    let main_model = sync.models["main"].source;

    // The final root layout the assembler resolves against (body layout +
    // the single shared root shift).
    let root_layout = compute_layout(&db, main_model, sync.project).root_shifted();

    // Documented root layout invariant: implicit globals at fixed slots, the
    // first body variable at IMPLICIT_VAR_COUNT.
    assert_eq!(root_layout.get("time").expect("time").offset, 0);
    assert_eq!(root_layout.get("dt").expect("dt").offset, 1);
    assert_eq!(
        root_layout
            .get("initial_time")
            .expect("initial_time")
            .offset,
        2
    );
    assert_eq!(root_layout.get("final_time").expect("final_time").offset, 3);
    // `aaa` is the canonical-sorted first body variable, so it lands exactly
    // at the implicit-var boundary.
    assert_eq!(
        root_layout.get("aaa").expect("aaa").offset,
        crate::vm::IMPLICIT_VAR_COUNT,
        "the first body variable must start at IMPLICIT_VAR_COUNT in the root layout"
    );

    // The SEPARATE results-map machinery, computed as root.
    let flat = crate::db::calc_flattened_offsets_incremental(&db, sync.project, "main", true);

    // Lockstep: every name the flattened results map exposes at the top level
    // (the implicit globals + scalar body vars + SMOOTH/DELAY implicit vars;
    // submodule entries are dotted names absent from the parent layout) must
    // match the root-shifted layout offset entry-for-entry. We track which
    // implicit-var (`$⁚`-prefixed) names were cross-checked so the test can
    // assert it is non-vacuous for that independently-computed section.
    let mut implicit_names_checked = 0usize;
    for (name, (off, _size)) in &flat {
        if let Some(entry) = root_layout.get(name.as_str()) {
            assert_eq!(
                entry.offset,
                *off,
                "results-map offset for `{}` ({off}) diverged from the assembled \
                 root layout offset ({}) -- the two root-shift machineries are \
                 NOT in lockstep",
                name.as_str(),
                entry.offset
            );
            if name.as_str().starts_with('$') {
                implicit_names_checked += 1;
            }
        }
    }

    // Non-vacuity for the implicit-var section: SMTH3 synthesizes at least one
    // `$⁚`-prefixed implicit variable that appears in BOTH the root layout and
    // the flattened results map, so the entry-for-entry loop above actually
    // cross-checked `calc_flattened_offsets_incremental`'s independent
    // implicit-section arithmetic against `root_shifted()`. Without a SMOOTH/
    // DELAY builtin this count would be 0 and the section would be uncovered.
    assert!(
        implicit_names_checked > 0,
        "expected at least one SMOOTH/DELAY implicit variable cross-checked in \
         both the root layout and the flattened results map; the implicit-var \
         section would otherwise be uncovered. flat keys: {:?}",
        flat.keys().map(|k| k.as_str()).collect::<Vec<_>>()
    );

    // The submodule's `output` is relocated under `sub.output`. Its results
    // offset must equal the root layout's `sub` module-decl offset plus the
    // submodule's (body) offset for `output` (here 0), proving the nested
    // module-decl `off` relocation stays consistent with the root shift.
    let sub_entry = root_layout.get("sub").expect("sub module in root layout");
    let sub_output_off = flat
        .iter()
        .find(|(k, _)| k.as_str() == "sub·output")
        .map(|(_, (off, _))| *off)
        .expect("sub.output must appear in the flattened results map");
    assert_eq!(
        sub_output_off, sub_entry.offset,
        "submodule `output` results offset must equal the root `sub` module-decl \
         offset + its body offset (0)"
    );

    // End-to-end: the assembled simulation's offsets come from the SAME
    // `calc_flattened_offsets_incremental` call, and its n_slots is the root
    // layout's n_slots -- the final proof the two machineries produced one
    // consistent picture.
    let sim = assemble_simulation(&db, sync.project, "main".to_string())
        .expect("assemble_simulation should succeed");
    assert_eq!(
        sim.n_slots(),
        root_layout.n_slots,
        "assembled simulation n_slots must equal the root-shifted layout n_slots"
    );
    assert_eq!(sim.get_offset(&crate::common::Ident::new("time")), Some(0));
    assert_eq!(
        sim.get_offset(&crate::common::Ident::new("aaa")),
        Some(crate::vm::IMPLICIT_VAR_COUNT)
    );
}

/// One-shot stdlib injection: the stdlib `SourceModel`/`SourceVariable` salsa
/// inputs are built EXACTLY ONCE per `SimlinDb` session and reused unchanged on
/// every subsequent sync. This is the key win of `SimlinDb::stdlib_models`: if
/// a stdlib handle changed across a sync, salsa would treat the stdlib model as
/// modified and re-run every query that depends on it -- e.g. a SMOOTH
/// instantiation's compiled fragment -- on every unrelated user edit.
///
/// We prove it two ways across an unrelated edit (changing `aaa`, not the
/// SMOOTH-using `smoothed`):
///   1. the `stdlib⁚smth3` `SourceModel` handle id is identical, and
///      every stdlib variable handle id is identical; and
///   2. a stdlib variable's compiled fragment query is a pointer-equal cache
///      hit (salsa never re-ran it because none of its stdlib inputs changed).
#[test]
fn test_stdlib_inputs_are_one_shot_and_stable_across_syncs() {
    use crate::canonicalize;

    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "stdlib_one_shot".to_string(),
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
        source: None,
        ai_information: None,
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                scalar_aux("aaa", "time * 2"),
                // SMTH3 instantiates the `stdlib⁚smth3` module, so the synced
                // project carries the stdlib SourceModel/SourceVariable inputs.
                scalar_aux("smoothed", "SMTH3(aaa, 5)"),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
    };

    let smth3_key = canonicalize("stdlib\u{205A}smth3").into_owned();

    // First sync (fresh path -- builds the one-shot stdlib cache).
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

    // Capture the stdlib model + variable handle ids and a stable pointer for
    // one stdlib variable's compiled fragment query.
    let (smth3_model_id_before, stdlib_var_ids_before, frag1, frag_ptr_before, delay_time_var) = {
        let sync1 = state1.to_sync_result();
        let smth3 = sync1
            .models
            .get(&smth3_key)
            .expect("stdlib⁚smth3 must be present after a SMTH3 instantiation");
        assert!(
            smth3.is_stdlib,
            "the stdlib entry must be flagged is_stdlib"
        );

        let mut var_ids: Vec<(String, salsa::Id)> = smth3
            .variables
            .iter()
            .map(|(name, sv)| (name.clone(), sv.id.as_id()))
            .collect();
        var_ids.sort();

        let delay_time_var = smth3
            .variables
            .get(&canonicalize("delay_time").into_owned())
            .expect("smth3 has a delay_time variable")
            .source;

        let frag = compile_var_fragment(
            &db,
            delay_time_var,
            smth3.source,
            sync1.project,
            ModuleInputSet::empty(&db),
        );
        assert!(frag.is_some(), "stdlib variable fragment should compile");

        (
            smth3.source.as_id(),
            var_ids,
            frag.as_ref().unwrap().fragment.clone(),
            frag as *const _,
            delay_time_var,
        )
    };

    // An unrelated edit: change `aaa` only. The stdlib models are untouched.
    let mut project2 = project.clone();
    project2.models[0].variables[0] = scalar_aux("aaa", "time * 3");

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let smth3_after = sync2
        .models
        .get(&smth3_key)
        .expect("stdlib⁚smth3 must still be present after the edit");

    // (1) The stdlib SourceModel handle is byte-identical across syncs.
    assert_eq!(
        smth3_model_id_before,
        smth3_after.source.as_id(),
        "the stdlib⁚smth3 SourceModel handle must be the SAME salsa input \
         across syncs -- otherwise salsa re-creates (and invalidates) it"
    );

    // ...and every stdlib variable handle id is identical too.
    let mut var_ids_after: Vec<(String, salsa::Id)> = smth3_after
        .variables
        .iter()
        .map(|(name, sv)| (name.clone(), sv.id.as_id()))
        .collect();
    var_ids_after.sort();
    assert_eq!(
        stdlib_var_ids_before, var_ids_after,
        "every stdlib variable handle must be stable across syncs"
    );

    // (2) The stdlib variable's compiled-fragment query is a pointer-equal
    // cache hit: salsa did not re-run it, because none of its (stdlib) inputs
    // changed when only the unrelated user variable `aaa` was edited.
    let delay_time_var_after = smth3_after
        .variables
        .get(&canonicalize("delay_time").into_owned())
        .expect("smth3 still has a delay_time variable")
        .source;
    assert_eq!(
        delay_time_var.as_id(),
        delay_time_var_after.as_id(),
        "the stdlib delay_time SourceVariable handle must be stable"
    );

    let frag2 = compile_var_fragment(
        &db,
        delay_time_var_after,
        smth3_after.source,
        sync2.project,
        ModuleInputSet::empty(&db),
    );
    assert!(frag2.is_some());
    assert_eq!(
        frag1,
        frag2.as_ref().unwrap().fragment,
        "the stdlib variable fragment must be unchanged across an unrelated edit"
    );
    assert_eq!(
        frag_ptr_before, frag2 as *const _,
        "the stdlib variable fragment query must be a pointer-equal cache hit \
         across an unrelated user edit -- proving the stdlib salsa inputs are \
         stable and never re-created"
    );
}
