// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The dimension an arrayed variable is over, axis by axis, as the reader
//! decides it from its equations (`ConversionContext::axis_dimension`).

use crate::datamodel::Equation;
use crate::mdl::parse_mdl;

const CONTROL: &str =
    "INITIAL TIME = 0 ~~|\nFINAL TIME = 2 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|\n";

/// `name`'s dimensions in the project `source` reads as.
fn dims_of(source: &str, name: &str) -> Vec<String> {
    let project = parse_mdl(&format!("{source}{CONTROL}")).expect("the source reads");
    let var = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == name)
        .unwrap_or_else(|| panic!("no variable {name}"));
    match var.get_equation() {
        Some(Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, _, _, _)) => dims.clone(),
        other => panic!("{name} is not arrayed: {other:?}"),
    }
}

/// Each row: the dimensions declared, the left-hand sides defining `x`, and
/// the dimensions `x` is over.
#[test]
fn an_axis_is_over_the_dimension_of_exactly_the_elements_defined_on_it() {
    let rows: &[(&str, &str, &[&str])] = &[
        // A named dimension of exactly the defined elements.
        (
            "DimA: A1, A2, A3 ~~|\nSubA: A2, A3 ~~|\n",
            "x[SubA] = 1 ~~|\nx[A3] = 2 ~~|\n",
            &["SubA"],
        ),
        // The elements alone: the smallest dimension holding them, of two
        // that size the first declared.
        (
            "DimA: A1, A2, A3 ~~|\nSubA: A2, A3 ~~|\n",
            "x[A2] = 1 ~~|\nx[A3] = 2 ~~|\n",
            &["SubA"],
        ),
        (
            "DimX: A1, A2 ~~|\nDimY: A1, A2 ~~|\n",
            "x[A1] = 1 ~~|\nx[A2] = 2 ~~|\n",
            &["DimX"],
        ),
        // An :EXCEPT: equation over a dimension whose excepted element no
        // equation defines: the elements defined are a subrange's.
        (
            "DimA: A1, A2, A3 ~~|\nSubA: A1, A2 ~~|\n",
            "x[DimA] :EXCEPT: [A3] = 1 ~~|\nx[A1] = 2 ~~|\n",
            &["SubA"],
        ),
        // Subranges of one parent that together define all of it.
        (
            "DimA: A1, A2, A3 ~~|\nSubA: A1, A2 ~~|\nSubB: A3 ~~|\n",
            "x[SubA] = 1 ~~|\nx[SubB] = 2 ~~|\n",
            &["DimA"],
        ),
        // Elements whose largest owners differ (`dead` is also in the larger
        // DisplaySeverity): the dimension holding all of them.
        (
            "Display: healthy, mild, dead, extra ~~|\nGraph: sick, dead, healthy ~~|\n",
            "x[dead] = 1 ~~|\nx[healthy] = 2 ~~|\nx[sick] = 3 ~~|\n",
            &["Graph"],
        ),
    ];
    for (dims, equations, expected) in rows {
        let source = format!("{dims}{equations}");
        assert_eq!(dims_of(&source, "x"), *expected, "{source}");
    }
}

#[test]
fn an_axis_whose_elements_no_dimension_holds_is_built_from_one_equation() {
    // A1 and B1 are in no one dimension: no dimension is the variable's
    // there, and it is not built element by element.
    let source = "DimA: A1, A2 ~~|\nDimB: B1, B2 ~~|\nx[A1] = 1 ~~|\nx[B1] = 2 ~~|\n";
    let project = parse_mdl(&format!("{source}{CONTROL}")).expect("the source reads");
    let x = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "x")
        .expect("x reads");
    assert!(
        !matches!(x.get_equation(), Some(Equation::Arrayed(_, slots, _, _)) if slots.len() == 2),
        "{:?}",
        x.get_equation()
    );
}

#[test]
fn an_element_an_except_equation_excepts_and_no_equation_defines_is_undefined() {
    // A3 is excepted and nothing defines it: the default does not fill it.
    let source =
        format!("DimA: A1, A2, A3 ~~|\nx[DimA] :EXCEPT: [A3] = 1 ~~|\nx[A1] = 2 ~~|\n{CONTROL}");
    let project = parse_mdl(&source).expect("the source reads");
    let x = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "x")
        .and_then(|v| v.get_equation().cloned());
    let Some(Equation::Arrayed(dims, slots, _, applies)) = x else {
        panic!("x is arrayed: {x:?}");
    };
    assert_eq!(dims, ["DimA"]);
    let keys: Vec<&str> = slots.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(keys, ["a1", "a2"]);
    assert!(!applies, "the default fills no element");
}
