// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `verify_findings`: whether a claim's evidence holds.
//!
//! A finding is a claim a person will read and the citations it rests on,
//! each naming something the tools report: a variable, its equation, the
//! variables that read it, a link, a diagnostic, a loop and its polarity, a
//! loop's lead over a span of a run, a fact of a run (a value at a time, it
//! goes negative, peaks at a time, ends near a value, shows a behavior mode),
//! a comparison of two runs, an absence (no diagnostic of a kind, no loop
//! through a variable, no reader), or a battery check's outcome. Each citation is
//! checked against the session at the current revision, deterministically,
//! within the tolerance its kind states, and one that does not hold says what
//! is true instead. A finding every citation of which holds gets an id (`F1`,
//! ...) keyed by its claim, for the host to show it under; one that does not
//! is the agent's to repair or withdraw.
//!
//! A number is judged against the number itself ([`near`]): within 5% of the
//! larger of the two, and a cited zero against the series' largest magnitude. A peak is a largest value the series comes down from on
//! both sides ([`peak`]): where a run ends is not where a series peaks.
//!
//! Whether the claim follows from its citations is a judgment this does not
//! make: "the reinforcing loop drives the growth", citing only that the loop
//! exists, holds here. A run made before the model changed is not evidence
//! about the model: a citation of one fails, and names the repair.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::common::CanonicalElementName;
use crate::datamodel;

use super::battery::{Outcome, recheck};
use super::behavior::{ModeKind, NOISE_FRACTION, classify_at};
use super::loops::{
    LoopAnalysis, LoopPolarityName, analysis_of, leadership, loops_through, polarity_of,
    rounded_share, through_of, unreported,
};
use super::outline::dimensions;
use super::runs::{CURRENT, Run};
use super::series::{KeyedSeries, SeriesCore, keyed_series_upto, round, scale_in_run};
use super::variables::LinkPolarityName;
use super::{DiagnosticCategoryName, Session, ToolError, Workspace, names, resolve_model};

/// The most findings one call verifies, and citations one finding makes.
pub(crate) const MAX_FINDINGS: usize = 12;
pub(crate) const MAX_CITATIONS: usize = 8;

/// How near a peak's time must be to the cited one: a fraction of the run.
pub(crate) const PEAK_TOLERANCE: f64 = 0.05;

/// How near a value must be to the cited one, at a time or at the end: a
/// fraction of the larger of the two.
pub(crate) const VALUE_TOLERANCE: f64 = 0.05;

/// How small a value is zero, as a fraction of the series' largest magnitude:
/// the precision of a summary, five significant digits, so "it ends at zero"
/// holds for a series that decays from 100 to 1e-12. It decides a cited zero
/// and whether two runs end apart, and nothing else: a cited value that is
/// not zero is judged by [`VALUE_TOLERANCE`] of itself however small it is
/// beside the series, since a summary shows it to five digits of its own
/// ("starts at 50,000" where growth to 3.4e10 starts at 2 is no claim about
/// zero).
pub(crate) const VALUE_FLOOR: f64 = 1e-5;

use super::MAX_NAMED;

/// The most characters of an equation a failure quotes.
const MAX_QUOTE_CHARS: usize = 240;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct VerifyFindingsInput {
    /// At most 12.
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 12)))]
    pub findings: Vec<Finding>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Finding {
    pub kind: FindingKind,
    /// The claim, in a sentence or two, as the person will read it.
    pub claim: String,
    /// The evidence: at least one, at most 8, each naming something a tool
    /// reported.
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 8)))]
    pub citations: Vec<Citation>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Something wrong with the model.
    Flaw,
    /// Something the model does well.
    Strength,
    /// What the model does, neither.
    Observation,
}

/// A final value's relation between two runs.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Higher,
    Lower,
}

/// One piece of evidence. `run` is "current" (the model as it is) when
/// absent; a variable of a run may be an element (`Population[north]`).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "cites", rename_all = "snake_case", deny_unknown_fields)]
pub enum Citation {
    /// The model has the variable.
    Variable { variable: String },
    /// The variable's equation (a stock's: its initial value) is `equation`,
    /// written any way that parses the same: spacing, case and the spelling
    /// of names aside. For an arrayed variable, name an element it has
    /// (`Population[north]`) for that element's equation. A variable with a
    /// table is its table at its equation's value: `LOOKUP(effect, input)`,
    /// not `input`. A table with no equation of its own, which other
    /// equations look up, is cited with an empty equation.
    Equation { variable: String, equation: String },
    /// The variable (or one element of it) is within 5% of `value` at `time`
    /// in the run (of the larger of the two); a `value` of zero holds for a
    /// value under a hundred-thousandth of the series' largest. At the run's
    /// start when `time` is absent, which for a constant is its value. A
    /// time outside the run is refused.
    Value {
        variable: String,
        value: f64,
        #[serde(default)]
        time: Option<f64>,
        #[serde(default)]
        run: Option<String>,
    },
    /// The variables that read `variable` -- those whose equations read it in
    /// any phase, an initial value or `INIT` included, those that look it up
    /// as a table, a stock or flow whose option names it (a conveyor's
    /// transit time), and for a flow the stocks it fills and drains -- are
    /// exactly `readers`: none, when it is empty, for a variable nothing
    /// reads. It cannot hold while an equation of the model does not parse,
    /// since that equation may read it.
    Readers {
        variable: String,
        readers: Vec<String>,
    },
    /// `variable` reads `reads`, through a link of `polarity` (its equation's
    /// sign) when given.
    Reads {
        variable: String,
        reads: String,
        #[serde(default)]
        polarity: Option<LinkPolarityName>,
    },
    /// The model has the diagnostic of this id now.
    Diagnostic { id: String },
    /// The model has no diagnostic of `category`, or none at all.
    NoDiagnostics {
        #[serde(default)]
        category: Option<DiagnosticCategoryName>,
    },
    /// The run has the loop of this id, of `polarity` when given (a mostly
    /// reinforcing loop is reinforcing, a mostly balancing one balancing).
    Loop {
        id: String,
        #[serde(default)]
        polarity: Option<LoopPolarityName>,
        #[serde(default)]
        run: Option<String>,
    },
    /// The loop was its partition's strongest for most of the steps between
    /// `from` and `to` in which a loop was active.
    Leads {
        #[serde(rename = "loop")]
        loop_id: String,
        from: f64,
        to: f64,
        #[serde(default)]
        run: Option<String>,
    },
    /// No loop of the run goes through the variable.
    NoLoopThrough {
        variable: String,
        #[serde(default)]
        run: Option<String>,
    },
    /// The variable (an element of it, for an arrayed one) goes below zero
    /// in the run: exactly when its summary says when it went negative.
    GoesNegative {
        variable: String,
        #[serde(default)]
        run: Option<String>,
    },
    /// The variable peaks at `time`, within 5% of the run: it is at its
    /// largest there, and lower at the run's start and at its end. A series
    /// still rising when the run ends, or falling from its start, has no
    /// peak in the run.
    PeaksAt {
        variable: String,
        time: f64,
        #[serde(default)]
        run: Option<String>,
    },
    /// The variable ends within 5% of `value`, judged as `value` judges one.
    EndsNear {
        variable: String,
        value: f64,
        #[serde(default)]
        run: Option<String>,
    },
    /// The variable's behavior mode in the run is `mode`.
    BehaviorMode {
        variable: String,
        mode: ModeKind,
        #[serde(default)]
        run: Option<String>,
    },
    /// The variable ends higher (or lower) in `run` than in `than`.
    Compares {
        variable: String,
        relation: Relation,
        run: String,
        than: String,
    },
    /// The battery check of this id comes out `outcome`, run again when the
    /// model has changed since.
    Test { id: String, outcome: Outcome },
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct VerifyFindingsOutput {
    pub revision: u64,
    /// A verdict per finding, in the order given.
    pub findings: Vec<FindingVerdict>,
    /// The last findings, left out to keep the answer within its budget:
    /// they get no id and no verdict here; verify them in a call of their
    /// own.
    #[serde(skip_serializing_if = "super::is_zero")]
    pub omitted: usize,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct FindingVerdict {
    /// The finding's id (`F1`, ...) when every citation holds: what the host
    /// shows it under.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub holds: bool,
    /// The citations that do not hold, by their place in the finding (from
    /// 1), each with what is true instead.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<CitationFailure>,
    /// Failures left out, the last of the finding's, to keep the answer
    /// within its budget.
    #[serde(skip_serializing_if = "super::is_zero")]
    pub omitted_failures: usize,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct CitationFailure {
    pub citation: usize,
    pub reason: String,
}

pub(crate) fn verify_findings(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: VerifyFindingsInput,
) -> Result<VerifyFindingsOutput, ToolError> {
    if input.findings.is_empty() || input.findings.len() > MAX_FINDINGS {
        return Err(ToolError::new(format!(
            "verify between 1 and {MAX_FINDINGS} findings at a time (this call has {})",
            input.findings.len()
        )));
    }
    for (i, finding) in input.findings.iter().enumerate() {
        if finding.claim.trim().is_empty() {
            return Err(ToolError::new(format!(
                "finding {} has no claim: say it as the person will read it",
                i + 1
            )));
        }
        if finding.citations.is_empty() || finding.citations.len() > MAX_CITATIONS {
            return Err(ToolError::new(format!(
                "finding {} cites {} things; a finding cites between 1 and {MAX_CITATIONS}",
                i + 1,
                finding.citations.len()
            )));
        }
    }
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let mut findings = Vec::with_capacity(input.findings.len());
    for finding in &input.findings {
        let mut failures = Vec::new();
        for (i, citation) in finding.citations.iter().enumerate() {
            let checked = check(session, ws, &resolved, citation);
            // A citation whose runs or checks stopped for other work failed
            // only for that; the work still waits, so the call stops here.
            ws.yield_point()?;
            if let Err(reason) = checked {
                failures.push(CitationFailure {
                    citation: i + 1,
                    reason: super::evidence::window(&reason, 0, 0, super::evidence::QUOTE_CHARS),
                });
            }
        }
        let holds = failures.is_empty();
        findings.push(FindingVerdict {
            // An id is given once the answer is fitted, to what it shows.
            id: holds.then(|| PLACEHOLDER_ID.to_string()),
            holds,
            failures,
            omitted_failures: 0,
        });
    }
    let mut output = VerifyFindingsOutput {
        revision: ws.revision,
        findings,
        omitted: 0,
    };
    // A finding's last failures first, from the finding with the most; then
    // the last findings whole.
    super::fit(&mut output, session.outline_budget, |output| {
        let most = output
            .findings
            .iter_mut()
            .filter(|verdict| verdict.failures.len() > 1)
            .max_by_key(|verdict| verdict.failures.len());
        if let Some(verdict) = most {
            verdict.failures.pop();
            verdict.omitted_failures += 1;
        } else if output.findings.len() > 1 {
            output.findings.pop();
            output.omitted += 1;
        } else {
            return false;
        }
        true
    });
    for (verdict, finding) in output.findings.iter_mut().zip(&input.findings) {
        if verdict.holds {
            verdict.id = Some(session.evidence.finding_id(finding.kind, &finding.claim));
        }
    }
    Ok(output)
}

/// What a finding's id is counted as while the answer is fitted, before ids
/// are given: as long as any id a session gives up to its 9,999th finding.
const PLACEHOLDER_ID: &str = "F9999";

/// Whether `citation` holds; what is true instead when it does not.
fn check(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    resolved: &super::ResolvedModel<'_>,
    citation: &Citation,
) -> Result<(), String> {
    let model = resolved.model;
    match citation {
        Citation::Variable { variable } => variable_of(model, variable).map(|_| ()),
        Citation::Equation { variable, equation } => {
            let (label, actual) = equation_of(ws.project, model, variable)?;
            let Some(actual) = actual else {
                return if crate::variable::is_empty_or_sentinel(equation) {
                    Ok(())
                } else {
                    Err(format!(
                        "{label} is a table other equations look up, with no equation of its \
                         own: cite it with an empty equation"
                    ))
                };
            };
            let parse = |text: &str| {
                crate::ast::Expr0::new(text, crate::lexer::LexerType::Equation)
                    .ok()
                    .flatten()
                    .map(|expr| normal(&expr))
            };
            // An equation deep enough overflows the stack of what parses it,
            // so the cited one's depth is read from its tokens first.
            if let Some(too_deep) = super::input::equation_too_deep(equation) {
                return Err(too_deep);
            }
            let Some(cited) = parse(equation) else {
                return Err(format!(
                    "`{}` is not an equation the engine can read",
                    super::evidence::echo(equation)
                ));
            };
            if parse(&actual).as_ref() == Some(&cited) {
                Ok(())
            } else {
                Err(format!("{label}'s equation is `{}`", quoted(&actual)))
            }
        }
        Citation::Value {
            variable,
            value,
            time,
            run,
        } => {
            let run = run_of(session, ws, model, run.as_deref())?;
            let (label, values) = one_series(&run, ws.project, model, variable)?;
            let times = run.times();
            if let Some(time) = time {
                within_run(&run, &times, *time)?;
            }
            let at = time.unwrap_or_else(|| times.first().copied().unwrap_or(0.0));
            let row = run.row_at(at);
            let actual = values.get(row).copied().unwrap_or(f64::NAN);
            if near(actual, *value, &values) {
                Ok(())
            } else {
                Err(format!(
                    "{label} is {} at {} in run '{}'",
                    round(actual),
                    round(times.get(row).copied().unwrap_or(at)),
                    run.name
                ))
            }
        }
        Citation::Readers { variable, readers } => {
            let var = variable_of(model, variable)?;
            let ident = crate::canonicalize(var.get_ident()).into_owned();
            let cited: std::collections::BTreeSet<String> = readers
                .iter()
                .map(|name| {
                    variable_of(model, name)
                        .map(|v| crate::canonicalize(v.get_ident()).into_owned())
                })
                .collect::<Result<_, String>>()?;
            let actual: std::collections::BTreeSet<String> = crate::analysis::model_reads(
                &*ws.db,
                resolved.source_model,
                resolved.source_project,
            )
            .into_iter()
            .filter(|l| l.from == ident)
            .map(|l| l.to)
            .collect();
            // An equation that does not parse may read the variable, so no
            // set of readers is the whole set while the model has one.
            let unread = crate::analysis::model_unread_equations(&*ws.db, resolved.source_model);
            if !unread.is_empty() {
                let found = if actual.is_empty() {
                    format!("no reader of {} found", var.get_ident())
                } else {
                    format!("{} readers of {} found", actual.len(), var.get_ident())
                };
                let named: Vec<String> = unread
                    .iter()
                    .take(MAX_NAMED)
                    .map(|name| super::evidence::display_name(model, name))
                    .collect();
                return Err(format!(
                    "{found}, but {} equations could not be read ({}{}), and any of them may \
                     read it: repair them, then cite the readers",
                    unread.len(),
                    named.join(", "),
                    if unread.len() > MAX_NAMED {
                        ", ..."
                    } else {
                        ""
                    }
                ));
            }
            if actual == cited {
                return Ok(());
            }
            let names: Vec<String> = actual
                .iter()
                .take(MAX_NAMED)
                .map(|name| super::evidence::display_name(model, name))
                .collect();
            let more = actual.len().saturating_sub(MAX_NAMED);
            Err(if actual.is_empty() {
                format!("nothing reads {}", var.get_ident())
            } else {
                format!(
                    "{} is read by {}{}",
                    var.get_ident(),
                    names.join(", "),
                    if more > 0 {
                        format!(" and {more} more")
                    } else {
                        String::new()
                    }
                )
            })
        }
        Citation::Reads {
            variable,
            reads,
            polarity,
        } => {
            let to = variable_of(model, variable)?;
            let from = variable_of(model, reads)?;
            let (to_ident, from_ident) = (
                crate::canonicalize(to.get_ident()).into_owned(),
                crate::canonicalize(from.get_ident()).into_owned(),
            );
            let links = crate::analysis::model_reads(
                &*ws.db,
                resolved.source_model,
                resolved.source_project,
            );
            let Some(link) = links
                .iter()
                .find(|l| l.to == to_ident && l.from == from_ident)
            else {
                let inputs: Vec<String> = links
                    .iter()
                    .filter(|l| l.to == to_ident)
                    .map(|l| super::evidence::display_name(model, &l.from))
                    .collect();
                return Err(format!(
                    "{} does not read {}; it reads {}",
                    to.get_ident(),
                    from.get_ident(),
                    listing(&inputs)
                ));
            };
            let sign = LinkPolarityName::from(link.polarity);
            match polarity {
                Some(cited) if *cited != sign => Err(format!(
                    "the link from {} to {} is {} by its equation",
                    from.get_ident(),
                    to.get_ident(),
                    sign_name(sign)
                )),
                _ => Ok(()),
            }
        }
        Citation::Diagnostic { id } => {
            let reports = session.evidence.report_diagnostics(ws, resolved);
            if reports.iter().any(|d| d.id == *id) {
                Ok(())
            } else {
                let ids: Vec<String> = reports.iter().map(|d| d.id.clone()).collect();
                Err(format!(
                    "the model has no diagnostic {id} now; its diagnostics are {}",
                    listing(&ids)
                ))
            }
        }
        Citation::NoDiagnostics { category } => {
            let found: Vec<String> = session
                .evidence
                .report_diagnostics(ws, resolved)
                .into_iter()
                .filter(|d| category.is_none_or(|c| d.category == c))
                .map(|d| format!("{} ({})", d.id, d.code))
                .collect();
            if found.is_empty() {
                Ok(())
            } else {
                Err(format!("the model has {}", found.join(", ")))
            }
        }
        Citation::Loop { id, polarity, run } => {
            let key = loop_key(session, id)?;
            let run = run_of(session, ws, model, run.as_deref())?;
            let analysis = analysis_of(&mut session.runs, ws, model, resolved.source_model, &run)
                .map_err(|err| err.error)?;
            let Some(actual) = polarity_of(&analysis, &key) else {
                return Err(not_active(&analysis, &key, id, &run.name));
            };
            match polarity {
                Some(cited) if !polarity_matches(*cited, actual) => Err(format!(
                    "{id} is {} in run '{}'",
                    polarity_name(actual),
                    run.name
                )),
                _ => Ok(()),
            }
        }
        Citation::Leads {
            loop_id,
            from,
            to,
            run,
        } => {
            if !from.is_finite() || !to.is_finite() || from > to {
                return Err("a span's from is a time no later than its to".to_string());
            }
            let key = loop_key(session, loop_id)?;
            let run = run_of(session, ws, model, run.as_deref())?;
            let analysis = analysis_of(&mut session.runs, ws, model, resolved.source_model, &run)
                .map_err(|err| err.error)?;
            let Some(lead) = leadership(&analysis, &key, *from, *to) else {
                return Err(not_active(&analysis, &key, loop_id, &run.name));
            };
            if lead.active == 0 {
                return Err(format!(
                    "no loop was active between {from} and {to} in run '{}'",
                    run.name
                ));
            }
            if lead.leads() {
                return Ok(());
            }
            let strongest = lead
                .strongest
                .map(|key| session.evidence.loop_id(&key))
                .unwrap_or_default();
            Err(format!(
                "{loop_id} held {} of its partition's loop activity between {from} and {to} in \
                 run '{}'; {strongest} held the most, {}",
                rounded_share(lead.share),
                run.name,
                rounded_share(lead.largest)
            ))
        }
        Citation::NoLoopThrough { variable, run } => {
            let ident = through_of(ws.project, model, variable).map_err(|err| err.error)?;
            let run = run_of(session, ws, model, run.as_deref())?;
            let analysis = analysis_of(&mut session.runs, ws, model, resolved.source_model, &run)
                .map_err(|err| err.error)?;
            let through = loops_through(&analysis, &ident).ok_or_else(|| {
                format!(
                    "the analysis of run '{}' does not have every loop of the model, so no \
                     absence can be shown",
                    run.name
                )
            })?;
            if through.is_empty() {
                Ok(())
            } else {
                let ids: Vec<String> = through
                    .iter()
                    .map(|(key, inactive)| {
                        let id = session.evidence.loop_id(key);
                        match unreported(&analysis, key, &run.name).filter(|_| *inactive) {
                            Some(why) => format!("{id} ({why})"),
                            None => id,
                        }
                    })
                    .collect();
                Err(format!("{} goes through {variable}", ids.join(", ")))
            }
        }
        Citation::GoesNegative { variable, run } => {
            let run = run_of(session, ws, model, run.as_deref())?;
            let series = series_of(&run, ws.project, model, variable)?;
            // The summary's own rule, so a citation of what a summary
            // reported holds: a number reported negative went negative.
            let times = run.times();
            if series.iter().any(|series| {
                SeriesCore::at(&times, &series.values, 0.0)
                    .negative_from
                    .is_some()
            }) {
                return Ok(());
            }
            let least = series
                .iter()
                .flat_map(|series| series.values.iter().copied())
                .fold(f64::INFINITY, f64::min);
            Err(format!(
                "{variable} never goes below zero in run '{}'; its least value is {}",
                run.name,
                round(least)
            ))
        }
        Citation::PeaksAt {
            variable,
            time,
            run,
        } => {
            let run = run_of(session, ws, model, run.as_deref())?;
            let KeyedSeries { label, key, values } =
                one_keyed_series(&run, ws.project, model, variable)?;
            let times = run.times();
            within_run(&run, &times, *time)?;
            let scale = scale_in_run(&run.results, model, &run.plan, &key);
            let (at, value) = peak(&times, &values, scale).map_err(|why| {
                let name = &run.name;
                match why {
                    NoPeak::Still(value) => format!(
                        "{label} holds at {} throughout run '{name}', so it has no peak",
                        round(value)
                    ),
                    NoPeak::LargestAtTheEnd(value) => format!(
                        "{label} is at its largest ({}) where run '{name}' ends, so the run \
                         shows no peak",
                        round(value)
                    ),
                    NoPeak::LargestAtTheStart(value) => format!(
                        "{label} is at its largest ({}) where run '{name}' starts, so the run \
                         shows no peak",
                        round(value)
                    ),
                }
            })?;
            let horizon =
                times.last().copied().unwrap_or(0.0) - times.first().copied().unwrap_or(0.0);
            if (at - time).abs() <= PEAK_TOLERANCE * horizon {
                Ok(())
            } else {
                Err(format!(
                    "{label} peaks at {} ({}) in run '{}'",
                    round(at),
                    round(value),
                    run.name
                ))
            }
        }
        Citation::EndsNear {
            variable,
            value,
            run,
        } => {
            let run = run_of(session, ws, model, run.as_deref())?;
            let (label, values) = one_series(&run, ws.project, model, variable)?;
            let last = values.last().copied().unwrap_or(f64::NAN);
            if near(last, *value, &values) {
                Ok(())
            } else {
                Err(format!(
                    "{label} ends at {} in run '{}'",
                    round(last),
                    run.name
                ))
            }
        }
        Citation::BehaviorMode {
            variable,
            mode,
            run,
        } => {
            let run = run_of(session, ws, model, run.as_deref())?;
            let KeyedSeries { label, key, values } =
                one_keyed_series(&run, ws.project, model, variable)?;
            let scale = scale_in_run(&run.results, model, &run.plan, &key);
            let actual = classify_at(&run.times(), &values, scale).kind;
            if actual == *mode {
                Ok(())
            } else {
                Err(format!(
                    "{label}'s behavior in run '{}' is {}",
                    run.name,
                    mode_name(actual)
                ))
            }
        }
        Citation::Compares {
            variable,
            relation,
            run,
            than,
        } => {
            let this = run_of(session, ws, model, Some(run))?;
            let that = run_of(session, ws, model, Some(than))?;
            let (label, a_series) = one_series(&this, ws.project, model, variable)?;
            let (_, b_series) = one_series(&that, ws.project, model, variable)?;
            let (a, b) = (
                a_series.last().copied().unwrap_or(f64::NAN),
                b_series.last().copied().unwrap_or(f64::NAN),
            );
            let both: Vec<f64> = a_series.iter().chain(&b_series).copied().collect();
            let holds = apart(a, b, &both)
                && match relation {
                    Relation::Higher => a > b,
                    Relation::Lower => a < b,
                };
            if holds {
                Ok(())
            } else {
                Err(format!(
                    "{label} ends at {} in run '{}' and {} in run '{}'",
                    round(a),
                    this.name,
                    round(b),
                    that.name
                ))
            }
        }
        Citation::Test { id, outcome } => {
            let actual = recheck(session, ws, resolved, id)?;
            if actual == *outcome {
                Ok(())
            } else {
                Err(format!("{id} comes out {}", outcome_name(actual)))
            }
        }
    }
}

/// Why the loop `id`, whose cycle is `key`, is not one of `analysis`'s run
/// `run`: inactive in it, or no loop of it.
fn not_active(analysis: &LoopAnalysis, key: &[String], id: &str, run: &str) -> String {
    match unreported(analysis, key, run) {
        Some(why) => format!("{id} is {why}"),
        None => format!("{id} is not a loop of run '{run}'"),
    }
}

/// The largest magnitude among the numbers of `series`.
fn magnitude(series: &[f64]) -> f64 {
    series
        .iter()
        .filter(|v| v.is_finite())
        .fold(0.0_f64, |m, v| m.max(v.abs()))
}

/// Whether `actual` is `cited`, as a value citation judges it: within
/// [`VALUE_TOLERANCE`] of the larger of the two, which two values of
/// opposite signs never are; or, for a cited zero, within [`VALUE_FLOOR`] of
/// the largest magnitude in `series`.
fn near(actual: f64, cited: f64, series: &[f64]) -> bool {
    if cited == 0.0 {
        actual.abs() <= VALUE_FLOOR * magnitude(series)
    } else {
        (actual - cited).abs() <= VALUE_TOLERANCE * actual.abs().max(cited.abs())
    }
}

/// Whether two runs end apart, at `a` and at `b`: by more than
/// [`VALUE_TOLERANCE`] of the larger, and by more than [`VALUE_FLOOR`] of
/// the largest magnitude in `series`, both runs' values. Runs that end a
/// hair apart, or both at what a summary shows as zero, end alike.
fn apart(a: f64, b: f64, series: &[f64]) -> bool {
    let alike = (VALUE_TOLERANCE * a.abs().max(b.abs())).max(VALUE_FLOOR * magnitude(series));
    a.is_finite() && b.is_finite() && (a - b).abs() > alike
}

/// Why a series has no peak in its run, with the value it holds or is
/// largest at.
enum NoPeak {
    Still(f64),
    /// It is still at its largest when the run ends: growth, or a plateau it
    /// rose to.
    LargestAtTheEnd(f64),
    /// It is at its largest when the run starts, and never as large again.
    LargestAtTheStart(f64),
}

/// Where `values` peaks, and its value there: its largest value, when the
/// series is lower at both ends of the run by more than the noise a behavior
/// mode ignores ([`NOISE_FRACTION`] of its range).
///
/// A largest value at an end of the run is where the run stops looking, not
/// a turn of the series: "peaks at the stop time" of exponential growth
/// claims a decline no run shows.
fn peak(times: &[f64], values: &[f64], scale: f64) -> Result<(f64, f64), NoPeak> {
    let first = values.first().copied().unwrap_or(f64::NAN);
    if classify_at(times, values, scale).kind == ModeKind::AtRest {
        return Err(NoPeak::Still(first));
    }
    let (at, largest) =
        times
            .iter()
            .zip(values)
            .fold((f64::NAN, f64::NEG_INFINITY), |best, (&t, &v)| {
                if v > best.1 { (t, v) } else { best }
            });
    let least = values
        .iter()
        .copied()
        .filter(|v| v.is_finite())
        .fold(f64::INFINITY, f64::min);
    let noise = NOISE_FRACTION * (largest - least);
    let last = values.last().copied().unwrap_or(f64::NAN);
    if largest - last <= noise {
        Err(NoPeak::LargestAtTheEnd(largest))
    } else if largest - first <= noise {
        Err(NoPeak::LargestAtTheStart(largest))
    } else {
        Ok((at, largest))
    }
}

/// A time a citation names, refused with the run's span when the run does
/// not reach it.
fn within_run(run: &Run, times: &[f64], time: f64) -> Result<(), String> {
    let (Some(&start), Some(&stop)) = (times.first(), times.last()) else {
        return Err(format!("run '{}' saved no time", run.name));
    };
    let slack = 1e-9 * (stop - start).abs().max(1.0);
    if time.is_finite() && time >= start - slack && time <= stop + slack {
        Ok(())
    } else {
        Err(format!(
            "run '{}' goes from {} to {}, so it has no time {}",
            run.name,
            round(start),
            round(stop),
            crate::results::written(time)
        ))
    }
}

/// The variable `name` names, or a refusal naming the closest.
fn variable_of<'m>(
    model: &'m datamodel::Model,
    name: &str,
) -> Result<&'m datamodel::Variable, String> {
    names::resolve(model, name).map_err(|suggestions| {
        let name = super::evidence::echo(name);
        if suggestions.is_empty() {
            format!("the model has no variable '{name}'")
        } else {
            format!(
                "the model has no variable '{name}'; the closest are {}",
                suggestions.join(", ")
            )
        }
    })
}

/// The variable `name` names, and the element of it a subscript names
/// (`population[north]`), spelled as the project spells it: named as the
/// read tools name one (`names::split_subscript`, `names::resolve_element`).
fn named<'m>(
    project: &datamodel::Project,
    model: &'m datamodel::Model,
    name: &str,
) -> Result<(&'m datamodel::Variable, Option<String>), String> {
    if let Some(var) = model.get_variable(name) {
        return Ok((var, None));
    }
    let Some((base, subscripts)) = names::split_subscript(name) else {
        return variable_of(model, name).map(|var| (var, None));
    };
    let var = variable_of(model, base)?;
    let dims = var.get_equation().map(dimensions).unwrap_or_default();
    names::resolve_element(project, &dims, &subscripts)
        .map(|element| (var, Some(element)))
        .map_err(|why| {
            format!(
                "{} has no element [{}]: {why}",
                var.get_ident(),
                super::evidence::echo(&subscripts.join(", "))
            )
        })
}

/// The equation `variable` names -- a variable's, or one element's of an
/// arrayed one -- with the label a failure names it by; no equation for a
/// table other equations look up, which has none of its own.
fn equation_of(
    project: &datamodel::Project,
    model: &datamodel::Model,
    variable: &str,
) -> Result<(String, Option<String>), String> {
    let (var, element) = named(project, model, variable)?;
    let Some(equation) = var.get_equation() else {
        return Err(format!("{} has no equation", var.get_ident()));
    };
    let own_table = match var {
        datamodel::Variable::Aux(aux) => aux.gf.is_some(),
        datamodel::Variable::Flow(flow) => flow.gf.is_some(),
        datamodel::Variable::Stock(_) | datamodel::Variable::Module(_) => false,
    };
    let (text, table) = match (equation, &element) {
        (datamodel::Equation::Scalar(text), _) | (datamodel::Equation::ApplyToAll(_, text), _) => {
            (text.clone(), own_table)
        }
        (datamodel::Equation::Arrayed(_, elements, default, _), Some(element)) => {
            let key = CanonicalElementName::from_subscript(element);
            let arm = elements
                .iter()
                .find(|(arm, ..)| CanonicalElementName::from_subscript(arm) == key)
                .map(|(_, text, _, table)| (text.clone(), own_table || table.is_some()));
            arm.or_else(|| default.clone().map(|text| (text, own_table)))
                .ok_or_else(|| {
                    format!("{}[{element}] has no equation of its own", var.get_ident())
                })?
        }
        (datamodel::Equation::Arrayed(..), None) => {
            return Err(format!(
                "{} has an equation per element: name one, as {}[...]",
                var.get_ident(),
                var.get_ident()
            ));
        }
    };
    let label = match &element {
        Some(element) => format!("{}[{element}]", var.get_ident()),
        None => var.get_ident().to_string(),
    };
    if table && crate::variable::is_empty_or_sentinel(&text) {
        return Ok((label, None));
    }
    // A variable with a table is the table at its equation's value.
    let canonical = crate::canonicalize(var.get_ident()).into_owned();
    let text = match (table, &element) {
        (true, Some(element)) => format!("LOOKUP({canonical}[{element}], {text})"),
        (true, None) => format!("LOOKUP({canonical}, {text})"),
        (false, _) => text,
    };
    Ok((label, Some(text)))
}

/// An equation as it reads whatever its spacing, the case of its builtins
/// and the spelling of its names: what two equations that say the same thing
/// share.
fn normal(expr: &crate::ast::Expr0) -> String {
    use crate::ast::{BinaryOp, Expr0, IndexExpr0, UnaryOp};
    let index = |index: &IndexExpr0| match index {
        IndexExpr0::Wildcard(_) => "*".to_string(),
        IndexExpr0::StarRange(dim, _) => format!("*:{}", crate::canonicalize(dim.as_str())),
        IndexExpr0::Range(l, r, _) => format!("{}:{}", normal(l), normal(r)),
        IndexExpr0::DimPosition(n, _) => format!("@{n}"),
        IndexExpr0::Expr(e) => normal(e),
    };
    match expr {
        Expr0::Const(_, n, _) => format!("{}", n.value()),
        Expr0::Var(raw, _) => raw.canonicalize().as_str().to_string(),
        Expr0::App(crate::builtins::UntypedBuiltinFn(name, args), _) => format!(
            "{}({})",
            name.to_lowercase(),
            args.iter().map(normal).collect::<Vec<_>>().join(",")
        ),
        Expr0::Subscript(raw, indices, _) => format!(
            "{}[{}]",
            raw.canonicalize().as_str(),
            indices.iter().map(index).collect::<Vec<_>>().join(",")
        ),
        Expr0::Op1(op, inner, _) => {
            let op = match op {
                UnaryOp::Positive => "+",
                UnaryOp::Negative => "-",
                UnaryOp::Not => "not ",
                UnaryOp::Transpose => "'",
            };
            format!("({op}{})", normal(inner))
        }
        Expr0::Op2(op, l, r, _) => {
            let op = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Exp => "^",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "mod",
                BinaryOp::Gt => ">",
                BinaryOp::Lt => "<",
                BinaryOp::Gte => ">=",
                BinaryOp::Lte => "<=",
                BinaryOp::Eq => "=",
                BinaryOp::Neq => "<>",
                BinaryOp::And => "and",
                BinaryOp::Or => "or",
            };
            format!("({} {op} {})", normal(l), normal(r))
        }
        Expr0::If(c, t, f, _) => format!("if({},{},{})", normal(c), normal(t), normal(f)),
    }
}

/// `text` cut to [`MAX_QUOTE_CHARS`].
fn quoted(text: &str) -> String {
    if text.chars().count() <= MAX_QUOTE_CHARS {
        text.to_string()
    } else {
        let cut: String = text.chars().take(MAX_QUOTE_CHARS - 3).collect();
        format!("{cut}...")
    }
}

/// Names as a list in words, "none" for none.
fn listing(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

fn sign_name(sign: LinkPolarityName) -> &'static str {
    match sign {
        LinkPolarityName::Positive => "positive",
        LinkPolarityName::Negative => "negative",
        LinkPolarityName::Unknown => "of no sign the engine can tell",
    }
}

/// The cycle of the loop this session calls `id`.
fn loop_key(session: &Session, id: &str) -> Result<Vec<String>, String> {
    session
        .evidence
        .loop_key(id.trim())
        .map(<[String]>::to_vec)
        .ok_or_else(|| {
            format!("no loop has the id '{id}' in this session: loop ids come from analyze_loops")
        })
}

/// The run `name` names ("current" when absent), made at the current
/// revision.
fn run_of(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    name: Option<&str>,
) -> Result<std::sync::Arc<Run>, String> {
    let name = name.map(str::trim).unwrap_or(CURRENT);
    let run = session.runs.get(ws, model, name).map_err(|err| err.error)?;
    if !session.runs.is_fresh(ws, &run) {
        return Err(format!(
            "run '{name}' was made at revision {}, before the model changed; run it again with \
             run_experiment",
            run.revision
        ));
    }
    Ok(run)
}

/// Every series `variable` names in `run`: the variable's, or its every
/// element's for an arrayed one, or the one element a subscript names.
fn series_of(
    run: &Run,
    project: &datamodel::Project,
    model: &datamodel::Model,
    variable: &str,
) -> Result<Vec<KeyedSeries>, String> {
    let (var, element) = named(project, model, variable)?;
    let (series, _) =
        keyed_series_upto(run, model, var.get_ident(), element.as_deref(), usize::MAX);
    if series.is_empty() {
        return Err(format!(
            "{} has no series in run '{}'",
            var.get_ident(),
            run.name
        ));
    }
    Ok(series)
}

/// The one series `variable` names in `run`: a scalar's, or an element's.
fn one_series(
    run: &Run,
    project: &datamodel::Project,
    model: &datamodel::Model,
    variable: &str,
) -> Result<(String, Vec<f64>), String> {
    one_keyed_series(run, project, model, variable).map(|series| (series.label, series.values))
}

/// [`one_series`], with its results key.
fn one_keyed_series(
    run: &Run,
    project: &datamodel::Project,
    model: &datamodel::Model,
    variable: &str,
) -> Result<KeyedSeries, String> {
    let mut series = series_of(run, project, model, variable)?.into_iter();
    match (series.next(), series.next()) {
        (Some(one), None) => Ok(one),
        (Some(first), Some(_)) => Err(format!(
            "{variable} is arrayed: name one of its elements, as {}",
            first.label
        )),
        (None, _) => Err(format!("{variable} has no series in run '{}'", run.name)),
    }
}

/// Whether a loop of `actual` polarity is of the `cited` one: a mostly
/// reinforcing loop is reinforcing, a mostly balancing one balancing.
fn polarity_matches(cited: LoopPolarityName, actual: LoopPolarityName) -> bool {
    use LoopPolarityName::*;
    cited == actual
        || matches!(
            (cited, actual),
            (Reinforcing, MostlyReinforcing) | (Balancing, MostlyBalancing)
        )
}

/// How a report spells a value of a serialized enum.
fn spelled<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(|s| s.replace('_', " ")))
        .unwrap_or_default()
}

fn polarity_name(polarity: LoopPolarityName) -> String {
    spelled(polarity)
}

fn mode_name(mode: ModeKind) -> String {
    spelled(mode)
}

fn outcome_name(outcome: Outcome) -> String {
    spelled(outcome)
}

#[cfg(test)]
#[path = "verify_tests.rs"]
mod tests;
