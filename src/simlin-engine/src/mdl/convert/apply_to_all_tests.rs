// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A single apply-to-all MDL equation must import as an `Equation::ApplyToAll`,
//! not as N identical per-element slots.
//!
//! `build_variable_with_elements` expands every subscripted LHS to the cartesian
//! product of its subscripts' elements, so `y[DimA] = <rhs>` became three
//! `Arrayed` slots all carrying the SAME `<rhs>` text. Where the right-hand side
//! mentions the dimension itself -- Vensim's `DimA` is the element's 1-based
//! position -- that text has no active apply-to-all dimension to resolve against
//! once it sits in a per-element slot, so the variable failed to compile. Any
//! dimension-position arithmetic hits it, not just the `VECTOR ELM MAP` offset
//! that surfaced it.
//!
//! The rule implemented is the one the MDL equivalence harness already applies
//! when comparing against xmutil (`mdl_equivalence::normalize_equation`): slots
//! that are identical in equation text, initial text and graphical function ARE
//! an apply-to-all, so collapsing them loses nothing. The tests below are in two
//! halves -- shapes that must collapse, and shapes that must NOT, each named for
//! the reason it is per-element.

use crate::datamodel::Equation;

/// Import `mdl` and return the named variable's equation.
fn equation_of(mdl: &str, ident: &str) -> Equation {
    let project = super::convert_mdl(mdl).expect("conversion should succeed");
    project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == ident)
        .unwrap_or_else(|| panic!("no variable {ident}"))
        .get_equation()
        .cloned()
        .unwrap_or_else(|| panic!("{ident} has no equation"))
}

/// Import `mdl`, simulate, and return the named element series' final values in
/// the order given.
fn finals(mdl: &str, keys: &[&str]) -> Vec<f64> {
    let project = crate::open_vensim(mdl).expect("import");
    let tp = crate::test_common::TestProject::from_datamodel(project);
    let all = tp.run_vm().expect("model must compile and run");
    keys.iter()
        .map(|k| {
            *all.get(*k)
                .unwrap_or_else(|| panic!("no series {k}"))
                .last()
                .expect("empty series")
        })
        .collect()
}

const PREAMBLE: &str = "{UTF-8}\nDimA: A1, A2, A3 ~~|\nDimB: B1, B2 ~~|\n\
     DimX: one, two, three, four, five ~~|\nSubX: two, three, four ~~|\n\
     x[DimX] = 1, 2, 3, 4, 5 ~~|\n";
const EPILOGUE: &str = "INITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\nTIME STEP = 1 ~~|\n\
     SAVEPER = TIME STEP ~~|\n";

fn model(body: &str) -> String {
    format!("{PREAMBLE}{body}{EPILOGUE}")
}

// ===========================================================================
// Must collapse to ApplyToAll
// ===========================================================================

/// The shape that surfaced this: `vector.mdl`'s `y`, spelled byte-identically.
///
/// The XMILE twin of this equation has always compiled and produced `3,4,5`
/// (real-Vensim ground truth, `test/sdeverywhere/models/vector/vector.dat`);
/// only the MDL import path failed, which is why the corpus never caught it --
/// the `vector` fixture runs the XMILE file.
#[test]
fn a_dimension_position_offset_imports_as_apply_to_all() {
    let mdl = model("y[DimA] = VECTOR ELM MAP(x[three], (DimA - 1)) ~~|\n");
    assert!(
        matches!(equation_of(&mdl, "y"), Equation::ApplyToAll(_, _)),
        "a single apply-to-all equation must not be exploded per element"
    );
    assert_eq!(
        finals(&mdl, &["y[a1]", "y[a2]", "y[a3]"]),
        vec![3.0, 4.0, 5.0]
    );
}

/// The same defect with no builtin involved: a dimension position used as plain
/// arithmetic. This is the row that shows the bug is about dimension references,
/// not about `VECTOR ELM MAP`.
#[test]
fn a_dimension_position_in_plain_arithmetic_imports_as_apply_to_all() {
    let mdl = model("z[DimA] = 10 * (DimA - 1) ~~|\n");
    assert!(matches!(equation_of(&mdl, "z"), Equation::ApplyToAll(_, _)));
    assert_eq!(
        finals(&mdl, &["z[a1]", "z[a2]", "z[a3]"]),
        vec![0.0, 10.0, 20.0]
    );
}

/// Two dimensions at once: each position must resolve against its own axis.
#[test]
fn dimension_positions_on_two_axes_import_as_apply_to_all() {
    let mdl = model("m[DimA,DimB] = 10 * (DimA - 1) + (DimB - 1) ~~|\n");
    assert!(matches!(equation_of(&mdl, "m"), Equation::ApplyToAll(_, _)));
    assert_eq!(
        finals(
            &mdl,
            &[
                "m[a1,b1]", "m[a1,b2]", "m[a2,b1]", "m[a2,b2]", "m[a3,b1]", "m[a3,b2]"
            ]
        ),
        vec![0.0, 1.0, 10.0, 11.0, 20.0, 21.0]
    );
}

/// A plain apply-to-all with no dimension reference at all. It compiled before
/// (the repeated text resolves fine per element), so this row is about the
/// STRUCTURE: the faithful translation of one MDL equation is one equation, and
/// exploding it bloats every imported arrayed model.
#[test]
fn an_ordinary_apply_to_all_equation_stays_one_equation() {
    let mdl = model("c[DimA] = x[three] * 2 ~~|\n");
    match equation_of(&mdl, "c") {
        Equation::ApplyToAll(dims, eqn) => {
            assert_eq!(dims, vec!["DimA".to_string()]);
            assert!(eqn.contains("three"), "unexpected equation text: {eqn}");
        }
        other => panic!("expected ApplyToAll, got {other:?}"),
    }
    assert_eq!(
        finals(&mdl, &["c[a1]", "c[a2]", "c[a3]"]),
        vec![6.0, 6.0, 6.0]
    );
}

/// A subrange LHS covering its own dimension collapses too -- the elements are
/// the full extent of `SubX`, so nothing is lost.
#[test]
fn a_subrange_apply_to_all_collapses_over_its_own_dimension() {
    let mdl = model("s[SubX] = 10 * (SubX - 1) ~~|\n");
    assert!(matches!(equation_of(&mdl, "s"), Equation::ApplyToAll(_, _)));
}

// ===========================================================================
// Must NOT collapse -- each row names why its slots genuinely differ
// ===========================================================================

/// Element-specific equations: three different right-hand sides.
#[test]
fn per_element_equations_stay_arrayed() {
    let mdl = model("p[A1] = 1 ~~|\np[A2] = 2 ~~|\np[A3] = 3 ~~|\n");
    match equation_of(&mdl, "p") {
        Equation::Arrayed(_, elements, _, _) => assert_eq!(elements.len(), 3),
        other => panic!("expected Arrayed, got {other:?}"),
    }
}

/// An `:EXCEPT:` equation: the excepted element differs from the default, and
/// the `Arrayed` form is what carries the default text.
#[test]
fn an_except_equation_stays_arrayed() {
    let mdl = model("e[DimA] :EXCEPT: [A2] = 7 ~~|\ne[A2] = 99 ~~|\n");
    match equation_of(&mdl, "e") {
        Equation::Arrayed(_, elements, _, _) => {
            assert!(!elements.is_empty(), "EXCEPT must keep its element slots")
        }
        other => panic!("expected Arrayed, got {other:?}"),
    }
}

/// A subscripted numeric list: each element has its own value, so the slots
/// differ by construction. This is the commonest arrayed shape in the corpus and
/// the one a careless collapse would flatten to its first value.
#[test]
fn a_numeric_element_list_stays_arrayed() {
    let mdl = model("n[DimA] = 4, 5, 6 ~~|\n");
    match equation_of(&mdl, "n") {
        Equation::Arrayed(_, elements, _, _) => assert_eq!(elements.len(), 3),
        other => panic!("expected Arrayed, got {other:?}"),
    }
    assert_eq!(
        finals(&mdl, &["n[a1]", "n[a2]", "n[a3]"]),
        vec![4.0, 5.0, 6.0]
    );
}

/// A single apply-to-all equation carrying a GRAPHICAL FUNCTION stays arrayed.
///
/// This is the one clause of `slots_are_one_apply_to_all` that blocks a shape
/// the source-shape clauses let through: `z[DimA] = WITH LOOKUP(...)` is ONE
/// equation, has no `:EXCEPT:`, no default, no INITIAL and no `GET DIRECT`, and
/// covers `DimA` fully -- so `single_apply_to_all` is true and only `gf.is_none()`
/// stands between it and a collapse. `Equation::ApplyToAll` is `(dims, equation)`
/// and has nowhere to put a table, and the table does NOT ride the variable here
/// (`Aux::gf` is `None`; it lives in the slots), so collapsing DELETES it:
/// measured, dropping that one condition turns this into
/// `ApplyToAll(["DimA"], "TIME")` -- a variable that was a lookup becomes plain
/// `= TIME`, with the whole suite green.
///
/// The bare arrayed lookup table is the same exposure through a different MDL
/// form, and it degrades further: its slots hold an EMPTY equation plus the
/// table, so a collapse yields `ApplyToAll(["DimA"], "")` -- an empty equation
/// with no data at all.
#[test]
fn an_arrayed_graphical_function_stays_arrayed() {
    let with_lookup = model("z[DimA] = WITH LOOKUP(Time, ((0,0.5),(1,1.36),(2,0.8))) ~~|\n");
    match equation_of(&with_lookup, "z") {
        Equation::Arrayed(_, elements, _, _) => {
            assert_eq!(elements.len(), 3);
            assert!(
                elements
                    .iter()
                    .all(|(_, eq, _, gf)| eq == "TIME" && gf.is_some()),
                "every slot keeps the WITH LOOKUP table"
            );
        }
        other => panic!("expected Arrayed, got {other:?}"),
    }

    let bare_table = model(
        "t[DimA]( [(0,0)-(10,10)],(0,0),(5,5),(10,10) ) ~~|\nw[DimA] = LOOKUP(t[DimA], 5) ~~|\n",
    );
    match equation_of(&bare_table, "t") {
        Equation::Arrayed(_, elements, _, _) => {
            assert_eq!(elements.len(), 3);
            assert!(
                elements
                    .iter()
                    .all(|(_, eq, _, gf)| eq.is_empty() && gf.is_some()),
                "a lookup-only holder keeps its table in every slot"
            );
        }
        other => panic!("expected Arrayed, got {other:?}"),
    }
    // Its consumer is an ordinary apply-to-all and DOES collapse, which is what
    // makes the pair a discriminator rather than a blanket "tables stay arrayed".
    match equation_of(&bare_table, "w") {
        Equation::ApplyToAll(_, eq) => assert_eq!(eq, "LOOKUP(t[DimA], 5)"),
        other => panic!("expected ApplyToAll, got {other:?}"),
    }
}

/// Two apply-to-all equations over DIFFERENT subranges of one dimension --
/// legal MDL, and the shape that says the collapse keys on the SOURCE equation
/// count rather than on the expanded slots.
///
/// `q[SubLo] = 7` + `q[SubHi] = 7` jointly cover `DimX` and every expanded slot
/// agrees, so slot agreement and full coverage BOTH hold; only
/// `expanded_eqs.len() == 1` refuses it. Two equations are two equations, and
/// merging them would rewrite the model's structure -- the sibling row with
/// different right-hand sides is the same shape where merging would also be
/// numerically wrong, and it must land per element.
#[test]
fn apply_to_all_equations_over_different_subranges_stay_arrayed() {
    let same = model(
        "SubLo: one, two ~~|\nSubHi: three, four, five ~~|\nq[SubLo] = 7 ~~|\nq[SubHi] = 7 ~~|\n",
    );
    match equation_of(&same, "q") {
        Equation::Arrayed(dims, elements, _, _) => {
            assert_eq!(
                dims,
                ["DimX"],
                "the two subranges normalize to their parent"
            );
            assert_eq!(elements.len(), 5);
        }
        other => panic!("expected Arrayed, got {other:?}"),
    }

    let differing = model(
        "SubLo: one, two ~~|\nSubHi: three, four, five ~~|\nr[SubLo] = 7 ~~|\nr[SubHi] = 9 ~~|\n",
    );
    match equation_of(&differing, "r") {
        Equation::Arrayed(_, elements, _, _) => assert_eq!(elements.len(), 5),
        other => panic!("expected Arrayed, got {other:?}"),
    }
    assert_eq!(
        finals(
            &differing,
            &["r[one]", "r[two]", "r[three]", "r[four]", "r[five]"]
        ),
        vec![7.0, 7.0, 9.0, 9.0, 9.0]
    );
}

// ===========================================================================
// Apply-to-all intent is read off the SOURCE SPELLING, not off coverage
// ===========================================================================

/// A SINGLETON dimension makes coverage arithmetic vacuous: with `DimA: a1`,
/// an element-specific `x[a1] = 5` produces one slot, which IS the whole
/// cartesian product. Coverage alone therefore cannot tell it from the
/// apply-to-all `x[DimA] = 5`, and the collapse silently rewrote the first into
/// the second.
///
/// Two things are lost by that. The writer re-renders it as `x[DimA] = 5`, so
/// the as-written intent does not survive a round trip; and a later dimension
/// edit in Simlin (adding `a2`) silently applies the equation to an element the
/// MDL source never defined.
///
/// The gate therefore asks the SOURCE: every LHS subscript must name a
/// dimension. That is the property "this equation is apply-to-all" actually
/// means, and unlike coverage it does not depend on how many elements the
/// dimension happens to have.
#[test]
fn an_element_specific_equation_over_a_singleton_dimension_stays_arrayed() {
    let mdl = "{UTF-8}\nDimA: a1 ~~|\nx[a1] = 5 ~~|\n\
        INITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|\n";
    match equation_of(mdl, "x") {
        Equation::Arrayed(dims, elements, _, _) => {
            assert_eq!(dims, vec!["DimA".to_string()]);
            assert_eq!(
                elements.len(),
                1,
                "the a1 slot must survive: the source named the ELEMENT"
            );
            assert_eq!(elements[0].0, "a1");
        }
        other => panic!("expected Arrayed (element-specific source), got {other:?}"),
    }
}

/// The control that keeps the rule honest: over the SAME one-element dimension,
/// the apply-to-all spelling must still collapse. If it did not, the fix would
/// just be "never collapse a singleton", which is a different (and wrong) rule.
#[test]
fn an_apply_to_all_equation_over_a_singleton_dimension_still_collapses() {
    let mdl = "{UTF-8}\nDimA: a1 ~~|\nx[DimA] = 5 ~~|\n\
        INITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|\n";
    match equation_of(mdl, "x") {
        Equation::ApplyToAll(dims, eq) => {
            assert_eq!(dims, vec!["DimA".to_string()]);
            assert_eq!(eq, "5");
        }
        other => panic!("expected ApplyToAll (range source), got {other:?}"),
    }
}

/// A MIXED spelling: one axis names a dimension, the other pins an element.
/// The element subscript blocks the collapse BY SPELLING.
///
/// Coverage would block this one too (two slots against a 3x2 product), so it
/// is not on its own evidence for the spelling clause -- the singleton row above
/// is. It is here because it is the shape a reader expects to find, and because
/// it pins that the two axes are judged independently rather than by whether
/// ANY axis is a dimension.
#[test]
fn an_element_pinned_axis_blocks_the_collapse_by_spelling() {
    let mdl = model("y[DimA,B2] = 7 ~~|\n");
    match equation_of(&mdl, "y") {
        Equation::Arrayed(_, elements, _, _) => {
            assert_eq!(elements.len(), 3, "one slot per DimA element, pinned at B2");
            assert!(elements.iter().all(|(key, _, _, _)| key.ends_with(",B2")));
        }
        other => panic!("expected Arrayed (element-pinned axis), got {other:?}"),
    }
}
