// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The agent tool surface: the tools an agent uses to read, run, analyze and
//! edit a model, with their semantics stated once so every host that mounts
//! them answers alike. libsimlin mounts them for native hosts
//! (`simlin_tool_session_*`), and pysimlin through it.
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
//!   series. Every answer, a refusal included, keeps to the session's budget:
//!   one over it leaves things out, counted or named (an outline is outlined
//!   by sector instead), and one that is over with everything it can leave
//!   out left out is refused. Text a caller sent is echoed cut, with its
//!   length. Success says little.
//! - **Ids a claim can cite.** A diagnostic or a loop is reported under an id
//!   that is stable for the life of the session (`evidence`), so an agent can
//!   refer to "D3" or "L2" across calls and a verifier can check what it
//!   names.
//! - **An edit is made by its host.** No tool changes the project it is
//!   given. `edit_model`, the one tool whose effect is an edit
//!   ([`ToolEffect::Edit`]), answers an edit its gate passed with the project
//!   as the edit leaves it ([`ToolOutput::edited`]), and the host makes that
//!   its project's contents in one edit, one step of its undo. The host holds
//!   the project's contents for the whole call, so nothing changes between
//!   the gate and the edit.
//! - **Refusals are output.** `is_error` is set exactly when the call did not
//!   do what was asked -- input that does not match the schema, an edit of
//!   what changed since the read, an edit its gate refused -- and the answer
//!   is then the one refusal shape the catalog publishes, naming the rule
//!   and the repair, for the agent to read and answer. Only a host's misuse
//!   (a tool name the catalog does not list) is an `Err`.
//! - **One owner per decision.** Names resolve through `names`, diagnostics
//!   are reported through `evidence`, what a variable reads and what reads
//!   it come from [`crate::analysis::model_reads`], and a model's causal
//!   links, which loops are made of, from [`crate::analysis::model_links`].
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
mod input;
mod loops;
mod names;
mod outline;
mod runs;
mod series;
mod strict_schema;
mod variables;
mod verify;

pub use battery::{
    BehaviorFamily, Condition, Difference, ExtremeInput, ExtremeRule, Outcome, Problem,
    ProblemKind, Response, RunTestsInput, RunTestsOutput, SameChange, TestName, TestResult,
    TestSummary, TimeConstantEvidence,
};
pub use behavior::{BehaviorMode, Damping, Direction, ModeKind, classify};
#[cfg(feature = "schema")]
pub use catalog::generate_catalog_json;
pub use catalog::{ToolEffect, ToolName, catalog_json};
pub use changes::{ChangedField, ChangedVariable, Changes};
pub use edit::{
    ChangeAction, ChangeLine, EditDiagnostic, EditModelInput, EditModelOutput, EditOperation,
    GateRule, LookupShape, RefusedEdit,
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
pub use strict_schema::{STRICT_FORMATS, StrictCost, strict_cost, strict_input_schema};
pub use variables::{
    ElementEquation, FindVariablesInput, FindVariablesOutput, LinkPolarityName, LinkRef, Lookup,
    LookupKind, ModuleRecord, NotFound, ReadVariablesInput, ReadVariablesOutput, StockOptionName,
    VariableKind, VariableMatch, VariableRecord,
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

/// A tool's answer: JSON, whether it is a refusal for the agent to read and
/// repair rather than a result, and for an edit that was made, the project
/// as it leaves it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
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
    /// The project as the call's edit leaves it: present exactly when a tool
    /// whose effect is an edit ([`ToolEffect::Edit`]) made one that changes
    /// the project. The host makes it the project's contents in one edit and
    /// syncs its database to it; the session already holds what the edit
    /// changed as read. Absent for every read, every refusal, a call that
    /// stopped, and an edit that changes nothing.
    pub edited: Option<datamodel::Project>,
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

/// How many bytes `answer` is as JSON: what the session's budget is in.
pub(crate) fn json_len<T: Serialize>(answer: &T) -> usize {
    serde_json::to_string(answer).map_or(0, |json| json.len())
}

/// Cut `answer` one step at a time with `cut` until its JSON is within
/// `budget` bytes, or until `cut` has nothing left to leave out (it answers
/// `false`); whether it fits. The one loop answers are fitted by: each tool
/// says only what it leaves out, in what order, and how it counts it.
pub(crate) fn fit<T: Serialize>(
    answer: &mut T,
    budget: usize,
    mut cut: impl FnMut(&mut T) -> bool,
) -> bool {
    while json_len(answer) > budget {
        if !cut(answer) {
            return false;
        }
    }
    true
}

/// The most characters of a refusal's reason: what it says of the rule it
/// broke and the repair, and what a reason the engine wrote at length is cut
/// to.
pub(crate) const MAX_REFUSAL_CHARS: usize = 2_000;

/// The most names an answer lists of a set it reports (readers, variables
/// that changed), the rest counted.
pub(crate) const MAX_NAMED: usize = 12;

/// Whether a count is zero: what an answer leaves out of its JSON.
pub(crate) fn is_zero(n: &usize) -> bool {
    *n == 0
}

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
            ToolOutput::cancelled()
        } else {
            output
        })
    }

    /// Answer a call of the tool `name`.
    fn answer(&mut self, ws: &mut Workspace<'_>, name: ToolName, input: &str) -> ToolOutput {
        if let Err(interrupted) = ws.yield_point() {
            return ToolOutput::refusal(&interrupted, self.outline_budget);
        }
        let budget = self.outline_budget;
        match name {
            ToolName::ReadModel => respond(name, input, budget, |input| {
                outline::read_model(self, ws, input)
            }),
            ToolName::ReadVariables => respond(name, input, budget, |input| {
                variables::read_variables(self, ws, input)
            }),
            ToolName::FindVariables => respond(name, input, budget, |input| {
                variables::find_variables(ws, &self.model_name, input, self.outline_budget)
            }),
            ToolName::RunExperiment => respond(name, input, budget, |input| {
                experiment::run_experiment(self, ws, input)
            }),
            ToolName::ReadBehavior => respond(name, input, budget, |input| {
                series::read_behavior(self, ws, input)
            }),
            ToolName::ListRuns => respond(name, input, budget, |input| {
                runs::list_runs(self, ws, input)
            }),
            ToolName::AnalyzeLoops => respond(name, input, budget, |input| {
                loops::analyze_loops(self, ws, input)
            }),
            ToolName::RunTests => respond(name, input, budget, |input| {
                battery::run_tests(self, ws, input)
            }),
            ToolName::EditModel => {
                let mut edited = None;
                let mut output = respond(name, input, budget, |input| {
                    let (output, project) = edit::edit_model(self, ws, input)?;
                    edited = project;
                    Ok(output)
                });
                output.edited = edited;
                output
            }
            ToolName::VerifyFindings => respond(name, input, budget, |input| {
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
    /// is always there and is not listed. `project` is the contents at
    /// `revision`; the listing reads them alone, never the database, so a
    /// host lists runs while another call holds it.
    pub fn runs(&mut self, project: &datamodel::Project, revision: u64) -> Vec<RunListing> {
        let model = resolve_datamodel_model(project, &self.model_name);
        self.runs.listing(project, revision, model)
    }

    /// The results of the run named `name` when the session keeps them, read
    /// from `project`, the contents at `revision`, without the database:
    /// what a host reads first, so a read of a run made earlier waits for no
    /// one's work. `None` for a run that must be made (the current run of a
    /// model changed since, or a run whose results were dropped), which
    /// [`Session::run_results`] makes.
    pub fn kept_run_results(
        &mut self,
        project: &datamodel::Project,
        revision: u64,
        name: &str,
    ) -> Option<RunResults> {
        let (run, stale) = self.runs.kept(project, revision, name)?;
        Some(RunResults {
            results: run.results.clone(),
            revision: run.revision,
            stale,
        })
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

    /// What changed in the model's variables and sim specs since this
    /// session's last `read_model`, or `None` before the first read and when
    /// nothing did: what a host tells an agent about the person's work before
    /// its next turn. The revision alone cannot say, since a layout-only
    /// change advances it and changes no variable.
    ///
    /// What the session's own edits changed is the agent's work and not news
    /// to it: the session holds it as read from the moment the edit is made,
    /// so it is no change here; a change someone else makes afterwards is
    /// reported.
    ///
    /// `project` at `revision` is the project as it is now, as a
    /// [`Workspace`] carries it. A project that no longer has the session's
    /// model is refused, naming the models it has, as every tool refuses it:
    /// "nothing changed" would be false of it.
    pub fn changes_since_read(
        &self,
        project: &datamodel::Project,
        revision: u64,
    ) -> Result<Option<Changes>, String> {
        let Some(model) = resolve_datamodel_model(project, &self.model_name) else {
            return Err(model_not_found(project, &self.model_name).error);
        };
        let Some(snapshot) = self.last_read.as_ref() else {
            return Ok(None);
        };
        // An optimization: a host's revision is equal exactly when the
        // contents are, so the diff at the read's revision is empty.
        if snapshot.revision == revision {
            return Ok(None);
        }
        let changes = changes::diff(snapshot, project, model);
        Ok((!changes.is_empty()).then_some(changes))
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

/// A refusal: what rule the call broke and how to repair it. Every tool
/// refuses in this one shape, which the catalog publishes beside the tools'
/// own schemas (`refusalSchema`).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schema", schemars(rename = "Refusal", deny_unknown_fields))]
pub(crate) struct ToolError {
    /// The rule the call broke, and the repair.
    error: String,
    /// Names the call may have meant: variables, models or runs.
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
    /// For an edit its gate refused: the rule that refused it and what the
    /// edit would have done, for the agent to repair its operations.
    #[serde(rename = "refusedEdit", skip_serializing_if = "Option::is_none")]
    refused_edit: Option<Box<edit::RefusedEdit>>,
}

impl ToolError {
    pub(crate) fn new(error: impl Into<String>) -> ToolError {
        ToolError {
            error: error.into(),
            suggestions: vec![],
            interrupted: false,
            cancelled: false,
            refused_edit: None,
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

    /// The refusal with names the call may have meant: at most
    /// [`MAX_NAMED`], each as an answer echoes a name.
    pub(crate) fn with_suggestions(mut self, suggestions: Vec<String>) -> ToolError {
        self.suggestions = suggestions
            .iter()
            .take(MAX_NAMED)
            .map(|name| evidence::echo(name))
            .collect();
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
    budget: usize,
    run: impl FnOnce(I) -> Result<O, ToolError>,
) -> ToolOutput {
    let text = if input.trim().is_empty() { "{}" } else { input };
    let result = input::parse::<I>(text)
        .map_err(|err| mismatch(name, err))
        .and_then(run);
    match result {
        Ok(output) => ToolOutput {
            json: serde_json::to_string(&output).expect("tool outputs serialize"),
            is_error: false,
            interrupted: false,
            cancelled: false,
            edited: None,
        },
        Err(error) => ToolOutput::refusal(&error, budget),
    }
}

/// The refusal of input that is not what `tool` takes, naming where in the
/// input the mismatch is ([`input::parse`]). The parser's reason repeats
/// what the input holds there (a field's name, a string), so it and the
/// place are echoed as any caller's text is ([`evidence::echo`]).
fn mismatch(tool: ToolName, mismatch: input::Mismatch) -> ToolError {
    let tool = tool.name();
    ToolError::new(match mismatch {
        input::Mismatch::NotJson(reason) => {
            format!(
                "the input to {tool} is not JSON: {}",
                evidence::echo(&reason)
            )
        }
        input::Mismatch::NotInput { path, reason } if path.is_empty() => {
            format!(
                "the input does not match {tool}'s schema: {}",
                evidence::echo(&reason)
            )
        }
        input::Mismatch::NotInput { path, reason } => {
            format!(
                "the input does not match {tool}'s schema at `{}`: {}",
                evidence::echo(&path),
                evidence::echo(&reason)
            )
        }
    })
}

impl ToolOutput {
    /// The answer of a call its host cancelled before it began its work
    /// ([`Workspace::cancelled`]): what a host that notices the cancel first,
    /// as a call does that waited for its session, answers without making the
    /// call.
    pub fn cancelled() -> ToolOutput {
        ToolOutput::refusal(&ToolError::cancelled(), OUTLINE_BUDGET)
    }

    /// `error` as an answer within `budget` bytes. Every refusal is built to
    /// its budget (echoed names, fitted edits); a reason the engine wrote at
    /// length (a compile message naming a model's every variable) is cut to
    /// [`MAX_REFUSAL_CHARS`] here, and under a budget smaller still its
    /// suggestions are left out and then its reason is halved, down to
    /// [`evidence::ECHO_CHARS`].
    fn refusal(error: &ToolError, budget: usize) -> ToolOutput {
        let mut error = error.clone();
        if error.error.chars().count() > MAX_REFUSAL_CHARS {
            error.error = evidence::window(&error.error, 0, 0, MAX_REFUSAL_CHARS);
        }
        fit(&mut error, budget, |error| {
            if error.suggestions.pop().is_some() {
                return true;
            }
            // A cut reason ends with an ellipsis, so it is halved only while
            // that leaves more than the floor: a cut that cannot shrink it
            // would never end.
            let length = error.error.chars().count();
            if length / 2 < evidence::ECHO_CHARS {
                return false;
            }
            error.error = evidence::window(&error.error, 0, 0, length / 2);
            true
        });
        let error = &error;
        ToolOutput {
            json: serde_json::to_string(error).expect("refusals serialize"),
            is_error: true,
            interrupted: error.interrupted,
            cancelled: error.cancelled,
            edited: None,
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

/// The refusal of a session whose model `name` the project does not have,
/// suggesting the models it has.
fn model_not_found(project: &datamodel::Project, name: &str) -> ToolError {
    let names: Vec<String> = project
        .models
        .iter()
        .filter(|m| m.macro_spec.is_none() && !m.name.starts_with("stdlib\u{205A}"))
        .map(|m| m.name.clone())
        .collect();
    ToolError::new(format!("the project has no model named '{name}'")).with_suggestions(names)
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
    let not_found = || model_not_found(project, name);
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
