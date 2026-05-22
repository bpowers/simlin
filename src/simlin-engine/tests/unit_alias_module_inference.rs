// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Minimal reproducers for the C-LEARN unit-error flood, isolated from the
//! 1.4MB model.  Each test targets one root cause:
//!
//!   F2: unit *inference* builds the Time variable and the stock/flow
//!       constraint from the raw `sim_specs.time_units` string instead of
//!       resolving it through the units `Context`'s alias map, so a model that
//!       declares some units with an aliased time name (`yr`) and uses the
//!       primary (`year`) elsewhere produces a spurious `year` vs `yr`
//!       inference mismatch even after the `Context` self-alias fix.
//!
//!   F3: unit *checking* must treat a dependency whose units are unknown (a
//!       module output / synthesized helper that inference did not resolve) as
//!       unconstrained -- skipping the consistency check -- rather than
//!       emitting a hard "can't find or no units for dependency" error.  The
//!       array-element path already does this (units_check.rs); the scalar /
//!       binary path must too.
//!
//!   F4: unit checking must skip macro-marked models (generic templates whose
//!       formal parameters are unitless), exactly as it already skips stdlib
//!       models.

use simlin_engine::common::UnitError;
use simlin_engine::db::{
    Diagnostic, DiagnosticError, SimlinDb, collect_all_diagnostics, sync_from_datamodel_incremental,
};
use simlin_engine::test_common::TestProject;

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

/// Collect every diagnostic (across all models) for a built datamodel.
fn diagnostics_for(project: &simlin_engine::datamodel::Project) -> Vec<Diagnostic> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    collect_all_diagnostics(&db, &sync.to_sync_result())
}

/// F2: a stock/flow pair whose flow units are declared with the *aliased*
/// time name (`yr`) while the model's time units use the *primary* name
/// (`year`).  Vensim treats `Yr,year,years,yr,...` as one unit, so this model
/// is dimensionally consistent and must produce no `year`/`yr` mismatch.
///
/// The `Context` self-alias fix (F1) makes declared-vs-declared comparisons
/// resolve, but unit *inference* still injects the raw time-unit name `year`
/// into the stock/flow constraint, conflicting with the flow's declared `yr`.
#[test]
fn aliased_time_unit_does_not_cause_inference_mismatch() {
    // `Yr` is the primary; `yr` and `year` are aliases.  `year` is also the
    // declared time unit -- exactly C-LEARN's `22:Yr,year,years,yr,Year,Years`.
    let project = TestProject::new("aliased_time")
        .with_time_units("year")
        .unit_with_aliases("Yr", &["year", "years", "yr", "Year", "Years"])
        .unit("widgets", None)
        .stock_with_units("store", "0", &["inflow"], &[], Some("widgets"))
        .flow_with_units("inflow", "1", Some("widgets/yr"))
        .build_datamodel();

    // The model is fully dimensionally consistent (store: widgets, inflow:
    // widgets/yr == widgets/year), so there must be no unit diagnostics at all.
    let diagnostics = diagnostics_for(&project);

    assert!(
        diagnostics.is_empty(),
        "aliased time unit (yr == year) must not produce any unit diagnostic. Got {}:\n{}",
        diagnostics.len(),
        diagnostics
            .iter()
            .map(|d| format!("  {}.{:?}: {}", d.model, d.variable, diag_details(d)))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// F3: a variable with declared units that references a dependency whose units
/// are unknown must NOT produce a hard "can't find or no units for dependency"
/// error.  Unknown units are not a mismatch -- like Vensim (which only warns
/// that the dependency itself lacks units) and like the existing arrayed
/// element path in `units_check`, the consistency check must be skipped when a
/// dependency's units cannot be determined.  This is the mechanism behind the
/// 366 main-model "can't find units" errors in C-LEARN (references to module
/// outputs / synthesized helper auxes whose units inference left unresolved).
///
/// We force a genuinely-unresolvable dependency with an under-determined
/// product: `result (meters) = driver1 * driver2` constrains only the PRODUCT
/// `driver1 * driver2 == meters`, so inference cannot isolate either factor and
/// leaves both unresolved.  (A direct `result = driver` would let inference
/// back-propagate `result`'s declared units onto `driver`.)
#[test]
fn unknown_dependency_units_do_not_hard_error() {
    let project = TestProject::new("unknown_dep")
        .with_time_units("year")
        .unit("meters", None)
        // Neither driver has declared units, and the product under-determines
        // them, so both are unknown-unit dependencies of `result`.
        .aux("driver1", "5", None)
        .aux("driver2", "3", None)
        .aux_with_units("result", "driver1 * driver2", Some("meters"))
        .build_datamodel();

    let diagnostics = diagnostics_for(&project);

    let cant_find: Vec<_> = diagnostics
        .iter()
        .filter(|d| diag_details(d).contains("can't find or no units for dependency"))
        .collect();

    assert!(
        cant_find.is_empty(),
        "a reference to an unknown-units dependency must not hard-error. Got {}:\n{}",
        cant_find.len(),
        cant_find
            .iter()
            .map(|d| format!("  {}.{:?}: {}", d.model, d.variable, diag_details(d)))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Regression guard for F3: tolerating *unknown* dependency units must NOT
/// silence a *genuine* mismatch between two KNOWN units.  `result` is declared
/// `meters` but computes `seconds`, so a "computed units don't match" error
/// must still be reported.
#[test]
fn known_unit_mismatch_is_still_caught() {
    let project = TestProject::new("known_mismatch")
        .with_time_units("year")
        .unit("meters", None)
        .unit("seconds", None)
        .aux_with_units("source", "5", Some("seconds"))
        .aux_with_units("result", "source", Some("meters"))
        .build_datamodel();

    let diagnostics = diagnostics_for(&project);

    let mismatch: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            let details = diag_details(d);
            details.contains("computed units 'seconds' don't match specified units")
        })
        .collect();

    assert!(
        !mismatch.is_empty(),
        "a genuine meters-vs-seconds mismatch must still be caught. All diagnostics:\n{}",
        diagnostics
            .iter()
            .map(|d| format!("  {}.{:?}: {}", d.model, d.variable, diag_details(d)))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
