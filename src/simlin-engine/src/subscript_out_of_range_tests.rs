// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A subscript that reads no element is told (`ErrorCode::SubscriptOutOfRange`).
//!
//! A chain reads the element before it with an offset in an equation for
//! every element -- `c[Age - 1]` -- and the first element has none before it.
//! The read is not a number at every step of the run, and nothing else says
//! so: the model compiles and runs, and the NaN flows on into whatever reads
//! it. Each element whose equation makes such a read gets one warning, on the
//! variable, naming the element and pointing at the reference, so a host can
//! show it beside that element.

use crate::common::ErrorCode;
use crate::db::{DiagnosticError, DiagnosticSeverity};
use crate::test_common::TestProject;

/// Every `SubscriptOutOfRange` row of `project`, as `(variable, the
/// referenced text, details)`, the text cut from `equation` by the row's span.
fn out_of_range(project: &TestProject, equation: &str) -> Vec<(String, String, String)> {
    let mut rows: Vec<(String, String, String)> = project
        .diagnostics_incremental()
        .into_iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Equation(e) if e.code == ErrorCode::SubscriptOutOfRange => {
                assert_eq!(d.severity, DiagnosticSeverity::Warning);
                let text = equation
                    .get(e.start as usize..e.end as usize)
                    .unwrap_or_default()
                    .to_string();
                Some((
                    d.variable.clone().unwrap_or_default(),
                    text,
                    e.details.clone().unwrap_or_default(),
                ))
            }
            _ => None,
        })
        .collect();
    rows.sort();
    rows
}

fn ages() -> TestProject {
    TestProject::new("out_of_range")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Age", &["young", "adult", "old"])
        .array_with_ranges(
            "c[Age]",
            vec![("young", "10"), ("adult", "20"), ("old", "30")],
        )
}

fn row(variable: &str, text: &str, details: &str) -> (String, String, String) {
    (variable.to_string(), text.to_string(), details.to_string())
}

#[test]
fn the_element_before_the_first_is_told() {
    let equation = "c[Age - 1]";
    let project = ages().array_aux("d[Age]", equation);
    assert_eq!(
        out_of_range(&project, equation),
        vec![row(
            "d",
            "c[Age - 1]",
            "the equation for 'young' reads 'c' before the first element of 'age', which is \
             not a number"
        )]
    );
    // The warning describes the run: the model still runs, and the first
    // element is the NaN it names.
    let results = project.run_vm().expect("an offset past the end still runs");
    assert!(results["d[young]"][0].is_nan());
    assert_eq!(results["d[adult]"][0], 10.0);
    assert_eq!(results["d[old]"][0], 20.0);
}

#[test]
fn the_element_past_the_last_is_told() {
    let equation = "c[Age + 1]";
    assert_eq!(
        out_of_range(&ages().array_aux("d[Age]", equation), equation),
        vec![row(
            "d",
            "c[Age + 1]",
            "the equation for 'old' reads 'c' past the last element of 'age', which is not a \
             number"
        )]
    );
}

/// A guard on the element's own position is decided as the equation is
/// compiled, and the element it excludes reads nothing past the end.
#[test]
fn a_read_guarded_by_the_elements_position_is_not_told() {
    let equation = "IF Age > 1 THEN c[Age - 1] ELSE 0";
    let project = ages().array_aux("d[Age]", equation);
    assert_eq!(out_of_range(&project, equation), vec![]);
    assert_eq!(project.run_vm().unwrap()["d[young]"][0], 0.0);
}

/// A guard that joins the element's position to a test of the model is
/// decided by its position all the same: `young` never makes the read, and is
/// 0 at every step.
#[test]
fn a_position_guard_beside_a_test_of_the_model_is_not_told() {
    for equation in [
        "IF Age > 1 AND switch = 1 THEN c[Age - 1] ELSE 0",
        "IF switch = 1 AND Age > 1 THEN c[Age - 1] ELSE 0",
        "IF Age = 1 OR switch = 0 THEN 0 ELSE c[Age - 1]",
        "IF NOT (Age > 1) THEN 0 ELSE c[Age - 1]",
    ] {
        let project = ages()
            .aux("switch", "1", None)
            .array_aux("d[Age]", equation);
        assert_eq!(out_of_range(&project, equation), vec![], "{equation}");
        assert_eq!(
            project.run_vm().unwrap()["d[young]"],
            vec![0.0, 0.0],
            "{equation}"
        );
    }
    // A guard on the model alone decides nothing as the equation compiles.
    let equation = "IF switch = 1 THEN c[Age - 1] ELSE 0";
    let project = ages()
        .aux("switch", "1", None)
        .array_aux("d[Age]", equation);
    assert_eq!(out_of_range(&project, equation).len(), 1);
}

/// Every element that makes the read is named, one row each: over two
/// dimensions, the whole first row.
#[test]
fn each_element_that_reads_past_the_end_has_its_own_row() {
    let equation = "m[Age - 1, Region]";
    let project = ages()
        .named_dimension("Region", &["north", "south"])
        .array_aux("m[Age, Region]", "1")
        .array_aux("d[Age, Region]", equation);
    assert_eq!(
        out_of_range(&project, equation),
        vec![
            row(
                "d",
                equation,
                "the equation for 'young,north' reads 'm' before the first element of 'age', \
                 which is not a number"
            ),
            row(
                "d",
                equation,
                "the equation for 'young,south' reads 'm' before the first element of 'age', \
                 which is not a number"
            ),
        ]
    );
}

/// An element's own equation reads its dimension's name as that element, so
/// an offset there is told for that element alone.
#[test]
fn an_offset_in_one_elements_own_equation_is_told_for_that_element() {
    let equation = "c[Age - 1] * 2";
    let project = ages().array_with_ranges(
        "d[Age]",
        vec![("young", equation), ("adult", "1"), ("old", "1")],
    );
    assert_eq!(
        out_of_range(&project, equation),
        vec![row(
            "d",
            "c[Age - 1]",
            "the equation for 'young' reads 'c' before the first element of 'age', which is \
             not a number"
        )]
    );
}

/// A scalar equation whose computed index runs off the end names no element.
#[test]
fn a_scalar_read_past_the_end_names_no_element() {
    let equation = "c[2 + 2]";
    assert_eq!(
        out_of_range(&ages().aux("x", equation, None), equation),
        vec![row(
            "x",
            "c[2 + 2]",
            "the equation reads 'c' past the last element of 'age', which is not a number"
        )]
    );
}

/// A stock's initial value and a builtin's hoisted argument are told as the
/// plain equation is: the helper's read is reported on the variable it was
/// made for, at the element it is one element of.
#[test]
fn a_stocks_initial_value_and_a_hoisted_argument_are_told() {
    let initial = "c[Age - 1]";
    let stock = ages().array_stock("s[Age]", initial, &[], &[], None);
    assert_eq!(
        out_of_range(&stock, initial),
        vec![row(
            "s",
            "c[Age - 1]",
            "the equation for 'young' reads 'c' before the first element of 'age', which is \
             not a number"
        )]
    );

    let smoothed = "SMTH1(c[Age - 1], 1)";
    assert_eq!(
        out_of_range(&ages().array_aux("d[Age]", smoothed), smoothed),
        vec![row(
            "d",
            "c[Age - 1]",
            "the equation for 'young' reads 'c' before the first element of 'age', which is \
             not a number"
        )]
    );
}

/// Under `PREVIOUS` and `INIT` the argument is computed over the variable's
/// own dimensions, and the element that makes the read is named as it is in
/// the plain equation.
#[test]
fn a_read_under_previous_or_init_names_its_element() {
    for equation in ["PREVIOUS(c[Age - 1], 0)", "INIT(c[Age - 1])"] {
        assert_eq!(
            out_of_range(&ages().array_aux("d[Age]", equation), equation),
            vec![row(
                "d",
                "c[Age - 1]",
                "the equation for 'young' reads 'c' before the first element of 'age', which \
                 is not a number"
            )],
            "{equation}"
        );
    }
}

/// The warning is not an error: a model with nothing else wrong still
/// compiles, and its code is the one a host matches on.
#[test]
fn the_warning_has_a_code_and_a_plain_description() {
    assert_eq!(
        ErrorCode::SubscriptOutOfRange.to_string(),
        "subscript_out_of_range"
    );
    let project = ages().array_aux("d[Age]", "c[Age - 1]");
    assert_eq!(project.error_diagnostics(), vec![]);
}
