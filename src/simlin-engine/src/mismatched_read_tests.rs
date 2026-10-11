// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A read whose dimensions disagree with its reader's is told once, on the
//! reader, at the reference (`common::MismatchedRead`).
//!
//! Arraying a variable over a dimension leaves every variable that reads it
//! where one value is needed with a read of many; removing a dimension leaves
//! every reference that subscripts it on that dimension subscripting one the
//! variable no longer has. A host that gathers the readers an edit broke,
//! and offers to split them or have them read one value, needs each such
//! read as one diagnostic naming the reader, the variable read, the
//! reference and the dimensions -- whichever stage of the compiler refuses
//! it. The rows below are every stage that does: arithmetic, a builtin's
//! scalar argument, `PREVIOUS`/`INIT`, a module function's argument (bare
//! and hoisted), a condition, a stock's flow, an apply-to-all element, an
//! element's own equation, a partly subscripted reference, and the four
//! spellings that subscript a variable past its dimensions.

use crate::common::{ErrorCode, MismatchedRead};
use crate::db::{DiagnosticError, DiagnosticSeverity};
use crate::test_common::TestProject;

/// One refused read as a host reads it: the reader, the element whose own
/// equation holds it (if any), the reference's text, and the read.
#[derive(Debug, PartialEq)]
struct Row {
    reader: String,
    element: Option<String>,
    text: String,
    read: MismatchedRead,
}

/// Every Error-severity diagnostic of `project`, each of which must be a
/// refused read; the text of each is cut from the reader's `equation`.
fn rows(project: &TestProject, equation: &str) -> Vec<Row> {
    project
        .diagnostics_incremental()
        .into_iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .map(|d| {
            let DiagnosticError::Equation(error) = &d.error else {
                panic!("a refused read is an equation error, not {:?}", d.error);
            };
            assert_eq!(error.code, ErrorCode::MismatchedDimensions);
            assert_eq!(d.owner, None, "filed on the variable the person wrote");
            Row {
                reader: d.variable.clone().unwrap_or_default(),
                element: error.element.clone(),
                text: equation
                    .get(error.start as usize..error.end as usize)
                    .unwrap_or_default()
                    .to_string(),
                read: error
                    .mismatched_read
                    .as_deref()
                    .cloned()
                    .expect("a refused read names what it reads"),
            }
        })
        .collect()
}

fn row(reader: &str, text: &str, variable: &str, unpaired: &[&str]) -> Row {
    Row {
        reader: reader.to_string(),
        element: None,
        text: text.to_string(),
        read: MismatchedRead {
            variable: variable.to_string(),
            unpaired: unpaired.iter().map(|d| d.to_string()).collect(),
        },
    }
}

fn regions() -> TestProject {
    TestProject::new("mismatched_read")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Region", &["north", "south"])
        .named_dimension("Age", &["young", "adult", "old"])
}

/// `v` arrayed over `Region`, and `x` reading it by `equation`.
fn read_by(equation: &str) -> TestProject {
    regions()
        .array_const("v[Region]", 1.0)
        .scalar_aux("x", equation)
}

#[test]
fn a_scalar_reading_an_arrayed_variable_is_told_at_the_reference() {
    for (equation, text) in [
        ("v * 2", "v"),
        ("(v + 1) * 2", "v"),
        ("MAX(v, 0)", "v"),
        ("IF v > 1 THEN 1 ELSE 0", "v"),
        ("INIT(v)", "v"),
        ("PREVIOUS(v)", "v"),
        // A fallback the array route cannot carry is no different: the read
        // is refused before the fallback is looked at.
        ("PREVIOUS(v, 66.6)", "v"),
        ("PREVIOUS(v, 1) + 1", "v"),
        // A module function's argument: the bare one wired to the port, and
        // one hoisted into a helper of its own. Neither is reported under the
        // helper's name.
        ("SMTH1(v, 3)", "v"),
        ("DELAY1(v, 3)", "v"),
        ("SMTH1(v * 2, 3)", "v"),
    ] {
        assert_eq!(
            rows(&read_by(equation), equation),
            vec![row("x", text, "v", &["region"])],
            "{equation}"
        );
    }
}

#[test]
fn a_stock_filled_by_an_arrayed_flow_is_told_with_no_span() {
    let project = regions()
        .array_flow("v[Region]", "1", None)
        .stock("s", "0", &["v"], &[], None);
    // The flow list is no equation: the span is empty.
    assert_eq!(rows(&project, ""), vec![row("s", "", "v", &["region"])]);
}

#[test]
fn an_arrayed_reader_over_another_dimension_is_told() {
    // Age has three elements and Region two, so no element of one reads one
    // of the other.
    let equation = "v * 2";
    let project = regions()
        .array_const("v[Region]", 1.0)
        .array_aux("x[Age]", equation);
    assert_eq!(
        rows(&project, equation),
        vec![row("x", "v", "v", &["region"])]
    );
}

/// An arrayed reader whose other operand is arrayed too (`pop[Age] *
/// rate`, once `rate` is split by Region): the operands' bounds disagree
/// before any one reference is lowered, and the reference the reader has no
/// element of is the one told.
#[test]
fn an_arrayed_operand_beside_the_read_does_not_hide_it() {
    for equation in ["a * v", "v * a", "a + (v + 1)", "IF a > 0 THEN v ELSE a"] {
        let project = regions()
            .array_const("v[Region]", 1.0)
            .array_const("a[Age]", 1.0)
            .array_aux("x[Age]", equation);
        assert_eq!(
            rows(&project, equation),
            vec![row("x", "v", "v", &["region"])],
            "{equation}"
        );
    }
}

#[test]
fn a_lookup_of_a_split_table_is_told_at_the_table() {
    let gf = crate::datamodel::GraphicalFunction {
        kind: crate::datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![0.0, 100.0],
        x_scale: crate::datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: crate::datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 100.0,
        },
    };
    for (equation, expected) in [
        (
            "LOOKUP(tbl, 1) + 1",
            vec![row("x", "tbl", "tbl", &["region"])],
        ),
        ("SUM(LOOKUP(tbl, 1))", vec![]),
    ] {
        let mut project = regions()
            .aux_with_gf("tbl", "TIME", gf.clone())
            .scalar_aux("x", equation)
            .build_datamodel();
        let project_dims = project.dimensions.clone();
        crate::patch_fields::set_dimensions(
            &project_dims,
            &mut project.models[0],
            "tbl",
            &["Region".to_string()],
            None,
        )
        .expect("the table splits");
        assert_eq!(
            rows(&TestProject::from_datamodel(project), equation),
            expected,
            "{equation}"
        );
    }
}

#[test]
fn an_elements_own_equation_names_the_element() {
    let equation = "v * 2";
    let project = regions().array_const("v[Region]", 1.0).array_with_ranges(
        "x[Age]",
        vec![("young", equation), ("adult", "1"), ("old", "1")],
    );
    assert_eq!(
        rows(&project, equation),
        vec![Row {
            element: Some("young".to_string()),
            ..row("x", "v", "v", &["region"])
        }]
    );
}

#[test]
fn a_reference_subscripted_on_fewer_dimensions_names_the_rest() {
    let equation = "w[north] * 2";
    let project = regions()
        .array_const("w[Region, Age]", 1.0)
        .scalar_aux("x", equation);
    assert_eq!(
        rows(&project, equation),
        vec![row("x", "w[north]", "w", &["age"])]
    );
}

#[test]
fn a_reference_subscripted_past_the_variables_dimensions_names_none() {
    let scalar = || regions().scalar_const("v", 1.0);
    for (project, equation, text, variable) in [
        (
            scalar().scalar_aux("x", "v[north]"),
            "v[north]",
            "v[north]",
            "v",
        ),
        (
            scalar().scalar_aux("x", "SUM(v[*])"),
            "SUM(v[*])",
            "v[*]",
            "v",
        ),
        (
            scalar().array_aux("x[Region]", "v[Region]"),
            "v[Region]",
            "v[Region]",
            "v",
        ),
        (
            regions()
                .array_const("w[Region]", 1.0)
                .scalar_aux("x", "w[north, young]"),
            "w[north, young]",
            "w[north, young]",
            "w",
        ),
    ] {
        assert_eq!(
            rows(&project, equation),
            vec![row("x", text, variable, &[])],
            "{equation}"
        );
    }
}

#[test]
fn the_reason_names_the_variable_and_the_dimensions() {
    let read = |unpaired: &[&str]| MismatchedRead {
        variable: "births".to_string(),
        unpaired: unpaired.iter().map(|d| d.to_string()).collect(),
    };
    let reason = |read| {
        crate::common::EquationError::mismatched_read(0, 0, read)
            .details
            .unwrap()
    };
    assert_eq!(
        reason(read(&["region"])),
        "'births' is arrayed over 'region', which the variable reading it is not"
    );
    assert_eq!(
        reason(read(&["region", "age"])),
        "'births' is arrayed over 'region' and 'age', which the variable reading it is not"
    );
    assert_eq!(
        reason(read(&[])),
        "'births' is subscripted on more dimensions than it has"
    );
}
