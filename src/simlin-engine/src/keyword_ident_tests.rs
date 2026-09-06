// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end contract for a variable whose canonical name collides with an
//! equation-language KEYWORD (`if`, `then`, `else`, `not`, `mod`, `and`, `or`,
//! `nan` -- `lexer::KEYWORDS`).
//!
//! Neither XMILE nor Vensim reserves those words as variable names, and
//! canonicalization preserves them, so such a variable reaches the compiler
//! intact from an ordinary file. But `lexer::identifierish` resolves a bare word
//! against the keyword table BEFORE falling back to `Ident`, so the variable can
//! only ever be *referenced* double-quoted. Every producer of equation text has
//! to agree with the lexer about that; the single predicate that decides it is
//! `ast::needs_quoting`, which had no keyword clause until GH #976.
//!
//! What that cost: `patch`'s rename reprints every dependent equation through
//! `print_eqn`, so an unrelated rename dropped the quotes and wrote unparseable
//! text back into the persisted `datamodel::Project` -- a silently-corrupted
//! save, the worst outcome in this codebase. The tests below pin the whole
//! chain: both readers admit the name, the printer quotes it, a rename keeps the
//! model compiling, and the corrupting spelling never appears in stored text.

use crate::common::Ident;
use crate::datamodel;
use crate::db::{
    SimlinDb, collect_all_diagnostics, compile_project_incremental, sync_from_datamodel_incremental,
};
use crate::patch::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};

/// The equation-language keywords in their `name=` spelling.
///
/// Deliberately a literal list rather than a read of `lexer::KEYWORDS`: these
/// tests are the independent side of the contract, and a fixture derived from
/// the table under test could not notice that table losing an entry.
const KEYWORD_NAMES: [&str; 8] = ["if", "then", "else", "not", "mod", "and", "or", "nan"];

fn xmile_doc(vars: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months">
    <start>0</start><stop>1</stop><dt>1</dt>
  </sim_specs>
  <model><variables>{vars}</variables></model>
</xmile>"#
    )
}

/// The issue's model: `if = 3`, `b = "if" * 2`, `c = b + 1`.
const KEYWORD_MODEL_VARS: &str = r#"
        <aux name="if"><eqn>3</eqn></aux>
        <aux name="b"><eqn>&quot;if&quot; * 2</eqn></aux>
        <aux name="c"><eqn>b + 1</eqn></aux>"#;

fn read_xmile(vars: &str) -> datamodel::Project {
    crate::compat::open_xmile(&mut xmile_doc(vars).as_bytes()).expect("XMILE must parse")
}

/// Diagnostics production would report for `project`, via the incremental path.
fn diagnostics(project: &datamodel::Project) -> Vec<crate::db::Diagnostic> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::Off)
}

fn assert_compiles_clean(project: &datamodel::Project, what: &str) {
    let diags = diagnostics(project);
    assert!(
        diags.is_empty(),
        "{what} must compile with no diagnostics, got: {diags:?}"
    );

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    compile_project_incremental(&db, sync.project, "main", crate::db::LtmOverlay::Off)
        .unwrap_or_else(|e| panic!("{what} must compile: {e:?}"));
}

/// Reachability, XMILE side: the reader admits every keyword as a variable name
/// and hands the canonical name through unchanged.
#[test]
fn xmile_reader_admits_every_keyword_as_a_variable_name() {
    for name in KEYWORD_NAMES {
        let project = read_xmile(&format!(r#"<aux name="{name}"><eqn>3</eqn></aux>"#));
        let idents: Vec<&str> = project.models[0]
            .variables
            .iter()
            .map(|v| v.get_ident())
            .collect();
        assert_eq!(
            vec![name],
            idents,
            "canonicalization must preserve the keyword-shaped name"
        );
    }
}

/// Run `project` and return the final value of `var`.
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

/// Reachability, MDL side: Vensim reserves none of our keywords either, so the
/// importer has to emit a reference to one QUOTED.
///
/// The bare spelling is the one that matters and the one that was broken
/// (GH #976, review finding F1): a modeler writing plain Vensim does not quote,
/// because Vensim has nothing to quote against, so every reference to such a
/// variable imported to an `UnrecognizedToken`. Both spellings are covered here,
/// and the numeric assertion is what distinguishes "compiles" from "computes the
/// right thing".
///
/// `nan` is excluded and handled by
/// [`a_bare_nan_reference_in_mdl_is_still_the_literal`]: it is the one keyword
/// the importer deliberately does NOT quote, because the stored text `NAN` is
/// also how we represent Vensim's `A FUNCTION OF` placeholder. Enumerated as an
/// explicit exclusion rather than a shortened list so the asymmetry is visible.
#[test]
fn mdl_reader_admits_a_keyword_named_variable() {
    for keyword in KEYWORD_NAMES.into_iter().filter(|k| *k != "nan") {
        for reference in [keyword.to_string(), format!("\"{keyword}\"")] {
            let mdl = format!(
                "{keyword} = 3\n\t~\t\n\t~\t|\n\n\
                 b = {reference} * 2\n\t~\t\n\t~\t|\n\n\
                 c = b + 1\n\t~\t\n\t~\t|\n\n\
                 \\\\\\---/// Sketch information - do not modify anything except names\n"
            );
            let project = crate::compat::open_vensim(&mdl)
                .unwrap_or_else(|e| panic!("MDL must parse ({keyword}, {reference}): {e:?}"));

            let mut idents: Vec<&str> = project.models[0]
                .variables
                .iter()
                .map(|v| v.get_ident())
                .collect();
            idents.sort_unstable();
            let mut expected = vec!["b", "c", keyword];
            expected.sort_unstable();
            assert_eq!(expected, idents);

            assert_compiles_clean(
                &project,
                &format!("the MDL-imported model referencing `{reference}`"),
            );
            assert_eq!(
                6.0,
                final_value(&project, "b"),
                "`b = {reference} * 2` must be 6, not a bare-keyword misreading"
            );
        }
    }
}

/// The residual: a bare `nan` reference naming a declared variable STILL reads
/// as the NaN literal. Known, pre-existing, silent, and deliberately not fixed
/// here.
///
/// Every other keyword is quoted by the importer now, so `b = mod * 2` resolves
/// to the variable. `nan` cannot join them, and the reason is a representation
/// choice two layers away: our importer stores Vensim's `A FUNCTION OF(...)` --
/// "this variable has no equation" -- as the equation text `NAN`, and the MDL
/// writer prints that back as a bare `NaN`. Quoting `nan` here would bind that
/// placeholder to any variable named `nan`, so a round-tripped model would
/// compute a value for a variable that has none. That is a strictly worse trade
/// than the one this test pins.
///
/// Nor can the writer fix it. Vensim's documentation says `A FUNCTION OF` "is
/// not intended for use in writing equations, and precludes simulation", so
/// emitting the marker in place of a bare `NaN` produces a file Vensim cannot
/// run at all. There is no writer-side spelling that resolves this.
///
/// Fixing it properly means changing how the importer represents "no equation"
/// -- its own piece of work, not this one. This test exists so the residual is
/// recorded rather than rediscovered; it asserts today's WRONG behavior on
/// purpose, and reds if that ever changes.
#[test]
fn a_bare_nan_reference_in_mdl_is_still_the_literal() {
    let mdl = concat!(
        "nan = 3\n\t~\t\n\t~\t|\n\n",
        "b = nan * 2\n\t~\t\n\t~\t|\n\n",
        "\\\\\\---/// Sketch information - do not modify anything except names\n"
    );
    let project = crate::compat::open_vensim(mdl).expect("MDL must parse");

    // Compiles clean either way, which is what makes the case silent: the
    // diagnostics channel cannot see it, only the number can.
    assert_compiles_clean(&project, "the bare-`nan` MDL model");
    assert!(
        final_value(&project, "b").is_nan(),
        "KNOWN RESIDUAL: `b = nan * 2` still reads the NaN literal rather than \
         the declared variable `nan` (3). If this now computes 6, the importer's \
         `nan` exclusion was removed -- check that the `A FUNCTION OF` \
         placeholder round trip still works before accepting the change."
    );

    // The contrast that makes the exclusion narrow: every other keyword IS
    // resolved to the variable.
    let mdl_mod = concat!(
        "mod = 3\n\t~\t\n\t~\t|\n\n",
        "b = mod * 2\n\t~\t\n\t~\t|\n\n",
        "\\\\\\---/// Sketch information - do not modify anything except names\n"
    );
    let project = crate::compat::open_vensim(mdl_mod).expect("MDL must parse");
    assert_compiles_clean(&project, "the bare-`mod` MDL model");
    assert_eq!(6.0, final_value(&project, "b"));
}

/// What the `nan` exclusion buys: an `A FUNCTION OF()` placeholder survives
/// `project_to_mdl` -> re-import as a placeholder, **including in a model that
/// also declares a variable named `nan`**.
///
/// That combination is the reason `nan` is excluded from the importer's keyword
/// quoting. The writer spells a placeholder as a bare `NaN`, so quoting `nan`
/// would bind it to the like-named variable and `marketing` would compute `3`
/// instead of `NaN`, with zero diagnostics. One model can be both a declarer of
/// `nan` and a carrier of a placeholder, so no rule that inspects the model can
/// separate them -- and per Vensim's documentation `A FUNCTION OF` "precludes
/// simulation", so the writer cannot emit the marker instead. Excluding the one
/// keyword is the only move that costs nothing here.
///
/// The no-`nan` control is what keeps this from passing vacuously: it is the arm
/// that works regardless, so only the declared arm can catch a regression.
#[test]
fn a_placeholder_round_trips_even_when_nan_names_a_variable() {
    let with_nan = concat!(
        "nan = 3\n\t~\t\n\t~\t|\n\n",
        "marketing = A FUNCTION OF( nan )\n\t~\t\n\t~\t|\n\n",
        "\\\\\\---/// Sketch information - do not modify anything except names\n"
    );
    let without_nan = concat!(
        "other = 3\n\t~\t\n\t~\t|\n\n",
        "marketing = A FUNCTION OF( other )\n\t~\t\n\t~\t|\n\n",
        "\\\\\\---/// Sketch information - do not modify anything except names\n"
    );

    for (label, src) in [("declares `nan`", with_nan), ("control", without_nan)] {
        let first = crate::compat::open_vensim(src).expect("MDL must parse");
        assert_eq!(
            Some(datamodel::Equation::Scalar("NAN".to_string())),
            placeholder_equation(&first),
            "{label}: the placeholder must import as the NaN placeholder"
        );
        assert!(
            final_value(&first, "marketing").is_nan(),
            "{label}: a variable with no equation must evaluate to NaN"
        );

        let written = crate::mdl::project_to_mdl(&first).expect("MDL must write");
        let second = crate::compat::open_vensim(&written).expect("re-import must parse");
        // Compared case-insensitively: the placeholder arrives as `NAN` from the
        // importer's `A FUNCTION OF` arm and comes back as `NaN` from the
        // writer's `Const` text. Both lex to the NaN literal, and that -- not
        // the spelling -- is the property. The value assertion below is the one
        // that would catch the reference binding the review found.
        let round_tripped = placeholder_equation(&second).expect("placeholder must survive");
        let datamodel::Equation::Scalar(text) = &round_tripped else {
            panic!("{label}: expected a scalar equation, got {round_tripped:?}");
        };
        assert!(
            text.eq_ignore_ascii_case("nan"),
            "{label}: the placeholder must survive the round trip as the NaN \
             literal, not as a reference; got {text:?}"
        );
        assert!(
            final_value(&second, "marketing").is_nan(),
            "{label}: the round-tripped placeholder must still be NaN, not the \
             value of a like-named variable"
        );
    }
}

fn placeholder_equation(project: &datamodel::Project) -> Option<datamodel::Equation> {
    project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "marketing")
        .and_then(|v| v.get_equation().cloned())
}

/// The keyword-named model compiles clean from XMILE too, and computes the
/// right number. A loud rejection here would be a wrong rejection: the name is
/// legal in both source formats.
#[test]
fn a_keyword_named_variable_compiles_clean() {
    let project = read_xmile(KEYWORD_MODEL_VARS);
    assert_compiles_clean(&project, "the keyword-named model");

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled =
        compile_project_incremental(&db, sync.project, "main", crate::db::LtmOverlay::Off)
            .expect("model must compile");
    assert!(
        compiled.offsets.contains_key(&Ident::new("if")),
        "the keyword-named variable must reach the compiled model"
    );

    // `if = 3`, `b = "if" * 2`, `c = b + 1`: the numbers are what separate
    // "compiles" from "means what the modeler wrote".
    assert_eq!(6.0, final_value(&project, "b"));
    assert_eq!(7.0, final_value(&project, "c"));
}

/// GH #976's verified repro. Renaming an UNRELATED variable (`b` -> `bee`)
/// reprints every dependent equation through `expr2_to_string` -> `print_ident`.
/// With no keyword clause in `needs_quoting` the reference to `if` came back
/// bare, so the patched -- and PERSISTED -- datamodel held `bee = if * 2`, which
/// no longer parses: an edit that touched neither `if` nor its reader silently
/// destroyed a valid saved model.
#[test]
fn renaming_an_unrelated_variable_preserves_a_keyword_reference() {
    let mut project = read_xmile(KEYWORD_MODEL_VARS);
    assert_compiles_clean(&project, "the pre-rename model");

    apply_patch(
        &mut project,
        ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "b".to_string(),
                    to: "bee".to_string(),
                }],
            }],
        },
    )
    .expect("rename must apply");

    assert_eq!(
        Some(datamodel::Equation::Scalar("\"if\" * 2".to_string())),
        project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "bee")
            .and_then(|v| v.get_equation().cloned()),
        "the reprinted equation must keep the quotes the lexer requires"
    );
    assert_compiles_clean(&project, "the renamed model");
}

/// The mirror direction: renaming a variable TO a keyword. Every equation that
/// referenced it is reprinted with the new name, so the new name is the one that
/// has to come out quoted.
#[test]
fn renaming_a_variable_to_a_keyword_keeps_the_model_compiling() {
    for name in KEYWORD_NAMES {
        let mut project = read_xmile(
            r#"
        <aux name="a"><eqn>3</eqn></aux>
        <aux name="c"><eqn>a + 1</eqn></aux>"#,
        );

        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::RenameVariable {
                        from: "a".to_string(),
                        to: name.to_string(),
                    }],
                }],
            },
        )
        .expect("rename must apply");

        assert_eq!(
            Some(datamodel::Equation::Scalar(format!("\"{name}\" + 1"))),
            project.models[0]
                .variables
                .iter()
                .find(|v| v.get_ident() == "c")
                .and_then(|v| v.get_equation().cloned()),
            "a reference to the renamed `{name}` must be quoted"
        );
        assert_compiles_clean(&project, &format!("the model renamed to `{name}`"));
    }
}

/// The one name shape a rename must REFUSE (review finding F2).
///
/// A canonical name containing `"` has no spelling at all -- the lexer's quoted
/// identifier ends at the first `"` and there is no escape -- so a rename to one
/// used to return `Ok(())` and persist `c = "x"y" + 1`, turning a valid saved
/// model into one that no longer compiles. Same class and same entry point as
/// GH #976, so it gets the same treatment, at the front door instead of in the
/// printer: nothing is lost by refusing a name that nothing could reference.
///
/// The `x$y` control is what keeps the rejection narrow: `$` is equally
/// unspellable BARE, but it has a working quoted spelling, so it must still be
/// allowed and must still compile.
#[test]
fn renaming_to_an_unspellable_name_is_refused() {
    const MODEL: &str = r#"
        <aux name="a"><eqn>3</eqn></aux>
        <aux name="c"><eqn>a + 1</eqn></aux>"#;

    let rename_to = |target: &str| {
        let mut project = read_xmile(MODEL);
        let result = apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::RenameVariable {
                        from: "a".to_string(),
                        to: target.to_string(),
                    }],
                }],
            },
        );
        (result, project)
    };

    let (result, project) = rename_to("x\"y");
    let err = result.expect_err("a rename to a `\"`-bearing name must be refused");
    assert_eq!(crate::common::ErrorCode::UnclosedQuotedIdent, err.code);
    assert_compiles_clean(&project, "the model a refused rename left alone");
    assert!(
        project.models[0]
            .variables
            .iter()
            .any(|v| v.get_ident() == "a"),
        "a refused rename must not have partially applied"
    );

    // Control: `$` is unspellable bare but spellable quoted, so it is allowed
    // and the reprinted reference compiles.
    let (result, project) = rename_to("x$y");
    result.expect("a quotable name must still be renameable");
    assert_eq!(
        Some(datamodel::Equation::Scalar("\"x$y\" + 1".to_string())),
        project.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "c")
            .and_then(|v| v.get_equation().cloned()),
    );
    assert_compiles_clean(&project, "the model renamed to `x$y`");
}
