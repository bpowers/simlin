// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An MDL save of a variable named like one of the engine's builtins.

use simlin_base::datamodel;
use simlin_engine::queue_compile::build_vm;
use simlin_formats::{open_vensim, to_mdl};
use simlin_save_check::{ChangeKind, SaveFormat, check_save};

/// An MDL model whose control section is `TIME STEP = 0.125` over 0 to 2,
/// with `variables` before it.
fn mdl_with_control(variables: &str) -> datamodel::Project {
    let mdl = format!(
        "{variables}\
         INITIAL TIME = 0\n\t~\t\n\t~\t|\n\n\
         FINAL TIME = 2\n\t~\t\n\t~\t|\n\n\
         TIME STEP = 0.125\n\t~\t\n\t~\t|\n\n\
         SAVEPER = TIME STEP\n\t~\t\n\t~\t|\n\n\
         \\\\\\---/// Sketch information - do not modify anything except names\n"
    );
    open_vensim(&mdl).expect("MDL must parse")
}

/// Run `project` and return the final value of `var`.
fn final_value(project: &datamodel::Project, var: &str) -> f64 {
    let mut vm = build_vm(project, "main").expect("model must build");
    vm.run_to_end().expect("simulation must run");
    let results = vm.into_results();
    let offset = *results
        .offsets
        .get(var)
        .unwrap_or_else(|| panic!("no results column for `{var}`"));
    results
        .iter()
        .next_back()
        .unwrap_or_else(|| panic!("`{var}` has an empty timeseries"))[offset]
}

/// An MDL save of `dt = TIME STEP` writes it as it was, and a reference to
/// `dt` as the variable: the corpus model that declares it saves its
/// definition so and keeps its run.
#[test]
fn an_mdl_save_writes_a_variable_defined_as_the_time_step_as_it_was() {
    let project = mdl_with_control(
        "dt = TIME STEP\n\t~\t\n\t~\t|\n\n\
         x = dt * 2\n\t~\t\n\t~\t|\n\n",
    );
    let written = to_mdl(&project).expect("MDL must write");
    let lines: Vec<&str> = written.lines().map(str::trim).collect();
    assert!(lines.contains(&"dt = TIME STEP"), "{written}");
    assert!(lines.contains(&"x = dt * 2"), "{written}");
    let read_back = open_vensim(&written).expect("the save reads");
    assert_eq!(final_value(&read_back, "dt"), 0.125);
    assert_eq!(final_value(&read_back, "x"), 0.25);

    let theil = std::fs::read_to_string("../../test/metasd/theil-statistics/Theil_2011.mdl")
        .expect("the corpus file reads");
    let theil = open_vensim(&theil).expect("the corpus file imports");
    let written = to_mdl(&theil).expect("MDL must write");
    assert!(
        written.lines().any(|line| line.trim() == "dt = TIME STEP"),
        "{written}"
    );
    let results: Vec<String> = check_save(&theil, SaveFormat::Mdl)
        .expect("the check runs")
        .into_iter()
        .filter(|c| c.kind == ChangeKind::Results)
        .map(|c| c.reason)
        .collect();
    assert_eq!(results, Vec::<String>::new());
}
