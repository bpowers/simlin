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

use std::collections::BTreeSet;

use crate::common::ErrorCode;
use crate::datamodel;
use crate::db::{
    Diagnostic, DiagnosticError, DiagnosticSeverity, SimlinDb, collect_all_diagnostics,
    compile_project_incremental, sync_from_datamodel_incremental,
};
use crate::variable::{ArmSelection, UnfilledArms, is_nan_literal, unfilled_arms};

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
// The per-shape decision, enumerated from `datamodel::Equation`'s variants
// ---------------------------------------------------------------------------

/// The `datamodel::Equation` variant `eqn` is, as an EXHAUSTIVE match with no
/// catch-all arm: a new equation shape added to the datamodel is a compile error
/// right here, which is what makes the decision table's coverage checkable
/// rather than asserted.
fn equation_shape(eqn: &datamodel::Equation) -> &'static str {
    match eqn {
        datamodel::Equation::Scalar(_) => "Scalar",
        datamodel::Equation::ApplyToAll(..) => "ApplyToAll",
        datamodel::Equation::Arrayed(..) => "Arrayed",
    }
}

/// Counted from `datamodel::Equation`: three variants, three shapes.
const EQUATION_SHAPES: [&str; 3] = ["Scalar", "ApplyToAll", "Arrayed"];

fn dims() -> Vec<String> {
    vec!["items".to_string()]
}

/// One per-element arm of an `Equation::Arrayed`:
/// `(subscript, equation, initial equation, graphical function)`.
type Arm = (
    String,
    String,
    Option<String>,
    Option<datamodel::GraphicalFunction>,
);

fn arm(subscript: &str, eqn: &str) -> Arm {
    (subscript.to_string(), eqn.to_string(), None, None)
}

/// An `Equation::Arrayed` with `default` as its EXCEPT default. `live` is the
/// `has_except_default` flag, which is what makes the default apply to elements
/// with no entry of their own -- a `Some` default with the flag clear is dead.
fn arrayed(arms: Vec<Arm>, default: Option<&str>, live: bool) -> datamodel::Equation {
    datamodel::Equation::Arrayed(dims(), arms, default.map(str::to_string), live)
}

// THREE axes decide an `Arrayed` verdict. The first two are properties of the
// equation TEXT; the third is not derivable from it at all, which is how the
// first version of this table came to be wrong in both directions at once.
// The table is their product and `the_decision_table_covers_every_cell` checks
// that mechanically, so the row count is DERIVED rather than asserted.

/// How many of the SELECTED per-element arms are unfilled -- selected meaning
/// the compiler evaluates that arm for some slot. Four states: `unfilled_arms`
/// tests `unfilled.is_empty()` and `unfilled.len() == selected_count`, and those
/// two comparisons distinguish exactly these -- with the no-arms case separate
/// because it satisfies BOTH (`0 == 0`).
///
/// `no-arms` covers "written with no arms" and "no arm is selected" alike: to the
/// compiled model they are the same variable, entirely default- or zero-filled.
const ARM_STATES: [&str; 4] = ["no-arms", "none-unfilled", "some-unfilled", "all-unfilled"];

/// The state of the EXCEPT default. Four states: it is dropped unless
/// `has_except_default` is set, and only then is its text read.
///
/// A dead default whose text is FILLED is deliberately not a fifth state: the
/// flag drops the default before its text is ever looked at, so it is the same
/// state as `dead-default` by construction, not by coincidence.
const DEFAULT_STATES: [&str; 4] = [
    "no-default",
    "dead-default",
    "live-filled-default",
    "live-unfilled-default",
];

/// Whether any declared slot falls past all the arms, so the EXCEPT default (or
/// the compiler's silent `0`) decides its value.
///
/// This axis is the one no amount of staring at the equation's text can supply
/// -- it needs the dimensions RESOLVED -- and leaving it out made the verdict
/// wrong in both directions on models that run today: a NaN default no slot can
/// reach was reported, and a sparse array whose omitted slots compile to `0` was
/// reported as wholly unfilled.
const COVERAGE_STATES: [&str; 2] = ["covers-every-slot", "leaves-slots-uncovered"];

fn coverage_state(selection: &ArmSelection) -> &'static str {
    if selection.default_is_selected() {
        "leaves-slots-uncovered"
    } else {
        "covers-every-slot"
    }
}

/// The selection an arrayed fixture is classified under: every arm it declares
/// is selected (the fixtures use only real, distinct subscripts), plus the
/// coverage state being exercised.
///
/// Arms the compiler does NOT select are deliberately not a further axis.
/// Whether one exists cannot change any verdict once selection is the input, and
/// asserting that INVARIANT over the whole table
/// (`an_arm_the_compiler_never_selects_cannot_change_the_verdict`) is both a
/// stronger claim and a smaller one than multiplying the product.
fn coverage_for(eqn: &datamodel::Equation, default_is_selected: bool) -> ArmSelection {
    let datamodel::Equation::Arrayed(_, arms, _, _) = eqn else {
        return ArmSelection::whole_variable();
    };
    ArmSelection::new((0..arms.len()).collect(), default_is_selected)
}

/// The cell of the decision space a row occupies, computed from the row's
/// EQUATION and COVERAGE rather than from its label -- so a mislabelled row
/// cannot fake coverage. `Scalar` and `ApplyToAll` carry one arm and no default,
/// so their only axis is whether that arm is unfilled.
fn decision_cell(eqn: &datamodel::Equation, coverage: &ArmSelection) -> String {
    match eqn {
        datamodel::Equation::Scalar(s) | datamodel::Equation::ApplyToAll(_, s) => {
            let filled = if is_nan_literal(s) {
                "unfilled"
            } else {
                "filled"
            };
            format!("{}/{filled}", equation_shape(eqn))
        }
        datamodel::Equation::Arrayed(_, elements, default, live) => {
            let unfilled = elements
                .iter()
                .filter(|(_, e, _, _)| is_nan_literal(e))
                .count();
            let arms = if elements.is_empty() {
                "no-arms"
            } else if unfilled == 0 {
                "none-unfilled"
            } else if unfilled == elements.len() {
                "all-unfilled"
            } else {
                "some-unfilled"
            };
            let default = match (default.as_deref(), live) {
                (None, _) => "no-default",
                (Some(_), false) => "dead-default",
                (Some(d), true) if is_nan_literal(d) => "live-unfilled-default",
                (Some(_), true) => "live-filled-default",
            };
            format!("Arrayed/{arms}/{default}/{}", coverage_state(coverage))
        }
    }
}

/// Every cell the table must cover.
///
/// **2 (Scalar) + 2 (ApplyToAll) + [(4 arm-states x 4 default-states x 2
/// coverage-states) - 4 impossible] = 32.**
///
/// The four excluded cells are `no-arms` x `covers-every-slot`: zero arms cannot
/// name every slot of a dimension that has at least one. Feeding the classifier
/// that pair would pin behaviour for an input the emitter cannot construct,
/// which is worth less than saying why it is absent.
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

/// Build the arrayed fixture for one `(arms, default)` cell. Generated rather
/// than hand-written so the table cannot drift out of the axis product; the
/// EXPECTED VERDICTS in [`decision_table`] stay authored, which is the half that
/// must not be derived from the implementation.
fn arrayed_fixture(arms: &str, default: &str) -> datamodel::Equation {
    let arms = match arms {
        "no-arms" => vec![],
        "none-unfilled" => vec![arm("a", "1"), arm("b", "2")],
        "some-unfilled" => vec![arm("a", "1"), arm("b", "NAN")],
        "all-unfilled" => vec![arm("a", "NAN"), arm("b", "nan")],
        other => panic!("unknown arm state {other:?}"),
    };
    let (default, live) = match default {
        "no-default" => (None, false),
        "dead-default" => (Some("NAN"), false),
        "live-filled-default" => (Some("7"), true),
        "live-unfilled-default" => (Some("nan"), true),
        other => panic!("unknown default state {other:?}"),
    };
    arrayed(arms, default, live)
}

/// The verdict each cell must produce, authored cell by cell.
///
/// Deliberately a flat list of literals rather than a `match` with grouped or
/// wildcard arms: a grouped expectation is a second copy of the rule under test,
/// and would agree with a wrong implementation for the same wrong reason. Every
/// entry here was worked out from what the compiled model actually evaluates,
/// slot by slot.
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
        // -- COVERS EVERY SLOT. No slot can reach the default, so it is dead
        //    code whatever its text says and all four default states agree.
        ("Arrayed/none-unfilled/no-default/covers-every-slot", None),
        ("Arrayed/none-unfilled/dead-default/covers-every-slot", None),
        (
            "Arrayed/none-unfilled/live-filled-default/covers-every-slot",
            None,
        ),
        (
            // P2-1, first half: this warned before the coverage axis existed,
            // on a model whose compiled slots hold 1 and 2 and no NaN at all.
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
            // Every slot falls to the default, and the default is unfilled.
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
            // The only cell producing a non-empty element list AND `default`,
            // i.e. the message branch that joins the two.
            "Arrayed/some-unfilled/live-unfilled-default/leaves-slots-uncovered",
            partial(&["b"], true),
        ),
        (
            // P2-1, second half: every ARM is unfilled, but the uncovered slots
            // compile to a finite 0, so the variable is not wholly unfilled.
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
fn decision_table() -> Vec<(
    String,
    datamodel::Equation,
    ArmSelection,
    Option<UnfilledArms>,
)> {
    let verdicts = expected_verdicts();
    let mut rows = Vec::new();
    let mut push = |cell: String, eqn: datamodel::Equation, coverage: ArmSelection| {
        let expected = verdicts
            .iter()
            .find(|(name, _)| *name == cell)
            .unwrap_or_else(|| panic!("no authored verdict for cell {cell:?}"))
            .1
            .clone();
        // The generated fixture must actually LAND in the cell it is filed
        // under, or the coverage check below would be measuring labels.
        assert_eq!(
            cell,
            decision_cell(&eqn, &coverage),
            "fixture/cell mismatch"
        );
        rows.push((cell, eqn, coverage, expected));
    };

    for (cell, eqn) in [
        ("Scalar/unfilled", datamodel::Equation::Scalar("NAN".into())),
        ("Scalar/filled", datamodel::Equation::Scalar("3 * x".into())),
        (
            "ApplyToAll/unfilled",
            datamodel::Equation::ApplyToAll(dims(), "nan".into()),
        ),
        (
            "ApplyToAll/filled",
            datamodel::Equation::ApplyToAll(dims(), "3 * x".into()),
        ),
    ] {
        let coverage = coverage_for(&eqn, false);
        push(cell.to_string(), eqn, coverage);
    }
    for arms in ARM_STATES {
        for default in DEFAULT_STATES {
            for coverage_name in COVERAGE_STATES {
                if arms == "no-arms" && coverage_name == "covers-every-slot" {
                    continue;
                }
                let eqn = arrayed_fixture(arms, default);
                let coverage = coverage_for(&eqn, coverage_name == "leaves-slots-uncovered");
                push(
                    format!("Arrayed/{arms}/{default}/{coverage_name}"),
                    eqn,
                    coverage,
                );
            }
        }
    }
    rows
}

#[test]
fn the_decision_table_pins_every_row() {
    for (cell, eqn, coverage, expected) in decision_table() {
        assert_eq!(
            expected,
            unfilled_arms(&eqn, &coverage),
            "decision-table cell {cell:?} disagrees"
        );
    }
}

/// An arm the compiler never selects cannot change ANY verdict.
///
/// Stated as an invariant over the whole table rather than as a fifth axis. An
/// ineffective arm is not a state the verdict depends on -- it is a thing that
/// must not matter -- and "adding one changes nothing, anywhere, whatever its
/// text" is both a stronger claim than 24 extra hand-authored rows and a much
/// smaller one. Both texts are exercised because the NaN one is the whole point
/// (it used to be reported as a slot that stops) and the filled one is the
/// control that would catch a filter keyed on the text instead of the subscript.
#[test]
fn an_arm_the_compiler_never_selects_cannot_change_the_verdict() {
    for (cell, eqn, selection, expected) in decision_table() {
        let datamodel::Equation::Arrayed(dim_names, arms, default, live) = &eqn else {
            continue;
        };
        for text in ["NAN", "7"] {
            // Appended: the arm sits at a position the selection does not name,
            // which is how an unknown subscript and a losing duplicate both
            // reach the classifier.
            let mut appended = arms.clone();
            appended.push(arm("ignored", text));
            let perturbed =
                datamodel::Equation::Arrayed(dim_names.clone(), appended, default.clone(), *live);
            assert_eq!(
                expected,
                unfilled_arms(&perturbed, &selection),
                "cell {cell:?}: an appended unselected `{text}` arm changed the verdict"
            );

            // Prepended, with every real arm's index shifted by one: this is the
            // SHADOWED-DUPLICATE shape specifically, where the arm the compiler
            // drops comes BEFORE the one it keeps. Appending alone would never
            // exercise it, since the survivor is always the later entry.
            let mut prepended = vec![arm("shadowed", text)];
            prepended.extend(arms.iter().cloned());
            let shifted =
                ArmSelection::new((1..=arms.len()).collect(), selection.default_is_selected());
            let perturbed =
                datamodel::Equation::Arrayed(dim_names.clone(), prepended, default.clone(), *live);
            assert_eq!(
                expected,
                unfilled_arms(&perturbed, &shifted),
                "cell {cell:?}: a shadowed leading `{text}` arm changed the verdict"
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
/// This is the check that makes the row count derived rather than asserted.
/// Set equality both ways is the point -- a subset test would let a shrinking
/// table pass, and the previous version of this file asserted only that the
/// three `Equation` variants appeared, which cannot see a missing combination
/// of the two `Arrayed` axes. Three cells were in fact missing.
///
/// What the pieces guarantee together: `equation_shape` and `decision_cell` are
/// catch-all-free matches over `datamodel::Equation`, so a new variant is a
/// compile error in both; the author must then name its cells, and this
/// assertion fails until `expected_cells` and the table agree about them.
#[test]
fn the_decision_table_covers_every_cell() {
    let covered: BTreeSet<String> = decision_table()
        .iter()
        .map(|(_, eqn, coverage, _)| decision_cell(eqn, coverage))
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

    // The shape list is still worth pinning on its own: it is what the two
    // catch-all-free matches above are enumerated FROM.
    let shapes: BTreeSet<&str> = decision_table()
        .iter()
        .map(|(_, eqn, _, _)| equation_shape(eqn))
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
