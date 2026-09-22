// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Series summaries: what a variable did in a run, bounded -- its start and
//! end, its extremes with their times, its turning points, when it first went
//! negative, its behavior mode, and a dozen samples -- never the series itself.
//! `read_behavior` answers with them, and experiments and `read_variables`
//! carry their core.
//!
//! An arrayed variable is summarized element by element, up to
//! [`MAX_ELEMENTS`] of them; any one element is read by naming it
//! (`population[north]`). Numbers are rounded to [`SIGNIFICANT_DIGITS`]: a
//! summary is for reading, and the digits past that are noise to a reader and
//! cost to an agent.
//!
//! An answer keeps to the session's byte budget. What does not fit is left
//! out in this order, and said: the samples, then the turning points, then
//! elements past the first of each arrayed variable, then whole variables from
//! the end.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::common::{Canonical, Ident};
use crate::datamodel;

use super::behavior::{BehaviorMode, shape};
use super::evidence::display_name;
use super::runs::{CURRENT, Run};
use super::variables::NotFound;
use super::{Session, ToolError, Workspace, names, resolve_model};

/// The most elements of one arrayed variable a summary lists.
pub(crate) const MAX_ELEMENTS: usize = 8;
/// The most turning points a summary lists.
pub(crate) const MAX_TURNS: usize = 6;
/// How many evenly spaced samples `read_behavior` gives.
pub(crate) const SAMPLES: usize = 12;
/// The most variables and runs one `read_behavior` call reads.
pub(crate) const MAX_BEHAVIOR_VARIABLES: usize = 12;
pub(crate) const MAX_BEHAVIOR_RUNS: usize = 4;
/// The digits a summary's numbers keep.
pub(crate) const SIGNIFICANT_DIGITS: i32 = 5;

/// `x` rounded to [`SIGNIFICANT_DIGITS`] significant digits.
pub(crate) fn round(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let scale = 10f64.powi(SIGNIFICANT_DIGITS - 1 - x.abs().log10().floor() as i32);
    if !scale.is_finite() || scale == 0.0 {
        return x;
    }
    (x * scale).round() / scale
}

/// A value at a time.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Point {
    pub time: f64,
    pub value: f64,
}

impl Point {
    /// The point at `i`; `None` when its value is not a number, which a
    /// summary leaves out.
    fn at(times: &[f64], values: &[f64], i: usize) -> Option<Point> {
        values[i].is_finite().then(|| Point {
            time: round(times[i]),
            value: round(values[i]),
        })
    }
}

/// What a series did, in brief. A value that is not a number (a division by
/// zero, an overflow) is left out, as are a start, an end, a sample or a
/// turning point that is not one; the mode says when the series went
/// undefined.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SeriesCore {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<f64>,
    /// Its least and greatest values that are numbers, with their times.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<Point>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<Point>,
    /// When it first went below zero, if it ever did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub negative_from: Option<f64>,
    pub mode: BehaviorMode,
}

impl SeriesCore {
    pub(crate) fn of(times: &[f64], values: &[f64]) -> SeriesCore {
        summarize(times, values, false).0
    }
}

/// A variable's (or one element's) behavior in a run.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SeriesSummary {
    /// The variable, with its element for an arrayed one (`population[north]`).
    pub variable: String,
    pub run: String,
    #[serde(flatten)]
    pub core: SeriesCore,
    /// Its turning points, the peaks and troughs it reversed from, in order;
    /// at most six.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub turns: Vec<Point>,
    /// About a dozen evenly spaced values, first and last included; left out
    /// when the answer is over its budget.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<Point>,
}

/// The core, and the turning points and samples when asked for.
fn summarize(times: &[f64], values: &[f64], full: bool) -> (SeriesCore, Vec<Point>, Vec<Point>) {
    let n = values.len();
    if n == 0 {
        return (
            SeriesCore {
                start: None,
                end: None,
                min: None,
                max: None,
                negative_from: None,
                mode: super::behavior::classify(times, values),
            },
            vec![],
            vec![],
        );
    }
    let finite = |i: &usize| values[*i].is_finite();
    let argmin = (0..n)
        .filter(finite)
        .min_by(|&a, &b| values[a].total_cmp(&values[b]));
    let argmax = (0..n)
        .filter(finite)
        .max_by(|&a, &b| values[a].total_cmp(&values[b]).then(b.cmp(&a)));
    let magnitude = values
        .iter()
        .filter(|v| v.is_finite())
        .fold(1.0f64, |m, v| m.max(v.abs()));
    // Below zero by more than rounding: a stock that ends an equilibrium at
    // -1e-15 has not gone negative.
    let negative_from = values
        .iter()
        .position(|&v| v < -1e-9 * magnitude)
        .map(|i| round(times[i]));
    let shape = shape(times, values);
    let number = |v: f64| v.is_finite().then(|| round(v));
    let core = SeriesCore {
        start: number(values[0]),
        end: number(values[n - 1]),
        min: argmin.and_then(|i| Point::at(times, values, i)),
        max: argmax.and_then(|i| Point::at(times, values, i)),
        negative_from,
        mode: rounded(shape.mode),
    };
    if !full {
        return (core, vec![], vec![]);
    }
    let turns = shape
        .turns
        .iter()
        .take(MAX_TURNS)
        .filter_map(|&i| Point::at(times, values, i))
        .collect();
    let count = SAMPLES.min(n);
    let samples = (0..count)
        .filter_map(|k| {
            let i = if count == 1 {
                0
            } else {
                k * (n - 1) / (count - 1)
            };
            Point::at(times, values, i)
        })
        .collect();
    (core, turns, samples)
}

fn rounded(mode: BehaviorMode) -> BehaviorMode {
    BehaviorMode {
        starts_at: mode.starts_at.map(round),
        settles_at: mode.settles_at.map(round),
        ..mode
    }
}

/// A variable's series in a run: one for a scalar, one per element for an
/// arrayed variable (at most [`MAX_ELEMENTS`]), each with the label a summary
/// names it by; and how many elements were left out.
pub(crate) fn element_series(
    run: &Run,
    model: &datamodel::Model,
    variable: &str,
) -> (Vec<(String, Vec<f64>)>, usize) {
    element_series_upto(run, model, variable, None, MAX_ELEMENTS)
}

/// [`element_series`] for one element, when `element` names one (as
/// `names::resolve_element` spells it), and for at most `limit` elements.
pub(crate) fn element_series_upto(
    run: &Run,
    model: &datamodel::Model,
    variable: &str,
    element: Option<&str>,
    limit: usize,
) -> (Vec<(String, Vec<f64>)>, usize) {
    let canonical = crate::canonicalize(variable).into_owned();
    let display = display_name(model, &canonical);
    let offsets = &run.results.offsets;
    if let Some(element) = element {
        let key = format!("{canonical}[{}]", element_key(element));
        return match offsets.get(&Ident::<Canonical>::new(&key)) {
            Some(&offset) => (
                vec![(format!("{display}[{element}]"), run.series(offset))],
                0,
            ),
            None => (vec![], 0),
        };
    }
    if let Some(&offset) = offsets.get(&Ident::<Canonical>::new(&canonical)) {
        return (vec![(display, run.series(offset))], 0);
    }
    let prefix = format!("{canonical}[");
    let mut elements: Vec<(&Ident<Canonical>, usize)> = offsets
        .iter()
        .filter(|(key, _)| key.as_str().starts_with(&prefix))
        .map(|(key, &offset)| (key, offset))
        .collect();
    elements.sort_by_key(|(_, offset)| *offset);
    let omitted = elements.len().saturating_sub(limit);
    let series = elements
        .into_iter()
        .take(limit)
        .map(|(key, offset)| {
            let label = format!("{display}{}", &key.as_str()[canonical.len()..]);
            (label, run.series(offset))
        })
        .collect();
    (series, omitted)
}

/// An element's results key, from its name as the project spells it: each
/// subscript canonicalized, joined with commas.
pub(crate) fn element_key(element: &str) -> String {
    element
        .split(',')
        .map(|part| crate::canonicalize(part.trim()).into_owned())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReadBehaviorInput {
    /// The variables to summarize, by name, or one element of an arrayed
    /// variable (`population[north]`); at most 12.
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 12)))]
    pub variables: Vec<String>,
    /// The runs to summarize them in, by name: "current" for the model as it
    /// is, or runs an experiment made. "current" alone when absent; at most 4.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 4)))]
    pub runs: Vec<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ReadBehaviorOutput {
    pub revision: u64,
    /// One summary per variable (or element) and run, variable by variable.
    pub series: Vec<SeriesSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_found: Vec<NotFound>,
    /// Arrayed variables with more elements than a summary lists, and how
    /// many were left out.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub omitted_elements: Vec<OmittedElements>,
    /// Runs made before the model last changed (its diagrams aside): what
    /// they show is what the model did then.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stale_runs: Vec<StaleRun>,
    /// What was left out to keep the answer within its budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub left_out: Option<LeftOut>,
}

/// What a `read_behavior` answer left out to fit its budget, in the order it
/// leaves things out.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct LeftOut {
    /// Every summary's samples.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub samples: bool,
    /// Every summary's turning points.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub turns: bool,
    /// Variables left out whole, to read in another call.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct OmittedElements {
    pub variable: String,
    pub count: usize,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct StaleRun {
    pub run: String,
    pub revision: u64,
}

/// Answer `read_behavior`.
pub(crate) fn read_behavior(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: ReadBehaviorInput,
) -> Result<ReadBehaviorOutput, ToolError> {
    if input.variables.is_empty() || input.variables.len() > MAX_BEHAVIOR_VARIABLES {
        return Err(ToolError::new(format!(
            "name between 1 and {MAX_BEHAVIOR_VARIABLES} variables (this call names {})",
            input.variables.len()
        )));
    }
    if input.runs.len() > MAX_BEHAVIOR_RUNS {
        return Err(ToolError::new(format!(
            "at most {MAX_BEHAVIOR_RUNS} runs per call (this call names {})",
            input.runs.len()
        )));
    }
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let model = resolved.model;
    let run_names = if input.runs.is_empty() {
        vec![CURRENT.to_string()]
    } else {
        input.runs.clone()
    };
    let mut runs = Vec::new();
    for name in &run_names {
        runs.push(session.runs.get(ws, model, name.trim())?);
    }

    let mut not_found = Vec::new();
    let mut variables: Vec<(&datamodel::Variable, Option<String>)> = Vec::new();
    for name in &input.variables {
        match resolve_series(ws.project, model, name) {
            Ok((var, element)) => {
                if !variables
                    .iter()
                    .any(|(v, e)| std::ptr::eq(*v, var) && *e == element)
                {
                    variables.push((var, element));
                }
            }
            Err(suggestions) => not_found.push(NotFound {
                name: name.clone(),
                suggestions,
            }),
        }
    }
    let mut stale_runs = Vec::new();
    for run in &runs {
        if !session.runs.is_fresh(ws, run) {
            stale_runs.push(StaleRun {
                run: run.name.clone(),
                revision: run.revision,
            });
        }
    }

    let answer = |detail: Detail, count: usize| {
        let mut series = Vec::new();
        let mut omitted_elements = Vec::new();
        for (var, element) in &variables[..count] {
            for run in &runs {
                let (elements, omitted) = element_series_upto(
                    run,
                    model,
                    var.get_ident(),
                    element.as_deref(),
                    detail.elements,
                );
                if omitted > 0 && run.name == runs[0].name {
                    omitted_elements.push(OmittedElements {
                        variable: var.get_ident().to_string(),
                        count: omitted,
                    });
                }
                let times = run.times();
                for (label, values) in elements {
                    let (core, turns, samples) = summarize(&times, &values, true);
                    series.push(SeriesSummary {
                        variable: label,
                        run: run.name.clone(),
                        core,
                        turns: if detail.turns { turns } else { vec![] },
                        samples: if detail.samples { samples } else { vec![] },
                    });
                }
            }
        }
        let left = LeftOut {
            samples: !detail.samples,
            turns: !detail.turns,
            variables: variables[count..]
                .iter()
                .map(|(var, element)| match element {
                    Some(element) => format!("{}[{element}]", var.get_ident()),
                    None => var.get_ident().to_string(),
                })
                .collect(),
        };
        let any_left = left.samples || left.turns || !left.variables.is_empty();
        ReadBehaviorOutput {
            revision: ws.revision,
            series,
            not_found: not_found.clone(),
            omitted_elements,
            stale_runs: stale_runs.clone(),
            left_out: any_left.then_some(left),
        }
    };
    let fits = |output: &ReadBehaviorOutput| {
        serde_json::to_string(output)
            .expect("summaries serialize")
            .len()
            <= session.outline_budget
    };
    let all = variables.len();
    let mut attempts: Vec<(Detail, usize)> = vec![
        (Detail::FULL, all),
        (
            Detail {
                samples: false,
                ..Detail::FULL
            },
            all,
        ),
        (
            Detail {
                samples: false,
                turns: false,
                ..Detail::FULL
            },
            all,
        ),
    ];
    for elements in [4, 2, 1] {
        attempts.push((
            Detail {
                samples: false,
                turns: false,
                elements,
            },
            all,
        ));
    }
    for count in (1..all).rev() {
        attempts.push((
            Detail {
                samples: false,
                turns: false,
                elements: 1,
            },
            count,
        ));
    }
    let mut output = None;
    for (detail, count) in attempts {
        let candidate = answer(detail, count);
        let done = fits(&candidate);
        output = Some(candidate);
        if done {
            break;
        }
    }
    Ok(output.unwrap_or_else(|| answer(Detail::FULL, all)))
}

/// How much of each summary an answer carries.
#[derive(Clone, Copy)]
struct Detail {
    samples: bool,
    turns: bool,
    /// The most elements of an arrayed variable it lists.
    elements: usize,
}

impl Detail {
    const FULL: Detail = Detail {
        samples: true,
        turns: true,
        elements: MAX_ELEMENTS,
    };
}

/// What `name` summarizes: a variable, or with a subscript one element of an
/// arrayed variable; the closest names when it names neither.
fn resolve_series<'a>(
    project: &datamodel::Project,
    model: &'a datamodel::Model,
    name: &str,
) -> Result<(&'a datamodel::Variable, Option<String>), Vec<String>> {
    if let Some(var) = model.get_variable(name) {
        return Ok((var, None));
    }
    let Some((base, subscripts)) = names::split_subscript(name) else {
        return names::resolve(model, name).map(|var| (var, None));
    };
    let var = names::resolve(model, base)?;
    let dims = var
        .get_equation()
        .map(super::outline::dimensions)
        .unwrap_or_default();
    names::resolve_element(project, &dims, &subscripts)
        .map(|element| (var, Some(element)))
        .map_err(|reason| vec![format!("{}: {reason}", var.get_ident())])
}

#[cfg(test)]
#[path = "series_tests.rs"]
mod tests;
