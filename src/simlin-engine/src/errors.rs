// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Helpers for formatting engine errors for human-readable output.

use crate::builtins::Loc;
use crate::common::{Error, ErrorCode};
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
    /// model/variable summary line that `message` carries (e.g. "the equation
    /// computes to units 'people', but the variable's specified units are
    /// 'person'"). `message` is formatted for terminal output; GUI consumers
    /// that already show the variable in context render this instead.
    ///
    /// Populated whenever the diagnostic carries one: from `Error::details`,
    /// from `EquationError::details`, and from the `UnitError` variants
    /// (inference errors always synthesize one via `unit_inference_reason`).
    /// `None` when the raising site had nothing to add beyond the code and the
    /// span -- a parse error, whose reason IS the snippet.
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
        details: error.details.clone(),
    }
}

/// The trailing `": {code}"` / `": {code} -- {reason}"` of a summary line.
///
/// One helper, so the equation and unit arms cannot drift on how a reason is
/// joined to its code -- the joiner is the ` -- ` every summary in this module
/// uses.
fn code_and_reason(code: ErrorCode, details: Option<&str>) -> String {
    match details {
        Some(details) => format!("{code} -- {details}"),
        None => code.to_string(),
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

/// Convert a salsa accumulator diagnostic into a `FormattedError`.
///
/// No datamodel variable is available, so snippets are omitted.
///
/// The diagnostic's `severity` rides through to the summary line's severity
/// word and onto `FormattedError::severity`: a `Warning` (an LTM-degraded
/// advisory, a conveyor spec advisory, a unit mismatch) reads as a warning
/// rather than as a compilation failure.
pub fn format_diagnostic(diag: &db::Diagnostic) -> FormattedError {
    format_diagnostic_inner(diag, None)
}

fn format_diagnostic_inner(diag: &db::Diagnostic, var: Option<&Variable>) -> FormattedError {
    use db::DiagnosticCategory;

    let severity = diag.severity;
    let word = severity_word(severity);
    // A generated helper's physical name remains on `Diagnostic::variable`
    // for compiler identity and deduplication. Presentation and source lookup
    // use the user-authored owner, so an internal `$...` name never displaces
    // the equation the modeler can edit.
    let source_var_name = diag.owner.as_deref().or(diag.variable.as_deref());
    let var_name = source_var_name.unwrap_or("<unknown>");
    let reason = diag.reason();
    let location = diag.location.unwrap_or_default();
    let (kind, unit_error_kind, summary, snippet) = match diag.category {
        DiagnosticCategory::Equation | DiagnosticCategory::Variable => (
            FormattedErrorKind::Variable,
            None,
            format!(
                "{word} in model '{}' variable '{var_name}': {}",
                diag.model,
                code_and_reason(diag.code, reason)
            ),
            (diag.category == DiagnosticCategory::Equation)
                .then(|| {
                    var.and_then(variable_equation_text)
                        .map(|eqn| format_snippet(&eqn, location.start, location.end))
                })
                .flatten(),
        ),
        DiagnosticCategory::UnitDefinition => (
            FormattedErrorKind::Units,
            Some(UnitErrorKind::Definition),
            format!(
                "units {word} in model '{}' variable '{var_name}': {}",
                diag.model,
                code_and_reason(diag.code, reason)
            ),
            var.and_then(|variable| variable.get_units())
                .map(|units| format_snippet(units, location.start, location.end)),
        ),
        DiagnosticCategory::UnitConsistency => (
            FormattedErrorKind::Units,
            Some(UnitErrorKind::Consistency),
            format!(
                "units {word} in model '{}' variable '{var_name}': {}",
                diag.model,
                code_and_reason(diag.code, reason)
            ),
            var.and_then(variable_equation_text)
                .map(|eqn| format_snippet(&eqn, location.start, location.end)),
        ),
        DiagnosticCategory::UnitInference => {
            let names: Vec<_> = diag
                .related
                .iter()
                .map(|source| source.variable.as_str())
                .collect();
            let subject = if names.len() > 1 {
                format!("involving {}", names.join(", "))
            } else {
                format!("variable '{var_name}'")
            };
            let first_loc = diag
                .related
                .first()
                .and_then(|source| source.location)
                .unwrap_or(location);
            (
                FormattedErrorKind::Units,
                Some(UnitErrorKind::Inference),
                format!(
                    "units inference {word} in model '{}' {subject}: {}",
                    diag.model,
                    code_and_reason(diag.code, diag.details.as_deref())
                ),
                var.and_then(variable_equation_text)
                    .map(|eqn| format_snippet(&eqn, first_loc.start, first_loc.end)),
            )
        }
        DiagnosticCategory::Import | DiagnosticCategory::Model => {
            let error_kind = if diag.category == DiagnosticCategory::Import {
                "ImportError"
            } else {
                "ModelError"
            };
            let rendered = match reason {
                Some(reason) => format!("{error_kind}{{{}: {reason}}}", diag.code),
                None => format!("{error_kind}{{{}}}", diag.code),
            };
            (
                FormattedErrorKind::Model,
                None,
                format!("{word} in model '{}': {rendered}", diag.model),
                None,
            )
        }
        DiagnosticCategory::Assembly => {
            let assembly_reason = reason
                .map(str::to_owned)
                .unwrap_or_else(|| diag.code.to_string());
            (
                FormattedErrorKind::Simulation,
                None,
                format!(
                    "assembly {word} in model '{}': {assembly_reason}",
                    diag.model
                ),
                None,
            )
        }
    };

    let (start_offset, end_offset) = if diag.category == DiagnosticCategory::UnitInference {
        diag.related
            .first()
            .and_then(|source| source.location)
            .map(|loc| (loc.start, loc.end))
            .unwrap_or((location.start, location.end))
    } else {
        (location.start, location.end)
    };
    FormattedError {
        code: diag.code,
        message: combine_snippet_and_summary(snippet, summary),
        model_name: Some(diag.model.clone()),
        variable_name: source_var_name.map(str::to_owned),
        start_offset,
        end_offset,
        kind,
        severity,
        unit_error_kind,
        details: reason.map(ToOwned::to_owned),
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
    let dm_var = datamodel.get_model(&diag.model).and_then(|m| {
        diag.owner
            .as_deref()
            .or(diag.variable.as_deref())
            .and_then(|v| m.get_variable(v))
    });
    format_diagnostic_inner(diag, dm_var)
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
    use crate::common::{EquationError, ErrorCode, UnitError};
    use crate::db::{Diagnostic, SimlinDb, collect_all_diagnostics, sync_from_datamodel};
    use crate::test_common::TestProject;

    fn format_test_unit_error(
        model_name: &str,
        var_name: &str,
        _var: Option<&Variable>,
        error: &UnitError,
        severity: DiagnosticSeverity,
    ) -> FormattedError {
        let display_reason = match error {
            UnitError::InferenceError {
                sources, details, ..
            } => Some(unit_inference_reason(sources, details.as_deref())),
            _ => None,
        };
        let mut diagnostic = Diagnostic::unit(error.clone(), severity);
        if let Some(reason) = display_reason {
            diagnostic = diagnostic.with_display_details(reason);
        }
        format_diagnostic(&diagnostic.with_context(model_name, Some(var_name.to_string())))
    }

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
            "error in model 'main' variable 'bad': unknown_dependency -- \
             'bogus' is not a variable of model 'main'"
        );
        assert!(lines.next().is_none());
        // The reason is also available on its own, for a consumer that renders
        // the variable itself and wants only the sentence.
        assert_eq!(
            error.details.as_deref(),
            Some("'bogus' is not a variable of model 'main'")
        );
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
        let formatted = format_test_unit_error(
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

        let formatted = format_test_unit_error(
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

        let formatted = format_test_unit_error(
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

        let formatted = format_test_unit_error(
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
        let formatted = format_test_unit_error(
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
            format_test_unit_error("main", "flow", None, &error, DiagnosticSeverity::Warning);
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
        let formatted =
            format_test_unit_error("main", "x", None, &error, DiagnosticSeverity::Warning);
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
        let formatted =
            format_test_unit_error("main", "x", None, &error, DiagnosticSeverity::Warning);
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

    /// Build one diagnostic from every formatter category at a chosen
    /// severity, so each category's severity word can be checked in both states.
    fn diagnostic_arms(severity: DiagnosticSeverity) -> Vec<db::Diagnostic> {
        use crate::common::{Error as CommonError, ErrorKind, UnitError};
        use crate::db::Diagnostic;

        let context =
            |diagnostic: Diagnostic| diagnostic.with_context("main", Some("v".to_string()));
        vec![
            context(Diagnostic::equation(
                EquationError::new(ErrorCode::UnknownDependency, 0, 1),
                severity,
            )),
            context(Diagnostic::engine(
                CommonError {
                    kind: ErrorKind::Model,
                    code: ErrorCode::ConveyorLtmDegraded,
                    details: Some("belt scores are advisory".to_string()),
                },
                severity,
            )),
            context(Diagnostic::unit(
                UnitError::ConsistencyError(ErrorCode::UnitMismatch, Loc::new(0, 1), None),
                severity,
            )),
            context(Diagnostic::assembly("could not assemble", severity)),
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

    /// The snippet-bearing twin of the check above exercises the datamodel
    /// lookup path in `format_diagnostic_with_datamodel`.
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
            &CommonError::new(
                ErrorKind::Simulation,
                ErrorCode::NotSimulatable,
                Some("queue 'waiting' cannot feed conveyor 'belt'".to_string()),
            ),
        );
        assert_eq!(fe.severity, DiagnosticSeverity::Error);
        assert_eq!(
            fe.details.as_deref(),
            Some("queue 'waiting' cannot feed conveyor 'belt'"),
            "the special-stock build path's complete reason reaches public formatting"
        );
        assert!(
            fe.message
                .as_deref()
                .expect("message missing")
                .starts_with("error compiling model 'main'")
        );
    }

    /// Beyond the severity word, `format_diagnostic` makes three per-category
    /// decisions -- the presentation `kind`, the `unit_error_kind`
    /// refinement, and where the source offsets come from. The rows below
    /// are the complete `DiagnosticCategory::ALL` list. `diagnostic_arms`
    /// above asserts severity wording; this matrix holds the category mapping.
    #[test]
    fn format_diagnostic_maps_every_arm() {
        use crate::common::{Error as CommonError, ErrorKind, UnitError};
        use crate::db::{Diagnostic, DiagnosticCategory};

        // (label, error, expected code/kind/unit_error_kind/offsets)
        type ArmRow = (
            &'static str,
            Diagnostic,
            ErrorCode,
            FormattedErrorKind,
            Option<UnitErrorKind>,
            (u16, u16),
        );
        let rows: Vec<ArmRow> = vec![
            (
                "equation",
                Diagnostic::equation(
                    EquationError::new(ErrorCode::UnknownDependency, 4, 9),
                    DiagnosticSeverity::Error,
                ),
                ErrorCode::UnknownDependency,
                FormattedErrorKind::Variable,
                None,
                (4, 9),
            ),
            (
                "model, non-unit",
                Diagnostic::engine(
                    CommonError {
                        kind: ErrorKind::Model,
                        code: ErrorCode::CircularDependency,
                        details: Some("a -> b -> a".to_string()),
                    },
                    DiagnosticSeverity::Error,
                ),
                ErrorCode::CircularDependency,
                FormattedErrorKind::Model,
                None,
                (0, 0),
            ),
            (
                "import",
                Diagnostic::engine(
                    CommonError {
                        kind: ErrorKind::Import,
                        code: ErrorCode::VensimConversion,
                        details: None,
                    },
                    DiagnosticSeverity::Error,
                ),
                ErrorCode::VensimConversion,
                FormattedErrorKind::Model,
                None,
                (0, 0),
            ),
            (
                "variable",
                Diagnostic::engine(
                    CommonError {
                        kind: ErrorKind::Variable,
                        code: ErrorCode::DoesNotExist,
                        details: None,
                    },
                    DiagnosticSeverity::Error,
                ),
                ErrorCode::DoesNotExist,
                FormattedErrorKind::Variable,
                None,
                (0, 0),
            ),
            (
                "unit inference",
                Diagnostic::unit(
                    UnitError::InferenceError {
                        code: ErrorCode::UnitMismatch,
                        sources: vec![("v".to_string(), Some(Loc::new(1, 6)))],
                        details: None,
                    },
                    DiagnosticSeverity::Error,
                ),
                ErrorCode::UnitMismatch,
                FormattedErrorKind::Units,
                Some(UnitErrorKind::Inference),
                (1, 6),
            ),
            (
                "unit definition",
                Diagnostic::unit(
                    UnitError::DefinitionError(EquationError::detailed(
                        ErrorCode::UnitDefinitionErrors,
                        0,
                        3,
                        "parse error",
                    )),
                    DiagnosticSeverity::Error,
                ),
                ErrorCode::UnitDefinitionErrors,
                FormattedErrorKind::Units,
                Some(UnitErrorKind::Definition),
                (0, 3),
            ),
            (
                "unit consistency",
                Diagnostic::unit(
                    UnitError::ConsistencyError(
                        ErrorCode::UnitMismatch,
                        Loc::new(2, 8),
                        Some("kg vs m".to_string()),
                    ),
                    DiagnosticSeverity::Error,
                ),
                ErrorCode::UnitMismatch,
                FormattedErrorKind::Units,
                Some(UnitErrorKind::Consistency),
                (2, 8),
            ),
            (
                "assembly",
                Diagnostic::assembly("could not assemble", DiagnosticSeverity::Error),
                ErrorCode::NotSimulatable,
                FormattedErrorKind::Simulation,
                None,
                (0, 0),
            ),
        ];

        assert_eq!(
            rows.iter()
                .map(|(_, diagnostic, ..)| diagnostic.category)
                .collect::<std::collections::HashSet<_>>(),
            DiagnosticCategory::ALL.into_iter().collect(),
            "the formatter matrix must derive a row from every category"
        );
        for (label, diagnostic, code, kind, unit_error_kind, (start, end)) in rows {
            let diag = diagnostic.with_context("main", Some("v".to_string()));
            let fe = format_diagnostic(&diag);
            assert_eq!(fe.code, code, "{label}: code");
            assert_eq!(fe.kind, kind, "{label}: kind");
            assert_eq!(
                fe.unit_error_kind, unit_error_kind,
                "{label}: unit_error_kind"
            );
            assert_eq!(fe.start_offset, start, "{label}: start_offset");
            assert_eq!(fe.end_offset, end, "{label}: end_offset");
            assert_eq!(
                fe.model_name.as_deref(),
                Some("main"),
                "{label}: model_name"
            );
            assert_eq!(fe.variable_name.as_deref(), Some("v"), "{label}: variable");
        }
    }

    /// A diagnostic can carry no variable, and the two arms whose summary
    /// line names one substitute the placeholder `<unknown>` rather than
    /// dropping the clause and producing `variable ''`.
    ///
    /// The placeholder belongs in the human MESSAGE only: on both arms the
    /// structured `variable_name` field carries `diag.variable` through
    /// unchanged, so a variable-less diagnostic reports `None` rather than a
    /// variable literally named `<unknown>` -- the leak to guard against is
    /// the formatter handing its substituted name back as the field.
    #[test]
    fn format_diagnostic_falls_back_to_unknown_variable() {
        use crate::common::UnitError;
        use crate::db::Diagnostic;

        let arms = [
            Diagnostic::equation(
                EquationError::new(ErrorCode::EmptyEquation, 0, 5),
                DiagnosticSeverity::Error,
            ),
            Diagnostic::unit(
                UnitError::ConsistencyError(ErrorCode::UnitMismatch, Loc::new(0, 1), None),
                DiagnosticSeverity::Error,
            ),
        ];

        for diagnostic in arms {
            let diag = diagnostic.with_context("m", None);
            let fe = format_diagnostic(&diag);
            assert_eq!(fe.variable_name, None);
            let message = fe.message.as_ref().expect("message missing");
            assert!(
                message.contains("variable '<unknown>'"),
                "a variable-less diagnostic must name the variable '<unknown>': {message}"
            );
        }
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
