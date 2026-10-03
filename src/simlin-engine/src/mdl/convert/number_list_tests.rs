// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A number list (or a tabbed array) is one of a variable's equations like
//! any other: it defines the elements its left-hand side names, and a
//! variable may have several, or a list beside ordinary equations. Vensim
//! writes an array of three or more dimensions as one list per element of the
//! leading dimensions, since a list holds at most two.

use super::convert_mdl;
use crate::datamodel::{Equation, Project};

const END: &str = "\\\\\\---///\n";

fn convert(equations: &str) -> Project {
    convert_mdl(&format!("{equations}{END}")).expect("the model converts")
}

/// The dimensions of `name` and each element's equation, by lowercased key.
fn arrayed(project: &Project, name: &str) -> (Vec<String>, Vec<(String, String)>) {
    let var = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == name)
        .unwrap_or_else(|| panic!("no variable {name}"));
    match var.get_equation() {
        Some(Equation::Arrayed(dims, elements, default, has_default)) => {
            assert!(default.is_none() && !has_default, "{name} has a default");
            let mut slots: Vec<(String, String)> = elements
                .iter()
                .map(|(key, eqn, initial, gf)| {
                    assert!(initial.is_none() && gf.is_none(), "{name}[{key}]");
                    (key.to_lowercase(), eqn.clone())
                })
                .collect();
            slots.sort();
            (dims.clone(), slots)
        }
        other => panic!("{name} is not arrayed: {other:?}"),
    }
}

fn slots(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

const DIMS: &str = "DimA: A1, A2 ~~|\nDimB: B1, B2 ~~|\nDimC: C1, C2 ~~|\n";

#[test]
fn each_number_list_of_a_variable_defines_its_elements() {
    let project = convert(&format!(
        "{DIMS}x[A1, DimB] = 1, 2 ~~|\nx[A2, DimB] = 3, 4 ~~|\n"
    ));
    let (dims, elements) = arrayed(&project, "x");
    assert_eq!(dims, ["DimA", "DimB"]);
    assert_eq!(
        elements,
        slots(&[
            ("a1,b1", "1"),
            ("a1,b2", "2"),
            ("a2,b1", "3"),
            ("a2,b2", "4")
        ])
    );
}

#[test]
fn a_three_dimensional_array_is_one_list_per_leading_element() {
    // Vensim's own spelling of a 3-D constant, whichever axis is pinned.
    let project = convert(&format!(
        "{DIMS}x[DimA, DimB, C1] = 1, 2; 3, 4; ~~|\nx[DimA, DimB, C2] = 5, 6; 7, 8; ~~|\n\
         y[A1, DimB, DimC] = 1, 2; 3, 4; ~~|\ny[A2, DimB, DimC] = 5, 6; 7, 8; ~~|\n"
    ));
    let (dims, elements) = arrayed(&project, "x");
    assert_eq!(dims, ["DimA", "DimB", "DimC"]);
    assert_eq!(
        elements,
        slots(&[
            ("a1,b1,c1", "1"),
            ("a1,b1,c2", "5"),
            ("a1,b2,c1", "2"),
            ("a1,b2,c2", "6"),
            ("a2,b1,c1", "3"),
            ("a2,b1,c2", "7"),
            ("a2,b2,c1", "4"),
            ("a2,b2,c2", "8"),
        ])
    );
    let (dims, elements) = arrayed(&project, "y");
    assert_eq!(dims, ["DimA", "DimB", "DimC"]);
    assert_eq!(
        elements,
        slots(&[
            ("a1,b1,c1", "1"),
            ("a1,b1,c2", "2"),
            ("a1,b2,c1", "3"),
            ("a1,b2,c2", "4"),
            ("a2,b1,c1", "5"),
            ("a2,b1,c2", "6"),
            ("a2,b2,c1", "7"),
            ("a2,b2,c2", "8"),
        ])
    );
}

#[test]
fn a_number_list_and_an_ordinary_equation_define_one_variable() {
    let project = convert(&format!(
        "{DIMS}k = 7 ~~|\nx[A1, DimB] = 1, 2 ~~|\nx[A2, DimB] = k * 2 ~~|\n"
    ));
    let (dims, elements) = arrayed(&project, "x");
    assert_eq!(dims, ["DimA", "DimB"]);
    assert_eq!(
        elements,
        slots(&[
            ("a1,b1", "1"),
            ("a1,b2", "2"),
            ("a2,b1", "k * 2"),
            ("a2,b2", "k * 2"),
        ])
    );
}

#[test]
fn each_tabbed_array_of_a_variable_defines_its_elements() {
    let project = convert(&format!(
        "{DIMS}x[A1, DimB] = TABBED ARRAY(1\t2) ~~|\nx[A2, DimB] = TABBED ARRAY(3\t4) ~~|\n"
    ));
    let (_, elements) = arrayed(&project, "x");
    assert_eq!(
        elements,
        slots(&[
            ("a1,b1", "1"),
            ("a1,b2", "2"),
            ("a2,b1", "3"),
            ("a2,b2", "4")
        ])
    );
}

#[test]
fn a_list_of_equal_numbers_stays_one_equation_per_element() {
    // The list says each element's value separately; that the values agree
    // today is a property of the data, as it is for an arrayed GET DIRECT.
    let project = convert(&format!("{DIMS}x[DimB] = 5, 5 ~~|\n"));
    let (dims, elements) = arrayed(&project, "x");
    assert_eq!(dims, ["DimB"]);
    assert_eq!(elements, slots(&[("b1", "5"), ("b2", "5")]));
}

#[test]
fn a_listed_element_takes_its_dimension_as_any_equation_does() {
    // `A1` is an element of both; a list's pinned element takes the
    // dimension an ordinary equation's does (the smallest that holds it),
    // whichever is declared first.
    let project = convert(
        "SubA: A1, A2 ~~|\nDimA: A1, A2, A3 ~~|\nDimB: B1, B2 ~~|\n\
         x[A1, DimB] = 1, 2 ~~|\ny[A1, DimB] = Time ~~|\n",
    );
    let (list_dims, _) = arrayed(&project, "x");
    let (equation_dims, _) = arrayed(&project, "y");
    assert_eq!(list_dims, ["SubA", "DimB"]);
    assert_eq!(list_dims, equation_dims);
}

#[test]
fn a_list_over_a_dimension_keeps_that_dimension() {
    // A subrange and a dimension holding the same elements in another order
    // are named by the left-hand side, and stay.
    let project = convert(
        "DimA: A1, A2, A3 ~~|\nSubA: A2, A3 ~~|\nDimX: A3, A1, A2 ~~|\n\
         u[SubA] = 1, 2 ~~|\nv[DimX] = 30, 10, 20 ~~|\n",
    );
    let (dims, elements) = arrayed(&project, "u");
    assert_eq!(dims, ["SubA"]);
    assert_eq!(elements, slots(&[("a2", "1"), ("a3", "2")]));
    let (dims, elements) = arrayed(&project, "v");
    assert_eq!(dims, ["DimX"]);
    assert_eq!(elements, slots(&[("a1", "10"), ("a2", "20"), ("a3", "30")]));
}

#[test]
fn a_list_of_the_wrong_length_is_refused() {
    // Which number a list of the wrong length gives an element is a guess,
    // and a guess is a wrong answer read silently. Whether Vensim refuses
    // such a model is unverified.
    let error = convert_mdl(&format!("{DIMS}x[DimB] = 1, 2, 3 ~~|\n{END}"))
        .expect_err("a list of 3 numbers over 2 elements is refused");
    assert!(error.to_string().contains("'x'"), "{error}");
}

#[test]
fn a_list_with_an_except_defines_the_elements_it_does_not_except() {
    // The list's numbers go to the elements the :EXCEPT: leaves, in order;
    // it has no default, since no equation text is the default.
    for list in ["1, 2", "TABBED ARRAY(1\t2)"] {
        let project = convert(&format!(
            "DimA: A1, A2, A3 ~~|\nx[DimA] :EXCEPT: [A1] = {list} ~~|\nx[A1] = 9 ~~|\n"
        ));
        let (dims, elements) = arrayed(&project, "x");
        assert_eq!(dims, ["DimA"], "{list}");
        assert_eq!(
            elements,
            slots(&[("a1", "9"), ("a2", "1"), ("a3", "2")]),
            "{list}"
        );
    }
}

#[test]
fn a_list_no_element_takes_numbers_from_is_refused() {
    let error = convert_mdl(&format!("x = 4, 5 ~~|\n{END}")).expect_err("x lists two numbers");
    assert!(error.to_string().contains("'x'"), "{error}");
}
