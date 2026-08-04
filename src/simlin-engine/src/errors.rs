// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Helpers for formatting engine errors for human-readable output.

use crate::builtins::Loc;
use crate::common::{EquationError, Error, ErrorCode, UnitError};
use crate::datamodel::{Equation, Project as DatamodelProject, Variable};
use crate::db;
use crate::db::DiagnosticSeverity;

/// Categorisation of the formatted error used for presentation purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormattedErrorKind {
    Project,
    Model,
    Variable,
    Units,
    Simulation,
}

/// Unit error kind for distinguishing types of unit-related errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitErrorKind {
    /// Syntax error in unit string definition
    Definition,
    /// Dimensional analysis mismatch
    Consistency,
    /// Inference error spanning multiple variables
    Inference,
}

/// The severity word the terminal-formatted summary line opens with, and the
/// word every other consumer should present the diagnostic under.
///
/// `message` embeds this word mid-string (it sits after the "units"/"assembly"
/// category noun), so it cannot be prepended at print time; deriving it here
/// from the diagnostic's own severity is what keeps an advisory from reading
/// as a hard failure (GH #919).
fn severity_word(severity: DiagnosticSeverity) -> &'static str {
    match severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
    }
}

/// A formatted error containing a human readable message and associated metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormattedError {
    pub code: ErrorCode,
    pub message: Option<String>,
    pub model_name: Option<String>,
    pub variable_name: Option<String>,
    pub start_offset: u16,
    pub end_offset: u16,
    pub kind: FormattedErrorKind,
    /// The severity of the diagnostic this was formatted from. `message` is
    /// already worded to match, so a consumer that renders `message` verbatim
    /// needs this only to route (filter, choose a stream, colour); a consumer
    /// that builds its own text should word it from here.
    pub severity: DiagnosticSeverity,
    /// For unit errors, indicates the specific type of unit error.
    /// None for non-unit errors.
    pub unit_error_kind: Option<UnitErrorKind>,
    /// The bare human-readable reason, without the source snippet or the
    /// model/variable summary line that `message` carries (e.g. "computed
    /// units 'people' don't match specified units"). `message` is formatted
    /// for terminal output; GUI consumers that already show the variable in
    /// context render this instead. Populated for unit errors (inference
    /// errors always synthesize one via `unit_inference_reason`) and for
    /// model-level errors whose `Error.details` is set; None elsewhere.
    pub details: Option<String>,
}

/// Collection of formatted errors plus bookkeeping flags that mirror previous CLI output
/// decisions.
///
/// The two flags count **`Error`-severity diagnostics only**: they gate
/// the CLI's suppression of a redundant simulation error (their sole
/// consumer), and an advisory `Warning` must never flip them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FormattedErrors {
    pub errors: Vec<FormattedError>,
    pub has_model_errors: bool,
    pub has_variable_errors: bool,
}

impl FormattedErrors {
    /// Append `error`, raising `has_model_errors` / `has_variable_errors` only
    /// when it is `Error`-severity.
    ///
    /// The sole place the flags are set. Callers that build a `FormattedErrors`
    /// from something other than `collect_formatted_errors` (the CLI's
    /// snippet-free diagnostic pass) go through here rather than re-deriving
    /// the rule, so the two cannot disagree about whether a warning counts.
    pub fn push(&mut self, error: FormattedError) {
        if error.severity == DiagnosticSeverity::Error {
            match error.kind {
                FormattedErrorKind::Variable => self.has_variable_errors = true,
                FormattedErrorKind::Model => self.has_model_errors = true,
                _ => {}
            }
        }
        self.errors.push(error);
    }
}

/// Format a simulation error reported while creating a VM.
///
/// A failure to build the VM is always fatal, so this is unconditionally
/// `Error`-severity (there is no `Diagnostic` to read a severity from).
pub fn format_simulation_error(model_name: &str, error: &Error) -> FormattedError {
    let message = format!("error compiling model '{model_name}': {error}");
    FormattedError {
        code: error.code,
        message: Some(message),
        model_name: Some(model_name.to_string()),
        variable_name: None,
        start_offset: 0,
        end_offset: 0,
        kind: FormattedErrorKind::Simulation,
        severity: DiagnosticSeverity::Error,
        unit_error_kind: None,
        details: None,
    }
}

fn format_equation_error(
    model_name: &str,
    var_name: &str,
    var: Option<&Variable>,
    error: &EquationError,
    severity: DiagnosticSeverity,
) -> FormattedError {
    let snippet = var
        .and_then(variable_equation_text)
        .map(|eqn| format_snippet(&eqn, error.start, error.end));
    let summary = format!(
        "{} in model '{model_name}' variable '{var_name}': {}",
        severity_word(severity),
        error.code
    );
    let message = combine_snippet_and_summary(snippet, summary);
    FormattedError {
        code: error.code,
        message,
        model_name: Some(model_name.to_string()),
        variable_name: Some(var_name.to_string()),
        start_offset: error.start,
        end_offset: error.end,
        kind: FormattedErrorKind::Variable,
        severity,
        unit_error_kind: None,
        details: None,
    }
}

/// Join variable names for a user-facing sentence: `'a'`, `'a' and 'b'`,
/// `'a', 'b', and 'c'`. Long lists are truncated to the first three names
/// plus a count, so a macro-instantiated conflict with dozens of sources
/// stays readable.
fn join_quoted_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [a] => format!("'{a}'"),
        [a, b] => format!("'{a}' and '{b}'"),
        [a, b, c] => format!("'{a}', '{b}', and '{c}'"),
        [a, b, c, rest @ ..] => {
            format!("'{a}', '{b}', '{c}', and {} more", rest.len())
        }
    }
}

/// The bare, user-facing reason for a unit inference error.
///
/// Inference conflicts are contradictions in the model-wide unit-constraint
/// system, and the raw reason on `UnitError::InferenceError` shows the
/// conflicting constraints in `1 == unit-expression` form with `@N`
/// metavariables -- useful in terminal output, meaningless to a modeler in
/// the GUI. A single-line reason is already plain language (e.g. the
/// clarified macro conflict from `units_infer::clarify_macro_conflict`) and
/// passes through; a multi-line constraint dump (or a missing reason) is
/// replaced with a sentence naming the involved variables so the modeler
/// knows where to look. The full constraint text still rides in the
/// terminal-formatted `message`.
pub(crate) fn unit_inference_reason(
    sources: &[(String, Option<Loc>)],
    details: Option<&str>,
) -> String {
    if let Some(details) = details
        && !details.contains('\n')
    {
        return details.to_string();
    }
    // Sources are deduped by (var, loc), so the same variable can appear at
    // several locations; dedupe by name (order-preserving) so the sentence
    // never reads "'x' and 'x'" and the 1-vs-many phrasing is right.
    let mut names: Vec<&str> = Vec::new();
    for (var, _) in sources {
        if !names.contains(&var.as_str()) {
            names.push(var.as_str());
        }
    }
    match names.len() {
        0 => "the units implied by the model's equations are inconsistent".to_string(),
        1 => format!(
            "the units in the equation for {} are inconsistent; check its equation and declared units",
            join_quoted_names(&names)
        ),
        _ => format!(
            "the units of {} are inconsistent with each other; check the equations and declared units of these variables",
            join_quoted_names(&names)
        ),
    }
}

/// Format a unit diagnostic. `severity` decides the summary line's severity
/// word: a unit *definition* error is a syntax error in a `<units>` string and
/// is accumulated as `Error`, while a *consistency* or *inference* mismatch does
/// not block simulation and is accumulated as `Warning` (`db/units.rs`). Both
/// used to render as "units error", which read as though the model would not
/// run.
fn format_unit_error(
    model_name: &str,
    var_name: &str,
    var: Option<&Variable>,
    error: &UnitError,
    severity: DiagnosticSeverity,
) -> FormattedError {
    let word = severity_word(severity);
    match error {
        UnitError::DefinitionError(eq_error, details) => {
            let snippet = var
                .and_then(|v| v.get_units())
                .map(|units| format_snippet(units, eq_error.start, eq_error.end));
            let summary = match details {
                Some(details) => format!(
                    "units {word} in model '{model_name}' variable '{var_name}': {} -- {}",
                    eq_error.code, details
                ),
                None => format!(
                    "units {word} in model '{model_name}' variable '{var_name}': {}",
                    eq_error.code
                ),
            };
            FormattedError {
                code: eq_error.code,
                message: combine_snippet_and_summary(snippet, summary),
                model_name: Some(model_name.to_string()),
                variable_name: Some(var_name.to_string()),
                start_offset: eq_error.start,
                end_offset: eq_error.end,
                kind: FormattedErrorKind::Units,
                severity,
                unit_error_kind: Some(UnitErrorKind::Definition),
                details: details.clone(),
            }
        }
        UnitError::ConsistencyError(code, loc, details) => {
            let snippet = var
                .and_then(variable_equation_text)
                .map(|eqn| format_snippet(&eqn, loc.start, loc.end));
            let summary = match details {
                Some(details) => format!(
                    "units {word} in model '{model_name}' variable '{var_name}': {code} -- {details}"
                ),
                None => {
                    format!("units {word} in model '{model_name}' variable '{var_name}': {code}")
                }
            };
            FormattedError {
                code: *code,
                message: combine_snippet_and_summary(snippet, summary),
                model_name: Some(model_name.to_string()),
                variable_name: Some(var_name.to_string()),
                start_offset: loc.start,
                end_offset: loc.end,
                kind: FormattedErrorKind::Units,
                severity,
                unit_error_kind: Some(UnitErrorKind::Consistency),
                details: details.clone(),
            }
        }
        UnitError::InferenceError {
            code,
            sources,
            details,
        } => {
            let (start, end) = sources
                .first()
                .and_then(|(_, loc)| *loc)
                .map(|loc| (loc.start, loc.end))
                .unwrap_or((0, 0));
            let snippet = var
                .and_then(variable_equation_text)
                .map(|eqn| format_snippet(&eqn, start, end));
            let involved_vars: Vec<_> = sources.iter().map(|(v, _)| v.as_str()).collect();
            let summary = match (details, involved_vars.len()) {
                (Some(details), n) if n > 1 => format!(
                    "units inference {word} in model '{model_name}' involving {}: {code} -- {details}",
                    involved_vars.join(", ")
                ),
                (Some(details), _) => format!(
                    "units inference {word} in model '{model_name}' variable '{var_name}': {code} -- {details}"
                ),
                (None, n) if n > 1 => format!(
                    "units inference {word} in model '{model_name}' involving {}: {code}",
                    involved_vars.join(", ")
                ),
                (None, _) => format!(
                    "units inference {word} in model '{model_name}' variable '{var_name}': {code}"
                ),
            };
            FormattedError {
                code: *code,
                message: combine_snippet_and_summary(snippet, summary),
                model_name: Some(model_name.to_string()),
                variable_name: Some(var_name.to_string()),
                start_offset: start,
                end_offset: end,
                kind: FormattedErrorKind::Units,
                severity,
                unit_error_kind: Some(UnitErrorKind::Inference),
                details: Some(unit_inference_reason(sources, details.as_deref())),
            }
        }
    }
}

/// Convert a salsa accumulator diagnostic into a `FormattedError`.
///
/// This produces the same structure as the per-field formatters
/// (`format_equation_error`, `format_unit_error`) but reads from a
/// `Diagnostic` instead of walking model/variable fields. No datamodel
/// variable is available, so snippets are omitted.
///
/// The diagnostic's `severity` rides through to the summary line's severity
/// word and onto `FormattedError::severity`: a `Warning` (an LTM-degraded
/// advisory, a conveyor spec advisory, a unit mismatch) reads as a warning
/// rather than as a compilation failure.
pub fn format_diagnostic(diag: &db::Diagnostic) -> FormattedError {
    use db::DiagnosticError;
    let severity = diag.severity;
    match &diag.error {
        DiagnosticError::Equation(err) => {
            let var_name = diag.variable.as_deref().unwrap_or("<unknown>");
            let summary = format!(
                "{} in model '{}' variable '{}': {}",
                severity_word(severity),
                diag.model,
                var_name,
                err.code
            );
            FormattedError {
                code: err.code,
                message: Some(summary),
                model_name: Some(diag.model.clone()),
                variable_name: diag.variable.clone(),
                start_offset: err.start,
                end_offset: err.end,
                kind: FormattedErrorKind::Variable,
                severity,
                unit_error_kind: None,
                details: None,
            }
        }
        DiagnosticError::Model(err) => {
            let (kind, unit_error_kind) = if err.code == ErrorCode::UnitMismatch {
                (FormattedErrorKind::Units, Some(UnitErrorKind::Inference))
            } else {
                (FormattedErrorKind::Model, None)
            };
            FormattedError {
                code: err.code,
                message: Some(format!(
                    "{} in model '{}': {}",
                    severity_word(severity),
                    diag.model,
                    err
                )),
                model_name: Some(diag.model.clone()),
                variable_name: diag.variable.clone(),
                start_offset: 0,
                end_offset: 0,
                kind,
                severity,
                unit_error_kind,
                // Model-level `Error.details` is a bare reason by
                // construction (e.g. the unit-inference umbrella built in
                // db/units.rs), so it rides in `details` for GUI consumers
                // just like per-variable unit errors.
                details: err.details.clone(),
            }
        }
        DiagnosticError::Unit(err) => {
            let var_name = diag.variable.as_deref().unwrap_or("<unknown>");
            format_unit_error(&diag.model, var_name, None, err, severity)
        }
        DiagnosticError::Assembly(msg) => FormattedError {
            code: ErrorCode::NotSimulatable,
            message: Some(format!(
                "assembly {} in model '{}': {}",
                severity_word(severity),
                diag.model,
                msg
            )),
            model_name: Some(diag.model.clone()),
            variable_name: diag.variable.clone(),
            start_offset: 0,
            end_offset: 0,
            kind: FormattedErrorKind::Simulation,
            severity,
            unit_error_kind: None,
            details: None,
        },
    }
}

/// Format a diagnostic with snippet context from the datamodel.
///
/// Like `format_diagnostic`, but looks up the variable's equation text
/// from `datamodel` to produce source-annotated snippet output for
/// equation and unit errors.
pub fn format_diagnostic_with_datamodel(
    diag: &db::Diagnostic,
    datamodel: &DatamodelProject,
) -> FormattedError {
    use db::DiagnosticError;
    let dm_var = datamodel
        .get_model(&diag.model)
        .and_then(|m| diag.variable.as_deref().and_then(|v| m.get_variable(v)));
    match &diag.error {
        DiagnosticError::Equation(err) => {
            let var_name = diag.variable.as_deref().unwrap_or("<unknown>");
            format_equation_error(&diag.model, var_name, dm_var, err, diag.severity)
        }
        DiagnosticError::Unit(err) => {
            let var_name = diag.variable.as_deref().unwrap_or("<unknown>");
            format_unit_error(&diag.model, var_name, dm_var, err, diag.severity)
        }
        _ => format_diagnostic(diag),
    }
}

/// Collect and format all diagnostics from the incremental (salsa) path,
/// enriching equation/unit errors with snippet context from the datamodel.
///
/// Accepts any iterator yielding references to `Diagnostic`, so callers
/// with `Vec<&Diagnostic>` (from filtering) can pass directly without
/// cloning into a new `Vec<Diagnostic>`.
///
/// Every diagnostic handed in is formatted and returned, but only
/// `Error`-severity ones raise `has_model_errors` / `has_variable_errors`:
/// those flags drive failure decisions, and a `Warning` is advisory.
pub fn collect_formatted_errors<'a>(
    diagnostics: impl IntoIterator<Item = &'a db::Diagnostic>,
    datamodel: &DatamodelProject,
) -> FormattedErrors {
    let mut formatted = FormattedErrors::default();
    for diag in diagnostics {
        formatted.push(format_diagnostic_with_datamodel(diag, datamodel));
    }
    formatted
}

fn variable_equation_text(var: &Variable) -> Option<String> {
    match var.get_equation() {
        Some(Equation::Scalar(eqn)) => Some(eqn.clone()),
        Some(Equation::ApplyToAll(_, eqn)) => Some(eqn.clone()),
        _ => None,
    }
}

fn format_snippet(text: &str, start: u16, end: u16) -> String {
    let len = text.len() as u16;
    let start = start.min(len) as usize;
    let end = end.min(len) as usize;
    let highlight_len = end.saturating_sub(start);
    let mut snippet = String::new();
    snippet.push_str("    ");
    snippet.push_str(text);
    snippet.push('\n');
    snippet.push_str("    ");
    snippet.push_str(&" ".repeat(start));
    snippet.push_str(&"~".repeat(highlight_len));
    snippet
}

fn combine_snippet_and_summary(snippet: Option<String>, summary: String) -> Option<String> {
    match snippet {
        Some(snippet) if !snippet.is_empty() => Some(format!("{snippet}\n{summary}")),
        _ => Some(summary),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::ErrorCode;
    use crate::db::{SimlinDb, collect_all_diagnostics, sync_from_datamodel};
    use crate::test_common::TestProject;

    #[test]
    fn equation_error_formats_snippet() {
        let datamodel = TestProject::new("equation-error")
            .aux("bad", "1 + bogus", None)
            .build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let diagnostics = collect_all_diagnostics(&db, sync.project);
        let formatted = collect_formatted_errors(&diagnostics, &datamodel);

        assert!(formatted.has_variable_errors);
        let error = formatted
            .errors
            .iter()
            .find(|err| err.variable_name.as_deref() == Some("bad"))
            .expect("equation error missing");

        assert_eq!(error.code, ErrorCode::UnknownDependency);
        assert_eq!(error.kind, FormattedErrorKind::Variable);
        assert_eq!(error.severity, DiagnosticSeverity::Error);
        let message = error.message.as_ref().expect("message missing");
        let mut lines = message.lines();
        assert_eq!(lines.next().unwrap(), "    1 + bogus");
        assert_eq!(lines.next().unwrap(), "        ~~~~~");
        assert_eq!(
            lines.next().unwrap(),
            "error in model 'main' variable 'bad': unknown_dependency"
        );
        assert!(lines.next().is_none());
    }

    #[test]
    fn unit_error_formats_snippet() {
        let datamodel = TestProject::new("unit-error")
            .unit("Person", None)
            .unit("Month", None)
            .aux("source", "1", Some("Month"))
            .aux("bad_units", "source", Some("Person"))
            .build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let diagnostics = collect_all_diagnostics(&db, sync.project);
        let formatted = collect_formatted_errors(&diagnostics, &datamodel);

        let error = formatted
            .errors
            .iter()
            .find(|err| err.variable_name.as_deref() == Some("bad_units"))
            .expect("unit error missing");
        assert_eq!(error.code, ErrorCode::UnitMismatch);
        assert_eq!(error.kind, FormattedErrorKind::Units);
        // A unit *consistency* mismatch does not block simulation: `db/units.rs`
        // accumulates it as a Warning, so it must not claim to be an error.
        assert_eq!(error.severity, DiagnosticSeverity::Warning);
        let message = error.message.as_ref().expect("message missing");
        let mut lines = message.lines();
        assert_eq!(lines.next().unwrap(), "    source");
        assert_eq!(lines.next().unwrap(), "    ~~~~~~");
        assert!(
            lines
                .next()
                .unwrap()
                .contains("units warning in model 'main' variable 'bad_units': unit_mismatch")
        );
        assert!(lines.next().is_none());
        assert!(
            !message.contains("units error in model"),
            "a Warning-severity unit mismatch must not render as an error: {message}"
        );

        // The bare reason rides separately from the terminal-formatted
        // message, so GUI consumers can show it without the snippet/summary.
        let details = error.details.as_ref().expect("details missing");
        assert!(
            details.starts_with("the equation computes to units"),
            "bare details should be the reason string alone: {details}"
        );
        assert!(!details.contains('~'), "details must not carry the snippet");
        // Match on the severity-independent phrase so the guard cannot pass
        // vacuously if the summary line's severity word changes.
        assert!(
            !details.contains("in model"),
            "details must not carry the summary line"
        );
    }

    /// A syntax error in a `<units>` string genuinely blocks nothing at
    /// simulation time but IS accumulated as `Error` severity
    /// (`db/var_fragment.rs`), so it keeps the "units error" wording. The two
    /// unit arms therefore prove the word tracks severity, not the arm.
    #[test]
    fn unit_definition_error_keeps_error_word() {
        let datamodel = TestProject::new("unit-def-error")
            .unit("BadUnit", Some("1///invalid"))
            .aux("x", "1", Some("BadUnit"))
            .build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let diagnostics = collect_all_diagnostics(&db, sync.project);
        let formatted = collect_formatted_errors(&diagnostics, &datamodel);

        let error = formatted
            .errors
            .iter()
            .find(|err| err.unit_error_kind == Some(UnitErrorKind::Definition))
            .expect("unit definition error missing");
        assert_eq!(error.severity, DiagnosticSeverity::Error);
        let message = error.message.as_ref().expect("message missing");
        assert!(
            message.contains("units error in model"),
            "an Error-severity unit definition error keeps the error word: {message}"
        );
    }

    #[test]
    fn inference_error_formats_correctly() {
        use crate::builtins::Loc;
        use crate::common::UnitError;

        let error = UnitError::InferenceError {
            code: ErrorCode::UnitMismatch,
            sources: vec![("my_var".to_string(), Some(Loc::new(5, 10)))],
            details: Some("test details".to_string()),
        };

        // Inference conflicts are accumulated as Warnings in production; the
        // Error rendering below proves the severity word is data-driven rather
        // than baked into the arm.
        let formatted = format_unit_error(
            "test_model",
            "my_var",
            None,
            &error,
            DiagnosticSeverity::Warning,
        );
        assert_eq!(formatted.code, ErrorCode::UnitMismatch);
        assert_eq!(formatted.kind, FormattedErrorKind::Units);
        assert_eq!(formatted.severity, DiagnosticSeverity::Warning);
        assert_eq!(formatted.start_offset, 5);
        assert_eq!(formatted.end_offset, 10);
        assert_eq!(formatted.model_name, Some("test_model".to_string()));
        assert_eq!(formatted.variable_name, Some("my_var".to_string()));
        let msg = formatted.message.expect("should have message");
        assert!(
            msg.contains("units inference warning"),
            "should mention inference at the diagnostic's severity: {msg}"
        );
        assert!(
            msg.contains("test details"),
            "should include details: {msg}"
        );
        assert_eq!(formatted.details.as_deref(), Some("test details"));

        let formatted = format_unit_error(
            "test_model",
            "my_var",
            None,
            &error,
            DiagnosticSeverity::Error,
        );
        let msg = formatted.message.expect("should have message");
        assert!(
            msg.contains("units inference error"),
            "the same arm at Error severity reads as an error: {msg}"
        );

        let error = UnitError::InferenceError {
            code: ErrorCode::UnitMismatch,
            sources: vec![
                ("var_a".to_string(), Some(Loc::new(0, 5))),
                ("var_b".to_string(), None),
            ],
            details: None,
        };

        let formatted = format_unit_error(
            "test_model",
            "var_a",
            None,
            &error,
            DiagnosticSeverity::Warning,
        );
        let msg = formatted.message.expect("should have message");
        assert!(
            msg.contains("involving var_a, var_b"),
            "should list involved variables: {msg}"
        );

        let error = UnitError::InferenceError {
            code: ErrorCode::UnitMismatch,
            sources: vec![("no_loc_var".to_string(), None)],
            details: None,
        };

        let formatted = format_unit_error(
            "test_model",
            "no_loc_var",
            None,
            &error,
            DiagnosticSeverity::Warning,
        );
        assert_eq!(formatted.start_offset, 0);
        assert_eq!(formatted.end_offset, 0);
    }

    #[test]
    fn inference_error_details_are_user_facing() {
        use crate::builtins::Loc;
        use crate::common::UnitError;

        // A multi-line constraint dump (what units_infer produces) is
        // replaced in `details` by a plain-language sentence naming the
        // involved variables; the terminal `message` keeps the full dump.
        let dump = "unit checking failed; inconsistent constraints:\n    1 == people*@3\u{207b}\u{b9}\n    1 == @3";
        let error = UnitError::InferenceError {
            code: ErrorCode::UnitMismatch,
            sources: vec![
                ("birth_rate".to_string(), Some(Loc::new(0, 5))),
                ("population".to_string(), None),
            ],
            details: Some(dump.to_string()),
        };
        let formatted = format_unit_error(
            "main",
            "birth_rate",
            None,
            &error,
            DiagnosticSeverity::Warning,
        );
        let details = formatted.details.expect("details missing");
        assert!(
            details.contains("'birth_rate'") && details.contains("'population'"),
            "details should name the involved variables: {details}"
        );
        assert!(
            !details.contains('\n') && !details.contains("1 =="),
            "details must not carry the constraint dump: {details}"
        );
        let message = formatted.message.expect("message missing");
        assert!(
            message.contains("1 =="),
            "terminal message keeps the constraint dump: {message}"
        );

        // A single involved variable reads as a within-equation problem.
        let error = UnitError::InferenceError {
            code: ErrorCode::UnitMismatch,
            sources: vec![("flow".to_string(), None)],
            details: Some(dump.to_string()),
        };
        let formatted =
            format_unit_error("main", "flow", None, &error, DiagnosticSeverity::Warning);
        let details = formatted.details.expect("details missing");
        assert!(
            details.contains("'flow'") && !details.contains('\n'),
            "single-source details should name the variable: {details}"
        );

        // No sources and no details still yields a plain-language reason.
        let error = UnitError::InferenceError {
            code: ErrorCode::UnitMismatch,
            sources: vec![],
            details: None,
        };
        let formatted = format_unit_error("main", "x", None, &error, DiagnosticSeverity::Warning);
        let details = formatted.details.expect("details missing");
        assert!(!details.is_empty() && !details.contains('\n'));

        // Sources are deduped by (var, loc), so the same variable can appear
        // at two locations; the sentence must not read "'x' and 'x'".
        let error = UnitError::InferenceError {
            code: ErrorCode::UnitMismatch,
            sources: vec![
                ("x".to_string(), Some(Loc::new(0, 3))),
                ("x".to_string(), Some(Loc::new(5, 8))),
            ],
            details: Some(dump.to_string()),
        };
        let formatted = format_unit_error("main", "x", None, &error, DiagnosticSeverity::Warning);
        let details = formatted.details.expect("details missing");
        assert_eq!(
            details.matches("'x'").count(),
            1,
            "duplicate names must collapse: {details}"
        );
        assert!(
            details.contains("the units in the equation for 'x'"),
            "single distinct variable should use the one-variable phrasing: {details}"
        );
    }

    #[test]
    fn join_quoted_names_truncates_long_lists() {
        assert_eq!(join_quoted_names(&[]), "");
        assert_eq!(join_quoted_names(&["a"]), "'a'");
        assert_eq!(join_quoted_names(&["a", "b"]), "'a' and 'b'");
        assert_eq!(join_quoted_names(&["a", "b", "c"]), "'a', 'b', and 'c'");
        assert_eq!(
            join_quoted_names(&["a", "b", "c", "d", "e"]),
            "'a', 'b', 'c', and 2 more"
        );
    }

    #[test]
    fn collect_formatted_errors_accepts_filtered_iterator() {
        let datamodel = TestProject::new("filter-iter")
            .aux("ok_var", "42", None)
            .aux("bad_var", "1 + bogus", None)
            .build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let diagnostics = collect_all_diagnostics(&db, sync.project);

        // Pass a filtered iterator directly (the use case for issue #426).
        let formatted = collect_formatted_errors(
            diagnostics
                .iter()
                .filter(|d| matches!(d.severity, db::DiagnosticSeverity::Error)),
            &datamodel,
        );

        assert!(!formatted.errors.is_empty());
        assert!(
            formatted
                .errors
                .iter()
                .any(|e| e.variable_name.as_deref() == Some("bad_var"))
        );
    }

    /// Build a `Diagnostic` for the four `DiagnosticError` arms at a chosen
    /// severity, so each arm's severity word can be checked in both states.
    fn diagnostic_arms(severity: DiagnosticSeverity) -> Vec<db::Diagnostic> {
        use crate::common::{Error as CommonError, ErrorKind, UnitError};
        use crate::db::{Diagnostic, DiagnosticError};

        let arm = |error| Diagnostic {
            model: "main".to_string(),
            variable: Some("v".to_string()),
            error,
            severity,
        };
        vec![
            arm(DiagnosticError::Equation(EquationError {
                start: 0,
                end: 1,
                code: ErrorCode::UnknownDependency,
            })),
            arm(DiagnosticError::Model(CommonError {
                kind: ErrorKind::Model,
                code: ErrorCode::ConveyorLtmDegraded,
                details: Some("belt scores are advisory".to_string()),
            })),
            arm(DiagnosticError::Unit(UnitError::ConsistencyError(
                ErrorCode::UnitMismatch,
                Loc::new(0, 1),
                None,
            ))),
            arm(DiagnosticError::Assembly("could not assemble".to_string())),
        ]
    }

    /// GH #919: every `format_diagnostic` arm words its summary line from the
    /// diagnostic's severity and carries that severity out on the struct. A
    /// `Warning` (the LTM-degraded conveyor advisory, a unit mismatch) must
    /// never render the word "error", which reads as a compilation failure.
    #[test]
    fn diagnostic_severity_drives_the_summary_word() {
        for diag in diagnostic_arms(DiagnosticSeverity::Warning) {
            let fe = format_diagnostic(&diag);
            let message = fe.message.as_ref().expect("message missing");
            assert_eq!(fe.severity, DiagnosticSeverity::Warning);
            assert!(
                message.contains("warning in model 'main'"),
                "a Warning must render as a warning: {message}"
            );
            assert!(
                !message.contains("error in model"),
                "a Warning must not render as an error: {message}"
            );
        }

        for diag in diagnostic_arms(DiagnosticSeverity::Error) {
            let fe = format_diagnostic(&diag);
            let message = fe.message.as_ref().expect("message missing");
            assert_eq!(fe.severity, DiagnosticSeverity::Error);
            assert!(
                message.contains("error in model 'main'"),
                "an Error must render as an error: {message}"
            );
            assert!(
                !message.contains("warning in model"),
                "an Error must not render as a warning: {message}"
            );
        }
    }

    /// The snippet-bearing twin of the check above: `format_diagnostic_with_datamodel`
    /// takes a different route through `format_equation_error`/`format_unit_error`,
    /// so it needs its own severity plumbing.
    #[test]
    fn diagnostic_with_datamodel_severity_drives_the_summary_word() {
        let datamodel = TestProject::new("severity-snippet")
            .aux("v", "1 + bogus", None)
            .build_datamodel();
        for diag in diagnostic_arms(DiagnosticSeverity::Warning) {
            let fe = format_diagnostic_with_datamodel(&diag, &datamodel);
            let message = fe.message.as_ref().expect("message missing");
            assert_eq!(fe.severity, DiagnosticSeverity::Warning);
            assert!(
                !message.contains("error in model"),
                "a Warning must not render as an error: {message}"
            );
        }
    }

    /// A simulation-build failure has no `Diagnostic` behind it, so it is
    /// unconditionally an error.
    #[test]
    fn simulation_error_is_always_error_severity() {
        use crate::common::{Error as CommonError, ErrorKind};
        let fe = format_simulation_error(
            "main",
            &CommonError::new(ErrorKind::Simulation, ErrorCode::NotSimulatable, None),
        );
        assert_eq!(fe.severity, DiagnosticSeverity::Error);
        assert!(
            fe.message
                .as_deref()
                .expect("message missing")
                .starts_with("error compiling model 'main'")
        );
    }

    /// GH #919: `has_model_errors` / `has_variable_errors` gate failure-shaped
    /// decisions (the CLI's simulation-error suppression, libsimlin's patch
    /// rejection). A Warning-only diagnostic set must leave both false, even
    /// though the warnings themselves are still formatted and returned.
    #[test]
    fn warning_severity_does_not_set_the_error_flags() {
        let datamodel = TestProject::new("warn-flags")
            .aux("v", "1", None)
            .build_datamodel();

        let warnings = diagnostic_arms(DiagnosticSeverity::Warning);
        let formatted = collect_formatted_errors(&warnings, &datamodel);
        assert_eq!(formatted.errors.len(), warnings.len());
        assert!(
            !formatted.has_model_errors && !formatted.has_variable_errors,
            "advisory warnings must not raise the error flags"
        );

        let errors = diagnostic_arms(DiagnosticSeverity::Error);
        let formatted = collect_formatted_errors(&errors, &datamodel);
        assert!(
            formatted.has_model_errors && formatted.has_variable_errors,
            "Error-severity diagnostics still raise the flags"
        );
    }
}
