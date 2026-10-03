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
//! Whether the claim follows from its citations is a judgment this does not
//! make: "the reinforcing loop drives the growth", citing only that the loop
//! exists, holds here. A run made before the model changed is not evidence
//! about the model: a citation of one fails, and names the repair.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::datamodel;

use super::battery::{Outcome, recheck};
use super::behavior::{ModeKind, classify_at};
use super::loops::{
    LoopPolarityName, analysis_of, leadership, loops_through, polarity_of, through_of,
};
use super::runs::{CURRENT, Run};
use super::series::{KeyedSeries, SeriesCore, round, scale_in_run};
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

/// The least difference a value may have from the cited one, as a fraction
/// of the series' largest magnitude: the precision of a summary, five
/// significant digits, so a value that reads as zero beside the series is
/// zero, and "it ends at zero" holds for a series that decays to 1e-12. It is
/// for values near zero: once either value is past 0.02% of the magnitude, 5%
/// of it is more, so the floor never lets one small value stand for another
/// on a series that spans orders of magnitude ("starts at 40" where exponential
/// growth to 136,420 starts at 1).
pub(crate) const VALUE_FLOOR: f64 = 1e-5;

/// The most readers a failed readers citation names.
const MAX_NAMED: usize = 12;

/// The most characters of an equation a failure quotes.
const MAX_QUOTE_CHARS: usize = 240;

/// The share of a span's active steps a loop leads to lead the span.
pub(crate) const LEAD_MAJORITY: f64 = 0.5;

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
    /// of names aside. For an arrayed variable, name an element
    /// (`Population[north]`) for that element's equation. A variable with a
    /// table is its table at its equation's value: `LOOKUP(effect, input)`,
    /// not `input`.
    Equation { variable: String, equation: String },
    /// The variable (or one element of it) is within 5% of `value` at `time`
    /// in the run, or so near zero beside the series that the difference
    /// reads as none: at the run's start when `time` is absent, which for a
    /// constant is its value. A time outside the run is refused.
    Value {
        variable: String,
        value: f64,
        #[serde(default)]
        time: Option<f64>,
        #[serde(default)]
        run: Option<String>,
    },
    /// The variables `variable` links to -- those whose equations read it,
    /// and for a flow the stocks it fills and drains -- are exactly
    /// `readers`: none, when it is empty, for a variable nothing reads.
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
    /// The variable is at its largest at `time`, within 5% of the run.
    PeaksAt {
        variable: String,
        time: f64,
        #[serde(default)]
        run: Option<String>,
    },
    /// The variable ends within 5% of `value`.
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
    for finding in input.findings {
        let mut failures = Vec::new();
        for (i, citation) in finding.citations.iter().enumerate() {
            let checked = check(session, ws, &resolved, citation);
            // A citation whose runs or checks stopped for other work failed
            // only for that; the work still waits, so the call stops here.
            ws.yield_point()?;
            if let Err(reason) = checked {
                failures.push(CitationFailure {
                    citation: i + 1,
                    reason,
                });
            }
        }
        let holds = failures.is_empty();
        findings.push(FindingVerdict {
            id: holds.then(|| session.evidence.finding_id(finding.kind, &finding.claim)),
            holds,
            failures,
        });
    }
    Ok(VerifyFindingsOutput {
        revision: ws.revision,
        findings,
    })
}

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
            let (label, actual) = equation_of(model, variable)?;
            let parse = |text: &str| {
                crate::ast::Expr0::new(text, crate::lexer::LexerType::Equation)
                    .ok()
                    .flatten()
                    .map(|expr| normal(&expr))
            };
            let Some(cited) = parse(equation) else {
                return Err(format!(
                    "`{equation}` is not an equation the engine can read"
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
            let (label, values) = one_series(&run, model, variable)?;
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
            let actual: std::collections::BTreeSet<String> = crate::analysis::model_links(
                &*ws.db,
                resolved.source_model,
                resolved.source_project,
                None,
                false,
            )
            .into_iter()
            .filter(|l| l.from == ident)
            .map(|l| l.to)
            .collect();
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
            let links = crate::analysis::model_links(
                &*ws.db,
                resolved.source_model,
                resolved.source_project,
                None,
                false,
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
                return Err(format!("{id} is not a loop of run '{}'", run.name));
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
            if !from.is_finite() || !to.is_finite() || from >= to {
                return Err("a span's from comes before its to".to_string());
            }
            let key = loop_key(session, loop_id)?;
            let run = run_of(session, ws, model, run.as_deref())?;
            let analysis = analysis_of(&mut session.runs, ws, model, resolved.source_model, &run)
                .map_err(|err| err.error)?;
            let Some(lead) = leadership(&analysis, &key, *from, *to) else {
                return Err(format!("{loop_id} is not a loop of run '{}'", run.name));
            };
            if lead.active == 0 {
                return Err(format!(
                    "no loop was active between {from} and {to} in run '{}'",
                    run.name
                ));
            }
            if lead.led as f64 >= LEAD_MAJORITY * lead.active as f64 {
                return Ok(());
            }
            let most = lead
                .most
                .map(|key| session.evidence.loop_id(&key))
                .unwrap_or_default();
            Err(format!(
                "{loop_id} led {} of the {} steps a loop was active between {from} and {to}; {most} \
                 led the most",
                lead.led, lead.active
            ))
        }
        Citation::NoLoopThrough { variable, run } => {
            let ident = through_of(ws.project, model, variable).map_err(|err| err.error)?;
            let run = run_of(session, ws, model, run.as_deref())?;
            let analysis = analysis_of(&mut session.runs, ws, model, resolved.source_model, &run)
                .map_err(|err| err.error)?;
            let through = loops_through(&analysis, &ident).ok_or_else(|| {
                format!(
                    "run '{}' has too many loops to list them all, so no absence can be shown",
                    run.name
                )
            })?;
            if through.is_empty() {
                Ok(())
            } else {
                let ids: Vec<String> = through
                    .iter()
                    .map(|key| session.evidence.loop_id(key))
                    .collect();
                Err(format!("{} goes through {variable}", ids.join(", ")))
            }
        }
        Citation::GoesNegative { variable, run } => {
            let run = run_of(session, ws, model, run.as_deref())?;
            let series = series_of(&run, model, variable)?;
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
            let KeyedSeries { label, key, values } = one_keyed_series(&run, model, variable)?;
            let times = run.times();
            within_run(&run, &times, *time)?;
            let scale = scale_in_run(&run.results, model, &run.plan, &key);
            if classify_at(&times, &values, scale).kind == ModeKind::AtRest {
                return Err(format!(
                    "{label} holds at {} throughout run '{}', so it has no peak",
                    round(values.first().copied().unwrap_or(f64::NAN)),
                    run.name
                ));
            }
            let (i, peak) =
                values
                    .iter()
                    .enumerate()
                    .fold((0, f64::NEG_INFINITY), |best, (i, &v)| {
                        if v > best.1 { (i, v) } else { best }
                    });
            let horizon =
                times.last().copied().unwrap_or(0.0) - times.first().copied().unwrap_or(0.0);
            if (times[i] - time).abs() <= PEAK_TOLERANCE * horizon {
                Ok(())
            } else {
                Err(format!(
                    "{label} peaks at {} ({}) in run '{}'",
                    round(times[i]),
                    round(peak),
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
            let (label, values) = one_series(&run, model, variable)?;
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
            let KeyedSeries { label, key, values } = one_keyed_series(&run, model, variable)?;
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
            let (label, a_series) = one_series(&this, model, variable)?;
            let (_, b_series) = one_series(&that, model, variable)?;
            let (a, b) = (
                a_series.last().copied().unwrap_or(f64::NAN),
                b_series.last().copied().unwrap_or(f64::NAN),
            );
            // A difference a value citation would call no difference is none
            // a person would see.
            let both: Vec<f64> = a_series.iter().chain(&b_series).copied().collect();
            let apart = a.is_finite() && b.is_finite() && !near(a, b, &both);
            let holds = apart
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

/// Whether `actual` is `cited`, as a value citation judges it: within
/// [`VALUE_TOLERANCE`] of the larger of the two, or, for two values near
/// zero, within [`VALUE_FLOOR`] of the largest magnitude in `series`.
fn near(actual: f64, cited: f64, series: &[f64]) -> bool {
    let magnitude = series
        .iter()
        .filter(|v| v.is_finite())
        .fold(0.0_f64, |m, v| m.max(v.abs()));
    let allowed = (VALUE_TOLERANCE * actual.abs().max(cited.abs())).max(VALUE_FLOOR * magnitude);
    (actual - cited).abs() <= allowed
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
            "run '{}' goes from {} to {}, so it has no time {time}",
            run.name,
            round(start),
            round(stop)
        ))
    }
}

/// The variable `name` names, or a refusal naming the closest.
fn variable_of<'m>(
    model: &'m datamodel::Model,
    name: &str,
) -> Result<&'m datamodel::Variable, String> {
    names::resolve(model, name).map_err(|suggestions| {
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

/// The equation `variable` names -- a variable's, or one element's of an
/// arrayed one -- with the label a failure names it by.
fn equation_of(model: &datamodel::Model, variable: &str) -> Result<(String, String), String> {
    let (name, subscript) = match variable.split_once('[') {
        Some((name, rest)) => (name.trim(), Some(rest.trim_end_matches(']'))),
        None => (variable.trim(), None),
    };
    let var = variable_of(model, name)?;
    let Some(equation) = var.get_equation() else {
        return Err(format!("{} has no equation", var.get_ident()));
    };
    // A variable with a table is the table at its equation's value.
    let canonical = crate::canonicalize(var.get_ident()).into_owned();
    let own_table = match var {
        datamodel::Variable::Aux(aux) => aux.gf.is_some(),
        datamodel::Variable::Flow(flow) => flow.gf.is_some(),
        _ => false,
    };
    let at_table = |table: bool, subscript: Option<&str>, text: String| match (table, subscript) {
        (true, Some(subscript)) => format!("LOOKUP({canonical}[{subscript}], {text})"),
        (true, None) => format!("LOOKUP({canonical}, {text})"),
        (false, _) => text,
    };
    match (equation, subscript) {
        (datamodel::Equation::Scalar(text), None) => Ok((
            var.get_ident().to_string(),
            at_table(own_table, None, text.clone()),
        )),
        (datamodel::Equation::ApplyToAll(_, text), _) => Ok((
            variable.trim().to_string(),
            at_table(own_table, subscript, text.clone()),
        )),
        (datamodel::Equation::Arrayed(_, elements, default, _), Some(subscript)) => {
            let wanted = super::series::element_key(subscript);
            elements
                .iter()
                .find(|(element, ..)| super::series::element_key(element) == wanted)
                .map(|(_, text, _, gf)| {
                    at_table(own_table || gf.is_some(), Some(subscript), text.clone())
                })
                .or_else(|| {
                    default
                        .clone()
                        .map(|text| at_table(own_table, Some(subscript), text))
                })
                .map(|text| (format!("{}[{subscript}]", var.get_ident()), text))
                .ok_or_else(|| format!("{} has no element [{subscript}]", var.get_ident()))
        }
        (datamodel::Equation::Arrayed(..), None) => Err(format!(
            "{} has an equation per element: name one, as {}[...]",
            var.get_ident(),
            var.get_ident()
        )),
        (datamodel::Equation::Scalar(_), Some(subscript)) => Err(format!(
            "{} is not arrayed, so it has no element [{subscript}]",
            var.get_ident()
        )),
    }
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
    model: &datamodel::Model,
    variable: &str,
) -> Result<Vec<KeyedSeries>, String> {
    let (name, subscript) = match variable.split_once('[') {
        Some((name, rest)) => (name.trim(), Some(rest.trim_end_matches(']'))),
        None => (variable.trim(), None),
    };
    let var = variable_of(model, name)?;
    let ident = crate::canonicalize(var.get_ident()).into_owned();
    let offsets = &run.results.offsets;
    let mut keys: Vec<(&str, usize)> = offsets
        .iter()
        .filter(|(key, _)| {
            super::series::column_of(model, key.as_str()).map(|(owner, _)| owner)
                == Some(ident.as_str())
        })
        .map(|(key, &offset)| (key.as_str(), offset))
        .collect();
    keys.sort_by_key(|(_, offset)| *offset);
    if let Some(subscript) = subscript {
        let wanted = format!("{ident}[{}]", super::series::element_key(subscript));
        keys.retain(|(key, _)| *key == wanted);
        if keys.is_empty() {
            return Err(format!("{} has no element [{subscript}]", var.get_ident()));
        }
    }
    if keys.is_empty() {
        return Err(format!(
            "{} has no series in run '{}'",
            var.get_ident(),
            run.name
        ));
    }
    Ok(keys
        .into_iter()
        .map(|(key, offset)| KeyedSeries {
            label: format!("{}{}", var.get_ident(), &key[ident.len()..]),
            key: key.to_string(),
            values: run.series(offset),
        })
        .collect())
}

/// The one series `variable` names in `run`: a scalar's, or an element's.
fn one_series(
    run: &Run,
    model: &datamodel::Model,
    variable: &str,
) -> Result<(String, Vec<f64>), String> {
    one_keyed_series(run, model, variable).map(|series| (series.label, series.values))
}

/// [`one_series`], with its results key.
fn one_keyed_series(
    run: &Run,
    model: &datamodel::Model,
    variable: &str,
) -> Result<KeyedSeries, String> {
    let mut series = series_of(run, model, variable)?;
    if series.len() > 1 {
        return Err(format!(
            "{variable} is arrayed: name one of its elements, as {}",
            series[0].label
        ));
    }
    Ok(series.remove(0))
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
