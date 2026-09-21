// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Evidence ids: the names a session gives what its tools report, so an agent
//! can cite them.
//!
//! A diagnostic's id (`D1`, `D2`, ...) is assigned the first time a session
//! reports it and belongs to that problem for the session's life: an agent
//! that said "D3" before an edit means the same problem after it, a new problem
//! gets a new number, and a problem that goes away and comes back gets its old
//! one. A diagnostic is identified by what a person would call
//! "the same problem" -- its variable, severity, code and reason -- and not by
//! where in the equation it points, since an edit that moves the span leaves
//! the problem the same. Two rows alike in all of that (one variable failing
//! the same way at two places) are told apart by their order in the report.
//!
//! A finding's id (`F1`, ...) is keyed by its kind and its claim.
//!
//! A battery check's id (`T1`, ...) is keyed by its test, the variable it
//! changes and how, so a check run again after an edit keeps its id.
//!
//! A loop's id (`L1`, `L2`, ...) is keyed by its cycle: its node sequence
//! rotated to start at its least node, so the same loop read from any run, at
//! any revision, from a run's scores or from structure, has one id, and the
//! same nodes in the other direction are another loop.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::common::ErrorCode;
use crate::datamodel;
use crate::db::{
    Diagnostic, DiagnosticCategory, DiagnosticSeverity, LtmOverlay, collect_all_diagnostics,
};
use crate::errors::format_diagnostic_with_datamodel;

use super::{ResolvedModel, Workspace};

/// Whether a diagnostic stops the model from simulating or only warns.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

impl From<DiagnosticSeverity> for Severity {
    fn from(severity: DiagnosticSeverity) -> Severity {
        match severity {
            DiagnosticSeverity::Error => Severity::Error,
            DiagnosticSeverity::Warning => Severity::Warning,
        }
    }
}

/// Where a diagnostic was raised: the engine's [`DiagnosticCategory`], named
/// for a reader.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategoryName {
    /// An equation that does not parse, names something that does not
    /// exist, or cannot be compiled.
    Equation,
    /// A problem with the model as a whole: a cycle, a duplicate name, a bad
    /// table.
    Model,
    /// A units string that does not parse.
    UnitDefinition,
    /// An equation whose computed units disagree with its declared units.
    UnitConsistency,
    /// A contradiction among the units the model's equations imply.
    UnitInference,
    /// A construct the compiler refuses.
    Assembly,
    /// A value a run produces that is not a number: what an edit's gate
    /// finds in the run, where the engine's diagnostics are silent.
    Value,
}

impl From<DiagnosticCategory> for DiagnosticCategoryName {
    fn from(category: DiagnosticCategory) -> DiagnosticCategoryName {
        match category {
            DiagnosticCategory::Equation => DiagnosticCategoryName::Equation,
            DiagnosticCategory::Model => DiagnosticCategoryName::Model,
            DiagnosticCategory::UnitDefinition => DiagnosticCategoryName::UnitDefinition,
            DiagnosticCategory::UnitConsistency => DiagnosticCategoryName::UnitConsistency,
            DiagnosticCategory::UnitInference => DiagnosticCategoryName::UnitInference,
            DiagnosticCategory::Assembly => DiagnosticCategoryName::Assembly,
        }
    }
}

/// A diagnostic as a tool reports it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    /// The session's id for this diagnostic (`D1`, ...), stable while it exists.
    pub id: String,
    pub severity: Severity,
    pub category: DiagnosticCategoryName,
    /// The engine's code for the class of failure, in snake_case.
    pub code: String,
    /// The variable to fix, as the model names it; absent for a problem with
    /// the model or the project as a whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    /// Why, in words: the raising site's reason, and the text it points at
    /// whenever it has a span; what its code means only when neither.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What identifies a diagnostic across revisions: see the module docs.
#[derive(Clone, PartialEq, Eq, Hash)]
struct DiagnosticKey {
    variable: Option<String>,
    severity: Severity,
    code: ErrorCode,
    reason: Option<String>,
    /// Which of the rows alike in all of the above this is, in report order.
    occurrence: usize,
}

/// The ids a session has given out.
#[derive(Default, Clone)]
pub(crate) struct Evidence {
    diagnostics: HashMap<DiagnosticKey, u32>,
    next_diagnostic: u32,
    loops: HashMap<Vec<String>, u32>,
    next_loop: u32,
    tests: HashMap<super::battery::TestKey, u32>,
    next_test: u32,
    findings: HashMap<(super::verify::FindingKind, String), u32>,
    next_finding: u32,
}

impl Evidence {
    /// The id of the finding of `kind` that claims `claim`, however it is
    /// spaced.
    pub(crate) fn finding_id(&mut self, kind: super::verify::FindingKind, claim: &str) -> String {
        let claim = claim.split_whitespace().collect::<Vec<_>>().join(" ");
        let next = &mut self.next_finding;
        let number = *self.findings.entry((kind, claim)).or_insert_with(|| {
            *next += 1;
            *next
        });
        format!("F{number}")
    }

    /// The id of the battery check `key` names.
    pub(crate) fn test_id(&mut self, key: &super::battery::TestKey) -> String {
        let next = &mut self.next_test;
        let number = *self.tests.entry(key.clone()).or_insert_with(|| {
            *next += 1;
            *next
        });
        format!("T{number}")
    }

    /// The cycle, in canonical rotation, of the loop this session calls `id`.
    pub(crate) fn loop_key(&self, id: &str) -> Option<&[String]> {
        let number: u32 = id.strip_prefix('L')?.parse().ok()?;
        self.loops
            .iter()
            .find(|&(_, &n)| n == number)
            .map(|(key, _)| key.as_slice())
    }

    /// The id of the loop whose cycle, in canonical rotation, is `key`.
    pub(crate) fn loop_id(&mut self, key: &[String]) -> String {
        let next = &mut self.next_loop;
        let number = *self.loops.entry(key.to_vec()).or_insert_with(|| {
            *next += 1;
            *next
        });
        format!("L{number}")
    }

    fn diagnostic_id(&mut self, key: DiagnosticKey) -> String {
        let next = &mut self.next_diagnostic;
        let number = *self.diagnostics.entry(key).or_insert_with(|| {
            *next += 1;
            *next
        });
        format!("D{number}")
    }

    /// The model's diagnostics as the engine collects them (the model as
    /// written, without the LTM overlay: an analysis's advisories belong to
    /// the analysis), each under its id. Project-level diagnostics, which
    /// belong to no model, are the model's too, since they stop it compiling.
    pub(crate) fn report_diagnostics(
        &mut self,
        ws: &Workspace<'_>,
        resolved: &ResolvedModel<'_>,
    ) -> Vec<DiagnosticReport> {
        let model_name = crate::canonicalize(&resolved.model.name).into_owned();
        let diagnostics: Vec<Diagnostic> =
            collect_all_diagnostics(ws.db, resolved.source_project, LtmOverlay::Off)
                .into_iter()
                .filter(|d| d.model.is_empty() || crate::canonicalize(&d.model) == model_name)
                .collect();
        let mut seen: HashMap<(Option<String>, Severity, ErrorCode, Option<String>), usize> =
            HashMap::new();
        diagnostics
            .iter()
            .map(|diagnostic| {
                let described = describe(diagnostic, ws.project, resolved.model);
                let occurrence = seen
                    .entry((
                        described.variable.clone(),
                        described.severity,
                        described.code,
                        described.engine_reason.clone(),
                    ))
                    .and_modify(|n| *n += 1)
                    .or_insert(0);
                let id = self.diagnostic_id(DiagnosticKey {
                    variable: described.variable.clone(),
                    severity: described.severity,
                    code: described.code,
                    reason: described.engine_reason.clone(),
                    occurrence: *occurrence,
                });
                described.report(id, resolved.model)
            })
            .collect()
    }
}

/// The reason a report gives, which an agent needs both halves of: what went
/// wrong, in the raising site's words, and where, as the span it points at
/// whenever it has one. What the code means ("the equation contains
/// unexpected input") only when there is neither.
fn reason_given(
    diagnostic: &Diagnostic,
    formatted: &crate::errors::FormattedError,
    model: &datamodel::Model,
) -> Option<String> {
    let description = diagnostic.code().description();
    let own = formatted
        .details
        .clone()
        .filter(|reason| reason != description);
    match (own, quoted_span(diagnostic, formatted, model)) {
        (Some(own), Some(quote)) => Some(format!("{own}, {quote}")),
        (Some(own), None) => Some(own),
        (None, Some(quote)) => Some(quote),
        (None, None) => formatted.details.clone(),
    }
}

/// A diagnostic as a tool describes it, before any id: what a report of it
/// says, and what identifies the problem.
pub(crate) struct Described {
    pub severity: Severity,
    pub category: DiagnosticCategoryName,
    pub code: ErrorCode,
    /// The variable to fix, canonically.
    pub variable: Option<String>,
    /// The engine's reason, which identifies the problem.
    pub engine_reason: Option<String>,
    /// The reason a report gives ([`reason_given`]).
    pub reason: Option<String>,
}

impl Described {
    /// The report of this diagnostic under `id`, its variable named as
    /// `model` spells it.
    pub(crate) fn report(self, id: String, model: &datamodel::Model) -> DiagnosticReport {
        DiagnosticReport {
            id,
            severity: self.severity,
            category: self.category,
            code: self.code.to_string(),
            variable: self.variable.map(|name| display_name(model, &name)),
            reason: self.reason,
        }
    }
}

/// Describe `diagnostic` of `model` in `project`, which may be a staged copy
/// of the project a session reads (the model as an edit would leave it).
pub(crate) fn describe(
    diagnostic: &Diagnostic,
    project: &datamodel::Project,
    model: &datamodel::Model,
) -> Described {
    let formatted = format_diagnostic_with_datamodel(diagnostic, project);
    // The key reads the engine's reason, never the quote a parse error is
    // shown with: the quote holds the equation's text, so keying on it would
    // give the same problem a new id at every edit of its equation.
    let engine_reason = formatted.details.clone();
    let reason = reason_given(diagnostic, &formatted, model);
    Described {
        severity: Severity::from(diagnostic.severity),
        category: diagnostic.category().into(),
        code: diagnostic.code(),
        variable: formatted.variable_name,
        engine_reason,
        reason,
    }
}

/// A variable's name as the model spells it, for a name the engine reports
/// canonically; the name itself when the model has no such variable.
pub(crate) fn display_name(model: &datamodel::Model, name: &str) -> String {
    model
        .get_variable(name)
        .map(|v| v.get_ident().to_string())
        .unwrap_or_else(|| name.to_string())
}

/// Where a diagnostic points, as the text its span covers, quoted with the
/// equation or units string it is part of.
///
/// Only an equation or units-string diagnostic with a location has a span to
/// quote. A model-level diagnostic gets none rather than a quote of some
/// equation, which would read as the problem's location.
fn quoted_span(
    diagnostic: &Diagnostic,
    formatted: &crate::errors::FormattedError,
    model: &datamodel::Model,
) -> Option<String> {
    diagnostic.location()?;
    let var = model.get_variable(formatted.variable_name.as_deref()?)?;
    let text = match diagnostic.category() {
        DiagnosticCategory::UnitDefinition => var.get_units()?.clone(),
        DiagnosticCategory::Equation => match var.get_equation()? {
            datamodel::Equation::Scalar(eqn) | datamodel::Equation::ApplyToAll(_, eqn) => {
                eqn.clone()
            }
            datamodel::Equation::Arrayed(..) => return None,
        },
        DiagnosticCategory::Model
        | DiagnosticCategory::UnitConsistency
        | DiagnosticCategory::UnitInference
        | DiagnosticCategory::Assembly => return None,
    };
    let (start, end) = (
        formatted.start_offset as usize,
        formatted.end_offset as usize,
    );
    let span = if end > start {
        text.get(start..end)
    } else {
        None
    };
    Some(match span {
        Some(span) if !span.trim().is_empty() => format!(
            "at `{}` in `{}`",
            window(span, 0, span.len(), QUOTE_CHARS / 2),
            window(&text, start, end, QUOTE_CHARS)
        ),
        _ => format!("in `{}`", window(&text, 0, 0, QUOTE_CHARS)),
    })
}

/// The most characters a quote of an equation or units string carries: an
/// equation can run to thousands, and a quote says where, not what.
pub(crate) const QUOTE_CHARS: usize = 240;

/// `text` when it has at most `limit` characters, else the `limit` characters
/// around the byte range `start..end` (the start when it is empty), with `…`
/// where it was cut.
pub(crate) fn window(text: &str, start: usize, end: usize, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }
    let char_at = |byte: usize| text.get(..byte).map_or(0, |prefix| prefix.chars().count());
    let (first, last) = (char_at(start), char_at(end).max(char_at(start)));
    let middle = (first + last) / 2;
    let from = middle.saturating_sub(limit / 2).min(chars.len() - limit);
    let to = from + limit;
    let mut quoted = String::new();
    if from > 0 {
        quoted.push('…');
    }
    quoted.extend(&chars[from..to]);
    if to < chars.len() {
        quoted.push('…');
    }
    quoted
}

#[cfg(test)]
#[path = "evidence_tests.rs"]
mod tests;
