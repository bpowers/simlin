// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Reproducer tests for two unit-error bugs found in the World3-03 Vensim model.
//!
//! Bug 1 (delay3 conflation): the stdlib `delay3` / `delay1` stock-init
//!   equation has the form `(if isModuleInput(initial_value) then initial_value
//!   else input) * delay_time`.  The stdlib-module argument compatibility check
//!   in `db::check_model_units` extracts ALL identifiers referenced by the init
//!   AST and pairwise-compares their declared units, so `input` (pollution/year)
//!   and `delay_time` (year) are wrongly flagged as conflicting even though
//!   `delay_time` participates only as a coefficient.  Only the identifiers in
//!   the value branches of the `if-then-else` (here: `initial_value` and
//!   `input`) need to share units.
//!
//! Bug 2 (resource_unit alias): the World3-03 MDL footer declares
//!   `22:Resource unit,Resource units` making them unit-equivalent.  Despite
//!   the alias entry being installed in the `units::Context`, the unit
//!   inference and checking paths produce mismatch diagnostics citing both
//!   `resource_unit` and `resource_units` as distinct units.

use simlin_engine::common::{ErrorCode, UnitError};
use simlin_engine::db::{
    Diagnostic, DiagnosticError, SimlinDb, collect_all_diagnostics, sync_from_datamodel_incremental,
};
use simlin_engine::open_vensim;

/// Extract a human-readable detail string from a diagnostic's unit-error payload.
///
/// `DiagnosticError::Unit` is the most common shape for unit errors, but a few
/// inference-driven mismatches land in `DiagnosticError::Model`.  We collapse
/// both into a single string so filters can use simple substring checks.
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

/// Test whether `needle` occurs in `haystack` as a distinct identifier token
/// (not merely as a prefix/suffix of a longer identifier).  Treats ASCII
/// alphanumeric characters and `_` as identifier characters; any other byte
/// is a boundary.  This lets us distinguish `resource_unit` from
/// `resource_units` (which has `resource_unit` as a prefix).
fn contains_token(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let hbytes = haystack.as_bytes();
    let nbytes = needle.as_bytes();
    if hbytes.len() < nbytes.len() {
        return false;
    }
    let is_id = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i + nbytes.len() <= hbytes.len() {
        if &hbytes[i..i + nbytes.len()] == nbytes {
            let before_ok = i == 0 || !is_id(hbytes[i - 1]);
            let after_ok = i + nbytes.len() == hbytes.len() || !is_id(hbytes[i + nbytes.len()]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn load_wrld3() -> simlin_engine::datamodel::Project {
    let mdl_content = std::fs::read_to_string("../../test/metasd/WRLD3-03/wrld3-03.mdl").expect(
        "failed to read wrld3-03.mdl -- run tests from the repo root or simlin-engine crate",
    );
    open_vensim(&mdl_content).expect("open_vensim should parse wrld3-03.mdl without I/O errors")
}

/// Temporary debug: print the parsed unit strings and the unit equivalences.
#[test]
#[ignore]
fn debug_wrld3_parsed_units() {
    let project = load_wrld3();
    for model in &project.models {
        for var in &model.variables {
            let (ident, units) = match var {
                simlin_engine::datamodel::Variable::Aux(a) => (a.ident.clone(), a.units.clone()),
                simlin_engine::datamodel::Variable::Flow(f) => (f.ident.clone(), f.units.clone()),
                simlin_engine::datamodel::Variable::Stock(s) => (s.ident.clone(), s.units.clone()),
                _ => continue,
            };
            if let Some(ref u) = units
                && u.to_lowercase().contains("resource")
            {
                println!("{}: units={:?}", ident, u);
            }
        }
    }
    println!("units equivalences:");
    for u in &project.units {
        println!("  name={:?} aliases={:?}", u.name, u.aliases);
    }
}

/// Verify that World3 loads without parse-level errors (the model is valid MDL).
#[test]
fn wrld3_parses_without_hard_errors() {
    let project = load_wrld3();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync_result = sync.to_sync_result();
    let diagnostics = collect_all_diagnostics(&db, &sync_result);

    // Unit errors are non-fatal warnings -- they must NOT prevent the model from
    // having a parseable datamodel.  Check that no *blocking* equation errors exist.
    let blocking: Vec<_> = diagnostics
        .iter()
        .filter(|d| match &d.error {
            DiagnosticError::Unit(_) => false,
            DiagnosticError::Model(e) if e.code == ErrorCode::UnitMismatch => false,
            _ => true,
        })
        .collect();
    assert!(
        blocking.is_empty(),
        "World3 should parse cleanly; unexpected blocking diagnostics:\n{}",
        blocking
            .iter()
            .map(|d| format!("  {}.{:?}: {:?}", d.model, d.variable, d.error))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Bug 2 repro: the engine currently reports unit mismatches between
/// `resource_unit` and `resource_units` even though the model's footer declares
/// `22:Resource unit,Resource units` making them aliases of the same canonical
/// unit.
///
/// Expected (correct) behaviour: the alias resolves both identifiers to the
/// same canonical unit name, so no unit diagnostic should cite both tokens as
/// distinct unit names.  This test currently FAILS; it must PASS after the
/// alias-resolution fix.
#[test]
fn wrld3_resource_unit_alias_should_not_conflict() {
    let project = load_wrld3();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync_result = sync.to_sync_result();
    let diagnostics = collect_all_diagnostics(&db, &sync_result);

    // Match ANY unit-related diagnostic that mentions BOTH `resource_unit` and
    // `resource_units` as distinct identifier tokens in its Debug string OR
    // its detail message.  `resource_units` contains `resource_unit` as a
    // prefix, so a naive `contains` check would match both tokens in strings
    // that only reference `resource_units`.  `contains_token` enforces
    // word-boundary semantics so we detect only genuine confusion where BOTH
    // names appear as distinct unit identifiers.
    let conflicts: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            // Only consider unit-related diagnostics (either Unit variant or
            // model-level UnitMismatch).
            let is_unit_diag = matches!(&d.error, DiagnosticError::Unit(_))
                || matches!(
                    &d.error,
                    DiagnosticError::Model(e) if e.code == ErrorCode::UnitMismatch
                );
            if !is_unit_diag {
                return false;
            }
            // Combine the Debug representation (catches unit-map keys) with
            // the human-readable details (catches rendered unit strings).
            let combined = format!("{:?} :: {}", &d.error, diag_details(d));
            contains_token(&combined, "resource_unit")
                && contains_token(&combined, "resource_units")
        })
        .collect();

    assert!(
        conflicts.is_empty(),
        "Bug 2: engine should not report resource_unit/resource_units as distinct \
         units when the model declares '22:Resource unit,Resource units'. \
         Got {} conflict(s):\n{}",
        conflicts.len(),
        conflicts
            .iter()
            .map(|d| format!("  {}.{:?}: {:?}", d.model, d.variable, d.error))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Bug 1 repro (minimal form): DELAY3's argument-compatibility check treats
/// the first argument (input, units X/time) and the second argument
/// (delay_time, units time) as "arguments that feed the same internal
/// variable", producing a spurious ConsistencyError.  In a delay function,
/// `input` and `delay_time` have inherently different roles and different
/// units; only `input` and `initial_value` must match.
///
/// Strengthened over the previous version: we now collect diagnostics
/// directly and assert that no `DiagnosticError::Unit` cites BOTH the
/// generation-rate and the transmission-delay identifiers as conflicting.
/// The earlier `assert_compiles_incremental()` check did not examine unit
/// warnings, so the bug was invisible to it.
#[test]
fn delay3_input_and_delay_time_different_units_is_valid() {
    use simlin_engine::test_common::TestProject;

    // Minimal reproducer: DELAY3(rate, delay) where rate has units
    // "pollution/year" and delay has units "year".  This is the exact
    // pattern from World3's persistent_pollution_appearance_rate.
    let project = TestProject::new("delay3_valid_units")
        .with_time_units("year")
        .unit("pollution", None)
        .unit("year", None)
        .aux_with_units("generation_rate", "100", Some("pollution/year"))
        .aux_with_units("transmission_delay", "20", Some("year"))
        .aux_with_units(
            "appearance_rate",
            "DELAY3(generation_rate, transmission_delay)",
            Some("pollution/year"),
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, &sync.to_sync_result());

    let spurious: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            if !matches!(&d.error, DiagnosticError::Unit(_)) {
                return false;
            }
            let details = diag_details(d);
            // The current buggy path emits a message citing both the
            // generation-rate source and the transmission-delay source.
            details.contains("generation_rate") && details.contains("transmission_delay")
        })
        .collect();

    assert!(
        spurious.is_empty(),
        "Bug 1: DELAY3(generation_rate, transmission_delay) should not emit a \
         unit mismatch citing BOTH argument identifiers. Got {} spurious \
         diagnostic(s):\n{}",
        spurious.len(),
        spurious
            .iter()
            .map(|d| format!("  {}.{:?}: {:?}", d.model, d.variable, d.error))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Regression guard: the 3-argument form of DELAY3, where all unit pairings
/// are valid, must also compile without spurious unit diagnostics on the
/// input/delay_time pair.
#[test]
fn delay3_with_explicit_initial_value_does_not_falsely_flag_delay_time() {
    use simlin_engine::test_common::TestProject;

    // Three-argument form: DELAY3(input, delay_time, initial_value).
    // Only input and initial_value need matching units; delay_time is independent.
    let project = TestProject::new("delay3_three_arg_valid")
        .with_time_units("year")
        .unit("pollution", None)
        .unit("year", None)
        .aux_with_units("generation_rate", "100", Some("pollution/year"))
        .aux_with_units("transmission_delay", "20", Some("year"))
        .aux_with_units("initial_pollution_rate", "80", Some("pollution/year"))
        .aux_with_units(
            "appearance_rate",
            "DELAY3(generation_rate, transmission_delay, initial_pollution_rate)",
            Some("pollution/year"),
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, &sync.to_sync_result());

    let spurious: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            if !matches!(&d.error, DiagnosticError::Unit(_)) {
                return false;
            }
            let details = diag_details(d);
            details.contains("generation_rate") && details.contains("transmission_delay")
        })
        .collect();

    assert!(
        spurious.is_empty(),
        "Bug 1 regression guard: DELAY3(rate, delay, initial) must not emit a \
         unit mismatch pairing rate and delay when all arguments have valid \
         independent units. Got {} spurious diagnostic(s):\n{}",
        spurious.len(),
        spurious
            .iter()
            .map(|d| format!("  {}.{:?}: {:?}", d.model, d.variable, d.error))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Regression guard: a genuine initial_value vs input unit mismatch MUST
/// still be caught.  After fixing Bug 1, we must not silence real errors:
/// this test looks specifically for a diagnostic citing `bad_initial` (the
/// culprit) together with `generation_rate` (the intended input), which is
/// the pair that truly must share units.
#[test]
fn delay3_initial_value_unit_mismatch_is_caught() {
    use simlin_engine::test_common::TestProject;

    let project = TestProject::new("delay3_bad_initial")
        .with_time_units("year")
        .unit("pollution", None)
        .unit("widgets", None)
        .unit("year", None)
        .aux_with_units("generation_rate", "100", Some("pollution/year"))
        .aux_with_units("transmission_delay", "20", Some("year"))
        // Wrong: initial_value should have same units as input (pollution/year),
        // but it's declared as widgets.
        .aux_with_units("bad_initial", "50", Some("widgets"))
        .aux_with_units(
            "appearance_rate",
            "DELAY3(generation_rate, transmission_delay, bad_initial)",
            Some("pollution/year"),
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, &sync.to_sync_result());

    let real_mismatch: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            if !matches!(&d.error, DiagnosticError::Unit(_)) {
                return false;
            }
            let details = diag_details(d);
            // The message must cite the (input, bad_initial) pair and call
            // out the widgets-vs-pollution-rate conflict.
            details.contains("generation_rate") && details.contains("bad_initial")
        })
        .collect();

    assert!(
        !real_mismatch.is_empty(),
        "Genuine initial_value mismatch (widgets vs pollution/year) was NOT \
         caught: no diagnostic cited generation_rate and bad_initial as \
         conflicting inputs. All diagnostics:\n{}",
        diagnostics
            .iter()
            .map(|d| format!("  {}.{:?}: {:?}", d.model, d.variable, d.error))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Regression guard for the smth3 stdlib module.  The smth3 stock-init
/// equation (`if isModuleInput(initial_value) then initial_value else input`)
/// does not multiply by `delay_time`, so the current buggy path does not
/// happen to trip here.  This test exists so that if someone changes the
/// smth3 definition to include a delay_time factor in the stock init, OR
/// changes the argument-unit-check to over-approximate, we catch the
/// regression immediately.  smth3 takes (input, averaging_time) where
/// averaging_time has units of time and the input has whatever units --
/// they must NOT be paired as conflicting.
#[test]
fn smth3_input_and_averaging_time_different_units_is_valid() {
    use simlin_engine::test_common::TestProject;

    let project = TestProject::new("smth3_valid_units")
        .with_time_units("year")
        .unit("widgets", None)
        .unit("year", None)
        .aux_with_units("noisy_signal", "100", Some("widgets"))
        .aux_with_units("averaging_time", "5", Some("year"))
        .aux_with_units(
            "smoothed_signal",
            "SMTH3(noisy_signal, averaging_time)",
            Some("widgets"),
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, &sync.to_sync_result());

    let spurious: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            if !matches!(&d.error, DiagnosticError::Unit(_)) {
                return false;
            }
            let details = diag_details(d);
            details.contains("noisy_signal") && details.contains("averaging_time")
        })
        .collect();

    assert!(
        spurious.is_empty(),
        "smth3(noisy_signal, averaging_time) must not pair the input and \
         averaging-time arguments as conflicting. Got {} diagnostic(s):\n{}",
        spurious.len(),
        spurious
            .iter()
            .map(|d| format!("  {}.{:?}: {:?}", d.model, d.variable, d.error))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Regression guard for smth3: a genuine initial_value vs input unit
/// mismatch MUST still be caught after the fix.
#[test]
fn smth3_initial_value_unit_mismatch_is_caught() {
    use simlin_engine::test_common::TestProject;

    let project = TestProject::new("smth3_bad_initial")
        .with_time_units("year")
        .unit("widgets", None)
        .unit("apples", None)
        .unit("year", None)
        .aux_with_units("noisy_signal", "100", Some("widgets"))
        .aux_with_units("averaging_time", "5", Some("year"))
        // Wrong: initial_value should have same units as input (widgets),
        // but it's declared as apples.
        .aux_with_units("bad_initial", "50", Some("apples"))
        .aux_with_units(
            "smoothed_signal",
            "SMTH3(noisy_signal, averaging_time, bad_initial)",
            Some("widgets"),
        )
        .build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, &sync.to_sync_result());

    let real_mismatch: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            if !matches!(&d.error, DiagnosticError::Unit(_)) {
                return false;
            }
            let details = diag_details(d);
            details.contains("noisy_signal") && details.contains("bad_initial")
        })
        .collect();

    assert!(
        !real_mismatch.is_empty(),
        "Genuine smth3 initial_value mismatch (apples vs widgets) was NOT \
         caught: no diagnostic cited noisy_signal and bad_initial as \
         conflicting inputs. All diagnostics:\n{}",
        diagnostics
            .iter()
            .map(|d| format!("  {}.{:?}: {:?}", d.model, d.variable, d.error))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Bug 1 repro (World3 level): the generated `delay3` sub-module for
/// `persistent_pollution_appearance_rate` must NOT carry a spurious unit
/// mismatch citing the generation-rate and transmission-delay inputs.
/// (Renamed from `..._currently_has_spurious_unit_error`: the assertion
/// direction flipped once Bug 1 was fixed.)
#[test]
fn wrld3_delay3_pollution_variable_has_no_spurious_unit_error() {
    let project = load_wrld3();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync_result = sync.to_sync_result();
    let diagnostics = collect_all_diagnostics(&db, &sync_result);

    // The variable name as it appears in the diagnostic (synthesized module name).
    let target_var_substring = "persistent_pollution_appearance_rate";

    let spurious: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            if !matches!(&d.error, DiagnosticError::Unit(_)) {
                return false;
            }
            let var_matches = d
                .variable
                .as_deref()
                .map(|v| v.contains(target_var_substring))
                .unwrap_or(false);
            if !var_matches {
                return false;
            }
            let details = diag_details(d);
            // Spurious means the message pairs the two World3 identifiers:
            // persistent_pollution_generation_rate and
            // persistent_pollution_transmission_delay.
            details.contains("persistent_pollution_generation_rate")
                && details.contains("persistent_pollution_transmission_delay")
        })
        .collect();

    assert!(
        spurious.is_empty(),
        "Bug 1: World3 delay3 module for persistent_pollution_appearance_rate \
         must not emit a spurious unit mismatch pairing generation_rate and \
         transmission_delay (they legitimately have different units). Got {} \
         diagnostic(s):\n{}",
        spurious.len(),
        spurious
            .iter()
            .map(|d| format!("  {}.{:?}: {:?}", d.model, d.variable, d.error))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
