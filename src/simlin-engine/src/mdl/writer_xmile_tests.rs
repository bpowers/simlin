// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An XMILE model saved as MDL. XMILE holds things MDL spells another way or
//! not at all: names in any case, functions Vensim does not have, elements
//! named through their dimension, dimensions that are only a size. Each test
//! reads a model the XMILE reader produces, saves it, and asks `check_save`
//! what the save changes.

use crate::datamodel;
use crate::mdl::builtins::BUILTINS;
use crate::mdl::{parse_mdl, project_to_mdl, project_to_mdl_with_warnings};
use crate::save_check::{ChangeKind, SaveFormat, check_save};

/// The project the XMILE reader makes of these dimensions and variables.
fn xmile(dimensions: &str, variables: &str) -> datamodel::Project {
    let text = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><vendor>Simlin</vendor><product version="1.0">Simlin</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>8</stop><dt>0.5</dt></sim_specs>
  <dimensions>{dimensions}</dimensions>
  <model><variables>{variables}</variables></model>
</xmile>"#
    );
    crate::compat::open_xmile(&mut text.as_bytes()).expect("the XMILE reads")
}

/// What saving `project` as MDL changes, as `check_save` reports it.
fn changes(project: &datamodel::Project) -> Vec<String> {
    check_save(project, SaveFormat::Mdl)
        .expect("the check runs")
        .into_iter()
        .map(|change| format!("[{:?}] {}", change.kind, change.reason))
        .collect()
}

/// The changes to the model's results alone: a save that spells an equation
/// another way changes its definition and nothing it computes.
fn result_changes(project: &datamodel::Project) -> Vec<String> {
    check_save(project, SaveFormat::Mdl)
        .expect("the check runs")
        .into_iter()
        .filter(|change| change.kind == ChangeKind::Results)
        .map(|change| change.reason)
        .collect()
}

/// The save's equation for `name`, on one line.
fn saved_equation(project: &datamodel::Project, name: &str) -> String {
    let save = project_to_mdl(project).expect("the model writes");
    let save = save.replace("\r\n", "\n").replace("\\\n\t\t", "");
    let save = save.trim_start_matches("{UTF-8}\n");
    save.split("\n\t|\n")
        .chain(save.split("~~|\n"))
        .map(str::trim)
        .find(|entry| entry.starts_with(name))
        .and_then(|entry| entry.split("\n\t~").next())
        .map(|entry| entry.replace("\n\t", " "))
        .unwrap_or_else(|| panic!("{name} is not in the save:\n{save}"))
}

fn no_changes(project: &datamodel::Project) {
    let found = changes(project);
    assert!(
        found.is_empty(),
        "the save changes the model:\n{}\n{}",
        found.join("\n"),
        project_to_mdl(project).unwrap_or_default()
    );
}

const LETTERS: &str =
    r#"<dim name="Letters"><elem name="A"/><elem name="B"/><elem name="C"/></dim>"#;

#[test]
fn a_wildcard_names_its_dimension_whatever_the_case_of_the_name() {
    // The XMILE reader keeps a name as the file spells it, and an equation
    // may spell it another way again; the reference is written as the
    // variable's definition is.
    let project = xmile(
        LETTERS,
        r#"<aux name="A Values"><dimensions><dim name="Letters"/></dimensions><eqn>2</eqn></aux>
           <aux name="total"><eqn>SUM(a_VALUES[*])</eqn></aux>"#,
    );
    assert_eq!(
        saved_equation(&project, "total"),
        "total = SUM(A Values[letters!])"
    );
    no_changes(&project);
}

#[test]
fn a_variable_named_for_a_function_is_read_back_as_a_variable() {
    // The MDL reader takes a bare builtin name for the start of a call, so a
    // variable of that name is written quoted. One row per name the reader
    // knows.
    let mut names: Vec<&str> = BUILTINS.iter().copied().collect();
    names.sort_unstable();
    let mut failures = Vec::new();
    for name in names {
        let ident = name.replace(' ', "_");
        let project = xmile(
            "",
            &format!(
                r#"<aux name="{ident}"><eqn>3</eqn></aux>
                   <aux name="reader"><eqn>{ident} + 1</eqn></aux>"#
            ),
        );
        let save = project_to_mdl(&project).expect("the model writes");
        match parse_mdl(&save) {
            Ok(back) => {
                let idents = |p: &datamodel::Project| {
                    let mut idents: Vec<String> = p.models[0]
                        .variables
                        .iter()
                        .map(|v| crate::common::canonicalize(v.get_ident()).into_owned())
                        .collect();
                    idents.sort();
                    idents
                };
                if idents(&back) != idents(&project) {
                    failures.push(format!("{name}: reads back as {:?}", idents(&back)));
                    continue;
                }
                let found = changes(&project);
                if !found.is_empty() {
                    failures.push(format!("{name}: {}", found.join("; ")));
                }
            }
            Err(err) => failures.push(format!(
                "{name}: the save does not read back: {err}\n{}",
                save.split("INITIAL TIME").next().unwrap_or("")
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_function_vensim_does_not_have_reads_back_and_is_warned_of() {
    // MDL has no spelling for these, so the call is written by name: Simlin
    // reads it back as the function it is, and the warning says Vensim will
    // not.
    let rows = [
        ("mean", "MEAN(vals[*])"),
        ("previous", "PREVIOUS(x)"),
        ("previous", "PREVIOUS(x, 2)"),
        ("round", "ROUND(x / 3)"),
        // XMILE's INT floors and its MOD is a floored modulus (XMILE 1.0
        // section 3.3.1 and footnote 7); Vensim's INTEGER and MODULO
        // truncate, so they are other functions for a negative operand.
        ("int", "INT(x - 7.5)"),
        ("int", "3 * INT((x - 7.5) / 3)"),
        ("mod", "(x - 7) MOD 3"),
        // The engine reads XMILE's MODULO call as the MOD operator.
        ("mod", "MODULO(x - 7, 3)"),
    ];
    for (function, call) in rows {
        let project = xmile(
            LETTERS,
            &format!(
                r#"<aux name="x"><eqn>TIME</eqn></aux>
                   <aux name="vals"><dimensions><dim name="Letters"/></dimensions><eqn>TIME * 2</eqn></aux>
                   <aux name="y"><eqn>{call}</eqn></aux>"#
            ),
        );
        no_changes(&project);
        let (_, warnings) = project_to_mdl_with_warnings(&project).expect("the model writes");
        let named = function.to_uppercase();
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("'y'") && w.message.contains(&named)),
            "{call}: no warning names {named}: {:?}",
            warnings.iter().map(|w| &w.message).collect::<Vec<_>>()
        );
    }
}

#[test]
fn a_function_vensim_has_is_not_warned_of() {
    let project = xmile(
        "",
        r#"<aux name="x"><eqn>TIME</eqn></aux>
           <aux name="y"><eqn>MAX(ABS(x), SQRT(x)) + SMTH1(x, 2)</eqn></aux>"#,
    );
    let (_, warnings) = project_to_mdl_with_warnings(&project).expect("the model writes");
    assert!(
        warnings.is_empty(),
        "{:?}",
        warnings.iter().map(|w| &w.message).collect::<Vec<_>>()
    );
}

#[test]
fn an_element_named_as_a_value_keeps_its_position() {
    // `Letters.B` is B's position in Letters. Vensim names the element alone.
    let project = xmile(
        LETTERS,
        r#"<aux name="second"><eqn>Letters.B</eqn></aux>
           <aux name="both"><eqn>Letters.C * 10 + Letters.A</eqn></aux>"#,
    );
    assert_eq!(saved_equation(&project, "second"), "second = b");
    no_changes(&project);
}

#[test]
fn an_element_of_a_subrange_named_as_a_value_keeps_its_position_there() {
    // B is second in Letters and first in Later: the bare name is its place
    // in the dimension that owns it, so its place in the subrange is written
    // as the number it is.
    let project = xmile(
        &format!(r#"{LETTERS}<dim name="Later"><elem name="B"/><elem name="C"/></dim>"#),
        r#"<aux name="in_letters"><eqn>Letters.B</eqn></aux>
           <aux name="in_later"><eqn>Later.B</eqn></aux>"#,
    );
    assert_eq!(saved_equation(&project, "in letters"), "in letters = b");
    assert_eq!(saved_equation(&project, "in later"), "in later = 1");
    assert_eq!(result_changes(&project), Vec::<String>::new());
}

#[test]
fn a_pulse_keeps_its_volume_and_its_interval() {
    // XMILE's PULSE(volume, first, interval) is `volume / DT` for one step at
    // `first` and every `interval` after; Vensim's PULSE(start, width) is 1
    // from `start` for `width`. The save spells out what XMILE's computes.
    for call in [
        "PULSE(6, 2)",
        "PULSE(6, 2, 0)",
        "PULSE(6, 2, 3)",
        "PULSE(x + 1, x / 4, 1.5)",
        "PULSE(6, 2, x - 6)",
    ] {
        let project = xmile(
            "",
            &format!(
                r#"<aux name="x"><eqn>8</eqn></aux>
                   <aux name="y"><eqn>{call}</eqn></aux>
                   <stock name="level"><eqn>0</eqn><inflow>filling</inflow></stock>
                   <flow name="filling"><eqn>y</eqn></flow>"#
            ),
        );
        assert_eq!(
            result_changes(&project),
            Vec::<String>::new(),
            "{call}: {}",
            saved_equation(&project, "y")
        );
        assert!(
            !saved_equation(&project, "y").contains("PULSE"),
            "{call} is written as Vensim's PULSE, another function: {}",
            saved_equation(&project, "y")
        );
    }
}

#[test]
fn a_dimension_that_is_only_a_size_is_written_as_named_elements() {
    // Vensim's subscripts are names. An indexed dimension's elements are
    // written `<dimension><position>`, and a reference by position names the
    // element at it.
    let project = xmile(
        r#"<dim name="slot" size="3"/>"#,
        r#"<aux name="held"><dimensions><dim name="slot"/></dimensions>
             <element subscript="1"><eqn>10</eqn></element>
             <element subscript="2"><eqn>20</eqn></element>
             <element subscript="3"><eqn>TIME</eqn></element>
           </aux>
           <aux name="doubled"><dimensions><dim name="slot"/></dimensions><eqn>held * 2</eqn></aux>
           <aux name="middle"><eqn>held[2] + doubled[3]</eqn></aux>
           <aux name="total"><eqn>SUM(held[*])</eqn></aux>"#,
    );
    let save = project_to_mdl(&project).expect("the model writes");
    assert!(
        save.replace("\r\n", "\n")
            .contains("slot:\n\t(slot1-slot3)"),
        "{save}"
    );
    assert_eq!(
        saved_equation(&project, "middle"),
        "middle = held[slot2] + doubled[slot3]"
    );
    // The save reads back and runs, and what it changes is the elements'
    // names: the scalars computed from them are what they were.
    assert_eq!(
        result_changes(&project),
        [
            "dimension 'slot' has named elements in the save, not numbered ones",
            "dimension 'slot' has elements slot1, slot2, slot3 in the save, not 1, 2, 3",
            "'doubled' loses elements '1', '2', and '3'",
            "'doubled' gains elements 'slot1', 'slot2', and 'slot3'",
            "'held' loses elements '1', '2', and '3'",
            "'held' gains elements 'slot1', 'slot2', and 'slot3'",
        ],
        "{}",
        save.split("INITIAL TIME").next().unwrap_or("")
    );
}

#[test]
fn a_stock_with_no_flows_gains_none() {
    let project = xmile("", r#"<stock name="held"><eqn>4</eqn></stock>"#);
    assert_eq!(saved_equation(&project, "held"), "held= INTEG(0, 4)");
    no_changes(&project);
}

#[test]
fn a_variable_with_no_equation_reads_back_with_none() {
    // The save writes `x =` with nothing after it, which the reader reads as
    // no equation, so the model keeps the error it has.
    let project = xmile(
        "",
        r#"<aux name="unknown"><eqn></eqn></aux>
           <aux name="reader"><eqn>unknown * 2</eqn></aux>
           <flow name="draining"><eqn></eqn></flow>
           <stock name="level"><eqn>3</eqn><outflow>draining</outflow></stock>"#,
    );
    assert_eq!(saved_equation(&project, "unknown"), "unknown =");
    no_changes(&project);
}

#[test]
fn a_name_with_a_line_break_keeps_it_and_its_references() {
    // Stella writes a multi-line name with the `\n` escape; an equation may
    // name the variable without the break (the engine's names fold it into
    // the space around it). The save spells every reference as the
    // definition, so the reader links the flow to its stock and the reader
    // to the flow.
    let project = xmile(
        "",
        r#"<stock name="water\nin tank"><eqn>10</eqn><outflow>leak\nrate</outflow></stock>
           <flow name="leak\nrate"><eqn>water_in_tank / 4</eqn></flow>
           <aux name="double leak"><eqn>leak_rate * 2</eqn></aux>"#,
    );
    let save = project_to_mdl(&project).expect("the model writes");
    assert!(save.contains(r#"INTEG(-"leak\nrate""#), "{save}");
    assert!(save.contains(r#"double leak = "leak\nrate" * 2"#), "{save}");
    no_changes(&project);
}

#[test]
fn a_variable_defining_some_elements_of_its_dimension_is_warned_of() {
    // XMILE can define a variable over a dimension and give only some of
    // its elements equations; MDL has no spelling for the rest, so the save
    // reads back over the dimension of exactly the elements it defines.
    let project = xmile(
        r#"<dim name="DimA"><elem name="A1"/><elem name="A2"/><elem name="A3"/></dim>
           <dim name="SubA"><elem name="A1"/><elem name="A2"/></dim>"#,
        r#"<aux name="x"><dimensions><dim name="DimA"/></dimensions>
             <element subscript="A1"><eqn>1</eqn></element>
             <element subscript="A2"><eqn>TIME</eqn></element></aux>"#,
    );
    let (_, warnings) = project_to_mdl_with_warnings(&project).expect("the model writes");
    // The XMILE reader keeps a dimension's name in its canonical form.
    assert!(
        warnings.iter().any(|w| {
            let message = w.message.to_lowercase();
            message.contains("'x'")
                && message.contains("defines only some elements of dima")
                && message.contains("suba")
        }),
        "{warnings:?}"
    );
}

#[test]
fn a_call_beside_a_variable_of_its_name_keeps_its_meaning() {
    // A variable is not a function: a PULSE or MODULO call is the engine's
    // builtin whatever the model names, so it is written as what it
    // computes, never as the Vensim function of that name, which means
    // something else. Rows: each builtin the writer spells another way.
    for (name, call) in [("pulse", "PULSE(1, 2)"), ("modulo", "MODULO(-7, 3)")] {
        let project = xmile(
            "",
            &format!(
                r#"<aux name="{name}"><eqn>5</eqn></aux><aux name="y"><eqn>{call}</eqn></aux>"#
            ),
        );
        assert_eq!(result_changes(&project), Vec::<String>::new(), "{call}");
    }
}

#[test]
fn a_name_drawn_with_a_run_of_spaces_is_written_with_one() {
    // An XMILE label wrapped after a space (`name="fractional \ngrowth
    // rate"`, whose line break XML reads as a second space) names the
    // variable `fractional_growth_rate`; the save spells it, in its
    // definition and its references alike, with one space between words.
    let path = "../../test/logistic_growth_ltm/logistic_growth.stmx";
    let text = std::fs::read_to_string(path).expect("the corpus file reads");
    let project = crate::compat::open_xmile(&mut text.as_bytes()).expect("the XMILE reads");
    let save = project_to_mdl(&project).expect("the model writes");
    assert!(
        save.contains("fractional growth rate") && !save.contains("fractional  growth"),
        "{save}"
    );
    assert_eq!(result_changes(&project), Vec::<String>::new());
}

#[test]
fn an_arrayed_table_with_one_shared_input_keeps_its_input() {
    // XMILE's arrayed graphical function with one shared `<eqn>` reads as an
    // :EXCEPT: default with a table and no equation in each element. No
    // :EXCEPT: equation defines an element there, so each element is written
    // as its own, with the default as its table's input, and the warning
    // says so. Rows: a minimal model and the corpus file.
    let minimal = xmile(
        r#"<dim name="D"><elem name="p"/><elem name="q"/></dim>"#,
        r#"<aux name="c"><dimensions><dim name="D"/></dimensions>
             <element subscript="p"><gf><xscale min="0" max="8"/><ypts>10,20,30</ypts></gf></element>
             <element subscript="q"><gf><xscale min="0" max="8"/><ypts>1,2,3</ypts></gf></element>
             <eqn>TIME</eqn></aux>"#,
    );
    let path = "../../test/test-models/samples/arrays/non-a2a/non-a2a-gf.stmx";
    let text = std::fs::read_to_string(path).expect("the corpus file reads");
    let corpus = crate::compat::open_xmile(&mut text.as_bytes()).expect("the XMILE reads");
    for project in [minimal, corpus] {
        assert_eq!(result_changes(&project), Vec::<String>::new());
        let (_, warnings) = project_to_mdl_with_warnings(&project).expect("the model writes");
        assert!(
            warnings.iter().any(|w| w
                .message
                .contains("take the default as their table's input")
                && !w.message.contains("are not written")),
            "{warnings:?}"
        );
    }
}
