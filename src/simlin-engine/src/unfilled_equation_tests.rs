// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Contract for the `UnfilledEquation` advisory: a variable whose equation is
//! nothing but the NaN literal has no usable equation, and the engine says so
//! before the model is ever run. What the shape means and why it earns a
//! diagnostic is on `common::ErrorCode::UnfilledEquation` and `crate::float`.
//!
//! Two levels are pinned here, because neither implies the other: the pure
//! per-shape decision (`variable::unfilled_arms` over every `datamodel::Equation`
//! variant) and the end-to-end path from a real `.mdl`/`.xmile` file to the
//! accumulated diagnostic. The end-to-end half is the one that matters -- the
//! shape's whole reason for existing is what the MDL importer produces, and a
//! fixture that hand-built a `datamodel::Equation::Scalar("NAN")` would prove
//! nothing about that.

use std::collections::{BTreeSet, HashMap};

use crate::ast::{Ast, Expr0};
use crate::common::{CanonicalElementName, ErrorCode};
use crate::datamodel;
use crate::db::{
    Diagnostic, DiagnosticError, DiagnosticSeverity, SimlinDb, collect_all_diagnostics,
    compile_project_incremental, sync_from_datamodel_incremental,
};
use crate::variable::{UnfilledArms, is_nan_literal, unfilled_arms};

// ---------------------------------------------------------------------------
// The atom: is this equation text nothing but the NaN literal?
// ---------------------------------------------------------------------------

/// Every spelling the LEXER accepts for a lone `nan` token must be recognized.
/// Case and surrounding whitespace are not the modeller's statement of intent,
/// and both spellings occur in the corpus: the MDL importer writes `NAN` for
/// `A FUNCTION OF(...)` while the MDL writer prints a `Const` NaN back as `NaN`,
/// so a round-tripped placeholder changes case (`keyword_ident_tests`).
#[test]
fn the_atom_accepts_every_lone_nan_spelling() {
    for text in ["nan", "NAN", "NaN", "nAn", " nan ", "\t NAN\n"] {
        assert!(
            is_nan_literal(text),
            "{text:?} is a lone NaN literal and must be recognized"
        );
    }
}

/// "Nothing but the NaN literal" is exactly one token. A NaN *inside* a larger
/// expression is a different claim with a different remedy -- a modeller
/// deliberately using NaN as an out-of-range sentinel (`crate::float`) -- and is
/// none of this diagnostic's business. A name that merely starts with `nan`
/// lexes as an identifier and is an ordinary reference.
///
/// Two rows are dividends of deciding this by LEXING that a string comparison
/// would have got wrong, so they are the ones to keep if the list is ever
/// trimmed. `"nan"` QUOTED is a reference to a variable legally named `nan`
/// (`keyword_ident_tests`): the lexer yields an `Ident`, never `Token::Nan`, so
/// a real equation is not mistaken for a missing one. And the Stella
/// input-port idiom -- 31 instances in this repo -- is a brace COMMENT, which
/// lexes to zero tokens, so the shape that actually occurs in the wild produces
/// no false positive.
#[test]
fn the_atom_rejects_everything_that_is_not_a_lone_nan() {
    for text in [
        "",
        "   ",
        "3",
        "nan + 0",
        "0 * nan",
        "(nan)",
        "nancy",
        "0/0",
        "IF x > 0 THEN 1 ELSE nan",
        "\"nan\"",
        "{Enter equation for use when not hooked up to other models}",
    ] {
        assert!(
            !is_nan_literal(text),
            "{text:?} is not a lone NaN literal and must not be recognized"
        );
    }
}

// ---------------------------------------------------------------------------
// The per-shape decision, over the PARSED equation
// ---------------------------------------------------------------------------
//
// The classifier now takes `Ast<Expr0>`, so these fixtures are parsed ASTs. That
// is deliberate rather than incidental: the four stages between an equation as
// written and the arms the compiler evaluates (empty-arm drop, last-wins over
// duplicate canonical subscripts, dimension resolution, declared-slot lookup)
// are the parser's and the compiler's, and every review finding on this
// diagnostic came from re-deriving one of them by hand. The table pins the
// verdict given a parsed equation; the stages that produce it are pinned by the
// end-to-end tests further down, which go through a real reader.

/// The `Ast` shape `ast` is, as an EXHAUSTIVE match with no catch-all arm: a new
/// shape is a compile error right here, which is what makes the decision table's
/// coverage checkable rather than asserted.
fn equation_shape(ast: &Ast<Expr0>) -> &'static str {
    match ast {
        Ast::Scalar(_) => "Scalar",
        Ast::ApplyToAll(..) => "ApplyToAll",
        Ast::Arrayed(..) => "Arrayed",
    }
}

/// Counted from `ast::Ast`: three variants, three shapes.
const EQUATION_SHAPES: [&str; 3] = ["Scalar", "ApplyToAll", "Arrayed"];

/// Parse one arm's or formula's text the way `variable::parse_equation` does.
/// `None` for text with no expression at all, which is exactly what that
/// function drops before collecting its map.
fn expr(text: &str) -> Option<Expr0> {
    Expr0::new(text, crate::lexer::LexerType::Equation).expect("fixture must parse")
}

fn scalar_ast(text: &str) -> Ast<Expr0> {
    Ast::Scalar(expr(text).expect("fixture must have an expression"))
}

fn a2a_ast(text: &str) -> Ast<Expr0> {
    Ast::ApplyToAll(
        test_dims(&["a", "b"]),
        expr(text).expect("fixture must have an expression"),
    )
}

/// Resolved dimensions declaring exactly `elements`, built through the same
/// `From<&datamodel::Dimension>` conversion the parser uses.
fn test_dims(elements: &[&str]) -> Vec<crate::dimensions::Dimension> {
    vec![crate::dimensions::Dimension::from(
        &datamodel::Dimension::named(
            "d".to_string(),
            elements.iter().map(|e| e.to_string()).collect(),
        ),
    )]
}

/// Build an arrayed `Ast` the way `parse_equation` would: arms keyed
/// canonically with empty ones dropped, over `declared` elements.
fn arrayed_ast(
    declared: &[&str],
    arms: &[(&str, &str)],
    default: Option<&str>,
    apply_default_to_missing: bool,
) -> Ast<Expr0> {
    let elements: HashMap<CanonicalElementName, Expr0> = arms
        .iter()
        .filter_map(|(subscript, text)| {
            expr(text).map(|e| (CanonicalElementName::from_raw(subscript), e))
        })
        .collect();
    Ast::Arrayed(
        test_dims(declared),
        elements,
        default.and_then(expr),
        apply_default_to_missing,
    )
}

// THREE axes decide an `Arrayed` verdict, and every one of them is now a
// property of the parsed fixture rather than a parameter -- which is why
// `decision_cell` can compute the cell from the `Ast` alone.

/// How many of the arms a declared slot SELECTS are unfilled. Four states:
/// `unfilled_arms` tests `unfilled.is_empty()` and
/// `unfilled.len() == slots_with_an_arm`, and those two comparisons distinguish
/// exactly these -- with the no-arms case separate because it satisfies BOTH
/// (`0 == 0`).
///
/// `no-arms` covers every way a variable ends up with no selected arm at all:
/// none written, every arm empty (the parser drops those), or every subscript
/// naming nothing. To the compiled model they are one variable, entirely
/// default- or zero-filled.
const ARM_STATES: [&str; 4] = ["no-arms", "none-unfilled", "some-unfilled", "all-unfilled"];

/// The state of the EXCEPT default: absent, present but not applied to missing
/// slots, applied-and-filled, or applied-and-unfilled.
///
/// A dead default whose text is FILLED is deliberately not a fifth state: the
/// flag is checked before the text is read, so it is the same state as
/// `dead-default` by construction, not by coincidence.
const DEFAULT_STATES: [&str; 4] = [
    "no-default",
    "dead-default",
    "live-filled-default",
    "live-unfilled-default",
];

/// Whether any declared slot falls past all the arms, so the EXCEPT default (or
/// the compiler's silent `0`) decides its value.
const COVERAGE_STATES: [&str; 2] = ["covers-every-slot", "leaves-slots-uncovered"];

/// The cell of the decision space a fixture occupies, computed from the `Ast`
/// itself rather than from a label -- so a mislabelled row cannot fake coverage.
fn decision_cell(ast: &Ast<Expr0>) -> String {
    match ast {
        Ast::Scalar(e) | Ast::ApplyToAll(_, e) => {
            let filled = if crate::variable::is_nan_constant_for_test(e) {
                "unfilled"
            } else {
                "filled"
            };
            format!("{}/{filled}", equation_shape(ast))
        }
        Ast::Arrayed(dims, elements, default, live) => {
            let mut with_arm = 0usize;
            let mut unfilled = 0usize;
            let mut uncovered = false;
            for combination in crate::dimensions::SubscriptIterator::new(dims) {
                let key = CanonicalElementName::from_raw(&combination.join(","));
                match elements.get(&key) {
                    Some(e) => {
                        with_arm += 1;
                        if crate::variable::is_nan_constant_for_test(e) {
                            unfilled += 1;
                        }
                    }
                    None => uncovered = true,
                }
            }
            let arms = if with_arm == 0 {
                "no-arms"
            } else if unfilled == 0 {
                "none-unfilled"
            } else if unfilled == with_arm {
                "all-unfilled"
            } else {
                "some-unfilled"
            };
            let default = match (default.as_ref(), live) {
                (None, _) => "no-default",
                (Some(_), false) => "dead-default",
                (Some(d), true) if crate::variable::is_nan_constant_for_test(d) => {
                    "live-unfilled-default"
                }
                (Some(_), true) => "live-filled-default",
            };
            let coverage = if uncovered {
                "leaves-slots-uncovered"
            } else {
                "covers-every-slot"
            };
            format!("Arrayed/{arms}/{default}/{coverage}")
        }
    }
}

/// Every cell the table must cover.
///
/// **2 (Scalar) + 2 (ApplyToAll) + [(4 arm-states x 4 default-states x 2
/// coverage-states) - 4 impossible] = 32.**
///
/// The four excluded cells are `no-arms` x `covers-every-slot`: with no selected
/// arm, every declared slot is by definition uncovered.
fn expected_cells() -> BTreeSet<String> {
    let mut cells = BTreeSet::new();
    for shape in ["Scalar", "ApplyToAll"] {
        for filled in ["unfilled", "filled"] {
            cells.insert(format!("{shape}/{filled}"));
        }
    }
    for arms in ARM_STATES {
        for default in DEFAULT_STATES {
            for coverage in COVERAGE_STATES {
                if arms == "no-arms" && coverage == "covers-every-slot" {
                    continue;
                }
                cells.insert(format!("Arrayed/{arms}/{default}/{coverage}"));
            }
        }
    }
    cells
}

/// Build the arrayed fixture for one `(arms, default, coverage)` cell.
///
/// Coverage is expressed by DECLARING a third element the arms do not name,
/// rather than by a flag, because that is how a real sparse array expresses it.
fn arrayed_fixture(arms: &str, default: &str, coverage: &str) -> Ast<Expr0> {
    let declared: &[&str] = if coverage == "covers-every-slot" {
        &["a", "b"]
    } else {
        &["a", "b", "c"]
    };
    let arms: &[(&str, &str)] = match arms {
        "no-arms" => &[],
        "none-unfilled" => &[("a", "1"), ("b", "2")],
        "some-unfilled" => &[("a", "1"), ("b", "NAN")],
        "all-unfilled" => &[("a", "NAN"), ("b", "nan")],
        other => panic!("unknown arm state {other:?}"),
    };
    let (default, live) = match default {
        "no-default" => (None, false),
        "dead-default" => (Some("NAN"), false),
        "live-filled-default" => (Some("7"), true),
        "live-unfilled-default" => (Some("nan"), true),
        other => panic!("unknown default state {other:?}"),
    };
    arrayed_ast(declared, arms, default, live)
}

/// The verdict each cell must produce, authored cell by cell.
///
/// Deliberately a flat list of literals rather than a `match` with grouped or
/// wildcard arms: a grouped expectation is a second copy of the rule under test,
/// and would agree with a wrong implementation for the same wrong reason. Every
/// entry was worked out from what the compiled model evaluates, slot by slot.
///
/// Element names are CANONICAL, because that is how the parsed arm map is keyed.
#[allow(clippy::type_complexity)]
fn expected_verdicts() -> Vec<(&'static str, Option<UnfilledArms>)> {
    let whole = || Some(UnfilledArms::Whole);
    let partial = |elements: &[&str], default: bool| {
        Some(UnfilledArms::Partial {
            elements: elements.iter().map(|e| e.to_string()).collect(),
            default,
        })
    };
    vec![
        // -- Scalar / ApplyToAll: one arm, one axis, no coverage question.
        ("Scalar/unfilled", whole()),
        ("Scalar/filled", None),
        ("ApplyToAll/unfilled", whole()),
        ("ApplyToAll/filled", None),
        // -- COVERS EVERY SLOT. No slot reaches the default, so it is dead code
        //    whatever its text says and all four default states agree.
        ("Arrayed/none-unfilled/no-default/covers-every-slot", None),
        ("Arrayed/none-unfilled/dead-default/covers-every-slot", None),
        (
            "Arrayed/none-unfilled/live-filled-default/covers-every-slot",
            None,
        ),
        (
            "Arrayed/none-unfilled/live-unfilled-default/covers-every-slot",
            None,
        ),
        (
            "Arrayed/some-unfilled/no-default/covers-every-slot",
            partial(&["b"], false),
        ),
        (
            "Arrayed/some-unfilled/dead-default/covers-every-slot",
            partial(&["b"], false),
        ),
        (
            "Arrayed/some-unfilled/live-filled-default/covers-every-slot",
            partial(&["b"], false),
        ),
        (
            "Arrayed/some-unfilled/live-unfilled-default/covers-every-slot",
            partial(&["b"], false),
        ),
        ("Arrayed/all-unfilled/no-default/covers-every-slot", whole()),
        (
            "Arrayed/all-unfilled/dead-default/covers-every-slot",
            whole(),
        ),
        (
            "Arrayed/all-unfilled/live-filled-default/covers-every-slot",
            whole(),
        ),
        (
            "Arrayed/all-unfilled/live-unfilled-default/covers-every-slot",
            whole(),
        ),
        // -- LEAVES SLOTS UNCOVERED. The armless slots take the default when it
        //    is live, else the compiler's silent 0.
        ("Arrayed/no-arms/no-default/leaves-slots-uncovered", None),
        ("Arrayed/no-arms/dead-default/leaves-slots-uncovered", None),
        (
            "Arrayed/no-arms/live-filled-default/leaves-slots-uncovered",
            None,
        ),
        (
            "Arrayed/no-arms/live-unfilled-default/leaves-slots-uncovered",
            whole(),
        ),
        (
            "Arrayed/none-unfilled/no-default/leaves-slots-uncovered",
            None,
        ),
        (
            "Arrayed/none-unfilled/dead-default/leaves-slots-uncovered",
            None,
        ),
        (
            "Arrayed/none-unfilled/live-filled-default/leaves-slots-uncovered",
            None,
        ),
        (
            "Arrayed/none-unfilled/live-unfilled-default/leaves-slots-uncovered",
            partial(&[], true),
        ),
        (
            "Arrayed/some-unfilled/no-default/leaves-slots-uncovered",
            partial(&["b"], false),
        ),
        (
            "Arrayed/some-unfilled/dead-default/leaves-slots-uncovered",
            partial(&["b"], false),
        ),
        (
            "Arrayed/some-unfilled/live-filled-default/leaves-slots-uncovered",
            partial(&["b"], false),
        ),
        (
            // The only cell producing a non-empty element list AND `default`.
            "Arrayed/some-unfilled/live-unfilled-default/leaves-slots-uncovered",
            partial(&["b"], true),
        ),
        (
            // Every ARM is unfilled, but the uncovered slot compiles to a finite
            // 0, so the variable is not wholly unfilled.
            "Arrayed/all-unfilled/no-default/leaves-slots-uncovered",
            partial(&["a", "b"], false),
        ),
        (
            "Arrayed/all-unfilled/dead-default/leaves-slots-uncovered",
            partial(&["a", "b"], false),
        ),
        (
            "Arrayed/all-unfilled/live-filled-default/leaves-slots-uncovered",
            partial(&["a", "b"], false),
        ),
        (
            "Arrayed/all-unfilled/live-unfilled-default/leaves-slots-uncovered",
            whole(),
        ),
    ]
}

/// The decision table: one row per cell, fixtures generated from the axes and
/// verdicts taken from [`expected_verdicts`].
#[allow(clippy::type_complexity)]
fn decision_table() -> Vec<(String, Ast<Expr0>, Option<UnfilledArms>)> {
    let verdicts = expected_verdicts();
    let mut rows = Vec::new();
    let mut push = |cell: String, ast: Ast<Expr0>| {
        let expected = verdicts
            .iter()
            .find(|(name, _)| *name == cell)
            .unwrap_or_else(|| panic!("no authored verdict for cell {cell:?}"))
            .1
            .clone();
        // The generated fixture must actually LAND in the cell it is filed
        // under, or the coverage check below would be measuring labels.
        assert_eq!(cell, decision_cell(&ast), "fixture/cell mismatch");
        rows.push((cell, ast, expected));
    };

    for (cell, ast) in [
        ("Scalar/unfilled", scalar_ast("NAN")),
        ("Scalar/filled", scalar_ast("3 * x")),
        ("ApplyToAll/unfilled", a2a_ast("nan")),
        ("ApplyToAll/filled", a2a_ast("3 * x")),
    ] {
        push(cell.to_string(), ast);
    }
    for arms in ARM_STATES {
        for default in DEFAULT_STATES {
            for coverage in COVERAGE_STATES {
                if arms == "no-arms" && coverage == "covers-every-slot" {
                    continue;
                }
                push(
                    format!("Arrayed/{arms}/{default}/{coverage}"),
                    arrayed_fixture(arms, default, coverage),
                );
            }
        }
    }
    rows
}

#[test]
fn the_decision_table_pins_every_row() {
    for (cell, ast, expected) in decision_table() {
        assert_eq!(
            expected,
            unfilled_arms(&ast),
            "decision-table cell {cell:?} disagrees"
        );
    }
}

/// An arm the compiler never selects cannot change ANY verdict.
///
/// Stated as an invariant over the whole table rather than as a further axis. An
/// unselected arm is not a state the verdict depends on -- it is a thing that
/// must not matter -- and "adding one changes nothing, anywhere, whatever its
/// text" is a stronger claim than duplicating every row, and a much smaller one.
///
/// The arm is added with a subscript naming no declared element, which is the
/// only way an unselected arm can still be present in the PARSED map: the parser
/// has already dropped empty arms and collapsed duplicates, so those two ways of
/// going unselected cannot reach this classifier at all. That is the point of
/// classifying the parsed form -- two of the four pipeline stages stop being
/// something this code can get wrong.
#[test]
fn an_arm_the_compiler_never_selects_cannot_change_the_verdict() {
    for (cell, ast, expected) in decision_table() {
        let Ast::Arrayed(dims, elements, default, live) = &ast else {
            continue;
        };
        for text in ["NAN", "7"] {
            let mut with_extra = elements.clone();
            with_extra.insert(
                CanonicalElementName::from_raw("ignored"),
                expr(text).unwrap(),
            );
            let perturbed = Ast::Arrayed(dims.clone(), with_extra, default.clone(), *live);
            assert_eq!(
                expected,
                unfilled_arms(&perturbed),
                "cell {cell:?}: an unselected `{text}` arm changed the verdict"
            );
        }
    }
}

/// Every authored verdict must belong to a real cell. Without this an entry left
/// behind by a renamed axis state would sit unused and unnoticed.
#[test]
fn every_authored_verdict_names_a_real_cell() {
    let cells = expected_cells();
    for (name, _) in expected_verdicts() {
        assert!(
            cells.contains(name),
            "authored verdict {name:?} names no cell of the decision space"
        );
    }
}

/// The table must cover the decision space EXACTLY: every cell present, and no
/// row outside it.
///
/// This is the check that makes the row count derived rather than asserted. Set
/// equality both ways is the point -- a subset test would let a shrinking table
/// pass.
#[test]
fn the_decision_table_covers_every_cell() {
    let covered: BTreeSet<String> = decision_table()
        .iter()
        .map(|(_, ast, _)| decision_cell(ast))
        .collect();
    let expected = expected_cells();
    assert_eq!(
        expected, covered,
        "the decision table must be exactly the product of the enumerated axes"
    );
    assert_eq!(
        expected.len(),
        decision_table().len(),
        "one row per cell -- a duplicate row would hide a missing one"
    );
    assert_eq!(
        32,
        expected.len(),
        "2 + 2 + (4 arm-states x 4 default-states x 2 coverage-states - 4 impossible)"
    );

    let shapes: BTreeSet<&str> = decision_table()
        .iter()
        .map(|(_, ast, _)| equation_shape(ast))
        .collect();
    assert_eq!(BTreeSet::from(EQUATION_SHAPES), shapes);
}

// ---------------------------------------------------------------------------
// End to end: from a real file to the accumulated diagnostic
// ---------------------------------------------------------------------------

fn diagnostics(project: &datamodel::Project) -> Vec<Diagnostic> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    collect_all_diagnostics(&db, sync.project)
}

/// Every `UnfilledEquation` diagnostic the project reports, as
/// `(model, variable, message)`.
fn unfilled_findings(project: &datamodel::Project) -> Vec<(String, String, String)> {
    diagnostics(project)
        .into_iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Model(e) if e.code == ErrorCode::UnfilledEquation => {
                assert_eq!(
                    DiagnosticSeverity::Warning,
                    d.severity,
                    "an unfilled equation must never be reported as an Error: the rest \
                     of the model is worth simulating"
                );
                Some((
                    d.model.clone(),
                    d.variable.clone().unwrap_or_default(),
                    e.get_details().unwrap_or_default().to_string(),
                ))
            }
            _ => None,
        })
        .collect()
}

fn xmile_doc(dimensions: &str, vars: &str, extra_models: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0"
       xmlns:simlin="https://simlin.com/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months">
    <start>0</start><stop>2</stop><dt>1</dt>
  </sim_specs>
  {dimensions}
  <model><variables>{vars}</variables></model>
  {extra_models}
</xmile>"#
    )
}

fn read_xmile(dimensions: &str, vars: &str) -> datamodel::Project {
    read_xmile_with_models(dimensions, vars, "")
}

fn read_xmile_with_models(dimensions: &str, vars: &str, extra_models: &str) -> datamodel::Project {
    crate::compat::open_xmile(&mut xmile_doc(dimensions, vars, extra_models).as_bytes())
        .expect("XMILE must parse")
}

/// Run `project`'s main model and return `var`'s final value.
fn final_value(project: &datamodel::Project, var: &str) -> f64 {
    let mut vm = crate::queue_compile::build_vm(project, "main").expect("model must build");
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

/// The MDL path, end to end: a Sketch-tool placeholder imports, compiles, and
/// produces exactly ONE warning naming the variable that has no equation.
///
/// `marketing` is the unfilled one and `revenue` reads it, so `revenue` is NaN
/// too -- which is the entire point of attributing the origin. The count
/// assertion is what pins that: exactly one finding for two NaN series.
#[test]
fn an_mdl_a_function_of_placeholder_warns_naming_the_variable() {
    let mdl = concat!(
        "price = 3\n\t~\t\n\t~\t|\n\n",
        "marketing = A FUNCTION OF( price )\n\t~\t\n\t~\t|\n\n",
        "revenue = marketing * price\n\t~\t\n\t~\t|\n\n",
        "\\\\\\---/// Sketch information - do not modify anything except names\n"
    );
    let project = crate::compat::open_vensim(mdl).expect("MDL must parse");

    // The premise: the importer really does store the placeholder as the NaN
    // literal. If this stops holding, the rest of the test proves nothing.
    let marketing = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "marketing")
        .expect("the placeholder variable must survive import");
    assert_eq!(
        Some(&datamodel::Equation::Scalar("NAN".to_string())),
        marketing.get_equation()
    );

    let findings = unfilled_findings(&project);
    assert_eq!(
        1,
        findings.len(),
        "exactly one unfilled-equation finding, for the variable that HAS no \
         equation -- not for the variables downstream of it, which is the whole \
         reason to report it. Got: {findings:#?}"
    );
    assert_eq!("marketing", findings[0].1);
    assert!(
        findings[0].2.contains("'marketing' has no equation"),
        "the message must name the variable: {:?}",
        findings[0].2
    );

    // And the model still compiles: this is an advisory, not a rejection.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    compile_project_incremental(&db, sync.project, "main")
        .expect("an unfilled equation must not block compilation");
}

/// The XMILE path: a hand-authored `<eqn>NAN</eqn>` is the same claim -- the
/// variable has no usable equation however it got that way -- so it gets the
/// same warning. This is why the diagnostic lives in the engine rather than in
/// the MDL reader.
#[test]
fn a_hand_authored_xmile_nan_equation_warns_naming_the_variable() {
    let project = read_xmile(
        "",
        r#"
        <aux name="price"><eqn>3</eqn></aux>
        <aux name="marketing"><eqn>NAN</eqn></aux>
        <aux name="revenue"><eqn>marketing * price</eqn></aux>"#,
    );
    let findings = unfilled_findings(&project);
    assert_eq!(
        vec!["marketing".to_string()],
        findings.iter().map(|f| f.1.clone()).collect::<Vec<_>>(),
        "got: {findings:#?}"
    );
}

/// The control, carrying both things that must NOT be reported.
///
/// `db::sync` splices all nine stdlib templates into EVERY project, so this is
/// simultaneously the tightest test of the stdlib exclusion: five of those
/// templates (`smth1`, `smth3`, `delay1`, `delay3`, `trend`) legitimately
/// declare their `initial_value` port as `<eqn>NAN</eqn>`, because the port's
/// value arrives from the caller. Without the exclusion this model -- and every
/// model ever compiled -- reports five findings it did not earn.
///
/// `guard` and `poison` are the other boundary, at the level a modeller sees it:
/// a NaN inside a larger expression is a filled equation, and widening the atom
/// past a single token would report it.
///
/// Both positions are present deliberately. A widening that drops the
/// single-token check catches whichever end it looks at: `guard` puts `NAN`
/// LAST, so it only reds a "contains a nan token anywhere" widening, while
/// `poison` puts it FIRST, which is what reds a widening that keeps only the
/// leading-token test. `poison` is contrived -- no modeller writes it -- and it
/// earns its place solely as that second tripwire.
#[test]
fn an_ordinary_model_reports_nothing() {
    let project = read_xmile(
        "",
        r#"
        <aux name="price"><eqn>3</eqn></aux>
        <aux name="revenue"><eqn>price * 2</eqn></aux>
        <aux name="guard"><eqn>IF revenue &gt; 0 THEN revenue ELSE NAN</eqn></aux>
        <aux name="poison"><eqn>NAN + revenue</eqn></aux>"#,
    );
    assert_eq!(
        Vec::<(String, String, String)>::new(),
        unfilled_findings(&project)
    );
}

/// A BOUND module input port in a user sub-model must NOT be reported, and the
/// simulated numbers are why.
///
/// The port's own equation is dead: the compiler replaces it with the caller's
/// binding, so `port` is 5 and `out` is 10 with no NaN anywhere. Every factual
/// clause of the warning would be false for this model -- the port is not NaN,
/// nothing downstream of it is NaN, and nobody forgot to write anything. The
/// value assertions are what make this a test of the CLAIM rather than of the
/// skip: if the port's equation ever did drive the result, they would fail here
/// before the finding count did.
///
/// This is the general rule that the stdlib skip is one instance of. The idiom
/// is one WE author and ship (`stdlib/*.stmx`), so it is what a modeller
/// building a reusable sub-model copies from us.
#[test]
fn a_bound_module_input_port_is_not_reported() {
    let project = read_xmile_with_models(
        "",
        r#"
        <aux name="drive"><eqn>5</eqn></aux>
        <module name="m" simlin:model_name="sub"><connect to="m.port" from="drive"/></module>
        <aux name="total"><eqn>m.out</eqn></aux>"#,
        r#"<model name="sub"><variables>
        <aux name="port" access="input"><eqn>NAN</eqn></aux>
        <aux name="out"><eqn>port * 2</eqn></aux>
        </variables></model>"#,
    );

    assert_eq!(
        Vec::<(String, String, String)>::new(),
        unfilled_findings(&project),
        "a bound input port's own equation is a fallback the caller overrides"
    );
    assert_eq!(5.0, final_value(&project, "m·port"));
    assert_eq!(10.0, final_value(&project, "m·out"));
    assert_eq!(10.0, final_value(&project, "total"));
}

/// The stdlib exclusion again, from the direction that would survive a
/// "skip models nothing instantiates" narrowing: a model that really does
/// INSTANTIATE `SMTH1` still reports nothing. The spliced template is now
/// reachable, its `initial_value = NAN` port is genuinely compiled, and it is
/// still not the user's unfilled equation.
#[test]
fn instantiating_a_stdlib_template_reports_nothing() {
    let project = read_xmile(
        "",
        r#"
        <aux name="price"><eqn>3 + TIME</eqn></aux>
        <aux name="smoothed"><eqn>SMTH1(price, 2)</eqn></aux>"#,
    );
    assert_eq!(
        Vec::<(String, String, String)>::new(),
        unfilled_findings(&project),
        "a stdlib template's `initial_value = NAN` port is a legitimate \
         declaration, not a modeller's unfilled equation"
    );
}

/// A NaN EXCEPT default that NO slot can reach is not an unfilled equation.
///
/// The arms name every element of `D`, so `expand_arrayed_with_hoisting` never
/// consults the default; the compiled model holds 1/2/3 and contains no NaN
/// anywhere. Before the coverage axis this reported "no equation for every
/// element with no equation of its own" -- a finding about a slot that does not
/// exist. The value assertions are what make this a test of the claim: they fail
/// first if the default ever does drive a slot.
#[test]
fn an_unreachable_nan_default_is_not_reported() {
    let project = read_xmile(
        r#"<dimensions><dim name="D"><elem name="a"/><elem name="b"/><elem name="c"/></dim></dimensions>"#,
        r#"
        <aux name="full">
          <eqn>NAN</eqn>
          <element subscript="a"><eqn>1</eqn></element>
          <element subscript="b"><eqn>2</eqn></element>
          <element subscript="c"><eqn>3</eqn></element>
          <dimensions><dim name="D"/></dimensions>
        </aux>"#,
    );
    assert_eq!(
        Vec::<(String, String, String)>::new(),
        unfilled_findings(&project),
        "the arms cover D, so the NaN default is dead code"
    );
    assert_eq!(1.0, final_value(&project, "full[a]"));
    assert_eq!(2.0, final_value(&project, "full[b]"));
    assert_eq!(3.0, final_value(&project, "full[c]"));
}

/// A SPARSE array whose every listed arm is unfilled is not WHOLLY unfilled:
/// the slots with no arm compile to a finite `0`, not to NaN.
///
/// `sparse[c]` has no entry and no default, so it is 0 while `[a]` and `[b]` are
/// NaN. Before the coverage axis this said "variable 'sparse' has no equation",
/// which reads as every slot being NaN. It now names the two arms that are.
///
/// (That silent `0` for an armless slot is its own reportable shape and a
/// deliberately separate one -- GH #905 covers it -- so this test asserts the
/// value rather than expecting a second finding.)
#[test]
fn a_sparse_array_of_unfilled_arms_names_the_arms_not_the_variable() {
    let project = read_xmile(
        r#"<dimensions><dim name="D"><elem name="a"/><elem name="b"/><elem name="c"/></dim></dimensions>"#,
        r#"
        <aux name="sparse">
          <element subscript="a"><eqn>NAN</eqn></element>
          <element subscript="b"><eqn>NAN</eqn></element>
          <dimensions><dim name="D"/></dimensions>
        </aux>"#,
    );
    let findings = unfilled_findings(&project);
    assert_eq!(1, findings.len(), "one finding per variable: {findings:#?}");
    assert!(
        findings[0]
            .2
            .starts_with("array variable 'sparse' has no equation for 'a', 'b'"),
        "the message must name the unfilled arms, not claim the whole variable \
         has no equation: {:?}",
        findings[0].2
    );
    assert!(final_value(&project, "sparse[a]").is_nan());
    assert!(final_value(&project, "sparse[b]").is_nan());
    assert_eq!(
        0.0,
        final_value(&project, "sparse[c]"),
        "the armless slot compiles to a finite 0 -- which is why the whole \
         variable does not 'have no equation'"
    );
}

/// Every `UnknownElementSubscript` diagnostic the project reports, as
/// `(variable, message)`. The unfilled-equation filter must not silence this
/// one: a subscript naming nothing is still worth telling the modeller about.
fn unknown_subscript_findings(project: &datamodel::Project) -> Vec<(String, String)> {
    diagnostics(project)
        .into_iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Model(e) if e.code == ErrorCode::UnknownElementSubscript => Some((
                d.variable.clone().unwrap_or_default(),
                e.get_details().unwrap_or_default().to_string(),
            )),
            _ => None,
        })
        .collect()
}

/// An arm whose subscript names nothing is not an arm: the compiler ignores it,
/// so it cannot be a slot that simulates as NaN.
///
/// `typo=NAN` beside arms that already cover `D`. Every declared slot is finite
/// (1 and 2), so there is no unfilled equation to report at all -- but the
/// modeller must still hear that `typo` names nothing, which is the SIBLING
/// advisory's job and is asserted here so the filter cannot silence it.
#[test]
fn an_unknown_subscript_arm_is_not_an_unfilled_equation() {
    let project = read_xmile(
        r#"<dimensions><dim name="D"><elem name="a"/><elem name="b"/></dim></dimensions>"#,
        r#"
        <aux name="typo_only">
          <element subscript="a"><eqn>1</eqn></element>
          <element subscript="b"><eqn>2</eqn></element>
          <element subscript="typo"><eqn>NAN</eqn></element>
          <dimensions><dim name="D"/></dimensions>
        </aux>"#,
    );
    assert_eq!(
        Vec::<(String, String, String)>::new(),
        unfilled_findings(&project),
        "the compiler drops the `typo` arm, so no slot simulates as NaN"
    );
    assert_eq!(
        1,
        unknown_subscript_findings(&project).len(),
        "the unknown-subscript advisory must still fire -- what was wrong was \
         the SECOND warning claiming `typo` simulates as NaN, not this one"
    );
    assert_eq!(1.0, final_value(&project, "typo_only[a]"));
    assert_eq!(2.0, final_value(&project, "typo_only[b]"));
}

/// A model that declares a variable NAMED `nan` gets no unfilled-equation
/// findings, because in such a model the stored text `NAN` has two readings.
///
/// This is two of this branch's changes meeting, not a defect in either. Batch 1
/// deliberately excluded `nan` from the MDL importer's keyword quoting -- quoting
/// it would bind Vensim's `A FUNCTION OF(...)` placeholder to any like-named
/// variable -- and disclosed the residual in
/// `keyword_ident_tests::a_bare_nan_reference_in_mdl_is_still_the_literal`. So
/// `b = nan` here IS a formula the modeller wrote, stored identically to a
/// placeholder they never wrote, and "b has no equation" was false.
///
/// The assertions bracket the ambiguity from both sides: `b` really is NaN at
/// runtime (batch 1's residual, unchanged by this), so the NaN half of the old
/// message was true and only the "has no equation" half was false -- which is
/// exactly why declining to claim beats guessing.
#[test]
fn a_model_declaring_a_variable_named_nan_gets_no_findings() {
    let mdl = concat!(
        "nan = 3\n\t~\t\n\t~\t|\n\n",
        "b = nan\n\t~\t\n\t~\t|\n\n",
        "\\\\\\---/// Sketch information - do not modify anything except names\n"
    );
    let project = crate::compat::open_vensim(mdl).expect("MDL must parse");
    assert_eq!(
        Vec::<(String, String, String)>::new(),
        unfilled_findings(&project),
        "`b = nan` is a reference the modeller wrote; we cannot tell it from a \
         placeholder, so we must not claim `b` has no equation"
    );
    assert_eq!(3.0, final_value(&project, "nan"));
    assert!(
        final_value(&project, "b").is_nan(),
        "batch 1's disclosed residual: the bare reference still reads as the \
         literal, so the NaN itself was never the false part of the message"
    );
}

/// The ambiguity is scoped to the MODEL that declares `nan`, because that is the
/// scope in which a bare reference resolves.
///
/// A sub-model declaring `nan` says nothing about how the main model's `NAN`
/// reads, so the main model's placeholder is still reported. Without this, one
/// oddly-named variable anywhere in a project would silence the diagnostic
/// everywhere in it.
#[test]
fn a_nan_variable_in_another_model_does_not_suppress_this_one() {
    let project = read_xmile_with_models(
        "",
        r#"
        <aux name="marketing"><eqn>NAN</eqn></aux>
        <module name="m" simlin:model_name="sub"><connect to="m.port" from="marketing"/></module>"#,
        r#"<model name="sub"><variables>
        <aux name="port" access="input"><eqn>0</eqn></aux>
        <aux name="nan"><eqn>3</eqn></aux>
        <aux name="out"><eqn>port + nan</eqn></aux>
        </variables></model>"#,
    );
    let findings = unfilled_findings(&project);
    assert_eq!(
        vec!["marketing".to_string()],
        findings.iter().map(|f| f.1.clone()).collect::<Vec<_>>(),
        "the sub-model's `nan` cannot make the MAIN model's text ambiguous: \
         {findings:#?}"
    );
    assert_eq!("main", findings[0].0);
}

/// The suppression is total: every equation shape that can reach a finding
/// stops being reportable when the model declares a variable named `nan`.
///
/// Stated over the GATE rather than over the decision table, because the gate is
/// where the rule lives and the gate reads the datamodel equation while the
/// table reads the parsed one. The four rows are the four ways the gate can say
/// yes -- one per `datamodel::Equation` variant, plus the default-only case that
/// probe O showed was unpinned.
#[test]
fn declaring_nan_suppresses_every_reportable_shape() {
    let dim = || vec!["d".to_string()];
    let elem = |subscript: &str, text: &str| {
        (
            subscript.to_string(),
            text.to_string(),
            None,
            None::<datamodel::GraphicalFunction>,
        )
    };
    let reportable = [
        ("scalar", datamodel::Equation::Scalar("NAN".to_string())),
        (
            "apply-to-all",
            datamodel::Equation::ApplyToAll(dim(), "nan".to_string()),
        ),
        (
            "arrayed arm",
            datamodel::Equation::Arrayed(dim(), vec![elem("a", "NAN")], None, false),
        ),
        (
            "arrayed default only",
            datamodel::Equation::Arrayed(
                dim(),
                vec![elem("a", "1")],
                Some("NAN".to_string()),
                true,
            ),
        ),
    ];
    for (label, eqn) in reportable {
        assert!(
            crate::variable::may_have_unfilled_arms(&eqn, false),
            "{label} must pass the gate in an ordinary model"
        );
        assert!(
            !crate::variable::may_have_unfilled_arms(&eqn, true),
            "{label} must make no claim when `nan` names a variable"
        );
    }
}

/// A variable whose ONLY NaN is its EXCEPT default is still reported.
///
/// Every arm is filled; the uncovered slot `c` falls to the default and is the
/// one that stops. This is the case that keeps `may_have_unfilled_arms` honest:
/// that pre-scan exists to skip the slot walk for variables that cannot report,
/// and it must stay a SUPERSET of what gets reported. A version testing only the
/// arms passes every other fixture here -- each of those has a NaN arm too -- and
/// silently loses this finding.
#[test]
fn a_variable_whose_only_nan_is_the_default_is_still_reported() {
    let project = read_xmile(
        r#"<dimensions><dim name="D"><elem name="a"/><elem name="b"/><elem name="c"/></dim></dimensions>"#,
        r#"
        <aux name="defaulted">
          <eqn>NAN</eqn>
          <element subscript="a"><eqn>1</eqn></element>
          <element subscript="b"><eqn>2</eqn></element>
          <dimensions><dim name="D"/></dimensions>
        </aux>"#,
    );
    let findings = unfilled_findings(&project);
    assert_eq!(1, findings.len(), "{findings:#?}");
    assert!(
        findings[0]
            .2
            .contains("no equation for every element with no equation of its own"),
        "the default is the unfilled arm here: {:?}",
        findings[0].2
    );
    assert_eq!(1.0, final_value(&project, "defaulted[a]"));
    assert_eq!(2.0, final_value(&project, "defaulted[b]"));
    assert!(
        final_value(&project, "defaulted[c]").is_nan(),
        "the slot that falls to the NaN default is the one that stops"
    );
}

/// An arm with an EMPTY equation does not cover its slot: the parser drops it,
/// so the slot falls to the EXCEPT default like any other armless slot.
///
/// This was the fourth instance of the arms-as-written class and the first FALSE
/// NEGATIVE -- `a` looked covered, so the live `NAN` default looked unreachable
/// and nothing was reported, while `a` really does simulate as NaN. It is fixed
/// not by mirroring the parser's empty-drop but by classifying the parsed form,
/// where the drop has already happened.
#[test]
fn an_empty_arm_does_not_cover_its_slot() {
    let project = read_xmile(
        r#"<dimensions><dim name="D"><elem name="a"/><elem name="b"/></dim></dimensions>"#,
        r#"
        <aux name="hollow">
          <eqn>NAN</eqn>
          <element subscript="a"><eqn></eqn></element>
          <element subscript="b"><eqn>2</eqn></element>
          <dimensions><dim name="D"/></dimensions>
        </aux>"#,
    );
    // The premise: the slot really is NaN, so a missing warning is a missing
    // one and not a difference of opinion. `b` is the control -- it has a real
    // arm, so it is finite and the finding must not name it.
    assert!(
        final_value(&project, "hollow[a]").is_nan(),
        "the empty arm is dropped and the slot takes the NaN default"
    );
    assert_eq!(2.0, final_value(&project, "hollow[b]"));
    let findings = unfilled_findings(&project);
    assert_eq!(1, findings.len(), "{findings:#?}");
    assert_eq!("hollow", findings[0].1);
    assert!(
        findings[0]
            .2
            .contains("no equation for every element with no equation of its own"),
        "the slot that stops is the one taking the default: {:?}",
        findings[0].2
    );
}

/// A duplicate arm that a later one SHADOWS is not reported: only the surviving
/// arm is what the slot evaluates to.
///
/// `a=NAN` followed by `A=1` canonicalize to one key, and `parse_equation`
/// collects arms into a `HashMap`, so the last wins and the slot holds 1. This
/// is the third instance of one root -- an arm the compiler never selects -- and
/// the reason the classifier now takes the SELECTION rather than testing arms
/// individually.
///
/// The value assertion is the point. It pins the last-wins rule against what the
/// compiler actually evaluates rather than against the reasoning in
/// `arm_selection`'s doc, so if the two ever disagreed this would fail before
/// the finding count did.
#[test]
fn a_shadowed_duplicate_arm_is_not_reported() {
    let project = read_xmile(
        r#"<dimensions><dim name="D"><elem name="a"/><elem name="b"/></dim></dimensions>"#,
        r#"
        <aux name="dup">
          <element subscript="a"><eqn>NAN</eqn></element>
          <element subscript="A"><eqn>1</eqn></element>
          <element subscript="b"><eqn>2</eqn></element>
          <dimensions><dim name="D"/></dimensions>
        </aux>"#,
    );
    assert_eq!(
        1.0,
        final_value(&project, "dup[a]"),
        "the LAST arm for a canonical key is the one the compiler keeps"
    );
    assert_eq!(2.0, final_value(&project, "dup[b]"));
    assert_eq!(
        Vec::<(String, String, String)>::new(),
        unfilled_findings(&project),
        "the shadowed `a=NAN` arm is not what any slot evaluates to"
    );
}

/// A GENUINE unfilled arm alongside a typo'd one still gets reported -- and only
/// the genuine one is named.
///
/// This is the half that a filter which simply gave up on any variable carrying
/// an unknown subscript would break, so it is the control that keeps the fix
/// from over-correcting.
#[test]
fn a_genuine_unfilled_arm_survives_a_typod_sibling() {
    let project = read_xmile(
        r#"<dimensions><dim name="D"><elem name="a"/><elem name="b"/></dim></dimensions>"#,
        r#"
        <aux name="mixed">
          <element subscript="a"><eqn>1</eqn></element>
          <element subscript="b"><eqn>NAN</eqn></element>
          <element subscript="typo"><eqn>NAN</eqn></element>
          <dimensions><dim name="D"/></dimensions>
        </aux>"#,
    );
    let findings = unfilled_findings(&project);
    assert_eq!(1, findings.len(), "{findings:#?}");
    assert!(
        findings[0].2.contains("no equation for 'b'") && !findings[0].2.contains("typo"),
        "only the arm that reaches a slot may be named: {:?}",
        findings[0].2
    );
    assert_eq!(1, unknown_subscript_findings(&project).len());
    assert_eq!(1.0, final_value(&project, "mixed[a]"));
    assert!(final_value(&project, "mixed[b]").is_nan());
}

/// The message may not claim that everything downstream of the unfilled variable
/// is NaN, because NaN is absorbing through ARITHMETIC only.
///
/// `arith` reads `marketing` and is NaN; `guarded` reads it through a comparison
/// and is a finite 0, and `downstream` below that is a finite 5. IEEE says every
/// comparison against a NaN is false, so the conditional takes its ELSE branch
/// and the NaN stops there. A modeller tracing an origin backward needs that: a
/// finite variable does not clear its inputs.
#[test]
fn the_message_does_not_claim_every_downstream_variable_is_nan() {
    let project = read_xmile(
        "",
        r#"
        <aux name="marketing"><eqn>NAN</eqn></aux>
        <aux name="arith"><eqn>marketing * 2</eqn></aux>
        <aux name="guarded"><eqn>IF marketing &gt; 0 THEN 1 ELSE 0</eqn></aux>
        <aux name="downstream"><eqn>guarded + 5</eqn></aux>"#,
    );

    // The premise: a conditional really does absorb the NaN here.
    assert!(final_value(&project, "arith").is_nan());
    assert_eq!(0.0, final_value(&project, "guarded"));
    assert_eq!(5.0, final_value(&project, "downstream"));

    let findings = unfilled_findings(&project);
    assert_eq!(1, findings.len(), "{findings:#?}");
    let message = &findings[0].2;
    assert!(
        message.contains("spreads through arithmetic"),
        "the message must scope the spread to arithmetic: {message:?}"
    );
    assert!(
        !message.contains("every variable downstream"),
        "the message must not assert that every downstream variable is NaN -- \
         `guarded` is a finite 0: {message:?}"
    );
}

/// An arrayed variable with only SOME unfilled elements is reported, and the
/// message names them.
///
/// The decision (`variable::unfilled_arms`): an arrayed variable's elements are
/// separate simulated series, so an unfilled `values[b]` stops `values[b]`'s
/// line exactly the way an unfilled scalar stops its own. Staying silent unless
/// EVERY arm is unfilled would go quiet precisely where the model is hardest to
/// read -- two lines fine, one stopping. It stays one finding per variable, so
/// a wholly-unfilled 500-element array is still a single message.
#[test]
fn a_partially_unfilled_array_names_its_unfilled_elements() {
    let project = read_xmile(
        r#"<dimensions><dim name="Items"><elem name="a"/><elem name="b"/><elem name="c"/></dim></dimensions>"#,
        r#"
        <aux name="values">
          <element subscript="a"><eqn>1</eqn></element>
          <element subscript="b"><eqn>NAN</eqn></element>
          <element subscript="c"><eqn>NAN</eqn></element>
          <dimensions><dim name="Items"/></dimensions>
        </aux>"#,
    );
    let findings = unfilled_findings(&project);
    assert_eq!(1, findings.len(), "one finding per variable: {findings:#?}");
    assert_eq!("values", findings[0].1);
    assert!(
        findings[0].2.contains("no equation for 'b', 'c'"),
        "the message must name the unfilled elements: {:?}",
        findings[0].2
    );
}

/// The one decision-table cell that reaches the message branch joining element
/// names AND the default phrase -- `some arms unfilled` + a LIVE unfilled EXCEPT
/// default -- built from a real XMILE file rather than by hand.
///
/// A top-level `<eqn>` alongside `<element>` entries is what sets
/// `has_except_default` in the reader (`xmile/variables.rs`), so this shape is
/// reachable and the join is production behaviour, not a formatting curiosity.
/// The review found this branch untested because the old table had collapsed
/// the default axis for the some-arms row.
#[test]
fn a_partial_array_with_an_unfilled_default_names_both() {
    let project = read_xmile(
        r#"<dimensions><dim name="Items"><elem name="a"/><elem name="b"/><elem name="c"/></dim></dimensions>"#,
        r#"
        <aux name="values">
          <eqn>NAN</eqn>
          <element subscript="a"><eqn>1</eqn></element>
          <element subscript="b"><eqn>NAN</eqn></element>
          <dimensions><dim name="Items"/></dimensions>
        </aux>"#,
    );
    let findings = unfilled_findings(&project);
    assert_eq!(1, findings.len(), "one finding per variable: {findings:#?}");
    assert!(
        findings[0]
            .2
            .contains("no equation for 'b', every element with no equation of its own"),
        "the message must name the unfilled element AND the default: {:?}",
        findings[0].2
    );
}

/// A stock's `equation` is its INITIAL VALUE, so the message says so. "variable
/// 's' has no equation" reads as though the stock had no dynamics at all, when
/// what it is missing is where to start; the NaN claim is unchanged, because a
/// NaN initial poisons the integration for the whole run.
#[test]
fn an_unfilled_stock_is_described_as_missing_its_initial_value() {
    let project = read_xmile(
        "",
        r#"
        <stock name="level"><eqn>NAN</eqn><inflow>fill</inflow></stock>
        <flow name="fill"><eqn>1</eqn></flow>"#,
    );
    let findings = unfilled_findings(&project);
    assert_eq!(1, findings.len(), "{findings:#?}");
    assert!(
        findings[0]
            .2
            .starts_with("stock 'level' has no initial value"),
        "a stock is missing an initial value, not an equation: {:?}",
        findings[0].2
    );
    assert!(
        final_value(&project, "level").is_nan(),
        "the NaN claim must still hold for a stock: a NaN initial poisons the \
         whole integration"
    );
}

/// Accumulation order is by variable name. The emitter's rustdoc claims this and
/// nothing pinned it -- every other fixture here reports zero or one finding, so
/// they are all order-blind.
///
/// Six variables, declared in reverse, deliberately: the source is a `HashMap`
/// whose iteration order is per-process random, so a three-name fixture comes
/// out sorted by chance often enough to be a useless detector (measured: it
/// caught a removed `sort_unstable` in 2 of 5 runs). Six names leave a 1-in-720
/// accidental pass, the same reasoning `db::fragment_determinism_tests` uses for
/// its repeat count.
#[test]
fn findings_are_accumulated_in_sorted_name_order() {
    let project = read_xmile(
        "",
        r#"
        <aux name="zulu"><eqn>NAN</eqn></aux>
        <aux name="victor"><eqn>NAN</eqn></aux>
        <aux name="quebec"><eqn>NAN</eqn></aux>
        <aux name="mike"><eqn>NAN</eqn></aux>
        <aux name="delta"><eqn>NAN</eqn></aux>
        <aux name="alpha"><eqn>NAN</eqn></aux>"#,
    );
    let names: Vec<String> = unfilled_findings(&project)
        .into_iter()
        .map(|f| f.1)
        .collect();
    assert_eq!(
        vec!["alpha", "delta", "mike", "quebec", "victor", "zulu"],
        names
    );
}

/// The advisory must not make a project look failed. `FormattedErrors::push`
/// raises `has_model_errors` / `has_variable_errors` for `Error` severity only,
/// and those flags gate failure-shaped decisions downstream (the CLI's
/// `NotSimulatable` suppression, and through it its reporting). A new warning
/// that flipped them would turn passing workflows red on merit alone.
#[test]
fn the_warning_does_not_raise_the_failure_flags() {
    use crate::errors::FormattedErrors;

    let project = read_xmile("", r#"<aux name="marketing"><eqn>NAN</eqn></aux>"#);
    let mut formatted = FormattedErrors::default();
    for diag in &diagnostics(&project) {
        formatted.push(crate::errors::format_diagnostic(diag));
    }
    assert!(
        !formatted.errors.is_empty(),
        "the warning must reach the formatted surface"
    );
    assert!(
        !formatted.has_model_errors && !formatted.has_variable_errors,
        "an unfilled-equation Warning must not raise the failure flags: \
         {formatted:#?}"
    );
}
