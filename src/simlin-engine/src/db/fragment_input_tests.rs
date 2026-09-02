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
pub(super) fn module_and_array_project() -> datamodel::Project {
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

    let crate::db::var_fragment::ExplicitFragment::Ready { input, .. } =
        crate::db::var_fragment::explicit_fragment_input(&db, var, model, sync.project, &[])
    else {
        panic!("usesub must lower");
    };

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
        **shape,
        **crate::db::layout::model_shape(&db, producer, sync.project),
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
        .unwrap_or_else(|reason| panic!("{}: {reason}", ltm_var.name));
        assert_eq!(
            lower_and_emit(&input, false),
            production.fragment.flow_bytecodes,
            "{}: the flow fragment is the constructor's input lowered and emitted",
            ltm_var.name
        );
    }
}

/// Row 4 of 4: the LTM implicit-helper constructor. The phases compared are
/// the ones the helper's capture kind demands -- the generator's helpers are
/// `PREVIOUS` captures, so flow only.
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
                .unwrap_or_else(|| panic!("{name} has a fragment input"));
        let kind = meta
            .variable
            .capture()
            .unwrap_or_else(|| panic!("{name} is a capture"))
            .kind();
        assert_eq!(
            kind.needs_flows()
                .then(|| lower_and_emit(&input, false))
                .flatten(),
            production.fragment.flow_bytecodes,
            "{name}: the flow fragment is the constructor's input lowered and emitted, \
             when the kind demands it"
        );
        assert_eq!(
            kind.needs_initials()
                .then(|| lower_and_emit(&input, true))
                .flatten(),
            production.fragment.initial_bytecodes,
            "{name}: the initial fragment likewise"
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
    let crate::db::var_fragment::ExplicitFragment::Ready { input, .. } =
        crate::db::var_fragment::explicit_fragment_input(&db, var, model, sync.project, &[])
    else {
        panic!("pick must lower");
    };
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
/// (`SMTH1(sub·output * 2, 2)`) compiles: the hoisted helper `$⁚sm⁚0⁚arg0 =
/// sub·output * 2` resolves `sub` through its own dependency shapes, like
/// every other fragment. This is the one shape the unified compiler changed
/// (the design plan's "Phase 3 semantic divergences" paragraph records the
/// refusal it replaces), so its values are pinned here.
///
/// Expected values, derived from the rules: `sub·output = 3 * 10 = 30`, so the
/// helper -- and the SMTH1 instance's `input` -- is 60 on every step. The
/// instance's stock starts from the value `input` has during the parent's
/// INITIALS phase; `producer` evaluates `output` there because every
/// value-bearing variable of an instantiated model is an initials member
/// (GH #1028), so the smooth starts at 60 and stays there.
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

/// GH #1028: a stock initialized from a STOCKLESS sub-model's output reads
/// the output's t=0 value. This is the issue's repro model: `producer`
/// (`input`, `output = input * 10`, no stock) is instantiated by `main` with
/// `src = 3`, and `level = INTEG(0, sub·output)` must start at 30 -- which it
/// does because every value-bearing variable of an instantiated model is an
/// initials member of that model, so `output` exists when the parent's
/// initials read it. The `sm = SMTH1(sub·output * 2, 2)` beside it is the
/// issue's second symptom, pinned at 60 by the test above.
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

/// A plain aux of `main`, the `producer` module's port wiring, and the stock
/// reading a module output, as `datamodel` variables.
fn plain_aux(ident: &str, equation: &str, active_initial: Option<&str>) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat {
            active_initial: active_initial.map(str::to_string),
            ..datamodel::Compat::default()
        },
    })
}

fn module_var(ident: &str, model_name: &str, refs: &[(&str, &str)]) -> datamodel::Variable {
    datamodel::Variable::Module(datamodel::Module {
        ident: ident.to_string(),
        model_name: model_name.to_string(),
        documentation: String::new(),
        units: None,
        references: refs
            .iter()
            .map(|(src, dst)| datamodel::ModuleReference {
                src: src.to_string(),
                dst: dst.to_string(),
            })
            .collect(),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn stock_from(ident: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Stock(datamodel::Stock {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        inflows: vec![],
        outflows: vec![],
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

/// GH #1028 through a NESTED instance: `main` instantiates `outer_model` as
/// `outer`, which instantiates `producer` as `inner` and exposes `out =
/// inner·output`. Both sub-models are stockless, both are module targets, so
/// both evaluate every aux in their initials, and `level = INTEG(0,
/// outer·out)` starts at 30.
#[test]
fn stock_initialized_through_a_nested_stockless_module_reads_its_t0_value() {
    let mut project = smooth_of_module_output_project();
    project.models.push(datamodel::Model {
        name: "outer_model".to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "feed".to_string(),
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
            module_var("inner", "producer", &[("feed", "inner.input")]),
            plain_aux("out", "inner.output", None),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project.models[0]
        .variables
        .push(module_var("outer", "outer_model", &[("src", "outer.feed")]));
    project.models[0]
        .variables
        .push(stock_from("level", "outer.out"));
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let series = run_series(&db, sync.project);
    assert_eq!(series["outer·out"], vec![30.0, 30.0, 30.0]);
    assert_eq!(series["level"], vec![30.0, 30.0, 30.0]);
}

/// GH #1028 with an `ACTIVE INITIAL`: the sub-model's initials evaluate its
/// auxes' INITIAL equations, so a parent stock initialized from a port whose
/// `ACTIVE INITIAL` is `input * 100` starts at 300 while the port's own
/// series is its dt equation's 30.
#[test]
fn stock_initialized_from_an_active_initial_module_output_reads_the_initial_equation() {
    let mut project = smooth_of_module_output_project();
    let producer = project
        .models
        .iter_mut()
        .find(|m| m.name == "producer")
        .expect("fixture has producer");
    producer
        .variables
        .push(plain_aux("staged", "input * 10", Some("input * 100")));
    project.models[0]
        .variables
        .push(stock_from("level", "sub.staged"));
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let series = run_series(&db, sync.project);
    assert_eq!(series["sub·staged"], vec![30.0, 30.0, 30.0]);
    assert_eq!(series["level"], vec![300.0, 300.0, 300.0]);
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

/// An XMILE document with the given main-model variables and extra models,
/// over `TIME = 0..3` at `dt = 1`, read through the production reader.
fn xmile_project(main_variables: &str, extra_models: &str) -> datamodel::Project {
    let source = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0" xmlns:simlin="https://simlin.com/XMILE/v1.0" version="1.0">
  <header><vendor>test</vendor><product lang="en">test</product></header>
  <sim_specs method="Euler" time_units="Month"><start>0</start><stop>3</stop><dt>1</dt></sim_specs>
  <model><variables>
    <aux name="src"><eqn>TIME + 3</eqn></aux>
    {main_variables}
  </variables></model>
  {extra_models}
</xmile>"#
    );
    crate::compat::open_xmile(&mut source.as_bytes()).expect("the XMILE fixture imports")
}

/// The t=0 value of every variable of `project`'s root, keyed by canonical
/// name.
fn t0_values(project: &datamodel::Project) -> std::collections::HashMap<String, f64> {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    run_series(&db, sync.project)
        .into_iter()
        .map(|(name, series)| (name, series[0]))
        .collect()
}

/// GH #1028, the shapes a per-instance propagation of "which ports does the
/// parent read" would get wrong and the flat rule gets right by construction.
/// Each row is a parent stock initialized from a sub-model port, with the
/// value derived from the flat-model reading of the equations.
///
/// * A sub-model aux that reads a sub-model STOCK: `out = s * 10 + input`
///   over `s = INTEG(input, input * 2)` with `input = 3` at t=0 is
///   `6 * 10 + 3 = 63`; the initials closure orders `s` before `out`.
/// * One sub-model instantiated twice with different input sets: `sub1`
///   leaves `extra` unwired (its own equation, 1) and `sub2` wires it to
///   `bonus = 100`, so `output = input * 10 + extra` is `31` and `130`.
/// * Stdlib instances inside a sub-model: `SMTH1(input, 2)` starts at its
///   input (3); `DELAY3(input, 2)` starts in equilibrium at its input (3);
///   `TREND(input, 2, 0.5)` starts at its initial trend (`stock = input / (1 +
///   2 * 0.5) = 1.5`, `output = (3 - 1.5) / (1.5 * 2) = 0.5`).
#[test]
fn stocks_initialized_from_module_ports_read_the_flat_model_values() {
    /// (what, main-model variables, extra models, expected t=0 values).
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        &'static [(&'static str, f64)],
    );
    let rows: [Row; 3] = [
        (
            "sub-model aux reading a sub-model stock",
            r#"<module name="sub"><connect to="sub.input" from="src"/></module>
               <stock name="level"><eqn>sub.out</eqn></stock>
               <aux name="init_out"><eqn>INIT(sub.out)</eqn></aux>"#,
            r#"<model name="sub"><variables>
                 <aux name="input" access="input"><eqn>0</eqn></aux>
                 <flow name="inflow"><eqn>input</eqn></flow>
                 <stock name="s"><eqn>input * 2</eqn><inflow>inflow</inflow></stock>
                 <aux name="out" access="output"><eqn>s * 10 + input</eqn></aux>
               </variables></model>"#,
            &[("level", 63.0), ("init_out", 63.0)],
        ),
        (
            "two instances with different input sets",
            r#"<aux name="bonus"><eqn>100</eqn></aux>
               <module name="sub1" simlin:model_name="sub"><connect to="sub1.input" from="src"/></module>
               <module name="sub2" simlin:model_name="sub">
                 <connect to="sub2.input" from="src"/><connect to="sub2.extra" from="bonus"/>
               </module>
               <stock name="level1"><eqn>sub1.output</eqn></stock>
               <stock name="level2"><eqn>sub2.output</eqn></stock>"#,
            r#"<model name="sub"><variables>
                 <aux name="input" access="input"><eqn>0</eqn></aux>
                 <aux name="extra" access="input"><eqn>1</eqn></aux>
                 <aux name="output" access="output"><eqn>input * 10 + extra</eqn></aux>
               </variables></model>"#,
            &[("level1", 31.0), ("level2", 130.0)],
        ),
        (
            "stdlib instances inside a sub-model",
            r#"<module name="sub"><connect to="sub.input" from="src"/></module>
               <stock name="level_s"><eqn>sub.smoothed</eqn></stock>
               <stock name="level_d"><eqn>sub.delayed</eqn></stock>
               <stock name="level_t"><eqn>sub.trended</eqn></stock>"#,
            r#"<model name="sub"><variables>
                 <aux name="input" access="input"><eqn>0</eqn></aux>
                 <aux name="smoothed" access="output"><eqn>SMTH1(input, 2)</eqn></aux>
                 <aux name="delayed" access="output"><eqn>DELAY3(input, 2)</eqn></aux>
                 <aux name="trended" access="output"><eqn>TREND(input, 2, 0.5)</eqn></aux>
               </variables></model>"#,
            &[("level_s", 3.0), ("level_d", 3.0), ("level_t", 0.5)],
        ),
    ];
    for (what, main_variables, extra_models, expected) in rows {
        let values = t0_values(&xmile_project(main_variables, extra_models));
        for (name, want) in expected.iter() {
            assert_eq!(values[*name], *want, "{what}: `{name}` at t=0");
        }
    }
}

/// A root model that some other model instantiates is a module target too,
/// so the flat rule seeds every value-bearing variable of it into its own
/// initials -- and that is only extra initial-phase work, never a different
/// number: the root simulates identically with and without the instantiating
/// model in the project.
#[test]
fn a_root_that_another_model_instantiates_simulates_as_it_does_alone() {
    let main_variables = r#"<aux name="prev_k"><eqn>PREVIOUS(src, -1)</eqn></aux>
        <aux name="init_k"><eqn>INIT(src)</eqn></aux>
        <flow name="inflow"><eqn>src</eqn></flow>
        <stock name="s"><eqn>10</eqn><inflow>inflow</inflow></stock>
        <aux name="reads_stock"><eqn>s * 2</eqn></aux>"#;
    let alone = xmile_project(main_variables, "");
    let instantiated = xmile_project(
        main_variables,
        r#"<model name="other"><variables>
             <module name="m" simlin:model_name="main"></module>
             <aux name="o"><eqn>m.src</eqn></aux>
           </variables></model>"#,
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &instantiated);
    let main = main_model(&db, sync.project);
    let initials = &model_dependency_graph(&db, main, sync.project, ModuleInputSet::empty(&db))
        .runlist_initials;
    assert!(
        initials.iter().any(|n| n == "reads_stock") && initials.iter().any(|n| n == "prev_k"),
        "an instantiated root's auxes are initials members; got {initials:?}"
    );
    let with = run_series(&db, sync.project);

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &alone);
    let main = main_model(&db, sync.project);
    let initials = &model_dependency_graph(&db, main, sync.project, ModuleInputSet::empty(&db))
        .runlist_initials;
    assert!(
        !initials.iter().any(|n| n == "reads_stock"),
        "a root nothing instantiates keeps only the seeds; got {initials:?}"
    );
    let alone = run_series(&db, sync.project);

    assert_eq!(
        with, alone,
        "the root's series must not depend on being a target"
    );
    assert_eq!(alone["reads_stock"], vec![20.0, 26.0, 34.0, 44.0]);
}
