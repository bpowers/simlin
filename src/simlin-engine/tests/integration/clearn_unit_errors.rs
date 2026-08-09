// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Exploratory / diagnostic harness for the C-LEARN unit-error flood.
//!
//! Loads the C-LEARN v77 Vensim model, collects every accumulated
//! diagnostic, and buckets the unit-related ones by a normalized message
//! "template" so we can see WHAT kinds of unit errors Simlin emits and how
//! many of each.  Run with:
//!
//!   cargo test -p simlin-engine --test integration -- --ignored --nocapture

use simlin_engine::common::{ErrorCode, UnitError};
use simlin_engine::db::{
    Diagnostic, DiagnosticError, SimlinDb, collect_all_diagnostics, sync_from_datamodel_incremental,
};
use simlin_engine::open_vensim;

fn load_clearn() -> simlin_engine::datamodel::Project {
    let mdl_content =
        std::fs::read_to_string("../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl")
            .expect("failed to read C-LEARN mdl -- run from the simlin-engine crate dir");
    open_vensim(&mdl_content).expect("open_vensim should parse C-LEARN without I/O errors")
}

fn diag_details(d: &Diagnostic) -> String {
    match &d.error {
        DiagnosticError::Unit(UnitError::ConsistencyError(_, _, Some(s))) => s.clone(),
        DiagnosticError::Unit(UnitError::InferenceError {
            details: Some(s), ..
        }) => s.clone(),
        DiagnosticError::Unit(UnitError::DefinitionError(_, Some(s))) => s.clone(),
        DiagnosticError::Unit(other) => format!("{:?}", other),
        DiagnosticError::Model(e) => e.details.as_deref().unwrap_or("").to_string(),
        _ => String::new(),
    }
}

/// Is this diagnostic unit-related (either a `Unit` variant or a model-level
/// `UnitMismatch`)?
fn is_unit_diag(d: &Diagnostic) -> bool {
    matches!(&d.error, DiagnosticError::Unit(_))
        || matches!(
            &d.error,
            DiagnosticError::Model(e) if e.code == ErrorCode::UnitMismatch
        )
}

/// Regression guard: the C-LEARN unit-error flood (481 spurious diagnostics)
/// must stay cleared.  Asserts the invariants established by the four fixes:
///
///   F1/F2 (yr/year alias): no diagnostic confuses `yr` with `year`.
///   F3 (unknown deps): no "can't find or no units for dependency" errors.
///   F4 (macro templates): no diagnostic is attributed to a macro model.
///
/// A small documented residual (~14) remains -- genuine-looking dimensional
/// subtleties Vensim tolerates (permafrost-methane `degreesc`, `ph`, an
/// IF-branch unit difference) plus one umbrella inference warning -- so we
/// also assert a coarse total bound to catch gross regressions without pinning
/// the exact residual.
///
/// Runs by default: loading the 1.4MB C-LEARN model, which is what this used to
/// be `#[ignore]`d for, is now under two seconds on a debug build.
#[test]
fn clearn_unit_error_flood_is_cleared() {
    let project = load_clearn();
    let macro_models: Vec<String> = project
        .models
        .iter()
        .filter(|m| m.macro_spec.is_some())
        .map(|m| m.name.clone())
        .collect();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, sync.project);
    let unit_diags: Vec<&Diagnostic> = diagnostics.iter().filter(|d| is_unit_diag(d)).collect();

    let render = |ds: &[&Diagnostic]| {
        ds.iter()
            .map(|d| format!("  {}.{:?}: {}", d.model, d.variable, diag_details(d)))
            .collect::<Vec<_>>()
            .join("\n")
    };

    // F3: unknown dependency units are tolerated, never a hard error.
    let cant_find: Vec<&Diagnostic> = unit_diags
        .iter()
        .copied()
        .filter(|d| diag_details(d).contains("can't find or no units for dependency"))
        .collect();
    assert!(
        cant_find.is_empty(),
        "F3 regression: {} 'can't find units for dependency' error(s):\n{}",
        cant_find.len(),
        render(&cant_find)
    );

    // F4: macro-marked models (templates) are never unit-checked.
    let macro_attributed: Vec<&Diagnostic> = unit_diags
        .iter()
        .copied()
        .filter(|d| macro_models.iter().any(|m| m == &d.model))
        .collect();
    assert!(
        macro_attributed.is_empty(),
        "F4 regression: {} diagnostic(s) attributed to macro models {:?}:\n{}",
        macro_attributed.len(),
        macro_models,
        render(&macro_attributed)
    );

    // F1/F2: `yr` and `year` resolve to the same canonical unit, so no
    // diagnostic mentions both as distinct units. ("year" does not contain the
    // substring "yr", so this only fires when both tokens genuinely appear.)
    let yr_year: Vec<&Diagnostic> = unit_diags
        .iter()
        .copied()
        .filter(|d| {
            let s = diag_details(d);
            s.contains("yr") && s.contains("year")
        })
        .collect();
    assert!(
        yr_year.is_empty(),
        "F1/F2 regression: {} diagnostic(s) confusing yr/year:\n{}",
        yr_year.len(),
        render(&yr_year)
    );

    // Coarse guard against gross regressions (was 481; documented residual ~14).
    assert!(
        unit_diags.len() <= 20,
        "unit-diagnostic count regressed to {} (expected the documented \
         residual of ~14); all unit diagnostics:\n{}",
        unit_diags.len(),
        render(&unit_diags)
    );
}
