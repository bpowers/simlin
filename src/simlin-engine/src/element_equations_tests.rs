// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An MDL file's per-element copies opened as one equation.

use crate::datamodel::{Equation, Project};
use crate::test_common::TestProject;

const CONTROL: &str =
    "INITIAL TIME = 0 ~~|\nFINAL TIME = 2 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = 1 ~~|\n";

fn open(mdl: &str) -> Project {
    crate::open_vensim(&format!("{mdl}{CONTROL}\\\\\\---///\n")).expect("the model opens")
}

fn equation(project: &Project, ident: &str) -> Equation {
    project.models[0]
        .get_variable(ident)
        .and_then(|v| v.get_equation())
        .cloned()
        .unwrap_or_else(|| panic!("no {ident}"))
}

fn run(project: &Project) -> std::collections::HashMap<String, Vec<f64>> {
    TestProject::from_datamodel(project.clone())
        .run_vm()
        .expect("the model runs")
}

const AGES: &str = "Age: young, adult, old ~~|\nOlder: adult, old ~~|\nc[Age] = 10, 20, 30 ~~|\n";

#[test]
fn elements_that_say_one_thing_open_as_one_equation() {
    // Two equations define `d`, so the reader writes it out per element, with
    // each element's name in its copy.
    let project = open(&format!(
        "{AGES}d[young] = c[young] * 2 ~~|\nd[Older] = c[Older] * 2 ~~|\n"
    ));
    assert_eq!(
        equation(&project, "d"),
        Equation::ApplyToAll(vec!["Age".to_string()], "c[Age] * 2".to_string())
    );
    assert_eq!(run(&project)["d[old]"], vec![60.0, 60.0, 60.0]);
}

#[test]
fn an_element_that_says_another_thing_keeps_its_own() {
    let project = open(&format!(
        "{AGES}d[young] = 1 ~~|\nd[Older] = c[Older] * 2 ~~|\n"
    ));
    let Equation::Arrayed(dims, arms, Some(default), true) = equation(&project, "d") else {
        panic!("a default and an exception: {:?}", equation(&project, "d"));
    };
    assert_eq!(dims, ["Age"]);
    assert_eq!(default, "c[Age] * 2");
    let arms: Vec<(&str, &str)> = arms.iter().map(|a| (a.0.as_str(), a.1.as_str())).collect();
    assert_eq!(arms, [("young", "1")]);
}

#[test]
fn an_except_equations_copies_follow_its_default() {
    let project = open(&format!(
        "{AGES}d[Age] :EXCEPT: [young] = c[Age] + 1 ~~|\nd[young] = 0 ~~|\n"
    ));
    let Equation::Arrayed(_, arms, Some(default), true) = equation(&project, "d") else {
        panic!("a default and an exception: {:?}", equation(&project, "d"));
    };
    assert_eq!(default, "c[Age] + 1");
    let keys: Vec<&str> = arms.iter().map(|a| a.0.as_str()).collect();
    assert_eq!(keys, ["young"]);
}

#[test]
fn what_cannot_be_read_as_one_equation_stays_per_element() {
    let rows = [
        // Each element says something else.
        "d[young] = 1 ~~|\nd[adult] = c[old] ~~|\nd[old] = 3 ~~|\n",
        // Numbers are a list of their own, negative ones too.
        "d[young] = 1 ~~|\nd[Older] = 1 ~~|\n",
        "d[young] = -1 ~~|\nd[Older] = -1, 5 ~~|\n",
        // `Older` lacks `young`, so `x[young]` is no reading of `x[Age]`.
        "x[Older] = 5, 6 ~~|\nd[young] = c[young] ~~|\nd[Older] = x[Older] ~~|\n",
        // An element left out has no equation, which a default would give it.
        "d[young] = c[young] ~~|\nd[adult] = c[adult] ~~|\n",
    ];
    for mdl in rows {
        let project = open(&format!("{AGES}{mdl}"));
        assert!(
            matches!(
                equation(&project, "d"),
                Equation::Arrayed(_, _, None, false)
            ),
            "{mdl}: {:?}",
            equation(&project, "d")
        );
    }
}

#[test]
fn an_element_a_fixed_number_of_places_along_is_kept_as_written() {
    // Read as "this element", each older cohort reads the one before it, an
    // offset; the compaction stores no offset (the module docs), so the
    // elements keep their equations, and the run is Vensim's.
    let project = open(&format!(
        "{AGES}d[young] = 0 ~~|\nd[adult] = c[young] ~~|\nd[old] = c[adult] ~~|\n"
    ));
    assert!(matches!(
        equation(&project, "d"),
        Equation::Arrayed(_, _, None, false)
    ));
    assert_eq!(run(&project)["d[old]"], vec![20.0, 20.0, 20.0]);
}

/// The save check reads each element's equation at its element, so an
/// element written with its names in place is the equation every element
/// follows.
#[test]
fn an_element_equation_is_compared_at_its_element() {
    let project = TestProject::new("compare")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Age", &["young", "adult", "old"])
        .array_with_ranges(
            "c[Age]",
            vec![("young", "10"), ("adult", "20"), ("old", "30")],
        )
        .build_datamodel();
    let reader = crate::element_equations::ElementReader::new(&project, &project.models[0]);
    let dims = ["Age".to_string()];
    assert_eq!(
        reader.read("c[adult] * 2", &dims, "adult").as_deref(),
        Some("c[Age] * 2")
    );
    assert_eq!(
        reader.read("c[young] * 2", &dims, "adult").as_deref(),
        Some("c[Age - 1] * 2")
    );
    assert_eq!(reader.read("c[Age] * 2", &dims, "adult"), None);
}
