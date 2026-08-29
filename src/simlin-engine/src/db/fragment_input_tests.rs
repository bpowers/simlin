// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Pins for the one fragment compiler (`compiler::fragment::lower_fragment`)
//! and the four `FragmentInput` constructors that feed it.
//!
//! The constructors ARE the production path: every fragment
//! `compile_var_fragment`, `compile_implicit_var_fragment`,
//! `compile_ltm_equation_fragment` and `compile_ltm_implicit_var_fragment`
//! emit is a constructor's `FragmentInput`, lowered by `lower_fragment` and
//! emitted through `FragmentInput::emit_ctx`. The rows below derive each
//! fixture THROUGH those production entry points and check that running the
//! three steps by hand reproduces the production bytecode byte for byte, one
//! row per constructor (the enumeration is the four emitters, and every arm
//! has a row).

use std::collections::BTreeSet;

use super::*;
use crate::compiler::fragment::{DepKind, FragmentInput, lower_fragment};
use crate::compiler::symbolic::PerVarBytecodes;
use crate::datamodel;
use crate::test_common::TestProject;

/// `main` holds a three-element `arr[d]`, `src`, a module `sub` instantiating
/// `producer` (wired `src -> sub.input`), and `usesub`, which reads the module
/// output beside a reduction over the arrayed variable -- so its fragment has
/// an arrayed dependency and a module dependency at once. `producer` exposes
/// `input`, an arrayed `arr[d] = input * d`, and `output = SUM(arr)`.
fn module_and_array_project() -> datamodel::Project {
    let mut project = TestProject::new("fragment_input")
        .with_sim_time(0.0, 1.0, 1.0)
        .indexed_dimension("d", 3)
        .aux("src", "3", None)
        .array_aux("arr[d]", "d")
        .aux("usesub", "sub.output * 2 + SUM(arr)", None)
        .aux("pick", "sub.arr[2]", None)
        .build_datamodel();

    let aux = |ident: &str, equation: datamodel::Equation, can_be_module_input: bool| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation,
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input,
                ..datamodel::Compat::default()
            },
        })
    };
    project.models.push(datamodel::Model {
        name: "producer".to_string(),
        sim_specs: None,
        variables: vec![
            aux("input", datamodel::Equation::Scalar("0".to_string()), true),
            aux(
                "arr",
                datamodel::Equation::ApplyToAll(vec!["d".to_string()], "input * d".to_string()),
                false,
            ),
            aux(
                "output",
                datamodel::Equation::Scalar("SUM(arr)".to_string()),
                false,
            ),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
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
        }));
    project
}

/// `main` with `src`, the `producer` module `sub`, and `sm = SMTH1(sub.output *
/// 2, 2)` -- a stdlib call whose hoisted argument helper reads a module output.
fn smooth_of_module_output_project() -> datamodel::Project {
    let mut project = TestProject::new("smooth_of_module_output")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("src", "3", None)
        .aux("sm", "SMTH1(sub.output * 2, 2)", None)
        .build_datamodel();
    project.models.push(datamodel::Model {
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
                equation: datamodel::Equation::Scalar("input * 10".to_string()),
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
    });
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
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
        }));
    project
}

fn ltm_loop_project() -> datamodel::Project {
    TestProject::new("fragment_input_ltm")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("rate", "0.1", None)
        .flow("growth", "level * rate", None)
        .stock("level", "10", &["growth"], &[], None)
        .build_datamodel()
}

fn main_model(db: &SimlinDb, project: SourceProject) -> SourceModel {
    *project
        .models(db)
        .get("main")
        .expect("fixture has a main model")
}

/// Lower one phase of `input` and emit it exactly as the production emitters
/// do, so a row can compare the hand-run pipeline against the emitter.
fn lower_and_emit(input: &FragmentInput<'_>, is_initial: bool) -> Option<PerVarBytecodes> {
    let var = lower_fragment(input, is_initial).ok()?;
    crate::db::assemble::compile_phase_to_per_var_bytecodes(&input.emit_ctx(), &var.ast)
}

/// Row 1 of 4: the explicit constructor, for a variable with an arrayed
/// dependency and a module dependency.
#[test]
fn explicit_constructor_is_compile_var_fragments_input() {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &module_and_array_project());
    let model = main_model(&db, sync.project);
    let var = model.variables(&db)["usesub"];

    let production =
        compile_var_fragment(&db, var, model, sync.project, ModuleInputSet::empty(&db))
            .as_ref()
            .expect("usesub compiles");

    let explicit =
        crate::db::var_fragment::explicit_fragment_input(&db, var, model, sync.project, &[]);
    let input = explicit.input.expect("usesub must lower");
    let lowered = lowered_source_variable(&db, var, model, sync.project);
    assert!(matches!(&input.target, std::borrow::Cow::Borrowed(_)));
    assert!(
        std::sync::Arc::ptr_eq(input.target.as_ref(), lowered),
        "the explicit constructor borrows the production lowering memo payload"
    );

    let arr = &input.deps["arr"];
    assert!(
        matches!(arr.kind, DepKind::Var) && arr.dims.len() == 1 && arr.dims[0].len() == 3,
        "the arrayed dependency carries its declared dimension"
    );
    let producer = *sync.project.models(&db).get("producer").unwrap();
    let DepKind::Module { shape } = &input.deps["sub"].kind else {
        panic!("the module dependency is a Module shape");
    };
    assert_eq!(
        shape.as_ref(),
        crate::db::layout::model_shape(&db, producer, sync.project).as_ref(),
        "a module dependency carries the sub-model's shape"
    );

    // The emitter lowers only the phases the variable's runlist membership
    // admits: `usesub` is a flow-phase aux, so the flow fragment is the one to
    // compare, and the initial phase is (correctly) absent.
    assert_eq!(
        lower_and_emit(&input, false),
        production.fragment.flow_bytecodes,
        "the flow fragment is the constructor's input lowered and emitted"
    );
    assert!(
        production.fragment.flow_bytecodes.is_some()
            && production.fragment.initial_bytecodes.is_none(),
        "usesub is a member of the flows runlist only"
    );
}

/// Row 2 of 4: the implicit-helper constructor, for both helper kinds a
/// `SMTH1` call synthesizes -- the hoisted delay-time aux and the module
/// instance itself.
#[test]
fn implicit_constructor_is_compile_implicit_var_fragments_input() {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &smooth_of_module_output_project());
    let model = main_model(&db, sync.project);
    let inputs = ModuleInputSet::empty(&db);

    let helpers = model_implicit_var_info(&db, model, sync.project);
    let mut names: Vec<&String> = helpers.keys().collect();
    names.sort();
    assert_eq!(
        names,
        ["$⁚sm⁚0⁚arg0", "$⁚sm⁚0⁚arg1", "$⁚sm⁚0⁚smth1"],
        "the SMTH1 call synthesizes two argument helpers and one module instance"
    );

    for name in names {
        let production =
            compile_implicit_var_fragment(&db, model, sync.project, name.clone(), inputs)
                .as_ref()
                .unwrap_or_else(|| panic!("{name} compiles"));
        let input = crate::db::fragment_compile::implicit_fragment_input(
            &db,
            &helpers[name],
            model,
            sync.project,
            &[],
        )
        .unwrap_or_else(|_| panic!("{name} has a fragment input"));
        let lowered = lowered_implicit_variable(&db, model, sync.project, name.clone())
            .as_ref()
            .unwrap_or_else(|| panic!("{name} has a production lowering memo"));
        assert!(matches!(&input.target, std::borrow::Cow::Borrowed(_)));
        assert!(
            std::sync::Arc::ptr_eq(input.target.as_ref(), lowered),
            "{name}: ordinary implicit construction borrows its production memo payload"
        );
        // Every phase the emitter's runlist gate admitted is the constructor's
        // input lowered for that phase; the module instance is the one helper
        // that carries all three.
        let frag = &production.fragment;
        assert!(
            frag.initial_bytecodes.is_some() && frag.flow_bytecodes.is_some()
                || frag.stock_bytecodes.is_some(),
            "{name} emits at least its value-bearing phase"
        );
        for (label, expected, is_initial) in [
            ("initial", &frag.initial_bytecodes, true),
            ("flow", &frag.flow_bytecodes, false),
            ("stock", &frag.stock_bytecodes, false),
        ] {
            if expected.is_some() {
                assert_eq!(
                    &lower_and_emit(&input, is_initial),
                    expected,
                    "{name}: the {label} fragment is the constructor's input lowered and emitted"
                );
            }
        }
    }
}

/// Row 3 of 4: the LTM synthetic-variable constructor.
#[test]
fn ltm_constructor_is_compile_ltm_equation_fragments_input() {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &ltm_loop_project());
    set_project_ltm_enabled(&mut db, sync.project, true);
    let model = main_model(&db, sync.project);

    let ltm_vars = model_ltm_variables(&db, model, sync.project);
    assert!(
        ltm_vars
            .vars
            .iter()
            .any(|v| v.name == "$⁚ltm⁚link_score⁚growth→level"),
        "the loop emits the growth->level link score"
    );
    for ltm_var in &ltm_vars.vars {
        let production = crate::db::ltm::compile_ltm_equation_fragment(
            &db,
            &ltm_var.name,
            &ltm_var.equation,
            model,
            sync.project,
            None,
        )
        .unwrap_or_else(|| panic!("{} compiles", ltm_var.name));
        let input = crate::db::ltm::ltm_fragment_input(
            &db,
            &ltm_var.name,
            &ltm_var.equation,
            model,
            sync.project,
        )
        .unwrap_or_else(|diagnostics| panic!("{}: {diagnostics:?}", ltm_var.name));
        assert!(
            matches!(&input.target, std::borrow::Cow::Owned(_)),
            "{}: an LTM synthetic equation is transient and must transfer ownership",
            ltm_var.name
        );
        assert_eq!(
            lower_and_emit(&input, false),
            production.fragment.flow_bytecodes,
            "{}: the flow fragment is the constructor's input lowered and emitted",
            ltm_var.name
        );
    }
}

/// Row 4 of 4: the LTM implicit-helper constructor.
#[test]
fn ltm_implicit_constructor_is_compile_ltm_implicit_var_fragments_input() {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &ltm_loop_project());
    set_project_ltm_enabled(&mut db, sync.project, true);
    let model = main_model(&db, sync.project);

    let helpers = model_ltm_implicit_var_info(&db, model, sync.project);
    assert!(
        helpers.contains_key("$⁚$⁚ltm⁚link_score⁚growth→level⁚0⁚arg0"),
        "the growth->level score synthesizes PREVIOUS capture helpers"
    );
    for (name, meta) in helpers.iter() {
        let production = crate::db::ltm::compile_ltm_implicit_var_fragment(
            &db,
            meta,
            model,
            sync.project,
            &[],
            None,
        )
        .unwrap_or_else(|| panic!("{name} compiles"));
        let input =
            crate::db::ltm::ltm_implicit_fragment_input(&db, meta, model, sync.project, &[])
                .unwrap_or_else(|diagnostics| {
                    panic!("{name} has a fragment input: {diagnostics:?}")
                });
        assert!(
            matches!(&input.target, std::borrow::Cow::Owned(_)),
            "{name}: an LTM helper is synthesized from a transient LTM parse and must be owned"
        );
        let capture = meta.variable.capture();
        let expected_flow = (!meta.is_stock
            && capture.is_none_or(|capture| capture.kind().needs_flows()))
        .then(|| lower_and_emit(&input, false))
        .flatten();
        assert_eq!(
            expected_flow, production.fragment.flow_bytecodes,
            "{name}: the flow fragment is the constructor's input lowered and emitted when demanded"
        );
        let expected_initial = capture
            .is_none_or(|capture| capture.kind().needs_initials())
            .then(|| lower_and_emit(&input, true))
            .flatten();
        assert_eq!(
            expected_initial, production.fragment.initial_bytecodes,
            "{name}: the initial fragment likewise, when demanded"
        );
    }
}

/// A cross-module read `sub·arr[2]` lowers to `VarRef { name: sub,
/// element_offset: arr's slot inside the instance + 1 }`, read off the
/// dependency's `DepKind::Module` shape -- the same slot `compute_layout`
/// assigns `arr` in `producer`. The parent's layout has one entry spanning the
/// whole instance and none for `sub·arr`, so this offset is the only slot
/// arithmetic lowering ever does.
#[test]
fn cross_module_read_offsets_through_the_sub_models_shape() {
    use crate::compiler::VarRef;
    use crate::compiler::symbolic::SymbolicOpcode;

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &module_and_array_project());
    let model = main_model(&db, sync.project);
    let producer = *sync.project.models(&db).get("producer").unwrap();
    let arr_slot = compute_layout(&db, producer, sync.project)
        .get("arr")
        .expect("producer lays out arr")
        .offset;

    let var = model.variables(&db)["pick"];
    let explicit =
        crate::db::var_fragment::explicit_fragment_input(&db, var, model, sync.project, &[]);
    let input = explicit.input.expect("pick must lower");
    let DepKind::Module { shape } = &input.deps["sub"].kind else {
        panic!("sub is a module dependency");
    };
    assert_eq!(shape.vars["arr"].offset, arr_slot);

    // The lowered form is a collapsed view over the instance (`sub` at `arr`'s
    // slot, element 1 selected in the view); the emitted read is the one
    // `LoadVar`, at the sub-model's slot plus the element.
    let emitted = lower_and_emit(&input, false).expect("pick emits");
    let loads: Vec<&VarRef> = emitted
        .symbolic
        .code
        .iter()
        .filter_map(|op| match op {
            SymbolicOpcode::LoadVar { var } => Some(var),
            _ => None,
        })
        .collect();
    assert_eq!(
        loads,
        vec![&VarRef::new(Ident::new("sub"), arr_slot + 1)],
        "the element read relocates through the instance by the sub-model's slot"
    );

    // End to end: producer.input = 3, so arr = [3, 6, 9] and pick = arr[2] = 6.
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = vm.into_results();
    let pick = results.offsets[&Ident::new("pick")];
    assert_eq!(results.data[pick], 6.0);
}

/// Every variable's series, step-major, keyed by canonical name.
fn run_series(
    db: &SimlinDb,
    project: SourceProject,
) -> std::collections::HashMap<String, Vec<f64>> {
    let compiled = compile_project_incremental(db, project, "main").expect("compiles");
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("runs");
    let results = vm.into_results();
    results
        .offsets
        .iter()
        .map(|(name, off)| {
            let series = (0..results.step_count)
                .map(|step| results.data[step * results.step_size + off])
                .collect();
            (name.as_str().to_string(), series)
        })
        .collect()
}

/// A stdlib call whose hoisted argument reads a module output
/// (`SMTH1(sub·output * 2, 2)`) resolves `sub` through its own dependency
/// shapes, like every other fragment.
///
/// Expected values, derived from the rules: `sub·output = 3 * 10 = 30`, so the
/// helper -- and the SMTH1 instance's `input` -- is 60 on every step. The
/// instance's stock starts from that initial-phase input. Cross-model initial
/// requirements seed `producer.output` and its local input closure, so the
/// smooth starts and remains at 60.
#[test]
fn smooth_argument_reading_a_module_output_compiles() {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &smooth_of_module_output_project());
    let diagnostics = collect_all_diagnostics(&db, sync.project);
    assert!(
        diagnostics.is_empty(),
        "no diagnostics expected, got {diagnostics:?}"
    );
    let series = run_series(&db, sync.project);
    assert_eq!(series["$⁚sm⁚0⁚arg0"], vec![60.0, 60.0, 60.0]);
    assert_eq!(series["$⁚sm⁚0⁚smth1·input"], vec![60.0, 60.0, 60.0]);
    assert_eq!(series["sm"], vec![60.0, 60.0, 60.0]);
}

/// A stock's initial equation can read a stockless sub-model output. The
/// qualified initial dependency seeds exactly that output and its child-local
/// dependency closure, so the parent snapshots the computed value rather than
/// the module block's zero-filled storage.
#[test]
fn stock_initialized_from_a_stockless_modules_output_reads_its_t0_value() {
    let mut project = smooth_of_module_output_project();
    project.models[0]
        .variables
        .push(datamodel::Variable::Stock(datamodel::Stock {
            ident: "level".to_string(),
            equation: datamodel::Equation::Scalar("sub.output".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let series = run_series(&db, sync.project);
    assert_eq!(series["sub·output"], vec![30.0, 30.0, 30.0]);
    assert_eq!(series["level"], vec![30.0, 30.0, 30.0]);
}

/// `module_input_set` is the one owner of "which ports does this wiring bind":
/// a `dst` inside the instance's namespace yields its bare port, a `dst`
/// outside it is dropped, and the result is a set (sorted, deduplicated).
#[test]
fn module_input_set_strips_the_instance_prefix_and_drops_foreign_targets() {
    let set = crate::db::assemble::module_input_set(
        "m\u{00B7}",
        [
            ("a", "m\u{00B7}zeta"),
            ("b", "other\u{00B7}port"),
            ("c", "m\u{00B7}alpha"),
            ("d", "m\u{00B7}zeta"),
        ]
        .into_iter(),
    );
    let expected: BTreeSet<Ident<Canonical>> = [Ident::new("alpha"), Ident::new("zeta")].into();
    assert_eq!(set, expected);
}
