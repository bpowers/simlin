// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// Pure validation logic for incoming save requests. Given the canonical
// JSON the Editor produced and a baseline of pre-edit error keys, this
// module returns either the parsed project + the set of *new* errors the
// edit would introduce, or a parse failure.
//
// The baseline mechanism (mirroring `simlin-mcp::EditModel`) is what
// allows a save that *fixes* some errors without introducing any new
// ones to be accepted. Without it, any project that opens with errors
// (which is common in real-world models) would become uneditable.

//! Pure validation pipeline for the save handler.
//!
//! Side-effect free: all inputs come in by value/reference, all outputs
//! are returned. The handler is responsible for I/O (reading the current
//! file, writing the new one) and for routing the outcomes back to the
//! HTTP response.

use std::collections::HashSet;

use simlin_engine::datamodel;
use simlin_engine::db::{
    DiagnosticSeverity, SimlinDb, collect_all_diagnostics, sync_from_datamodel,
};
use simlin_engine::errors::collect_formatted_errors;
use simlin_engine::json;

use crate::handlers::ValidationError;

/// Successful validation outcome. `new_errors` is empty when the save
/// introduces no errors that weren't already present in the baseline.
/// Otherwise it carries only the *new* errors so the handler can render a
/// 422 response that shows the user what their edit broke (rather than
/// also re-listing pre-existing errors which were already there).
#[derive(Debug)]
pub struct ValidationOutcome {
    pub project: datamodel::Project,
    pub new_errors: Vec<ValidationError>,
}

/// Pre-edit error keys captured from the *current* on-disk project, used
/// to filter post-edit errors so we only reject the save on *new* errors.
///
/// The key shape `(code, variable_name)` mirrors `simlin-mcp::EditModel`'s
/// error-classification scheme. Comparing on the variable name (not just
/// the code) lets us detect "the same kind of error moved to a different
/// variable" as a new error.
#[derive(Debug, Default, Clone)]
pub struct BaselineErrors {
    pub keys: HashSet<(String, Option<String>)>,
}

/// Failure modes for `validate_save`. `JsonParse` is distinguished from
/// model-level errors so the handler can return 400 (the request body
/// itself is malformed) vs. 422 (the project the body decoded to is
/// internally inconsistent).
#[derive(Debug)]
pub enum ValidationFailure {
    /// `serde_json::from_str::<json::Project>` failed.
    JsonParse(serde_json::Error),
}

impl std::fmt::Display for ValidationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationFailure::JsonParse(e) => write!(f, "json parse error: {e}"),
        }
    }
}

impl std::error::Error for ValidationFailure {}

/// Capture the pre-edit error set for `project`. The handler calls this
/// against the *current on-disk* project so that errors which already
/// exist in the file are not held against an incoming save.
///
/// This re-runs the engine's full diagnostic pipeline on every save. For
/// Phase 2 the cost is acceptable (single-process, single-user, model
/// sizes in the kilobytes); Phase 3's Loro doc cache eliminates the
/// re-parse by keeping the parsed project resident.
pub fn compute_baseline(project: &datamodel::Project) -> BaselineErrors {
    BaselineErrors {
        keys: error_keys_for(project),
    }
}

/// Validate an incoming save body. Pipeline:
///
/// 1. Parse `json` as a `json::Project` (camelCase schema; produced by
///    the Editor's `engine.serializeJson()`).
/// 2. Convert into the engine's `datamodel::Project`.
/// 3. Run the engine's salsa-based diagnostic pipeline; filter to
///    `DiagnosticSeverity::Error`; format via `collect_formatted_errors`.
/// 4. Subtract `baseline.keys` to keep only NEW errors.
///
/// The handler decides whether to gate on the result: any non-empty
/// `new_errors` is grounds for a 422 response.
pub fn validate_save(
    json_body: &str,
    baseline: &BaselineErrors,
) -> Result<ValidationOutcome, ValidationFailure> {
    let json_project: json::Project =
        serde_json::from_str(json_body).map_err(ValidationFailure::JsonParse)?;
    let project: datamodel::Project = json_project.into();

    let post_keys = error_keys_for(&project);
    let post_errors = formatted_errors_for(&project);

    let new_errors: Vec<ValidationError> = post_errors
        .into_iter()
        .filter(|e| {
            let key = (e.code.clone(), e.variable_name.clone());
            !baseline.keys.contains(&key) && post_keys.contains(&key)
        })
        .collect();

    Ok(ValidationOutcome {
        project,
        new_errors,
    })
}

/// Run the engine diagnostic pipeline and return only the
/// (code, variable_name) keys for severity == Error diagnostics.
fn error_keys_for(project: &datamodel::Project) -> HashSet<(String, Option<String>)> {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    let diagnostics = collect_all_diagnostics(&db, &sync);
    let formatted = collect_formatted_errors(
        diagnostics
            .iter()
            .filter(|d| matches!(d.severity, DiagnosticSeverity::Error)),
        project,
    );
    formatted
        .errors
        .into_iter()
        .map(|e| (e.code.to_string(), e.variable_name))
        .collect()
}

/// Run the engine diagnostic pipeline and surface
/// `severity == Error` diagnostics formatted into wire-shape
/// `ValidationError` records (caller-facing structure).
fn formatted_errors_for(project: &datamodel::Project) -> Vec<ValidationError> {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    let diagnostics = collect_all_diagnostics(&db, &sync);
    let formatted = collect_formatted_errors(
        diagnostics
            .iter()
            .filter(|d| matches!(d.severity, DiagnosticSeverity::Error)),
        project,
    );
    formatted
        .errors
        .into_iter()
        .map(|fe| {
            use simlin_engine::errors::FormattedErrorKind;
            let kind = match fe.kind {
                FormattedErrorKind::Project => "project",
                FormattedErrorKind::Model => "model",
                FormattedErrorKind::Variable => "variable",
                FormattedErrorKind::Units => "units",
                FormattedErrorKind::Simulation => "simulation",
            };
            ValidationError {
                code: fe.code.to_string(),
                message: fe.message.unwrap_or_default(),
                model_name: fe.model_name,
                variable_name: fe.variable_name,
                kind: kind.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid project: one model with no variables. Should pass
    /// validation cleanly (no errors).
    const EMPTY_VALID: &str = r#"{
        "name": "x",
        "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
        "models": [{"name": "main"}]
    }"#;

    /// A project with a single auxiliary that references an undefined
    /// identifier. Should produce an `unknown_dependency` error.
    const HAS_UNDEFINED_REF: &str = r#"{
        "name": "x",
        "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
        "models": [{
            "name": "main",
            "auxiliaries": [
                {"name": "bad", "equation": "1 + bogus"}
            ]
        }]
    }"#;

    fn project_from_json(json_body: &str) -> datamodel::Project {
        let json_project: json::Project =
            serde_json::from_str(json_body).expect("test fixture parses");
        json_project.into()
    }

    #[test]
    fn json_parse_error_is_reported_explicitly() {
        let baseline = BaselineErrors::default();
        let err = validate_save("not actually json", &baseline).unwrap_err();
        assert!(matches!(err, ValidationFailure::JsonParse(_)));
    }

    #[test]
    fn valid_empty_project_has_no_new_errors() {
        let baseline = BaselineErrors::default();
        let outcome = validate_save(EMPTY_VALID, &baseline).expect("validates");
        assert!(
            outcome.new_errors.is_empty(),
            "empty model must have no errors, got {:?}",
            outcome.new_errors
        );
    }

    #[test]
    fn empty_baseline_surfaces_undefined_reference_as_new_error() {
        let baseline = BaselineErrors::default();
        let outcome = validate_save(HAS_UNDEFINED_REF, &baseline).expect("validates");
        assert!(
            !outcome.new_errors.is_empty(),
            "expected at least one new error for undefined reference"
        );
        let bad = outcome
            .new_errors
            .iter()
            .find(|e| e.variable_name.as_deref() == Some("bad"))
            .expect("error for variable 'bad' present");
        assert_eq!(bad.code, "unknown_dependency");
    }

    #[test]
    fn matching_baseline_suppresses_pre_existing_error() {
        // Pre-edit project already had this error. Post-edit project still
        // has it but introduces no new ones; the save should be accepted.
        let baseline_project = project_from_json(HAS_UNDEFINED_REF);
        let baseline = compute_baseline(&baseline_project);

        let outcome = validate_save(HAS_UNDEFINED_REF, &baseline).expect("validates");
        assert!(
            outcome.new_errors.is_empty(),
            "errors already in the baseline must not be re-reported as new; got {:?}",
            outcome.new_errors
        );
    }

    #[test]
    fn compute_baseline_on_clean_project_yields_empty_keys() {
        let project = project_from_json(EMPTY_VALID);
        let baseline = compute_baseline(&project);
        assert!(baseline.keys.is_empty());
    }

    #[test]
    fn compute_baseline_on_broken_project_captures_error_keys() {
        let project = project_from_json(HAS_UNDEFINED_REF);
        let baseline = compute_baseline(&project);
        assert!(
            !baseline.keys.is_empty(),
            "broken project must have at least one captured error key"
        );
        assert!(
            baseline
                .keys
                .iter()
                .any(|(code, var)| code == "unknown_dependency" && var.as_deref() == Some("bad")),
            "expected (unknown_dependency, Some(\"bad\")) in baseline, got {:?}",
            baseline.keys
        );
    }
}
