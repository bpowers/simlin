// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Runs: the model as it is ("current"), and the experiments a session keeps
//! under names.
//!
//! A run is its [`RunPlan`] -- what it changed from the model -- and the
//! results that plan produced at a revision. A plan is data: values set on
//! constants (each from a time on, or from the start), replacement equations,
//! and run specs. Values are VM overrides, applied without recompiling, and
//! so are run specs: the compiled program does not depend on them, so the
//! model's own program runs under the plan's (`Vm::with_specs`). A
//! replacement equation stages a copy of the datamodel on the host's database
//! (`SimlinDb::sync_staged`), compiles and runs it, and restores
//! (`SimlinDb::restore`) exactly as a dry-run patch does, so the project is
//! never changed and unchanged variables keep their compiled fragments. (A
//! model with a conveyor or a queue stages a spec change too: its expansion
//! reads the specs.)
//!
//! A run is fresh while the model has what the run simulated: the project as
//! a simulation reads it, without its diagrams, notes and units
//! (`db::simulation_key`, taken from the salsa sync's own extraction of what
//! the compiler reads). The revision alone cannot say, since a layout edit
//! advances it and changes no run. The current run is cached for its key, and
//! a named run keeps the key and revision it was made at, so a reader can tell
//! a run of the model as it was from one of the model as it is.
//!
//! The store keeps the results of its runs within [`MAX_RUN_BYTES`]: past it,
//! the oldest runs keep their plans and lose their results, and a fresh one is
//! run again from its plan when asked for. A stale run whose results went is
//! gone, since running its plan now would simulate a different model.
//!
//! A host lists the named runs with what each changed ([`RunListing`]), as
//! the agent's `list_runs` does, and forgets one when the person discards it,
//! after which no tool reads it.
//!
//! A run is made without the LTM overlay. Its loops are analyzed on demand by
//! replaying its plan under the overlay ([`execute_then`], `loops`), and the
//! analysis is kept with the run.

use std::sync::{Arc, OnceLock};

use indexmap::IndexMap;
#[cfg(feature = "schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Canonical, Ident};
use crate::datamodel::{self, Equation, Variable};
use crate::db::{
    DiagnosticSeverity, LtmOverlay, SimlinDb, SourceProject, collect_all_diagnostics,
    simulation_key,
};
use crate::results::{Results, written, written_count, written_rows};

use super::evidence::{QUOTE_CHARS, window};
use super::loops::LoopAnalysis;
use super::outline::IntegrationMethod;
use super::{Session, ToolError, Workspace, resolve_model};

/// The name of the run of the model as it is.
pub(crate) const CURRENT: &str = "current";

/// The most named runs a session keeps; making one more forgets the oldest.
pub(crate) const MAX_RUNS: usize = 32;

/// The most bytes of results a session's runs keep; past it the oldest runs
/// keep only their plans. A large model's run is about ten megabytes.
pub(crate) const MAX_RUN_BYTES: usize = 64 * 1024 * 1024;

/// The most numbers a run may save, its saved rows times the model's slots:
/// 16 MB of results. A run over it is refused before it allocates them.
pub(crate) const MAX_RUN_VALUES: usize = 2_000_000;

/// The most a run may compute, its steps times the model's slots: well under
/// a second of a release build's simulation.
pub(crate) const MAX_RUN_STEPS: usize = 200_000_000;

/// The most bytes of results the runs of one batch ([`execute_values`])
/// hold at once: each run holds its results until it is summarized, so this
/// bounds how many run at a time, whatever the host's cores.
pub(crate) const MAX_BATCH_BYTES: usize = 64 * 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// The batch budget a test sets on its own thread, to observe the bound
    /// on a small model rather than a large one.
    pub(crate) static TEST_BATCH_BYTES: std::cell::Cell<usize> =
        const { std::cell::Cell::new(MAX_BATCH_BYTES) };
}

/// How many runs of `run_bytes` of results each a batch makes at once: as
/// many as [`MAX_BATCH_BYTES`] holds, and one however large a run is.
pub(crate) fn concurrent_runs(run_bytes: usize) -> usize {
    #[cfg(test)]
    let budget = TEST_BATCH_BYTES.with(std::cell::Cell::get);
    #[cfg(not(test))]
    let budget = MAX_BATCH_BYTES;
    (budget / run_bytes.max(1)).max(1)
}

/// How many slices a run is taken in. Between two, it asks whether other work
/// waits for the project, and stops if so: that work waits at most a slice
/// of a run, not the whole of a long one.
pub(crate) const RUN_SLICES: f64 = 16.0;

/// Why a run did not finish: the reason, in words, or other work that waits
/// for the project, which a run stops for between its slices.
#[derive(Debug)]
pub(crate) enum RunFailure {
    Failed(String),
    Stopped,
}

/// The most characters of why a run failed: every answer that says so
/// (a refusal, a skipped check, records without behavior) repeats it, and the
/// engine's own reasons can name every variable of a model.
pub(crate) const MAX_REASON_CHARS: usize = 400;

impl From<String> for RunFailure {
    /// The reason as answers give it: the engine's helper names said as what
    /// they stand for, cut to [`MAX_REASON_CHARS`].
    fn from(reason: String) -> RunFailure {
        let reason = super::evidence::explain_helpers(&reason);
        RunFailure::Failed(super::evidence::window(&reason, 0, 0, MAX_REASON_CHARS))
    }
}

impl RunFailure {
    /// What a tool answers a run that did not finish with: `failed`'s
    /// refusal for the reason, or the answer of a call that stopped.
    pub(crate) fn refusal(self, failed: impl FnOnce(String) -> ToolError) -> ToolError {
        match self {
            RunFailure::Failed(reason) => failed(reason),
            RunFailure::Stopped => ToolError::interrupted(),
        }
    }
}

/// The values a change gives a constant, one per results key: one key for a
/// scalar, one per element for an arrayed constant.
pub(crate) type ElementValues = Vec<(Ident<Canonical>, f64)>;

/// A constant's value from a time on (or from the start), for each of its
/// element keys.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct ValueChange {
    /// The variable's canonical name.
    pub variable: String,
    pub from_time: Option<f64>,
    /// Each element's results key and the value it takes.
    pub values: ElementValues,
}

/// A variable's equation replaced, from the start (for a stock, its initial
/// value).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct EquationChange {
    /// The variable's canonical name.
    pub variable: String,
    pub replacement: Replacement,
}

/// What a replaced equation becomes.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) enum Replacement {
    /// One equation, every element's for an arrayed variable.
    Equation(String),
    /// An equation for each element of an arrayed variable, by its subscript
    /// as a results key spells it (`north`, `a,b`).
    Elements(Vec<(String, String)>),
}

impl Replacement {
    /// Whether the replacement is a number (each element's, for an arrayed
    /// variable): the variable is then a constant in the run, which a value
    /// can be set on from a time.
    pub(crate) fn is_constant(&self) -> bool {
        let number = |text: &str| text.trim().parse::<f64>().is_ok_and(f64::is_finite);
        match self {
            Replacement::Equation(text) => number(text),
            Replacement::Elements(elements) => elements.iter().all(|(_, text)| number(text)),
        }
    }

    /// The replacement as one line, as an equation is quoted.
    pub(crate) fn text(&self) -> String {
        match self {
            Replacement::Equation(text) => text.trim().to_string(),
            Replacement::Elements(elements) => elements
                .iter()
                .map(|(element, text)| format!("{element}: {}", text.trim()))
                .collect::<Vec<_>>()
                .join("; "),
        }
    }
}

/// The specs a run ran under where they are not the model's; one left out
/// is the model's.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Default, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SpecsChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<IntegrationMethod>,
    /// How often the run saves. An experiment keeps the model's; a battery
    /// check that refines DT saves at the model's own times, so its rows
    /// compare with the model's one for one and it holds no more than the
    /// model's run does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_step: Option<f64>,
}

impl SpecsChange {
    fn is_empty(&self) -> bool {
        self == &SpecsChange::default()
    }

    /// `specs` with these changes: the one statement of what a plan's specs
    /// do to a model's, for the staged copy and for the VM alike.
    fn applied_to(&self, specs: &datamodel::SimSpecs) -> datamodel::SimSpecs {
        let mut specs = specs.clone();
        if let Some(start) = self.start {
            specs.start = start;
        }
        if let Some(stop) = self.stop {
            specs.stop = stop;
        }
        if let Some(dt) = self.dt {
            specs.dt = datamodel::Dt::Dt(dt);
        }
        if let Some(save) = self.save_step {
            specs.save_step = Some(datamodel::Dt::Dt(save));
        }
        if let Some(method) = self.method {
            specs.sim_method = match method {
                IntegrationMethod::Euler => datamodel::SimMethod::Euler,
                IntegrationMethod::Rk2 => datamodel::SimMethod::RungeKutta2,
                IntegrationMethod::Rk4 => datamodel::SimMethod::RungeKutta4,
            };
        }
        specs
    }

    /// `self`'s changes over `base`'s.
    pub(crate) fn over(&self, base: &SpecsChange) -> SpecsChange {
        SpecsChange {
            start: self.start.or(base.start),
            stop: self.stop.or(base.stop),
            dt: self.dt.or(base.dt),
            method: self.method.or(base.method),
            save_step: self.save_step.or(base.save_step),
        }
    }
}

/// What a run changed from the model.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Default)]
pub(crate) struct RunPlan {
    pub values: Vec<ValueChange>,
    pub equations: Vec<EquationChange>,
    pub specs: SpecsChange,
    /// The run an experiment made the plan over, as it was named then; what
    /// a listing says the run started from.
    pub from: Option<String>,
}

impl RunPlan {
    /// Whether the plan changes values only, so that it runs on the model's
    /// own compile under the model's own specs.
    pub(crate) fn only_sets_values(&self) -> bool {
        self.equations.is_empty() && self.specs.is_empty()
    }

    /// Whether the plan compiles a staged copy of `model_name` in `project`:
    /// it replaces an equation, or it changes the run specs of a model whose
    /// compile reads them (one with a conveyor or a queue, whose expansion
    /// does; `Vm::with_specs`).
    fn stages(&self, project: &datamodel::Project, model_name: &str) -> bool {
        !self.equations.is_empty()
            || (!self.specs.is_empty()
                && (crate::conveyor_compile::project_has_conveyor(project, model_name)
                    || crate::queue_compile::project_has_queue(project, model_name)))
    }

    /// `base`'s plan with `self`'s changes after it, and `self`'s specs over
    /// `base`'s.
    ///
    /// A variable's changes are a timeline. A change of this plan's that
    /// holds from the start -- a replacement equation, or a value with no
    /// time -- replaces every change `base` made to that variable. A value
    /// from a time on replaces only the changes `base` made from that time or
    /// later: what `base` set before it still holds until then, so the run is
    /// `base`'s up to that time, as its answer's `was` says.
    pub(crate) fn over(&self, base: &RunPlan) -> RunPlan {
        // When this plan first changes `variable`: `None` when it does not,
        // negative infinity for a change from the start.
        let changed_from = |variable: &str| -> Option<f64> {
            let from_start = self.equations.iter().any(|c| c.variable == variable);
            self.values
                .iter()
                .filter(|c| c.variable == variable)
                .map(|c| c.from_time.unwrap_or(f64::NEG_INFINITY))
                .chain(from_start.then_some(f64::NEG_INFINITY))
                .min_by(f64::total_cmp)
        };
        let survives = |variable: &str, from_time: Option<f64>| match changed_from(variable) {
            None => true,
            Some(changed) => from_time.unwrap_or(f64::NEG_INFINITY) < changed,
        };
        let mut values: Vec<ValueChange> = base
            .values
            .iter()
            .filter(|c| survives(&c.variable, c.from_time))
            .cloned()
            .collect();
        values.extend(self.values.iter().cloned());
        let mut equations: Vec<EquationChange> = base
            .equations
            .iter()
            .filter(|c| survives(&c.variable, None))
            .cloned()
            .collect();
        equations.extend(self.equations.iter().cloned());
        RunPlan {
            values,
            equations,
            specs: self.specs.over(&base.specs),
            from: self.from.clone(),
        }
    }
}

/// A run: its plan, and what the plan produced at `revision`, when the model
/// had the simulation key `key`.
pub(crate) struct Run {
    pub name: String,
    pub revision: u64,
    pub key: u64,
    pub plan: RunPlan,
    pub results: Results,
    /// The run's loop analysis, once asked for.
    pub loops: OnceLock<Arc<LoopAnalysis>>,
}

impl Run {
    pub(crate) fn new(
        name: String,
        revision: u64,
        key: u64,
        plan: RunPlan,
        results: Results,
    ) -> Run {
        Run {
            name,
            revision,
            key,
            plan,
            results,
            loops: OnceLock::new(),
        }
    }

    /// The saved times.
    pub(crate) fn times(&self) -> Vec<f64> {
        self.series(crate::results::TIME_OFF)
    }

    /// The series at a results offset, over the rows the run saved.
    pub(crate) fn series(&self, offset: usize) -> Vec<f64> {
        self.results.iter().map(|row| row[offset]).collect()
    }

    /// The row saved at `time`, or the last one before it: the row before the
    /// first saved past `time`.
    pub(crate) fn row_at(&self, time: f64) -> usize {
        let times = self.times();
        times
            .iter()
            .position(|&t| t > time)
            .unwrap_or(times.len())
            .saturating_sub(1)
    }

    /// The bytes the run holds: its results, and its loop analysis once it
    /// has one.
    fn bytes(&self) -> usize {
        self.results.data.len() * std::mem::size_of::<f64>()
            + self.loops.get().map_or(0, |analysis| analysis.bytes())
    }
}

/// A named run as the store keeps it: with its results, or, past the store's
/// budget, only its plan, to run again while it is fresh.
enum Kept {
    Run(Arc<Run>),
    Planned {
        revision: u64,
        key: u64,
        plan: RunPlan,
    },
}

/// What a session tells a host, or an agent, of one of its runs.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct RunListing {
    pub name: String,
    /// The revision the run was made at.
    pub revision: u64,
    /// Whether the model has changed, beyond its diagrams, since.
    pub stale: bool,
    /// Whether the run is gone: stale, with its results no longer kept.
    pub gone: bool,
    /// The run it started from, as it was named then: "current", the model
    /// as it was, or a run an experiment made.
    pub from: String,
    /// Everything the run changed from the model, with the changes it kept
    /// from the run it started from: values first, then equations.
    pub changes: Vec<ListedChange>,
    /// The run specs it changed; a spec left out is the model's.
    pub specs: SpecsChange,
}

/// One change a run makes to one variable, as it was made: exactly the value
/// the run set, where the experiment's answer rounds it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ListedChange {
    /// The variable as the model spells it, or its canonical name when the
    /// model no longer has it.
    pub variable: String,
    /// A scalar constant's value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// An arrayed constant's value element by element, or an equation for
    /// each element.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<ListedElement>,
    /// A replacement equation, which holds from the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equation: Option<String>,
    /// That the replacement is the variable's value, where the model's
    /// variable is a table read at its equation's value.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub table_dropped: bool,
    /// When a value takes effect: from the first step at or after this
    /// time, or from the start (initial values included) when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_time: Option<f64>,
}

/// One element's value, or its replacement equation, in a listed change.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ListedElement {
    /// The element's subscript, as a results key spells it (`north`, `a,b`).
    pub element: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equation: Option<String>,
}

impl RunPlan {
    /// The plan's changes as a host lists them, named as `model` spells its
    /// variables.
    fn listed(&self, model: Option<&datamodel::Model>) -> Vec<ListedChange> {
        let variable = |canonical: &str| model.and_then(|m| m.get_variable(canonical));
        let name = |canonical: &str| {
            variable(canonical).map_or_else(|| canonical.to_string(), |v| v.get_ident().to_string())
        };
        let values = self.values.iter().map(|change| {
            let scalar = match change.values.as_slice() {
                [(key, value)] if key.as_str() == change.variable => Some(*value),
                _ => None,
            };
            let prefix = format!("{}[", change.variable);
            ListedChange {
                variable: name(&change.variable),
                value: scalar,
                elements: if scalar.is_some() {
                    vec![]
                } else {
                    change
                        .values
                        .iter()
                        .map(|(key, value)| ListedElement {
                            element: key
                                .as_str()
                                .strip_prefix(&prefix)
                                .and_then(|rest| rest.strip_suffix(']'))
                                .unwrap_or(key.as_str())
                                .to_string(),
                            value: Some(*value),
                            equation: None,
                        })
                        .collect()
                },
                equation: None,
                table_dropped: false,
                from_time: change.from_time,
            }
        });
        let equations = self.equations.iter().map(|change| {
            let (equation, elements) = match &change.replacement {
                Replacement::Equation(text) => (Some(text.clone()), vec![]),
                Replacement::Elements(elements) => (
                    None,
                    elements
                        .iter()
                        .map(|(element, text)| ListedElement {
                            element: element.clone(),
                            value: None,
                            equation: Some(text.clone()),
                        })
                        .collect(),
                ),
            };
            ListedChange {
                variable: name(&change.variable),
                value: None,
                elements,
                equation,
                table_dropped: variable(&change.variable).is_some_and(has_table),
                from_time: None,
            }
        });
        values.chain(equations).collect()
    }

    /// The listing of a run made under this plan.
    fn listing(
        &self,
        name: &str,
        revision: u64,
        stale: bool,
        gone: bool,
        model: Option<&datamodel::Model>,
    ) -> RunListing {
        RunListing {
            name: name.to_string(),
            revision,
            stale,
            gone,
            from: self.from.clone().unwrap_or_else(|| CURRENT.to_string()),
            changes: self.listed(model),
            specs: self.specs.clone(),
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ListRunsInput {}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ListRunsOutput {
    pub revision: u64,
    /// The session's named runs, oldest first. "current", the model as it
    /// is, is always there and is not listed.
    pub runs: Vec<RunListing>,
    /// Runs left out to keep the answer within its budget, oldest first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
}

/// Answer `list_runs`: the listing a host reads, fitted to the answer budget
/// by quoting long equations around their start, then leaving out the oldest
/// runs, named.
pub(crate) fn list_runs(
    session: &mut Session,
    ws: &Workspace<'_>,
    _input: ListRunsInput,
) -> Result<ListRunsOutput, ToolError> {
    let model = resolve_model(ws.project, ws.db, &session.model_name)?.model;
    let mut output = ListRunsOutput {
        revision: ws.revision,
        runs: session.runs.listing(ws.project, ws.revision, Some(model)),
        omitted: vec![],
    };
    let mut quoted = false;
    super::fit(&mut output, session.outline_budget, |output| {
        if !quoted {
            quoted = true;
            for change in output.runs.iter_mut().flat_map(|run| &mut run.changes) {
                if let Some(equation) = &mut change.equation {
                    *equation = window(equation, 0, 0, QUOTE_CHARS);
                }
            }
        } else if output.runs.is_empty() {
            return false;
        } else {
            let oldest = output.runs.remove(0);
            output.omitted.push(oldest.name);
        }
        true
    });
    Ok(output)
}

/// The runs a session keeps.
#[derive(Default)]
pub(crate) struct RunStore {
    /// The run of the model as it is, for the simulation key it ran at.
    current: Option<Arc<Run>>,
    /// Named runs, oldest first: a run keeps its place when its results are
    /// dropped and when it is run again from its plan.
    named: IndexMap<String, Kept>,
    /// The simulation key at a revision, the last one asked for.
    key: Option<(u64, u64)>,
    /// A test's store budget in place of [`MAX_RUN_BYTES`].
    #[cfg(test)]
    pub(crate) byte_budget: Option<usize>,
}

impl RunStore {
    /// The model's simulation key at the workspace's revision.
    pub(crate) fn key(&mut self, ws: &Workspace<'_>) -> u64 {
        self.key_of(ws.project, ws.revision)
    }

    /// The simulation key of `project`, the contents at `revision`: read
    /// from the contents alone, never the database.
    pub(crate) fn key_of(&mut self, project: &datamodel::Project, revision: u64) -> u64 {
        match self.key {
            Some((at, key)) if at == revision => key,
            _ => {
                let key = simulation_key(project);
                self.key = Some((revision, key));
                key
            }
        }
    }

    /// The run named `name` when the store keeps its results -- the current
    /// run while it is of the model as it is, or a named run that still has
    /// its results -- with whether it is stale; `None` for a run that must
    /// be made, which [`RunStore::get`] makes. Reads the contents alone, so a
    /// host's read of a kept run waits for no one's work on the database.
    pub(crate) fn kept(
        &mut self,
        project: &datamodel::Project,
        revision: u64,
        name: &str,
    ) -> Option<(Arc<Run>, bool)> {
        let key = self.key_of(project, revision);
        let run = if name == CURRENT {
            self.current.as_ref().filter(|run| run.key == key)?
        } else {
            match self.named.get(name)? {
                Kept::Run(run) => run,
                Kept::Planned { .. } => return None,
            }
        };
        Some((run.clone(), run.key != key))
    }

    /// Whether `run` is of the model as it is.
    pub(crate) fn is_fresh(&mut self, ws: &Workspace<'_>, run: &Run) -> bool {
        run.key == self.key(ws)
    }

    /// The run of the model as it is: the cached one while the model has what
    /// it simulated, or a new run.
    pub(crate) fn current(
        &mut self,
        ws: &mut Workspace<'_>,
        model: &datamodel::Model,
    ) -> Result<Arc<Run>, ToolError> {
        let key = self.key(ws);
        if let Some(run) = &self.current
            && run.key == key
        {
            return Ok(run.clone());
        }
        ws.yield_point()?;
        let results = execute(ws, model, &RunPlan::default()).map_err(|failure| {
            failure.refusal(|reason| {
                ToolError::new(format!(
                    "the model does not simulate: {reason}; read_model lists its diagnostics"
                ))
            })
        })?;
        let run = Arc::new(Run::new(
            CURRENT.to_string(),
            ws.revision,
            key,
            RunPlan::default(),
            results,
        ));
        self.current = Some(run.clone());
        Ok(run)
    }

    /// The run named `name`: the current run for `"current"`, else a named
    /// run -- run again from its plan when only its plan is kept and it is
    /// fresh -- refused with the names the session has.
    pub(crate) fn get(
        &mut self,
        ws: &mut Workspace<'_>,
        model: &datamodel::Model,
        name: &str,
    ) -> Result<Arc<Run>, ToolError> {
        if name == CURRENT {
            return self.current(ws, model);
        }
        let key = self.key(ws);
        let (revision, plan) = match self.named.get(name) {
            Some(Kept::Run(run)) => return Ok(run.clone()),
            Some(Kept::Planned {
                key: planned_key, ..
            }) if *planned_key != key => {
                return Err(ToolError::new(format!(
                    "run '{name}' was made before the model changed, and the session no longer \
                     keeps its results; run the experiment again"
                )));
            }
            Some(Kept::Planned { revision, plan, .. }) => (*revision, plan.clone()),
            None => {
                let mut names = vec![CURRENT.to_string()];
                names.extend(self.named.keys().cloned());
                return Err(ToolError::new(format!(
                    "there is no run named '{}'",
                    super::evidence::echo(name)
                ))
                .with_suggestions(names));
            }
        };
        ws.yield_point()?;
        let results = execute(ws, model, &plan).map_err(|failure| {
            failure.refusal(|reason| {
                ToolError::new(format!("run '{name}' does not run again: {reason}"))
            })
        })?;
        let run = Arc::new(Run::new(name.to_string(), revision, key, plan, results));
        self.named.insert(name.to_string(), Kept::Run(run.clone()));
        self.within_budget(name);
        Ok(run)
    }

    /// Keep `run` under its name, replacing a run of that name, and forget the
    /// oldest run when the session holds more than [`MAX_RUNS`]. Returns
    /// whether a run was replaced and the name of any run forgotten.
    pub(crate) fn keep(&mut self, run: Run) -> (bool, Option<String>) {
        let replaced = self.named.shift_remove(&run.name).is_some();
        let name = run.name.clone();
        self.named.insert(name.clone(), Kept::Run(Arc::new(run)));
        let forgotten = (self.named.len() > MAX_RUNS)
            .then(|| self.named.shift_remove_index(0).map(|(name, _)| name))
            .flatten();
        self.within_budget(&name);
        (replaced, forgotten)
    }

    /// Forget the named run `name`, its results and its plan, so no tool
    /// reads it again; whether the session had it.
    pub(crate) fn forget(&mut self, name: &str) -> bool {
        self.named.shift_remove(name).is_some()
    }

    /// Every named run, oldest first, as a host lists them, with the
    /// variables named as `model` spells them.
    pub(crate) fn listing(
        &mut self,
        project: &datamodel::Project,
        revision: u64,
        model: Option<&datamodel::Model>,
    ) -> Vec<RunListing> {
        let key = self.key_of(project, revision);
        self.named
            .iter()
            .map(|(name, kept)| match kept {
                Kept::Run(run) => {
                    run.plan
                        .listing(name, run.revision, run.key != key, false, model)
                }
                Kept::Planned {
                    revision,
                    key: planned_key,
                    plan,
                } => {
                    let stale = *planned_key != key;
                    plan.listing(name, *revision, stale, stale, model)
                }
            })
            .collect()
    }

    /// Drop the results of the oldest runs, other than `keep`, until the
    /// store's results fit its budget; the current run counts too.
    fn within_budget(&mut self, keep: &str) {
        #[cfg(test)]
        let budget = self.byte_budget.unwrap_or(MAX_RUN_BYTES);
        #[cfg(not(test))]
        let budget = MAX_RUN_BYTES;
        let bytes = |kept: &Kept| match kept {
            Kept::Run(run) => run.bytes(),
            Kept::Planned { .. } => 0,
        };
        let mut held: usize = self.named.values().map(bytes).sum::<usize>()
            + self.current.as_ref().map_or(0, |run| run.bytes());
        for (name, kept) in self.named.iter_mut() {
            if held <= budget {
                break;
            }
            let Kept::Run(run) = kept else { continue };
            if name == keep {
                continue;
            }
            held -= run.bytes();
            *kept = Kept::Planned {
                revision: run.revision,
                key: run.key,
                plan: run.plan.clone(),
            };
        }
    }
}

/// Compile and run `model` under `plan`, on the workspace's database, and
/// return the results; the reason, in words, when it does not simulate, and
/// [`RunFailure::Stopped`] when other work waits for the project between two
/// of its slices.
pub(crate) fn execute(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    plan: &RunPlan,
) -> Result<Results, RunFailure> {
    execute_then(ws, model, plan, LtmOverlay::Off, |_, _, _| ()).map(|(results, ())| results)
}

/// [`execute`] compiled under `overlay`, calling `after` with the database,
/// the project the run compiled and its results while the database still
/// holds that project: for a plan that stages, the staged copy, which an
/// analysis of the run reads the structure of.
///
/// A plan that stages ([`RunPlan::stages`]) stages a copy of the datamodel,
/// compiles it, and restores the database to the project before returning,
/// whatever the outcome (a panic included, from [`Staging`]'s drop); `after`
/// may change the database's inputs other than the project's contents (a
/// mode flag), and must set them back itself.
pub(crate) fn execute_then<T>(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    plan: &RunPlan,
    overlay: LtmOverlay,
    after: impl FnOnce(&mut SimlinDb, SourceProject, &Results) -> T,
) -> Result<(Results, T), RunFailure> {
    let specs = super::changes::effective_specs(ws.project, model).clone();
    let waiting = ws.waiting;
    if plan.stages(ws.project, &model.name) {
        let staged = staged_project(ws.project, &model.name, plan)?;
        let staging = Staging::new(ws.db, ws.project, &staged);
        let source_project = staging.source_project;
        let results = simulate(
            &mut *staging.db,
            source_project,
            &staged,
            model,
            &specs,
            plan,
            true,
            overlay,
            waiting,
        )?;
        let analysis = after(&mut *staging.db, source_project, &results);
        return Ok((results, analysis));
    }
    let source_project = ws
        .db
        .current_source_project()
        .ok_or_else(|| "the project has not been compiled".to_string())?;
    let results = simulate(
        ws.db,
        source_project,
        ws.project,
        model,
        &specs,
        plan,
        false,
        overlay,
        waiting,
    )?;
    let analysis = after(ws.db, source_project, &results);
    Ok((results, analysis))
}

/// Compile `model` in `project` (the one `source_project` holds, `staged`
/// when it is a staged copy with `plan`'s equations and specs) under
/// `overlay`, and run it under `plan`'s specs and values, a slice at a time.
/// `specs` are the model's own, which `plan`'s override, and what a run's
/// cost is held against.
#[allow(clippy::too_many_arguments)]
fn simulate(
    db: &mut SimlinDb,
    source_project: SourceProject,
    project: &datamodel::Project,
    model: &datamodel::Model,
    specs: &datamodel::SimSpecs,
    plan: &RunPlan,
    staged: bool,
    overlay: LtmOverlay,
    waiting: Option<&(dyn Fn() -> bool + Sync)>,
) -> Result<Results, RunFailure> {
    let own = crate::results::Specs::from(specs);
    let run = crate::results::Specs::from(&plan.specs.applied_to(specs));
    // Specs a plan changed are an agent's; the model's own run as they are.
    if let Some(reason) = no_run(&run).filter(|_| !plan.specs.is_empty()) {
        return Err(RunFailure::from(format!("under its specs, {reason}")));
    }
    let vm = build_vm_under(
        db,
        source_project,
        project,
        &model.name,
        overlay,
        &own,
        Some(run),
    )
    .map_err(|err| match err {
        Unbuilt::Compile(err) if staged => refusal_reason(db, source_project, &model.name, &err),
        Unbuilt::Compile(err) => describe(&err),
        Unbuilt::Cost(reason) => reason,
    })?;
    run_values(vm, plan, waiting)
}

/// Run each of `plans`, which change values only, on one compile of `model`,
/// in parallel where the platform has threads, and give each run's results to
/// `summarize` as it finishes: what a battery of checks, each a run with one
/// value changed, costs one compile for. A run's results are dropped once
/// summarized, so a battery holds the runs it makes at once, no more of them
/// than [`concurrent_runs`] allows, not every run.
///
/// Each run is a unit of the call's work: once other work waits for the
/// project, no run starts, and the batch answers that it was interrupted
/// rather than with the runs it did.
pub(crate) fn execute_values<T: Send>(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    plans: &[RunPlan],
    summarize: impl Fn(&RunPlan, Results) -> T + Sync,
) -> Result<Vec<Result<T, String>>, ToolError> {
    debug_assert!(plans.iter().all(RunPlan::only_sets_values));
    ws.yield_point()?;
    let Some(source_project) = ws.db.current_source_project() else {
        return Ok(plans
            .iter()
            .map(|_| Err("the project has not been compiled".to_string()))
            .collect());
    };
    let build = match crate::queue_compile::compile_sim(
        ws.db,
        source_project,
        ws.project,
        &model.name,
        LtmOverlay::Off,
    ) {
        Ok(build) => build,
        Err(err) => return Ok(plans.iter().map(|_| Err(describe(&err))).collect()),
    };
    let waiting = ws.waiting;
    let stopped = std::sync::atomic::AtomicBool::new(false);
    let run = |plan: &RunPlan| {
        if stopped.load(std::sync::atomic::Ordering::SeqCst) || waiting.is_some_and(|w| w()) {
            stopped.store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(String::new());
        }
        let mut vm = crate::vm::Vm::new(build.compiled.clone()).map_err(|err| describe(&err))?;
        if build.special {
            vm.set_conveyor_plans(build.conveyor_plans.clone());
            vm.set_queue_plans(build.queue_plans.clone());
        }
        match run_values(vm, plan, waiting) {
            Ok(results) => Ok(summarize(plan, results)),
            Err(RunFailure::Failed(reason)) => Err(reason),
            Err(RunFailure::Stopped) => {
                stopped.store(true, std::sync::atomic::Ordering::SeqCst);
                Err(String::new())
            }
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    let outcomes = {
        use rayon::prelude::*;
        let parallel = || plans.par_iter().map(&run).collect();
        let run_bytes = (build.compiled.specs.n_chunks)
            .saturating_mul(build.compiled.n_slots())
            .saturating_mul(std::mem::size_of::<f64>());
        let at_once = concurrent_runs(run_bytes);
        if at_once >= rayon::current_num_threads() {
            parallel()
        } else {
            match rayon::ThreadPoolBuilder::new().num_threads(at_once).build() {
                Ok(pool) => pool.install(parallel),
                // A pool the host will not give runs the batch one at a time.
                Err(_) => plans.iter().map(&run).collect(),
            }
        }
    };
    #[cfg(target_arch = "wasm32")]
    let outcomes = plans.iter().map(run).collect();
    if stopped.into_inner() {
        return Err(ToolError::interrupted());
    }
    Ok(outcomes)
}

/// Run `vm` to its end under `plan`'s values, a slice at a time, stopping
/// between two when `waiting` says other work waits for the project. The
/// VM's specs are the run's.
fn run_values(
    mut vm: crate::vm::Vm,
    plan: &RunPlan,
    waiting: Option<&(dyn Fn() -> bool + Sync)>,
) -> Result<Results, RunFailure> {
    // Values from the start go in before the run, so initial values read
    // them; the others at their times, in order. A value from `t` holds from
    // the first step at or after `t` (`Vm::run_until` stops at it, before it
    // is evaluated): the step an equation's `TIME >= t` first holds at under
    // Euler integration. A time at or before the first step is the start.
    let specs = vm.specs().clone();
    let mut slices = Slices {
        at: 0.0,
        // The run's steps in `RUN_SLICES` parts, and at least a step.
        slice: (specs.final_step() as f64 / RUN_SLICES).max(1.0),
        waiting,
    };
    let takes_effect = |change: &ValueChange| {
        change
            .from_time
            .map(|time| specs.step_at_or_after(time))
            .filter(|&step| step > 0.0)
    };
    let mut timed: Vec<&ValueChange> = plan.values.iter().collect();
    timed.sort_by(|a, b| {
        takes_effect(a)
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&takes_effect(b).unwrap_or(f64::NEG_INFINITY))
    });
    for change in timed {
        if takes_effect(change).is_some()
            && let Some(time) = change.from_time
        {
            slices.run_until(&mut vm, time)?;
        }
        for (key, value) in &change.values {
            vm.set_value(key, *value).map_err(|err| describe(&err))?;
        }
    }
    slices.run_until(&mut vm, specs.stop)?;
    vm.run_to_end().map_err(|err| describe(&err))?;
    Ok(vm.into_results())
}

/// A run taken a slice at a time, counted in steps as the VM counts them:
/// the step count it stands at (`at`), and the steps of a slice (`slice`, at
/// least one). A slice's end is a step count, never a time: where the clock's
/// last place is coarser than a DT, a time names no one step, and a clock
/// that adds a slice's length to a time can stop advancing.
struct Slices<'w> {
    at: f64,
    slice: f64,
    waiting: Option<&'w (dyn Fn() -> bool + Sync)>,
}

impl Slices<'_> {
    /// Run `vm` until `time` (`Vm::run_until`) a slice at a time, stopping
    /// between two slices when other work waits for the project. A run is at
    /// most `RUN_SLICES` slices, whatever its specs.
    fn run_until(&mut self, vm: &mut crate::vm::Vm, time: f64) -> Result<(), RunFailure> {
        // No step past the run's last is taken, however late `time` is.
        let target = vm
            .specs()
            .step_at_or_after(time)
            .min(vm.specs().final_step() as f64 + 1.0);
        while self.at + self.slice < target {
            self.at += self.slice;
            vm.run_until_step(self.at).map_err(|err| describe(&err))?;
            if self.waiting.is_some_and(|waiting| waiting()) {
                return Err(RunFailure::Stopped);
            }
        }
        vm.run_until(time).map_err(|err| describe(&err))?;
        self.at = self.at.max(target);
        Ok(())
    }
}

/// Why a VM was not built.
pub(crate) enum Unbuilt {
    /// The model does not compile.
    Compile(crate::common::Error),
    /// The run would cost more than a run may ([`over_budget`]).
    Cost(String),
}

/// A VM of `model_name` in `project`, compiled on `db` (synced to it) under
/// `overlay` -- unless the run would cost more than a run may, measured
/// against `own`, the model's own specs, before the VM allocates its results.
pub(crate) fn build_vm(
    db: &mut SimlinDb,
    source_project: SourceProject,
    project: &datamodel::Project,
    model_name: &str,
    overlay: LtmOverlay,
    own: &crate::results::Specs,
) -> Result<crate::vm::Vm, Unbuilt> {
    build_vm_under(db, source_project, project, model_name, overlay, own, None)
}

/// [`build_vm`], to run under `run` when given in place of the specs the
/// model compiles with: a plan's specs, on a model whose program does not
/// depend on them ([`RunPlan::stages`] says which).
fn build_vm_under(
    db: &mut SimlinDb,
    source_project: SourceProject,
    project: &datamodel::Project,
    model_name: &str,
    overlay: LtmOverlay,
    own: &crate::results::Specs,
    run: Option<crate::results::Specs>,
) -> Result<crate::vm::Vm, Unbuilt> {
    let build = crate::queue_compile::compile_sim(db, source_project, project, model_name, overlay)
        .map_err(Unbuilt::Compile)?;
    // A model with a conveyor or a queue runs under the specs its expansion
    // read; a plan that changes them staged a copy that has them.
    let run = run
        .filter(|_| !build.special)
        .unwrap_or_else(|| build.compiled.specs.clone());
    if let Some(reason) = over_budget(&run, build.compiled.n_slots(), own) {
        return Err(Unbuilt::Cost(reason));
    }
    let mut vm = crate::vm::Vm::with_specs(build.compiled, run).map_err(Unbuilt::Compile)?;
    if build.special {
        vm.set_conveyor_plans(build.conveyor_plans);
        vm.set_queue_plans(build.queue_plans);
    }
    Ok(vm)
}

/// Why no run can be made under `specs`, or `None`: a start, stop or DT that
/// is no finite number, what both backends refuse (`Specs::refusal`), and a DT
/// longer than the run, which would take no step past its start.
pub(crate) fn no_run(specs: &crate::results::Specs) -> Option<String> {
    let number = crate::results::written;
    let span = specs.stop - specs.start;
    if !(specs.start.is_finite() && specs.stop.is_finite() && specs.dt.is_finite())
        || !span.is_finite()
    {
        return Some(format!(
            "a run from {} to {} by a DT of {} is not one a computer's numbers can take",
            number(specs.start),
            number(specs.stop),
            number(specs.dt)
        ));
    }
    if let Some(reason) = specs.refusal() {
        return Some(reason.to_string());
    }
    if specs.dt > span {
        return Some(format!(
            "a DT of {} is longer than the run, from {} to {}, so the run would take no step",
            number(specs.dt),
            number(specs.start),
            number(specs.stop)
        ));
    }
    None
}

/// Why a run under `specs` of a model with `slots` values per row would cost
/// more than a run may, in numbers and with specs that would fit, or `None`.
///
/// A run's memory is the rows it saves times its slots, held to
/// [`MAX_RUN_VALUES`]; its time is its steps times its slots, held to
/// [`MAX_RUN_STEPS`]. Each limit is at least what the model's own specs
/// (`own`) cost, so the model as it stands always runs, and only a change of
/// specs can be refused.
pub(crate) fn over_budget(
    specs: &crate::results::Specs,
    slots: usize,
    own: &crate::results::Specs,
) -> Option<String> {
    let slots = slots.max(1);
    let steps = |specs: &crate::results::Specs| specs.final_step() as f64;
    let span = specs.stop - specs.start;
    let slots_f = slots as f64;
    let save = specs.save_step.max(specs.dt);
    let values = specs.n_chunks as f64 * slots_f;
    let values_limit = (MAX_RUN_VALUES as f64).max(own.n_chunks as f64 * slots_f);
    // A stop time to suggest: one after the start, which three digits can
    // round down to the start itself (a run from 1e18), and then none.
    let stop_at_most = |stop: f64| {
        let stop = rounded_down(stop);
        (stop > specs.start).then(|| format!("a stop time of at most {}", written(stop)))
    };
    if values > values_limit {
        let rows = (values_limit / slots_f).floor().max(2.0);
        let longer = written(rounded_up(span / (rows - 1.0)));
        let stop = stop_at_most(specs.start + (rows - 1.0) * save);
        let alternatives = match (save <= specs.dt, stop) {
            (true, Some(stop)) => {
                format!("a DT of at least {longer} (the run saves every step), or {stop}")
            }
            (true, None) => format!("a DT of at least {longer} (the run saves every step)"),
            (false, Some(stop)) => stop,
            // A DT that long saves every step and computes no more than it
            // saves, so it fits where a longer save step alone would not.
            (false, None) => format!("a DT of at least {longer}"),
        };
        // A count past counting has no product to give.
        let saved = if specs.n_chunks == usize::MAX {
            format!("{}, each of {slots} values", written_rows(specs.n_chunks))
        } else {
            format!(
                "{} of {slots} values, {} numbers",
                written_rows(specs.n_chunks),
                written_count(values)
            )
        };
        return Some(format!(
            "the run would save {saved}, more than the {} a run may hold; {alternatives} \
             would fit",
            written_count(values_limit)
        ));
    }
    let computed = steps(specs) * slots_f;
    let steps_limit = (MAX_RUN_STEPS as f64).max(steps(own) * slots_f);
    if computed > steps_limit {
        let fit = (steps_limit / slots_f).floor().max(1.0);
        let dt = format!("a DT of at least {}", written(rounded_up(span / fit)));
        let alternatives = match stop_at_most(specs.start + fit * specs.dt) {
            Some(stop) => format!("{dt}, or {stop},"),
            None => dt,
        };
        return Some(format!(
            "the run would take {} steps of {slots} values, {} in all, more than the {} a run \
             may compute; {alternatives} would fit",
            written_count(steps(specs)),
            written_count(computed),
            written_count(steps_limit),
        ));
    }
    None
}

/// `x` rounded up to three significant digits, for a bound a suggestion must
/// meet.
fn rounded_up(x: f64) -> f64 {
    three_digits(x, f64::ceil)
}

/// `x` rounded down to three significant digits.
fn rounded_down(x: f64) -> f64 {
    three_digits(x, f64::floor)
}

/// `x` to three significant digits, its fourth and later taken off by
/// `whole`, so the answer prints with three digits. The digits are found by
/// scaling by a power of ten (multiplied or divided as its sign needs: `114 /
/// 1e-5` is `11399999.999999998`, and `114 * 1e5` is `11400000`), and the
/// answer is read back from the decimal they spell, which makes it the float
/// nearest that decimal at any magnitude: past `1e22` a power of ten is no
/// float, and scaling back by one would leave digits past the third.
fn three_digits(x: f64, whole: fn(f64) -> f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    let last = x.abs().log10().floor() as i32 - 2;
    let unit = 10f64.powi(last.abs());
    let digits = if last >= 0 {
        whole(x / unit)
    } else {
        whole(x * unit)
    };
    format!("{digits}e{last}").parse().unwrap_or(x)
}

/// The host's database staged on a copy of its project, for a run that
/// changes what compiles; restored to the project when dropped, whatever the
/// run did, a panic included.
pub(crate) struct Staging<'a> {
    pub(crate) db: &'a mut SimlinDb,
    project: &'a datamodel::Project,
    prev: Option<Option<crate::db::PersistentSyncState>>,
    pub(crate) source_project: SourceProject,
}

impl<'a> Staging<'a> {
    pub(crate) fn new(
        db: &'a mut SimlinDb,
        project: &'a datamodel::Project,
        staged: &datamodel::Project,
    ) -> Staging<'a> {
        let (source_project, prev) = db.sync_staged(staged);
        Staging {
            db,
            project,
            prev: Some(prev),
            source_project,
        }
    }
}

impl Drop for Staging<'_> {
    fn drop(&mut self) {
        if let Some(prev) = self.prev.take() {
            self.db.restore(self.project, prev);
        }
    }
}

/// `project` with `plan`'s equations and specs applied to `model_name`.
fn staged_project(
    project: &datamodel::Project,
    model_name: &str,
    plan: &RunPlan,
) -> Result<datamodel::Project, String> {
    let mut staged = project.clone();
    let model = staged
        .models
        .iter_mut()
        .find(|m| m.name == model_name)
        .ok_or_else(|| format!("the project has no model named '{model_name}'"))?;
    apply_equations(model, plan)?;
    if !plan.specs.is_empty() {
        let specs = match &mut model.sim_specs {
            Some(specs) => specs,
            None => &mut staged.sim_specs,
        };
        *specs = plan.specs.applied_to(specs);
    }
    Ok(staged)
}

/// `plan`'s replacement equations applied to `model`: the model as a run of
/// the plan compiles it, and so as every reader of that run's structure
/// must read it (`series::scale_in_run`).
pub(crate) fn apply_equations(model: &mut datamodel::Model, plan: &RunPlan) -> Result<(), String> {
    for change in &plan.equations {
        let var = model.get_variable_mut(&change.variable).ok_or_else(|| {
            format!(
                "the model has no variable '{}'",
                super::evidence::echo(&change.variable)
            )
        })?;
        replace_equation(var, &change.replacement);
    }
    Ok(())
}

/// Replace a variable's value, keeping its dimensions: one equation applies
/// to every element of an arrayed variable, and per-element equations to
/// theirs. A stock's equation is its initial value. A variable whose equation
/// feeds a table (the value is the table at the equation's value) loses its
/// table: the replacement is the value, as a knockout that holds an "effect
/// of" at 1 means it.
pub(crate) fn replace_equation(var: &mut Variable, replacement: &Replacement) {
    let replaced = |old: &Equation| match (replacement, old) {
        (Replacement::Equation(text), Equation::Scalar(_)) => Equation::Scalar(text.clone()),
        (
            Replacement::Equation(text),
            Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..),
        ) => Equation::ApplyToAll(dims.clone(), text.clone()),
        (
            Replacement::Elements(elements),
            Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..),
        ) => Equation::Arrayed(
            dims.clone(),
            elements
                .iter()
                .map(|(element, text)| (element.clone(), text.clone(), None, None))
                .collect(),
            None,
            false,
        ),
        // A scalar is its one element.
        (Replacement::Elements(elements), Equation::Scalar(_)) => Equation::Scalar(
            elements
                .first()
                .map(|(_, text)| text.clone())
                .unwrap_or_default(),
        ),
    };
    match var {
        Variable::Stock(stock) => stock.equation = replaced(&stock.equation),
        Variable::Flow(flow) => {
            flow.equation = replaced(&flow.equation);
            flow.gf = None;
        }
        Variable::Aux(aux) => {
            aux.equation = replaced(&aux.equation);
            aux.gf = None;
        }
        Variable::Module(_) => {}
    }
}

/// Whether replacing `var`'s equation drops a table its value is read from:
/// its own, or an element's.
pub(crate) fn has_table(var: &Variable) -> bool {
    let element_table = |equation: &Equation| matches!(equation, Equation::Arrayed(_, elements, ..) if elements.iter().any(|e| e.3.is_some()));
    match var {
        Variable::Flow(flow) => flow.gf.is_some() || element_table(&flow.equation),
        Variable::Aux(aux) => aux.gf.is_some() || element_table(&aux.equation),
        Variable::Stock(_) | Variable::Module(_) => false,
    }
}

/// Why a staged plan does not compile: the errors the staged model's
/// diagnostics report, else the build's own error.
fn refusal_reason(
    db: &SimlinDb,
    source_project: SourceProject,
    model_name: &str,
    err: &crate::common::Error,
) -> String {
    let model = crate::canonicalize(model_name);
    let reasons: Vec<String> = collect_all_diagnostics(db, source_project, LtmOverlay::Off)
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Error
                && (d.model.is_empty() || crate::canonicalize(&d.model) == model)
        })
        .map(|d| {
            let variable = d.owner.as_deref().or(d.variable.as_deref());
            let reason = d.reason().unwrap_or_else(|| d.code().description());
            match variable {
                Some(variable) => format!("{variable}: {reason}"),
                None => reason.to_string(),
            }
        })
        .take(5)
        .collect();
    if reasons.is_empty() {
        describe(err)
    } else {
        reasons.join("; ")
    }
}

/// An engine error in words: the raising site's reason, else what its code
/// means ([`crate::common::Error::reason`]).
fn describe(err: &crate::common::Error) -> String {
    err.reason().to_string()
}

#[cfg(test)]
#[path = "runs_tests.rs"]
mod tests;
