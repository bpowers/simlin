// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The agent tool surface: the tools an agent uses to read a model, with their
//! semantics stated once so every host that mounts them answers alike.
//! libsimlin mounts them for native hosts (`simlin_tool_session_*`).
//!
//! A host calls a tool by name with JSON input through a [`Session`] bound to
//! one model and gets JSON back ([`ToolOutput`]); [`catalog_json`] describes
//! every tool -- its name, what it is for, the JSON Schema of its input and of
//! its output, and its effect. The design, including the tools that build on
//! these, is `docs/design-plans/2026-09-20-agent-tool-surface.md`.
//!
//! Rules every tool keeps:
//!
//! - **Bounded, quiet results.** No tool returns a whole model or a raw
//!   series, and an outline that would exceed its budget is outlined by
//!   sector instead. Success says little.
//! - **Ids a claim can cite.** A diagnostic or a loop is reported under an id
//!   that is stable for the life of the session (`evidence`), so an agent can
//!   refer to "D3" or "L2" across calls and a verifier can check what it
//!   names.
//! - **Edits are plans.** No tool changes the project: `edit_model` returns a
//!   plan the gate passed, which the host lands, once the person approves,
//!   with `Session::land_plan`: a plan lands by construction, on the project
//!   as it is then.
//! - **Refusals are output.** A domain failure -- an unknown variable, input
//!   that does not match the schema -- is a [`ToolOutput`] with `is_error` set,
//!   naming the rule and the repair, for the agent to read and answer. Only a
//!   host's misuse (a tool name the catalog does not list) is an `Err`.
//! - **One owner per decision.** Names resolve through `names`, diagnostics
//!   are reported through `evidence`, and a model's causal links come from
//!   [`crate::analysis::model_links`].
//! - **A person's work comes first.** A call that other work on the project
//!   waits for -- an edit, a run of the model, a read of its diagnostics --
//!   stops between units
//!   of its own work ([`Workspace::waiting`]) and answers that it kept
//!   nothing, so that work waits at most one unit, and the agent calls again.
//!   A call its host cancels stops at the same points
//!   ([`Workspace::cancelled`]), and is not called again.

mod battery;
mod behavior;
mod catalog;
mod changes;
mod edit;
mod evidence;
mod experiment;
mod loops;
mod names;
mod outline;
mod runs;
mod series;
mod variables;
mod verify;

pub use battery::{
    Condition, Difference, Outcome, Problem, ProblemKind, Response, RunTestsInput, RunTestsOutput,
    TestName, TestResult, TestSummary, TimeConstantEvidence,
};
pub use behavior::{BehaviorMode, Damping, Direction, ModeKind, classify};
#[cfg(feature = "schema")]
pub use catalog::generate_catalog_json;
pub use catalog::{ToolEffect, ToolName, catalog_json};
pub use changes::{ChangedField, ChangedVariable, Changes};
pub use edit::{
    ChangeAction, EditModelInput, EditModelOutput, EditOperation, Landing, LookupShape,
    PlannedChange, PlannedDiagnostic, Verdict,
};
pub use evidence::{DiagnosticCategoryName, DiagnosticReport, Severity};
pub use experiment::{
    AppliedChange, ChangeInput, Comparison, ElementValue, RunExperimentInput, RunExperimentOutput,
    SpecsInput,
};
pub use loops::{
    AnalyzeLoopsInput, AnalyzeLoopsOutput, ChainLink, CutLink, CutReport, DominanceSpan, LoopBasis,
    LoopPolarityName, LoopReport, LoopShare, OmittedLoops, PartitionReport,
};
pub use outline::{
    ConstantOutline, Counts, IntegrationMethod, LookupOutline, LookupSummary, ModuleInputOutline,
    ModuleOutline, Omitted, ReadModelInput, ReadModelOutput, SectorOutline, SpecsOutline,
    StockOutline, VariableOutline,
};
pub use runs::{
    ListRunsInput, ListRunsOutput, ListedChange, ListedElement, RunListing, SpecsChange,
};
pub use series::{
    LeftOut, OmittedElements, Point, ReadBehaviorInput, ReadBehaviorOutput, SeriesCore,
    SeriesSummary, StaleRun,
};
pub use variables::{
    ElementEquation, FindVariablesInput, FindVariablesOutput, LinkPolarityName, LinkRef, Lookup,
    LookupKind, ModuleRecord, NotFound, ReadVariablesInput, ReadVariablesOutput, VariableKind,
    VariableMatch, VariableRecord,
};
pub use verify::{
    Citation, CitationFailure, Finding, FindingKind, FindingVerdict, Relation, VerifyFindingsInput,
    VerifyFindingsOutput,
};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::datamodel;
use crate::db::{SimlinDb, SourceModel, SourceProject};

/// What a tool call reads: a project, the salsa database synced to it, and the
/// project's revision.
///
/// `db` must have been synced to `project` with [`SimlinDb::sync`], so that
/// `db.current_source_project()` names it. `revision` is the host's statement
/// of which contents these are: two calls that pass the same revision must pass
/// the same contents (libsimlin's `ProjectContents` counts every mutation; a
/// host with no counter passes a hash of what it read).
pub struct Workspace<'a> {
    pub project: &'a datamodel::Project,
    pub db: &'a mut SimlinDb,
    pub revision: u64,
    /// Whether other work on the project waits for the call to release the
    /// database: a person's edit or undo, a run of the model as it is, a
    /// host's read of its diagnostics or its loops. A call
    /// asks between units of its work -- a slice of a simulation, a stage of an analysis
    /// -- and stops there, keeping nothing, so that work waits at most one
    /// unit. `None` for a host whose calls never share the project.
    pub waiting: Option<&'a (dyn Fn() -> bool + Sync)>,
    /// Whether the host has cancelled the call: what it was for is gone, as
    /// when the person closes the window it answers. A call stops for it
    /// where it stops for `waiting`, keeping nothing, and answers that it was
    /// cancelled, which no host calls again. `None` for a host that never
    /// cancels a call.
    pub cancelled: Option<&'a (dyn Fn() -> bool + Sync)>,
}

impl Workspace<'_> {
    /// Whether other work waits for the project ([`Workspace::waiting`]).
    pub(crate) fn is_waited_on(&self) -> bool {
        self.waiting.is_some_and(|waiting| waiting())
    }

    /// Stop the call here, keeping nothing, when other work waits for the
    /// project: what a call checks between units of its work.
    pub(crate) fn yield_point(&self) -> Result<(), ToolError> {
        if self.is_waited_on() {
            Err(ToolError::interrupted())
        } else {
            Ok(())
        }
    }
}

/// A tool's answer: JSON, and whether it is a refusal for the agent to read and
/// repair rather than a result.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub json: String,
    pub is_error: bool,
    /// That the call stopped for other work on the project and kept nothing
    /// ([`Workspace::waiting`]): a refusal a host that retries by itself
    /// looks for.
    pub interrupted: bool,
    /// That the host cancelled the call, which stopped and kept nothing
    /// ([`Workspace::cancelled`]): a refusal no host retries.
    pub cancelled: bool,
}

/// A tool name the catalog does not list: the host's mistake, not the agent's,
/// since a host calls only the tools it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTool(pub String);

impl std::fmt::Display for UnknownTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "no tool named '{}' in the catalog", self.0)
    }
}

impl std::error::Error for UnknownTool {}

/// The outline budget, in bytes of the outline's JSON: roughly 3,000 tokens.
/// An outline over it is outlined by sector instead.
pub(crate) const OUTLINE_BUDGET: usize = 12_000;

/// One agent's work on one model: the evidence ids it has been given, what it
/// last read, and the outline budget.
///
/// A session owns nothing of the project's, so a host can keep one per
/// conversation and discard it freely. Its ids are stable for its own life,
/// and mean nothing to another session.
pub struct Session {
    model_name: String,
    evidence: evidence::Evidence,
    last_read: Option<changes::ReadSnapshot>,
    runs: runs::RunStore,
    plans: edit::PlanStore,
    checks: battery::CheckLog,
    outline_budget: usize,
}

impl Session {
    /// A session over the project's model named `model_name`; `"main"` names
    /// the project's first model when none is called that.
    pub fn new(model_name: &str) -> Session {
        Session {
            model_name: model_name.to_string(),
            evidence: evidence::Evidence::default(),
            last_read: None,
            runs: runs::RunStore::default(),
            plans: edit::PlanStore::default(),
            checks: battery::CheckLog::default(),
            outline_budget: OUTLINE_BUDGET,
        }
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Answer a call of `tool` with `input` (JSON; empty means `{}`).
    ///
    /// Every domain failure is an `Ok` output with `is_error` set; `Err` means
    /// the catalog has no such tool.
    pub fn call(
        &mut self,
        ws: Workspace<'_>,
        tool: &str,
        input: &str,
    ) -> Result<ToolOutput, UnknownTool> {
        let Some(name) = ToolName::from_name(tool) else {
            return Err(UnknownTool(tool.to_string()));
        };
        // A call that stops keeps nothing, the ids it gave out on the way
        // included.
        let evidence = self.evidence.clone();
        let (output, cancelled) = checkpointed(ws, |ws| self.answer(ws, name, input));
        if output.interrupted || cancelled {
            self.evidence = evidence;
        }
        Ok(if cancelled {
            ToolOutput::refusal(&ToolError::cancelled())
        } else {
            output
        })
    }

    /// Answer a call of the tool `name`.
    fn answer(&mut self, ws: &mut Workspace<'_>, name: ToolName, input: &str) -> ToolOutput {
        if let Err(interrupted) = ws.yield_point() {
            return ToolOutput::refusal(&interrupted);
        }
        match name {
            ToolName::ReadModel => {
                respond(name, input, |input| outline::read_model(self, ws, input))
            }
            ToolName::ReadVariables => respond(name, input, |input| {
                variables::read_variables(self, ws, input)
            }),
            ToolName::FindVariables => respond(name, input, |input| {
                variables::find_variables(ws, &self.model_name, input)
            }),
            ToolName::RunExperiment => respond(name, input, |input| {
                experiment::run_experiment(self, ws, input)
            }),
            ToolName::ReadBehavior => {
                respond(name, input, |input| series::read_behavior(self, ws, input))
            }
            ToolName::ListRuns => respond(name, input, |input| runs::list_runs(self, ws, input)),
            ToolName::AnalyzeLoops => {
                respond(name, input, |input| loops::analyze_loops(self, ws, input))
            }
            ToolName::RunTests => respond(name, input, |input| battery::run_tests(self, ws, input)),
            ToolName::EditModel => respond(name, input, |input| edit::edit_model(self, ws, input)),
            ToolName::VerifyFindings => respond(name, input, |input| {
                verify::verify_findings(self, ws, input)
            }),
        }
    }

    /// The results of the run named `name` -- `"current"` for the model as
    /// it is -- for a host to chart: every saved series, not a summary, with
    /// the revision the run was made at and whether the model has changed
    /// since (its diagrams aside). A read that stops for other work on the
    /// project says so ([`RunUnavailable::interrupted`]), as does one its host
    /// cancelled ([`RunUnavailable::cancelled`]).
    pub fn run_results(
        &mut self,
        ws: Workspace<'_>,
        name: &str,
    ) -> Result<RunResults, RunUnavailable> {
        let (results, cancelled) = checkpointed(ws, |ws| {
            let resolved = resolve_model(ws.project, ws.db, &self.model_name)?;
            let run = self.runs.get(ws, resolved.model, name)?;
            let stale = !self.runs.is_fresh(ws, &run);
            Ok(RunResults {
                results: run.results.clone(),
                revision: run.revision,
                stale,
            })
        });
        results.map_err(|err: ToolError| {
            if cancelled {
                ToolError::cancelled().into()
            } else {
                err.into()
            }
        })
    }

    /// The session's named runs, oldest first, for a host to list: each with
    /// the revision it was made at, whether it is stale, whether it is gone
    /// (stale, with its results no longer kept), the run it started from, and
    /// everything it changed from the model. "current", the model as it is,
    /// is always there and is not listed.
    pub fn runs(&mut self, ws: &Workspace<'_>) -> Vec<RunListing> {
        let model = resolve_datamodel_model(ws.project, &self.model_name);
        self.runs.listing(ws, model)
    }

    /// Forget the named run `name`, its results and its plan, as when the
    /// person discards it: no tool reads it again, and a run made from it
    /// keeps what it changed. Whether the session had it; "current", the
    /// model as it is, is refused.
    pub fn forget_run(&mut self, name: &str) -> Result<bool, String> {
        if name.trim() == runs::CURRENT {
            return Err(format!(
                "\"{}\" is the model as it is, which a session does not forget",
                runs::CURRENT
            ));
        }
        Ok(self.runs.forget(name.trim()))
    }

    /// Land the plan `edit_model` gave the id `id` on the project as `ws`
    /// has it: the project as the plan leaves it, for the host to make its
    /// contents in one edit, or why it cannot land there, for the agent to
    /// plan again. `None` for an id the session never gave, or a plan it
    /// has forgotten (it keeps the last 16).
    ///
    /// A plan lands by construction: at the revision it was planned at, its
    /// patch, gated against those very contents; at another, planned again
    /// on the contents as they are, and landed only when what it writes is
    /// as it was, the gate passes again and its lines are the ones the
    /// person approved. The host holds the project's contents for the call,
    /// so nothing lands between the check and the edit.
    pub fn land_plan(&mut self, ws: Workspace<'_>, id: &str) -> Option<Landing> {
        let (landing, cancelled) = checkpointed(ws, |ws| edit::land_plan(self, ws, id));
        match landing {
            Some(Landing::Refused(_)) if cancelled => {
                Some(Landing::Refused(ToolError::cancelled().error))
            }
            landing => landing,
        }
    }

    /// What changed in the model's variables and sim specs since this
    /// session's last `read_model`, or `None` before the first read and when
    /// nothing did: what a host tells an agent about the person's work before
    /// its next turn. The revision alone cannot say, since a layout-only
    /// change advances it and changes no variable.
    ///
    /// What the session's own plans left, once a host landed them, is the
    /// agent's work and not news to it, so a variable or the sim specs as one
    /// of its plans would leave them is left out; a change the person made
    /// after the plan landed is reported.
    ///
    /// `project` at `revision` is the project as it is now, as a
    /// [`Workspace`] carries it.
    pub fn changes_since_read(
        &self,
        project: &datamodel::Project,
        revision: u64,
    ) -> Option<Changes> {
        let snapshot = self.last_read.as_ref()?;
        if snapshot.revision == revision {
            return None;
        }
        let model = resolve_datamodel_model(project, &self.model_name)?;
        let changes = changes::diff(snapshot, project, model, &self.plans);
        (!changes.is_empty()).then_some(changes)
    }
}

/// Run `answer` on `ws` with its checkpoints stopping for the host's cancel
/// as for other work waiting, and say whether one stopped the call for the
/// cancel: a checkpoint asks only whether to stop, so the entry point says
/// why.
fn checkpointed<T>(ws: Workspace<'_>, answer: impl FnOnce(&mut Workspace<'_>) -> T) -> (T, bool) {
    let Workspace {
        project,
        db,
        revision,
        waiting,
        cancelled,
    } = ws;
    let stopped_for_cancel = std::sync::atomic::AtomicBool::new(false);
    let stop = || {
        if cancelled.is_some_and(|cancelled| cancelled()) {
            stopped_for_cancel.store(true, std::sync::atomic::Ordering::SeqCst);
            return true;
        }
        waiting.is_some_and(|waiting| waiting())
    };
    let mut ws = Workspace {
        project,
        db,
        revision,
        waiting: Some(&stop),
        cancelled,
    };
    let answer = answer(&mut ws);
    (answer, stopped_for_cancel.into_inner())
}

/// A run's series for a host to chart, and what the host needs to say of
/// them.
pub struct RunResults {
    pub results: crate::Results,
    /// The revision the run was made at.
    pub revision: u64,
    /// Whether the model has changed since, beyond its diagrams.
    pub stale: bool,
}

/// Why a host's read of a run has no results.
#[derive(Debug, Clone, PartialEq)]
pub struct RunUnavailable {
    /// What went wrong, in words: no such run, or a model that does not
    /// simulate.
    pub reason: String,
    /// That the read stopped for other work on the project and kept nothing
    /// ([`Workspace::waiting`]), not that there is no such run: a host reads
    /// it again once that work is done.
    pub interrupted: bool,
    /// That the host cancelled the read ([`Workspace::cancelled`]), which it
    /// does not make again.
    pub cancelled: bool,
}

impl From<ToolError> for RunUnavailable {
    fn from(err: ToolError) -> RunUnavailable {
        RunUnavailable {
            interrupted: err.is_interrupted(),
            cancelled: err.cancelled,
            reason: err.error,
        }
    }
}

/// A refusal: what rule the call broke and how to repair it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
pub(crate) struct ToolError {
    error: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    suggestions: Vec<String>,
    /// That the call stopped for other work on the project and kept nothing:
    /// what a host that retries by itself looks for. It retries once that
    /// work is done -- after an edit, at the next revision -- and never in a
    /// loop against a project that stays busy.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    interrupted: bool,
    /// That the host cancelled the call, which stopped and kept nothing: no
    /// host calls it again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    cancelled: bool,
}

impl ToolError {
    pub(crate) fn new(error: impl Into<String>) -> ToolError {
        ToolError {
            error: error.into(),
            suggestions: vec![],
            interrupted: false,
            cancelled: false,
        }
    }

    /// Whether this is the answer of a call that stopped for other work,
    /// which a caller passes on rather than reads as a refusal of its own.
    pub(crate) fn is_interrupted(&self) -> bool {
        self.interrupted
    }

    /// The answer of a call that stopped for other work on the project
    /// ([`Workspace::waiting`]).
    pub(crate) fn interrupted() -> ToolError {
        ToolError {
            interrupted: true,
            ..ToolError::new(
                "the call stopped for the person's work on the project, which was waiting for \
                 it, and kept nothing; the model may have changed, so read what this call \
                 needs again, then make it once more",
            )
        }
    }

    /// The answer of a call its host cancelled ([`Workspace::cancelled`]).
    pub(crate) fn cancelled() -> ToolError {
        ToolError {
            cancelled: true,
            ..ToolError::new(
                "the host cancelled the call, which stopped before it finished and kept nothing",
            )
        }
    }

    pub(crate) fn with_suggestions(mut self, suggestions: Vec<String>) -> ToolError {
        self.suggestions = suggestions;
        self
    }

    /// The refusal with `prefix` before what it says: which of several
    /// parts of a call it is about.
    pub(crate) fn prefixed(mut self, prefix: &str) -> ToolError {
        self.error = format!("{prefix}{}", self.error);
        self
    }
}

/// Parse `input` as the tool's input type, run it, and serialize whichever of
/// its result or its refusal came back.
fn respond<I: DeserializeOwned, O: Serialize>(
    name: ToolName,
    input: &str,
    run: impl FnOnce(I) -> Result<O, ToolError>,
) -> ToolOutput {
    let text = if input.trim().is_empty() { "{}" } else { input };
    let result = serde_json::from_str::<I>(text)
        .map_err(|err| {
            ToolError::new(format!(
                "the input does not match {}'s schema: {err}",
                name.name()
            ))
        })
        .and_then(run);
    match result {
        Ok(output) => ToolOutput {
            json: serde_json::to_string(&output).expect("tool outputs serialize"),
            is_error: false,
            interrupted: false,
            cancelled: false,
        },
        Err(error) => ToolOutput::refusal(&error),
    }
}

impl ToolOutput {
    /// The answer of a call its host cancelled before it began its work
    /// ([`Workspace::cancelled`]): what a host that notices the cancel first,
    /// as a call does that waited for its session, answers without making the
    /// call.
    pub fn cancelled() -> ToolOutput {
        ToolOutput::refusal(&ToolError::cancelled())
    }

    fn refusal(error: &ToolError) -> ToolOutput {
        ToolOutput {
            json: serde_json::to_string(error).expect("refusals serialize"),
            is_error: true,
            interrupted: error.interrupted,
            cancelled: error.cancelled,
        }
    }
}

/// The datamodel model a session's name names: the model of that name, or for
/// `"main"` the project's first model when none is called that -- the rule
/// `analysis::analyze_model` and the MCP tools apply.
pub(crate) fn resolve_datamodel_model<'a>(
    project: &'a datamodel::Project,
    name: &str,
) -> Option<&'a datamodel::Model> {
    project.get_model(name).or_else(|| {
        (name == "main")
            .then(|| project.models.iter().find(|m| m.macro_spec.is_none()))
            .flatten()
    })
}

/// A session's model in both representations: the datamodel a tool reads text
/// from and the salsa handles it runs queries against.
pub(crate) struct ResolvedModel<'a> {
    pub model: &'a datamodel::Model,
    pub source_project: SourceProject,
    pub source_model: SourceModel,
}

/// Resolve the session's model in `project`, whose database is `db`, or refuse
/// naming the models the project has. The model borrows the project, not the
/// workspace, so a tool can hold it while it runs the workspace's database.
pub(crate) fn resolve_model<'p>(
    project: &'p datamodel::Project,
    db: &SimlinDb,
    name: &str,
) -> Result<ResolvedModel<'p>, ToolError> {
    let not_found = || {
        let names: Vec<String> = project
            .models
            .iter()
            .filter(|m| m.macro_spec.is_none() && !m.name.starts_with("stdlib\u{205A}"))
            .map(|m| m.name.clone())
            .collect();
        ToolError::new(format!("the project has no model named '{name}'")).with_suggestions(names)
    };
    let model = resolve_datamodel_model(project, name).ok_or_else(not_found)?;
    let source_project = db.current_source_project().ok_or_else(|| {
        ToolError::new(
            "the project has not been compiled; the host must sync it before calling a tool",
        )
    })?;
    let canonical = crate::canonicalize(&model.name);
    let source_model = source_project
        .models(db)
        .get(canonical.as_ref())
        .copied()
        .ok_or_else(not_found)?;
    Ok(ResolvedModel {
        model,
        source_project,
        source_model,
    })
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
#[path = "corpus_tests.rs"]
mod corpus_tests;
