// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `edit_model`: an edit as a plan the host lands, gated so it never adds an
//! error.
//!
//! An agent describes an edit as operations -- add a stock, set an equation,
//! connect a flow, rename, delete, name a loop, change the sim specs -- and
//! gets a plan back, applied to nothing:
//!
//! 1. **Fresh.** Everything the edit writes -- each variable, the sim specs,
//!    each loop name -- must be as the session last read it (with
//!    `read_model`), or as one of the session's own plans left it, so an
//!    agent never overwrites work it has not seen. Provenance aside: who made
//!    a variable is no change to it. An edit before any read is refused.
//! 2. **One patch.** The operations become one patch, applied in order to a
//!    copy of the project so each sees the ones before it. A field edit keeps
//!    every field it does not set. An edit that changes the model's structure
//!    places what it adds with the engine's incremental layout, inside the
//!    same patch, so the placement undoes with the edit.
//! 3. **The gate.** The patch is staged on the host's database, its
//!    diagnostics compared with the model's by code and variable (the model's
//!    mapped through the edit's renames), and its run with the model's. It is
//!    refused when it adds an error the model did not have, stops a model
//!    that simulated from simulating, makes a value not a number that was a
//!    number throughout, or gives a model without unit warnings its first
//!    (the rule a person's patch meets). Errors the model had already are
//!    tolerated, so a broken model can be repaired a step at a time, and
//!    every warning it adds is listed for the agent to fix. `Gate::refusal`
//!    states what else it tolerates.
//! 4. **The plan.** What the host shows the person before landing it: one
//!    line per variable the edit changes (a rename's rewritten readers
//!    included), the diagnostics it adds, and whether the model simulates,
//!    fitted to the answer budget. A plan that passes is kept under an id
//!    (`P1`, ...), and the host lands it by id with [`land_plan`], once the
//!    person approves, which is how an agent's edit reaches the project, its
//!    undo, and the person's view of it. A plan lands by construction, on the
//!    project as it is then.
//! 5. **Provenance.** In a project that records who made its variables
//!    (ISEE's AI information), what the edit adds is marked made by AI and
//!    what it edits marked edited by AI (`after_ai_edit`).
//!
//! The patch never travels in the tool's output: a view's JSON is nothing an
//! agent reads, and the person approves the plan's lines, not its bytes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::datamodel::{self, AiState, Equation, Variable};
use crate::db::{LtmOverlay, collect_all_diagnostics};
use crate::patch::{ModelOperation, ModelPatch, ProjectOperation, ProjectPatch};

use super::changes::{
    ChangedField, Explains, ReadSnapshot, changed_fields, effective_specs, loop_entry, same_record,
};
use super::evidence::{Described, Evidence, describe, display_name};
use super::outline::{IntegrationMethod, dt_value, equation_text};
use super::runs::{Run, RunStore, Staging, Unbuilt, build_vm};
use super::{
    DiagnosticCategoryName, Session, Severity, ToolError, Workspace, names, resolve_model,
};

/// The most operations one edit takes.
pub(crate) const MAX_OPERATIONS: usize = 24;

/// The most plans a session keeps; making one more forgets the oldest.
pub(crate) const MAX_PLANS: usize = 16;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EditModelInput {
    /// What the edit does and why, in a sentence, for the person approving
    /// it.
    pub summary: String,
    /// The operations, applied in order: each sees the ones before it. At
    /// most 24.
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 24)))]
    pub operations: Vec<EditOperation>,
}

/// One operation of an edit.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditOperation {
    /// Add a stock with its initial value, and the flows (existing ones, or
    /// ones added earlier in this edit) that fill and drain it.
    AddStock {
        name: String,
        initial: String,
        #[serde(default)]
        units: Option<String>,
        #[serde(default)]
        notes: Option<String>,
        #[serde(default)]
        inflows: Vec<String>,
        #[serde(default)]
        outflows: Vec<String>,
    },
    /// Add a flow with its equation, draining the stock `from` and filling
    /// the stock `to`; a side left out is a cloud, outside the model.
    AddFlow {
        name: String,
        equation: String,
        #[serde(default)]
        units: Option<String>,
        #[serde(default)]
        notes: Option<String>,
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        to: Option<String>,
    },
    /// Add an auxiliary: a constant, or a variable computed from others.
    AddVariable {
        name: String,
        equation: String,
        #[serde(default)]
        units: Option<String>,
        #[serde(default)]
        notes: Option<String>,
    },
    /// Replace a variable's equation (a stock's: its initial value). For an
    /// arrayed variable, the equation applies to every element, or to the
    /// one `element` names (elements of several dimensions joined by commas).
    SetEquation {
        variable: String,
        equation: String,
        #[serde(default)]
        element: Option<String>,
    },
    /// Set a variable's units; empty clears them.
    SetUnits { variable: String, units: String },
    /// Set a variable's documentation; empty clears it.
    SetNotes { variable: String, notes: String },
    /// Make a variable a lookup of its equation: `points` are `[x, y]` pairs,
    /// x increasing.
    SetLookup {
        variable: String,
        points: Vec<[f64; 2]>,
        /// How it reads between and past its points: continuous (the
        /// default: interpolated, held at the ends), extrapolate, or
        /// discrete (stepped).
        #[serde(default)]
        kind: Option<LookupShape>,
    },
    /// Set which stock a flow drains (`from`) and fills (`to`); a side left
    /// out is a cloud.
    ConnectFlow {
        flow: String,
        #[serde(default)]
        from: Option<String>,
        #[serde(default)]
        to: Option<String>,
    },
    /// Rename a variable; every equation that reads it is rewritten.
    Rename { variable: String, to: String },
    /// Delete a variable; a deleted flow leaves the stocks it filled and
    /// drained.
    Delete { variable: String },
    /// Name a feedback loop, by the id analyze_loops gave it or by its
    /// variables.
    NameLoop {
        #[serde(default, rename = "loop")]
        loop_id: Option<String>,
        #[serde(default)]
        variables: Vec<String>,
        name: String,
        #[serde(default)]
        description: Option<String>,
    },
    /// Change the run's start, stop, DT or integration method.
    SetSimSpecs {
        #[serde(default)]
        start: Option<f64>,
        #[serde(default)]
        stop: Option<f64>,
        #[serde(default)]
        dt: Option<f64>,
        #[serde(default)]
        method: Option<IntegrationMethod>,
    },
}

impl EditOperation {
    fn name(&self) -> &'static str {
        match self {
            EditOperation::AddStock { .. } => "add_stock",
            EditOperation::AddFlow { .. } => "add_flow",
            EditOperation::AddVariable { .. } => "add_variable",
            EditOperation::SetEquation { .. } => "set_equation",
            EditOperation::SetUnits { .. } => "set_units",
            EditOperation::SetNotes { .. } => "set_notes",
            EditOperation::SetLookup { .. } => "set_lookup",
            EditOperation::ConnectFlow { .. } => "connect_flow",
            EditOperation::Rename { .. } => "rename",
            EditOperation::Delete { .. } => "delete",
            EditOperation::NameLoop { .. } => "name_loop",
            EditOperation::SetSimSpecs { .. } => "set_sim_specs",
        }
    }

    /// Whether the operation can change what the diagram draws.
    fn structural(&self) -> bool {
        !matches!(
            self,
            EditOperation::SetUnits { .. }
                | EditOperation::SetNotes { .. }
                | EditOperation::NameLoop { .. }
                | EditOperation::SetSimSpecs { .. }
        )
    }
}

/// How a lookup reads between and past its points.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LookupShape {
    Continuous,
    Extrapolate,
    Discrete,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct EditModelOutput {
    pub revision: u64,
    pub verdict: Verdict,
    /// The plan's id, for the host to land it once the person approves:
    /// present when the verdict is ready.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    pub summary: String,
    /// One line per variable the edit changes, and for the sim specs and
    /// loop names. A line's detail is cut at 240 characters.
    pub changes: Vec<PlannedChange>,
    /// Lines left out to keep the answer within its budget.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted: usize,
    /// What the edit would add: the errors and values that are not a number
    /// (why it is refused), and every warning (to fix; the first unit warning
    /// of a model without one refuses it).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<PlannedDiagnostic>,
    /// Whether the model simulates as the edit leaves it.
    pub simulates: bool,
    /// Why the edit is refused, and anything else the plan needs said.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// The gate's verdict.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// The edit adds no error: the host may land it.
    Ready,
    /// The edit adds an error, stops the model simulating, makes a value
    /// not a number, or gives a model its first unit warning: fix the
    /// operations and plan again.
    Refused,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlannedChange {
    /// The variable, as the model names it (before a rename), a loop's name,
    /// or "sim specs".
    pub variable: String,
    pub action: ChangeAction,
    /// What changes, in words.
    pub detail: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChangeAction {
    Added,
    Changed,
    Renamed,
    Deleted,
    LoopNamed,
    SimSpecs,
}

/// A diagnostic the edit would add.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct PlannedDiagnostic {
    pub severity: Severity,
    pub category: DiagnosticCategoryName,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What an edit writes, as a model has it at some moment: each variable's
/// record (none: absent), the sim specs when the edit sets them, and the
/// name of each loop the edit names (none: unnamed).
#[derive(Clone, Default)]
struct Written {
    records: BTreeMap<String, Option<Variable>>,
    specs: Option<datamodel::SimSpecs>,
    loops: BTreeMap<BTreeSet<String>, Option<LoopName>>,
}

impl Written {
    /// What of `names`, the specs (when `specs`) and `loops` the model named
    /// `model_name` in `project` has.
    fn of(
        project: &datamodel::Project,
        model_name: &str,
        names: impl IntoIterator<Item = String>,
        specs: bool,
        loops: &[BTreeSet<String>],
    ) -> Written {
        let Some(model) = project.get_model(model_name) else {
            return Written::default();
        };
        Written {
            records: names
                .into_iter()
                .map(|name| {
                    let record = model
                        .variables
                        .iter()
                        .find(|v| crate::canonicalize(v.get_ident()).as_ref() == name.as_str())
                        .cloned();
                    (name, record)
                })
                .collect(),
            specs: specs.then(|| effective_specs(project, model).clone()),
            loops: loops
                .iter()
                .map(|names| (names.clone(), LoopName::of(model, names)))
                .collect(),
        }
    }

    /// Whether the model named `model_name` in `project` has exactly this,
    /// provenance aside.
    fn holds(&self, project: &datamodel::Project, model_name: &str) -> bool {
        let now = Written::of(
            project,
            model_name,
            self.records.keys().cloned(),
            self.specs.is_some(),
            &self.loops.keys().cloned().collect::<Vec<_>>(),
        );
        self.records
            .iter()
            .all(|(name, record)| same_record(record.as_ref(), now.records[name].as_ref()))
            && self.specs == now.specs
            && self.loops == now.loops
    }
}

/// A loop's name, as a model's loop metadata gives it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
struct LoopName {
    name: String,
    description: String,
    deleted: bool,
}

impl LoopName {
    /// The name `model` gives the loop through the variables `names`.
    fn of(model: &datamodel::Model, names: &BTreeSet<String>) -> Option<LoopName> {
        loop_entry(&model.variables, &model.loop_metadata, names).map(LoopName::from)
    }
}

impl From<&datamodel::LoopMetadata> for LoopName {
    fn from(entry: &datamodel::LoopMetadata) -> LoopName {
        LoopName {
            name: entry.name.clone(),
            description: entry.description.clone(),
            deleted: entry.deleted,
        }
    }
}

/// A plan the gate passed, kept for the host to land.
#[derive(Clone)]
pub(crate) struct Plan {
    /// The revision it was planned at.
    revision: u64,
    /// The operations, to plan again on the project as it is when the plan
    /// lands at another revision.
    operations: Vec<EditOperation>,
    /// The patch, for the project at `revision`.
    patch: ProjectPatch,
    /// Each line of the plan whole: what the person approved.
    changes: Vec<PlannedChange>,
    /// What the edit writes as it was when the plan was made, and as the
    /// plan leaves it: what landing checks it against, and what makes the
    /// session's next edit of it fresh and its change no news to the agent.
    before: Written,
    after: Written,
}

impl Plan {
    #[cfg(test)]
    pub(crate) fn patch(&self) -> &ProjectPatch {
        &self.patch
    }
}

/// The plans a session keeps.
#[derive(Default)]
pub(crate) struct PlanStore {
    plans: IndexMap<String, Plan>,
    next: u32,
}

impl PlanStore {
    fn keep(&mut self, plan: Plan) -> String {
        self.next += 1;
        let id = format!("P{}", self.next);
        self.plans.insert(id.clone(), plan);
        if self.plans.len() > MAX_PLANS {
            self.plans.shift_remove_index(0);
        }
        id
    }

    pub(crate) fn get(&self, id: &str) -> Option<&Plan> {
        self.plans.get(id)
    }

    /// Whether one of the plans would leave the variable named `name` as
    /// `record` is, provenance aside.
    fn produced(&self, name: &str, record: Option<&Variable>) -> bool {
        self.plans.values().any(|plan| {
            plan.after
                .records
                .get(name)
                .is_some_and(|r| same_record(r.as_ref(), record))
        })
    }

    fn produced_specs(&self, specs: &datamodel::SimSpecs) -> bool {
        self.plans
            .values()
            .any(|plan| plan.after.specs.as_ref() == Some(specs))
    }

    fn produced_loop(&self, names: &BTreeSet<String>, entry: Option<&LoopName>) -> bool {
        self.plans.values().any(|plan| {
            plan.after
                .loops
                .get(names)
                .is_some_and(|e| e.as_ref() == entry)
        })
    }
}

/// A change one of the session's plans would make is the agent's own work.
impl Explains for PlanStore {
    fn variable(&self, name: &str, record: Option<&Variable>) -> bool {
        self.produced(name, record)
    }

    fn specs(&self, specs: &datamodel::SimSpecs) -> bool {
        self.produced_specs(specs)
    }
}

/// What an edit's writes must find to be fresh: what it may overwrite.
trait Expected {
    fn variable(&self, name: &str, now: Option<&Variable>) -> bool;
    fn specs(&self, now: &datamodel::SimSpecs) -> bool;
    fn loop_name(&self, names: &BTreeSet<String>, now: Option<&LoopName>) -> bool;
    /// The refusal for what is not fresh, named.
    fn stale(&self, what: String) -> ToolError;
}

/// As the session last read the model, or as one of its own plans left it:
/// what an agent's edit may overwrite.
struct AsRead<'a> {
    snapshot: &'a ReadSnapshot,
    plans: &'a PlanStore,
}

impl Expected for AsRead<'_> {
    fn variable(&self, name: &str, now: Option<&Variable>) -> bool {
        same_record(now, self.snapshot.record(name)) || self.plans.produced(name, now)
    }

    fn specs(&self, now: &datamodel::SimSpecs) -> bool {
        now == self.snapshot.specs() || self.plans.produced_specs(now)
    }

    fn loop_name(&self, names: &BTreeSet<String>, now: Option<&LoopName>) -> bool {
        self.snapshot.loop_name(names).map(LoopName::from).as_ref() == now
            || self.plans.produced_loop(names, now)
    }

    fn stale(&self, what: String) -> ToolError {
        ToolError::new(format!(
            "{what} changed since you last read the model; read_model again before editing"
        ))
    }
}

/// As they were when a plan was made: what the plan may land on.
struct AsPlanned<'a>(&'a Written);

impl Expected for AsPlanned<'_> {
    fn variable(&self, name: &str, now: Option<&Variable>) -> bool {
        self.0
            .records
            .get(name)
            .is_some_and(|record| same_record(record.as_ref(), now))
    }

    fn specs(&self, now: &datamodel::SimSpecs) -> bool {
        self.0.specs.as_ref() == Some(now)
    }

    fn loop_name(&self, names: &BTreeSet<String>, now: Option<&LoopName>) -> bool {
        self.0
            .loops
            .get(names)
            .is_some_and(|entry| entry.as_ref() == now)
    }

    fn stale(&self, what: String) -> ToolError {
        ToolError::new(format!(
            "{what} changed since the plan was made; plan the edit again"
        ))
    }
}

pub(crate) fn edit_model(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: EditModelInput,
) -> Result<EditModelOutput, ToolError> {
    if input.operations.is_empty() || input.operations.len() > MAX_OPERATIONS {
        return Err(ToolError::new(format!(
            "an edit has between 1 and {MAX_OPERATIONS} operations (this one has {})",
            input.operations.len()
        )));
    }
    if input.summary.trim().is_empty() {
        return Err(ToolError::new(
            "say what the edit does in its summary, for the person approving it",
        ));
    }
    resolve_model(ws.project, ws.db, &session.model_name)?;
    let Some(snapshot) = session.last_read.as_ref() else {
        return Err(ToolError::new(
            "read_model first: an edit is planned against what you last read",
        ));
    };
    let expected = AsRead {
        snapshot,
        plans: &session.plans,
    };
    let planned = plan_edit(
        &mut session.runs,
        &session.evidence,
        ws,
        &session.model_name,
        &input.operations,
        &expected,
    )?;

    let plan = if planned.refusal.is_some() {
        None
    } else {
        Some(session.plans.keep(Plan {
            revision: ws.revision,
            operations: input.operations,
            patch: planned.patch,
            changes: planned.changes.clone(),
            before: planned.before,
            after: planned.after,
        }))
    };
    let note = [planned.refusal, planned.note]
        .into_iter()
        .flatten()
        .reduce(|a, b| format!("{a} {b}"));
    Ok(fitted(
        EditModelOutput {
            revision: ws.revision,
            verdict: if plan.is_some() {
                Verdict::Ready
            } else {
                Verdict::Refused
            },
            plan,
            summary: input.summary,
            changes: planned.changes,
            omitted: 0,
            diagnostics: planned.diagnostics,
            simulates: planned.simulates,
            note,
        },
        session.outline_budget,
    ))
}

/// The most characters of a change line's detail an answer gives.
const MAX_DETAIL_CHARS: usize = 240;

/// `output` within `budget` bytes: each line's detail cut to
/// [`MAX_DETAIL_CHARS`], then the last lines left out, counted.
fn fitted(mut output: EditModelOutput, budget: usize) -> EditModelOutput {
    for change in &mut output.changes {
        if change.detail.chars().count() > MAX_DETAIL_CHARS {
            let cut: String = change.detail.chars().take(MAX_DETAIL_CHARS - 3).collect();
            change.detail = format!("{cut}...");
        }
    }
    while output.changes.len() > 1
        && serde_json::to_string(&output).map_or(0, |json| json.len()) > budget
    {
        output.changes.pop();
        output.omitted += 1;
    }
    output
}

/// What landing a plan came to.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub enum Landing {
    /// The project as the plan leaves it, for the host to make its contents
    /// in one edit.
    Landed(Box<datamodel::Project>),
    /// Why the plan cannot land on the project as it is: the agent plans the
    /// edit again.
    Refused(String),
}

/// Land the plan `id` on the project as `ws` has it; `None` for an id the
/// session never gave or has forgotten.
///
/// At the revision it was planned at, the project is the one the plan was
/// gated against, and its patch lands. At another, the plan's operations
/// are planned again on the project as it is: the plan lands only if
/// everything it writes -- each variable, the sim specs, each loop name --
/// is as it was when the plan was made, the gate passes again, and the plan
/// comes out with the lines the person approved. So a person's diagram edits
/// meanwhile are kept (the placement is made again around them), and an edit
/// of what the plan writes, or of what it reads that the gate catches, sends
/// the agent back to plan again.
pub(crate) fn land_plan(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    id: &str,
) -> Option<Landing> {
    let plan = session.plans.get(id)?.clone();
    if ws.revision == plan.revision {
        let mut project = ws.project.clone();
        return Some(match crate::apply_patch(&mut project, plan.patch) {
            Ok(()) => Landing::Landed(Box::new(project)),
            Err(err) => Landing::Refused(format!("the plan does not apply: {err}")),
        });
    }
    if plan.after.holds(ws.project, &session.model_name) {
        return Some(Landing::Refused("the plan has landed already".to_string()));
    }
    let planned = plan_edit(
        &mut session.runs,
        &session.evidence,
        ws,
        &session.model_name,
        &plan.operations,
        &AsPlanned(&plan.before),
    );
    Some(match planned {
        Err(err) => Landing::Refused(err.error),
        Ok(planned) => match planned.refusal {
            Some(reason) => Landing::Refused(format!(
                "the model changed since the plan was made, and the edit no longer passes the \
                 gate: {}; plan it again",
                reason.trim_end_matches('.')
            )),
            None if planned.changes != plan.changes => {
                let lines: Vec<String> = planned
                    .changes
                    .iter()
                    .filter(|c| !plan.changes.contains(c))
                    .map(|c| format!("{}: {}", c.variable, c.detail))
                    .take(3)
                    .collect();
                Landing::Refused(format!(
                    "the model changed since the plan was made, so the edit would now do \
                     something else ({}); plan it again",
                    lines.join("; ")
                ))
            }
            None => Landing::Landed(Box::new(planned.working)),
        },
    })
}

/// An edit planned on the project as a workspace has it.
struct Planned {
    /// The project as the edit leaves it.
    working: datamodel::Project,
    patch: ProjectPatch,
    changes: Vec<PlannedChange>,
    diagnostics: Vec<PlannedDiagnostic>,
    simulates: bool,
    /// Why the gate refuses the edit, when it does.
    refusal: Option<String>,
    note: Option<String>,
    before: Written,
    after: Written,
}

/// Plan `operations` on the model named `model_name` as `ws` has it, the
/// edit's writes fresh by `expected`, and gate it.
fn plan_edit(
    runs: &mut RunStore,
    evidence: &Evidence,
    ws: &mut Workspace<'_>,
    model_name: &str,
    operations: &[EditOperation],
    expected: &dyn Expected,
) -> Result<Planned, ToolError> {
    let resolved = resolve_model(ws.project, ws.db, model_name)?;
    let model = resolved.model;
    let structural = operations.iter().any(EditOperation::structural);
    let mut builder = Builder::new(ws.project, model, evidence);
    for (i, op) in operations.iter().enumerate() {
        let name = op.name();
        builder
            .operate(op.clone())
            .map_err(|err| err.prefixed(&format!("operation {} ({name}): ", i + 1)))?;
    }

    // What the edit writes is as `expected` has it.
    let mut stale: Vec<String> = builder
        .written
        .iter()
        .filter(|name| {
            let now = model
                .variables
                .iter()
                .find(|v| crate::canonicalize(v.get_ident()).as_ref() == name.as_str());
            !expected.variable(name, now)
        })
        .map(|name| display_name(model, name))
        .collect();
    stale.sort();
    if builder.writes_specs && !expected.specs(effective_specs(ws.project, model)) {
        stale.push("the sim specs".to_string());
    }
    for names in &builder.loops_named {
        if !expected.loop_name(names, LoopName::of(model, names).as_ref()) {
            let through: Vec<String> = names.iter().map(|n| display_name(model, n)).collect();
            stale.push(format!(
                "the name of the loop through {}",
                through.join(", ")
            ));
        }
    }
    if !stale.is_empty() {
        return Err(expected.stale(stale.join(", ")));
    }

    builder.mark_provenance(model, tracks_provenance(ws.project))?;
    let mut note = None;
    if structural && let Err(reason) = builder.place(model) {
        note = Some(format!(
            "The diagram will not show this edit until it is laid out: {reason}."
        ));
    }
    let base = match runs.current(ws, model) {
        Ok(base) => Some(base),
        Err(err) if err.is_interrupted() => return Err(err),
        Err(_) => None,
    };
    let gate = gate(
        ws,
        &model.name,
        &builder.working,
        &builder.renames,
        base.as_deref(),
    )?;
    let refusal = gate.refusal(base.is_some());
    let changes = planned_changes(ws.project, &builder.working, &model.name, &builder.renames);
    let working_model = builder
        .working
        .get_model(&model.name)
        .expect("the edit keeps the model");
    let reported = |d: Described| PlannedDiagnostic {
        severity: d.severity,
        category: d.category,
        code: d.code.to_string(),
        variable: d.variable.map(|name| display_name(working_model, &name)),
        reason: d.reason,
    };
    let diagnostics = gate
        .new_errors
        .into_iter()
        .map(reported)
        .chain(
            gate.non_finite
                .iter()
                .map(|(variable, time)| PlannedDiagnostic {
                    severity: Severity::Error,
                    category: DiagnosticCategoryName::Value,
                    code: "non_finite".to_string(),
                    variable: Some(variable.clone()),
                    reason: Some(format!("not a number from time {time}")),
                }),
        )
        .chain(gate.new_warnings.into_iter().map(reported))
        .collect();

    let loops = builder.loops_named.clone();
    let before = Written::of(
        ws.project,
        &model.name,
        builder.written.iter().cloned(),
        builder.writes_specs,
        &loops,
    );
    // What the plan leaves: the variables it wrote under the names it left
    // them (a renamed one absent under its old name), and those it added.
    let renamed = rename_map(&builder.renames);
    let left: BTreeSet<String> = builder
        .written
        .iter()
        .flat_map(|name| [Some(name.clone()), renamed.get(name).cloned()])
        .flatten()
        .chain(builder.created.iter().cloned())
        .collect();
    let after = Written::of(
        &builder.working,
        &model.name,
        left,
        builder.writes_specs,
        &loops,
    );
    Ok(Planned {
        patch: builder.patch(&model.name),
        working: builder.working,
        changes,
        diagnostics,
        simulates: gate.simulates,
        refusal,
        note,
        before,
        after,
    })
}

/// The names an edit's renames leave, by canonical name before the edit: a
/// name renamed twice ends where the last rename leaves it.
fn rename_map(renames: &[(String, String)]) -> BTreeMap<String, String> {
    let mut renamed: BTreeMap<String, String> = BTreeMap::new();
    for (from, to) in renames {
        let from = crate::canonicalize(from).into_owned();
        let origin = renamed
            .iter()
            .find(|(_, now)| **now == from)
            .map(|(origin, _)| origin.clone())
            .unwrap_or(from);
        renamed.insert(origin, crate::canonicalize(to).into_owned());
    }
    renamed
}

/// Whether the project records who made its variables (ISEE's AI
/// information): it carries AI information, or a variable's provenance.
fn tracks_provenance(project: &datamodel::Project) -> bool {
    project.ai_information.is_some()
        || project
            .models
            .iter()
            .any(|m| m.variables.iter().any(|v| v.get_ai_state().is_some()))
}

/// A variable's provenance once an AI has edited it, from what it was
/// (ISEE's AI information): made by AI and never touched by a person stays
/// so (C); made by a person, or of unknown making, is now also edited by AI
/// (D, or H once a person has edited it too); made by AI then edited by a
/// person stays so (G), since that state already allows a later AI edit.
fn after_ai_edit(state: Option<AiState>) -> AiState {
    match state {
        None | Some(AiState::A) | Some(AiState::B) | Some(AiState::D) => AiState::D,
        Some(AiState::C) => AiState::C,
        Some(AiState::E) | Some(AiState::F) | Some(AiState::H) => AiState::H,
        Some(AiState::G) => AiState::G,
    }
}

/// An edit's operations, applied in order to a copy of the project.
struct Builder<'a> {
    working: datamodel::Project,
    model_name: String,
    evidence: &'a Evidence,
    project_ops: Vec<ProjectOperation>,
    ops: Vec<ModelOperation>,
    /// Renames, `(from, to)` as written.
    renames: Vec<(String, String)>,
    /// The canonical names of the variables the edit writes, as the model
    /// had them before it.
    written: HashSet<String>,
    /// The canonical names of the variables the edit adds, as it leaves
    /// them.
    created: HashSet<String>,
    /// Whether the edit sets the sim specs.
    writes_specs: bool,
    /// The loops the edit names, by their variables' canonical names.
    loops_named: Vec<BTreeSet<String>>,
}

impl<'a> Builder<'a> {
    fn new(
        project: &datamodel::Project,
        model: &datamodel::Model,
        evidence: &'a Evidence,
    ) -> Builder<'a> {
        Builder {
            working: project.clone(),
            model_name: model.name.clone(),
            evidence,
            project_ops: Vec::new(),
            ops: Vec::new(),
            renames: Vec::new(),
            written: HashSet::new(),
            created: HashSet::new(),
            writes_specs: false,
            loops_named: Vec::new(),
        }
    }

    fn model(&self) -> &datamodel::Model {
        self.working
            .get_model(&self.model_name)
            .expect("the working copy keeps the model")
    }

    /// The variable `name` names in the model as the edit has it so far.
    fn existing(&self, name: &str) -> Result<Variable, ToolError> {
        names::resolve(self.model(), name)
            .cloned()
            .map_err(|suggestions| {
                ToolError::new(format!("the model has no variable '{name}'"))
                    .with_suggestions(suggestions)
            })
    }

    /// Note that the edit writes `var`.
    fn writes(&mut self, var: &Variable) {
        self.written
            .insert(crate::canonicalize(var.get_ident()).into_owned());
    }

    /// Note that the edit adds the variable named `name`.
    fn creates(&mut self, name: &str) {
        self.created.insert(crate::canonicalize(name).into_owned());
    }

    /// Record that an AI made or edited what the edit adds and writes, as
    /// ISEE's AI information does (`after_ai_edit`), when the project
    /// records it: an upsert of each such variable as the edit leaves it,
    /// with its provenance.
    fn mark_provenance(
        &mut self,
        before: &datamodel::Model,
        tracks: bool,
    ) -> Result<(), ToolError> {
        if !tracks {
            return Ok(());
        }
        let renamed = rename_map(&self.renames);
        let origin: HashMap<&String, &String> =
            renamed.iter().map(|(from, to)| (to, from)).collect();
        let now: BTreeSet<String> = self
            .written
            .iter()
            .map(|name| renamed.get(name).unwrap_or(name).clone())
            .chain(self.created.iter().cloned())
            .collect();
        let mut ops = Vec::new();
        for name in &now {
            let Some(mut var) = self
                .model()
                .variables
                .iter()
                .find(|v| crate::canonicalize(v.get_ident()).as_ref() == name.as_str())
                .cloned()
            else {
                continue;
            };
            let state = if self.created.contains(name) {
                AiState::C
            } else {
                let was = origin.get(name).copied().unwrap_or(name);
                after_ai_edit(before.get_variable(was).and_then(Variable::get_ai_state))
            };
            if var.get_ai_state() == Some(state) {
                continue;
            }
            ops.push(match &mut var {
                Variable::Stock(stock) => {
                    stock.ai_state = Some(state);
                    ModelOperation::UpsertStock(stock.clone())
                }
                Variable::Flow(flow) => {
                    flow.ai_state = Some(state);
                    ModelOperation::UpsertFlow(flow.clone())
                }
                Variable::Aux(aux) => {
                    aux.ai_state = Some(state);
                    ModelOperation::UpsertAux(aux.clone())
                }
                Variable::Module(module) => {
                    module.ai_state = Some(state);
                    ModelOperation::UpsertModule(module.clone())
                }
            });
        }
        self.apply(ops)
    }

    /// `name`, if it can name a new variable: not empty, an identifier, and no
    /// variable's already.
    fn free(&self, name: &str) -> Result<String, ToolError> {
        let name = name.trim();
        let canonical = crate::canonicalize(name).into_owned();
        let is_identifier = matches!(
            crate::ast::Expr0::new(&canonical, crate::lexer::LexerType::Equation),
            Ok(Some(crate::ast::Expr0::Var(ref raw, _)))
                if raw.canonicalize().as_str() == canonical
        );
        if name.is_empty() || !is_identifier {
            return Err(ToolError::new(format!(
                "'{name}' is not a name a variable can have"
            )));
        }
        if self.model().get_variable(name).is_some() {
            return Err(ToolError::new(format!(
                "the model already has a variable '{name}'; change it with set_equation, or \
                 choose another name"
            )));
        }
        Ok(name.to_string())
    }

    /// Apply `ops` to the working copy and add them to the patch.
    fn apply(&mut self, ops: Vec<ModelOperation>) -> Result<(), ToolError> {
        crate::apply_patch(
            &mut self.working,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: self.model_name.clone(),
                    ops: ops.clone(),
                }],
            },
        )
        .map_err(|err| ToolError::new(format!("it cannot apply: {err}")))?;
        self.ops.extend(ops);
        Ok(())
    }

    /// The stock `name` names, which the edit writes.
    fn stock(&mut self, name: &str) -> Result<datamodel::Stock, ToolError> {
        match self.existing(name)? {
            Variable::Stock(stock) => {
                self.writes(&Variable::Stock(stock.clone()));
                Ok(stock)
            }
            other => Err(ToolError::new(format!(
                "'{}' is not a stock",
                other.get_ident()
            ))),
        }
    }

    /// The flows `names` name, as the model writes them.
    fn flows(&self, names: &[String]) -> Result<Vec<String>, ToolError> {
        names
            .iter()
            .map(|name| match self.existing(name)? {
                Variable::Flow(flow) => Ok(flow.ident),
                other => Err(ToolError::new(format!(
                    "'{}' is not a flow",
                    other.get_ident()
                ))),
            })
            .collect()
    }

    fn operate(&mut self, op: EditOperation) -> Result<(), ToolError> {
        match op {
            EditOperation::AddStock {
                name,
                initial,
                units,
                notes,
                inflows,
                outflows,
            } => {
                let name = self.free(&name)?;
                let stock = datamodel::Stock {
                    ident: name,
                    equation: Equation::Scalar(initial),
                    documentation: notes.unwrap_or_default(),
                    units: units.filter(|u| !u.trim().is_empty()),
                    inflows: self.flows(&inflows)?,
                    outflows: self.flows(&outflows)?,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                };
                self.writes(&Variable::Stock(stock.clone()));
                self.creates(&stock.ident);
                self.apply(vec![ModelOperation::UpsertStock(stock)])
            }
            EditOperation::AddFlow {
                name,
                equation,
                units,
                notes,
                from,
                to,
            } => {
                let name = self.free(&name)?;
                let flow = datamodel::Flow {
                    ident: name.clone(),
                    equation: Equation::Scalar(equation),
                    documentation: notes.unwrap_or_default(),
                    units: units.filter(|u| !u.trim().is_empty()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                };
                self.writes(&Variable::Flow(flow.clone()));
                self.creates(&flow.ident);
                self.apply(vec![ModelOperation::UpsertFlow(flow)])?;
                self.connect(&name, from.as_deref(), to.as_deref())
            }
            EditOperation::AddVariable {
                name,
                equation,
                units,
                notes,
            } => {
                let name = self.free(&name)?;
                let aux = datamodel::Aux {
                    ident: name,
                    equation: Equation::Scalar(equation),
                    documentation: notes.unwrap_or_default(),
                    units: units.filter(|u| !u.trim().is_empty()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                };
                self.writes(&Variable::Aux(aux.clone()));
                self.creates(&aux.ident);
                self.apply(vec![ModelOperation::UpsertAux(aux)])
            }
            EditOperation::SetEquation {
                variable,
                equation,
                element,
            } => {
                let mut var = self.existing(&variable)?;
                if equation.trim().is_empty() {
                    return Err(ToolError::new(format!(
                        "give '{}' an equation that is not empty",
                        var.get_ident()
                    )));
                }
                let new = self.equation(&var, &equation, element.as_deref())?;
                match &mut var {
                    Variable::Stock(stock) => stock.equation = new,
                    Variable::Flow(flow) => flow.equation = new,
                    Variable::Aux(aux) => aux.equation = new,
                    Variable::Module(_) => {
                        return Err(ToolError::new(format!(
                            "'{}' is a module, which has no equation",
                            var.get_ident()
                        )));
                    }
                }
                self.upsert(var)
            }
            EditOperation::SetUnits { variable, units } => {
                let mut var = self.existing(&variable)?;
                let units = Some(units.trim().to_string()).filter(|u| !u.is_empty());
                match &mut var {
                    Variable::Stock(stock) => stock.units = units,
                    Variable::Flow(flow) => flow.units = units,
                    Variable::Aux(aux) => aux.units = units,
                    Variable::Module(module) => module.units = units,
                }
                self.upsert(var)
            }
            EditOperation::SetNotes { variable, notes } => {
                let mut var = self.existing(&variable)?;
                let notes = notes.trim().to_string();
                match &mut var {
                    Variable::Stock(stock) => stock.documentation = notes,
                    Variable::Flow(flow) => flow.documentation = notes,
                    Variable::Aux(aux) => aux.documentation = notes,
                    Variable::Module(module) => module.documentation = notes,
                }
                self.upsert(var)
            }
            EditOperation::SetLookup {
                variable,
                points,
                kind,
            } => {
                let mut var = self.existing(&variable)?;
                let gf = lookup(var.get_ident(), &points, kind)?;
                match &mut var {
                    Variable::Flow(flow) => flow.gf = Some(gf),
                    Variable::Aux(aux) => aux.gf = Some(gf),
                    _ => {
                        return Err(ToolError::new(format!(
                            "'{}' cannot be a lookup: only an auxiliary or a flow can",
                            var.get_ident()
                        )));
                    }
                }
                self.upsert(var)
            }
            EditOperation::ConnectFlow { flow, from, to } => {
                let flow = match self.existing(&flow)? {
                    Variable::Flow(flow) => flow,
                    other => {
                        return Err(ToolError::new(format!(
                            "'{}' is not a flow",
                            other.get_ident()
                        )));
                    }
                };
                self.connect(&flow.ident, from.as_deref(), to.as_deref())
            }
            EditOperation::Rename { variable, to } => {
                let var = self.existing(&variable)?;
                let to = self.free(&to)?;
                self.writes(&var);
                if self
                    .created
                    .remove(crate::canonicalize(var.get_ident()).as_ref())
                {
                    self.creates(&to);
                }
                self.renames.push((var.get_ident().to_string(), to.clone()));
                self.apply(vec![ModelOperation::RenameVariable {
                    from: var.get_ident().to_string(),
                    to,
                }])
            }
            EditOperation::Delete { variable } => {
                let var = self.existing(&variable)?;
                self.writes(&var);
                let ident = var.get_ident().to_string();
                if let Variable::Flow(_) = var {
                    for stock in self.stocks_with(&ident) {
                        self.writes(&Variable::Stock(stock));
                    }
                }
                self.apply(vec![ModelOperation::DeleteVariable { ident }])
            }
            EditOperation::NameLoop {
                loop_id,
                variables,
                name,
                description,
            } => {
                if name.trim().is_empty() {
                    return Err(ToolError::new("give the loop a name that is not empty"));
                }
                let variables = match (loop_id, variables.is_empty()) {
                    (Some(id), true) => self.loop_variables(&id)?,
                    (None, false) => variables
                        .iter()
                        .map(|name| self.existing(name).map(|v| v.get_ident().to_string()))
                        .collect::<Result<Vec<String>, ToolError>>()?,
                    _ => {
                        return Err(ToolError::new(
                            "name a loop by its id (loop) or by its variables, one of them",
                        ));
                    }
                };
                self.loops_named.push(
                    variables
                        .iter()
                        .map(|v| crate::canonicalize(v).into_owned())
                        .collect(),
                );
                self.apply(vec![ModelOperation::SetLoopName {
                    variables,
                    name: name.trim().to_string(),
                    description: description.filter(|d| !d.trim().is_empty()),
                }])
            }
            EditOperation::SetSimSpecs {
                start,
                stop,
                dt,
                method,
            } => self.sim_specs(start, stop, dt, method),
        }
    }

    /// Upsert `var`, which the edit writes.
    fn upsert(&mut self, var: Variable) -> Result<(), ToolError> {
        self.writes(&var);
        let op = match var {
            Variable::Stock(stock) => ModelOperation::UpsertStock(stock),
            Variable::Flow(flow) => ModelOperation::UpsertFlow(flow),
            Variable::Aux(aux) => ModelOperation::UpsertAux(aux),
            Variable::Module(module) => ModelOperation::UpsertModule(module),
        };
        self.apply(vec![op])
    }

    /// The stocks that list the flow `ident` among their flows.
    fn stocks_with(&self, ident: &str) -> Vec<datamodel::Stock> {
        let canonical = crate::canonicalize(ident);
        self.model()
            .variables
            .iter()
            .filter_map(|v| match v {
                Variable::Stock(stock)
                    if stock
                        .inflows
                        .iter()
                        .chain(&stock.outflows)
                        .any(|f| crate::canonicalize(f) == canonical) =>
                {
                    Some(stock.clone())
                }
                _ => None,
            })
            .collect()
    }

    /// Connect the flow `ident` to drain `from` and fill `to`, leaving every
    /// stock it drained or filled before.
    fn connect(
        &mut self,
        ident: &str,
        from: Option<&str>,
        to: Option<&str>,
    ) -> Result<(), ToolError> {
        let canonical = crate::canonicalize(ident).into_owned();
        let from = from.map(|name| self.stock(name)).transpose()?;
        let to = to.map(|name| self.stock(name)).transpose()?;
        if let (Some(from), Some(to)) = (&from, &to)
            && crate::canonicalize(&from.ident) == crate::canonicalize(&to.ident)
        {
            return Err(ToolError::new(format!(
                "a flow cannot drain and fill the same stock, '{}'",
                from.ident
            )));
        }
        let mut stocks: Vec<datamodel::Stock> = self.stocks_with(ident);
        for stock in from.iter().chain(&to) {
            if !stocks
                .iter()
                .any(|s| crate::canonicalize(&s.ident) == crate::canonicalize(&stock.ident))
            {
                stocks.push(stock.clone());
            }
        }
        let is = |stock: &datamodel::Stock, side: &Option<datamodel::Stock>| {
            side.as_ref()
                .is_some_and(|s| crate::canonicalize(&s.ident) == crate::canonicalize(&stock.ident))
        };
        let mut ops = Vec::new();
        for stock in &stocks {
            self.writes(&Variable::Stock(stock.clone()));
            let keep = |list: &[String]| -> Vec<String> {
                list.iter()
                    .filter(|f| crate::canonicalize(f).as_ref() != canonical)
                    .cloned()
                    .collect()
            };
            let mut inflows = keep(&stock.inflows);
            let mut outflows = keep(&stock.outflows);
            if is(stock, &to) {
                inflows.push(ident.to_string());
            }
            if is(stock, &from) {
                outflows.push(ident.to_string());
            }
            ops.push(ModelOperation::UpdateStockFlows {
                ident: stock.ident.clone(),
                inflows,
                outflows,
            });
        }
        self.apply(ops)
    }

    /// `var`'s equation with `text` in place: of every element, or of the one
    /// `element` names.
    fn equation(
        &self,
        var: &Variable,
        text: &str,
        element: Option<&str>,
    ) -> Result<Equation, ToolError> {
        let Some(old) = var.get_equation() else {
            return Err(ToolError::new(format!(
                "'{}' has no equation",
                var.get_ident()
            )));
        };
        let text = text.to_string();
        let Some(element) = element else {
            return Ok(match old {
                Equation::Scalar(_) => Equation::Scalar(text),
                Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => {
                    Equation::ApplyToAll(dims.clone(), text)
                }
            });
        };
        let (dims, mut elements, default) = match old {
            Equation::Scalar(_) => {
                return Err(ToolError::new(format!(
                    "'{}' is not arrayed, so it has no element '{element}'",
                    var.get_ident()
                )));
            }
            Equation::ApplyToAll(dims, all) => {
                let elements = element_names(&self.working, dims)
                    .into_iter()
                    .map(|e| (e, all.clone(), None, None))
                    .collect::<Vec<_>>();
                (dims.clone(), elements, None)
            }
            Equation::Arrayed(dims, elements, default, _) => {
                (dims.clone(), elements.clone(), default.clone())
            }
        };
        let names = element_names(&self.working, &dims);
        let wanted = crate::canonicalize(element);
        let Some(name) = names.iter().find(|n| crate::canonicalize(n) == wanted) else {
            return Err(ToolError::new(format!(
                "'{}' has no element '{element}'",
                var.get_ident()
            ))
            .with_suggestions(names.into_iter().take(12).collect()));
        };
        match elements
            .iter_mut()
            .find(|(e, ..)| crate::canonicalize(e) == wanted)
        {
            Some(entry) => entry.1 = text,
            None => elements.push((name.clone(), text, None, None)),
        }
        Ok(Equation::Arrayed(dims, elements, default, false))
    }

    /// The variables of the loop this session calls `id`, as the model
    /// names them.
    fn loop_variables(&self, id: &str) -> Result<Vec<String>, ToolError> {
        let key = self.evidence.loop_key(id.trim()).ok_or_else(|| {
            ToolError::new(format!(
                "no loop has the id '{id}' in this session: loop ids come from analyze_loops"
            ))
        })?;
        let mut seen = HashSet::new();
        key.iter()
            .map(|node| crate::ltm::strip_subscript(node))
            .filter(|name| seen.insert(name.to_string()))
            .map(|name| {
                self.model()
                    .get_variable(name)
                    .map(|v| v.get_ident().to_string())
                    .ok_or_else(|| {
                        ToolError::new(format!(
                            "loop {id} goes through '{name}', which the model no longer has"
                        ))
                    })
            })
            .collect()
    }

    fn sim_specs(
        &mut self,
        start: Option<f64>,
        stop: Option<f64>,
        dt: Option<f64>,
        method: Option<IntegrationMethod>,
    ) -> Result<(), ToolError> {
        if self.model().sim_specs.is_some() {
            return Err(ToolError::new(
                "the model has sim specs of its own, which an edit cannot change",
            ));
        }
        let mut specs = self.working.sim_specs.clone();
        if let Some(start) = start {
            specs.start = start;
        }
        if let Some(stop) = stop {
            specs.stop = stop;
        }
        if let Some(dt) = dt {
            if !dt.is_finite() || dt <= 0.0 {
                return Err(ToolError::new("dt must be a number more than zero"));
            }
            specs.dt = datamodel::Dt::Dt(dt);
            if let Some(save) = &specs.save_step
                && dt_value(save) < dt
            {
                specs.save_step = None;
            }
        }
        if !specs.start.is_finite() || !specs.stop.is_finite() || specs.stop <= specs.start {
            return Err(ToolError::new(
                "the stop time must come after the start time",
            ));
        }
        if let Some(method) = method {
            specs.sim_method = match method {
                IntegrationMethod::Euler => datamodel::SimMethod::Euler,
                IntegrationMethod::Rk2 => datamodel::SimMethod::RungeKutta2,
                IntegrationMethod::Rk4 => datamodel::SimMethod::RungeKutta4,
            };
        }
        crate::apply_patch(
            &mut self.working,
            ProjectPatch {
                project_ops: vec![ProjectOperation::SetSimSpecs(specs.clone())],
                models: vec![],
            },
        )
        .map_err(|err| ToolError::new(format!("it cannot apply: {err}")))?;
        self.project_ops.push(ProjectOperation::SetSimSpecs(specs));
        self.writes_specs = true;
        Ok(())
    }

    /// Place what the edit adds on the model's diagram, inside the patch: an
    /// incremental layout of the model's first view, or a whole layout when
    /// it has none.
    ///
    /// The layout reaches the patch as an edit of the elements it changed,
    /// which keeps everything else the view holds (its font, what an MDL
    /// writer needs to write it back), when that edit implies no model
    /// operation of its own: an edit of equations, which adds and removes
    /// connectors. An edit that adds, deletes or renames variables replaces
    /// the view, keeping the view's own fields, since an element edit would
    /// create again a variable the patch has created; the JSON a plan
    /// travels in carries neither a view's font nor its MDL sketch metadata,
    /// so a view replaced through it keeps what the JSON holds.
    fn place(&mut self, before: &datamodel::Model) -> Result<(), String> {
        let old = before.views.first().map(|view| match view {
            datamodel::View::StockFlow(stock_flow) => stock_flow,
        });
        let patch = ModelPatch {
            name: self.model_name.clone(),
            ops: self.ops.clone(),
        };
        let mut view = match old {
            Some(old) => crate::layout::incremental_layout(
                old,
                &self.working,
                &self.model_name,
                &patch,
                None,
            )?,
            None => crate::layout::generate_best_layout(&self.working, &self.model_name, None)?,
        };
        // The view as the patch's model operations left it (a rename renames
        // its element).
        let current = self.model().views.first().map(|view| match view {
            datamodel::View::StockFlow(stock_flow) => stock_flow.clone(),
        });
        let op = match current.filter(|current| !current.elements.is_empty()) {
            Some(current) => {
                let was: HashMap<i32, &datamodel::ViewElement> =
                    current.elements.iter().map(|e| (e.get_uid(), e)).collect();
                let kept: HashSet<i32> = view.elements.iter().map(|e| e.get_uid()).collect();
                let upsert: Vec<datamodel::ViewElement> = view
                    .elements
                    .iter()
                    .filter(|e| was.get(&e.get_uid()) != Some(e))
                    .cloned()
                    .collect();
                let remove: Vec<i32> = current
                    .elements
                    .iter()
                    .map(|e| e.get_uid())
                    .filter(|uid| !kept.contains(uid))
                    .collect();
                let next = crate::editing::edited_view(&current, &upsert, &remove);
                let pure = crate::editing::derived_operations(self.model(), &current, &next)
                    .is_ok_and(|ops| ops.is_empty());
                if pure {
                    ModelOperation::EditView {
                        index: 0,
                        upsert,
                        remove,
                    }
                } else {
                    view.name = current.name.clone();
                    view.zoom = current.zoom;
                    view.use_lettered_polarity = current.use_lettered_polarity;
                    view.font = current.font.clone();
                    view.sketch_compat = current.sketch_compat.clone();
                    ModelOperation::UpsertView {
                        index: 0,
                        view: datamodel::View::StockFlow(view),
                    }
                }
            }
            None => {
                if let Some(old) = old
                    && old.zoom > 0.0
                {
                    view.zoom = old.zoom;
                }
                ModelOperation::UpsertView {
                    index: 0,
                    view: datamodel::View::StockFlow(view),
                }
            }
        };
        self.apply(vec![op]).map_err(|err| err.error)
    }

    /// The patch: the project operations, then the model's.
    fn patch(&self, model_name: &str) -> ProjectPatch {
        ProjectPatch {
            project_ops: self.project_ops.clone(),
            models: vec![ModelPatch {
                name: model_name.to_string(),
                ops: self.ops.clone(),
            }],
        }
    }
}

/// The element names of dimensions `dims`, joined by commas across
/// dimensions, in order.
fn element_names(project: &datamodel::Project, dims: &[String]) -> Vec<String> {
    let mut names = vec![String::new()];
    for dim in dims {
        let Some(dim) = project
            .dimensions
            .iter()
            .find(|d| crate::canonicalize(&d.name) == crate::canonicalize(dim))
        else {
            return vec![];
        };
        let elements: Vec<String> = match &dim.elements {
            datamodel::DimensionElements::Named(names) => names.clone(),
            datamodel::DimensionElements::Indexed(n) => (1..=*n).map(|i| i.to_string()).collect(),
        };
        names = names
            .iter()
            .flat_map(|prefix| {
                elements.iter().map(move |e| {
                    if prefix.is_empty() {
                        e.clone()
                    } else {
                        format!("{prefix},{e}")
                    }
                })
            })
            .collect();
    }
    names
}

/// A lookup through `points`, x increasing.
fn lookup(
    name: &str,
    points: &[[f64; 2]],
    shape: Option<LookupShape>,
) -> Result<datamodel::GraphicalFunction, ToolError> {
    if points.len() < 2 {
        return Err(ToolError::new(format!(
            "a lookup for '{name}' needs two points at least"
        )));
    }
    if points.iter().flatten().any(|v| !v.is_finite()) {
        return Err(ToolError::new(format!(
            "every point of '{name}''s lookup must be a number"
        )));
    }
    if points.windows(2).any(|w| w[1][0] <= w[0][0]) {
        return Err(ToolError::new(format!(
            "the x values of '{name}''s lookup must increase"
        )));
    }
    let scale = |values: Vec<f64>| datamodel::GraphicalFunctionScale {
        min: values.iter().copied().fold(f64::INFINITY, f64::min),
        max: values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    };
    let xs: Vec<f64> = points.iter().map(|p| p[0]).collect();
    let ys: Vec<f64> = points.iter().map(|p| p[1]).collect();
    Ok(datamodel::GraphicalFunction {
        kind: match shape.unwrap_or(LookupShape::Continuous) {
            LookupShape::Continuous => datamodel::GraphicalFunctionKind::Continuous,
            LookupShape::Extrapolate => datamodel::GraphicalFunctionKind::Extrapolate,
            LookupShape::Discrete => datamodel::GraphicalFunctionKind::Discrete,
        },
        x_scale: scale(xs.clone()),
        y_scale: scale(ys.clone()),
        x_points: Some(xs),
        y_points: ys,
    })
}

/// What the gate found of an edit.
struct Gate {
    /// Errors the model did not have, by code and variable.
    new_errors: Vec<Described>,
    /// Warnings the model did not have, by code and variable, of every
    /// category.
    new_warnings: Vec<Described>,
    /// Whether the edit gives the model its first unit warning.
    first_unit_warning: bool,
    /// The variables (or elements) that are not a number somewhere in the
    /// edit's run and a number throughout the model's, with when they first
    /// are not, as the model names them.
    non_finite: Vec<(String, f64)>,
    simulates: bool,
    run_error: Option<String>,
    /// Why the edited model's run would cost more than a run may: specs that
    /// ask for more than the model's own and more than the limit.
    too_costly: Option<String>,
}

impl Gate {
    /// Why the gate refuses the edit, if it does, for a model that
    /// `simulated` before it.
    ///
    /// The gate tolerates, by design: an error the model had already, by
    /// code and variable, so a broken model can be repaired a step at a time
    /// (a second error of a code a variable has already is tolerated with
    /// it); errors in another model of the project, a module's, which count
    /// through whether the model simulates; values not a number in a model
    /// that did not simulate before, which has no run to compare with; and
    /// warnings, which it lists, but for a unit-clean model's first unit
    /// warning, which a person's patch is refused for too.
    fn refusal(&self, simulated: bool) -> Option<String> {
        let named = |d: &Described| {
            let what = d
                .reason
                .clone()
                .unwrap_or_else(|| d.code.description().to_string());
            match &d.variable {
                Some(variable) => format!("{variable}: {what}"),
                None => what,
            }
        };
        if let Some(error) = self.new_errors.first() {
            return Some(format!("It adds an error ({}).", named(error)));
        }
        if let Some(reason) = &self.too_costly {
            return Some(format!("It asks more of a run than a run may: {reason}."));
        }
        if simulated && !self.simulates {
            return Some(format!(
                "The model would not simulate: {}.",
                self.run_error.as_deref().unwrap_or("its build fails")
            ));
        }
        if let Some((variable, time)) = self.non_finite.first() {
            return Some(format!(
                "It makes {variable} not a number from time {time}."
            ));
        }
        if self.first_unit_warning {
            let first = self
                .new_warnings
                .iter()
                .find(|d| is_unit(d.category))
                .map(named)
                .unwrap_or_default();
            return Some(format!(
                "It gives a model without unit warnings its first ({first}): fix the units or \
                 the equation."
            ));
        }
        None
    }
}

fn is_unit(category: DiagnosticCategoryName) -> bool {
    matches!(
        category,
        DiagnosticCategoryName::UnitDefinition
            | DiagnosticCategoryName::UnitConsistency
            | DiagnosticCategoryName::UnitInference
    )
}

/// Stage `working` on the host's database, compare its diagnostics for the
/// model with the model's own by code and variable (the model's mapped
/// through the edit's `renames`), see whether it simulates, within what a
/// run may cost, and compare its run with `base`, the model's own; restore
/// the database whatever it finds, a panic included (`runs::Staging`).
///
/// Its two stages -- the staged model's diagnostics, and its run -- are units
/// of the call's work: it stops before each when other work waits for the
/// project, and the staging guard restores the database as it stops.
fn gate(
    ws: &mut Workspace<'_>,
    model_name: &str,
    working: &datamodel::Project,
    renames: &[(String, String)],
    base: Option<&Run>,
) -> Result<Gate, ToolError> {
    let canonical = crate::canonicalize(model_name).into_owned();
    let of_model = |d: &crate::db::Diagnostic| {
        d.model.is_empty() || crate::canonicalize(&d.model).as_ref() == canonical
    };
    let working_model = working
        .get_model(model_name)
        .expect("the edit keeps the model");
    let before_model = ws.project.get_model(model_name).expect("the model exists");
    let renamed = rename_map(renames);
    type Key = (String, Option<String>);
    let keys = |described: &[Described], severity: Severity, rename: bool| -> HashSet<Key> {
        described
            .iter()
            .filter(|d| d.severity == severity)
            .map(|d| {
                let variable = d.variable.clone().map(|v| match rename {
                    true => renamed.get(&v).cloned().unwrap_or(v),
                    false => v,
                });
                (d.code.to_string(), variable)
            })
            .collect()
    };

    let before: Vec<Described> = ws.db.current_source_project().map_or_else(Vec::new, |sp| {
        collect_all_diagnostics(ws.db, sp, LtmOverlay::Off)
            .iter()
            .filter(|d| of_model(d))
            .map(|d| describe(d, ws.project, before_model))
            .collect()
    });
    let before_errors = keys(&before, Severity::Error, true);
    let before_warnings = keys(&before, Severity::Warning, true);
    let had_unit_warnings = before
        .iter()
        .any(|d| d.severity == Severity::Warning && is_unit(d.category));

    let own = crate::results::Specs::from(effective_specs(ws.project, before_model));
    ws.yield_point()?;
    let waiting = ws.waiting;
    let staging = Staging::new(ws.db, ws.project, working);
    let staged = staging.source_project;
    let after: Vec<Described> = collect_all_diagnostics(staging.db, staged, LtmOverlay::Off)
        .iter()
        .filter(|d| of_model(d))
        .map(|d| describe(d, working, working_model))
        .collect();
    if waiting.is_some_and(|waiting| waiting()) {
        return Err(ToolError::interrupted());
    }
    let mut too_costly = None;
    let run = match build_vm(
        staging.db,
        staged,
        working,
        model_name,
        LtmOverlay::Off,
        &own,
    ) {
        Ok(mut vm) => vm.run_to_end().map(|()| vm.into_results()),
        Err(Unbuilt::Compile(err)) => Err(err),
        Err(Unbuilt::Cost(reason)) => {
            too_costly = Some(reason);
            Err(crate::common::Error::new(
                crate::common::ErrorKind::Simulation,
                crate::common::ErrorCode::Generic,
                too_costly.clone(),
            ))
        }
    };
    drop(staging);

    let key = |d: &Described| (d.code.to_string(), d.variable.clone());
    let (new_errors, rest): (Vec<Described>, Vec<Described>) = after
        .into_iter()
        .partition(|d| d.severity == Severity::Error && !before_errors.contains(&key(d)));
    let new_warnings: Vec<Described> = rest
        .into_iter()
        .filter(|d| d.severity == Severity::Warning && !before_warnings.contains(&key(d)))
        .collect();
    let first_unit_warning = !had_unit_warnings && new_warnings.iter().any(|d| is_unit(d.category));
    let non_finite = match (&run, base) {
        (Ok(results), Some(base)) => {
            newly_non_finite(results, base, working_model, &rename_map(renames))
        }
        _ => vec![],
    };
    Ok(Gate {
        new_errors,
        new_warnings,
        first_unit_warning,
        non_finite,
        simulates: run.is_ok(),
        run_error: run.err().map(|err| err.reason().to_string()),
        too_costly,
    })
}

/// The series of `results` not a number somewhere, whose series in `base`
/// (the model's run, under the name the edit's renames came from) is a
/// number throughout, or that `base` lacks: each with when it first is not,
/// labeled as `model` names it, earliest first.
fn newly_non_finite(
    results: &crate::Results,
    base: &Run,
    model: &datamodel::Model,
    renamed: &BTreeMap<String, String>,
) -> Vec<(String, f64)> {
    let origin: HashMap<&str, &str> = renamed
        .iter()
        .map(|(from, to)| (to.as_str(), from.as_str()))
        .collect();
    let rows: Vec<&[f64]> = results.iter().collect();
    let declared: BTreeSet<String> = model
        .variables
        .iter()
        .map(|var| crate::canonicalize(var.get_ident()).into_owned())
        .collect();
    let mut found: Vec<(String, f64)> = Vec::new();
    for (key, &offset) in &results.offsets {
        let key = key.as_str();
        // The clock and the compiler's helpers are no variable's series; a
        // variable's name may hold `$`, `[` or the word `time`.
        let Some(owner) = crate::save_check::column_variable(key, &declared) else {
            continue;
        };
        let Some(row) = rows.iter().position(|r| !r[offset].is_finite()) else {
            continue;
        };
        let variable = if owner == key {
            key
        } else {
            crate::ltm::strip_subscript(key)
        };
        let subscript = &key[variable.len()..];
        let base_key = format!(
            "{}{subscript}",
            origin.get(variable).copied().unwrap_or(variable)
        );
        let was_finite = base
            .results
            .offsets
            .get(&crate::common::Ident::<crate::common::Canonical>::new(
                &base_key,
            ))
            .is_none_or(|&offset| base.series(offset).iter().all(|v| v.is_finite()));
        if was_finite {
            let label = format!("{}{subscript}", display_name(model, variable));
            found.push((
                label,
                crate::tools::series::round(rows[row][crate::results::TIME_OFF]),
            ));
        }
    }
    found.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    found
}

/// One line per variable the edit changes, then loop names and sim specs.
fn planned_changes(
    before: &datamodel::Project,
    after: &datamodel::Project,
    model_name: &str,
    renames: &[(String, String)],
) -> Vec<PlannedChange> {
    let (Some(old), Some(new)) = (before.get_model(model_name), after.get_model(model_name)) else {
        return vec![];
    };
    let by_name = |model: &datamodel::Model| -> BTreeMap<String, Variable> {
        model
            .variables
            .iter()
            .map(|v| (crate::canonicalize(v.get_ident()).into_owned(), v.clone()))
            .collect()
    };
    let (old_vars, new_vars) = (by_name(old), by_name(new));
    // A rename is one line, from the old name to the new.
    let renamed = rename_map(renames);
    let mut changes = Vec::new();
    for var in &old.variables {
        let name = crate::canonicalize(var.get_ident()).into_owned();
        let now_name = renamed.get(&name).unwrap_or(&name);
        let Some(now) = new_vars.get(now_name) else {
            changes.push(PlannedChange {
                variable: var.get_ident().to_string(),
                action: ChangeAction::Deleted,
                detail: format!("deleted {}", kind_name(var)),
            });
            continue;
        };
        let mut details = Vec::new();
        if now_name != &name {
            details.push(format!("renamed to {}", now.get_ident()));
        }
        details.extend(
            changed_fields(var, now)
                .into_iter()
                // A rename says so itself.
                .filter(|field| !(*field == ChangedField::Name && now_name != &name))
                .map(|field| field_detail(field, var, now)),
        );
        if !details.is_empty() {
            changes.push(PlannedChange {
                variable: var.get_ident().to_string(),
                action: if now_name != &name {
                    ChangeAction::Renamed
                } else {
                    ChangeAction::Changed
                },
                detail: details.join("; "),
            });
        }
    }
    let arrived: HashSet<&String> = renamed.values().collect();
    for var in &new.variables {
        let name = crate::canonicalize(var.get_ident()).into_owned();
        if old_vars.contains_key(&name) || arrived.contains(&name) {
            continue;
        }
        changes.push(PlannedChange {
            variable: var.get_ident().to_string(),
            action: ChangeAction::Added,
            detail: added_detail(var),
        });
    }
    for loop_name in &new.loop_metadata {
        let unchanged = old.loop_metadata.iter().any(|l| {
            l.uids == loop_name.uids && l.name == loop_name.name && l.deleted == loop_name.deleted
        });
        if unchanged || loop_name.deleted {
            continue;
        }
        let through: Vec<String> = loop_name
            .uids
            .iter()
            .filter_map(|uid| {
                new.variables
                    .iter()
                    .find(|v| crate::patch::variable_uid(v) == Some(*uid))
                    .map(|v| v.get_ident().to_string())
            })
            .collect();
        changes.push(PlannedChange {
            variable: loop_name.name.clone(),
            action: ChangeAction::LoopNamed,
            detail: format!("names the loop through {}", through.join(", ")),
        });
    }
    let (old_specs, new_specs) = (effective_specs(before, old), effective_specs(after, new));
    let mut specs = Vec::new();
    for (field, was, is) in [
        ("start", old_specs.start, new_specs.start),
        ("stop", old_specs.stop, new_specs.stop),
        ("dt", dt_value(&old_specs.dt), dt_value(&new_specs.dt)),
    ] {
        if was != is {
            specs.push(format!("{field} {is} (was {was})"));
        }
    }
    if old_specs.sim_method != new_specs.sim_method {
        specs.push(format!(
            "method {} (was {})",
            method_name(new_specs.sim_method),
            method_name(old_specs.sim_method)
        ));
    }
    if !specs.is_empty() {
        changes.push(PlannedChange {
            variable: "sim specs".to_string(),
            action: ChangeAction::SimSpecs,
            detail: specs.join("; "),
        });
    }
    changes
}

fn method_name(method: datamodel::SimMethod) -> &'static str {
    match method {
        datamodel::SimMethod::Euler => "euler",
        datamodel::SimMethod::RungeKutta2 => "rk2",
        datamodel::SimMethod::RungeKutta4 => "rk4",
    }
}

fn kind_name(var: &Variable) -> &'static str {
    match var {
        Variable::Stock(_) => "stock",
        Variable::Flow(_) => "flow",
        Variable::Aux(_) => "variable",
        Variable::Module(_) => "module",
    }
}

fn equation_of(var: &Variable) -> String {
    var.get_equation().map(equation_text).unwrap_or_default()
}

fn added_detail(var: &Variable) -> String {
    let mut detail = match var {
        Variable::Stock(stock) => {
            let mut detail = format!("stock, initial value {}", equation_of(var));
            if !stock.inflows.is_empty() {
                detail += &format!(", filled by {}", stock.inflows.join(", "));
            }
            if !stock.outflows.is_empty() {
                detail += &format!(", drained by {}", stock.outflows.join(", "));
            }
            detail
        }
        Variable::Flow(_) => format!("flow = {}", equation_of(var)),
        Variable::Aux(_) => format!("variable = {}", equation_of(var)),
        Variable::Module(_) => "module".to_string(),
    };
    if let Some(units) = var.get_units() {
        detail += &format!(" ({units})");
    }
    detail
}

fn field_detail(field: ChangedField, was: &Variable, is: &Variable) -> String {
    let list = |flows: &[String]| {
        if flows.is_empty() {
            "none".to_string()
        } else {
            flows.join(", ")
        }
    };
    let flows = |var: &Variable, inflows: bool| match var {
        Variable::Stock(stock) => list(if inflows {
            &stock.inflows
        } else {
            &stock.outflows
        }),
        _ => "none".to_string(),
    };
    match field {
        ChangedField::Kind => format!("now a {} (was a {})", kind_name(is), kind_name(was)),
        ChangedField::Equation => {
            let what = if matches!(is, Variable::Stock(_)) {
                "initial value"
            } else {
                "equation"
            };
            format!("{what} {} (was {})", equation_of(is), equation_of(was))
        }
        ChangedField::Units => format!(
            "units {} (was {})",
            is.get_units().map_or("none", String::as_str),
            was.get_units().map_or("none", String::as_str)
        ),
        ChangedField::Documentation => "notes".to_string(),
        ChangedField::Lookup => "lookup".to_string(),
        ChangedField::Inflows => format!("inflows {} (was {})", flows(is, true), flows(was, true)),
        ChangedField::Outflows => {
            format!("outflows {} (was {})", flows(is, false), flows(was, false))
        }
        ChangedField::NonNegative => "non-negative".to_string(),
        ChangedField::Module => "module wiring".to_string(),
        ChangedField::Name => format!("written {} (was {})", is.get_ident(), was.get_ident()),
        ChangedField::Other => "other settings".to_string(),
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
