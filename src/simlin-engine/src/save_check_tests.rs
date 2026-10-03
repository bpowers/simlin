// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What [`check_save`] says about saves that keep a model and saves that
//! change it, each change found the way it can only be found: by the
//! structure, by the run, or by the errors of a model that does not run,
//! and each said to change the results or only the definition.
//!
//! A test of what the check says compares two projects (`compare`: the
//! project and the project its save reads back as), so it holds whatever a
//! writer does. The tests that go through a writer are of what a FORMAT
//! cannot hold, which no writer changes.

use std::io::BufReader;

use super::{ChangeKind, MeaningChange, SaveFormat, check_save, compare};
use crate::datamodel::{Equation, Project, Variable};
use crate::test_common::TestProject;

#[path = "save_check_field_tests.rs"]
mod fields;

const CONTROL: &str = "
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
TIME STEP = 1 ~~|
SAVEPER = TIME STEP ~~|
";

fn mdl(source: &str) -> Project {
    crate::compat::open_vensim(source).expect("the model reads")
}

fn corpus_file(path: &str) -> Project {
    let bytes = std::fs::read(format!("../../{path}")).expect("the corpus file reads");
    if path.ends_with(".mdl") {
        crate::compat::open_vensim(std::str::from_utf8(&bytes).unwrap()).expect("it opens")
    } else {
        crate::compat::open_xmile(&mut BufReader::new(bytes.as_slice())).expect("it opens")
    }
}

/// A project from XMILE `variables`, simulated from 0 to `stop` by 1.
fn xmile(variables: &str, stop: f64) -> Project {
    xmile_with(
        variables,
        &format!("<start>0</start><stop>{stop}</stop><dt>1</dt>"),
    )
}

fn xmile_with(variables: &str, specs: &str) -> Project {
    let text = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0" xmlns:isee="http://iseesystems.com/XMILE">
<header><name>t</name><vendor>v</vendor><product version="1">p</product></header>
<sim_specs>{specs}</sim_specs>
<model><variables>{variables}</variables></model></xmile>"#
    );
    crate::compat::open_xmile(&mut BufReader::new(text.as_bytes())).expect("the model reads")
}

fn reasons(changes: &[MeaningChange]) -> Vec<&str> {
    changes.iter().map(|c| c.reason.as_str()).collect()
}

/// What the structure alone says of `saved` as a save of `original`.
fn structure(original: &Project, saved: &Project) -> Vec<String> {
    super::compare_structure(&super::Meaning::of(original), &super::Meaning::of(saved))
        .into_iter()
        .map(|found| found.reason)
        .collect()
}

/// The change about `variable`, which there must be.
fn about<'a>(changes: &'a [MeaningChange], variable: &str) -> &'a MeaningChange {
    changes
        .iter()
        .find(|c| c.variable.as_deref() == Some(variable))
        .unwrap_or_else(|| panic!("no change about '{variable}': {changes:?}"))
}

/// `project` with the variable `name` edited.
fn with_variable(project: &Project, name: &str, edit: impl FnOnce(&mut Variable)) -> Project {
    let mut project = project.clone();
    let var = project.models[0]
        .get_variable_mut(name)
        .unwrap_or_else(|| panic!("no variable '{name}'"));
    edit(var);
    project
}

#[test]
fn a_save_that_keeps_the_model_reports_nothing() {
    for path in [
        "test/test-models/samples/teacup/teacup.mdl",
        "test/test-models/samples/teacup/teacup.xmile",
    ] {
        let project = corpus_file(path);
        for format in [
            SaveFormat::Mdl,
            SaveFormat::Xmile,
            SaveFormat::Json,
            SaveFormat::Protobuf,
        ] {
            let changes = check_save(&project, format).expect("the format holds the model");
            assert!(changes.is_empty(), "{path} as {format:?}: {changes:?}");
        }
    }
}

/// A variable over another dimension of the same elements is another
/// variable: the structure names it, and the run says nothing more about it
/// than that its results change where the elements do.
#[test]
fn a_variable_over_another_dimension_is_named() {
    let over = |dim: &str| {
        TestProject::new("p")
            .named_dimension("dim", &["a", "b", "c"])
            .named_dimension("dim2", &["a", "b", "c", "d", "e"])
            .array_aux_direct("demands", vec![dim.to_string()], "10", None)
            .aux("total", "SUM(demands[*])", None)
            .build_datamodel()
    };
    let changes = compare(&over("dim"), &over("dim2"));
    assert_eq!(
        reasons(&changes),
        [
            "'demands' is defined over dim2, not dim",
            "'demands' gains elements 'd' and 'e'",
            "'total' simulates differently: 50 where it was 30 at time 0",
        ],
        "{changes:?}"
    );
    assert_eq!(changes[0].variable.as_deref(), Some("demands"));
    assert_eq!(changes[0].model.as_deref(), Some("main"));
    // The save simulates series for d and e, which the model does not.
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Results));
}

#[test]
fn a_variable_of_another_kind_is_named() {
    let flow = TestProject::new("p")
        .stock("s", "0", &["f"], &[], None)
        .flow("f", "1", None)
        .build_datamodel();
    let aux = TestProject::new("p")
        .stock("s", "0", &[], &[], None)
        .aux("f", "1", None)
        .build_datamodel();
    let changes = compare(&flow, &aux);
    let reasons = reasons(&changes);
    for reason in [
        "'f' is an auxiliary in the save, not a flow",
        "'s' loses inflow 'f'",
    ] {
        assert!(reasons.contains(&reason), "{reason}: {changes:?}");
    }
}

/// A table that leaves out its x points is the table that writes out the x
/// points the compiler spreads over its scale, a lone point at its start
/// included. A y point that is not a number, in both, is the same.
#[test]
fn tables_are_compared_as_the_compiler_reads_them() {
    use crate::datamodel::{GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale};
    let scale = |min, max| GraphicalFunctionScale { min, max };
    let implied = |y: Vec<f64>| GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: None,
        y_points: y,
        x_scale: scale(0.0, 4.0),
        y_scale: scale(0.0, 10.0),
    };
    let written = |gf: &GraphicalFunction, x: Vec<f64>| GraphicalFunction {
        x_points: Some(x),
        ..gf.clone()
    };
    let table = super::Table::of;

    let three = implied(vec![1.0, 2.0, 3.0]);
    assert!(table(&three) == table(&written(&three, vec![0.0, 2.0, 4.0])));
    assert!(table(&three) != table(&written(&three, vec![0.0, 1.0, 4.0])));

    let lone = implied(vec![7.0]);
    assert!(table(&lone) == table(&written(&lone, vec![0.0])));
    assert!(table(&lone) != table(&implied(vec![6.0])));

    let nan = implied(vec![f64::NAN, 1.0]);
    assert!(table(&nan) == table(&nan.clone()));
}

/// A stock that drains by `outflow` a step from `start`, the stock and its
/// outflow non-negative when `marked`, beside `other`.
fn draining(start: f64, outflow: f64, marked: bool, other: &str) -> Project {
    let mut project = TestProject::new("p")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("s", &start.to_string(), &[], &["drain"], None)
        .flow("drain", &outflow.to_string(), None)
        .aux("other", other, None)
        .build_datamodel();
    for name in ["s", "drain"] {
        project = with_variable(&project, name, |var| match var {
            Variable::Stock(s) => s.compat.non_negative = marked,
            Variable::Flow(f) => f.compat.non_negative = marked,
            _ => unreachable!("the model holds a stock and a flow"),
        });
    }
    project
}

/// The engine does not enforce a non-negative marking, so no run of it shows
/// whether a marking only one side has binds: the two runs are the same
/// whether the marked stock is drained below zero or never comes near it. A
/// simulator that enforces the marking computes other results wherever it
/// binds, so the change is one to the results in both, for a marking the
/// save loses and for one it gains. Only where nothing simulates, and there
/// are no results, is it a change to the definition alone.
#[test]
fn a_marking_the_engine_does_not_enforce_is_a_change_to_the_results() {
    // (the stock's start, its outflow, another equation, the grade)
    let rows = [
        (100.0, 20.0, "1", ChangeKind::Results),
        (10.0, 20.0, "1", ChangeKind::Results),
        (100.0, -5.0, "1", ChangeKind::Results),
        (10.0, 20.0, "nosuch + 1", ChangeKind::Structure),
    ];
    for (start, outflow, other, kind) in rows {
        let marked = draining(start, outflow, true, other);
        let unmarked = draining(start, outflow, false, other);
        for (original, saved, verb) in [
            (&marked, &unmarked, "is no longer"),
            (&unmarked, &marked, "becomes"),
        ] {
            let changes = compare(original, saved);
            assert_eq!(
                reasons(&changes),
                [
                    format!("'drain' {verb} non-negative"),
                    format!("'s' {verb} non-negative"),
                ],
                "no series differs: {changes:?}"
            );
            assert!(changes.iter().all(|c| c.kind == kind), "{changes:?}");
        }
    }
}

/// Teacup's stock and flow are non-negative in Stella's file; MDL cannot say
/// so (the MDL writer warns of it: Vensim has no such marking, a claim
/// unverified against Vensim's documentation).
#[test]
fn a_marking_a_format_cannot_hold_is_named() {
    let project = corpus_file("test/test-models/samples/teacup/teacup.stmx");
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert_eq!(
        reasons(&changes),
        [
            "'heat loss to room' is no longer non-negative",
            "'teacup temperature' is no longer non-negative",
        ]
    );
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Results));
}

/// QUANTUM truncates toward zero where INT floors, so `q * INT(x / q)` and
/// `QUANTUM(x, q)` are two equations: the structure names the change on every
/// variable, and the results differ only where x is negative.
#[test]
fn a_respelled_function_is_named_by_its_equation() {
    let model = |spelling: &dyn Fn(&str) -> String| {
        TestProject::new("p")
            .aux("a", &spelling("1.9"), None)
            .aux("c", &spelling("-0.9"), None)
            .build_datamodel()
    };
    let changes = compare(
        &model(&|x| format!("1 * INT({x} / 1)")),
        &model(&|x| format!("QUANTUM({x}, 1)")),
    );
    let c = about(&changes, "c");
    assert_eq!(
        c.reason,
        "'c' is computed as 'quantum(-0.9, 1)' in the save, not as '1 * int(-0.9 / 1)'"
    );
    assert_eq!(c.kind, ChangeKind::Results);
    assert_eq!(about(&changes, "a").kind, ChangeKind::Structure);
}

/// A pulse after FINAL TIME never fires, so the run cannot tell one pulse
/// from another. The equation can.
#[test]
fn an_equation_the_run_does_not_reach_is_named() {
    let model = |pulse: &str| {
        TestProject::new("p")
            .with_sim_time(0.0, 5.0, 1.0)
            .aux("p", pulse, None)
            .stock("s", "0", &["f"], &[], None)
            .flow("f", "p", None)
            .build_datamodel()
    };
    let changes = compare(&model("PULSE(20, 20, 0)"), &model("PULSE(20, 30, 0)"));
    assert_eq!(
        reasons(&changes),
        ["'p' is computed as 'pulse(20, 30, 0)' in the save, not as 'pulse(20, 20, 0)'"],
        "{changes:?}"
    );
    assert_eq!(changes[0].kind, ChangeKind::Structure);
}

/// A model with an unfinished equation does not simulate, and its other
/// equations are still compared.
#[test]
fn a_model_that_does_not_simulate_has_its_equations_compared() {
    let model = |pulse: &str| {
        TestProject::new("p")
            .aux("p", pulse, None)
            .aux("unfinished", "nosuch * 2", None)
            .build_datamodel()
    };
    let changes = compare(&model("PULSE(1, 1, 1)"), &model("PULSE(1, 2, 1)"));
    let p = about(&changes, "p");
    assert!(p.reason.ends_with(", not as 'pulse(1, 1, 1)'"), "{p:?}");
    assert_eq!(p.kind, ChangeKind::Structure);
}

/// Where the structure names nothing, the run still names a variable whose
/// series differ, by its first difference.
#[test]
fn the_run_names_what_the_structure_does_not() {
    let (a, b) = (
        mdl(&format!("x = 1 ~~|\n{CONTROL}")),
        mdl(&format!("x = 2 ~~|\n{CONTROL}")),
    );
    let specs = super::Meaning::of(&a).main_specs;
    let runs = super::compare_runs(&a, &b, &specs, &specs);
    assert_eq!(
        super::settle(&a, &b, Vec::new(), runs),
        [MeaningChange {
            model: Some("main".to_string()),
            variable: Some("x".to_string()),
            kind: ChangeKind::Results,
            reason: "'x' simulates differently: 2 where it was 1 at time 0".to_string(),
        }]
    );
}

/// -0 is 0, and NaN is NaN.
#[test]
fn values_are_compared_as_numbers() {
    assert!(super::same_value(-0.0, 0.0));
    assert!(super::same_value(f64::NAN, f64::NAN));
    assert!(!super::same_value(1.0, 1.0 + f64::EPSILON));
    let (a, b) = (
        mdl(&format!("x = 0 * -1 ~~|\n{CONTROL}")),
        mdl(&format!("x = 0 ~~|\n{CONTROL}")),
    );
    let specs = super::Meaning::of(&a).main_specs;
    let runs = super::compare_runs(&a, &b, &specs, &specs);
    assert!(super::settle(&a, &b, Vec::new(), runs).is_empty());
}

/// Two runs over other specs differ in every series, so other specs are a
/// change to the results whether or not the structure named how they differ.
#[test]
fn other_specs_are_a_change_whatever_the_structure_named() {
    let over = |stop: f64, dt: f64| {
        TestProject::new("p")
            .with_sim_time(0.0, stop, dt)
            .aux("x", "TIME", None)
            .build_datamodel()
    };
    for (a, b, expected) in [
        (
            over(4.0, 1.0),
            over(8.0, 1.0),
            vec!["the save simulates from 0 to 8, not from 0 to 4"],
        ),
        (
            over(4.0, 1.0),
            over(4.0, 0.5),
            vec![
                "the save's time step is 0.5, not 1",
                "the save's save step is 0.5, not 1",
            ],
        ),
    ] {
        let (before, after) = (super::Meaning::of(&a), super::Meaning::of(&b));
        let runs = super::compare_runs(&a, &b, &before.main_specs, &after.main_specs);
        let unnamed = super::settle(&a, &b, Vec::new(), runs);
        assert_eq!(reasons(&unnamed), expected);
        assert!(unnamed.iter().all(|c| c.kind == ChangeKind::Results));
        // Named by the structure, each difference is said once.
        let changes = compare(&a, &b);
        assert_eq!(reasons(&changes), expected);
        assert!(changes.iter().all(|c| c.kind == ChangeKind::Results));
    }
}

/// Two equations are the same when their canonical forms are
/// (`ast::CanonicalEqn`, whose own tests hold the form): spellings of a name
/// or a number, and the engine's rewrites, except a call a macro of the
/// project takes.
#[test]
fn equations_are_compared_in_their_canonical_form() {
    let model = |equation: &str| {
        TestProject::new("p")
            .aux("heat loss", "7", None)
            .aux("x", equation, None)
            .build_datamodel()
    };
    for (a, b) in [
        ("\"Heat Loss\" * 6e+05", "heat_loss * 600000"),
        ("MODULO(heat_loss, 3)", "heat_loss mod 3"),
        ("PI() * 2", "3.141592653589793 * 2"),
    ] {
        let changes = compare(&model(a), &model(b));
        assert!(changes.is_empty(), "{a} and {b}: {changes:?}");
    }
    let changes = compare(&model("heat_loss + 1"), &model("1 + heat_loss"));
    assert_eq!(
        reasons(&changes),
        ["'x' is computed as '1 + heat_loss' in the save, not as 'heat_loss + 1'"]
    );

    // A macro named for the call is what the call expands to.
    let with_macro = |equation: &str| {
        let mut project = model(equation);
        let mut body = project.models[0].clone();
        body.name = "modulo".to_string();
        body.macro_spec = Some(crate::datamodel::MacroSpec {
            parameters: vec!["heat loss".to_string(), "x".to_string()],
            primary_output: "modulo".to_string(),
            additional_outputs: Vec::new(),
        });
        project.models.push(body);
        project
    };
    let named = structure(
        &with_macro("MODULO(heat_loss, 3)"),
        &with_macro("heat_loss mod 3"),
    );
    assert!(
        named
            .iter()
            .any(|r| r.starts_with("'x' is computed as 'heat_loss mod 3' in the save")),
        "{named:?}"
    );
}

/// An arrayed variable over `d` (a, b) whose elements hold `equations`, each
/// beside a table when `tables`, with `default` as its `:EXCEPT:` default,
/// applied or not.
fn arrayed(equations: [&str; 2], tables: bool, default: Option<(&str, bool)>) -> Project {
    use crate::datamodel::{GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale};
    let table = || {
        tables.then(|| GraphicalFunction {
            kind: GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 10.0]),
            y_points: vec![5.0, 15.0],
            x_scale: GraphicalFunctionScale {
                min: 0.0,
                max: 10.0,
            },
            y_scale: GraphicalFunctionScale {
                min: 0.0,
                max: 20.0,
            },
        })
    };
    let project = TestProject::new("p")
        .named_dimension("d", &["a", "b"])
        .array_with_ranges_direct("t", vec!["d".to_string()], vec![("a", ""), ("b", "")], None)
        .aux("reader", "LOOKUP(t[a], TIME)", None)
        .build_datamodel();
    with_variable(&project, "t", |var| {
        let Variable::Aux(aux) = var else {
            unreachable!("t is an auxiliary");
        };
        aux.equation = Equation::Arrayed(
            vec!["d".to_string()],
            vec![
                ("a".to_string(), equations[0].to_string(), None, table()),
                ("b".to_string(), equations[1].to_string(), None, table()),
            ],
            default.map(|(text, _)| text.to_string()),
            default.is_some_and(|(_, applies)| applies),
        );
    })
}

/// A variable is a table alone by the compiler's own rule
/// (`variable::is_lookup_only`), which reads every arm and the default,
/// applied or not: an empty equation and the MDL sentinel `0+0` both say so,
/// and a default with an equation, even one that fills no element, says it is
/// computed.
#[test]
fn a_table_alone_is_one_however_it_is_written() {
    let alone = arrayed(["", ""], true, None);
    assert!(compare(&alone, &arrayed(["0+0", "0+0"], true, None)).is_empty());
    assert!(compare(&alone, &arrayed(["", "0+0"], true, Some(("", false)))).is_empty());

    let changes = compare(&alone, &arrayed(["", ""], true, Some(("12345", false))));
    assert_eq!(
        reasons(&changes)[0],
        "'t' is computed in the save, where it is a table alone",
        "{changes:?}"
    );
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Results));

    // Beside a real equation, `0+0` is the number it says.
    let named = structure(
        &arrayed(["", "TIME"], true, None),
        &arrayed(["0+0", "TIME"], true, None),
    );
    assert_eq!(
        named,
        ["'t' is computed as '0 + 0' for element 'a' in the save, not as ''"]
    );
}

/// An `:EXCEPT:` default the compiler applies is part of the definition even
/// where every element is written out: a dimension that gained an element
/// would take it. A default it does not apply defines no element.
#[test]
fn an_except_default_the_compiler_applies_is_part_of_the_definition() {
    let applied = arrayed(["1", "2"], false, Some(("9", true)));
    let changes = compare(&applied, &arrayed(["1", "2"], false, None));
    assert_eq!(
        reasons(&changes),
        ["'t' no longer has its :EXCEPT: default '9'"]
    );
    assert_eq!(changes[0].kind, ChangeKind::Structure);
    assert_eq!(
        structure(&applied, &arrayed(["1", "2"], false, Some(("8", true)))),
        ["the :EXCEPT: default of 't' is '8' in the save, not '9'"]
    );

    let unapplied = arrayed(["1", "2"], false, Some(("9", false)));
    assert!(compare(&unapplied, &arrayed(["1", "2"], false, None)).is_empty());
}

/// A dimension's parent, whether its elements are numbered or named, and a
/// model's own specs where the other side has none, are part of the
/// definition.
#[test]
fn dimension_parents_and_a_models_own_specs_are_compared() {
    let with_parent = TestProject::new("p")
        .indexed_dimension("full", 5)
        .indexed_subdimension("sub", 3, "full")
        .build_datamodel();
    let without = TestProject::new("p")
        .indexed_dimension("full", 5)
        .indexed_dimension("sub", 3)
        .build_datamodel();
    assert_eq!(
        structure(&with_parent, &without),
        ["dimension 'sub' is no longer a subdimension of 'full'"]
    );

    let named = TestProject::new("p")
        .indexed_dimension("full", 5)
        .named_dimension("sub", &["1", "2", "3"])
        .build_datamodel();
    assert_eq!(
        structure(&without, &named),
        ["dimension 'sub' has named elements in the save, not numbered ones"]
    );

    let project = TestProject::new("p").aux("x", "1", None).build_datamodel();
    let mut own = project.clone();
    own.models[0].sim_specs = Some(crate::datamodel::SimSpecs {
        stop: project.sim_specs.stop * 2.0,
        ..project.sim_specs.clone()
    });
    assert_eq!(
        structure(&own, &project),
        ["the save simulates from 0 to 1, not from 0 to 2"]
    );
}

/// A queue serves its outflows, and a conveyor admits its inflows, in their
/// order, so the same flows in another order are a change. A flow listed
/// twice counts once, as the engine counts it.
#[test]
fn a_stocks_flows_are_compared_in_order() {
    let stock = |outflows: &[&str]| {
        TestProject::new("p")
            .stock("s", "10", &[], outflows, None)
            .flow("a", "1", None)
            .flow("b", "2", None)
            .aux("bad", "1 +", None)
            .build_datamodel()
    };
    let changes = compare(&stock(&["a", "b"]), &stock(&["b", "a"]));
    assert_eq!(
        reasons(&changes),
        ["'s' takes its outflows in another order in the save: b, a, not a, b"]
    );
    assert_eq!(changes[0].kind, ChangeKind::Structure);
    assert!(compare(&stock(&["a", "b", "a"]), &stock(&["a", "b"])).is_empty());
}

/// A lookup-only table has no series of its own: a change to it shows in
/// the variables that read it, so it is a change to the results when any
/// series differs.
#[test]
fn a_change_to_a_variable_with_no_series_is_graded_by_the_whole_run() {
    let table = |y: &str| {
        xmile(
            &format!(
                r#"<aux name="t"><gf><xscale min="0" max="10"/><yscale min="0" max="100"/><ypts>{y}</ypts></gf></aux>
<aux name="w"><eqn>LOOKUP(t, time)</eqn></aux>"#
            ),
            3.0,
        )
    };
    let changes = compare(&table("0,10"), &table("0,20"));
    assert_eq!(
        reasons(&changes),
        [
            "the graphical function of 't' changes",
            "'w' simulates differently: 2 where it was 1 at time 1",
        ],
        "{changes:?}"
    );
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Results));
}

/// Variables pair by the engine's canonical ident, and nothing looser:
/// `a__b` and `a_b` are two variables.
#[test]
fn variables_pair_by_their_canonical_ident() {
    let a = TestProject::new("p")
        .aux("a__b", "1", None)
        .build_datamodel();
    let b = TestProject::new("p")
        .aux("a_b", "1", None)
        .build_datamodel();
    assert_eq!(
        structure(&a, &b),
        ["'a__b' is not in the save", "'a_b' is new in the save"]
    );
}

/// MDL has no conveyor (Vensim has no such stock; what the MDL writer warns
/// of), so the belt reads back as a plain stock and its outflow, which the
/// belt computes, with no equation. The compiler's helpers for the belt and
/// its leaks are not the model's variables: the changes are named on the
/// belt, its leaks and the outflow.
#[test]
fn a_change_is_named_on_the_models_variables_not_the_compilers_helpers() {
    let project = corpus_file("test/conveyors/leaky_conveyor.xmile");
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    let reasons = reasons(&changes);
    assert!(
        reasons.contains(&"'belt' is no longer a conveyor"),
        "{changes:?}"
    );
    assert!(
        reasons.contains(&"'seepage' is no longer a conveyor leak"),
        "{changes:?}"
    );
    assert!(
        changes.iter().any(|c| c.kind == ChangeKind::Results),
        "{changes:?}"
    );
    for change in &changes {
        assert!(!change.reason.contains('$'), "{changes:?}");
        assert!(
            !change.variable.as_deref().unwrap_or("").starts_with('$'),
            "{changes:?}"
        );
    }
}

/// A model can hold its specs as variables (`SAVEPER = TIME STEP`, as a
/// converted Vensim model does). A save that keeps them as the specs, with
/// the same values, changes nothing; with another value, it does.
#[test]
fn control_variables_kept_as_the_specs_are_no_change() {
    let specs = |stop: f64| {
        TestProject::new("p")
            .with_sim_time(-2.0, stop, 0.5)
            .aux("x", "TIME", None)
            .build_datamodel()
    };
    let as_variables = |final_time: &str, saveper: &str| {
        let mut project = specs(10.0);
        for (name, equation) in [
            ("INITIAL TIME", "-2"),
            ("FINAL TIME", final_time),
            ("TIME STEP", "0.5"),
            ("SAVEPER", saveper),
        ] {
            let mut var = project.models[0]
                .get_variable("x")
                .expect("x is in the model")
                .clone();
            var.set_ident(name.to_string());
            var.set_scalar_equation(equation);
            project.models[0].variables.rewrite(|vars| vars.push(var));
        }
        project
    };
    for saveper in ["TIME_STEP", "DT", "0.5", "5e-1"] {
        let model = as_variables("10", saveper);
        assert!(structure(&model, &specs(10.0)).is_empty(), "{saveper}");
        assert!(structure(&specs(10.0), &model).is_empty(), "{saveper}");
    }
    assert_eq!(
        structure(&as_variables("10", "2"), &specs(10.0)),
        ["'SAVEPER' is not in the save"]
    );
    assert_eq!(
        structure(&as_variables("x * 2", "DT"), &specs(10.0)),
        ["'FINAL TIME' is not in the save"]
    );
    // The specs themselves are compared, so a save whose specs hold another
    // value than the model's are a change of the specs.
    assert_eq!(
        structure(&as_variables("10", "DT"), &specs(12.0)),
        [
            "the save simulates from -2 to 12, not from -2 to 10",
            "'FINAL TIME' is not in the save"
        ]
    );

    // Through a writer: MDL keeps a converted model's control variables as
    // its specs.
    let project = corpus_file("test/test-models/tests/builtin_max/builtin_max.stmx");
    assert!(
        project.models[0]
            .variables
            .iter()
            .any(|v| v.get_ident() == "SAVEPER"),
        "the file holds its save step as a variable"
    );
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert!(changes.is_empty(), "{changes:?}");
}

/// A model that does not simulate keeps its meaning when its save has the
/// same errors.
#[test]
fn a_model_that_does_not_simulate_keeps_its_meaning_with_the_same_errors() {
    let project = mdl(&format!("x = y + 1 ~~|\n{CONTROL}"));
    for format in [SaveFormat::Mdl, SaveFormat::Xmile, SaveFormat::Json] {
        let changes = check_save(&project, format).unwrap();
        assert!(changes.is_empty(), "{format:?}: {changes:?}");
    }
}

/// A model that does not simulate changes when its save's errors differ.
/// Here the stock's flow is over more elements than the stock in the save,
/// and the error both have is no change.
#[test]
fn a_model_that_does_not_simulate_changes_when_its_errors_do() {
    let over = |dim: &str| {
        TestProject::new("p")
            .named_dimension("dim", &["a", "b", "c"])
            .named_dimension("dim2", &["a", "b", "c", "d", "e"])
            .array_flow(&format!("demands[{dim}]"), "10", None)
            .array_stock("stock[dim]", "0", &["demands"], &[], None)
            .aux("x", "y + 1", None)
            .build_datamodel()
    };
    let changes = compare(&over("dim"), &over("dim2"));
    assert_eq!(
        reasons(&changes),
        [
            "'demands' is defined over dim2, not dim",
            "'demands' gains elements 'd' and 'e'",
            "'stock' has an error in the save that the model does not: \
             mismatched_dimensions -- the dimensions of 'demands' do not match \
             the dimensions of the equation that uses it",
        ],
        "{changes:?}"
    );
    assert_eq!(changes[2].variable.as_deref(), Some("stock"));
    // Neither simulates, so no results change.
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Structure));
}

#[test]
fn a_format_that_cannot_hold_the_model_refuses() {
    let project = corpus_file("test/modules_hares_and_foxes/modules_hares_and_foxes.stmx");
    assert!(
        check_save(&project, SaveFormat::Mdl).is_err(),
        "MDL holds one model"
    );
    assert!(check_save(&project, SaveFormat::Xmile).unwrap().is_empty());
}

/// sd-ai JSON holds one model's variables, specs and diagram, and nothing of
/// its dimensions.
#[test]
fn what_a_format_leaves_out_is_named() {
    let project = mdl(&format!(
        "Region: north, south ~~|
population[Region] = 10, 20 ~~|
{CONTROL}"
    ));
    let changes = check_save(&project, SaveFormat::SdaiJson).unwrap();
    assert!(
        reasons(&changes).contains(&"dimension 'Region' is not in the save"),
        "{changes:?}"
    );
}

/// The run names a variable whose series differ whatever its name holds: a
/// `$`, a `[`, or (as an arrayed variable's columns do) an element key after
/// a name that holds one. Where the structure names nothing, only the run
/// can say the save changes the model.
#[test]
fn the_run_names_a_variable_whatever_its_name_holds() {
    let model = |name: &str, arrayed: bool, value: &str| {
        let over = if arrayed {
            r#"<dimensions><dim name="d"/></dimensions>"#
        } else {
            ""
        };
        let text = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
<header><name>t</name><vendor>v</vendor><product version="1">p</product></header>
<sim_specs><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
<dimensions><dim name="d"><elem name="e1"/><elem name="e2"/></dim></dimensions>
<model><variables><aux name="{name}">{over}<eqn>{value}</eqn></aux><aux name="other"><eqn>5</eqn></aux></variables></model></xmile>"#
        );
        crate::compat::open_xmile(&mut BufReader::new(text.as_bytes())).expect("the model reads")
    };
    // (the name as written, whether it is arrayed, its canonical ident)
    let rows = [
        ("plain", false, "plain"),
        ("cost ($)", false, "cost_($)"),
        ("$x", false, "$x"),
        ("x [usd]", false, "x_[usd]"),
        ("a[b]", false, "a[b]"),
        ("a[b]", true, "a[b]"),
    ];
    for (name, arrayed, ident) in rows {
        let (a, b) = (model(name, arrayed, "1"), model(name, arrayed, "2"));
        let specs = super::Meaning::of(&a).main_specs;
        let runs = super::compare_runs(&a, &b, &specs, &specs);
        let changes = super::settle(&a, &b, Vec::new(), runs);
        assert_eq!(changes.len(), 1, "{name} (arrayed: {arrayed}): {changes:?}");
        assert_eq!(changes[0].variable.as_deref(), Some(ident), "{changes:?}");
        assert_eq!(changes[0].kind, ChangeKind::Results);
    }
}

/// Which variable a results column belongs to: the declared name the whole
/// column is, else the longest declared name it extends with an element key
/// or a sub-model path; a column the compiler adds for itself (its names
/// carry `ltm::SYNTHETIC_NODE_PREFIX`) and the clock belong to none, even
/// beside a quoted name that spells a helper's.
#[test]
fn a_results_column_belongs_to_the_variable_it_extends() {
    // A quoted name may spell anything, a helper's name included.
    let helper_spelled = "$\u{205a}y\u{205a}0\u{205a}smth1";
    let declared: std::collections::BTreeSet<String> =
        ["x", "$x", "a[b]", "a", "hares", "x_[usd]", helper_spelled]
            .into_iter()
            .map(String::from)
            .collect();
    for (column, owner) in [
        ("x", Some("x")),
        ("x[e1,e2]", Some("x")),
        ("$x", Some("$x")),
        ("$x[e1]", Some("$x")),
        ("x_[usd]", Some("x_[usd]")),
        ("x_[usd][e1]", Some("x_[usd]")),
        ("a[b]", Some("a[b]")),
        ("a[b][e1]", Some("a[b]")),
        ("a[c]", Some("a")),
        ("hares\u{b7}births", Some("hares")),
        ("hares\u{b7}births[e1]", Some("hares")),
        ("$\u{205a}x\u{205a}0\u{205a}smth1\u{b7}output", None),
        ("$\u{205a}x\u{205a}0\u{205a}arg0[e1]", None),
        (helper_spelled, Some(helper_spelled)),
        ("$\u{205a}y\u{205a}0\u{205a}smth1\u{b7}output", None),
        ("$conv$len$belt", None),
        ("time", None),
        ("nowhere", None),
        ("nowhere[x]", None),
    ] {
        assert_eq!(super::column_variable(column, &declared), owner, "{column}");
    }
}

/// A model may declare a variable named `time`, whose series then has the
/// results' `time` key: a difference is still placed at the clock's time.
#[test]
fn a_difference_is_placed_at_the_clocks_time_beside_a_variable_named_time() {
    let model = |value: &str| {
        xmile(
            &format!(
                r#"<aux name="time"><eqn>42</eqn></aux><aux name="x"><eqn>IF TIME &gt; 1 THEN {value} ELSE 0</eqn></aux>"#
            ),
            3.0,
        )
    };
    let (a, b) = (model("1"), model("2"));
    let specs = super::Meaning::of(&a).main_specs;
    let runs = super::compare_runs(&a, &b, &specs, &specs);
    assert_eq!(
        reasons(&super::settle(&a, &b, Vec::new(), runs)),
        ["'x' simulates differently: 2 where it was 1 at time 2"]
    );
}

/// A positional mapping relates the elements pair by pair, in order, so it
/// is the explicit mapping of those pairs and not the mapping of others.
#[test]
fn a_positional_mapping_is_the_pairs_it_makes() {
    let mapped = |pairs: &[(&str, &str)]| {
        let project = TestProject::new("p").named_dimension("b", &["b1", "b2"]);
        let project = if pairs.is_empty() {
            project.named_dimension_with_mapping("a", &["a1", "a2"], "b")
        } else {
            project.named_dimension_with_element_mapping("a", &["a1", "a2"], "b", pairs)
        };
        project.build_datamodel()
    };
    let positional = mapped(&[]);
    assert!(structure(&positional, &mapped(&[("a1", "b1"), ("a2", "b2")])).is_empty());
    assert_eq!(
        structure(&positional, &mapped(&[("a1", "b2"), ("a2", "b1")])),
        ["the mapping of dimension 'a' to 'b' relates other elements in the save"]
    );
}

/// An equation that does not parse is compared as its text, so another
/// such equation is a change, and the same one with other spacing or case
/// is none.
#[test]
fn an_equation_that_does_not_parse_is_compared_as_its_text() {
    let model = |equation: &str| {
        TestProject::new("p")
            .aux("x", equation, None)
            .build_datamodel()
    };
    assert_eq!(
        structure(&model("1 +"), &model("2 +")),
        ["'x' is computed as '2 +' in the save, not as '1 +'"]
    );
    assert!(structure(&model("A +"), &model("  a +")).is_empty());
}

/// The model a host simulates is the same model in a save that names it
/// otherwise (MDL's one model is always `main`): its variables are compared,
/// and it is neither lost nor new.
#[test]
fn the_simulated_model_pairs_whatever_the_save_calls_it() {
    let project = TestProject::new("p").aux("x", "1", None).build_datamodel();
    let renamed = |equation: &str| {
        let mut saved = project.clone();
        saved.models[0].name = "elsewhere".to_string();
        with_variable(&saved, "x", |var| var.set_scalar_equation(equation))
    };
    assert!(compare(&project, &renamed("1")).is_empty());
    assert_eq!(
        structure(&project, &renamed("2")),
        ["'x' is computed as '2' in the save, not as '1'"]
    );
}

/// A variable that is a table alone on one side and computed on the other is
/// a change whichever side it is computed on.
#[test]
fn a_table_alone_is_named_on_either_side() {
    let alone = arrayed(["", ""], true, None);
    let computed = arrayed(["", ""], true, Some(("12345", false)));
    for (original, saved, reason) in [
        (
            &alone,
            &computed,
            "'t' is computed in the save, where it is a table alone",
        ),
        (
            &computed,
            &alone,
            "'t' is a table alone in the save, where it is computed",
        ),
    ] {
        let named = structure(original, saved);
        assert_eq!(named.first().map(String::as_str), Some(reason), "{named:?}");
    }
}

/// Only a scalar auxiliary can be a control variable a format keeps as its
/// specs: a stock, a flow or an arrayed variable of the name is a variable
/// like any other, which the save loses.
#[test]
fn a_control_variable_is_a_scalar_auxiliary() {
    let specs = || {
        TestProject::new("p")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "TIME", None)
    };
    let kept = specs().aux("FINAL TIME", "10", None).build_datamodel();
    assert!(structure(&kept, &specs().build_datamodel()).is_empty());
    for other in [
        specs().stock("FINAL TIME", "10", &[], &[], None),
        specs().flow("FINAL TIME", "10", None),
        specs()
            .indexed_dimension("d", 1)
            .array_aux("FINAL TIME[d]", "10"),
    ] {
        let mut without = other.build_datamodel();
        without.models[0]
            .variables
            .rewrite(|vars| vars.retain(|v| v.get_ident() != "FINAL TIME"));
        let named = structure(&other.build_datamodel(), &without);
        assert_eq!(named, ["'FINAL TIME' is not in the save"]);
    }
}
