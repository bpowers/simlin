// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What [`check_save`] says about saves that keep a model and saves that
//! change it, each change found the way it can only be found: by the
//! structure, by the run, or by the errors of a model that does not run,
//! and each said to change the results now or only the definition.

use std::collections::BTreeSet;
use std::io::BufReader;

use super::{ChangeKind, MeaningChange, SaveFormat, check_save};
use crate::datamodel::Project;

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
    let text = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0" xmlns:isee="http://iseesystems.com/XMILE">
<header><name>t</name><vendor>v</vendor><product version="1">p</product></header>
<sim_specs><start>0</start><stop>{stop}</stop><dt>1</dt></sim_specs>
<model><variables>{variables}</variables></model></xmile>"#
    );
    crate::compat::open_xmile(&mut BufReader::new(text.as_bytes())).expect("the model reads")
}

fn reasons(changes: &[MeaningChange]) -> Vec<&str> {
    changes.iter().map(|c| c.reason.as_str()).collect()
}

/// What the check says of `saved` as a save of `original`: the two compared
/// as a project and its save are, without a writer between them.
fn compare_pair(original: &Project, saved: &Project) -> Vec<MeaningChange> {
    let (a, b) = (super::Meaning::of(original), super::Meaning::of(saved));
    let changes = super::compare_structure(&a, &b);
    let runs = super::compare_runs(original, saved, a.main_specs != b.main_specs);
    super::settle(original, saved, changes, runs)
}

/// The change about `variable`, which there must be.
fn about<'a>(changes: &'a [MeaningChange], variable: &str) -> &'a MeaningChange {
    changes
        .iter()
        .find(|c| c.variable.as_deref() == Some(variable))
        .unwrap_or_else(|| panic!("no change about '{variable}': {changes:?}"))
}

#[test]
fn a_save_that_keeps_the_model_reports_nothing() {
    for path in [
        "test/test-models/samples/teacup/teacup.mdl",
        "test/test-models/samples/teacup/teacup.xmile",
        "test/metasd/WRLD3-03/wrld3-03.mdl",
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

/// A variable over a subrange, written one element equation each, reads
/// back over the elements' whole family: the structure names it, and the run
/// says nothing more about it than that its results change.
#[test]
fn a_variable_read_back_over_another_dimension_is_named() {
    let project = mdl(&format!(
        "dim: A, B, C ~~|
dim2: A, B, C, D, E ~~|
demands[dim] = 10, 6, 3 ~~|
total = SUM(demands[dim!]) ~~|
{CONTROL}"
    ));
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert_eq!(
        reasons(&changes),
        ["'demands' is defined over dim2, not dim"],
        "{changes:?}"
    );
    assert_eq!(changes[0].variable.as_deref(), Some("demands"));
    assert_eq!(changes[0].model.as_deref(), Some("main"));
    // The save simulates series for D and E, which the model does not.
    assert_eq!(changes[0].kind, ChangeKind::Results);
    // XMILE names the dimension.
    assert!(check_save(&project, SaveFormat::Xmile).unwrap().is_empty());
}

/// XMILE lets one flow drain two stocks. MDL writes each stock's INTEG
/// over the flow, which reads back as an auxiliary beside a net flow of
/// each stock's own.
#[test]
fn a_variable_read_back_as_another_kind_is_named() {
    let project =
        corpus_file("test/test-models/tests/non_negative_all/test_non_negative_all1.xmile");
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    let reasons = reasons(&changes);
    for reason in [
        "'OutFlow' is an auxiliary in the save, not a flow",
        "'TestStock0' loses outflow 'outflow'",
        "'TestStock0' gains inflow 'teststock0_net_flow'",
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

/// Teacup's stocks and flows are non-negative in Stella's file; MDL cannot
/// say so. Its run never goes below zero, so only the structure shows it.
#[test]
fn a_dropped_flag_is_named_where_the_run_does_not_show_it() {
    let project = corpus_file("test/test-models/samples/teacup/teacup.stmx");
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert_eq!(
        reasons(&changes),
        [
            "'heat loss to room' is no longer non-negative",
            "'teacup temperature' is no longer non-negative",
        ]
    );
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Structure));
}

/// This file writes QUANTUM out as `q * INT(x / q)`, which the MDL writer
/// writes back as QUANTUM: that truncates toward zero where INT floors. The
/// equation names the change on every variable; the results differ only
/// where x is negative.
#[test]
fn a_respelled_function_is_named_by_its_equation() {
    let project = corpus_file("test/sdeverywhere/models/quantum/quantum.xmile");
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    let c = about(&changes, "c");
    assert_eq!(
        c.reason,
        "'c' is computed as 'quantum(-0.9, 1)' in the save, not as '1 * int(-0.9 / 1)'"
    );
    assert_eq!(c.kind, ChangeKind::Results);
    assert_eq!(about(&changes, "a").kind, ChangeKind::Structure);
}

/// `PULSE(20, 20, 0)` fires after FINAL TIME, so the run cannot tell the
/// pulse MDL writes for it from the one it was. The equation can.
#[test]
fn an_equation_the_run_does_not_reach_is_named() {
    let project = xmile(
        r#"<aux name="p"><eqn>PULSE(20, 20, 0)</eqn></aux>
<stock name="s"><eqn>0</eqn><inflow>f</inflow></stock><flow name="f"><eqn>p</eqn></flow>"#,
        5.0,
    );
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(
        changes[0].reason.starts_with("'p' is computed as "),
        "{changes:?}"
    );
    assert!(changes[0].reason.ends_with(", not as 'pulse(20, 20, 0)'"));
    assert_eq!(changes[0].kind, ChangeKind::Structure);
}

/// A model with an unfinished equation does not simulate, and its other
/// equations are still compared.
#[test]
fn a_model_that_does_not_simulate_has_its_equations_compared() {
    let project = xmile(
        r#"<aux name="p"><eqn>PULSE(1, 1, 1)</eqn></aux>
<stock name="s"><eqn>0</eqn><inflow>f</inflow></stock><flow name="f"><eqn>p</eqn></flow>
<aux name="unfinished"><eqn>nosuch * 2</eqn></aux>"#,
        5.0,
    );
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
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
    let runs = super::compare_runs(&a, &b, false);
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
    assert!(super::settle(&a, &b, Vec::new(), super::compare_runs(&a, &b, false)).is_empty());
}

/// Only spellings that provably mean the same to the engine are the same
/// equation.
#[test]
fn spellings_that_mean_the_same_are_the_same_equation() {
    let none = BTreeSet::new();
    let canonical = |text: &str| super::canonical_equation(text, false, &none);
    for (a, b) in [
        ("600000", "6e+05"),
        (".12", "0.12"),
        ("0.0001", "1e-04"),
        ("PI()", "3.141592653589793"),
        ("SIN(PI())", "sin(3.141592653589793)"),
        ("MODULO(a, 3)", "a mod 3"),
        ("\"Heat Loss\" + 1", "heat_loss + 1"),
        ("Heat_Loss + 1", "heat_loss + 1"),
        ("x[A1] * 2", "x[a1] * 2"),
    ] {
        assert_eq!(canonical(a), canonical(b), "{a} and {b}");
    }
    // A literal too large for a number is infinite, as INF() is (and a
    // bare `inf`, which is the call); a quoted "inf" is a variable.
    assert_eq!(canonical("1e400"), canonical("INF()"));
    assert_eq!(canonical("1e400"), canonical("inf"));
    for (a, b) in [
        ("1 * INT(x / 1)", "QUANTUM(x, 1)"),
        ("a + b", "b + a"),
        ("0 + 0", ""),
        ("1e400", "\"inf\""),
        ("NaN", "\"nan\""),
    ] {
        assert_ne!(canonical(a), canonical(b), "{a} and {b}");
    }
    // Beside a table, an empty equation and the MDL sentinel both say the
    // variable is the table alone.
    assert_eq!(
        super::canonical_equation("", true, &none),
        super::canonical_equation("0+0", true, &none)
    );
    // A macro of the name is what the call would expand.
    let macros = BTreeSet::from(["modulo".to_string(), "inf".to_string(), "pi".to_string()]);
    for (a, b) in [
        ("MODULO(a, 3)", "a mod 3"),
        ("INF()", "1e400"),
        ("PI()", "3.141592653589793"),
    ] {
        assert_ne!(
            super::canonical_equation(a, false, &macros),
            super::canonical_equation(b, false, &macros),
            "{a} and {b} beside a macro of the name"
        );
    }
    // Inside a subscript the engine reads an index literal otherwise than a
    // call, so no call is folded there.
    assert_ne!(canonical("x[1e400]"), canonical("x[INF()]"));
    assert_ne!(canonical("x[3.141592653589793]"), canonical("x[PI()]"));
    assert_eq!(canonical("x[6e+05]"), canonical("x[600000]"));
}

/// test_except's `:EXCEPT:` default applies, though every element is
/// written out: a dimension that gained an element would take it. MDL
/// drops it. except.mdl's defaults never apply (each is its variable's only
/// equation), so XMILE dropping them changes nothing the compiler reads.
#[test]
fn an_except_default_the_compiler_applies_is_part_of_the_definition() {
    let project = corpus_file("test/test-models/tests/except/test_except.mdl");
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert_eq!(
        reasons(&changes),
        ["'inventory' no longer has its :EXCEPT: default 'init_inventory[store, product]'"]
    );
    assert_eq!(changes[0].kind, ChangeKind::Structure);

    let project = corpus_file("test/sdeverywhere/models/except/except.mdl");
    assert!(check_save(&project, SaveFormat::Xmile).unwrap().is_empty());
}

/// A dimension's parent, and a model's own specs where the other side has
/// none, are part of the definition.
#[test]
fn dimension_parents_and_a_models_own_specs_are_compared() {
    use crate::test_common::TestProject;
    let compare = |a: &Project, b: &Project| {
        super::compare_structure(&super::Meaning::of(a), &super::Meaning::of(b))
    };
    let with_parent = TestProject::new("p")
        .indexed_dimension("full", 5)
        .indexed_subdimension("sub", 3, "full")
        .build_datamodel();
    let without = TestProject::new("p")
        .indexed_dimension("full", 5)
        .indexed_dimension("sub", 3)
        .build_datamodel();
    assert_eq!(
        reasons(&compare(&with_parent, &without)),
        ["dimension 'sub' is no longer a subdimension of 'full'"]
    );

    let project = TestProject::new("p").aux("x", "1", None).build_datamodel();
    let mut own = project.clone();
    own.models[0].sim_specs = Some(crate::datamodel::SimSpecs {
        stop: project.sim_specs.stop * 2.0,
        ..project.sim_specs.clone()
    });
    let changes = compare(&own, &project);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0].model.as_deref(), Some("main"));
    assert!(changes[0].reason.starts_with("the save simulates from "));
}

/// A macro's signature is part of the definition: which argument binds to
/// which parameter, and what the call returns. With an unfinished equation
/// elsewhere, no run could show it.
#[test]
fn a_macros_signature_is_compared() {
    use crate::datamodel::MacroSpec;
    let mut a = xmile(r#"<aux name="bad"><eqn>1 +</eqn></aux>"#, 3.0);
    let mut body = a.models[0].clone();
    body.name = "double".to_string();
    body.macro_spec = Some(MacroSpec {
        parameters: vec!["input".to_string(), "parameter".to_string()],
        primary_output: "Double".to_string(),
        additional_outputs: Vec::new(),
    });
    a.models.push(body);

    let mut swapped = a.clone();
    let spec = swapped.models[1].macro_spec.as_mut().unwrap();
    spec.parameters.reverse();
    spec.primary_output = "other".to_string();
    spec.additional_outputs = vec!["rest".to_string()];
    let changes = compare_pair(&a, &swapped);
    assert_eq!(
        reasons(&changes),
        [
            "macro 'double' takes parameter, input in the save, not input, parameter",
            "macro 'double' returns 'other' in the save, not 'double'",
            "macro 'double' also returns rest in the save, not nothing",
        ]
    );
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Structure));

    let mut plain = a.clone();
    plain.models[1].macro_spec = None;
    assert_eq!(
        reasons(&compare_pair(&a, &plain)),
        ["model 'double' is no longer a macro"]
    );
}

/// A conveyor leak's zone, whether it leaks whole units and its fraction,
/// and an inflow's spread over its conveyor, are compared by value, in a
/// model that does not simulate as in one that does.
#[test]
fn a_leaks_zone_and_an_inflows_spread_are_compared() {
    let conveyor = |spread: &str, zone: (&str, &str), fraction: &str, integers: &str| {
        xmile(
            &format!(
                r#"<stock name="belt"><eqn>0</eqn><inflow>arriving</inflow><outflow>departing</outflow><outflow>spillage</outflow><conveyor><len>2</len></conveyor></stock>
<flow name="arriving" isee:spreadflow="{spread}"><eqn>100</eqn></flow>
<flow name="departing"></flow>
<flow name="spillage" leak_start="{}" leak_end="{}"><leak>{fraction}</leak>{integers}</flow>
<stock name="spilled"><eqn>0</eqn><inflow>spillage</inflow></stock>
<aux name="bad"><eqn>1 +</eqn></aux>"#,
                zone.0, zone.1
            ),
            3.0,
        )
    };
    let a = conveyor("even", ("0.5", "1"), "0.1", "");
    let b = conveyor("dest", ("0", "0.5"), "0.2", "<leak_integers/>");
    let changes = compare_pair(&a, &b);
    assert_eq!(
        reasons(&changes),
        [
            "how the inflow 'arriving' spreads over its conveyor changes",
            "the zone the leak 'spillage' drains changes",
            "the leak 'spillage' drains whole units in the save, not any amount",
            "the fraction the leak 'spillage' drains changes",
        ],
        "{changes:?}"
    );
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Structure));
}

/// A queue serves its outflows, and a conveyor admits its inflows, in their
/// order, so the same flows in another order are a change. A flow listed
/// twice counts once, as the engine counts it.
#[test]
fn a_stocks_flows_are_compared_in_order() {
    use crate::test_common::TestProject;
    let stock = |outflows: &[&str]| {
        TestProject::new("p")
            .stock("s", "10", &[], outflows, None)
            .flow("a", "1", None)
            .flow("b", "2", None)
            .aux("bad", "1 +", None)
            .build_datamodel()
    };
    let changes = compare_pair(&stock(&["a", "b"]), &stock(&["b", "a"]));
    assert_eq!(
        reasons(&changes),
        ["'s' takes its outflows in another order in the save: b, a, not a, b"]
    );
    assert_eq!(changes[0].kind, ChangeKind::Structure);
    assert!(compare_pair(&stock(&["a", "b", "a"]), &stock(&["a", "b"])).is_empty());
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
    let changes = compare_pair(&table("0,10"), &table("0,20"));
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
    use crate::test_common::TestProject;
    let a = TestProject::new("p")
        .aux("a__b", "1", None)
        .build_datamodel();
    let b = TestProject::new("p")
        .aux("a_b", "1", None)
        .build_datamodel();
    let changes = super::compare_structure(&super::Meaning::of(&a), &super::Meaning::of(&b));
    assert_eq!(
        reasons(&changes),
        ["'a__b' is not in the save", "'a_b' is new in the save"]
    );
}

/// MDL has no conveyor, so the belt reads back as a plain stock. The
/// compiler's helpers for the belt and its leaks are not the model's
/// variables: the change is named on the belt and on the series it feeds.
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
        changes
            .iter()
            .any(|c| c.reason.contains("simulates differently")),
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

/// A converted Vensim model can hold its specs as variables (`SAVEPER =
/// TIME STEP`). MDL keeps them as the specs, with the same values.
#[test]
fn control_variables_kept_as_the_specs_are_no_change() {
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
/// Here the stock's flow reads back over more elements than the stock, and
/// the error both have is no change.
#[test]
fn a_model_that_does_not_simulate_changes_when_its_errors_do() {
    let project = mdl(&format!(
        "dim: A, B, C ~~|
dim2: A, B, C, D, E ~~|
demands[dim] = 10, 6, 3 ~~|
stock[dim] = INTEG(demands[dim], 0) ~~|
x = y + 1 ~~|
{CONTROL}"
    ));
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert_eq!(
        reasons(&changes),
        [
            "'demands' is defined over dim2, not dim",
            "'stock' has an error in the save that the model does not: \
             mismatched_dimensions -- the dimensions of 'demands' do not match \
             the dimensions of the equation that uses it",
        ],
        "{changes:?}"
    );
    assert_eq!(changes[1].variable.as_deref(), Some("stock"));
    // Neither simulates, so no results change.
    assert!(changes.iter().all(|c| c.kind == ChangeKind::Structure));
}

#[test]
fn a_save_that_does_not_read_back_is_a_change() {
    // MDL's reader takes no `*` in a subscript list, which the writer
    // writes for this model.
    let project = corpus_file("test/sdeverywhere/models/sumif/sumif.xmile");
    let changes = check_save(&project, SaveFormat::Mdl).unwrap();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(
        changes[0]
            .reason
            .starts_with("the save does not read back: ")
    );
    assert_eq!(changes[0].variable, None);
    assert_eq!(changes[0].kind, ChangeKind::Results);
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
