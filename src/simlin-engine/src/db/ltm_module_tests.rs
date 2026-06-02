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

    let n_slots_ltm = compute_layout(&db, source_model, source_project).n_slots;
    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_no_ltm = compute_layout(&db, source_model, source_project).n_slots;
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

    let n_slots_ltm = compute_layout(&db, source_model, source_project).n_slots;
    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_no_ltm = compute_layout(&db, source_model, source_project).n_slots;
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

/// Issue #417: modules with stocks whose output variable is not named
/// "output" should still get composite/pathway LTM variables. This test
/// uses a user-defined module with an internal stock and output named
/// "result" instead of the stdlib convention "output".
#[test]
fn test_ltm_module_with_non_standard_output_name() {
    use salsa::Setter;

    let project = datamodel::Project {
        name: "custom_output_name".to_string(),
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
                    x_aux("goal", "100", None),
                    x_stock("level", "50", &["adjustment"], &[], None),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "smoothed".to_string(),
                        equation: datamodel::Equation::Scalar("custom_smooth.result".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    x_aux("gap", "goal - smoothed", None),
                    x_flow("adjustment", "gap / 5", None),
                    x_module(
                        "custom_smooth",
                        &[("level", "input"), ("3", "delay_time")],
                        None,
                    ),
                ],
            ),
            // Custom smooth module with output named "result" instead of "output"
            x_model(
                "custom_smooth",
                vec![
                    x_aux("input", "0", None),
                    x_aux("delay_time", "1", None),
                    datamodel::Variable::Flow(datamodel::Flow {
                        ident: "flow".to_string(),
                        equation: datamodel::Equation::Scalar(
                            "(input - result) / delay_time".to_string(),
                        ),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    x_stock("result", "0", &["flow"], &[], None),
                ],
            ),
        ],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, sub_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["custom_smooth"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    // The sub-model should generate pathway/composite variables
    // despite its output stock being named "result" instead of "output".
    let ltm_vars = model_ltm_variables(&db, sub_model, source_project);
    let has_composite = ltm_vars.vars.iter().any(|v| v.name.contains("composite"));
    assert!(
        has_composite,
        "sub-model should have composite score variable (output named 'result'). vars: {:?}",
        ltm_vars.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

/// Issue #418: loops through SMOOTH modules should have determined polarity
/// (Balancing), not Undetermined. This verifies that module_graphs is
/// properly populated from sub-model causal edges.
#[test]
fn test_module_loop_polarity_is_determined() {
    let project = datamodel::Project {
        name: "smooth_polarity".to_string(),
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

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let detected = model_detected_loops(&db, source_model, sync.project);

    assert!(
        !detected.loops.is_empty(),
        "Should detect loops through SMOOTH"
    );

    // Every loop should have a determined polarity
    for loop_item in &detected.loops {
        assert_ne!(
            loop_item.polarity,
            super::DetectedLoopPolarity::Undetermined,
            "Loop {} ({}) should have determined polarity, not Undetermined",
            loop_item.id,
            loop_item.variables.join(" -> ")
        );
    }
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

    let n_slots_ltm = compute_layout(&db, source_model, source_project).n_slots;

    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_no_ltm = compute_layout(&db, source_model, source_project).n_slots;

    assert!(
        n_slots_ltm > n_slots_no_ltm,
        "LTM-enabled layout should have more slots: ltm={n_slots_ltm}, no_ltm={n_slots_no_ltm}"
    );
}

/// A module input port with many internal pathways to the output must produce
/// composite-selection equations whose TOTAL text is linear in the pathway
/// count.
///
/// Regression guard for the exponential composite bug: the composite
/// "pathway with the largest absolute score" equation was built by recursively
/// nesting `if ABS(last) >= ABS((rest)) then last else (rest)` -- `rest`
/// appears TWICE per level, so the equation text doubled per pathway
/// (O(2^n) bytes). 20 parallel pathways produced a ~16MB equation; real Vensim
/// macro modules with hundreds of pathways (covid19's SSTATS) exhausted all
/// memory. The linear form folds the selection through O(1)-sized accumulator
/// helper variables instead.
#[test]
fn test_module_composite_equation_size_is_linear_in_pathways() {
    use salsa::Setter;

    // A module body with PATHS parallel pathways:
    // input -> mid_i -> total_flow -> output. The output is a STOCK because
    // LTM generation skips stockless models entirely.
    const PATHS: usize = 20;
    let mut module_vars = vec![x_aux("input", "0", None)];
    let mut total_flow_eq = String::new();
    for i in 0..PATHS {
        module_vars.push(x_aux(&format!("mid_{i}"), &format!("input * {i}"), None));
        if i > 0 {
            total_flow_eq.push_str(" + ");
        }
        total_flow_eq.push_str(&format!("mid_{i}"));
    }
    module_vars.push(x_flow("total_flow", &total_flow_eq, None));
    module_vars.push(x_stock("output", "0", &["total_flow"], &[], None));

    let project = datamodel::Project {
        name: "many_pathways".to_string(),
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
                    x_aux("driver", "1", None),
                    x_aux("reader", "m.output", None),
                    x_module("m", &[("driver", "input")], None),
                ],
            ),
            x_model("m", module_vars),
        ],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, sub_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["m"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, sub_model, source_project);

    // The composite selection must exist for the input port...
    let composite_count = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}composite\u{205A}"))
        .count();
    assert!(
        composite_count >= 1,
        "module with input pathways should emit a composite var; vars: {:?}",
        ltm_vars.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );

    // ...and the TOTAL equation text across every LTM synthetic variable must
    // be linear in the pathway count: comfortably under 100KB for 20 pathways.
    // (The exponential nested form produced ~16MB here.)
    let total_equation_bytes: usize = ltm_vars
        .vars
        .iter()
        .map(|v| v.equation.source_text().len())
        .sum();
    assert!(
        total_equation_bytes < 100_000,
        "total LTM equation text should be linear in pathway count; \
         got {total_equation_bytes} bytes across {} vars",
        ltm_vars.vars.len()
    );

    // Every selection step must reference variables that were ALREADY emitted
    // (sort earlier in evaluation order): the runlist evaluates LTM fragments
    // in `vars` order, so an accumulator referencing a later-sorted variable
    // would read an unevaluated value.
    let positions: std::collections::HashMap<&str, usize> = ltm_vars
        .vars
        .iter()
        .enumerate()
        .map(|(i, v)| (v.name.as_str(), i))
        .collect();
    for (i, v) in ltm_vars.vars.iter().enumerate() {
        if !v.name.contains("\u{205A}path\u{205A}") && !v.name.contains("\u{205A}composite\u{205A}")
        {
            continue;
        }
        let text = v.equation.source_text();
        // Extract quoted identifiers and check each referenced LTM var that
        // exists in this set sorts before the referencing var.
        for referenced in text.split('"').skip(1).step_by(2) {
            if let Some(&ref_pos) = positions.get(referenced) {
                assert!(
                    ref_pos < i,
                    "{} (position {i}) references {referenced} (position {ref_pos}), \
                     which would be evaluated AFTER it",
                    v.name
                );
            }
        }
    }
}

/// Build a sub-model whose body has `paths` *distinct* internal pathways from
/// input port `input` to output port `output` (a stock so LTM does not skip
/// the stockless model): `input -> mid_i -> total_flow -> output`, one branch
/// per `i`. The main model drives the module and reads its output. Returned as
/// `(project, "m" sub-model name)`. Used by the GH #649 pathway-budget tests.
fn parallel_pathways_module_project(paths: usize) -> datamodel::Project {
    let mut module_vars = vec![x_aux("input", "1", None)];
    let mut total_flow_eq = String::new();
    for i in 0..paths {
        module_vars.push(x_aux(
            &format!("mid_{i}"),
            &format!("input * {}", i + 1),
            None,
        ));
        if i > 0 {
            total_flow_eq.push_str(" + ");
        }
        total_flow_eq.push_str(&format!("mid_{i}"));
    }
    module_vars.push(x_flow("total_flow", &total_flow_eq, None));
    module_vars.push(x_stock("output", "0", &["total_flow"], &[], None));

    datamodel::Project {
        name: "many_pathways".to_string(),
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
            x_model(
                "main",
                vec![
                    x_aux("driver", "1", None),
                    x_aux("reader", "m.output", None),
                    x_module("m", &[("driver", "input")], None),
                ],
            ),
            x_model("m", module_vars),
        ],
        source: None,
        ai_information: None,
    }
}

/// GH #649: a module body with more internal input->output pathways than the
/// per-port pathway budget has its pathway enumeration truncated
/// deterministically: the kept pathway count equals the budget,
/// `LtmVariablesResult.pathways_truncated` is set, and a `CompilationDiagnostic`
/// `Warning` names the module, the budget, and the clipped input port. The
/// fixture is tiny (12 parallel pathways) and the budget is shrunk to 4 via the
/// test-only `ModulePathwayBudgetGuard` so the budget is what clips (never trip
/// the real 8192 gate with a giant fixture; docs/dev/rust.md#test-time-budgets).
#[test]
fn module_pathway_enumeration_truncates_at_budget() {
    use crate::db::{CompilationDiagnostic, DiagnosticError, DiagnosticSeverity};
    use salsa::Setter;

    const PATHS: usize = 12;
    const TEST_BUDGET: usize = 4;

    let project = parallel_pathways_module_project(PATHS);
    let mut db = SimlinDb::default();
    let (source_project, sub_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["m"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    // Hold the override for the whole test: `model_ltm_variables` is salsa-
    // memoized, so a later call on this db would otherwise return the cached
    // tiny-budget result regardless of the override state.
    let _guard = crate::ltm::ModulePathwayBudgetGuard::new(TEST_BUDGET);
    let ltm = model_ltm_variables(&db, sub_model, source_project);

    assert!(
        ltm.pathways_truncated,
        "with {PATHS} pathways and a budget of {TEST_BUDGET}, pathway enumeration \
         must report truncation"
    );

    // Exactly the budget number of pathway vars are minted for the `input` port.
    let path_var_count = ltm
        .vars
        .iter()
        .filter(|v| {
            v.name.contains("\u{205A}path\u{205A}") && !v.name.contains("\u{205A}acc\u{205A}")
        })
        .count();
    assert_eq!(
        path_var_count, TEST_BUDGET,
        "the kept pathway count must equal the budget; got {path_var_count}"
    );

    // The composite over the kept prefix still exists (no panic, no skip).
    let composite_count = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}composite\u{205A}"))
        .count();
    assert!(
        composite_count >= 1,
        "a truncated module must still emit a composite var over the kept prefix"
    );

    let diags =
        model_ltm_variables::accumulated::<CompilationDiagnostic>(&db, sub_model, source_project);
    let has_warning = diags.iter().any(|CompilationDiagnostic(d)| {
        d.severity == DiagnosticSeverity::Warning
            && matches!(
                &d.error,
                DiagnosticError::Assembly(msg)
                    if msg.contains("truncated")
                        && msg.contains(&TEST_BUDGET.to_string())
                        && msg.contains("input")
            )
    });
    assert!(
        has_warning,
        "pathway truncation must emit a Warning mentioning truncation, the budget \
         ({TEST_BUDGET}), and the clipped input port; got: {:?}",
        diags.iter().map(|c| &c.0).collect::<Vec<_>>()
    );
}

/// GH #649: a module whose internal pathway count is *under* the budget emits
/// NO truncation flag and NO warning, and mints exactly one pathway var per
/// pathway (the under-budget byte-identical-to-before guarantee).
#[test]
fn module_pathway_enumeration_under_budget_no_truncation() {
    use crate::db::CompilationDiagnostic;
    use salsa::Setter;

    const PATHS: usize = 4;
    const TEST_BUDGET: usize = 64;

    let project = parallel_pathways_module_project(PATHS);
    let mut db = SimlinDb::default();
    let (source_project, sub_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["m"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let _guard = crate::ltm::ModulePathwayBudgetGuard::new(TEST_BUDGET);
    let ltm = model_ltm_variables(&db, sub_model, source_project);

    assert!(
        !ltm.pathways_truncated,
        "an under-budget module must NOT report pathway truncation"
    );
    let path_var_count = ltm
        .vars
        .iter()
        .filter(|v| {
            v.name.contains("\u{205A}path\u{205A}") && !v.name.contains("\u{205A}acc\u{205A}")
        })
        .count();
    assert_eq!(
        path_var_count, PATHS,
        "every pathway must be enumerated when under budget"
    );
    let diags =
        model_ltm_variables::accumulated::<CompilationDiagnostic>(&db, sub_model, source_project);
    let has_truncation_warning = diags.iter().any(|CompilationDiagnostic(d)| {
        matches!(&d.error, crate::db::DiagnosticError::Assembly(msg) if msg.contains("module-pathway"))
    });
    assert!(
        !has_truncation_warning,
        "an under-budget module must emit no pathway-truncation warning"
    );
}

/// GH #649: a truncated module still compiles end to end and simulates -- the
/// composite link score over the kept pathway prefix is finite (degraded, not a
/// panic or a silent NaN). This is the "no fragment-compile failure" guarantee.
#[test]
fn module_pathway_truncation_still_compiles_and_simulates() {
    use salsa::Setter;

    const PATHS: usize = 12;
    const TEST_BUDGET: usize = 4;

    let project = parallel_pathways_module_project(PATHS);
    let mut db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    source_project.set_ltm_enabled(&mut db).to(true);

    let _guard = crate::ltm::ModulePathwayBudgetGuard::new(TEST_BUDGET);
    let compiled = compile_project_incremental(&db, source_project, "main");
    assert!(
        compiled.is_ok(),
        "a pathway-truncated module must still compile: {:?}",
        compiled.err()
    );
    let mut vm = crate::vm::Vm::new(compiled.unwrap()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("a pathway-truncated module must simulate to completion");
}

/// The results offsets map (`calc_flattened_offsets_incremental`, what
/// `CompiledSimulation.offsets` / `Results.offsets` is built from) and the
/// compiled layout (`compute_layout`, what `resolve_module` assigns bytecode
/// slot offsets from) MUST agree on every variable's slot. If they diverge,
/// every results column after the divergence point reads some other
/// variable's data -- silently.
///
/// This is exercised with LTM enabled on a model containing a SMOOTH module:
/// the module variable's size is computed by both functions independently
/// (`compute_layout` uses the sub-model's `n_slots`; the offsets map sums its
/// own recursive entries), and with LTM enabled the sub-model's layout
/// includes LTM synthetic variables, which is where the two historically
/// diverged.
#[test]
fn test_results_offsets_agree_with_layout_under_ltm() {
    use crate::db::calc_flattened_offsets_incremental;
    use salsa::Setter;

    let project = datamodel::Project {
        name: "offsets_vs_layout".to_string(),
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
                // Variables that sort alphabetically AFTER "smoothed_level":
                // any module-size divergence shifts these.
                x_aux("z_downstream_a", "gap * 2", None),
                x_aux("z_downstream_b", "level + 1", None),
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

    let offsets = calc_flattened_offsets_incremental(&db, source_project, "main", true);
    let layout = compute_layout(&db, source_model, source_project).root_shifted();

    let mut mismatches: Vec<String> = Vec::new();
    for (name, (off, _size)) in offsets.iter() {
        // Only names that exist verbatim in the layout are directly
        // comparable (per-element `x[a1]` and module-flattened `mod·sub`
        // names are offsets-map-only expansions).
        if let Some(entry) = layout.get(name.as_str())
            && entry.offset != *off
        {
            mismatches.push(format!(
                "{name}: offsets-map says {off}, layout says {}",
                entry.offset
            ));
        }
    }
    mismatches.sort();
    assert!(
        mismatches.is_empty(),
        "results offsets map and compiled layout disagree on {} slots:\n  {}",
        mismatches.len(),
        mismatches.join("\n  ")
    );
}

/// C-LEARN-scale version of the offsets-vs-layout consistency check.
/// Ignored by default (loads a 1.4 MB model); run explicitly with
/// `cargo test -- --ignored test_clearn_results_offsets_agree_with_layout`.
#[test]
#[ignore]
fn test_clearn_results_offsets_agree_with_layout() {
    use crate::db::calc_flattened_offsets_incremental;
    use salsa::Setter;

    let path = format!(
        "{}/../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
        env!("CARGO_MANIFEST_DIR")
    );
    let contents = std::fs::read_to_string(&path).expect("read C-LEARN mdl");
    let project = crate::open_vensim(&contents).expect("parse C-LEARN");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);
    let source_model = source_project
        .models(&db)
        .get(crate::canonicalize("main").as_ref())
        .copied()
        .expect("main model");

    let offsets = calc_flattened_offsets_incremental(&db, source_project, "main", true);
    let layout = compute_layout(&db, source_model, source_project).root_shifted();

    let mut mismatches: Vec<(usize, String)> = Vec::new();
    let mut compared = 0usize;
    for (name, (off, _size)) in offsets.iter() {
        if let Some(entry) = layout.get(name.as_str()) {
            compared += 1;
            if entry.offset != *off {
                mismatches.push((
                    *off.min(&entry.offset),
                    format!(
                        "{}: offsets-map={off} layout={} (delta {})",
                        name.as_str(),
                        entry.offset,
                        *off as i64 - entry.offset as i64
                    ),
                ));
            }
        }
    }
    mismatches.sort();
    eprintln!(
        "compared {compared} names; {} mismatches; earliest 15:",
        mismatches.len()
    );
    for (_, msg) in mismatches.iter().take(15) {
        eprintln!("  {msg}");
    }
    assert!(
        mismatches.is_empty(),
        "results offsets map and compiled layout disagree on {} of {compared} comparable slots",
        mismatches.len(),
    );
}
