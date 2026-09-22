// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `run_experiment`: a what-if run on a copy of the model, kept under a name.
//!
//! An experiment is data -- values or multipliers for constants, replacement
//! equations, a time the values take effect, run specs -- so it is bounded,
//! checkable, and cheap to describe. It starts from another run (the model as
//! it is by default, or a named run, whose changes it keeps unless it changes
//! the same variables), is kept under its name for later tools and claims to
//! cite, and is answered with the changes as applied and, for each recorded
//! variable, its behavior beside the run it started from.
//!
//! A multiplier is resolved to the value it gives when the experiment is made
//! -- the constant's value in the starting run, at the time the change takes
//! effect, times the factor -- so a run's plan is values and equations only,
//! and replaying it reproduces it.
//!
//! A model that does not simulate can still be tried in a copy: an
//! experiment from "current" that replaces equations runs without a run to
//! start from, and is compared with nothing, so an agent can try a fix to a
//! learner's broken model before proposing it.

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::common::{Canonical, Ident};
use crate::datamodel::{self, Variable};

use super::outline::{
    AuxKind, IntegrationMethod, SpecsOutline, aux_kind, constant_value, equation_text,
};
use super::runs::{
    CURRENT, ElementValues, EquationChange, Run, RunPlan, SpecsChange, ValueChange, execute,
    has_table,
};
use super::series::{SeriesCore, element_series, round};
use super::{Session, ToolError, Workspace, names, resolve_model};

/// The most variables an experiment summarizes.
pub(crate) const MAX_RECORD: usize = 12;
/// The longest name a run can have.
pub(crate) const MAX_RUN_NAME: usize = 64;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RunExperimentInput {
    /// A short name the run is kept and cited under; a name already used
    /// replaces that run. "current" is the model as it is, and is not a name
    /// an experiment can take.
    pub name: String,
    /// The run to start from and compare with: "current" (the default) or a
    /// run an experiment made. Its changes are kept unless this experiment
    /// changes the same variables.
    #[serde(default)]
    pub from: Option<String>,
    /// The changes, each to one variable.
    #[serde(default)]
    pub set: Vec<ChangeInput>,
    /// When the value and multiplier changes take effect: from the first step
    /// at or after this time; from the start (initial values included) when
    /// absent. Equation changes always hold from the start.
    #[serde(default)]
    pub from_time: Option<f64>,
    /// Run specs to change.
    #[serde(default)]
    pub specs: Option<SpecsInput>,
    /// The variables to summarize, by name; the model's stocks when absent.
    /// At most 12.
    #[serde(default)]
    #[cfg_attr(feature = "schema", schemars(length(max = 12)))]
    pub record: Vec<String>,
}

/// One change to one variable: exactly one of `value`, `multiply` and
/// `equation`. Its schema says so with a `oneOf`, while the type reads all
/// three, so a call that gives two is refused with the rule rather than with
/// a parser's "no variant matched".
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeInput {
    pub variable: String,
    /// A constant's new value (for an arrayed constant, every element's).
    #[serde(default)]
    pub value: Option<f64>,
    /// A factor for a constant's value in the run the experiment starts from.
    #[serde(default)]
    pub multiply: Option<f64>,
    /// A replacement equation, in this run only; for a stock, its initial
    /// value. Replacing a computed variable's equation cuts the links from
    /// what it read: that is how a link is taken out to test an explanation.
    #[serde(default)]
    pub equation: Option<String>,
}

#[cfg(feature = "schema")]
impl JsonSchema for ChangeInput {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ChangeInput".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let variant = |field: &str, schema: serde_json::Value| {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "variable": {"type": "string"},
                    field: schema,
                },
                "required": ["variable", field],
                "additionalProperties": false,
            })
        };
        schemars::json_schema!({
            "description": "One change to one variable: its new value, a multiplier for its value, or a replacement equation.",
            "oneOf": [
                variant("value", serde_json::json!({
                    "description": "A constant's new value (for an arrayed constant, every element's).",
                    "type": "number",
                })),
                variant("multiply", serde_json::json!({
                    "description": "A factor for a constant's value in the run the experiment starts from.",
                    "type": "number",
                })),
                variant("equation", serde_json::json!({
                    "description": "A replacement equation, in this run only; for a stock, its initial value. It replaces the variable's value, a table it fed included. Replacing a computed variable's equation cuts the links from what it read: that is how a link is taken out to test an explanation.",
                    "type": "string",
                })),
            ],
        })
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct SpecsInput {
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub stop: Option<f64>,
    #[serde(default)]
    pub dt: Option<f64>,
    #[serde(default)]
    pub method: Option<IntegrationMethod>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunExperimentOutput {
    pub revision: u64,
    /// The run's name.
    pub run: String,
    /// The run it started from and is compared with.
    pub from: String,
    /// This experiment's changes as applied.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub applied: Vec<AppliedChange>,
    /// The specs the run ran under.
    pub specs: SpecsOutline,
    /// Each recorded variable (or element) in this run and in the one it
    /// started from.
    pub behavior: Vec<Comparison>,
    /// Recorded variables and elements left out, when there were more than a
    /// summary lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<usize>,
    /// Whether the run replaced an earlier run of the same name.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub replaced: bool,
    /// A run forgotten to keep the session within its runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forgotten: Option<String>,
    /// Why the run is compared with nothing: the model as it is does not
    /// simulate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One change as applied.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct AppliedChange {
    pub variable: String,
    /// The value it took, and what it was in the starting run (a scalar
    /// constant).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was: Option<f64>,
    /// Each element's value and what it was (an arrayed constant).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<ElementValue>,
    /// The replacement equation, and the equation it replaced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was_equation: Option<String>,
    /// That the variable's table no longer applies: its value was the table
    /// at its equation's value, and is now the replacement itself.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub table_dropped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_time: Option<f64>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ElementValue {
    pub element: String,
    pub value: f64,
    pub was: f64,
}

/// A variable's behavior in the new run and in the run it started from.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub variable: String,
    pub this: SeriesCore,
    /// Absent when there is no run to compare with: the model as it is does
    /// not simulate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base: Option<SeriesCore>,
}

/// Answer `run_experiment`.
pub(crate) fn run_experiment(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: RunExperimentInput,
) -> Result<RunExperimentOutput, ToolError> {
    let name = input.name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_RUN_NAME {
        return Err(ToolError::new(format!(
            "a run's name is 1 to {MAX_RUN_NAME} characters"
        )));
    }
    if name == CURRENT {
        return Err(ToolError::new(
            "\"current\" names the model as it is; give the experiment a name of its own",
        ));
    }
    if input.record.len() > MAX_RECORD {
        return Err(ToolError::new(format!(
            "record at most {MAX_RECORD} variables (this call names {})",
            input.record.len()
        )));
    }
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let model = resolved.model;
    let from = input
        .from
        .as_deref()
        .map(str::trim)
        .unwrap_or(CURRENT)
        .to_string();
    // A model that does not simulate is tried in a copy with no run to start
    // from: only its equations and specs can change, since a value change
    // runs nothing.
    let (base, note) = match session.runs.get(ws, model, &from) {
        Ok(base) => (Some(base), None),
        Err(err) if err.is_interrupted() => return Err(err),
        Err(err) if from == CURRENT => {
            if input.set.iter().any(|c| c.equation.is_none()) {
                return Err(ToolError::new(format!(
                    "{}; a value change has nothing to run, but a replacement equation can try \
                     a fix in a copy",
                    err.error
                )));
            }
            let note = format!("{}; this run is compared with nothing", err.error);
            (None, Some(note))
        }
        Err(err) => return Err(err),
    };
    if let Some(base) = &base
        && !session.runs.is_fresh(ws, base)
    {
        return Err(ToolError::new(format!(
            "run '{from}' was made at revision {} and the model has changed since (revision {}); \
             start from \"current\", or run '{from}' again",
            base.revision, ws.revision
        )));
    }

    let specs = specs_change(input.specs.as_ref())?;
    let from_time = input.from_time;
    if let Some(time) = from_time {
        let (start, stop) = match &base {
            Some(base) => (base.results.specs.start, base.results.specs.stop),
            None => {
                let model_specs = super::changes::effective_specs(ws.project, model);
                (model_specs.start, model_specs.stop)
            }
        };
        let (start, stop) = (specs.start.unwrap_or(start), specs.stop.unwrap_or(stop));
        if !time.is_finite() || time < start || time > stop {
            return Err(ToolError::new(format!(
                "fromTime {time} is outside the run, which goes from {start} to {stop}"
            )));
        }
    }

    let mut plan = RunPlan {
        specs,
        from: Some(from.clone()),
        ..RunPlan::default()
    };
    let mut applied = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for change in &input.set {
        let var = names::resolve(model, &change.variable).map_err(|suggestions| {
            ToolError::new(format!("the model has no variable '{}'", change.variable))
                .with_suggestions(suggestions)
        })?;
        let canonical = crate::canonicalize(var.get_ident()).into_owned();
        if seen.contains(&canonical) {
            return Err(ToolError::new(format!(
                "'{}' is changed twice; give it one change",
                var.get_ident()
            )));
        }
        seen.push(canonical.clone());
        match (change.value, change.multiply, &change.equation) {
            (Some(value), None, None) => {
                let base = base
                    .as_ref()
                    .expect("a value change has a run to start from");
                let (values, applied_change) = value_change(base, var, from_time, |_| value)?;
                plan.values.push(ValueChange {
                    variable: canonical,
                    from_time,
                    values,
                });
                applied.push(applied_change);
            }
            (None, Some(factor), None) => {
                let base = base
                    .as_ref()
                    .expect("a value change has a run to start from");
                let (values, applied_change) =
                    value_change(base, var, from_time, |was| was * factor)?;
                plan.values.push(ValueChange {
                    variable: canonical,
                    from_time,
                    values,
                });
                applied.push(applied_change);
            }
            (None, None, Some(equation)) => {
                if from_time.is_some() {
                    return Err(ToolError::new(format!(
                        "an equation change holds from the start, so it cannot take fromTime; \
                         to change '{}' at a time, write the time into its equation (with STEP, \
                         or IF TIME >= t THEN new ELSE old)",
                        var.get_ident()
                    )));
                }
                if matches!(var, Variable::Module(_)) {
                    return Err(ToolError::new(format!(
                        "'{}' is a module, which has no equation to replace",
                        var.get_ident()
                    )));
                }
                if equation.trim().is_empty() {
                    return Err(ToolError::new(format!(
                        "give '{}' an equation that is not empty",
                        var.get_ident()
                    )));
                }
                if let Variable::Aux(aux) = var
                    && matches!(aux_kind(aux), AuxKind::Lookup(_))
                {
                    return Err(ToolError::new(format!(
                        "'{}' is a lookup table other equations call, which has no value of its \
                         own to replace; replace the equation of a variable that reads it",
                        var.get_ident()
                    )));
                }
                plan.equations.push(EquationChange {
                    variable: canonical,
                    equation: equation.clone(),
                });
                applied.push(AppliedChange {
                    variable: var.get_ident().to_string(),
                    value: None,
                    was: None,
                    elements: vec![],
                    equation: Some(equation.clone()),
                    was_equation: var.get_equation().map(equation_text),
                    table_dropped: has_table(var),
                    from_time: None,
                });
            }
            _ => {
                return Err(ToolError::new(format!(
                    "give '{}' exactly one of value, multiply and equation",
                    change.variable
                )));
            }
        }
    }

    let record: Vec<&Variable> = if input.record.is_empty() {
        model
            .variables
            .iter()
            .filter(|v| matches!(v, Variable::Stock(_)))
            .collect()
    } else {
        let mut record = Vec::new();
        for name in &input.record {
            let var = names::resolve(model, name).map_err(|suggestions| {
                ToolError::new(format!("the model has no variable '{name}' to record"))
                    .with_suggestions(suggestions)
            })?;
            record.push(var);
        }
        record
    };

    let plan = match &base {
        Some(base) => plan.over(&base.plan),
        None => plan,
    };
    ws.yield_point()?;
    let results = execute(ws, model, &plan).map_err(|failure| {
        failure.refusal(|reason| ToolError::new(format!("the experiment does not run: {reason}")))
    })?;
    let key = session.runs.key(ws);
    let run = Run::new(name.clone(), ws.revision, key, plan, results);

    let (behavior, omitted) = compare(&run, base.as_deref(), model, &record);
    let specs = run_specs(&run, model, ws.project);
    let (replaced, forgotten) = session.runs.keep(run);
    Ok(RunExperimentOutput {
        revision: ws.revision,
        run: name,
        from,
        applied,
        specs,
        behavior,
        omitted: (omitted > 0).then_some(omitted),
        replaced,
        forgotten,
        note,
    })
}

/// Whether a variable holds a value an experiment can set: a constant, or a
/// flow whose equation is a number. Refused with the repair otherwise.
fn settable(var: &Variable) -> Result<(), ToolError> {
    let name = var.get_ident();
    match var {
        Variable::Aux(aux) => match aux_kind(aux) {
            AuxKind::Constant(_) => Ok(()),
            AuxKind::Lookup(_) => Err(ToolError::new(format!(
                "'{name}' is a lookup table, which has no value of its own; change the variable \
                 that reads it, or give that variable an equation"
            ))),
            AuxKind::Computed => Err(ToolError::new(format!(
                "'{name}' is computed ({}); give it an equation instead, or change a constant it \
                 reads",
                equation_text(&aux.equation)
            ))),
        },
        Variable::Flow(flow) if flow.gf.is_none() && constant_value(&flow.equation).is_some() => {
            Ok(())
        }
        Variable::Flow(flow) => Err(ToolError::new(format!(
            "'{name}' is computed ({}); give it an equation instead, or change a constant it reads",
            equation_text(&flow.equation)
        ))),
        Variable::Stock(_) => Err(ToolError::new(format!(
            "'{name}' is a stock, whose value is integrated; set its initial value with an \
             equation, or change its flows"
        ))),
        Variable::Module(_) => Err(ToolError::new(format!(
            "'{name}' is a module, which has no value of its own"
        ))),
    }
}

/// A value change's per-element values, each `to(what it was in the base run
/// when the change takes effect)`, and the change as applied.
fn value_change(
    base: &Run,
    var: &Variable,
    from_time: Option<f64>,
    to: impl Fn(f64) -> f64,
) -> Result<(ElementValues, AppliedChange), ToolError> {
    settable(var)?;
    let row = base.row_at(from_time.unwrap_or(base.results.specs.start));
    let canonical = crate::canonicalize(var.get_ident()).into_owned();
    let offsets = &base.results.offsets;
    let mut keys: Vec<(Ident<Canonical>, usize)> =
        if let Some((key, &offset)) = offsets.get_key_value(&Ident::<Canonical>::new(&canonical)) {
            vec![(key.clone(), offset)]
        } else {
            let prefix = format!("{canonical}[");
            offsets
                .iter()
                .filter(|(key, _)| key.as_str().starts_with(&prefix))
                .map(|(key, &offset)| (key.clone(), offset))
                .collect()
        };
    if keys.is_empty() {
        return Err(ToolError::new(format!(
            "'{}' has no value in the run to change",
            var.get_ident()
        )));
    }
    keys.sort_by_key(|(_, offset)| *offset);
    let data = base.results.iter().nth(row).expect("the row is in the run");
    let values: Vec<(Ident<Canonical>, f64, f64)> = keys
        .into_iter()
        .map(|(key, offset)| {
            let was = data[offset];
            (key, was, to(was))
        })
        .collect();
    if let Some((key, _, value)) = values.iter().find(|(_, _, v)| !v.is_finite()) {
        return Err(ToolError::new(format!(
            "'{}' would be {value}, which is not a number a run can hold",
            key.as_str()
        )));
    }
    let scalar = values.len() == 1 && !values[0].0.as_str().contains('[');
    let display = var.get_ident().to_string();
    let applied = AppliedChange {
        variable: display.clone(),
        value: scalar.then(|| round(values[0].2)),
        was: scalar.then(|| round(values[0].1)),
        elements: if scalar {
            vec![]
        } else {
            values
                .iter()
                .map(|(key, was, value)| ElementValue {
                    element: key.as_str()[canonical.len()..]
                        .trim_start_matches('[')
                        .trim_end_matches(']')
                        .to_string(),
                    value: round(*value),
                    was: round(*was),
                })
                .collect()
        },
        equation: None,
        was_equation: None,
        table_dropped: false,
        from_time,
    };
    Ok((
        values
            .into_iter()
            .map(|(key, _, value)| (key, value))
            .collect(),
        applied,
    ))
}

/// The input specs as a plan's specs, refused when they describe no run.
fn specs_change(input: Option<&SpecsInput>) -> Result<SpecsChange, ToolError> {
    let Some(input) = input else {
        return Ok(SpecsChange::default());
    };
    for (field, value) in [
        ("start", input.start),
        ("stop", input.stop),
        ("dt", input.dt),
    ] {
        if let Some(value) = value
            && !value.is_finite()
        {
            return Err(ToolError::new(format!("specs.{field} must be a number")));
        }
    }
    if let Some(dt) = input.dt
        && dt <= 0.0
    {
        return Err(ToolError::new("specs.dt must be more than zero"));
    }
    if let (Some(start), Some(stop)) = (input.start, input.stop)
        && stop <= start
    {
        return Err(ToolError::new("specs.stop must come after specs.start"));
    }
    Ok(SpecsChange {
        start: input.start,
        stop: input.stop,
        dt: input.dt,
        method: input.method,
    })
}

/// The specs a run ran under, as an outline states them.
fn run_specs(run: &Run, model: &datamodel::Model, project: &datamodel::Project) -> SpecsOutline {
    let specs = &run.results.specs;
    SpecsOutline {
        start: specs.start,
        stop: specs.stop,
        dt: specs.dt,
        save_step: (specs.save_step != specs.dt).then_some(specs.save_step),
        method: match specs.method {
            crate::results::Method::Euler => IntegrationMethod::Euler,
            crate::results::Method::RungeKutta2 => IntegrationMethod::Rk2,
            crate::results::Method::RungeKutta4 => IntegrationMethod::Rk4,
        },
        time_units: super::changes::effective_specs(project, model)
            .time_units
            .clone()
            .filter(|u| !u.is_empty()),
    }
}

/// Each recorded variable's (or element's) behavior in `run` and `base`, up to
/// the summary's limit, and how many were left out.
fn compare(
    run: &Run,
    base: Option<&Run>,
    model: &datamodel::Model,
    record: &[&Variable],
) -> (Vec<Comparison>, usize) {
    let mut comparisons = Vec::new();
    let mut omitted = 0;
    let run_times = run.times();
    let base_times = base.map(Run::times);
    for var in record {
        let (this, this_omitted) = element_series(run, model, var.get_ident());
        let that = base.map(|base| element_series(base, model, var.get_ident()).0);
        omitted += this_omitted;
        for (i, (label, values)) in this.into_iter().enumerate() {
            if comparisons.len() == MAX_RECORD {
                omitted += 1;
                continue;
            }
            let base_core = match (&that, &base_times) {
                (Some(that), Some(times)) => that.get(i).map(|(_, v)| SeriesCore::of(times, v)),
                _ => None,
            };
            comparisons.push(Comparison {
                variable: label,
                this: SeriesCore::of(&run_times, &values),
                base: base_core,
            });
        }
    }
    (comparisons, omitted)
}

#[cfg(test)]
#[path = "experiment_tests.rs"]
mod tests;
