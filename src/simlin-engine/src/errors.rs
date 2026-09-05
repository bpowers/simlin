// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `FormattedError`: the one presentation of a `Diagnostic`, shared by the
//! terminal (the CLI), libsimlin and the MCP servers.

use crate::builtins::Loc;
use crate::common::{Error, ErrorCode, UnitError};
use crate::datamodel::{Equation, Project as DatamodelProject, Variable};
use crate::db::{self, DiagnosticCategory, DiagnosticError, DiagnosticSeverity};

/// Categorisation of the formatted error used for presentation purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormattedErrorKind {
    Project,
    Model,
    Variable,
    Units,
    Simulation,
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
    /// The variable to present the error under: a generated helper's owner
    /// (the variable it was synthesized for -- a user variable, or for an LTM
    /// helper the synthetic link score), else the variable the diagnostic is
    /// filed under.
    pub variable_name: Option<String>,
    pub start_offset: u16,
    pub end_offset: u16,
    pub kind: FormattedErrorKind,
    /// The severity of the diagnostic this was formatted from. `message` is
    /// already worded to match, so a consumer that renders `message` verbatim
    /// needs this only to route (filter, choose a stream, colour); a consumer
    /// that builds its own text should word it from here.
    pub severity: DiagnosticSeverity,
    /// The surface the diagnostic was raised on, which is how the FFI tells
    /// the three unit kinds apart. `None` for a simulation-build failure,
    /// which has no `Diagnostic` behind it.
    pub category: Option<DiagnosticCategory>,
    /// The bare human-readable reason, without the source snippet or the
    /// model/variable summary line that `message` carries (e.g. "the equation
    /// computes to units 'people', but the variable's specified units are
    /// 'person'"). `message` is formatted for terminal output; GUI consumers
    /// that already show the variable in context render this instead.
    ///
    /// Populated whenever the diagnostic carries one (`Diagnostic::reason`;
    /// an inference error's is rewritten by `unit_inference_reason`). `None`
    /// when the raising site had nothing to add beyond the code and the span
    /// -- a parse error, whose reason IS the snippet.
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
        category: None,
        details: None,
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

/// The variables a unit inference error involves, each named once in
/// first-seen order: sources are deduped by (var, loc), so one variable can
/// appear at several locations, and a sentence must never read "'x' and 'x'".
fn involved_names(sources: &[(String, Option<Loc>)]) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for (var, _) in sources {
        if !names.contains(&var.as_str()) {
            names.push(var.as_str());
        }
    }
    names
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
    let names = involved_names(sources);
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

/// Convert a diagnostic into a `FormattedError`.
///
/// No datamodel variable is available, so snippets are omitted;
/// [`format_diagnostic_with_datamodel`] renders them. The summary line's
/// severity word and `FormattedError::severity` are the diagnostic's own
/// (`severity_word`), so an advisory never reads as a compilation failure.
pub fn format_diagnostic(diag: &db::Diagnostic) -> FormattedError {
    format_diagnostic_inner(diag, None)
}

/// Format a diagnostic with snippet context from the datamodel.
///
/// Like `format_diagnostic`, but looks up the variable's equation (or unit
/// string) from `datamodel` to produce source-annotated snippet output for
/// equation and unit errors.
pub fn format_diagnostic_with_datamodel(
    diag: &db::Diagnostic,
    datamodel: &DatamodelProject,
) -> FormattedError {
    let dm_var = datamodel
        .get_model(&diag.model)
        .and_then(|m| presented_variable(diag).and_then(|v| m.get_variable(v)));
    format_diagnostic_inner(diag, dm_var)
}

/// The variable a diagnostic is presented under: a generated helper's owner,
/// which is the equation the modeler can edit, else the variable it is filed
/// under.
fn presented_variable(diag: &db::Diagnostic) -> Option<&str> {
    diag.owner.as_deref().or(diag.variable.as_deref())
}

/// The presentation kind, the category noun the summary line opens with and
/// the text a snippet is rendered from follow `Diagnostic::category`; the
/// line's shape follows the arm.
fn format_diagnostic_inner(diag: &db::Diagnostic, var: Option<&Variable>) -> FormattedError {
    let word = severity_word(diag.severity);
    let model = &diag.model;
    let presented = presented_variable(diag);
    let var_name = presented.unwrap_or("<unknown>");
    let code = diag.code();
    let mut details = diag.reason().map(str::to_owned);
    let (kind, noun, snippet_text) = match diag.category() {
        DiagnosticCategory::Equation => (
            FormattedErrorKind::Variable,
            "",
            var.and_then(variable_equation_text),
        ),
        DiagnosticCategory::Model => (FormattedErrorKind::Model, "", None),
        DiagnosticCategory::UnitDefinition => (
            FormattedErrorKind::Units,
            "units ",
            var.and_then(|v| v.get_units())
                .map(|units| units.to_string()),
        ),
        DiagnosticCategory::UnitConsistency => (
            FormattedErrorKind::Units,
            "units ",
            var.and_then(variable_equation_text),
        ),
        DiagnosticCategory::UnitInference => (
            FormattedErrorKind::Units,
            "units inference ",
            var.and_then(variable_equation_text),
        ),
        DiagnosticCategory::Assembly => (FormattedErrorKind::Simulation, "assembly ", None),
    };
    let summary = match &diag.error {
        // A model-level error renders whole (`ModelError{code: reason}`), and
        // an assembly refusal's message is its whole payload, so neither
        // carries a separate reason.
        DiagnosticError::Model(err) => format!("{word} in model '{model}': {err}"),
        DiagnosticError::Assembly(message) => {
            details = None;
            format!("{noun}{word} in model '{model}': {message}")
        }
        DiagnosticError::Unit(UnitError::InferenceError {
            sources,
            details: raw,
            ..
        }) => {
            // The terminal summary keeps the raw constraint text; `details`
            // carries the plain-language reason a GUI shows in place of it.
            // A conflict names the variables it involves; the variable the
            // row is filed under is the subject only when it is the single
            // one involved (the model-level umbrella is filed under none).
            details = Some(unit_inference_reason(sources, raw.as_deref()));
            let involved = involved_names(sources);
            let subject = match (presented, involved.as_slice()) {
                (Some(_), [] | [_]) | (None, []) => format!("variable '{var_name}'"),
                _ => format!("involving {}", involved.join(", ")),
            };
            format!(
                "{noun}{word} in model '{model}' {subject}: {}",
                code_and_reason(code, raw.as_deref())
            )
        }
        DiagnosticError::Equation(_) | DiagnosticError::Unit(_) => format!(
            "{noun}{word} in model '{model}' variable '{var_name}': {}",
            code_and_reason(code, diag.reason())
        ),
    };
    // A span is an offset into the presented variable's text; a diagnostic
    // filed under no variable has nothing for it to index.
    let (start_offset, end_offset) = match (presented, diag.location()) {
        (Some(_), Some(loc)) => (loc.start, loc.end),
        _ => (0, 0),
    };
    let snippet = snippet_text.map(|text| format_snippet(&text, start_offset, end_offset));
    FormattedError {
        code,
        message: combine_snippet_and_summary(snippet, summary),
        model_name: Some(model.clone()),
        variable_name: presented.map(str::to_owned),
        start_offset,
        end_offset,
        kind,
        severity: diag.severity,
        category: Some(diag.category()),
        details,
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
    use crate::common::{EquationError, ErrorCode};
    use crate::db::{Diagnostic, SimlinDb, collect_all_diagnostics, sync_from_datamodel};
    use crate::test_common::TestProject;

    /// A unit diagnostic as `db/units.rs` files one, for the formatter arms
    /// that no small production fixture reaches at every source shape.
    fn unit_diagnostic(
        model: &str,
        variable: Option<&str>,
        error: UnitError,
        severity: DiagnosticSeverity,
    ) -> Diagnostic {
        Diagnostic {
            model: model.to_string(),
            variable: variable.map(str::to_owned),
            owner: None,
            severity,
            error: DiagnosticError::Unit(error),
        }
    }

    #[test]
    fn equation_error_formats_snippet() {
        let datamodel = TestProject::new("equation-error")
            .aux("bad", "1 + bogus", None)
            .build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let diagnostics = collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::Off);
        let formatted = collect_formatted_errors(&diagnostics, &datamodel);

        assert!(formatted.has_variable_errors);
        let error = formatted
            .errors
            .iter()
            .find(|err| err.variable_name.as_deref() == Some("bad"))
            .expect("equation error missing");

        assert_eq!(error.code, ErrorCode::UnknownDependency);
        assert_eq!(error.kind, FormattedErrorKind::Variable);
        assert_eq!(error.category, Some(DiagnosticCategory::Equation));
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
        let diagnostics = collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::Off);
        let formatted = collect_formatted_errors(&diagnostics, &datamodel);

        let error = formatted
            .errors
            .iter()
            .find(|err| err.variable_name.as_deref() == Some("bad_units"))
            .expect("unit error missing");
        assert_eq!(error.code, ErrorCode::UnitMismatch);
        assert_eq!(error.kind, FormattedErrorKind::Units);
        assert_eq!(error.category, Some(DiagnosticCategory::UnitConsistency));
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
        let diagnostics = collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::Off);
        let formatted = collect_formatted_errors(&diagnostics, &datamodel);

        let error = formatted
            .errors
            .iter()
            .find(|err| err.category == Some(DiagnosticCategory::UnitDefinition))
            .expect("unit definition error missing");
        assert_eq!(error.severity, DiagnosticSeverity::Error);
        let message = error.message.as_ref().expect("message missing");
        assert!(
            message.contains("units error in model"),
            "an Error-severity unit definition error keeps the error word: {message}"
        );
    }

    /// The model-level inference umbrella `db/units.rs` raises (a
    /// contradiction the constraint solver found, filed under no variable)
    /// presents as a unit inference warning naming the variables it involves.
    #[test]
    fn inference_umbrella_presents_as_a_unit_inference_warning() {
        let datamodel = TestProject::new("inference-umbrella")
            .unit("apples", None)
            .unit("oranges", None)
            .aux("apple_count", "10", Some("apples"))
            .aux("orange_count", "20", Some("oranges"))
            .aux("fruit_total", "apple_count + orange_count", None)
            .build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let diagnostics = collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::Off);
        let umbrella = diagnostics
            .iter()
            .find(|d| d.variable.is_none() && d.category() == DiagnosticCategory::UnitInference)
            .expect("the model-level inference umbrella");
        let formatted = format_diagnostic_with_datamodel(umbrella, &datamodel);
        assert_eq!(formatted.code, ErrorCode::UnitMismatch);
        assert_eq!(formatted.kind, FormattedErrorKind::Units);
        assert_eq!(formatted.category, Some(DiagnosticCategory::UnitInference));
        assert_eq!(formatted.severity, DiagnosticSeverity::Warning);
        assert_eq!(formatted.variable_name, None);
        assert_eq!(
            (formatted.start_offset, formatted.end_offset),
            (0, 0),
            "a row filed under no variable has no text for a span to index"
        );
        let message = formatted.message.expect("message missing");
        assert!(
            message.starts_with("units inference warning in model 'main' involving ")
                && message.contains("apple_count")
                && message.contains("unit_mismatch"),
            "the umbrella names the model and the variables involved: {message}"
        );
        let details = formatted.details.expect("details missing");
        assert!(
            !details.contains('\n') && !details.contains("1 =="),
            "the bare reason is plain language, not the constraint dump: {details}"
        );
    }

    /// A per-variable inference row names each involved variable once: the
    /// sources are deduped by (variable, location), so one variable at two
    /// locations is two sources and one name.
    #[test]
    fn an_inference_row_names_each_involved_variable_once() {
        use crate::builtins::Loc;
        use crate::common::UnitError;

        let formatted = format_diagnostic(&unit_diagnostic(
            "main",
            Some("x"),
            UnitError::InferenceError {
                code: ErrorCode::UnitMismatch,
                sources: vec![
                    ("x".to_string(), Some(Loc::new(0, 1))),
                    ("x".to_string(), Some(Loc::new(4, 5))),
                    ("y".to_string(), None),
                ],
                details: None,
            },
            DiagnosticSeverity::Warning,
        ));
        let message = formatted.message.expect("message missing");
        assert!(
            message.starts_with("units inference warning in model 'main' involving x, y:"),
            "{message}"
        );
        assert_eq!(
            formatted.details.as_deref(),
            Some(
                "the units of 'x' and 'y' are inconsistent with each other; check the \
                 equations and declared units of these variables"
            )
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
        let formatted = format_diagnostic(&unit_diagnostic(
            "test_model",
            Some("my_var"),
            error.clone(),
            DiagnosticSeverity::Warning,
        ));
        assert_eq!(formatted.code, ErrorCode::UnitMismatch);
        assert_eq!(formatted.kind, FormattedErrorKind::Units);
        assert_eq!(formatted.category, Some(DiagnosticCategory::UnitInference));
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

        let formatted = format_diagnostic(&unit_diagnostic(
            "test_model",
            Some("my_var"),
            error,
            DiagnosticSeverity::Error,
        ));
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
        let formatted = format_diagnostic(&unit_diagnostic(
            "test_model",
            Some("var_a"),
            error,
            DiagnosticSeverity::Warning,
        ));
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
        let formatted = format_diagnostic(&unit_diagnostic(
            "test_model",
            Some("no_loc_var"),
            error,
            DiagnosticSeverity::Warning,
        ));
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
        let formatted = format_diagnostic(&unit_diagnostic(
            "main",
            Some("birth_rate"),
            error,
            DiagnosticSeverity::Warning,
        ));
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
        let formatted = format_diagnostic(&unit_diagnostic(
            "main",
            Some("flow"),
            error,
            DiagnosticSeverity::Warning,
        ));
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
        let formatted = format_diagnostic(&unit_diagnostic(
            "main",
            Some("x"),
            error,
            DiagnosticSeverity::Warning,
        ));
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
        let formatted = format_diagnostic(&unit_diagnostic(
            "main",
            Some("x"),
            error,
            DiagnosticSeverity::Warning,
        ));
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
        let diagnostics = collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::Off);

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

        let arm = |error| Diagnostic {
            model: "main".to_string(),
            variable: Some("v".to_string()),
            owner: None,
            severity,
            error,
        };
        vec![
            arm(DiagnosticError::Equation(EquationError::new(
                ErrorCode::UnknownDependency,
                0,
                1,
            ))),
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
    /// renders through the same arms with a datamodel variable in hand.
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
        assert_eq!(fe.category, None);
        assert!(
            fe.message
                .as_deref()
                .expect("message missing")
                .starts_with("error compiling model 'main'")
        );
    }

    /// Beyond the severity word, `format_diagnostic` makes three per-arm
    /// decisions -- the presentation `kind`, the `category`, and where the
    /// source offsets come from. The rows are the arms of that decision:
    /// `DiagnosticError`'s four variants, with `Unit` split into `UnitError`'s
    /// three, which is exactly `DiagnosticCategory`'s enumeration.
    /// `diagnostic_arms` above ranges over the same space but asserts only the
    /// severity wording, so this is what holds the mapping itself.
    #[test]
    fn format_diagnostic_maps_every_arm() {
        use crate::common::{Error as CommonError, ErrorKind, UnitError};

        // (label, error, expected code/kind/category/offsets)
        type ArmRow = (
            &'static str,
            DiagnosticError,
            ErrorCode,
            FormattedErrorKind,
            DiagnosticCategory,
            (u16, u16),
        );
        let rows: Vec<ArmRow> = vec![
            (
                "equation",
                DiagnosticError::Equation(EquationError::new(ErrorCode::UnknownDependency, 4, 9)),
                ErrorCode::UnknownDependency,
                FormattedErrorKind::Variable,
                DiagnosticCategory::Equation,
                (4, 9),
            ),
            (
                "model",
                DiagnosticError::Model(CommonError {
                    kind: ErrorKind::Model,
                    code: ErrorCode::CircularDependency,
                    details: Some("a -> b -> a".to_string()),
                }),
                ErrorCode::CircularDependency,
                FormattedErrorKind::Model,
                DiagnosticCategory::Model,
                (0, 0),
            ),
            (
                "unit definition",
                DiagnosticError::Unit(UnitError::DefinitionError(EquationError::detailed(
                    ErrorCode::UnitDefinitionErrors,
                    0,
                    3,
                    "parse error",
                ))),
                ErrorCode::UnitDefinitionErrors,
                FormattedErrorKind::Units,
                DiagnosticCategory::UnitDefinition,
                (0, 3),
            ),
            (
                "unit consistency",
                DiagnosticError::Unit(UnitError::ConsistencyError(
                    ErrorCode::UnitMismatch,
                    Loc::new(2, 8),
                    Some("kg vs m".to_string()),
                )),
                ErrorCode::UnitMismatch,
                FormattedErrorKind::Units,
                DiagnosticCategory::UnitConsistency,
                (2, 8),
            ),
            (
                "unit inference",
                DiagnosticError::Unit(UnitError::InferenceError {
                    code: ErrorCode::UnitMismatch,
                    sources: vec![("v".to_string(), Some(Loc::new(1, 6)))],
                    details: None,
                }),
                ErrorCode::UnitMismatch,
                FormattedErrorKind::Units,
                DiagnosticCategory::UnitInference,
                (1, 6),
            ),
            (
                "assembly",
                DiagnosticError::Assembly("could not assemble".to_string()),
                ErrorCode::NotSimulatable,
                FormattedErrorKind::Simulation,
                DiagnosticCategory::Assembly,
                (0, 0),
            ),
        ];

        for (label, error, code, kind, category, (start, end)) in rows {
            let diag = Diagnostic {
                model: "main".to_string(),
                variable: Some("v".to_string()),
                owner: None,
                severity: DiagnosticSeverity::Error,
                error,
            };
            let fe = format_diagnostic(&diag);
            assert_eq!(fe.code, code, "{label}: code");
            assert_eq!(fe.kind, kind, "{label}: kind");
            assert_eq!(fe.category, Some(category), "{label}: category");
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

    /// A diagnostic can carry no variable, and the arms whose summary line
    /// names one substitute the placeholder `<unknown>` rather than dropping
    /// the clause and producing `variable ''`.
    ///
    /// The placeholder belongs in the human MESSAGE only: the structured
    /// `variable_name` field carries `diag.variable` through unchanged, so a
    /// variable-less diagnostic reports `None` rather than a variable literally
    /// named `<unknown>`.
    #[test]
    fn format_diagnostic_falls_back_to_unknown_variable() {
        use crate::common::UnitError;

        let arms = [
            DiagnosticError::Equation(EquationError::new(ErrorCode::EmptyEquation, 0, 5)),
            DiagnosticError::Unit(UnitError::ConsistencyError(
                ErrorCode::UnitMismatch,
                Loc::new(0, 1),
                None,
            )),
        ];

        for error in arms {
            let diag = Diagnostic {
                model: "m".to_string(),
                variable: None,
                owner: None,
                severity: DiagnosticSeverity::Error,
                error,
            };
            let fe = format_diagnostic(&diag);
            assert_eq!(fe.variable_name, None);
            let message = fe.message.as_ref().expect("message missing");
            assert!(
                message.contains("variable '<unknown>'"),
                "a variable-less diagnostic must name the variable '<unknown>': {message}"
            );
        }
    }

    /// A generated helper's row is filed under its physical name and presented
    /// under its owner: the owner is the variable a consumer can find and
    /// edit, and the one whose equation text a snippet is rendered from. Read
    /// through a production refusal: a reducer over arithmetic inside a
    /// `PREVIOUS` capture, which codegen refuses on the helper.
    #[test]
    fn a_helpers_row_presents_under_its_owner() {
        let datamodel = TestProject::new("helper-owner")
            .named_dimension("region", &["north", "south"])
            .array_const("pop[region]", 10.0)
            .scalar_const("scale", 2.0)
            .array_aux("aggx[region]", "PREVIOUS(SUM(pop * scale))")
            .build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let diagnostics = collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::Off);
        let helper_row = diagnostics
            .iter()
            .find(|d| d.owner.as_deref() == Some("aggx"))
            .expect("the helper's refusal names its owner");
        assert!(
            helper_row
                .variable
                .as_deref()
                .is_some_and(|v| v.starts_with("$\u{205a}aggx\u{205a}")),
            "the row is filed under the helper's physical name: {helper_row:?}"
        );
        let formatted = format_diagnostic_with_datamodel(helper_row, &datamodel);
        assert_eq!(formatted.variable_name.as_deref(), Some("aggx"));
        assert_eq!(formatted.kind, FormattedErrorKind::Simulation);
        assert_eq!(formatted.category, Some(DiagnosticCategory::Assembly));
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
