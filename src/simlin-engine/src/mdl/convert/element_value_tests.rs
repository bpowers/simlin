// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An element's name in an expression is a number: Vensim reads it as the
//! element's position (`IF THEN ELSE(DimA = B, 1, 0)`, whose output is
//! checked in at `test/test-models/tests/conditional_subscripts`), and the
//! reader writes it as `Dimension.Element`, which the engine reads as that
//! position.

use crate::datamodel::Equation;
use crate::mdl::parse_mdl;

const CONTROL: &str =
    "INITIAL TIME = 0 ~~|\nFINAL TIME = 2 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|\n";

fn equation_of(source: &str, name: &str) -> String {
    let project = parse_mdl(&format!("{source}{CONTROL}")).expect("the source reads");
    let var = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == name)
        .unwrap_or_else(|| panic!("no variable {name}"));
    match var.get_equation() {
        Some(Equation::Scalar(eqn)) => eqn.clone(),
        other => panic!("{name} is not scalar: {other:?}"),
    }
}

#[test]
fn an_element_name_is_its_position_in_the_dimension_that_owns_it() {
    // Rows: an element of one dimension; an element a subrange holds too,
    // which is the position in the dimension that owns it, the largest
    // (`element_owners`; which dimension Vensim takes is unverified); and a
    // name that is an element and a variable, which is the variable.
    let rows = [
        ("DimA: A, B, C ~~|\nx = B ~~|\n", "DimA.B"),
        (
            "DimA: A1, A2, A3 ~~|\nSubA: A2, A3 ~~|\nx = A2 ~~|\n",
            "DimA.A2",
        ),
        ("DimA: A, B, C ~~|\nB = 4 ~~|\nx = B ~~|\n", "B"),
    ];
    for (source, equation) in rows {
        assert_eq!(equation_of(source, "x"), equation, "{source}");
    }
}

#[test]
fn a_reference_with_a_run_of_spaces_names_the_variable_its_definition_does() {
    // A definition and a reference spell one name the same way, whatever
    // run of spaces the file writes between its words. What Vensim does with
    // repeated spaces inside a name is unverified.
    let source = "a  b = 3 ~~|\nc = a  b * 2 ~~|\nd = a b + 1 ~~|\n";
    assert_eq!(equation_of(source, "c"), "a_b * 2");
    assert_eq!(equation_of(source, "d"), "a_b + 1");
}

#[test]
fn a_file_asking_for_more_elements_than_the_reader_reads_is_refused() {
    // Rows: a numeric range, and a left-hand side's product of subscripts.
    for source in [
        "DimA: (A1-A4000000000) ~~|\nx[DimA] = 1 ~~|\n",
        "D1: (a1-a3000) ~~|\nD2: (b1-b3000) ~~|\nx[D1, D2] = 1 ~~|\n",
    ] {
        let error = parse_mdl(&format!("{source}{CONTROL}")).expect_err("the file is refused");
        assert!(error.to_string().contains("more than"), "{error}");
    }
}
