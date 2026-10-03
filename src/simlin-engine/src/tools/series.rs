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

use super::behavior::{BehaviorMode, magnitude, shape_at};
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

/// `x` rounded to [`SIGNIFICANT_DIGITS`] significant digits: the number that
/// prints with those digits and no more.
///
/// It is rounded as a decimal, by writing it with those digits and reading
/// that back, so the answer is the float nearest the decimal at any
/// magnitude. Scaling by a power of ten and back is not: `1.6e9` scaled by
/// `1e-5`, which no float holds exactly, comes back `1599999999.9999998`.
pub(crate) fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let digits = (SIGNIFICANT_DIGITS - 1) as usize;
    // What Rust writes for a finite float it reads back.
    format!("{x:.digits$e}").parse().unwrap_or(x)
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

/// What a series did, in brief. A value that is not a number (a division by
/// zero, an overflow) is left out, as are a start, an end, a sample or a
/// turning point that is not one; the mode says when the series went
/// undefined.
///
/// Every number it reports is the series' own, rounded to five significant
/// digits, so every tool that reports a number of a series reports the same
/// one. Its parts agree with each other: a series that is not at rest
/// reports a least and a greatest value that read differently, and it went
/// negative exactly when a number reported of it is negative. A series that
/// is the residue of quantities that cancel (`behavior::classify_at`) is at
/// rest, and reports the residue it holds.
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
    /// The core of a series read at `scale` ([`scale_in_run`] is a
    /// variable's in a run; zero is the series' own).
    pub(crate) fn at(times: &[f64], values: &[f64], scale: f64) -> SeriesCore {
        summarize(times, values, scale, false).0
    }
}

/// The scale the series under results key `key` (a variable, or one
/// element of one: `gap[north]`) is read at in a run
/// (`behavior::classify_at`): the magnitude of the quantities its values are
/// sums and differences of, which is the scale their arithmetic residue is
/// small beside. Zero where nothing says: the series is then read at its own.
///
/// The equations are the run's: `model`'s with the run's `plan` applied
/// (`runs::apply_equations`), so a replaced equation's terms are the
/// replacement's, and a table it drops is gone.
///
/// - A stock is its flows added up over the run, so its scale is the largest
///   scale any of its flows has times the run's horizon: what the flow
///   reaches, or its own scale as a sum where that is larger, so a stock
///   that integrates residue (`error = target - measurement`) is at the
///   scale of the terms that cancelled. That is one level: a flow's scale
///   goes no further back than its own equation.
/// - A flow or an auxiliary whose equation is, at its top level, a sum or a
///   difference (`orders - fulfilled`) is at the scale of its terms: the
///   magnitude each term that is a variable or a number reaches. A term that
///   is anything else is left out, which can only make the scale smaller and
///   the series read as moving.
///
/// An element's scale is its own: what a term reaches at the same element,
/// at the element a subscript names, or as a scalar; a term of another shape
/// is left out. The magnitude one element's flows reach says nothing of
/// another element's residue.
///
/// What a variable is a product or a quotient of says nothing of its scale
/// (a fraction of two large numbers is small on purpose), so nothing else
/// has one. Residue that reaches a series through anything but a sum -- a
/// quotient (`gap / 4` feeding a stock), a function (`SMTH1(gap, 2)`,
/// `MAX(0, gap)`) -- is therefore read as the movement its numbers show, and
/// `analyze_loops` can then report a share for a loop whose stocks move only
/// by rounding.
pub(crate) fn scale_in_run(
    results: &crate::results::Results,
    model: &datamodel::Model,
    plan: &super::runs::RunPlan,
    key: &str,
) -> f64 {
    let as_run;
    let model = if plan.equations.is_empty() {
        model
    } else {
        let mut replaced = model.clone();
        // A plan that ran had every variable it replaces.
        if super::runs::apply_equations(&mut replaced, plan).is_err() {
            unreachable!("a run's plan replaces variables of its model");
        }
        as_run = replaced;
        &as_run
    };
    let Some((base, element)) = column_of(model, key) else {
        return 0.0;
    };
    let Some(var) = model.get_variable(base) else {
        return 0.0;
    };
    // The largest magnitude a series of the run reaches.
    let series = |key: &str| -> Option<f64> {
        let &offset = results.offsets.get(&Ident::<Canonical>::new(key))?;
        Some(
            results
                .iter()
                .map(|row| row[offset])
                .filter(|v| v.is_finite())
                .fold(0.0, |m: f64, v| m.max(v.abs())),
        )
    };
    // What a term naming `name` reaches where this series is read: at the
    // element `pinned` names, else at this series' element, else as a
    // scalar.
    let reaches = |name: &str, pinned: Option<&str>| -> f64 {
        let canonical = crate::canonicalize(name).into_owned();
        pinned
            .and_then(|pinned| series(&format!("{canonical}[{pinned}]")))
            .or_else(|| element.and_then(|element| series(&format!("{canonical}[{element}]"))))
            .or_else(|| series(&canonical))
            .unwrap_or(0.0)
    };
    // A flow's or an auxiliary's scale as a sum, at this series' element.
    let as_a_sum = |var: &datamodel::Variable| -> f64 {
        if !matches!(
            var,
            datamodel::Variable::Flow(_) | datamodel::Variable::Aux(_)
        ) {
            return 0.0;
        }
        let text = match var.get_equation() {
            Some(datamodel::Equation::Scalar(text))
            | Some(datamodel::Equation::ApplyToAll(_, text)) => Some(text.as_str()),
            // The element's own equation, else the default the others
            // take.
            Some(datamodel::Equation::Arrayed(_, elements, default, _)) => elements
                .iter()
                .find(|(name, ..)| Some(element_key(name).as_str()) == element)
                .map(|(_, text, ..)| text.as_str())
                .or(default.as_deref()),
            None => None,
        };
        text.and_then(|text| {
            crate::ast::Expr0::new(text, crate::lexer::LexerType::Equation)
                .ok()
                .flatten()
        })
        .map_or(0.0, |equation| scale_of_a_sum(&equation, &reaches))
    };
    match var {
        datamodel::Variable::Stock(stock) => {
            let horizon = results.specs.stop - results.specs.start;
            stock
                .inflows
                .iter()
                .chain(&stock.outflows)
                .map(|flow| {
                    let own = model.get_variable(flow).map_or(0.0, as_a_sum);
                    reaches(flow, None).max(own) * horizon
                })
                .fold(0.0, f64::max)
        }
        datamodel::Variable::Flow(_) | datamodel::Variable::Aux(_) => as_a_sum(var),
        datamodel::Variable::Module(_) => 0.0,
    }
}

/// The canonical idents of `model`'s variables: what a results column is
/// read as belonging to ([`crate::save_check::column_variable`]).
pub(crate) fn declared(model: &datamodel::Model) -> std::collections::BTreeSet<String> {
    model
        .variables
        .iter()
        .map(|var| crate::canonicalize(var.get_ident()).into_owned())
        .collect()
}

/// The variable a results column `key` belongs to among `model`'s, and the
/// element key it names (`a1,b2` of `x[a1,b2]`), if any.
pub(crate) fn column_of<'k>(
    model: &datamodel::Model,
    key: &'k str,
) -> Option<(&'k str, Option<&'k str>)> {
    let base = crate::save_check::column_variable(key, &declared(model))?;
    let element = key[base.len()..]
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'));
    Some((base, element))
}

/// The scales the series under results key `key` is read at in two runs that
/// are compared, `this` and `that` (each a run and the series' values there):
/// each run's own [`scale_in_run`], with the larger of the two series'
/// magnitudes shared between them. Residue in one run beside a movement in
/// the other is at rest, not a movement of its own, and a change of scale
/// between the runs is not read as a change of behavior. What a series is
/// computed from is its own run's: a replacement that ends a cancellation of
/// large terms leaves no trace of them in the run it made. A series that is
/// not a number somewhere has no magnitude to share: its last numbers before
/// it stopped being one would make the other run's residue beside them.
pub(crate) fn compared_scales(
    model: &datamodel::Model,
    key: &str,
    this: (&Run, &[f64]),
    that: Option<(&Run, &[f64])>,
) -> (f64, Option<f64>) {
    let shared = std::iter::once(this)
        .chain(that)
        .map(|(_, values)| values)
        .filter(|values| values.iter().all(|v| v.is_finite()))
        .map(magnitude)
        .fold(0.0, f64::max);
    let own =
        |(run, _): (&Run, &[f64])| scale_in_run(&run.results, model, &run.plan, key).max(shared);
    (own(this), that.map(own))
}

/// The scale of an equation that is a sum or a difference at its top level:
/// the largest magnitude among its terms that are variables or numbers. Zero
/// for any other equation. `reaches` is what a variable reaches, at the
/// element a subscript of plain names pins when it has one.
fn scale_of_a_sum(
    equation: &crate::ast::Expr0,
    reaches: &impl Fn(&str, Option<&str>) -> f64,
) -> f64 {
    use crate::ast::{BinaryOp, Expr0, IndexExpr0, UnaryOp};
    fn terms<'a>(expr: &'a Expr0, into: &mut Vec<&'a Expr0>) {
        match expr {
            Expr0::Op2(BinaryOp::Add | BinaryOp::Sub, left, right, _) => {
                terms(left, into);
                terms(right, into);
            }
            Expr0::Op1(UnaryOp::Negative | UnaryOp::Positive, inner, _) => terms(inner, into),
            term => into.push(term),
        }
    }
    let mut found = Vec::new();
    terms(equation, &mut found);
    if found.len() < 2 {
        return 0.0;
    }
    found
        .into_iter()
        .map(|term| match term {
            Expr0::Var(name, _) => reaches(name.as_str(), None),
            Expr0::Subscript(name, indices, _) => {
                // Plain names may be elements (`stock[north]`) or the
                // dimensions the equation ranges over (`stock[region]`);
                // `reaches` tries the element they spell first.
                let names: Option<Vec<String>> = indices
                    .iter()
                    .map(|index| match index {
                        IndexExpr0::Expr(Expr0::Var(index, _)) => {
                            Some(crate::canonicalize(index.as_str()).into_owned())
                        }
                        _ => None,
                    })
                    .collect();
                reaches(name.as_str(), names.map(|names| names.join(",")).as_deref())
            }
            Expr0::Const(_, number, _) => number.value().abs(),
            _ => 0.0,
        })
        .fold(0.0, f64::max)
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

/// The core, and the turning points and samples when asked for, of a series
/// read at `scale`.
fn summarize(
    times: &[f64],
    values: &[f64],
    scale: f64,
    full: bool,
) -> (SeriesCore, Vec<Point>, Vec<Point>) {
    let n = values.len();
    if n == 0 {
        return (
            SeriesCore {
                start: None,
                end: None,
                min: None,
                max: None,
                negative_from: None,
                mode: super::behavior::classify_at(times, values, scale),
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
    // Rounding keeps a number's sign, so a series went negative exactly
    // when a number reported of it is negative.
    let reported = round;
    let point = |i: usize| {
        values[i].is_finite().then(|| Point {
            time: round(times[i]),
            value: reported(values[i]),
        })
    };
    let negative_from = values
        .iter()
        .position(|&v| reported(v) < 0.0)
        .map(|i| round(times[i]));
    let shape = shape_at(times, values, scale);
    let number = |v: f64| v.is_finite().then(|| reported(v));
    let core = SeriesCore {
        start: number(values[0]),
        end: number(values[n - 1]),
        min: argmin.and_then(point),
        max: argmax.and_then(point),
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
        .filter_map(|&i| point(i))
        .collect();
    let count = SAMPLES.min(n);
    let samples = (0..count)
        .filter_map(|k| {
            let i = if count == 1 {
                0
            } else {
                k * (n - 1) / (count - 1)
            };
            point(i)
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

/// One series of a run: what a summary names it, its results key, and its
/// values.
pub(crate) struct KeyedSeries {
    pub label: String,
    pub key: String,
    pub values: Vec<f64>,
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
    let (series, omitted) = keyed_series_upto(run, model, variable, element, limit);
    (
        series
            .into_iter()
            .map(|series| (series.label, series.values))
            .collect(),
        omitted,
    )
}

/// [`element_series_upto`], with each series' results key.
pub(crate) fn keyed_series_upto(
    run: &Run,
    model: &datamodel::Model,
    variable: &str,
    element: Option<&str>,
    limit: usize,
) -> (Vec<KeyedSeries>, usize) {
    let canonical = crate::canonicalize(variable).into_owned();
    let display = display_name(model, &canonical);
    let offsets = &run.results.offsets;
    if let Some(element) = element {
        let key = format!("{canonical}[{}]", element_key(element));
        return match offsets.get(&Ident::<Canonical>::new(&key)) {
            Some(&offset) => (
                vec![KeyedSeries {
                    label: format!("{display}[{element}]"),
                    values: run.series(offset),
                    key,
                }],
                0,
            ),
            None => (vec![], 0),
        };
    }
    let elements = variable_columns(&run.results, model, &canonical);
    let omitted = elements.len().saturating_sub(limit);
    let series = elements
        .into_iter()
        .take(limit)
        .map(|column| {
            let key = format!("{canonical}{}", column.subscript);
            KeyedSeries {
                label: format!("{display}{}", column.subscript),
                values: run.series(column.offset),
                key,
            }
        })
        .collect();
    (series, omitted)
}

/// One results column of a variable.
pub(crate) struct VariableColumn<'a> {
    /// What the key adds to the variable's name: `[e1,e2]` for an element,
    /// empty for a scalar's one column.
    pub subscript: &'a str,
    pub offset: usize,
}

/// The results columns of the variable `canonical` of `model`, in slot
/// order: those `save_check::column_variable` gives the variable, the one
/// owner of which variable a column belongs to, so a name that holds `[` or
/// `$` itself is no prefix of another's. A module instance's columns are its
/// model's variables', reached through it, and are not the instance's own.
pub(crate) fn variable_columns<'a>(
    results: &'a crate::Results,
    model: &datamodel::Model,
    canonical: &str,
) -> Vec<VariableColumn<'a>> {
    let declared: std::collections::BTreeSet<String> = model
        .variables
        .iter()
        .map(|var| crate::canonicalize(var.get_ident()).into_owned())
        .collect();
    let mut columns: Vec<VariableColumn<'a>> = results
        .offsets
        .iter()
        .filter_map(|(key, &offset)| {
            let owner = crate::save_check::column_variable(key.as_str(), &declared)?;
            let subscript = &key.as_str()[owner.len()..];
            (owner == canonical && (subscript.is_empty() || subscript.starts_with('[')))
                .then_some(VariableColumn { subscript, offset })
        })
        .collect();
    columns.sort_by_key(|column| column.offset);
    columns
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
                reason: None,
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
                let (elements, omitted) = keyed_series_upto(
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
                for KeyedSeries { label, key, values } in elements {
                    let scale = scale_in_run(&run.results, model, &run.plan, &key);
                    let (core, turns, samples) = summarize(&times, &values, scale, true);
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
