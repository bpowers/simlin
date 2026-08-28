// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for LTM compilation with models containing modules
//! (stdlib SMOOTH/DELAY and user-defined passthrough modules).

use super::*;
use crate::capture::ImplicitVar;
use crate::datamodel;
use crate::test_common::TestProject;
use crate::testutils::{x_aux, x_flow, x_model, x_module, x_module_named, x_stock};

/// Sparse DELAYN/SMTHN initialization must be indistinguishable from calling
/// the selected stdlib module directly with its optional port omitted. This
/// compares the complete emitted LTM name/equation topology and every runtime
/// slot, in addition to proving no ambiguous-input diagnostic was needed.
#[test]
fn ltm_sparse_n_order_initials_match_direct_stdlib_topology_and_series() {
    use salsa::Setter;

    struct CompiledCase {
        references: Vec<String>,
        hoisted: Vec<String>,
        input_schedule: (bool, bool),
        ltm_equations: Vec<(String, String, Vec<String>, bool)>,
        loop_partitions: indexmap::IndexMap<String, Vec<Option<usize>>>,
        offsets: std::collections::HashMap<Ident<Canonical>, usize>,
        data: Box<[f64]>,
    }

    fn compile_case(equation: &str) -> CompiledCase {
        let project = TestProject::new("ltm_sparse_n_order")
            .with_sim_time(0.0, 4.0, 1.0)
            .aux("goal", "100", None)
            .stock("level", "50", &["adjustment"], &[], None)
            .aux("delayed", equation, None)
            .aux("gap", "goal - delayed", None)
            .flow("adjustment", "gap / 5", None)
            .build_datamodel();
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        sync.project.set_ltm_enabled(&mut db).to(true);
        let delayed = sync.models["main"].variables["delayed"].source;
        let parsed = parse_source_variable(&db, delayed, sync.project);
        let hoisted: Vec<_> = parsed
            .implicit_vars
            .iter()
            .filter_map(|helper| match helper {
                ImplicitVar::HoistedArg(arg) => Some(arg.ident().to_string()),
                ImplicitVar::Capture(_) | ImplicitVar::Module(_) => None,
            })
            .collect();
        let module = parsed
            .implicit_vars
            .iter()
            .find_map(ImplicitVar::module)
            .expect("stdlib module");
        let references = module
            .references()
            .iter()
            .map(|reference| {
                format!(
                    "{}->{}",
                    reference.src,
                    reference.dst.rsplit('.').next().unwrap()
                )
            })
            .collect();

        let graph = model_dependency_graph(
            &db,
            sync.models["main"].source,
            sync.project,
            ModuleInputSet::empty(&db),
        );
        let input_schedule = (
            graph
                .runlist_initials
                .iter()
                .any(|name| name == "$⁚delayed⁚0⁚arg0"),
            graph
                .runlist_flows
                .iter()
                .any(|name| name == "$⁚delayed⁚0⁚arg0"),
        );

        let ltm = model_ltm_variables(&db, sync.models["main"].source, sync.project);
        let ltm_equations = ltm
            .vars
            .iter()
            .map(|var| {
                (
                    var.name.clone(),
                    var.equation.source_text(),
                    var.dimensions.clone(),
                    var.compile_directly,
                )
            })
            .collect();
        let diagnostics = collect_all_diagnostics(&db, sync.project);
        assert!(
            diagnostics.is_empty(),
            "sparse stdlib inputs must not take the ambiguous-port decline: {diagnostics:?}"
        );

        let compiled = compile_project_incremental(&db, sync.project, "main")
            .expect("LTM sparse-port fixture compiles");
        let mut vm = crate::vm::Vm::new(compiled).expect("vm");
        vm.run_to_end().expect("LTM sparse-port fixture runs");
        let results = vm.into_results();
        CompiledCase {
            references,
            hoisted,
            input_schedule,
            ltm_equations,
            loop_partitions: ltm.loop_partitions.clone(),
            offsets: results.offsets,
            data: results.data,
        }
    }

    for (n_order, direct) in [
        ("DELAYN(level * 1, 3, 1)", "DELAY1(level * 1, 3)"),
        ("DELAYN(level * 1, 3, 3)", "DELAY3(level * 1, 3)"),
        ("SMTHN(level * 1, 3, 1)", "SMTH1(level * 1, 3)"),
        ("SMTHN(level * 1, 3, 3)", "SMTH3(level * 1, 3)"),
    ] {
        let actual = compile_case(n_order);
        let control = compile_case(direct);
        assert_eq!(
            actual.references, control.references,
            "{n_order}: sparse port topology"
        );
        assert_eq!(actual.references.len(), 2, "input and delay_time only");
        assert!(
            actual
                .references
                .iter()
                .all(|reference| !reference.ends_with("->initial_value"))
        );
        assert_eq!(actual.hoisted, control.hoisted, "{n_order}: one input body");
        assert_eq!(
            actual.input_schedule,
            (true, true),
            "{n_order}: module fallback reads input in both phases"
        );
        assert_eq!(actual.input_schedule, control.input_schedule);
        assert_eq!(
            actual.ltm_equations, control.ltm_equations,
            "{n_order}: exact LTM names, equations, dimensions and compile routing"
        );
        assert!(
            actual
                .ltm_equations
                .iter()
                .any(|(name, _, _, _)| name.contains("link_score")),
            "{n_order}: control must exercise link-score topology"
        );
        assert_eq!(
            actual.loop_partitions, control.loop_partitions,
            "{n_order}: loop topology"
        );
        assert_eq!(
            actual.offsets, control.offsets,
            "{n_order}: result topology"
        );
        assert_eq!(actual.data.len(), control.data.len());
        for (index, (actual, control)) in actual.data.iter().zip(&control.data).enumerate() {
            assert!(
                actual == control || (actual.is_nan() && control.is_nan()),
                "{n_order}: runtime slot {index} differs: {actual} versus {control}"
            );
        }
    }
}

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

    // LTM link equations consume the module instance synthesized while the
    // source equation was parsed. Drive the generated equation back through
    // the production LTM parser, structured-target resolver and fragment-input
    // constructor: no LTM-owned module registry is needed or reachable here.
    let source_module_name = model_implicit_var_info(&db, source_model, source_project)
        .iter()
        .find_map(|(name, meta)| meta.is_module.then_some(name.as_str()))
        .expect("SMTH1 source parse should synthesize a module instance");
    let qualified_output = format!("{source_module_name}·output");
    let consumer = ltm_vars
        .vars
        .iter()
        .find(|var| var.equation.source_text().contains(&qualified_output))
        .expect("an LTM equation should retain the source module-output read");
    let input = ltm_fragment_input(
        &db,
        &consumer.name,
        &consumer.equation,
        source_model,
        source_project,
    )
    .expect("generated LTM equation should lower");
    assert!(
        matches!(
            input.deps[source_module_name].kind,
            crate::compiler::fragment::DepKind::Module { .. }
        ),
        "the source implicit module must resolve structurally, not as a flat scalar"
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
/// with LTM enabled without errors. Since PR #684 the passthrough sub-model
/// itself emits pathway/composite LTM vars (its `scaled_output = input_val * 2`
/// chain is a real input->output pathway); this test only asserts the MAIN
/// model gets LTM offsets for its feedback loop through the module.
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

/// Fixture shared by the non-standard-output-port tests: a main model
/// driving a user-defined `custom_smooth` module whose output stock is
/// named `result` (not the stdlib `output` convention), read back via
/// `custom_smooth.result`.
fn custom_output_port_project() -> datamodel::Project {
    datamodel::Project {
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
                    // dst uses the XMILE `<connect to="module.var">` convention
                    // (prefixed with the module instance name); a bare dst is
                    // dropped by `build_module_inputs` / `resolve_module_input`.
                    x_module(
                        "custom_smooth",
                        &[
                            ("level", "custom_smooth.input"),
                            ("3", "custom_smooth.delay_time"),
                        ],
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
    }
}

/// Issue #417: modules with stocks whose output variable is not named
/// "output" should still get composite/pathway LTM variables. This test
/// uses a user-defined module with an internal stock and output named
/// "result" instead of the stdlib convention "output".
#[test]
fn test_ltm_module_with_non_standard_output_name() {
    use salsa::Setter;

    let project = custom_output_port_project();

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

/// GH #680: a module with several output ports must return its port set in a
/// deterministic (canonical-name-sorted) order. `find_model_output_ports`
/// historically drained an unordered `HashSet`, so the pathway enumeration,
/// magnitude-tie winner selection, and `$⁚ltm⁚path⁚{port}⁚{idx}` indices all
/// inherited process-random ordering -- byte-unstable salsa cache values for
/// multi-output modules with tied pathway scores. The fix sorts before
/// returning; this test pins that contract through the `sub_model_output_ports`
/// wrapper (which delegates straight to `find_model_output_ports` for a
/// non-stdlib model).
///
/// The fixture uses **six** output ports (out_a through out_f) so that a
/// randomly-ordered HashSet drain has only a 1/720 chance of producing the
/// sorted order by accident (vs. 1/6 for three ports). Ports are declared in
/// the sub-model in the order (a, c, e, b, d, f) and read by `main` in the
/// order (f, d, b, e, c, a) -- neither declaration order nor read order matches
/// the expected canonical sort, so the sort must actively reorder them.
#[test]
fn test_multi_output_port_module_ports_are_sorted() {
    let project = datamodel::Project {
        name: "multi_output_ports".to_string(),
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
                    x_stock("level", "50", &["adjustment"], &[], None),
                    // Read ports in (f, d, b, e, c, a) order -- neither
                    // declaration order nor alphabetical -- so a non-deterministic
                    // (unsorted) drain cannot pass by accident.
                    x_aux("read_f", "multi_out.out_f", None),
                    x_aux("read_d", "multi_out.out_d", None),
                    x_aux("read_b", "multi_out.out_b", None),
                    x_aux("read_e", "multi_out.out_e", None),
                    x_aux("read_c", "multi_out.out_c", None),
                    x_aux("read_a", "multi_out.out_a", None),
                    x_aux(
                        "gap",
                        "100 - read_a - read_b - read_c - read_d - read_e - read_f + level",
                        None,
                    ),
                    x_flow("adjustment", "gap / 5", None),
                    x_module("multi_out", &[("level", "multi_out.input")], None),
                ],
            ),
            x_model(
                "multi_out",
                vec![
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
                    // Six output ports declared in (a, c, e, b, d, f) order --
                    // different from both read order and the expected sorted result
                    // -- so neither declaration nor read order can accidentally
                    // produce the sorted output.
                    x_aux("out_a", "input * 2", None),
                    x_aux("out_c", "input * 4", None),
                    x_aux("out_e", "input * 6", None),
                    x_aux("out_b", "input * 3", None),
                    x_aux("out_d", "input * 5", None),
                    x_aux("out_f", "input * 7", None),
                ],
            ),
        ],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let (source_project, sub_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["multi_out"].source)
    };

    // `find_model_output_ports` reads structural deps (no `ltm_enabled` needed):
    // it scans parent models for `multi_out·{port}` references.
    let ports = sub_model_output_ports(&db, sub_model, source_project);
    let port_names: Vec<&str> = ports.iter().map(|p| p.as_str()).collect();
    assert_eq!(
        port_names,
        vec!["out_a", "out_b", "out_c", "out_d", "out_e", "out_f"],
        "find_model_output_ports must return its multi-output-port set in \
         canonical-name-sorted order regardless of read/declaration/hash order; \
         with 6 ports a random drain hits sorted order with probability 1/720"
    );
}

/// Output-port discovery is a dt-phase projection of production dependency
/// facts, not a scan of every syntactic module-output name. Direct and
/// computed INIT-only reads do not create pathways. A current read of hidden
/// INIT capture storage makes that storage flow-live and restores its source
/// port, while PREVIOUS storage remains flow-live by definition.
#[test]
fn model_output_ports_follow_explicit_and_implicit_flow_liveness() {
    let project = datamodel::Project {
        name: "phase_aware_output_ports".to_string(),
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
        models: vec![
            x_model(
                "main",
                vec![
                    x_module_named("m", "child", &[], None),
                    x_aux("direct_init", "INIT(m.init_direct)", None),
                    x_aux("init_only", "INIT(m.init_capture * 2)", None),
                    x_aux("promoted", "INIT(m.promoted * 3)", None),
                    x_aux("current_reader", "\"$⁚promoted⁚0⁚arg0\"", None),
                    x_aux("lagged", "PREVIOUS(m.previous * 4, 0)", None),
                    x_aux("current", "m.current", None),
                    x_aux("smoothed", "SMTH1(m.smooth_input, 1, 0)", None),
                ],
            ),
            x_model(
                "child",
                vec![
                    x_aux("init_direct", "1", None),
                    x_aux("init_capture", "2", None),
                    x_aux("promoted", "3", None),
                    x_aux("previous", "4", None),
                    x_aux("current", "5", None),
                    x_aux("smooth_input", "6", None),
                ],
            ),
        ],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let ports = sub_model_output_ports(&db, sync.models["child"].source, sync.project);
    let names: Vec<&str> = ports.iter().map(Ident::as_str).collect();
    assert_eq!(
        names,
        ["current", "previous", "promoted", "smooth_input"],
        "only current and flow-live snapshot storage reads are dt output ports"
    );
}

/// The within-parent module-port index consumed by module link-score fallback
/// follows the same phase boundary as cross-parent output-port discovery.
/// INIT-only reads must not choose an output for a dt score, while promoted
/// INIT storage, PREVIOUS storage, current reads, and implicit-module inputs do.
#[test]
fn local_module_output_ports_follow_flow_liveness() {
    let project = datamodel::Project {
        name: "local_phase_aware_output_ports".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            x_model(
                "main",
                vec![
                    x_module_named("m", "child", &[], None),
                    x_aux("direct_init", "INIT(m.init_direct)", None),
                    x_aux("init_only", "INIT(m.init_capture * 2)", None),
                    x_aux("promoted", "INIT(m.promoted * 3)", None),
                    x_aux("current_reader", "\"$⁚promoted⁚0⁚arg0\"", None),
                    x_aux("lagged", "PREVIOUS(m.previous * 4, 0)", None),
                    x_aux("current", "m.current", None),
                    x_aux("smoothed", "SMTH1(m.smooth_input, 1, 0)", None),
                ],
            ),
            x_model(
                "child",
                vec![
                    x_aux("init_direct", "1", None),
                    x_aux("init_capture", "2", None),
                    x_aux("promoted", "3", None),
                    x_aux("previous", "4", None),
                    x_aux("current", "5", None),
                    x_aux("smooth_input", "6", None),
                ],
            ),
        ],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let ports = model_module_output_ports_by_name(
        &db,
        sync.models["main"].source,
        sync.project,
        "m".to_string(),
    );
    assert_eq!(
        ports.as_slice(),
        ["current", "previous", "promoted", "smooth_input"],
        "only current and flow-live snapshot storage reads are local dt output ports"
    );
}

/// Discovery mode now uses the same composite link-score reference for a
/// DynamicModule input port that exhaustive mode does (GH #675): since
/// GH #548 the sub-model's `$⁚ltm⁚composite⁚{port}` var is laid out in the
/// parent's flattened offset map in BOTH modes (an empirical probe showed
/// it resolving to a nonzero value in a discovery run), so the pre-#675
/// discovery-only black-box delta-ratio against the module output was both
/// a less faithful score (the gain, not a link score) and an unnecessary
/// divergence. `custom_smooth` has an internal stock and an input->output
/// pathway, so it emits a composite for the `input` port.
#[test]
fn test_discovery_dynamic_module_link_score_uses_composite() {
    use salsa::Setter;

    let project = custom_output_port_project();

    let mut db = SimlinDb::default();
    let (source_project, main_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, main_model, source_project);
    let link = ltm_vars
        .vars
        .iter()
        .find(|v| v.name.contains("level\u{2192}custom_smooth"))
        .expect("discovery mode must emit the level→custom_smooth link score");
    let eqn_text = match &link.equation {
        crate::db::LtmEquation::Scalar(arm) => arm.text.clone(),
        other => panic!("module link score should be scalar, got {other:?}"),
    };
    assert!(
        eqn_text.contains("custom_smooth\u{00B7}$\u{205A}ltm\u{205A}composite\u{205A}input"),
        "discovery-mode link score into a DynamicModule must reference the \
         sub-model's composite for the input port; got: {eqn_text}"
    );
}

/// A *passthrough* (stockless) module whose output DOES depend on its input
/// (`result = input * 2`) now exposes a composite (PR #684, Part C: a
/// passthrough's internals are a pure aux chain LTM scores exactly). The base
/// `input → module` link score therefore references that composite
/// (`custom_pt·$⁚ltm⁚composite⁚input`) -- the single-pathway composite IS the
/// exact path score -- never the bare module name nor a hardcoded `output`
/// port. The magnitude-1 unit-transfer fallback only fires now when there is
/// genuinely no internal pathway; that case is covered by
/// `test_pathless_module_link_score_uses_unit_transfer` below.
#[test]
fn test_passthrough_module_link_score_uses_composite_on_real_output_port() {
    use salsa::Setter;

    // A passthrough sub-model: `result = input * 2`, output port named
    // `result` (not the stdlib `output` convention), no internal stock, ONE
    // input->output pathway.
    let project = datamodel::Project {
        name: "passthrough_output_name".to_string(),
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
                    x_stock("level", "50", &["adjustment"], &[], None),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "scaled".to_string(),
                        equation: datamodel::Equation::Scalar("custom_pt.result".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    x_aux("gap", "100 - scaled", None),
                    x_flow("adjustment", "gap / 5", None),
                    x_module("custom_pt", &[("level", "custom_pt.input")], None),
                ],
            ),
            x_model(
                "custom_pt",
                vec![
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
                    x_aux("result", "input * 2", None),
                ],
            ),
        ],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, main_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    for discovery in [false, true] {
        source_project.set_ltm_discovery_mode(&mut db).to(discovery);
        let ltm_vars = model_ltm_variables(&db, main_model, source_project);
        // The BASE link score (not a `⁚via⁚` per-exit-port alias).
        let base_name = "$\u{205A}ltm\u{205A}link_score\u{205A}level\u{2192}custom_pt";
        let link = ltm_vars
            .vars
            .iter()
            .find(|v| v.name == base_name)
            .expect("must emit the base level→custom_pt link score");
        let eqn_text = match &link.equation {
            crate::db::LtmEquation::Scalar(arm) => arm.text.clone(),
            other => panic!("module link score should be scalar, got {other:?}"),
        };
        assert!(
            eqn_text.contains("custom_pt\u{00B7}$\u{205A}ltm\u{205A}composite\u{205A}input"),
            "discovery={discovery}: a passthrough with an input->output pathway now exposes a \
             composite; the base link score must reference it; got: {eqn_text}"
        );
        assert!(
            !eqn_text.contains("custom_pt\u{00B7}output"),
            "discovery={discovery}: must not reference the hardcoded custom_pt·output port; \
             got: {eqn_text}"
        );
    }
}

/// A module whose output does NOT depend on its input (`result = 7`, a
/// constant) has no internal input->output pathway, so it exposes NO
/// composite. The base `input → module` link score then falls back to the
/// magnitude-1 signed unit transfer against the module's real output port
/// (`custom_pt·result`), never the bare module name nor a hardcoded `output`
/// port (GH #675; PR #684 confines the arbitrary-port fallback to this
/// genuinely pathway-less residual).
#[test]
fn test_pathless_module_link_score_uses_unit_transfer() {
    use salsa::Setter;

    let project = datamodel::Project {
        name: "pathless_module".to_string(),
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
                    x_stock("level", "50", &["adjustment"], &[], None),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "scaled".to_string(),
                        equation: datamodel::Equation::Scalar("custom_pt.result".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    x_aux("gap", "100 - scaled + level", None),
                    x_flow("adjustment", "gap / 5", None),
                    x_module("custom_pt", &[("level", "custom_pt.input")], None),
                ],
            ),
            x_model(
                "custom_pt",
                vec![
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
                    // Output ignores the input: NO internal pathway, NO composite.
                    x_aux("result", "7", None),
                ],
            ),
        ],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, main_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    for discovery in [false, true] {
        source_project.set_ltm_discovery_mode(&mut db).to(discovery);
        let ltm_vars = model_ltm_variables(&db, main_model, source_project);
        let base_name = "$\u{205A}ltm\u{205A}link_score\u{205A}level\u{2192}custom_pt";
        let link = ltm_vars.vars.iter().find(|v| v.name == base_name);
        // In exhaustive mode there may be no loop through the pathless module
        // (its output is constant), so the base link score is only guaranteed
        // in discovery mode (which scores every edge). Skip if absent.
        let Some(link) = link else {
            assert!(!discovery, "discovery mode must score every edge");
            continue;
        };
        let eqn_text = match &link.equation {
            crate::db::LtmEquation::Scalar(arm) => arm.text.clone(),
            other => panic!("module link score should be scalar, got {other:?}"),
        };
        assert!(
            eqn_text.contains("custom_pt\u{00B7}result"),
            "discovery={discovery}: pathless link score must reference the module's real \
             output port custom_pt·result; got: {eqn_text}"
        );
        assert!(
            !eqn_text.contains("custom_pt\u{00B7}output"),
            "discovery={discovery}: must not reference the hardcoded custom_pt·output port; \
             got: {eqn_text}"
        );
        assert!(
            eqn_text.contains("SIGN("),
            "discovery={discovery}: the fallback must be the magnitude-1 signed unit transfer; \
             got: {eqn_text}"
        );
        assert!(
            !eqn_text.contains("composite"),
            "discovery={discovery}: a pathless module exposes no composite; got: {eqn_text}"
        );
    }
}

/// A module->module link (`mod_a` output wired into `mod_b`'s input port)
/// between two DynamicModules must reference `mod_b`'s composite for that
/// input port (GH #675). The edge source in the parent graph is the
/// normalized module node `mod_a`, but `mod_b`'s `ModuleInput::src` is the
/// module-qualified `mod_a·result`, so the match is by
/// `normalize_module_ref(src)`, not raw equality. The pre-#675 arm emitted
/// the gain `Δmod_b/Δmod_a` against the *bare* module names (not readable
/// scalars), which stubbed the fragment to 0.
#[test]
fn test_module_to_module_link_score_uses_target_composite() {
    use salsa::Setter;

    // Two DynamicModule smooths chained: mod_a smooths `level`, mod_b
    // smooths mod_a's output. Both have an internal stock + input->output
    // pathway, so both emit a composite for their `input` port.
    let smooth_model = |name: &str| {
        x_model(
            name,
            vec![
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
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "flow".to_string(),
                    equation: datamodel::Equation::Scalar("(input - result) / 3".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                x_stock("result", "0", &["flow"], &[], None),
            ],
        )
    };

    let project = datamodel::Project {
        name: "module_to_module".to_string(),
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
                    x_stock("level", "50", &["adjustment"], &[], None),
                    x_module("mod_a", &[("level", "mod_a.input")], None),
                    x_module("mod_b", &[("mod_a.result", "mod_b.input")], None),
                    x_aux("gap", "100 - mod_b.result", None),
                    x_flow("adjustment", "gap / 5", None),
                ],
            ),
            smooth_model("mod_a"),
            smooth_model("mod_b"),
        ],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, main_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    for discovery in [false, true] {
        source_project.set_ltm_discovery_mode(&mut db).to(discovery);
        let ltm_vars = model_ltm_variables(&db, main_model, source_project);
        let link = ltm_vars
            .vars
            .iter()
            .find(|v| v.name.contains("mod_a\u{2192}mod_b"))
            .expect("must emit the mod_a→mod_b link score");
        let eqn_text = match &link.equation {
            crate::db::LtmEquation::Scalar(arm) => arm.text.clone(),
            other => panic!("module link score should be scalar, got {other:?}"),
        };
        assert_eq!(
            eqn_text, "\"mod_b\u{00B7}$\u{205A}ltm\u{205A}composite\u{205A}input\"",
            "discovery={discovery}: module->module link score must reference mod_b's \
             composite for the input port; got: {eqn_text}"
        );
    }
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

/// The results offsets map (`flattened_offsets`, what
/// `CompiledSimulation.offsets` / `Results.offsets` is built from) and the
/// compiled layout (`compute_layout`, what `resolve_module` assigns bytecode
/// slot offsets from) MUST agree on every variable's slot. If they diverge,
/// every results column after the divergence point reads some other
/// variable's data -- silently.
///
/// This is exercised with LTM enabled on a model containing a SMOOTH module:
/// with LTM enabled the sub-model's layout includes LTM synthetic variables,
/// so the module variable spans more than its stdlib template's slots and
/// every variable laid out after it depends on that span.
#[test]
fn test_results_offsets_agree_with_layout_under_ltm() {
    use crate::db::flattened_offsets;
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

    let offsets = flattened_offsets(&db, source_project, source_model);
    let layout = compute_layout(&db, source_model, source_project).root_shifted();

    let mut mismatches: Vec<String> = Vec::new();
    for (name, off) in offsets.iter() {
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
    use crate::db::flattened_offsets;
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

    let offsets = flattened_offsets(&db, source_project, source_model);
    let layout = compute_layout(&db, source_model, source_project).root_shifted();

    let mut mismatches: Vec<(usize, String)> = Vec::new();
    let mut compared = 0usize;
    for (name, off) in offsets.iter() {
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

/// The reviewer's example (PR #684 r3344948690): a passthrough sub-model
/// exposing two parent-visible outputs of opposing sign, `pos = input_val *
/// 0.02` and `neg = 0 - input_val`. The loop reads `m·pos`; a non-loop
/// `watcher` reads `m·neg` so `neg` joins the output ports (sorted first).
fn multi_output_passthrough_project() -> datamodel::Project {
    let sub_model = x_model(
        "passthrough",
        vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "input_val".to_string(),
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
            x_aux("pos", "input_val * 0.02", None),
            x_aux("neg", "0 - input_val", None),
        ],
    );

    let main = x_model(
        "main",
        vec![
            x_stock("s", "100", &["growth"], &[], None),
            datamodel::Variable::Module(datamodel::Module {
                ident: "m".to_string(),
                model_name: "passthrough".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: "s".to_string(),
                    dst: "m.input_val".to_string(),
                }],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }),
            x_flow("growth", "m.pos * 0.1", None),
            x_aux("watcher", "m.neg", None),
        ],
    );

    datamodel::Project {
        name: "multi_output_passthrough".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 8.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![main, sub_model],
        source: None,
        ai_information: None,
    }
}

/// Part C: a stockless passthrough sub-model with input->output pathways
/// must emit pathway (`$⁚ltm⁚path⁚`) and composite (`$⁚ltm⁚composite⁚`)
/// synthetic vars, even though it has no internal stocks. Before the fix
/// the stock-free early return in `model_ltm_variables` dropped them all.
#[test]
fn test_passthrough_submodel_emits_pathway_and_composite_vars() {
    use salsa::Setter;

    let project = multi_output_passthrough_project();
    let mut db = SimlinDb::default();
    let (source_project, sub_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["passthrough"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, sub_model, source_project);
    let has_path = ltm_vars
        .vars
        .iter()
        .any(|v| v.name.contains("\u{205A}path\u{205A}"));
    let has_composite = ltm_vars
        .vars
        .iter()
        .any(|v| v.name.contains("\u{205A}composite\u{205A}"));
    assert!(
        has_path,
        "stockless passthrough sub-model must emit $⁚ltm⁚path⁚ vars; got: {:?}",
        ltm_vars.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    assert!(
        has_composite,
        "stockless passthrough sub-model must emit $⁚ltm⁚composite⁚ vars; got: {:?}",
        ltm_vars.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

/// Part C: a genuinely stock-free ROOT model (no parent reads its
/// variables, so `find_model_output_ports` is empty) must still emit no LTM
/// vars -- the restructured early return only suppresses output when there
/// are also no input-port pathways.
#[test]
fn test_stock_free_root_model_emits_no_ltm_vars() {
    use salsa::Setter;

    let project = datamodel::Project {
        name: "stock_free_root".to_string(),
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
            vec![x_aux("a", "1", None), x_aux("b", "a * 2", None)],
        )],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let (source_project, main_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, main_model, source_project);
    assert!(
        ltm_vars.vars.is_empty(),
        "a stock-free root model with no input ports must emit no LTM vars; got: {:?}",
        ltm_vars.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
}

/// Part P: the parent's `s -> m` loop link score must be the per-exit-port
/// alias that selects the `pos`-terminal pathway the loop actually
/// traverses -- not the composite (which max-abs-picks `neg`), not the raw
/// `m·neg` unit transfer. The alias's equation must reference the pathway
/// var that ends at `pos`.
#[test]
fn test_multi_output_loop_link_uses_per_exit_port_alias() {
    use salsa::Setter;

    let project = multi_output_passthrough_project();
    let mut db = SimlinDb::default();
    let (source_project, main_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    let ltm_vars = model_ltm_variables(&db, main_model, source_project);

    // The per-exit-port alias for the s->m link via the `pos` exit port.
    let alias_name = "$\u{205A}ltm\u{205A}link_score\u{205A}s\u{2192}m\u{205A}via\u{205A}pos";
    let alias = ltm_vars
        .vars
        .iter()
        .find(|v| v.name == alias_name)
        .unwrap_or_else(|| {
            panic!(
                "must emit per-exit-port alias {alias_name}; got: {:?}",
                ltm_vars.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
            )
        });
    let alias_eqn = match &alias.equation {
        crate::db::LtmEquation::Scalar(arm) => arm.text.clone(),
        other => panic!("alias should be scalar, got {other:?}"),
    };
    // The alias must reference one of m's pathway vars (the pos-terminal
    // one), never the composite or the raw neg output.
    assert!(
        alias_eqn.contains("m\u{00B7}$\u{205A}ltm\u{205A}path\u{205A}"),
        "alias must reference a pathway var of m, got: {alias_eqn}"
    );
    assert!(
        !alias_eqn.contains("composite"),
        "alias must not reference the composite (which max-abs picks neg), got: {alias_eqn}"
    );
    assert!(
        !alias_eqn.contains("m\u{00B7}neg"),
        "alias must not reference the raw m·neg output, got: {alias_eqn}"
    );

    // The loop score equation must reference the alias for its s->m link.
    let loop_score = ltm_vars
        .vars
        .iter()
        .find(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .expect("must emit a loop_score var");
    let loop_eqn = match &loop_score.equation {
        crate::db::LtmEquation::Scalar(arm) => arm.text.clone(),
        other => panic!("loop score should be scalar, got {other:?}"),
    };
    assert!(
        loop_eqn.contains(alias_name),
        "loop score must reference the per-exit-port alias {alias_name}; got: {loop_eqn}"
    );
}

/// Assign sequential UIDs to every variable of every model in `project`, so
/// `loop_metadata` (the pinned-loop primitive) can reference them.
fn assign_uids_for_pinning(project: &mut datamodel::Project) {
    for model in &mut project.models {
        for (i, var) in model.variables.iter_mut().enumerate() {
            let uid = (i as i32) + 1;
            match var {
                datamodel::Variable::Stock(s) => s.uid = Some(uid),
                datamodel::Variable::Flow(f) => f.uid = Some(uid),
                datamodel::Variable::Aux(a) => a.uid = Some(uid),
                datamodel::Variable::Module(m) => m.uid = Some(uid),
            }
        }
    }
}

/// Pin a loop on `model_name` by variable idents, mirroring the `SetLoopName`
/// patch path: assign UIDs to the named variables and append the
/// `LoopMetadata` entry those UIDs resolve from at sync time.
fn pin_loop_by_names(
    project: &mut datamodel::Project,
    model_name: &str,
    name: &str,
    variables: &[&str],
) {
    let model = project
        .models
        .iter_mut()
        .find(|m| m.name == model_name)
        .expect("model exists");
    let uids: Vec<i32> = variables
        .iter()
        .map(|v| {
            model
                .variables
                .iter()
                .find(|var| var.get_ident() == *v)
                .and_then(crate::patch::variable_uid)
                .unwrap_or_else(|| panic!("variable {v} has no uid"))
        })
        .collect();
    model.loop_metadata.push(datamodel::LoopMetadata {
        uids,
        deleted: false,
        name: name.to_string(),
        description: String::new(),
    });
}

/// Two-model fixture for the GH #673 pin-through-module tests: `main` has a
/// 3-node cycle `driver -> {module} -> reader -> driver` whose ONLY state (if
/// any) lives inside the module's sub-model. `sub_vars` supplies the
/// passthrough.
fn pin_through_module_project(sub_vars: Vec<datamodel::Variable>) -> datamodel::Project {
    datamodel::Project {
        name: "pin_through_module".to_string(),
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
                    x_aux("driver", "100 + reader * 0.5", None),
                    x_module("sub", &[("driver", "sub.input")], None),
                    x_aux("reader", "sub.output", None),
                ],
            ),
            x_model("sub", sub_vars),
        ],
        source: None,
        ai_information: None,
    }
}

/// GH #673: a pinned loop whose only stock lives INSIDE a module it traverses
/// (a smooth-like sub-model) must validate -- the old check compared the
/// cycle's nodes against the PARENT-level stock set only, so the module-borne
/// state was invisible and the pin was wrongly rejected as "contains no
/// stock" even though the enumerator finds and scores the same cycle.
#[test]
fn test_pinned_loop_through_module_with_internal_stock_validates() {
    let mut project = pin_through_module_project(vec![
        x_aux("input", "0", None),
        x_flow("chg", "(input - output) / 3", None),
        x_stock("output", "0", &["chg"], &[], None),
    ]);
    assign_uids_for_pinning(&mut project);
    pin_loop_by_names(
        &mut project,
        "main",
        "smooth loop",
        &["driver", "sub", "reader"],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let result = model_pinned_loops(&db, sync.models["main"].source, sync.project);

    assert!(
        result.invalid.is_empty(),
        "a pin whose only stock is module-internal must validate; got invalid: {:?}",
        result.invalid
    );
    assert_eq!(result.loops.len(), 1, "the pin must resolve to one loop");
    let pin = &result.loops[0];
    assert_eq!(pin.name, "smooth loop");
    assert_eq!(pin.loops.len(), 1);
    let l = &pin.loops[0];
    assert_eq!(l.id, "pin1");
    // The resolved Loop must carry the module-internal stock (namespaced with
    // the module instance name), the same enrichment the enumerator applies,
    // so downstream partition resolution sees the loop's state.
    assert!(
        l.stocks.iter().any(|s| s.as_str() == "sub\u{00B7}output"),
        "the loop must carry the module-internal stock; got stocks: {:?}",
        l.stocks
    );
}

/// GH #673 (the other direction): a pinned cycle through a stockless
/// PASSTHROUGH module -- no stock anywhere on the cycle, inside or out -- is
/// a purely-instantaneous circular dependency, not a feedback loop, and must
/// STILL be rejected with the clear "contains no stock" reason.
#[test]
fn test_pinned_loop_through_stockless_passthrough_still_rejected() {
    let mut project = pin_through_module_project(vec![
        x_aux("input", "0", None),
        x_aux("output", "input * 2", None),
    ]);
    assign_uids_for_pinning(&mut project);
    pin_loop_by_names(
        &mut project,
        "main",
        "instantaneous",
        &["driver", "sub", "reader"],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let result = model_pinned_loops(&db, sync.models["main"].source, sync.project);

    assert!(
        result.loops.is_empty(),
        "a stockless cycle through a passthrough module must not score; got: {:?}",
        result.loops.iter().map(|p| &p.name).collect::<Vec<_>>()
    );
    assert_eq!(result.invalid.len(), 1);
    let (name, reason) = &result.invalid[0];
    assert_eq!(name, "instantaneous");
    assert!(
        reason.contains("contains no stock"),
        "rejection must carry the clear no-stock reason; got: {reason}"
    );
}

/// GH #971: when a target reads MORE THAN ONE output of a single module
/// instance, `module_link_score_equation`'s module->variable branch holds ONE
/// output live (its change is what the score attributes) and freezes the
/// rest. The DOCUMENT-order-first output comes from the reference-site IR's
/// per-occurrence enumeration (`OccurrenceRef::ModuleOutput`, walk order), so
/// the choice is deterministic and follows source evaluation order.
///
/// This pins the fix by building two models that differ ONLY in the order the
/// target reads the module's two outputs and asserting they hold different
/// sources live. An alphabetical sort would pick `out_a` in both. The live source is
/// identifiable because it -- and only it -- drives the guard form's SIGN
/// normalizer `(src - PREVIOUS(src))`; the frozen output appears solely inside
/// a `PREVIOUS(...)`.
#[test]
fn test_multi_output_module_link_score_holds_document_order_first_live() {
    use salsa::Setter;

    let build_link_eqn = |combined_eqn: &str| -> String {
        let project = datamodel::Project {
            name: "multi_output".to_string(),
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
                        x_stock("level", "50", &["adjustment"], &[], None),
                        x_aux("combined", combined_eqn, None),
                        x_flow("adjustment", "combined / 5", None),
                        x_module("multi_out", &[("level", "multi_out.input")], None),
                    ],
                ),
                x_model(
                    "multi_out",
                    vec![
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
                        x_aux("out_a", "input * 2", None),
                        x_aux("out_b", "input * 3", None),
                    ],
                ),
            ],
            source: None,
            ai_information: None,
        };

        let mut db = SimlinDb::default();
        let (source_project, main_model) = {
            let sync = sync_from_datamodel(&db, &project);
            (sync.project, sync.models["main"].source)
        };
        source_project.set_ltm_enabled(&mut db).to(true);

        let ltm_vars = model_ltm_variables(&db, main_model, source_project);
        let link = ltm_vars
            .vars
            .iter()
            .find(|v| v.name.contains("multi_out\u{2192}combined"))
            .unwrap_or_else(|| {
                panic!(
                    "must emit the multi_out->combined link score; got: {:?}",
                    ltm_vars.vars.iter().map(|v| &v.name).collect::<Vec<_>>()
                )
            });
        match &link.equation {
            crate::db::LtmEquation::Scalar(arm) => arm.text.clone(),
            other => panic!("module link score should be scalar, got {other:?}"),
        }
    };

    // The live source is the sole reference spelled as `(src - PREVIOUS(src))`
    // (the SIGN normalizer); a frozen output only ever appears wrapped in
    // `PREVIOUS(...)`.
    let live_diff =
        |out: &str| format!("\"multi_out\u{00B7}{out}\" - PREVIOUS(\"multi_out\u{00B7}{out}\")");

    let eqn_ab = build_link_eqn("multi_out.out_a + multi_out.out_b");
    assert!(
        eqn_ab.contains(&live_diff("out_a")),
        "reads out_a first, so out_a is held live: {eqn_ab}"
    );
    assert!(
        !eqn_ab.contains(&live_diff("out_b")),
        "out_b (read second) is frozen, never the SIGN source: {eqn_ab}"
    );

    let eqn_ba = build_link_eqn("multi_out.out_b + multi_out.out_a");
    assert!(
        eqn_ba.contains(&live_diff("out_b")),
        "reads out_b first, so out_b is held live -- document order, not \
         alphabetical (which would pick out_a): {eqn_ba}"
    );
    assert!(
        !eqn_ba.contains(&live_diff("out_a")),
        "out_a (read second) is frozen: {eqn_ba}"
    );
}
