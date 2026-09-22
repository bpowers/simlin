// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Runs: the model as it is ("current"), and the experiments a session keeps
//! under names.
//!
//! A run is its [`RunPlan`] -- what it changed from the model -- and the
//! results that plan produced at a revision. A plan is data: values set on
//! constants (each from a time on, or from the start), replacement equations,
//! and run specs. Values are VM overrides, applied without recompiling; a
//! replacement equation or a spec change stages a copy of the datamodel on the
//! host's database (`SimlinDb::sync_staged`), compiles and runs it, and
//! restores (`SimlinDb::restore`) exactly as a dry-run patch does, so the
//! project is never changed and unchanged variables keep their compiled
//! fragments.
//!
//! A run is fresh while the model has what the run simulated: the project as
//! a simulation reads it, without its diagrams, notes and units
//! ([`simulation_key`]). The revision alone cannot say, since a layout edit
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
//! the agent's `list_runs` does.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use buffa::Message;
use indexmap::IndexMap;
#[cfg(feature = "schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Canonical, Ident};
use crate::datamodel::{self, Equation, Variable};
use crate::db::{DiagnosticSeverity, LtmOverlay, SimlinDb, SourceProject, collect_all_diagnostics};
use crate::results::Results;

use super::evidence::{QUOTE_CHARS, window};
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

/// How many slices a run is taken in. Between two, it asks whether other work
/// waits for the project, and stops if so: that work waits at most a slice
/// of a run, not the whole of a long one.
const RUN_SLICES: f64 = 16.0;

/// Why a run did not finish: the reason, in words, or other work that waits
/// for the project, which a run stops for between its slices.
#[derive(Debug)]
pub(crate) enum RunFailure {
    Failed(String),
    Stopped,
}

impl From<String> for RunFailure {
    fn from(reason: String) -> RunFailure {
        RunFailure::Failed(reason)
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
    pub equation: String,
}

/// Run specs a plan changes; `None` keeps the model's.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Default, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct SpecsChange {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<IntegrationMethod>,
}

impl SpecsChange {
    fn is_empty(&self) -> bool {
        self == &SpecsChange::default()
    }

    /// `self`'s changes over `base`'s.
    pub(crate) fn over(&self, base: &SpecsChange) -> SpecsChange {
        SpecsChange {
            start: self.start.or(base.start),
            stop: self.stop.or(base.stop),
            dt: self.dt.or(base.dt),
            method: self.method.or(base.method),
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
    /// `base`'s plan with `self`'s changes after it: a change of a variable
    /// `self` also changes is replaced, not compounded, and `self`'s specs
    /// override `base`'s.
    pub(crate) fn over(&self, base: &RunPlan) -> RunPlan {
        let changed = |variable: &str| {
            self.values.iter().any(|c| c.variable == variable)
                || self.equations.iter().any(|c| c.variable == variable)
        };
        let mut values: Vec<ValueChange> = base
            .values
            .iter()
            .filter(|c| !changed(&c.variable))
            .cloned()
            .collect();
        values.extend(self.values.iter().cloned());
        let mut equations: Vec<EquationChange> = base
            .equations
            .iter()
            .filter(|c| !changed(&c.variable))
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
}

impl Run {
    /// The saved times.
    pub(crate) fn times(&self) -> Vec<f64> {
        self.series(crate::results::TIME_OFF)
    }

    /// The series at a results offset, over the rows the run saved.
    pub(crate) fn series(&self, offset: usize) -> Vec<f64> {
        self.results
            .iter()
            .take(self.saved_rows())
            .map(|row| row[offset])
            .collect()
    }

    /// How many rows the run saved: the rows up to the first whose time goes
    /// back. A save step that is not a multiple of DT leaves the results'
    /// last rows unwritten, at time zero, and they are no part of the run.
    pub(crate) fn saved_rows(&self) -> usize {
        let mut previous = f64::NEG_INFINITY;
        self.results
            .iter()
            .position(|row| {
                let time = row[crate::results::TIME_OFF];
                let back = time < previous;
                previous = time;
                back
            })
            .unwrap_or(self.results.step_count)
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

    /// The bytes its results hold.
    fn bytes(&self) -> usize {
        self.results.data.len() * std::mem::size_of::<f64>()
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
    /// An arrayed constant's value, element by element.
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

/// One element's value in a listed change.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct ListedElement {
    /// The element's subscript, as a results key spells it (`north`, `a,b`).
    pub element: String,
    pub value: f64,
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
                            value: *value,
                        })
                        .collect()
                },
                equation: None,
                table_dropped: false,
                from_time: change.from_time,
            }
        });
        let equations = self.equations.iter().map(|change| ListedChange {
            variable: name(&change.variable),
            value: None,
            elements: vec![],
            equation: Some(change.equation.clone()),
            table_dropped: variable(&change.variable).is_some_and(has_table),
            from_time: None,
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
        runs: session.runs.listing(ws, Some(model)),
        omitted: vec![],
    };
    let fits = |output: &ListRunsOutput| {
        serde_json::to_string(output).map_or(0, |json| json.len()) <= session.outline_budget
    };
    if !fits(&output) {
        for change in output.runs.iter_mut().flat_map(|run| &mut run.changes) {
            if let Some(equation) = &mut change.equation {
                *equation = window(equation, 0, 0, QUOTE_CHARS);
            }
        }
    }
    while !fits(&output) && !output.runs.is_empty() {
        let oldest = output.runs.remove(0);
        output.omitted.push(oldest.name);
    }
    Ok(output)
}

/// The project as a simulation reads it, as a number: equal for two projects
/// that differ only in their diagrams, sectors, provenance, source file, or
/// their variables' documentation and units, and (but for a hash collision)
/// different otherwise.
///
/// Units are checked, never simulated, so a fix of a variable's units, like
/// an edit of its notes, stales no run: nothing a run is kept fresh for reads
/// either.
pub(crate) fn simulation_key(project: &datamodel::Project) -> u64 {
    let mut stripped = project.clone();
    stripped.source = None;
    stripped.ai_information = None;
    for model in &mut stripped.models {
        model.views.clear();
        model.groups.clear();
        // Only a variable with something to strip is copied out of the
        // sharing.
        model.variables.edit_where(
            |var| {
                var.get_units().is_some()
                    || var.get_ai_state().is_some()
                    || !documentation(var).is_empty()
            },
            |var| {
                var.set_units("");
                var.set_documentation("");
                match var {
                    Variable::Stock(v) => v.ai_state = None,
                    Variable::Flow(v) => v.ai_state = None,
                    Variable::Aux(v) => v.ai_state = None,
                    Variable::Module(v) => v.ai_state = None,
                }
            },
        );
    }
    let bytes = crate::serde::serialize(&stripped)
        .map(|p| p.encode_to_vec())
        .unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// A variable's documentation, its notes.
fn documentation(var: &Variable) -> &str {
    match var {
        Variable::Stock(v) => &v.documentation,
        Variable::Flow(v) => &v.documentation,
        Variable::Aux(v) => &v.documentation,
        Variable::Module(v) => &v.documentation,
    }
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
        match self.key {
            Some((revision, key)) if revision == ws.revision => key,
            _ => {
                let key = simulation_key(ws.project);
                self.key = Some((ws.revision, key));
                key
            }
        }
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
        let run = Arc::new(Run {
            name: CURRENT.to_string(),
            revision: ws.revision,
            key,
            plan: RunPlan::default(),
            results,
        });
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
                return Err(ToolError::new(format!("there is no run named '{name}'"))
                    .with_suggestions(names));
            }
        };
        ws.yield_point()?;
        let results = execute(ws, model, &plan).map_err(|failure| {
            failure.refusal(|reason| {
                ToolError::new(format!("run '{name}' does not run again: {reason}"))
            })
        })?;
        let run = Arc::new(Run {
            name: name.to_string(),
            revision,
            key,
            plan,
            results,
        });
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

    /// Every named run, oldest first, as a host lists them, with the
    /// variables named as `model` spells them.
    pub(crate) fn listing(
        &mut self,
        ws: &Workspace<'_>,
        model: Option<&datamodel::Model>,
    ) -> Vec<RunListing> {
        let key = self.key(ws);
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
///
/// A plan with equation or spec changes stages a copy of the datamodel,
/// compiles it, and restores the database to the project before returning,
/// whatever the outcome.
pub(crate) fn execute(
    ws: &mut Workspace<'_>,
    model: &datamodel::Model,
    plan: &RunPlan,
) -> Result<Results, RunFailure> {
    let waiting = ws.waiting;
    let staged_needed = !plan.equations.is_empty() || !plan.specs.is_empty();
    let own = crate::results::Specs::from(super::changes::effective_specs(ws.project, model));
    let mut vm = if staged_needed {
        let staged = staged_project(ws.project, &model.name, plan)?;
        let staging = Staging::new(ws.db, ws.project, &staged);
        let source_project = staging.source_project;
        let built = build_vm(
            &mut *staging.db,
            source_project,
            &staged,
            &model.name,
            LtmOverlay::Off,
            &own,
        );
        built.map_err(|err| match err {
            Unbuilt::Compile(err) => refusal_reason(staging.db, source_project, &model.name, &err),
            Unbuilt::Cost(reason) => reason,
        })?
    } else {
        let source_project = ws
            .db
            .current_source_project()
            .ok_or_else(|| "the project has not been compiled".to_string())?;
        build_vm(
            ws.db,
            source_project,
            ws.project,
            &model.name,
            LtmOverlay::Off,
            &own,
        )
        .map_err(|err| match err {
            Unbuilt::Compile(err) => describe(&err),
            Unbuilt::Cost(reason) => reason,
        })?
    };

    // Values from the start go in before the run, so initial values read
    // them; the others at their times, in order. A value from `t` holds from
    // the first step at or after `t`, the step `IF TIME >= t` turns on at.
    // `run_to(t)` evaluates the step at `t` before it stops, so the value is
    // set after running to half a step before that step: the clock then
    // stands at it, and its flows read the new value.
    let specs = super::changes::effective_specs(ws.project, model);
    let start = plan.specs.start.unwrap_or(specs.start);
    let stop = plan.specs.stop.unwrap_or(specs.stop);
    let dt = plan
        .specs
        .dt
        .unwrap_or_else(|| super::outline::dt_value(&specs.dt));
    let mut slices = Slices {
        at: start,
        slice: (stop - start) / RUN_SLICES,
        waiting,
    };
    let takes_effect = |change: &ValueChange| {
        change
            .from_time
            .filter(|&time| time > start)
            .map(|time| first_step_at_or_after(start, dt, time))
    };
    let mut timed: Vec<&ValueChange> = plan.values.iter().collect();
    timed.sort_by(|a, b| {
        takes_effect(a)
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&takes_effect(b).unwrap_or(f64::NEG_INFINITY))
    });
    for change in timed {
        if let Some(time) = takes_effect(change) {
            slices.run_to(&mut vm, time - dt / 2.0)?;
        }
        for (key, value) in &change.values {
            vm.set_value(key, *value).map_err(|err| describe(&err))?;
        }
    }
    slices.run_to(&mut vm, stop)?;
    vm.run_to_end().map_err(|err| describe(&err))?;
    Ok(vm.into_results())
}

/// A run taken a slice at a time, from where it stands (`at`).
struct Slices<'w> {
    at: f64,
    slice: f64,
    waiting: Option<&'w (dyn Fn() -> bool + Sync)>,
}

impl Slices<'_> {
    /// Run `vm` to `time` a slice at a time, stopping between two slices
    /// when other work waits for the project.
    fn run_to(&mut self, vm: &mut crate::vm::Vm, time: f64) -> Result<(), RunFailure> {
        while self.slice > 0.0 && self.at + self.slice < time {
            self.at += self.slice;
            vm.run_to(self.at).map_err(|err| describe(&err))?;
            if self.waiting.is_some_and(|waiting| waiting()) {
                return Err(RunFailure::Stopped);
            }
        }
        vm.run_to(time).map_err(|err| describe(&err))?;
        self.at = self.at.max(time);
        Ok(())
    }
}

/// Why a VM was not built.
enum Unbuilt {
    /// The model does not compile.
    Compile(crate::common::Error),
    /// The run would cost more than a run may ([`over_budget`]).
    Cost(String),
}

/// A VM of `model_name` in `project`, compiled on `db` (synced to it) under
/// `overlay` -- unless the run would cost more than a run may, measured
/// against `own`, the model's own specs, before the VM allocates its results.
fn build_vm(
    db: &mut SimlinDb,
    source_project: SourceProject,
    project: &datamodel::Project,
    model_name: &str,
    overlay: LtmOverlay,
    own: &crate::results::Specs,
) -> Result<crate::vm::Vm, Unbuilt> {
    let build = crate::queue_compile::compile_sim(db, source_project, project, model_name, overlay)
        .map_err(Unbuilt::Compile)?;
    if let Some(reason) = over_budget(&build.compiled.specs, build.compiled.n_slots(), own) {
        return Err(Unbuilt::Cost(reason));
    }
    let mut vm = crate::vm::Vm::new(build.compiled).map_err(Unbuilt::Compile)?;
    if build.special {
        vm.set_conveyor_plans(build.conveyor_plans);
        vm.set_queue_plans(build.queue_plans);
    }
    Ok(vm)
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
    let steps = |specs: &crate::results::Specs| ((specs.stop - specs.start) / specs.dt).ceil();
    let span = specs.stop - specs.start;
    let slots_f = slots as f64;
    let save = specs.save_step.max(specs.dt);
    let values = specs.n_chunks as f64 * slots_f;
    let values_limit = (MAX_RUN_VALUES as f64).max(own.n_chunks as f64 * slots_f);
    if values > values_limit {
        let rows = (values_limit / slots_f).floor().max(2.0);
        let stop = specs.start + (rows - 1.0) * save;
        let alternatives = if save <= specs.dt {
            format!(
                "a DT of at least {} (the run saves every step), or a stop time of at most {}",
                rounded_up(span / (rows - 1.0)),
                rounded_down(stop)
            )
        } else {
            format!("a stop time of at most {}", rounded_down(stop))
        };
        return Some(format!(
            "the run would save {} rows of {slots} values, {} numbers, more than the {} a run \
             may hold; {alternatives} would fit",
            specs.n_chunks, values as u64, values_limit as u64
        ));
    }
    let computed = steps(specs) * slots_f;
    let steps_limit = (MAX_RUN_STEPS as f64).max(steps(own) * slots_f);
    if computed > steps_limit {
        let fit = (steps_limit / slots_f).floor().max(1.0);
        return Some(format!(
            "the run would take {} steps of {slots} values, {} in all, more than the {} a run \
             may compute; a DT of at least {}, or a stop time of at most {}, would fit",
            steps(specs) as u64,
            computed as u64,
            steps_limit as u64,
            rounded_up(span / fit),
            rounded_down(specs.start + fit * specs.dt)
        ));
    }
    None
}

/// `x` rounded up to three significant digits, for a bound a suggestion must
/// meet.
fn rounded_up(x: f64) -> f64 {
    if x == 0.0 {
        return x;
    }
    let scale = 10f64.powi(2 - x.abs().log10().floor() as i32);
    (x * scale).ceil() / scale
}

/// `x` rounded down to three significant digits.
fn rounded_down(x: f64) -> f64 {
    if x == 0.0 {
        return x;
    }
    let scale = 10f64.powi(2 - x.abs().log10().floor() as i32);
    (x * scale).floor() / scale
}

/// The first step of a run from `start` in steps of `dt` at or after `time`.
/// A time within a millionth of a step of one is that step, so a time written
/// as a step's (5.1 with a DT of 0.1) is not moved past it by rounding.
pub(crate) fn first_step_at_or_after(start: f64, dt: f64, time: f64) -> f64 {
    let steps = ((time - start) / dt - 1e-6).ceil().max(0.0);
    start + steps * dt
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
    for change in &plan.equations {
        let var = model
            .get_variable_mut(&change.variable)
            .ok_or_else(|| format!("the model has no variable '{}'", change.variable))?;
        replace_equation(var, &change.equation);
    }
    if !plan.specs.is_empty() {
        let specs = match &mut model.sim_specs {
            Some(specs) => specs,
            None => &mut staged.sim_specs,
        };
        if let Some(start) = plan.specs.start {
            specs.start = start;
        }
        if let Some(stop) = plan.specs.stop {
            specs.stop = stop;
        }
        if let Some(dt) = plan.specs.dt {
            specs.dt = datamodel::Dt::Dt(dt);
            // A save step finer than the new DT is one the run cannot keep.
            if let Some(save) = &specs.save_step
                && super::outline::dt_value(save) < dt
            {
                specs.save_step = None;
            }
        }
        if let Some(method) = plan.specs.method {
            specs.sim_method = match method {
                IntegrationMethod::Euler => datamodel::SimMethod::Euler,
                IntegrationMethod::Rk2 => datamodel::SimMethod::RungeKutta2,
                IntegrationMethod::Rk4 => datamodel::SimMethod::RungeKutta4,
            };
        }
    }
    Ok(staged)
}

/// Replace a variable's value, keeping its dimensions: an arrayed variable's
/// replacement applies to every element. A stock's equation is its initial
/// value. A variable whose equation feeds a table (the value is the table at
/// the equation's value) loses its table: the replacement is the value, as a
/// knockout that holds an "effect of" at 1 means it.
pub(crate) fn replace_equation(var: &mut Variable, equation: &str) {
    let replaced = |old: &Equation| match old {
        Equation::Scalar(_) => Equation::Scalar(equation.to_string()),
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => {
            Equation::ApplyToAll(dims.clone(), equation.to_string())
        }
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
