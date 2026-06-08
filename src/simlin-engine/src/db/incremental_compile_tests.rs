// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Incremental compilation and acceptance-criteria tests for the salsa
//! pipeline, split out of `tests.rs` to keep that file under the 6000-line
//! per-file lint cap (GH #645).

use super::*;
use crate::datamodel;

use super::tests::simple_project;
// ── Incremental compilation tests ──────────────────────────────

fn two_var_project() -> datamodel::Project {
    datamodel::Project {
        name: "test".to_string(),
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
                    equation: datamodel::Equation::Scalar("alpha * 2".to_string()),
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
    }
}

#[test]
fn test_variable_dimensions_scalar() {
    let db = SimlinDb::default();
    let project = simple_project();
    let sync = sync_from_datamodel(&db, &project);

    let pop_var = sync.models["main"].variables["population"].source;
    let dims = variable_dimensions(&db, pop_var, sync.project);
    assert!(dims.is_empty());
}

#[test]
fn test_variable_size_scalar() {
    let db = SimlinDb::default();
    let project = simple_project();
    let sync = sync_from_datamodel(&db, &project);

    let pop_var = sync.models["main"].variables["population"].source;
    assert_eq!(variable_size(&db, pop_var, sync.project), 1);
}

#[test]
fn test_compute_layout_simple() {
    let db = SimlinDb::default();
    let project = two_var_project();
    let sync = sync_from_datamodel(&db, &project);

    let model = sync.models["main"].source;
    // `compute_layout` is now the role-independent *body* layout: no implicit
    // globals, body offsets start at 0.
    let layout = compute_layout(&db, model, sync.project);

    assert!(
        layout.get("time").is_none(),
        "the body layout must NOT contain the implicit global `time` -- it is \
         added only by the root shift at assembly"
    );
    let alpha_entry = layout.get("alpha").expect("alpha should be in layout");
    let beta_entry = layout.get("beta").expect("beta should be in layout");
    // Two user vars occupy offsets 0 and 1 (in canonical-sorted order).
    assert!(alpha_entry.offset < 2);
    assert!(beta_entry.offset < 2);
    assert_ne!(alpha_entry.offset, beta_entry.offset);
    assert_eq!(alpha_entry.size, 1);
    assert_eq!(beta_entry.size, 1);
    assert_eq!(layout.n_slots, 2);

    // The root shift relocates the body and inserts the implicit globals at
    // their fixed slots. This is the single shared shift that
    // `assemble_module`'s root path applies.
    let root = layout.root_shifted();
    let time_entry = root.get("time").expect("time should be in root layout");
    assert_eq!(time_entry.offset, 0);
    assert_eq!(time_entry.size, 1);
    assert_eq!(root.get("dt").expect("dt").offset, 1);
    assert_eq!(root.get("initial_time").expect("initial_time").offset, 2);
    assert_eq!(root.get("final_time").expect("final_time").offset, 3);
    // Body vars shifted past the implicit globals (offset >= 4).
    assert!(root.get("alpha").expect("alpha").offset >= 4);
    assert!(root.get("beta").expect("beta").offset >= 4);
    assert_eq!(root.n_slots, layout.n_slots + 4);
}

#[test]
fn test_compile_var_fragment_produces_result() {
    let db = SimlinDb::default();
    let project = two_var_project();
    let sync = sync_from_datamodel(&db, &project);

    let model = sync.models["main"].source;
    let alpha_var = sync.models["main"].variables["alpha"].source;

    let result = compile_var_fragment(
        &db,
        alpha_var,
        model,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    assert!(result.is_some(), "alpha should compile successfully");

    let frag = &result.as_ref().unwrap().fragment;
    assert_eq!(frag.ident, "alpha");
    // Alpha is an aux, should have flow_bytecodes (in the flows runlist)
    assert!(
        frag.flow_bytecodes.is_some() || frag.initial_bytecodes.is_some(),
        "alpha should produce bytecodes in at least one phase"
    );
}

#[test]
fn test_assemble_simulation_simple() {
    let db = SimlinDb::default();
    let project = two_var_project();
    let sync = sync_from_datamodel(&db, &project);

    let result = assemble_simulation(&db, sync.project, "main".to_string());
    assert!(
        result.is_ok(),
        "assemble_simulation failed: {:?}",
        result.err()
    );

    let compiled = result.unwrap();
    // Two user vars (alpha, beta) + 4 implicit (time, dt, initial_time, final_time) = 6
    assert_eq!(compiled.n_slots(), 6);

    // Verify offsets exist for the variables
    assert!(
        compiled
            .get_offset(&crate::common::Ident::new("alpha"))
            .is_some()
    );
    assert!(
        compiled
            .get_offset(&crate::common::Ident::new("beta"))
            .is_some()
    );
    assert!(
        compiled
            .get_offset(&crate::common::Ident::new("time"))
            .is_some()
    );
}

/// Teacup model: stock with flow, two constants. Matches the teacup.stmx
/// fixture used by TS engine tests.
fn teacup_project() -> datamodel::Project {
    datamodel::Project {
        name: "teacup".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 30.0,
            dt: datamodel::Dt::Dt(0.125),
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
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "teacup_temperature".to_string(),
                    equation: datamodel::Equation::Scalar("180".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec!["heat_loss_to_room".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        non_negative: true,
                        ..datamodel::Compat::default()
                    },
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "heat_loss_to_room".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "(teacup_temperature - room_temperature) / characteristic_time".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        non_negative: true,
                        ..datamodel::Compat::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "characteristic_time".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "room_temperature".to_string(),
                    equation: datamodel::Equation::Scalar("70".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
        }],
    }
}

/// Test that mimics the libsimlin XMILE flow: load teacup.stmx, sync to
/// DB via sync_from_datamodel_incremental (None prev), convert back with
/// to_sync_result(), then compile incrementally.
#[test]
fn test_incremental_teacup_via_persistent_sync() {
    use crate::vm::Vm;

    let dm_project = teacup_project();

    // Mirror the libsimlin path: use sync_from_datamodel_incremental with None prev
    let mut db = SimlinDb::default();
    let persistent_state = sync_from_datamodel_incremental(&mut db, &dm_project, None);

    // Now reconstruct SyncResult from PersistentSyncState (like simlin_sim_new does)
    let sync = persistent_state.to_sync_result();

    let incr_compiled = assemble_simulation(&db, sync.project, "main".to_string())
        .expect("incremental compilation failed");

    // Verify constant detection
    let room_temp_ident = crate::common::Ident::new("room_temperature");
    let incr_off = incr_compiled
        .get_offset(&room_temp_ident)
        .expect("no offset for room_temperature in incremental");
    assert!(
        incr_compiled.is_constant_offset(incr_off),
        "room_temperature should be detected as constant via persistent sync path"
    );

    // Run simulation and verify results
    let mut vm = Vm::new((*incr_compiled).clone()).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let temp_ident = crate::common::Ident::new("teacup_temperature");
    let temp_off = results.offsets[&temp_ident];
    let first_temp = results.data[temp_off];
    assert!(
        (first_temp - 180.0).abs() < 1e-10,
        "initial temperature should be 180, got {}",
        first_temp
    );
}

// ── AC acceptance-criteria tests ──────────────────────────────────

/// AC1.3/AC1.4: Adding or removing a variable reuses existing variables'
/// compile_var_fragment results (salsa cache hit) while compute_layout
/// changes to reflect the new variable set.
#[test]
fn test_ac1_3_ac1_4_fragment_reuse_on_add_remove() {
    let mut db = SimlinDb::default();
    let project = two_var_project();

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();
    let model1 = sync1.models["main"].source;

    // Prime layout cache
    let layout_ptr1 = compute_layout(&db, model1, sync1.project)
        as *const crate::compiler::symbolic::VariableLayout;

    // Add a new variable "gamma"
    let mut project2 = project.clone();
    project2.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "gamma".to_string(),
            equation: datamodel::Equation::Scalar("99".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let model2 = sync2.models["main"].source;

    // Clone fragment contents before mutation
    let alpha_frag1 = compile_var_fragment(
        &db,
        sync1.models["main"].variables["alpha"].source,
        model1,
        sync1.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    let beta_frag1 = compile_var_fragment(
        &db,
        sync1.models["main"].variables["beta"].source,
        model1,
        sync1.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    // Existing variables' fragments should be value-equal (salsa recomputes
    // but the symbolic bytecodes are independent of variable set size)
    let alpha_frag2 = compile_var_fragment(
        &db,
        sync2.models["main"].variables["alpha"].source,
        model2,
        sync2.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    let beta_frag2 = compile_var_fragment(
        &db,
        sync2.models["main"].variables["beta"].source,
        model2,
        sync2.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    assert_eq!(
        alpha_frag1, alpha_frag2,
        "AC1.3: alpha's fragment content should be unchanged after adding gamma"
    );
    assert_eq!(
        beta_frag1, beta_frag2,
        "AC1.3: beta's fragment content should be unchanged after adding gamma"
    );

    // Layout MUST change (gamma added)
    let layout_ptr2 = compute_layout(&db, model2, sync2.project)
        as *const crate::compiler::symbolic::VariableLayout;
    assert_ne!(
        layout_ptr1, layout_ptr2,
        "AC1.3: compute_layout should change when a variable is added"
    );

    // Now remove gamma (AC1.4)
    let state3 = sync_from_datamodel_incremental(&mut db, &project, Some(&state2));
    let sync3 = state3.to_sync_result();
    let model3 = sync3.models["main"].source;

    let alpha_frag3 = compile_var_fragment(
        &db,
        sync3.models["main"].variables["alpha"].source,
        model3,
        sync3.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    let beta_frag3 = compile_var_fragment(
        &db,
        sync3.models["main"].variables["beta"].source,
        model3,
        sync3.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    assert_eq!(
        alpha_frag1, alpha_frag3,
        "AC1.4: alpha's fragment content should be unchanged after removing gamma"
    );
    assert_eq!(
        beta_frag1, beta_frag3,
        "AC1.4: beta's fragment content should be unchanged after removing gamma"
    );

    // Layout should change again (back to 2 variables)
    let layout_ptr3 = compute_layout(&db, model3, sync3.project)
        as *const crate::compiler::symbolic::VariableLayout;
    assert_ne!(
        layout_ptr2, layout_ptr3,
        "AC1.4: compute_layout should change when a variable is removed"
    );
}

/// AC1.5: Changing a dimension definition recompiles only variables that
/// use that dimension. Variables not referencing the dimension should
/// have their compile_var_fragment cached (via salsa backdating).
#[test]
fn test_ac1_5_dimension_change_selective_recompile() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "dim_test".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Region".to_string(),
            vec!["North".to_string(), "South".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "sales".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["Region".to_string()],
                        "10".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "price".to_string(),
                    equation: datamodel::Equation::Scalar("42".to_string()),
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
    let sync1 = state1.to_sync_result();
    let model1 = sync1.models["main"].source;

    // Prime caches and capture fragment content
    let price_frag1 = compile_var_fragment(
        &db,
        sync1.models["main"].variables["price"].source,
        model1,
        sync1.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    let sales_frag1 = compile_var_fragment(
        &db,
        sync1.models["main"].variables["sales"].source,
        model1,
        sync1.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    // Change dimension size: add "East" element
    let mut project2 = project.clone();
    project2.dimensions[0] = datamodel::Dimension::named(
        "Region".to_string(),
        vec!["North".to_string(), "South".to_string(), "East".to_string()],
    );

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let model2 = sync2.models["main"].source;

    // Price doesn't use the dimension, so its fragment should be value-equal
    let price_frag2 = compile_var_fragment(
        &db,
        sync2.models["main"].variables["price"].source,
        model2,
        sync2.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    assert_eq!(
        price_frag1, price_frag2,
        "AC1.5: price fragment should be unchanged (price doesn't use Region)"
    );

    // Sales uses the dimension, so its fragment should differ
    let sales_frag2 = compile_var_fragment(
        &db,
        sync2.models["main"].variables["sales"].source,
        model2,
        sync2.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .unwrap()
    .fragment
    .clone();

    assert_ne!(
        sales_frag1, sales_frag2,
        "AC1.5: sales fragment should be recomputed (sales uses Region)"
    );
}

/// AC1.6: Changing module connections in model B should not invalidate
/// model A's dependency graph. Cross-model isolation means the dep graph
/// for an unrelated model is a cache hit.
#[test]
fn test_ac1_6_cross_model_isolation() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "multi".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "model_a".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "x".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "y".to_string(),
                        equation: datamodel::Equation::Scalar("x + 1".to_string()),
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
                name: "model_b".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "sub".to_string(),
                    model_name: "model_a".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![datamodel::ModuleReference {
                        src: "input_a".to_string(),
                        dst: "x".to_string(),
                    }],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync1 = state1.to_sync_result();
    let model_a_src = sync1.models["model_a"].source;

    // Prime model_a's dep graph
    let graph_a_ptr1 =
        model_dependency_graph(&db, model_a_src, sync1.project, ModuleInputSet::empty(&db))
            as *const ModelDepGraphResult;

    // Change model_b's module connections
    let mut project2 = project.clone();
    if let datamodel::Variable::Module(ref mut m) = project2.models[1].variables[0] {
        m.references = vec![
            datamodel::ModuleReference {
                src: "input_a".to_string(),
                dst: "x".to_string(),
            },
            datamodel::ModuleReference {
                src: "input_b".to_string(),
                dst: "y".to_string(),
            },
        ];
    }

    let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
    let sync2 = state2.to_sync_result();
    let model_a_src2 = sync2.models["model_a"].source;

    // Model A's dep graph should be a cache hit (pointer-equal)
    let graph_a_ptr2 =
        model_dependency_graph(&db, model_a_src2, sync2.project, ModuleInputSet::empty(&db))
            as *const ModelDepGraphResult;
    assert_eq!(
        graph_a_ptr1, graph_a_ptr2,
        "AC1.6: model A's dependency graph should be cached when only model B changes"
    );
}

/// AC2.4: LTM variables for models with SMOOTH modules compute once and
/// are cached. Calling model_ltm_variables twice with unchanged inputs
/// returns pointer-equal results.
#[test]
fn test_ac2_4_stdlib_composite_scores_cached() {
    use super::model_ltm_variables;
    use crate::testutils::{x_aux, x_flow, x_model, x_stock};
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "smooth_cache_test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![
                x_stock("level", "50", &["adj"], &[], None),
                x_aux("smoothed", "SMTH1(level, 3)", None),
                x_aux("gap", "100 - smoothed", None),
                x_flow("adj", "gap / 5", None),
            ],
        )],
        source: None,
        ai_information: None,
    };
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    // First call: compute
    let result1 = model_ltm_variables(&db, model, source_project) as *const LtmVariablesResult;

    // Second call: should be cached (pointer-equal)
    let result2 = model_ltm_variables(&db, model, source_project) as *const LtmVariablesResult;

    assert_eq!(
        result1, result2,
        "AC2.4: model_ltm_variables should be cached on unchanged inputs"
    );
}

/// Test loading teacup.stmx via open_xmile and running through the
/// full incremental compilation path, mirroring the libsimlin XMILE flow.
/// Catches regressions where display names with spaces (from XMILE) don't
/// match canonical names used in dependency graphs and variable maps.
#[test]
fn test_incremental_teacup_xmile_file() {
    use crate::vm::Vm;
    use std::io::BufReader;

    let xmile_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("src/pysimlin/tests/fixtures/teacup.stmx");
    if !xmile_path.exists() {
        panic!("teacup.stmx not found at {:?}", xmile_path);
    }
    let xmile_data = std::fs::read(&xmile_path).unwrap();
    let mut reader = BufReader::new(xmile_data.as_slice());
    let dm_project = crate::open_xmile(&mut reader).expect("failed to parse teacup.stmx");

    let engine_project: crate::project::Project = dm_project.into();
    let mut db = SimlinDb::default();
    let persistent_state =
        sync_from_datamodel_incremental(&mut db, &engine_project.datamodel, None);

    let sync = persistent_state.to_sync_result();

    let incr_compiled = assemble_simulation(&db, sync.project, "main".to_string())
        .expect("incremental compilation failed");

    // Constant detection must work for XMILE-loaded models
    let room_temp_ident = crate::common::Ident::new("room_temperature");
    let incr_off = incr_compiled
        .get_offset(&room_temp_ident)
        .expect("no offset for room_temperature");
    assert!(
        incr_compiled.is_constant_offset(incr_off),
        "room_temperature should be detected as constant (XMILE path)"
    );

    // Simulation must produce correct results
    let mut vm = Vm::new((*incr_compiled).clone()).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let temp_ident = crate::common::Ident::new("teacup_temperature");
    let temp_off = results.offsets[&temp_ident];
    let first_temp = results.data[temp_off];
    assert!(
        (first_temp - 180.0).abs() < 1e-10,
        "initial temperature should be 180, got {}",
        first_temp
    );
}

// ====================================================================
// Fix #3: model-specific sim_specs override
// ====================================================================

#[test]
fn test_model_sim_specs_override() {
    let project_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 10.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let model_specs = datamodel::SimSpecs {
        start: 5.0,
        stop: 20.0,
        dt: datamodel::Dt::Dt(0.5),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let dm_project = datamodel::Project {
        name: "test_override".to_string(),
        sim_specs: project_specs.clone(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: Some(model_specs.clone()),
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
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
        }],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm_project);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let specs = &compiled.specs;

    assert!(
        (specs.start - 5.0).abs() < f64::EPSILON,
        "start should be 5.0 (from model specs), got {}",
        specs.start
    );
    assert!(
        (specs.stop - 20.0).abs() < f64::EPSILON,
        "stop should be 20.0 (from model specs), got {}",
        specs.stop
    );
    assert!(
        (specs.dt - 0.5).abs() < f64::EPSILON,
        "dt should be 0.5 (from model specs), got {}",
        specs.dt
    );
}

#[test]
fn test_model_sim_specs_defaults_to_project() {
    let project_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 10.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let dm_project = datamodel::Project {
        name: "test_no_override".to_string(),
        sim_specs: project_specs.clone(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
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
        }],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm_project);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let specs = &compiled.specs;

    assert!(
        (specs.start - 0.0).abs() < f64::EPSILON,
        "start should be 0.0 (from project specs), got {}",
        specs.start
    );
    assert!(
        (specs.stop - 10.0).abs() < f64::EPSILON,
        "stop should be 10.0 (from project specs), got {}",
        specs.stop
    );
}

#[test]
fn test_circular_dependency_blocks_incremental_compilation() {
    let project = datamodel::Project {
        name: "circular".to_string(),
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
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "a".to_string(),
                    equation: datamodel::Equation::Scalar("b".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "b".to_string(),
                    equation: datamodel::Equation::Scalar("a".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
        }],
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let dep_graph = model_dependency_graph(
        &db,
        sync.models["main"].source,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    assert!(dep_graph.has_cycle, "should detect circular dependency");

    let result = assemble_simulation(&db, sync.project, "main".to_string());
    assert!(
        result.is_err(),
        "incremental compilation should fail for circular dependencies"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("circular"),
        "error should mention circular dependencies, got: {}",
        err
    );
}

#[test]
fn test_malformed_graphical_function_fails_fragment() {
    // A variable with a graphical function where x_points and y_points
    // have different lengths should fail compile_var_fragment (returning
    // None) rather than silently dropping the table and producing bytecode
    // that references a missing lookup.
    let project = datamodel::Project {
        name: "test".to_string(),
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
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "lookup_var".to_string(),
                equation: datamodel::Equation::Scalar("time".to_string()),
                documentation: String::new(),
                units: None,
                gf: Some(datamodel::GraphicalFunction {
                    kind: datamodel::GraphicalFunctionKind::Continuous,
                    x_points: Some(vec![0.0, 1.0, 2.0]),
                    y_points: vec![10.0, 20.0], // mismatched length
                    x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
                    y_scale: datamodel::GraphicalFunctionScale {
                        min: 0.0,
                        max: 20.0,
                    },
                }),
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
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

    let model = sync.models["main"].source;
    let var = sync.models["main"].variables["lookup_var"].source;

    let result = compile_var_fragment(&db, var, model, sync.project, ModuleInputSet::empty(&db));
    assert!(
        result.is_none(),
        "compile_var_fragment should return None for malformed graphical function"
    );
}

#[test]
fn test_sparse_per_element_gfs_preserve_table_indices() {
    // When an arrayed variable has per-element graphical functions but some
    // elements lack a GF, the table vector must contain empty placeholders
    // to keep table[element_offset] aligned.  Without placeholders, later
    // elements would get the wrong lookup table.
    let project = datamodel::Project {
        name: "sparse_gf".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![datamodel::Dimension::named(
            "Dim".to_string(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "lookup_var".to_string(),
                equation: datamodel::Equation::Arrayed(
                    vec!["Dim".to_string()],
                    vec![
                        // Element A: has a GF
                        (
                            "A".to_string(),
                            "time".to_string(),
                            None,
                            Some(datamodel::GraphicalFunction {
                                kind: datamodel::GraphicalFunctionKind::Continuous,
                                x_points: Some(vec![0.0, 10.0]),
                                y_points: vec![100.0, 200.0],
                                x_scale: datamodel::GraphicalFunctionScale {
                                    min: 0.0,
                                    max: 10.0,
                                },
                                y_scale: datamodel::GraphicalFunctionScale {
                                    min: 0.0,
                                    max: 200.0,
                                },
                            }),
                        ),
                        // Element B: NO GF (placeholder needed)
                        ("B".to_string(), "time".to_string(), None, None),
                        // Element C: has a different GF
                        (
                            "C".to_string(),
                            "time".to_string(),
                            None,
                            Some(datamodel::GraphicalFunction {
                                kind: datamodel::GraphicalFunctionKind::Continuous,
                                x_points: Some(vec![0.0, 10.0]),
                                y_points: vec![500.0, 600.0],
                                x_scale: datamodel::GraphicalFunctionScale {
                                    min: 0.0,
                                    max: 10.0,
                                },
                                y_scale: datamodel::GraphicalFunctionScale {
                                    min: 0.0,
                                    max: 600.0,
                                },
                            }),
                        ),
                    ],
                    None,
                    false,
                ),
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
        }],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let var = sync.models["main"].variables["lookup_var"].source;

    // extract_tables_from_source_var must produce exactly 3 tables (one per
    // element), including an empty placeholder for element B. The dimension is
    // declared in sorted order (A, B, C), so the element-name -> dimension-index
    // mapping is the identity here: index i holds element i's table. This pins
    // the table CONTENT per slot (not just count/emptiness), so the
    // per-element-GF mapping fix must be byte-identical for sorted order.
    let tables = extract_tables_from_source_var(&db, &var, sync.project);
    assert_eq!(
        tables.len(),
        3,
        "should have 3 tables (including empty placeholder for element B), got {}",
        tables.len()
    );
    // Element A (index 0): x=[0,10], y=[100,200].
    assert_eq!(
        tables[0].data,
        vec![(0.0, 100.0), (10.0, 200.0)],
        "element A's table must be at index 0 with its own y=[100,200]"
    );
    // Element B (index 1): empty placeholder.
    assert!(
        tables[1].data.is_empty(),
        "element B should have an empty placeholder table at index 1"
    );
    // Element C (index 2): x=[0,10], y=[500,600].
    assert_eq!(
        tables[2].data,
        vec![(0.0, 500.0), (10.0, 600.0)],
        "element C's table must be at index 2 with its own y=[500,600]"
    );

    // Ensure the model still compiles successfully through the incremental path.
    let result = compile_project_incremental(&db, sync.project, "main");
    assert!(
        result.is_ok(),
        "model with sparse per-element GFs should compile: {:?}",
        result.err()
    );
}

#[test]
fn test_incremental_compile_smooth_over_module_output() {
    use crate::vm::Vm;

    let project = datamodel::Project {
        name: "smooth_module_output".to_string(),
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
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "producer".to_string(),
                        model_name: "producer".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "delay_time".to_string(),
                        equation: datamodel::Equation::Scalar("2".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "smoothed".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "SMTH1(producer.output, delay_time)".to_string(),
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
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
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
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let incremental = compile_project_incremental(&db, sync.project, "main")
        .expect("incremental compile should handle SMTH1(module.output, ...)");

    let mut incr_vm = Vm::new(incremental).expect("incremental VM should build");
    incr_vm
        .run_to_end()
        .expect("incremental simulation should run");
    let smoothed = crate::common::Ident::new("smoothed");
    let series = incr_vm
        .get_series(&smoothed)
        .expect("smoothed should exist in simulation output");
    assert!(!series.is_empty(), "smoothed series should not be empty");
}

#[test]
fn test_incremental_compile_distinguishes_module_input_sets() {
    use crate::vm::Vm;

    let project = datamodel::Project {
        name: "module_input_sets".to_string(),
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
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "shared_input".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "override_value".to_string(),
                        equation: datamodel::Equation::Scalar("99".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "without_override".to_string(),
                        model_name: "sub".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "shared_input".to_string(),
                            dst: "without_override.input".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "with_override".to_string(),
                        model_name: "sub".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![
                            datamodel::ModuleReference {
                                src: "shared_input".to_string(),
                                dst: "with_override.input".to_string(),
                            },
                            datamodel::ModuleReference {
                                src: "override_value".to_string(),
                                dst: "with_override.initial_value".to_string(),
                            },
                        ],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "out_without".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "without_override.output".to_string(),
                        ),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "out_with".to_string(),
                        equation: datamodel::Equation::Scalar("with_override.output".to_string()),
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
                name: "sub".to_string(),
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
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "initial_value".to_string(),
                        equation: datamodel::Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "output".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "if isModuleInput(initial_value) then initial_value else input"
                                .to_string(),
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

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("incremental compile should support per-instance module inputs");

    let sub_input_sets: std::collections::HashSet<Vec<String>> = compiled
        .modules
        .keys()
        .filter(|(model_name, _)| model_name.as_str() == "sub")
        .map(|(_, inputs)| inputs.iter().map(|i| i.as_str().to_string()).collect())
        .collect();
    assert_eq!(
        sub_input_sets.len(),
        2,
        "sub should have two module instances"
    );
    assert!(
        sub_input_sets.contains(&vec!["input".to_string()]),
        "missing sub instance wired with only input"
    );
    assert!(
        sub_input_sets.contains(&vec!["initial_value".to_string(), "input".to_string()]),
        "missing sub instance wired with input+initial_value"
    );

    let mut vm = Vm::new(compiled).expect("incremental VM should build");
    vm.run_to_end()
        .expect("incremental simulation should run to completion");

    let out_without = crate::common::Ident::new("out_without");
    let out_with = crate::common::Ident::new("out_with");
    let out_without_series = vm
        .get_series(&out_without)
        .expect("out_without should exist");
    let out_with_series = vm.get_series(&out_with).expect("out_with should exist");

    let without_value = *out_without_series
        .last()
        .expect("out_without series should be non-empty");
    let with_value = *out_with_series
        .last()
        .expect("out_with series should be non-empty");

    assert!(
        (without_value - 10.0).abs() < 1e-6,
        "without_override output should be 10, got {without_value}"
    );
    assert!(
        (with_value - 99.0).abs() < 1e-6,
        "with_override output should be 99, got {with_value}"
    );
}

fn implicit_lookup_smth1_project() -> datamodel::Project {
    datamodel::Project {
        name: "implicit_lookup_tables".to_string(),
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
                    ident: "table_var".to_string(),
                    equation: datamodel::Equation::Scalar("time".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: Some(datamodel::GraphicalFunction {
                        kind: datamodel::GraphicalFunctionKind::Continuous,
                        x_points: Some(vec![0.0, 10.0]),
                        y_points: vec![0.0, 100.0],
                        x_scale: datamodel::GraphicalFunctionScale {
                            min: 0.0,
                            max: 10.0,
                        },
                        y_scale: datamodel::GraphicalFunctionScale {
                            min: 0.0,
                            max: 100.0,
                        },
                    }),
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "delay_time".to_string(),
                    equation: datamodel::Equation::Scalar("2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "smoothed".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "SMTH1(LOOKUP(table_var, time), delay_time)".to_string(),
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
        }],
        source: None,
        ai_information: None,
    }
}

fn pre_lookup_smth1_project() -> datamodel::Project {
    datamodel::Project {
        name: "implicit_lookup_tables".to_string(),
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
                    ident: "table_var".to_string(),
                    equation: datamodel::Equation::Scalar("time".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "delay_time".to_string(),
                    equation: datamodel::Equation::Scalar("2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "smoothed".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "SMTH1(table_var, delay_time)".to_string(),
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
        }],
        source: None,
        ai_information: None,
    }
}

fn run_smoothed_series(compiled: crate::vm::CompiledSimulation) -> Vec<f64> {
    let mut vm = crate::vm::Vm::new(compiled).expect("VM should build");
    vm.run_to_end().expect("simulation should run");
    let smoothed = crate::common::Ident::new("smoothed");
    vm.get_series(&smoothed)
        .expect("smoothed should exist in simulation output")
        .to_vec()
}

#[test]
fn test_incremental_compile_implicit_lookup_dep_tables_after_equation_update() {
    let mut db = SimlinDb::default();

    let before_lookup = pre_lookup_smth1_project();
    let state1 = sync_from_datamodel_incremental(&mut db, &before_lookup, None);
    let before_lookup_compiled = compile_project_incremental(&db, state1.project, "main")
        .expect("baseline incremental compile should succeed");
    let before_lookup_series = run_smoothed_series(before_lookup_compiled);
    assert!(
        before_lookup_series.iter().all(|v| v.is_finite()),
        "baseline series should be finite before lookup rewrite"
    );

    // Fresh incremental compile as reference
    let project = implicit_lookup_smth1_project();
    let ref_db = SimlinDb::default();
    let ref_sync = sync_from_datamodel(&ref_db, &project);
    let ref_compiled = assemble_simulation(&ref_db, ref_sync.project, "main".to_string())
        .expect("reference incremental compile should succeed");
    let ref_series = run_smoothed_series((*ref_compiled).clone());

    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));
    let incr_compiled = compile_project_incremental(&db, state2.project, "main")
        .expect("incremental compile after equation rewrite should succeed");
    let incr_series = run_smoothed_series(incr_compiled);

    assert_eq!(
        incr_series.len(),
        ref_series.len(),
        "incremental and reference should have same number of timesteps"
    );

    for (step, (reference, incr)) in ref_series.iter().zip(incr_series.iter()).enumerate() {
        assert!(
            reference.is_finite(),
            "reference produced non-finite value at step {step}: {reference}"
        );
        assert!(
            incr.is_finite(),
            "incremental produced non-finite value at step {step}: {incr}"
        );
        assert!(
            (incr - reference).abs() < 1e-10,
            "incremental mismatch at step {step}: incr={incr}, reference={reference}"
        );
    }
}

#[test]
fn test_incremental_compile_implicit_lookup_dep_tables() {
    let project = implicit_lookup_smth1_project();

    // Fresh incremental compile as reference
    let ref_db = SimlinDb::default();
    let ref_sync = sync_from_datamodel(&ref_db, &project);
    let ref_compiled = assemble_simulation(&ref_db, ref_sync.project, "main".to_string())
        .expect("reference incremental compile should succeed");
    let ref_series = run_smoothed_series((*ref_compiled).clone());
    assert!(
        !ref_series.is_empty(),
        "reference smoothed series should not be empty"
    );

    let mut db = SimlinDb::default();

    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let incr_compiled1 = compile_project_incremental(&db, state1.project, "main")
        .expect("incremental compile should include lookup tables from implicit deps");
    let incr_series1 = run_smoothed_series(incr_compiled1);

    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));
    let incr_compiled2 = compile_project_incremental(&db, state2.project, "main")
        .expect("incremental compile after state reuse should succeed");
    let incr_series2 = run_smoothed_series(incr_compiled2);

    assert_eq!(
        incr_series1.len(),
        ref_series.len(),
        "incremental (fresh state) should have same number of timesteps as reference"
    );
    assert_eq!(
        incr_series2.len(),
        ref_series.len(),
        "incremental (reused state) should have same number of timesteps as reference"
    );

    for (step, (reference, incr)) in ref_series.iter().zip(incr_series1.iter()).enumerate() {
        assert!(
            reference.is_finite(),
            "reference produced non-finite value at step {step}: {reference}"
        );
        assert!(
            incr.is_finite(),
            "incremental produced non-finite value at step {step}: {incr}"
        );
        assert!(
            (incr - reference).abs() < 1e-10,
            "incremental mismatch at step {step}: incr={incr}, reference={reference}"
        );
    }

    for (step, (reference, incr)) in ref_series.iter().zip(incr_series2.iter()).enumerate() {
        assert!(
            incr.is_finite(),
            "incremental (reused state) produced non-finite value at step {step}: {incr}"
        );
        assert!(
            (incr - reference).abs() < 1e-10,
            "incremental (reused state) mismatch at step {step}: incr={incr}, reference={reference}"
        );
    }
}

#[test]
fn test_implicit_module_offsets_in_flattened_map() {
    // SMOOTH creates implicit MODULE variables whose sub-models contain
    // multiple slots.  calc_flattened_offsets_incremental must account for
    // the full sub-model size, not just 1 slot per implicit var.
    let project = datamodel::Project {
        name: "smooth_offsets".to_string(),
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
                    ident: "input".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "delay_time".to_string(),
                    equation: datamodel::Equation::Scalar("2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "smoothed".to_string(),
                    equation: datamodel::Equation::Scalar("SMTH3(input, delay_time)".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                // Add a variable after the SMOOTH to verify its offset isn't
                // shifted by undercounting the implicit module's size.
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "trailing".to_string(),
                    equation: datamodel::Equation::Scalar("42".to_string()),
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

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    // Compile through the incremental path.
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("SMOOTH model should compile incrementally");

    // The flattened offsets should match the layout: implicit MODULE vars
    // must occupy their sub-model's full slot count. `calc_flattened_offsets`
    // is computed as root (it reserves the implicit-global slots), so compare
    // against the root-shifted layout -- the SAME final layout
    // `assemble_module`'s root path resolves against (the lockstep guarantee).
    let layout = compute_layout(&db, sync.models["main"].source, sync.project).root_shifted();
    let offsets = calc_flattened_offsets_incremental(&db, sync.project, "main", true);

    // The total size from offsets should equal the layout's n_slots.
    let offsets_total: usize = if offsets.is_empty() {
        0
    } else {
        offsets
            .values()
            .map(|(off, size)| off + size)
            .max()
            .unwrap_or(0)
    };
    assert_eq!(
        offsets_total, layout.n_slots,
        "flattened offsets total ({offsets_total}) must match layout n_slots ({})",
        layout.n_slots
    );

    // Verify the simulation runs and produces correct results.
    let mut vm = crate::vm::Vm::new(compiled).expect("VM should build");
    vm.run_to_end().expect("simulation should run to end");

    // "trailing" should be 42 at every timestep.
    let trailing_ident: crate::common::Ident<crate::common::Canonical> =
        crate::common::Ident::new("trailing");
    let trailing_series = vm
        .get_series(&trailing_ident)
        .expect("trailing variable should be in results");
    for (t, &val) in trailing_series.iter().enumerate() {
        assert!(
            (val - 42.0).abs() < 1e-6,
            "trailing should be 42.0 at step {t}, got {val}"
        );
    }
}

/// When a user model shadows a stdlib model (same canonical name) and is
/// later removed, the stdlib definition must be rebuilt from scratch rather
/// than reusing the stale user override from `PersistentSyncState`.
#[test]
fn test_incremental_stdlib_restored_after_user_override_removed() {
    let mut db = SimlinDb::default();

    // Build a project with a user model that shadows stdlib delay1.
    let shadow_model = datamodel::Model {
        name: "stdlib\u{205A}delay1".to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "custom_var".to_string(),
            equation: datamodel::Equation::Scalar("999".to_string()),
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
    };

    let mut project = simple_project();
    project.models.push(shadow_model);

    // Sync twice so the override lands in PersistentSyncState.
    let state1 = sync_from_datamodel_incremental(&mut db, &project, None);
    let canonical = canonicalize("stdlib\u{205A}delay1").into_owned();
    let pm1 = &state1.models[&canonical];
    assert!(
        !pm1.is_stdlib,
        "user override should be marked is_stdlib=false"
    );
    assert!(
        pm1.variables.contains_key("custom_var"),
        "override should contain the user-defined variable"
    );

    let state2 = sync_from_datamodel_incremental(&mut db, &project, Some(&state1));
    assert!(!state2.models[&canonical].is_stdlib);

    // Now remove the shadowing model and sync again.
    project.models.retain(|m| m.name != "stdlib\u{205A}delay1");
    let state3 = sync_from_datamodel_incremental(&mut db, &project, Some(&state2));

    let pm3 = &state3.models[&canonical];
    assert!(
        pm3.is_stdlib,
        "restored entry should be marked is_stdlib=true"
    );
    assert!(
        !pm3.variables.contains_key("custom_var"),
        "user variable should not be present in restored stdlib model"
    );

    // The real stdlib delay1 has variables like "delay_time", "output", etc.
    let real_stdlib = crate::stdlib::get("delay1").unwrap();
    let expected_vars: std::collections::HashSet<String> = real_stdlib
        .variables
        .iter()
        .map(|v| canonicalize(v.get_ident()).into_owned())
        .collect();
    let actual_vars: std::collections::HashSet<String> = pm3.variables.keys().cloned().collect();
    assert_eq!(
        expected_vars, actual_vars,
        "restored stdlib model should have exactly the real stdlib variables"
    );
}

/// After a fresh sync (prev_state=None), stdlib models in the resulting
/// PersistentSyncState should be marked is_stdlib=true so that subsequent
/// incremental syncs can reuse their salsa inputs without rebuilding.
#[test]
fn test_initial_sync_marks_stdlib_models() {
    let mut db = SimlinDb::default();
    let project = simple_project();

    let state = sync_from_datamodel_incremental(&mut db, &project, None);

    for stdlib_name in crate::stdlib::MODEL_NAMES {
        let canonical = canonicalize(&format!("stdlib\u{205A}{stdlib_name}")).into_owned();
        let pm = state
            .models
            .get(&canonical)
            .unwrap_or_else(|| panic!("stdlib model {stdlib_name} missing from sync state"));
        assert!(
            pm.is_stdlib,
            "stdlib model {stdlib_name} should have is_stdlib=true after initial sync"
        );
    }
}
