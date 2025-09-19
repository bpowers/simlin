// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Helpers for formatting engine errors for human-readable output.

use simlin_engine::common::{EquationError, Error, ErrorCode, UnitError};
use simlin_engine::datamodel::{Equation, Project as DatamodelProject, Variable};
use simlin_engine::{self as engine};

/// Categorisation of the formatted error used for presentation purposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormattedErrorKind {
    Project,
    Model,
    Variable,
    Units,
    Simulation,
}

/// A formatted error containing a human readable message and associated metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormattedError {
    pub code: ErrorCode,
    pub message: Option<String>,
    pub model_name: Option<String>,
    pub variable_name: Option<String>,
    pub start_offset: u16,
    pub end_offset: u16,
    pub kind: FormattedErrorKind,
}

/// Collection of formatted errors plus bookkeeping flags that mirror previous CLI output
/// decisions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormattedErrors {
    pub errors: Vec<FormattedError>,
    pub has_model_errors: bool,
    pub has_variable_errors: bool,
}

/// Format all static errors for a compiled project, matching the CLI style.
pub fn collect_formatted_errors(project: &engine::Project) -> FormattedErrors {
    let mut formatted = FormattedErrors::default();

    for error in &project.errors {
        formatted.errors.push(format_project_error(error));
    }

    let datamodel: &DatamodelProject = &project.datamodel;

    for (model_name, model) in &project.models {
        let model_name = model_name.as_str();
        let datamodel_model = datamodel.get_model(model_name);

        let variable_errors = model.get_variable_errors();
        if !variable_errors.is_empty() {
            formatted.has_variable_errors = true;
        }
        for (var_name, errors) in variable_errors {
            let datamodel_var = datamodel_model.and_then(|m| m.get_variable(var_name.as_str()));
            for error in errors {
                formatted.errors.push(format_equation_error(
                    model_name,
                    var_name.as_str(),
                    datamodel_var,
                    &error,
                ));
            }
        }

        let unit_errors = model.get_unit_errors();
        for (var_name, errors) in unit_errors {
            let datamodel_var = datamodel_model.and_then(|m| m.get_variable(var_name.as_str()));
            for error in errors {
                formatted.errors.push(format_unit_error(
                    model_name,
                    var_name.as_str(),
                    datamodel_var,
                    &error,
                ));
            }
        }

        if let Some(model_errors) = &model.errors {
            for error in model_errors {
                if error.code == ErrorCode::VariablesHaveErrors
                    && !model.get_variable_errors().is_empty()
                {
                    continue;
                }
                formatted.has_model_errors = true;
                formatted.errors.push(format_model_error(model_name, error));
            }
        }
    }

    formatted
}

/// Format a simulation error reported while creating a VM.
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
    }
}

fn format_project_error(error: &Error) -> FormattedError {
    let message = format!("project error: {error}");
    FormattedError {
        code: error.code,
        message: Some(message),
        model_name: None,
        variable_name: None,
        start_offset: 0,
        end_offset: 0,
        kind: FormattedErrorKind::Project,
    }
}

fn format_model_error(model_name: &str, error: &Error) -> FormattedError {
    let message = format!("error in model '{model_name}': {error}");
    FormattedError {
        code: error.code,
        message: Some(message),
        model_name: Some(model_name.to_string()),
        variable_name: None,
        start_offset: 0,
        end_offset: 0,
        kind: FormattedErrorKind::Model,
    }
}

fn format_equation_error(
    model_name: &str,
    var_name: &str,
    var: Option<&Variable>,
    error: &EquationError,
) -> FormattedError {
    let snippet = var
        .and_then(variable_equation_text)
        .map(|eqn| format_snippet(&eqn, error.start, error.end));
    let summary = format!(
        "error in model '{model_name}' variable '{var_name}': {}",
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
    }
}

fn format_unit_error(
    model_name: &str,
    var_name: &str,
    var: Option<&Variable>,
    error: &UnitError,
) -> FormattedError {
    match error {
        UnitError::DefinitionError(eq_error, details) => {
            let snippet = var
                .and_then(|v| v.get_units())
                .map(|units| format_snippet(units, eq_error.start, eq_error.end));
            let summary = match details {
                Some(details) => format!(
                    "units error in model '{model_name}' variable '{var_name}': {} -- {}",
                    eq_error.code, details
                ),
                None => format!(
                    "units error in model '{model_name}' variable '{var_name}': {}",
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
            }
        }
        UnitError::ConsistencyError(code, loc, details) => {
            let snippet = var
                .and_then(variable_equation_text)
                .map(|eqn| format_snippet(&eqn, loc.start, loc.end));
            let summary = match details {
                Some(details) => format!(
                    "units error in model '{model_name}' variable '{var_name}': {code} -- {details}"
                ),
                None => {
                    format!("units error in model '{model_name}' variable '{var_name}': {code}")
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
            }
        }
    }
}

fn variable_equation_text(var: &Variable) -> Option<String> {
    match var.get_equation() {
        Some(Equation::Scalar(eqn, _)) => Some(eqn.clone()),
        Some(Equation::ApplyToAll(_, eqn, _)) => Some(eqn.clone()),
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
    use simlin_engine::common::ErrorCode;
    use simlin_engine::test_common::TestProject;

    #[test]
    fn equation_error_formats_snippet() {
        let datamodel = TestProject::new("equation-error")
            .aux("bad", "1 + bogus", None)
            .build_datamodel();
        let project = engine::Project::from(datamodel);
        let formatted = collect_formatted_errors(&project);

        assert!(formatted.has_variable_errors);
        let error = formatted
            .errors
            .iter()
            .find(|err| err.variable_name.as_deref() == Some("bad"))
            .expect("equation error missing");

        assert_eq!(error.code, ErrorCode::UnknownDependency);
        assert_eq!(error.kind, FormattedErrorKind::Variable);
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
        let project = engine::Project::from(datamodel);
        let formatted = collect_formatted_errors(&project);

        let error = formatted
            .errors
            .iter()
            .find(|err| err.variable_name.as_deref() == Some("bad_units"))
            .expect("unit error missing");
        assert_eq!(error.code, ErrorCode::UnitMismatch);
        assert_eq!(error.kind, FormattedErrorKind::Units);
        let message = error.message.as_ref().expect("message missing");
        let mut lines = message.lines();
        assert_eq!(lines.next().unwrap(), "    source");
        assert_eq!(lines.next().unwrap(), "    ~~~~~~");
        assert!(lines
            .next()
            .unwrap()
            .contains("units error in model 'main' variable 'bad_units': unit_mismatch"));
        assert!(lines.next().is_none());
    }
}
