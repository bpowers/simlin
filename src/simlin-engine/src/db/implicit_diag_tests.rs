// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Attributing implicit-helper compile failures (GH #1000).
//!
//! GH #994 made a codegen rejection during per-variable fragment compilation
//! attributable for EXPLICIT variables (`compile_var_fragment` routes through
//! the reporting emission twin and accumulates a diagnostic naming the
//! variable and the refused construct). The implicit-helper path still
//! discarded its errors, so a
//! failure in a SMOOTH/DELAY/TREND capture helper surfaced only as
//! `assemble_module`'s unattributed batch message ("failed to compile
//! fragments for variables: $⁚out⁚0⁚arg0⁚c1") -- the GH #913 shape -- and
//! `collect_all_diagnostics` showed NOTHING at all.
//!
//! Two details carried over from the #994 landing (its commit's own
//! cautions):
//! * the helper name is interpolated into the MESSAGE, not left on the
//!   `variable` field alone -- `errors.rs`'s `Assembly` arm never renders
//!   that field and the CLI prints only the message;
//! * severity is `Error` because a dropped implicit fragment fails the
//!   build exactly as a dropped explicit one does -- verified by the same
//!   corpus-sweep shape as #994 (added rows land only on projects that
//!   already fail to compile; see the commit message).

use crate::db::{
    DiagnosticError, DiagnosticSeverity, SimlinDb, collect_all_diagnostics,
    sync_from_datamodel_incremental,
};
use crate::test_common::TestProject;

/// A scalar SMTH1 whose argument is an array slice: the hoisted helper
/// `$⁚out⁚0⁚arg0` holds `vals[*]` in a SCALAR equation, which codegen rejects
/// ("an array of shape [2] is used where a single value is required"). Two
/// such calls, so the attribution is once per helper. (Under an apply-to-all
/// parent the same argument is one element of the body and reads the active
/// element, as the plain equation does, so it compiles.)
/// The model does not compile -- the point of the fixture is what the
/// diagnostics SAY about that.
fn failing_implicit_fixture() -> TestProject {
    TestProject::new("implicit_fail")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges("vals[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .aux("out", "SMTH1(vals[*], 3) + SMTH1(vals[*], 5)", None)
        .flow("g", "(out + 1) * 0.01", None)
        .stock("s", "10", &["g"], &[], None)
}

/// The failing helper is named, with the codegen reason, in
/// `collect_all_diagnostics` -- not just in the assembly batch message.
#[test]
fn implicit_helper_codegen_failure_is_attributed() {
    let datamodel = failing_implicit_fixture().build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);

    let diags = collect_all_diagnostics(&db, sync.project);
    let attributed: Vec<&crate::db::Diagnostic> = diags
        .iter()
        .filter(|d| match &d.error {
            DiagnosticError::Assembly(msg) => {
                msg.contains("$\u{205A}out\u{205A}")
                    && msg.contains("\u{205A}arg0")
                    && msg.contains("failed to compile")
            }
            _ => false,
        })
        .collect();
    assert!(
        !attributed.is_empty(),
        "the failing implicit helper must be named in collect_all_diagnostics; got: {:?}",
        diags
            .iter()
            .map(|d| format!("{:?}: {:?}", d.variable, d.error))
            .collect::<Vec<_>>()
    );
    for d in &attributed {
        // The name must be IN the message (errors.rs's Assembly arm never
        // renders the `variable` field), and the codegen reason must ride
        // along -- an unexplained failure is the #913 shape this closes.
        let DiagnosticError::Assembly(msg) = &d.error else {
            unreachable!()
        };
        assert!(
            msg.contains("used where a single value is required") || msg.contains("codegen"),
            "the refused construct must be named: {msg}"
        );
        assert!(
            msg.contains("synthesized while parsing 'out'"),
            "the PARENT the helper was synthesized for must be named: {msg}"
        );
        assert_eq!(d.severity, DiagnosticSeverity::Error);
        assert!(d.variable.is_some(), "structured consumers read the field");
    }
    // One row per helper: the two failing phases (initial + flow) refuse the
    // same construct, and identical reasons collapse rather than duplicate.
    assert_eq!(
        attributed.len(),
        2,
        "exactly one row per failing helper (the two calls' arg0): {attributed:#?}"
    );
}

/// A helper whose BODY the compiler refuses is reported on the PARENT
/// variable, at the span of the argument inside the parent's equation, with
/// the code the plain equation is refused with -- not as an assembly row
/// naming a helper the user never wrote.
///
/// `x` is declared over `Region` and read as `x[Region]` inside an
/// apply-to-all over `State` with no relation between the two. The hoisted
/// helper is one element of that body, so the compiler refuses it exactly as
/// it refuses `target[State] = x[Region]`: `MismatchedDimensions`. The three
/// per-element helpers refuse the same construct at the same span, which is
/// one row, and the CLI's rendering underlines the argument in the parent's
/// equation.
#[test]
fn implicit_helper_lowering_failure_is_an_equation_error_on_the_parent() {
    use crate::common::ErrorCode;
    use crate::datamodel;

    let mut project = TestProject::new("implicit_lowering");
    project.dimensions = vec![
        datamodel::Dimension::named(
            "Region".to_string(),
            vec!["Ruby".to_string(), "Rose".to_string(), "Reed".to_string()],
        ),
        datamodel::Dimension::named(
            "State".to_string(),
            vec![
                "Steel".to_string(),
                "Slate".to_string(),
                "Stone".to_string(),
            ],
        ),
    ];
    let equation = "SMTH1(x[Region], 1)";
    let project = project
        .array_with_ranges(
            "x[Region]",
            vec![("Ruby", "10"), ("Rose", "20"), ("Reed", "30")],
        )
        .array_aux("target[State]", equation);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);

    let diags = collect_all_diagnostics(&db, sync.project);
    let on_parent: Vec<&crate::db::Diagnostic> = diags
        .iter()
        .filter(|d| {
            d.variable.as_deref() == Some("target")
                && matches!(
                    &d.error,
                    DiagnosticError::Equation(e) if e.code == ErrorCode::MismatchedDimensions
                )
        })
        .collect();
    assert_eq!(
        on_parent.len(),
        1,
        "one row on the parent for the three per-element helpers: {diags:#?}"
    );
    let DiagnosticError::Equation(err) = &on_parent[0].error else {
        unreachable!()
    };
    let argument = equation.find("x[Region]").unwrap();
    assert!(
        argument <= err.start as usize
            && err.end as usize <= argument + "x[Region]".len()
            && err.start < err.end,
        "the span indexes the argument inside the PARENT's equation: {err:?}"
    );
    assert!(
        !diags
            .iter()
            .any(|d| matches!(&d.error, DiagnosticError::Assembly(m) if m.contains("arg0"))),
        "no assembly row names the helper: {diags:#?}"
    );

    // The CLI's rendering of that diagnostic: the parent's equation with the
    // argument underlined.
    let rendered = crate::errors::format_diagnostic_with_datamodel(on_parent[0], &datamodel);
    let message = rendered.message.expect("a rendered message");
    let mut lines = message.lines();
    let (snippet, underline) = (lines.next().unwrap(), lines.next().unwrap());
    assert_eq!(snippet.trim(), equation);
    let underlined = &snippet[underline.find('~').unwrap()..=underline.rfind('~').unwrap()];
    assert!(
        "x[Region]".contains(underlined) && !underlined.is_empty(),
        "the underline sits inside the argument; rendered:\n{message}"
    );
    assert!(
        message.contains("variable 'target'") && message.contains("mismatched_dimensions"),
        "rendered:\n{message}"
    );
}

/// A COMPILING model with the same builtin gains no implicit-helper
/// diagnostics -- the severity argument rests on added rows landing only on
/// projects that already fail.
#[test]
fn compiling_smooth_model_gains_no_implicit_diagnostics() {
    let project = TestProject::new("implicit_ok")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("C", &["c1", "c2"])
        .array_with_ranges("vals[C]", vec![("c1", "s"), ("c2", "s * 2")])
        .array_aux("out[C]", "SMTH1(vals[C], 3)")
        .flow("g", "(out[c1] + 1) * 0.01", None)
        .stock("s", "10", &["g"], &[], None);
    let datamodel = project.build_datamodel();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);

    crate::db::compile_project_incremental(&db, sync.project, "main")
        .expect("the control fixture must compile");
    let diags = collect_all_diagnostics(&db, sync.project);
    let implicit_rows: Vec<String> = diags
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Assembly(msg) if msg.contains("arg0") => Some(msg.clone()),
            _ => None,
        })
        .collect();
    assert!(
        implicit_rows.is_empty(),
        "a compiling project must gain no implicit-helper diagnostics: {implicit_rows:?}"
    );
}
