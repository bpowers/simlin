// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Vensim's PULSE and PULSE TRAIN as the reader imports them and the writer
//! saves them: each row is simulated, and the steps the pulse is on are
//! compared with the steps Vensim's documented rule puts it on
//! (vensim.com/documentation/fn_pulse.html, fn_pulse_train.html; the
//! `xmile_compat` arms cite the rules).

use crate::datamodel::{Equation, Project};
use crate::db::{
    LtmOverlay, SimlinDb, compile_project_incremental, sync_from_datamodel_incremental,
};
use crate::mdl::{parse_mdl, project_to_mdl};
use crate::vm::Vm;

/// The project an MDL file defining `p` (and the variables `w`, a pulse
/// width of 0, and `two`) reads as, run from 0 to `stop` by `dt`.
fn model(p: &str, dt: &str, stop: f64) -> String {
    format!(
        "p = {p} ~~|\nzero width = 0 ~~|\ntwo = 2 ~~|\n\
         INITIAL TIME = 0 ~~|\nFINAL TIME = {stop} ~~|\nTIME STEP = {dt} ~~|\nSAVEPER = TIME STEP ~~|\n"
    )
}

/// The saved steps (by index) at which `p` is 1; every other step must be 0.
fn on_steps(project: &Project) -> Vec<usize> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off)
        .expect("the model compiles");
    let mut vm = Vm::new(compiled).expect("the VM builds");
    vm.run_to_end().expect("the model runs");
    let series = crate::test_common::collect_results(&vm.into_results());
    let p = series.get("p").expect("p is saved");
    assert!(
        p.iter().all(|v| *v == 0.0 || *v == 1.0),
        "a pulse is 0 or 1: {p:?}"
    );
    p.iter()
        .enumerate()
        .filter(|(_, v)| **v == 1.0)
        .map(|(step, _)| step)
        .collect()
}

/// Each row: the equation, TIME STEP, FINAL TIME, and the steps it is on.
const PULSES: &[(&str, &str, f64, &[usize])] = &[
    // On while Time + TIME STEP/2 is strictly between start and start +
    // width.
    ("PULSE(3, 2)", "1", 8.0, &[3, 4]),
    // A start off the time grid starts the pulse at the step half a step
    // short of it.
    ("PULSE(3.2, 1)", "1", 8.0, &[3]),
    // Three steps of 0.3 are 0.8999999999999999: a comparison of Time itself
    // with 0.9 never fires.
    ("PULSE(0.9, 0.3)", "0.3", 3.0, &[3]),
    // A width passed as 0 is one TIME STEP, whether it is written as 0 or
    // is a variable that is 0.
    ("PULSE(3, 0)", "1", 8.0, &[3]),
    ("PULSE(3, zero width)", "1", 8.0, &[3]),
    ("PULSE(3, two)", "1", 8.0, &[3, 4]),
    ("PULSE(3, TIME STEP)", "1", 8.0, &[3]),
    // Any other width is the width, so one of half a step or less is never
    // strictly inside its window, and one between half a step and a step is
    // on for one step.
    ("PULSE(3, 0.5)", "1", 8.0, &[]),
    ("PULSE(3, 0.75)", "1", 8.0, &[3]),
    ("PULSE(3, -1)", "1", 8.0, &[]),
];

#[test]
fn a_pulse_is_on_while_time_plus_half_a_step_is_inside_its_window() {
    for (equation, dt, stop, expected) in PULSES {
        let project = parse_mdl(&model(equation, dt, *stop)).expect("the model reads");
        assert_eq!(
            on_steps(&project),
            *expected,
            "{equation} at TIME STEP {dt}"
        );
    }
}

const PULSE_TRAINS: &[(&str, &str, f64, &[usize])] = &[
    ("PULSE TRAIN(1, 1, 3, 8)", "1", 10.0, &[1, 4, 7]),
    // A pulse that starts at the end is on at the end and no later.
    ("PULSE TRAIN(1, 1, 2, 5)", "1", 8.0, &[1, 3, 5]),
    // Starts off the time grid by a rounding error.
    ("PULSE TRAIN(0.9, 0.3, 0.9, 2)", "0.3", 3.0, &[3, 6]),
    // A width at or below one TIME STEP lasts one TIME STEP.
    ("PULSE TRAIN(2, 0.25, 3, 9)", "1", 10.0, &[2, 5, 8]),
    ("PULSE TRAIN(2, 0, 3, 9)", "1", 10.0, &[2, 5, 8]),
    // An interval shorter than the width is on from start to end.
    ("PULSE TRAIN(2, 3, 1, 5)", "1", 8.0, &[2, 3, 4, 5]),
    // A width that is not a whole number of steps ends where a PULSE of
    // that width ends.
    ("PULSE TRAIN(1, 1.5, 4, 9)", "1", 10.0, &[1, 5, 9]),
];

#[test]
fn a_pulse_train_repeats_a_pulse_of_at_least_one_step_up_to_its_end() {
    for (equation, dt, stop, expected) in PULSE_TRAINS {
        let project = parse_mdl(&model(equation, dt, *stop)).expect("the model reads");
        assert_eq!(
            on_steps(&project),
            *expected,
            "{equation} at TIME STEP {dt}"
        );
    }
}

/// `p`'s equation as the project stores it.
fn stored(project: &Project) -> String {
    let p = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "p")
        .expect("p is a variable");
    match p.get_equation() {
        Some(Equation::Scalar(text)) => text.clone(),
        other => panic!("p is scalar: {other:?}"),
    }
}

#[test]
fn a_save_writes_each_pulse_back_as_the_call_it_was() {
    let rows = PULSES
        .iter()
        .chain(PULSE_TRAINS)
        .map(|(equation, dt, stop, _)| (*equation, *dt, *stop));
    for (equation, dt, stop) in rows {
        let first = parse_mdl(&model(equation, dt, stop)).expect("the model reads");
        let save = project_to_mdl(&first).expect("the model writes");
        let line = save
            .replace("\r\n", "\n")
            .replace("\n\t", " ")
            .lines()
            .find(|line| line.starts_with("p "))
            .and_then(|line| line.split(" ~").next())
            .unwrap_or("<no p>")
            .trim_end()
            .to_owned();
        assert_eq!(line, format!("p = {equation}"), "{save}");
        let second = parse_mdl(&save).expect("the save reads back");
        assert_eq!(stored(&second), stored(&first), "{equation}");
    }
}
