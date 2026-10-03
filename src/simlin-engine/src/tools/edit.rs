// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `edit_model`: an edit of the model, made when the gate passes it.
//!
//! An agent describes an edit as operations -- add a stock, set an equation,
//! connect a flow, rename, delete, name a loop, change the sim specs -- and
//! the call makes it or refuses it:
//!
//! 1. **One edit.** The operations are applied in order to a copy of the
//!    project, so each sees the ones before it. A field edit keeps every field
//!    it does not set. What the edit changes is its [`Delta`]: every variable
//!    whose record differs, in any model of the project (a rename's rewritten
//!    readers, the stocks a flow leaves, the modules that instantiate the
//!    model), each with its record before and after, the sim specs, and the
//!    loop names.
//! 2. **Fresh.** Everything in the delta must be, before the edit, as the
//!    session last read it with `read_model`, so an agent never overwrites
//!    work it has not seen: a write to something that changed since the read
//!    is refused, naming it. Who made a variable and its uid are not the
//!    variable (`changes::same_record`). An edit before any read is refused.
//! 3. **The gate.** The edited project is staged on the host's database and
//!    judged by a [`GatePolicy`], against the project as it is ([`gate`]):
//!    its diagnostics, every model's, by the evidence module's identity of a
//!    problem; whether the session's model and the project's root model still
//!    simulate; and their runs' values. `Gate::refusal` states what it
//!    refuses and tolerates.
//! 4. **Placed.** An edit the gate passes, which can change what the diagram
//!    draws, places what it adds with the engine's incremental layout, around
//!    the diagram as it is.
//! 5. **Made.** An edit the gate passes is made: the call answers with the
//!    project as the edit leaves it (`ToolOutput::edited`), which the host
//!    makes its project's contents in one edit, one step of its undo. The
//!    engine changes no project itself. The session then holds everything
//!    the edit changed as read (`Delta::absorbed_by`), so the agent's next
//!    edit is fresh without another read and the change report tells it only
//!    what someone else changed. An edit the gate refuses, or a call that
//!    stops, changes nothing.
//! 6. **The answer.** One line per variable the edit changes (the delta in
//!    words, `change_lines`), the diagnostics it adds -- each one a made edit
//!    leaves in the model under the id a read gives it -- and whether the
//!    model simulates, fitted to the answer budget; an edit that changes
//!    nothing says so (`unchanged`). An edit the gate refuses is a refusal,
//!    `is_error` set, in the one refusal shape every tool answers with: its
//!    reason, and in `refusedEdit` ([`RefusedEdit`]) the rule that refused it
//!    ([`GateRule`]) and the lines and diagnostics the edit would have had.
//! 7. **Provenance.** In a project that records who made its variables, what
//!    the edit adds is marked made by AI and what it edits marked edited by
//!    AI (`after_ai_edit`).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::common::CanonicalElementName;
use crate::datamodel::{self, AiState, Equation, Variable};
use crate::db::{LtmOverlay, SourceProject, collect_all_diagnostics};
use crate::patch::{ModelOperation, ModelPatch, ProjectOperation, ProjectPatch};

use super::changes::{ChangedField, ReadSnapshot, changed_fields, effective_specs, same_record};
use super::evidence::{Described, DiagnosticIdentity, describe, display_name};
use super::outline::{IntegrationMethod, dimensions, dt_value, equation_text};
use super::runs::{RunStore, Staging, Unbuilt, build_vm};
use super::{
    DiagnosticCategoryName, Session, Severity, ToolError, Workspace, names,
    resolve_datamodel_model, resolve_model,
};

/// The most operations one edit takes.
pub(crate) const MAX_OPERATIONS: usize = 24;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EditModelInput {
    /// What the edit does and why, in a sentence: what the person whose
    /// model it is reads of it, beside its undo.
    pub summary: String,
    /// The operations, applied in order: each sees the ones before it. At
    /// most 24.
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 24)))]
    pub operations: Vec<EditOperation>,
}

/// One operation of an edit. An equation names a variable as the equation
/// language spells it: `room_heat`, or `"Room Heat"` in quotes, for a
/// variable shown as Room Heat.
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
    /// one `element` names (`north`; `north, young` for an element of several
    /// dimensions).
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
    /// x never decreasing (two points at one x are a vertical step).
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
    /// Rename a variable; every equation that reads it is rewritten. A new
    /// name that differs only in case or spacing changes how the name is
    /// written and nothing else.
    Rename { variable: String, to: String },
    /// Delete a variable; a deleted flow leaves the stocks it filled and
    /// drained, and a module input it fed is left unwired.
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

    /// The equation text the operation writes, if any: what is bounded
    /// before anything parses it (`input::equation_too_deep`).
    fn equation_text(&self) -> Option<&str> {
        match self {
            EditOperation::AddStock { initial, .. } => Some(initial),
            EditOperation::AddFlow { equation, .. }
            | EditOperation::AddVariable { equation, .. }
            | EditOperation::SetEquation { equation, .. } => Some(equation),
            EditOperation::SetUnits { .. }
            | EditOperation::SetNotes { .. }
            | EditOperation::SetLookup { .. }
            | EditOperation::ConnectFlow { .. }
            | EditOperation::Rename { .. }
            | EditOperation::Delete { .. }
            | EditOperation::NameLoop { .. }
            | EditOperation::SetSimSpecs { .. } => None,
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
    /// The revision of the model the edit was made on. The edited model has
    /// a revision of its own, which the host gives it and the next read
    /// reports.
    pub revision: u64,
    pub summary: String,
    /// One line per variable the edit changes, in any model of the project,
    /// and for the sim specs and loop names. A line's detail is cut at 240
    /// characters.
    pub changes: Vec<ChangeLine>,
    /// Lines left out to keep the answer within its budget.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted: usize,
    /// Diagnostics left out to keep the answer within its budget, the last
    /// listed: a read lists them all.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted_diagnostics: usize,
    /// That the model was already as the edit asks: nothing changed, and
    /// there is no edit for the host to make.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unchanged: bool,
    /// What the edit adds or leaves: every warning it adds (to fix), and an
    /// error whose reason it changes, each under the id a read gives it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<EditDiagnostic>,
    /// Whether the model simulates as the edit leaves it; absent when the
    /// edit changed nothing (`unchanged`), which compiles and runs nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simulates: Option<bool>,
    /// Anything else that needs saying of the edit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What a refusal of an edit by its gate carries beside its reason
/// (`refusedEdit` in the one refusal shape): the rule that refused it and
/// what the edit would have done, for the agent to repair its operations.
/// Nothing changed.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefusedEdit {
    /// The rule that refused the edit.
    pub rule: GateRule,
    /// One line per variable the edit would have changed, as a made edit's
    /// lines say them. A line's detail is cut at 240 characters.
    pub changes: Vec<ChangeLine>,
    /// Lines left out to keep the refusal within its budget.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted: usize,
    /// Diagnostics left out to keep the refusal within its budget, the last
    /// listed.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted_diagnostics: usize,
    /// What the edit would add or leave: the errors it would add, an error
    /// in an equation it writes, the values that would not be a number, the
    /// first unit warning of a model without one, and the warnings it would
    /// add. None has an id: they are of no model.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<EditDiagnostic>,
    /// Whether the model would simulate as the edit would leave it.
    pub simulates: bool,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// A rule of the gate, as a refusal names the one that refused an edit.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GateRule {
    /// The edit adds an error the project did not have, in any of its
    /// models, or leaves one in an equation it writes.
    Errors,
    /// The edit's specs ask more of a run than a run may.
    RunCost,
    /// A model that simulated would not: the model the edit is of, or the
    /// project's root model.
    Simulation,
    /// A value would not be a number where the run of the model as it is
    /// has a number throughout.
    Values,
    /// A model without unit warnings would get its first.
    UnitWarnings,
}

impl GateRule {
    /// Every rule, in the order the gate asks them.
    pub(crate) const ALL: [GateRule; 5] = [
        GateRule::Errors,
        GateRule::RunCost,
        GateRule::Simulation,
        GateRule::Values,
        GateRule::UnitWarnings,
    ];
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ChangeLine {
    /// The variable, as the model names it (before a rename; with its model
    /// when it is another model's), a loop's name, or "sim specs".
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

/// A diagnostic the edit adds or leaves, or would.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct EditDiagnostic {
    /// The session's id for it (`D1`, ...), the one `read_model` reports it
    /// under, so a finding can cite it: present once the edit is made, for a
    /// diagnostic of the model the edit is of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub severity: Severity,
    pub category: DiagnosticCategoryName,
    pub code: String,
    /// The model it is in, when that is not the model the edit is of.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// A loop's name, as a model's loop metadata gives it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
struct LoopName {
    name: String,
    description: String,
    deleted: bool,
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

/// A variable's record before and after an edit (none: absent).
#[derive(Clone)]
struct VariableChange {
    before: Option<Variable>,
    after: Option<Variable>,
}

impl VariableChange {
    /// The variable, as a refusal names it: as the model spells it, with its
    /// model when that is not `session_model`.
    fn label(&self, model: &str, name: &str, session_model: &str) -> String {
        let ident = self
            .before
            .as_ref()
            .or(self.after.as_ref())
            .map_or(name, Variable::get_ident);
        in_model(ident, model, session_model)
    }
}

/// `ident`, with its model when that is not `session_model`.
fn in_model(ident: &str, model: &str, session_model: &str) -> String {
    if model == session_model {
        ident.to_string()
    } else {
        format!("{ident} (in {model})")
    }
}

/// A loop's name before and after an edit that changes it (none: unnamed),
/// with the variables the loop goes through as the model spells them.
#[derive(Clone)]
struct LoopChange {
    through: Vec<String>,
    before: Option<LoopName>,
    after: Option<LoopName>,
}

/// What an edit changes of a project: what must be as the session read it
/// for the edit to be made, and what the session holds as read once it is.
///
/// It holds every variable whose record the edit changes, in any model, by
/// its model's name and its canonical name, with the record before and
/// after (a rename is the old name gone and the new one there); the
/// project's sim specs as they were, when the edit changes them; and every
/// loop name it changes, by its model and its variables' uids. The answer's
/// lines are this in words (`change_lines`). Records compare by
/// `changes::same_record`. Views and groups are no part of it: a diagram is
/// nothing an agent reads.
#[derive(Clone, Default)]
pub(crate) struct Delta {
    variables: BTreeMap<(String, String), VariableChange>,
    specs: Option<datamodel::SimSpecs>,
    loops: BTreeMap<(String, Vec<i32>), LoopChange>,
}

/// A model's variables by their canonical names.
fn by_name(model: &datamodel::Model) -> BTreeMap<String, &Variable> {
    model
        .variables
        .iter()
        .map(|v| (crate::canonicalize(v.get_ident()).into_owned(), v))
        .collect()
}

/// A model's loop names by their variables' uids, sorted.
fn loop_names(model: &datamodel::Model) -> BTreeMap<Vec<i32>, LoopName> {
    model
        .loop_metadata
        .iter()
        .map(|entry| {
            let mut uids = entry.uids.clone();
            uids.sort_unstable();
            (uids, LoopName::from(entry))
        })
        .collect()
}

impl Delta {
    /// Whether the edit changes nothing an agent reads: no record, no sim
    /// specs, no loop name.
    fn is_empty(&self) -> bool {
        self.variables.is_empty() && self.specs.is_none() && self.loops.is_empty()
    }

    /// The variables the edit leaves out of the project, deleted or renamed
    /// away: `(model, canonical name, as the model spelled it)`.
    fn gone(&self) -> Vec<(String, String, String)> {
        self.variables
            .iter()
            .filter_map(
                |((model, name), change)| match (&change.before, &change.after) {
                    (Some(before), None) => {
                        Some((model.clone(), name.clone(), before.get_ident().to_string()))
                    }
                    _ => None,
                },
            )
            .collect()
    }

    /// What differs between `before` and `after`, a copy of it an edit was
    /// applied to. An edit adds and removes no model, so models pair by name.
    fn between(before: &datamodel::Project, after: &datamodel::Project) -> Delta {
        let mut delta = Delta::default();
        for was in &before.models {
            let Some(is) = after.models.iter().find(|m| m.name == was.name) else {
                continue;
            };
            // Most models an edit leaves alone, and most variables of the one
            // it edits: they are the very same allocations (`SharedVec`),
            // which is cheaper to see than records are to compare.
            let untouched = was.variables.len() == is.variables.len()
                && was
                    .variables
                    .iter()
                    .zip(is.variables.iter())
                    .all(|(a, b)| std::ptr::eq(a, b));
            if !untouched {
                let (old, new) = (by_name(was), by_name(is));
                let names: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
                for name in names {
                    let (b, a) = (old.get(name).copied(), new.get(name).copied());
                    let shared = matches!((b, a), (Some(b), Some(a)) if std::ptr::eq(b, a));
                    if !shared && !same_record(b, a) {
                        delta.variables.insert(
                            (was.name.clone(), name.clone()),
                            VariableChange {
                                before: b.cloned(),
                                after: a.cloned(),
                            },
                        );
                    }
                }
            }
            let (old, new) = (loop_names(was), loop_names(is));
            let uids: BTreeSet<&Vec<i32>> = old.keys().chain(new.keys()).collect();
            for uids in uids {
                let (b, a) = (old.get(uids), new.get(uids));
                if b != a {
                    let through = uids
                        .iter()
                        .filter_map(|uid| {
                            is.variables
                                .iter()
                                .find(|v| crate::patch::variable_uid(v) == Some(*uid))
                                .map(|v| v.get_ident().to_string())
                        })
                        .collect();
                    delta.loops.insert(
                        (was.name.clone(), uids.clone()),
                        LoopChange {
                            through,
                            before: b.cloned(),
                            after: a.cloned(),
                        },
                    );
                }
            }
        }
        if before.sim_specs != after.sim_specs {
            delta.specs = Some(before.sim_specs.clone());
        }
        delta
    }

    /// What the edit would overwrite that is not as `snapshot`, the session's
    /// last read of `session_model`, gave it, named for a refusal.
    fn stale_since_read(&self, session_model: &str, snapshot: &ReadSnapshot) -> Vec<String> {
        let mut stale = Vec::new();
        for ((model, name), change) in &self.variables {
            let read = if model == session_model {
                snapshot.record(name)
            } else {
                snapshot.record_in(model, name)
            };
            if !same_record(change.before.as_ref(), read) {
                stale.push(change.label(model, name, session_model));
            }
        }
        if let Some(before) = &self.specs
            && before != snapshot.specs()
        {
            stale.push("the sim specs".to_string());
        }
        for ((model, uids), change) in &self.loops {
            // A read holds the loop names of the session's model alone, the
            // only ones an edit names.
            let read = (model == session_model)
                .then(|| snapshot.loop_named(uids).map(LoopName::from))
                .flatten();
            if read != change.before {
                stale.push(change.label());
            }
        }
        stale
    }

    /// Hold everything the edit changed as read in `snapshot`, the session's
    /// last read of `session_model`: each variable as the edit leaves it, the
    /// sim specs and each loop name, read off `edited`, the project as the
    /// edit leaves it.
    fn absorbed_by(
        &self,
        snapshot: &mut ReadSnapshot,
        session_model: &str,
        edited: &datamodel::Project,
    ) {
        for ((model, name), change) in &self.variables {
            let other = (model != session_model).then_some(model.as_str());
            snapshot.absorb_variable(other, name, change.after.as_ref());
        }
        if self.specs.is_some()
            && let Some(model) = edited.models.iter().find(|m| m.name == session_model)
        {
            snapshot.absorb_specs(effective_specs(edited, model));
        }
        for (model, uids) in self.loops.keys() {
            if model == session_model
                && let Some(model) = edited.models.iter().find(|m| m.name == *model)
            {
                let entry = model.loop_metadata.iter().find(|entry| {
                    let mut entry_uids = entry.uids.clone();
                    entry_uids.sort_unstable();
                    entry_uids == *uids
                });
                snapshot.absorb_loop(uids, entry);
            }
        }
    }
}

impl LoopChange {
    fn label(&self) -> String {
        format!("the name of the loop through {}", self.through.join(", "))
    }
}

/// Make the edit `input` describes of the session's model, when the gate
/// passes it: the answer, and the project as the edit leaves it when it is
/// made and changes the project.
pub(crate) fn edit_model(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: EditModelInput,
) -> Result<(EditModelOutput, Option<datamodel::Project>), ToolError> {
    if input.operations.is_empty() || input.operations.len() > MAX_OPERATIONS {
        return Err(ToolError::new(format!(
            "an edit has between 1 and {MAX_OPERATIONS} operations (this one has {})",
            input.operations.len()
        )));
    }
    if input.summary.trim().is_empty() {
        return Err(ToolError::new(
            "say what the edit does in its summary, for the person whose model it is",
        ));
    }
    // An equation deep enough overflows the stack of whatever parses it, so
    // its depth is read from its tokens before anything does.
    for (i, op) in input.operations.iter().enumerate() {
        if let Some(too_deep) = op.equation_text().and_then(super::input::equation_too_deep) {
            return Err(ToolError::new(format!(
                "operation {} ({}): {too_deep}",
                i + 1,
                op.name()
            )));
        }
    }
    let model = resolve_model(ws.project, ws.db, &session.model_name)?.model;
    let Some(snapshot) = session.last_read.as_ref() else {
        return Err(ToolError::new(
            "read_model first: an edit is made of what you last read",
        ));
    };
    let built = Built::new(ws.project, model, &session.evidence, &input.operations)?;
    let stale = built.delta.stale_since_read(&model.name, snapshot);
    if !stale.is_empty() {
        // The changed variables named, up to a few: the read lists them all.
        let named: Vec<&str> = stale
            .iter()
            .take(super::MAX_NAMED)
            .map(String::as_str)
            .collect();
        let more = stale.len().saturating_sub(super::MAX_NAMED);
        return Err(ToolError::new(format!(
            "{}{} changed since you last read the model; read_model again before editing",
            named.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            }
        )));
    }
    // What the edit changes is its delta. One that changes no record leaves
    // the project as it was, whatever else its operations wrote (an empty
    // units string where there were none): it hands the host nothing, and
    // needs no gate, which would compile and run the model to judge nothing.
    if built.delta.is_empty() {
        let output = EditModelOutput {
            revision: ws.revision,
            summary: super::evidence::echo(&input.summary),
            changes: vec![],
            omitted: 0,
            omitted_diagnostics: 0,
            unchanged: true,
            diagnostics: vec![],
            simulates: None,
            note: Some("The model is already as the edit asks: nothing changed.".to_string()),
        };
        return Ok((output, None));
    }
    let made = built.finish(&mut session.runs, ws, model, &GatePolicy::AGENT_EDIT)?;

    // A refused edit is a refusal like any other, and changes nothing: no
    // diagnostic of it gets an id, and what the session read stays as read.
    if let Some((rule, reason)) = made.refusal {
        let error = [
            Some(reason),
            made.note,
            Some("Nothing changed: fix the operations and call again.".to_string()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
        let mut refusal = ToolError::new(error);
        refusal.refused_edit = Some(Box::new(RefusedEdit {
            rule,
            changes: made.changes,
            omitted: 0,
            omitted_diagnostics: 0,
            diagnostics: made
                .diagnostics
                .into_iter()
                .map(|found| found.diagnostic)
                .collect(),
            simulates: made.simulates,
        }));
        return Err(fitted(refusal, refusal_parts, session.outline_budget));
    }

    // A diagnostic the edit leaves in the model has an id from here on, the
    // one a read gives it.
    let ids = session.evidence.diagnostic_ids(made.report);
    let diagnostics = made
        .diagnostics
        .into_iter()
        .map(|found| EditDiagnostic {
            id: found.at.and_then(|at| ids.get(at).cloned()),
            ..found.diagnostic
        })
        .collect();
    // The host makes the edit: what it changed is as read from here on, so
    // the agent's next edit of it is fresh.
    if let Some(snapshot) = session.last_read.as_mut() {
        made.delta.absorbed_by(snapshot, &model.name, &made.working);
    }
    let output = fitted(
        EditModelOutput {
            revision: ws.revision,
            summary: super::evidence::echo(&input.summary),
            changes: made.changes,
            omitted: 0,
            omitted_diagnostics: 0,
            unchanged: false,
            diagnostics,
            simulates: Some(made.simulates),
            note: made.note,
        },
        output_parts,
        session.outline_budget,
    );
    Ok((output, Some(made.working)))
}

/// The most characters of a change line's detail an answer gives.
const MAX_DETAIL_CHARS: usize = 240;

/// What of an edit's answer is fitted to its budget, a made edit's or a
/// refused one's: its change lines and its diagnostics, each with the count
/// of those left out.
struct Fitted<'a> {
    lines: &'a mut Vec<ChangeLine>,
    omitted: &'a mut usize,
    diagnostics: &'a mut Vec<EditDiagnostic>,
    omitted_diagnostics: &'a mut usize,
}

fn output_parts(output: &mut EditModelOutput) -> Option<Fitted<'_>> {
    Some(Fitted {
        lines: &mut output.changes,
        omitted: &mut output.omitted,
        diagnostics: &mut output.diagnostics,
        omitted_diagnostics: &mut output.omitted_diagnostics,
    })
}

fn refusal_parts(refusal: &mut ToolError) -> Option<Fitted<'_>> {
    refusal.refused_edit.as_deref_mut().map(|refused| Fitted {
        lines: &mut refused.changes,
        omitted: &mut refused.omitted,
        diagnostics: &mut refused.diagnostics,
        omitted_diagnostics: &mut refused.omitted_diagnostics,
    })
}

/// `text` cut to [`MAX_DETAIL_CHARS`].
fn cut_detail(text: &mut String) {
    if text.chars().count() > MAX_DETAIL_CHARS {
        let cut: String = text.chars().take(MAX_DETAIL_CHARS - 3).collect();
        *text = format!("{cut}...");
    }
}

/// `answer` within `budget` bytes of JSON: each change line's detail and
/// each diagnostic's reason cut to [`MAX_DETAIL_CHARS`], then the last
/// diagnostics (the warnings, listed after the errors) and then the last
/// lines left out, counted, keeping the first of each: a refusal's first
/// diagnostic is the one its reason names.
fn fitted<T: Serialize>(
    mut answer: T,
    parts: fn(&mut T) -> Option<Fitted<'_>>,
    budget: usize,
) -> T {
    if let Some(fitted) = parts(&mut answer) {
        fitted
            .lines
            .iter_mut()
            .for_each(|line| cut_detail(&mut line.detail));
        for diagnostic in fitted.diagnostics.iter_mut() {
            if let Some(reason) = &mut diagnostic.reason {
                cut_detail(reason);
            }
        }
    }
    super::fit(&mut answer, budget, |answer| {
        let Some(fitted) = parts(answer) else {
            return false;
        };
        if fitted.diagnostics.len() > 1 {
            fitted.diagnostics.pop();
            *fitted.omitted_diagnostics += 1;
        } else if fitted.lines.len() > 1 {
            fitted.lines.pop();
            *fitted.omitted += 1;
        } else {
            return false;
        }
        true
    });
    answer
}

/// An edit applied to a copy of the project as a workspace has it, placed
/// and gated.
struct Made {
    /// The project as the edit leaves it.
    working: datamodel::Project,
    delta: Delta,
    changes: Vec<ChangeLine>,
    diagnostics: Vec<Found>,
    /// What identifies each diagnostic of the model the edit is of, as the
    /// edit leaves it, in the order the engine reports them.
    report: Vec<DiagnosticIdentity>,
    simulates: bool,
    /// The rule that refuses the edit and why, when the gate refuses it.
    refusal: Option<(GateRule, String)>,
    note: Option<String>,
}

/// An edit's operations applied to a copy of the project, before it is
/// placed and gated.
struct Built<'a> {
    builder: Builder<'a>,
    delta: Delta,
    structural: bool,
    /// The place (from 1) of each `name_loop` operation, in the order of
    /// `Builder::pins`.
    pin_operations: Vec<usize>,
}

impl<'a> Built<'a> {
    /// Apply `operations` in order to a copy of `project`, whose model
    /// `model` they edit.
    fn new(
        project: &datamodel::Project,
        model: &datamodel::Model,
        evidence: &'a super::evidence::Evidence,
        operations: &[EditOperation],
    ) -> Result<Built<'a>, ToolError> {
        let mut builder = Builder::new(project, model, evidence);
        let mut pin_operations = Vec::new();
        for (i, op) in operations.iter().enumerate() {
            let name = op.name();
            builder
                .operate(op.clone())
                .map_err(|err| err.prefixed(&format!("operation {} ({name}): ", i + 1)))?;
            if matches!(op, EditOperation::NameLoop { .. }) {
                pin_operations.push(i + 1);
            }
        }
        // The delta is taken before provenance is marked: an edit that
        // changes no record is no edit, which draws nothing
        // (`Built::finish`) and hands the host nothing (`edit_model` decides
        // `edited` by the delta), whatever provenance marks on the copy.
        let delta = Delta::between(project, &builder.working);
        builder.mark_provenance(model, tracks_provenance(project))?;
        Ok(Built {
            builder,
            delta,
            structural: operations.iter().any(EditOperation::structural),
            pin_operations,
        })
    }

    /// Gate the edit, on the project as `ws` has it, under `policy`, and
    /// place what it adds on the diagram when the gate passes it: a layout
    /// is the most an edit costs on a large diagram, and a refused edit is
    /// never drawn.
    fn finish(
        self,
        runs: &mut RunStore,
        ws: &mut Workspace<'_>,
        model: &datamodel::Model,
        policy: &GatePolicy,
    ) -> Result<Made, ToolError> {
        let Built {
            mut builder,
            delta,
            structural,
            pin_operations,
        } = self;
        if !builder.pins.is_empty() {
            ws.yield_point()?;
            let staging = Staging::new(ws.db, ws.project, &builder.working);
            for ((name, variables), place) in builder.pins.iter().zip(&pin_operations) {
                if let Some(why) = no_loop(
                    staging.db,
                    staging.source_project,
                    &model.name,
                    name,
                    variables,
                ) {
                    return Err(ToolError::new(format!(
                        "operation {place} (name_loop): {why}; analyze_loops lists the model's \
                         loops, each with an id to name it by"
                    )));
                }
            }
        }
        let mut notes = Vec::new();
        let base = match runs.current(ws, model) {
            Ok(base) => Some(base),
            Err(err) if err.is_interrupted() => return Err(err),
            Err(_) => None,
        };
        let gate = gate(
            ws,
            &Edit {
                model,
                working: &builder.working,
                renames: &builder.renames,
                authored: &builder.authored(),
                gone: &delta.gone(),
            },
            base.as_deref().map(|run| &run.results),
        )?;
        let refusal = gate.refusal(policy);
        if refusal.is_none()
            && structural
            && !delta.is_empty()
            && let Err(reason) = builder.place(model)
        {
            notes.push(format!(
                "The diagram will not show this edit until it is laid out: {reason}."
            ));
        }
        if let Some(remaining) = gate
            .errors
            .iter()
            .find(|(_, why)| *why == Listed::Changed)
            .map(|(error, _)| named(&error.diagnostic))
        {
            notes.push(format!("An error remains ({remaining})."));
        }
        let changes = change_lines(&delta, &builder.working, &model.name, &builder.renames);
        let mut diagnostics = gate.diagnostics();
        diagnostics.extend(
            unwired_inputs(ws.project, &builder.working, &model.name)
                .into_iter()
                .map(Found::by_the_gate),
        );
        Ok(Made {
            working: builder.working,
            delta,
            changes,
            diagnostics,
            report: gate.report,
            simulates: gate.simulates,
            refusal,
            note: notes.into_iter().reduce(|a, b| format!("{a} {b}")),
        })
    }
}

/// Why `variables`, named as the loop `name` in the model `model_name` of
/// the staged project, are no feedback loop the engine scores; `None` when
/// they are one. The engine reads a loop's variables as a set and orders
/// them by the causal graph (`db::model_pinned_loops`, the one judge of a
/// named loop), so the order they are given in is no part of it. The reason
/// names a missing link when one is plain: a variable none of the others
/// reads, or one that reads none of them.
fn no_loop(
    db: &crate::db::SimlinDb,
    project: crate::db::SourceProject,
    model_name: &str,
    name: &str,
    variables: &[String],
) -> Option<String> {
    let canonical = crate::canonicalize(model_name);
    let model = project.models(db).get(canonical.as_ref()).copied()?;
    let pinned = crate::db::model_pinned_loops(db, model, project);
    if pinned.loops.iter().any(|pin| pin.name == name)
        && !pinned.invalid.iter().any(|(invalid, _)| invalid == name)
    {
        return None;
    }
    let listed = variables.join(", ");
    let members: BTreeSet<String> = variables
        .iter()
        .map(|v| crate::canonicalize(v).into_owned())
        .collect();
    if members.len() < 2 {
        return Some(format!(
            "a loop goes through at least two variables, and this names {}",
            members.len()
        ));
    }
    let graph = crate::db::causal_graph_with_modules(db, model, project);
    let reads = |reader: &str, read: &str| {
        graph
            .edges
            .get(&crate::common::Ident::<crate::common::Canonical>::new(read))
            .is_some_and(|to| to.iter().any(|t| t.as_str() == reader))
    };
    // In the order given, so the link named is the first a reader of the
    // refusal would look for.
    for (spelled, member) in variables
        .iter()
        .map(|v| (v, crate::canonicalize(v).into_owned()))
    {
        if !members
            .iter()
            .any(|other| *other != member && reads(other, &member))
        {
            return Some(format!(
                "{listed} are no feedback loop: none of the others reads '{spelled}'"
            ));
        }
        if !members
            .iter()
            .any(|other| *other != member && reads(&member, other))
        {
            return Some(format!(
                "{listed} are no feedback loop: '{spelled}' reads none of the others"
            ));
        }
    }
    let reason = pinned
        .invalid
        .iter()
        .find(|(invalid, _)| invalid == name)
        .map(|(_, reason)| reason.clone());
    Some(match reason {
        Some(reason) => format!("{listed} are no feedback loop the engine scores: {reason}"),
        None => format!("{listed} are no one feedback loop through them all"),
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

/// Whether the project records who made its variables: it carries AI
/// information, or a variable's provenance.
fn tracks_provenance(project: &datamodel::Project) -> bool {
    project.ai_information.is_some()
        || project
            .models
            .iter()
            .any(|m| m.variables.iter().any(|v| v.get_ai_state().is_some()))
}

/// A variable's provenance once an AI has edited it, from what it was: made
/// by AI and never touched by a person stays so (C); made by a person, or of
/// unknown making, is also edited by AI (D, or H once a person has edited
/// it too); made by AI then edited by a person stays so (G), since that
/// state already allows a later AI edit.
///
/// This is in-app provenance: the letters are `datamodel::AiState`'s, which
/// follow the meanings its variants document, but the table is UNVERIFIED
/// against ISEE. ISEE's public documentation of its AI information (the
/// Stella help's "AI Usage Report" page) states no letters and no
/// transitions; it says the information is reliable only for a file no
/// other application edited, and the XMILE writer keeps the letters without
/// the signed `<ai_information>` block they belong to. What Stella makes of
/// a letter written here is unknown.
fn after_ai_edit(state: Option<AiState>) -> AiState {
    match state {
        None | Some(AiState::A) | Some(AiState::B) | Some(AiState::D) => AiState::D,
        Some(AiState::C) => AiState::C,
        Some(AiState::E) | Some(AiState::F) | Some(AiState::H) => AiState::H,
        Some(AiState::G) => AiState::G,
    }
}

/// Which of a variable's equations an edit wrote.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
enum Authored {
    /// Its equation whole: every element's, or its lookup.
    Whole,
    /// The equations of these elements alone, by their canonical names.
    Elements(BTreeSet<CanonicalElementName>),
}

/// What an edit's operations did to a variable they name.
#[derive(Clone, Default)]
struct Touch {
    /// The edit added the variable.
    created: bool,
    /// The edit wrote its equation, or some elements' equations.
    authored: Option<Authored>,
}

/// An edit's operations, applied in order to a copy of the project.
struct Builder<'a> {
    working: datamodel::Project,
    model_name: String,
    evidence: &'a super::evidence::Evidence,
    /// The model operations so far, which the placement reads to know what
    /// the edit names.
    ops: Vec<ModelOperation>,
    /// Renames, `(from, to)` as written.
    renames: Vec<(String, String)>,
    /// The variables the operations name, by their canonical names as the
    /// edit leaves them: what provenance marks, and whose equations the gate
    /// holds the edit to.
    touched: BTreeMap<String, Touch>,
    /// The loops the edit names, as `(name, variables)`, judged once the
    /// edit is whole ([`Built::finish`]): a later operation can make or
    /// break the loop.
    pins: Vec<(String, Vec<String>)>,
}

impl<'a> Builder<'a> {
    fn new(
        project: &datamodel::Project,
        model: &datamodel::Model,
        evidence: &'a super::evidence::Evidence,
    ) -> Builder<'a> {
        Builder {
            working: project.clone(),
            model_name: model.name.clone(),
            evidence,
            ops: Vec::new(),
            renames: Vec::new(),
            touched: BTreeMap::new(),
            pins: Vec::new(),
        }
    }

    /// The model as the edit has it so far.
    fn model(&self) -> Result<&datamodel::Model, ToolError> {
        self.working.get_model(&self.model_name).ok_or_else(|| {
            ToolError::new(format!(
                "the project has no model named '{}'",
                self.model_name
            ))
        })
    }

    /// The variable `name` names in the model as the edit has it so far.
    fn existing(&self, name: &str) -> Result<Variable, ToolError> {
        let model = self.model()?;
        names::resolve(model, name).cloned().map_err(|suggestions| {
            // A variable's element written into its name: the element is a
            // field of its own.
            let element = names::split_subscript(name)
                .filter(|(base, _)| model.get_variable(base).is_some())
                .map(|(base, subscripts)| {
                    format!(
                        "; to write one element of '{base}', give the variable as '{base}' and \
                         the element as `element`: \"{}\"",
                        subscripts.join(", ")
                    )
                })
                .unwrap_or_default();
            ToolError::new(format!(
                "the model has no variable '{}'{element}",
                super::evidence::echo(name)
            ))
            .with_suggestions(suggestions)
        })
    }

    /// What the edit did to the variable named `ident`, to note more of.
    fn touch(&mut self, ident: &str) -> &mut Touch {
        self.touched
            .entry(crate::canonicalize(ident).into_owned())
            .or_default()
    }

    /// Note that the edit wrote the equation of the variable named `ident`:
    /// of the element `element`, or whole.
    fn authors(&mut self, ident: &str, element: Option<CanonicalElementName>) {
        let touch = self.touch(ident);
        touch.authored = Some(match (touch.authored.take(), element) {
            (Some(Authored::Elements(mut elements)), Some(element)) => {
                elements.insert(element);
                Authored::Elements(elements)
            }
            (None, Some(element)) => Authored::Elements(BTreeSet::from([element])),
            (Some(Authored::Whole), _) | (_, None) => Authored::Whole,
        });
    }

    /// The equations the edit wrote, by the canonical names of their
    /// variables as the edit leaves them.
    fn authored(&self) -> BTreeMap<String, Authored> {
        self.touched
            .iter()
            .filter_map(|(name, touch)| Some((name.clone(), touch.authored.clone()?)))
            .collect()
    }

    /// Record that an AI made or edited the variables the edit's operations
    /// name (`after_ai_edit`), when the project records who made its
    /// variables: an upsert of each such variable as the edit leaves it,
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
        let mut ops = Vec::new();
        for (name, touch) in &self.touched {
            let Some(mut var) = self
                .model()?
                .variables
                .iter()
                .find(|v| crate::canonicalize(v.get_ident()).as_ref() == name.as_str())
                .cloned()
            else {
                continue;
            };
            let state = if touch.created {
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

    /// `name` as a variable is named, if it can name one: trimmed, without
    /// the quotes an equation would write around it, not empty, and made of
    /// words -- letters, digits, spaces and underscores -- that an equation
    /// reads back as one variable when it spells them (`ast::print_ident`,
    /// which quotes a name that starts with a digit). A word the equation
    /// language reserves is refused, and so is anything with an operator or
    /// punctuation in it (`a+b`, `module.variable`): a quoted name can hold
    /// them, but one an agent gives is far likelier an expression or a
    /// module's variable than a name.
    fn named(&self, name: &str) -> Result<String, ToolError> {
        let name = name.trim();
        let name = name
            .strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .map_or(name, str::trim);
        let canonical = crate::canonicalize(name).into_owned();
        let words = canonical.chars().all(|c| c.is_alphanumeric() || c == '_');
        let reads_as_itself = matches!(
            crate::ast::Expr0::new(
                &crate::ast::print_ident(&canonical),
                crate::lexer::LexerType::Equation
            ),
            Ok(Some(crate::ast::Expr0::Var(ref raw, _)))
                if raw.canonicalize().as_str() == canonical
        );
        if name.is_empty()
            || !words
            || !reads_as_itself
            || crate::lexer::is_reserved_word(&canonical)
            // A number in quotes reads as a variable, and is no name.
            || canonical.parse::<f64>().is_ok()
        {
            return Err(ToolError::new(format!(
                "'{}' is not a name a variable can have: a name is words of letters, digits \
                 and underscores (spaces allowed), not a number, not a word the equation \
                 language reserves, and not the clock's own (time, dt); a name like 'x_time' \
                 is one",
                super::evidence::echo(name)
            )));
        }
        Ok(name.to_string())
    }

    /// `name`, if it can name a new variable: a name a variable can have
    /// ([`Builder::named`]) that is no variable's already.
    fn free(&self, name: &str) -> Result<String, ToolError> {
        let name = self.named(name)?;
        if self.model()?.get_variable(&name).is_some() {
            return Err(ToolError::new(format!(
                "the model already has a variable '{name}'; change it with set_equation, or \
                 choose another name"
            )));
        }
        Ok(name)
    }

    /// Apply `ops` to the working copy, through the production patch path.
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

    /// The stock `name` names, which the edit rewires.
    fn stock(&mut self, name: &str) -> Result<datamodel::Stock, ToolError> {
        match self.existing(name)? {
            Variable::Stock(stock) => {
                self.touch(&stock.ident);
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

    /// Apply `op`, which adds the variable named `ident`.
    fn add(&mut self, ident: &str, op: ModelOperation) -> Result<(), ToolError> {
        self.touch(ident).created = true;
        self.authors(ident, None);
        self.apply(vec![op])
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
                    ident: name.clone(),
                    equation: Equation::Scalar(initial),
                    documentation: notes.unwrap_or_default(),
                    units: units.filter(|u| !u.trim().is_empty()),
                    inflows: self.flows(&inflows)?,
                    outflows: self.flows(&outflows)?,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                };
                self.add(&name, ModelOperation::UpsertStock(stock))
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
                self.add(&name, ModelOperation::UpsertFlow(flow))?;
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
                    ident: name.clone(),
                    equation: Equation::Scalar(equation),
                    documentation: notes.unwrap_or_default(),
                    units: units.filter(|u| !u.trim().is_empty()),
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                };
                self.add(&name, ModelOperation::UpsertAux(aux))
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
                let (new, element) = self.equation(&var, &equation, element.as_deref())?;
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
                self.authors(var.get_ident(), element);
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
                    Variable::Stock(_) | Variable::Module(_) => {
                        return Err(ToolError::new(format!(
                            "'{}' cannot be a lookup: only an auxiliary or a flow can",
                            var.get_ident()
                        )));
                    }
                }
                self.authors(var.get_ident(), None);
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
                let ident = var.get_ident().to_string();
                let to = self.named(&to)?;
                let (old, new) = (
                    crate::canonicalize(&ident).into_owned(),
                    crate::canonicalize(&to).into_owned(),
                );
                // A name that differs only in how it is written is the same
                // name: the patch restamps it and rewrites nothing.
                if old == new {
                    if to == ident {
                        return Err(ToolError::new(format!("'{to}' is its name already")));
                    }
                } else {
                    self.free(&to)?;
                }
                if let Some(touch) = self.touched.remove(&old) {
                    self.touched.insert(new, touch);
                }
                self.touch(&to);
                self.renames.push((ident.clone(), to.clone()));
                self.apply(vec![ModelOperation::RenameVariable { from: ident, to }])
            }
            EditOperation::Delete { variable } => {
                let var = self.existing(&variable)?;
                let ident = var.get_ident().to_string();
                if let Variable::Flow(_) = var {
                    for stock in self.stocks_with(&ident)? {
                        self.touch(&stock.ident);
                    }
                }
                self.touched.remove(crate::canonicalize(&ident).as_ref());
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
                self.pins.push((name.trim().to_string(), variables.clone()));
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

    /// Upsert `var`, which the edit names.
    fn upsert(&mut self, var: Variable) -> Result<(), ToolError> {
        self.touch(var.get_ident());
        let op = match var {
            Variable::Stock(stock) => ModelOperation::UpsertStock(stock),
            Variable::Flow(flow) => ModelOperation::UpsertFlow(flow),
            Variable::Aux(aux) => ModelOperation::UpsertAux(aux),
            Variable::Module(module) => ModelOperation::UpsertModule(module),
        };
        self.apply(vec![op])
    }

    /// The stocks that list the flow `ident` among their flows.
    fn stocks_with(&self, ident: &str) -> Result<Vec<datamodel::Stock>, ToolError> {
        let canonical = crate::canonicalize(ident);
        Ok(self
            .model()?
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
            .collect())
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
        let mut stocks: Vec<datamodel::Stock> = self.stocks_with(ident)?;
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
            self.touch(&stock.ident);
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

    /// `var`'s equation with `text` in place: of every element, or of the
    /// one `element` names, which is returned as the model's arms name it.
    ///
    /// An element is named as the read tools name one
    /// (`names::resolve_element`), and an arm is matched as the engine
    /// matches one (`CanonicalElementName::from_subscript`).
    fn equation(
        &self,
        var: &Variable,
        text: &str,
        element: Option<&str>,
    ) -> Result<(Equation, Option<CanonicalElementName>), ToolError> {
        let Some(old) = var.get_equation() else {
            return Err(ToolError::new(format!(
                "'{}' has no equation",
                var.get_ident()
            )));
        };
        let text = text.to_string();
        let Some(element) = element else {
            return Ok((
                match old {
                    Equation::Scalar(_) => Equation::Scalar(text),
                    Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => {
                        Equation::ApplyToAll(dims.clone(), text)
                    }
                },
                None,
            ));
        };
        let dims = dimensions(old);
        let subscripts: Vec<&str> = element.split(',').map(str::trim).collect();
        let name = names::resolve_element(&self.working, &dims, &subscripts).map_err(|why| {
            ToolError::new(format!(
                "'{}' has no element '{}': {why}",
                var.get_ident(),
                super::evidence::echo(element)
            ))
            .with_suggestions(
                // A scalar's one "element" is no name to suggest.
                element_names(&self.working, &dims)
                    .into_iter()
                    .filter(|name| !name.is_empty())
                    .take(12)
                    .collect(),
            )
        })?;
        let key = CanonicalElementName::from_subscript(&name);
        // An apply-to-all equation becomes an arm per element; an arrayed
        // one keeps its arms, its default and whether the default applies to
        // the elements with no arm.
        let (mut elements, default, applies_default) = match old {
            Equation::Scalar(_) => {
                return Err(ToolError::new(format!(
                    "'{}' is not arrayed, so it has no element '{}'",
                    var.get_ident(),
                    super::evidence::echo(element)
                )));
            }
            Equation::ApplyToAll(dims, all) => (
                element_names(&self.working, dims)
                    .into_iter()
                    .map(|e| (e, all.clone(), None, None))
                    .collect::<Vec<_>>(),
                None,
                false,
            ),
            Equation::Arrayed(_, elements, default, applies_default) => {
                (elements.clone(), default.clone(), *applies_default)
            }
        };
        match elements
            .iter_mut()
            .find(|(e, ..)| CanonicalElementName::from_subscript(e) == key)
        {
            Some(entry) => entry.1 = text,
            None => elements.push((name, text, None, None)),
        }
        Ok((
            Equation::Arrayed(dims, elements, default, applies_default),
            Some(key),
        ))
    }

    /// The variables of the loop this session calls `id`, as the model
    /// names them.
    fn loop_variables(&self, id: &str) -> Result<Vec<String>, ToolError> {
        let key = self.evidence.loop_key(id.trim()).ok_or_else(|| {
            ToolError::new(format!(
                "no loop has the id '{id}' in this session: loop ids come from analyze_loops"
            ))
        })?;
        let model = self.model()?;
        let mut seen = BTreeSet::new();
        key.iter()
            .map(|node| crate::ltm::strip_subscript(node))
            .filter(|name| seen.insert(name.to_string()))
            .map(|name| {
                model
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
        if self.model()?.sim_specs.is_some() {
            return Err(ToolError::new(
                "the model has sim specs of its own, which an edit cannot change; to try other \
                 specs, run_experiment takes `specs` and changes nothing in the model",
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
        if let Some(reason) = super::runs::no_run(&crate::results::Specs::from(&specs)) {
            return Err(ToolError::new(reason));
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
                project_ops: vec![ProjectOperation::SetSimSpecs(specs)],
                models: vec![],
            },
        )
        .map_err(|err| ToolError::new(format!("it cannot apply: {err}")))
    }

    /// Place what the edit adds on the model's diagram: an incremental
    /// layout of the model's first view, around the diagram as it is, or a
    /// whole layout when it has none.
    ///
    /// An incremental layout decides the view's elements and nothing else
    /// (`layout::incremental`): its box, which is the viewport an editor
    /// keeps there, its name, zoom, polarity lettering, font and what an MDL
    /// writer needs to write it back stay the view's own.
    fn place(&mut self, before: &datamodel::Model) -> Result<(), String> {
        let drawn = before.views.first().map(|view| match view {
            datamodel::View::StockFlow(stock_flow) => stock_flow,
        });
        let patch = ModelPatch {
            name: self.model_name.clone(),
            ops: self.ops.clone(),
        };
        let laid_out = match drawn {
            Some(drawn) => crate::layout::incremental_layout(
                drawn,
                &self.working,
                &self.model_name,
                &patch,
                None,
            )?,
            None => crate::layout::generate_best_layout(&self.working, &self.model_name, None)?,
        };
        self.apply(vec![ModelOperation::UpsertView {
            index: 0,
            view: datamodel::View::StockFlow(laid_out),
        }])
        .map_err(|err| err.error)
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

/// A lookup through `points`, x never decreasing: neighbouring points that
/// share an x are a vertical step, which the engine reads by each lookup
/// mode's rule (the engine CLAUDE.md, graphical functions).
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
    if points.windows(2).any(|w| w[1][0] < w[0][0]) {
        return Err(ToolError::new(format!(
            "the x values of '{name}''s lookup must not decrease: list the points in x order"
        )));
    }
    let scale = |values: &[f64]| datamodel::GraphicalFunctionScale {
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
        x_scale: scale(&xs),
        y_scale: scale(&ys),
        x_points: Some(xs),
        y_points: ys,
    })
}

/// The rules a gate holds an edit to, each asked for by name: what a host's
/// entry point passes says what it refuses, and nothing is refused by a rule
/// it did not ask for. A run that would cost more than a run may is refused
/// under every policy: that is the session's limit, not the edit's.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct GatePolicy {
    /// Refuse an error the project did not have, in any of its models, and
    /// an error in an equation the edit writes.
    pub errors: bool,
    /// Refuse the first unit warning of a model that had none.
    pub unit_warnings: bool,
    /// Refuse an edit after which a model that simulated no longer does:
    /// the model the edit is of, and the project's root model.
    pub simulation: bool,
    /// Refuse a value that is not a number where the run of the project as
    /// it is has a number throughout.
    pub values: bool,
}

impl GatePolicy {
    /// What an agent's edit is held to: every rule.
    pub(crate) const AGENT_EDIT: GatePolicy = GatePolicy {
        errors: true,
        unit_warnings: true,
        simulation: true,
        values: true,
    };

    /// Whether the policy asks for `rule`. A run's cost is the session's
    /// limit, not the edit's, so every policy asks for it.
    pub(crate) fn asks(&self, rule: GateRule) -> bool {
        match rule {
            GateRule::Errors => self.errors,
            GateRule::RunCost => true,
            GateRule::Simulation => self.simulation,
            GateRule::Values => self.values,
            GateRule::UnitWarnings => self.unit_warnings,
        }
    }
}

/// Why the gate lists an error.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Listed {
    /// Its variable has more errors of its code than it had: an error the
    /// project did not have.
    New,
    /// It is in an equation the edit writes.
    Authored,
    /// It is a different problem than its variable had, of a code the
    /// variable had an error of already: the engine reports one error of an
    /// equation at a time, so it may be the next of the variable's own.
    Changed,
}

/// The run of one model the gate judges.
struct JudgedRun {
    /// The model, when it is not the one the edit is of: the project's root.
    root: Option<String>,
    /// Whether it simulated before the edit.
    simulated: bool,
    /// Why it does not simulate after the edit, when it does not.
    error: Option<String>,
}

/// A diagnostic the gate lists.
#[derive(Clone, PartialEq)]
struct Found {
    diagnostic: EditDiagnostic,
    /// Its place in the report of the model the edit is of (`Gate::report`),
    /// when it is one of that report's: what its id is read from.
    at: Option<usize>,
}

impl Found {
    /// A finding of the gate's own, which no report of the engine's has.
    fn by_the_gate(diagnostic: EditDiagnostic) -> Found {
        Found {
            diagnostic,
            at: None,
        }
    }
}

/// What the gate found of an edit.
struct Gate {
    /// The errors the edit adds or leaves, each with why it is listed.
    errors: Vec<(Found, Listed)>,
    /// Warnings the project did not have, of every category.
    warnings: Vec<Found>,
    /// What identifies each diagnostic of the model the edit is of (the
    /// project's own among them, as a read reports them), as the edit leaves
    /// it, in the order the engine reports them.
    report: Vec<DiagnosticIdentity>,
    /// The unit warning that is the first of a model that had none.
    first_unit_warning: Option<EditDiagnostic>,
    /// The variables (or elements) that are not a number somewhere in the
    /// edit's run and a number throughout the run before it, with when they
    /// first are not, earliest first.
    non_finite: Vec<NonFinite>,
    /// The series not a number somewhere in a run of a model that did not
    /// simulate before the edit: no rule refuses them (a repair a step at a
    /// time brings a model back to life), and they are listed as warnings.
    undefined: Vec<NonFinite>,
    runs: Vec<JudgedRun>,
    /// Whether the model the edit is of simulates after it.
    simulates: bool,
    /// Why the edited model's run would cost more than a run may: specs that
    /// ask for more than the model's own and more than the limit.
    too_costly: Option<String>,
    /// Each variable the edit removes that a text of the project still names,
    /// with the variables whose texts name it.
    dangling: Vec<Dangling>,
}

/// A variable an edit removes (deletes, or renames away) that an expression
/// text of the project still names: what no diagnostic may report, when the
/// text's equation failed already.
struct Dangling {
    /// The removed variable, as the model spelled it, with its model when it
    /// is not the one the edit is of.
    gone: String,
    /// The variables that still name it, each `(model when not the edit's,
    /// variable)`.
    readers: Vec<(Option<String>, String)>,
}

/// A diagnostic as a refusal names it: its variable, with its model when it
/// is another's, and its reason.
fn named(d: &EditDiagnostic) -> String {
    let what = d.reason.clone().unwrap_or_else(|| d.code.clone());
    let place = match (&d.variable, &d.model) {
        (Some(variable), Some(model)) => Some(format!("{variable} in {model}")),
        (Some(variable), None) => Some(variable.clone()),
        (None, Some(model)) => Some(format!("model {model}")),
        (None, None) => None,
    };
    match place {
        Some(place) => format!("{place}: {what}"),
        None => what,
    }
}

impl Gate {
    /// Why the gate refuses the edit under `policy`, if it does.
    ///
    /// By its rules the gate tolerates: an error the project had already
    /// (the same problem by the evidence module's identity), so a broken
    /// model can be repaired a step at a time; an error that is a different
    /// problem of a code its variable had an error of already, on a variable
    /// the edit does not write, which it lists (`Listed::Changed`); values
    /// not a number in a model that did not simulate before, which has no
    /// run to compare with; and warnings, which it lists, but for a
    /// unit-clean model's first unit warning.
    fn refusal(&self, policy: &GatePolicy) -> Option<(GateRule, String)> {
        GateRule::ALL
            .into_iter()
            .filter(|rule| policy.asks(*rule))
            .find_map(|rule| self.refused_by(rule).map(|reason| (rule, reason)))
    }

    /// Why `rule` refuses the edit, if it does.
    fn refused_by(&self, rule: GateRule) -> Option<String> {
        match rule {
            GateRule::Errors => self
                .errors
                .iter()
                .find_map(|(error, why)| match why {
                    Listed::New => {
                        Some(format!("It adds an error ({}).", named(&error.diagnostic)))
                    }
                    Listed::Authored => Some(format!(
                        "It leaves an error in an equation it writes ({}).",
                        named(&error.diagnostic)
                    )),
                    Listed::Changed => None,
                })
                .or_else(|| {
                    self.dangling.first().map(|dangling| {
                        let readers: Vec<String> = dangling
                            .readers
                            .iter()
                            .take(super::MAX_NAMED)
                            .map(|(model, reader)| match model {
                                Some(model) => format!("{reader} in {model}"),
                                None => reader.clone(),
                            })
                            .collect();
                        let more = dangling.readers.len().saturating_sub(super::MAX_NAMED);
                        let one = dangling.readers.len() == 1;
                        format!(
                            "It removes {}, which {}{} still name{}: edit or delete {} first.",
                            dangling.gone,
                            readers.join(", "),
                            if more > 0 {
                                format!(" and {more} more")
                            } else {
                                String::new()
                            },
                            if one { "s" } else { "" },
                            if one { "it" } else { "them" }
                        )
                    })
                }),
            GateRule::RunCost => self
                .too_costly
                .as_ref()
                .map(|reason| format!("It asks more of a run than a run may: {reason}.")),
            GateRule::Simulation => self
                .runs
                .iter()
                .find_map(|run| {
                    run.error
                        .as_ref()
                        .filter(|_| run.simulated)
                        .map(|e| (run, e))
                })
                .map(|(run, error)| match &run.root {
                    None => format!("The model would not simulate: {error}."),
                    Some(root) => {
                        format!("The project's model '{root}' would not simulate: {error}.")
                    }
                }),
            GateRule::Values => self.non_finite.first().map(|found| {
                let place = match &found.model {
                    Some(model) => format!("{} in {model}", found.variable),
                    None => found.variable.clone(),
                };
                format!("It makes {place} not a number from time {}.", found.time)
            }),
            GateRule::UnitWarnings => self.first_unit_warning.as_ref().map(|first| {
                format!(
                    "It gives a model without unit warnings its first ({}): fix the units or the \
                     equation.",
                    named(first)
                )
            }),
        }
    }

    /// Everything the gate lists: the errors, the names the edit leaves
    /// dangling, the values that are not a number, then the warnings.
    fn diagnostics(&self) -> Vec<Found> {
        self.errors
            .iter()
            .map(|(error, _)| error.clone())
            .chain(self.dangling.iter().flat_map(|dangling| {
                dangling.readers.iter().map(|(model, reader)| {
                    Found::by_the_gate(EditDiagnostic {
                        id: None,
                        severity: Severity::Error,
                        category: DiagnosticCategoryName::Equation,
                        code: "names_a_removed_variable".to_string(),
                        model: model.clone(),
                        variable: Some(reader.clone()),
                        reason: Some(format!("names {}, which the edit removes", dangling.gone)),
                    })
                })
            }))
            .chain(
                self.non_finite
                    .iter()
                    .map(|found| (found, Severity::Error))
                    .chain(
                        self.undefined
                            .iter()
                            .map(|found| (found, Severity::Warning)),
                    )
                    .map(|(found, severity)| {
                        Found::by_the_gate(EditDiagnostic {
                            id: None,
                            severity,
                            category: DiagnosticCategoryName::Value,
                            code: "non_finite".to_string(),
                            model: found.model.clone(),
                            variable: Some(found.variable.clone()),
                            reason: Some(format!("not a number from time {}", found.time)),
                        })
                    }),
            )
            .chain(self.warnings.iter().cloned())
            .collect()
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

/// An edit as the gate judges it.
struct Edit<'a> {
    /// The model the edit is of, as the project has it.
    model: &'a datamodel::Model,
    /// The project as the edit leaves it.
    working: &'a datamodel::Project,
    /// The edit's renames, `(from, to)` as written.
    renames: &'a [(String, String)],
    /// The equations the edit wrote.
    authored: &'a BTreeMap<String, Authored>,
    /// The variables no longer in the project after the edit, deleted or
    /// renamed away, each `(model, canonical name, as the model spelled it)`.
    gone: &'a [(String, String, String)],
}

/// A diagnostic of a project with the model it is in (as the project names
/// it; empty for one of the project as a whole).
struct Finding {
    model: String,
    described: Described,
}

/// Every diagnostic of `project`, which `db` holds as `source_project`, each
/// described against its own model (the model named `fallback` for one that
/// belongs to none of the project's).
fn findings(
    db: &crate::db::SimlinDb,
    source_project: SourceProject,
    project: &datamodel::Project,
    fallback: &datamodel::Model,
) -> Vec<Finding> {
    collect_all_diagnostics(db, source_project, LtmOverlay::Off)
        .iter()
        .map(|d| {
            let canonical = crate::canonicalize(&d.model);
            let model = project
                .models
                .iter()
                .find(|m| !d.model.is_empty() && crate::canonicalize(&m.name) == canonical);
            Finding {
                model: model.map_or_else(|| d.model.clone(), |m| m.name.clone()),
                described: describe(d, project, model.unwrap_or(fallback)),
            }
        })
        .collect()
}

/// Why a model has no run.
enum NoRun {
    /// Its run would cost more than a run may.
    Cost(String),
    /// It does not simulate.
    Fails(String),
}

/// The run of the model named `model_name` of `project`, which `db` holds
/// as `source_project`.
fn run_of(
    db: &mut crate::db::SimlinDb,
    source_project: SourceProject,
    project: &datamodel::Project,
    model_name: &str,
    own: &crate::results::Specs,
) -> Result<crate::Results, NoRun> {
    match build_vm(
        db,
        source_project,
        project,
        model_name,
        LtmOverlay::Off,
        own,
    ) {
        Ok(mut vm) => vm
            .run_to_end()
            .map(|()| vm.into_results())
            .map_err(|err| NoRun::Fails(super::evidence::explain_helpers(err.reason()))),
        Err(Unbuilt::Compile(err)) => {
            Err(NoRun::Fails(super::evidence::explain_helpers(err.reason())))
        }
        Err(Unbuilt::Cost(reason)) => Err(NoRun::Cost(reason)),
    }
}

/// `working` with every arm the edit did not write, of each arrayed variable
/// in `hidden` it wrote some arms of, set to `0`; `None` when there is no
/// such arm.
///
/// The engine reports one error of an arrayed equation, the first failing
/// arm's (`ast::lower_arrayed_arms`), so an arm that failed before the edit
/// hides one the edit breaks. With the other arms out of the way, an error
/// the variable still has is in an arm the edit wrote.
fn isolated(
    working: &datamodel::Project,
    model_name: &str,
    authored: &BTreeMap<String, Authored>,
    hidden: &BTreeSet<String>,
) -> Option<datamodel::Project> {
    let mut project = working.clone();
    let model = project.get_model_mut(model_name)?;
    let mut any = false;
    for (name, authored) in authored {
        let Authored::Elements(written) = authored else {
            continue;
        };
        if !hidden.contains(name) {
            continue;
        }
        let Some(var) = model.get_variable_mut(name) else {
            continue;
        };
        let equation = match var {
            Variable::Stock(stock) => &mut stock.equation,
            Variable::Flow(flow) => &mut flow.equation,
            Variable::Aux(aux) => &mut aux.equation,
            Variable::Module(_) => continue,
        };
        let Equation::Arrayed(_, elements, default, _) = equation else {
            continue;
        };
        for (element, text, initial, table) in elements.iter_mut() {
            if !written.contains(&CanonicalElementName::from_subscript(element)) {
                *text = "0".to_string();
                *initial = None;
                *table = None;
                any = true;
            }
        }
        if let Some(default) = default {
            *default = "0".to_string();
            any = true;
        }
    }
    any.then_some(project)
}

/// `project` with the equation of each variable of the model `model_name`
/// named in `aside` (canonically) set to `0`: its every equation text,
/// elements, initials and default alike (`Variable::map_expression_texts`,
/// its options left). `None` when no such variable has a text to set.
///
/// The engine reports one error of an equation and one cycle of a model, so a
/// variable that has an error hides others behind it. With the variables that
/// were broken before an edit set aside, the errors the edit makes, and the
/// ones the broken ones hid, show.
fn set_aside(
    project: &datamodel::Project,
    model_name: &str,
    aside: &BTreeSet<String>,
) -> Option<datamodel::Project> {
    let mut project = project.clone();
    let model = project.get_model_mut(model_name)?;
    let mut any = false;
    for name in aside {
        let Some(index) = model.variable_index(name) else {
            continue;
        };
        let zeroed = model.variables[index].map_expression_texts(|role, text| match role {
            datamodel::ExpressionRole::Equation | datamodel::ExpressionRole::Initial => {
                (text.trim() != "0").then(|| "0".to_string())
            }
            datamodel::ExpressionRole::Option(_) => None,
        });
        if let Some(zeroed) = zeroed {
            model.variables.replace(index, zeroed);
            any = true;
        }
    }
    any.then_some(project)
}

/// Judge `edit` against the project as `ws` has it: stage the edited project
/// on the host's database, compare its diagnostics with the project's own,
/// every model's, by the evidence module's identity of a problem (the
/// project's mapped through the edit's renames), see whether the model the
/// edit is of and the project's root model simulate, within what a run may
/// cost, and compare their runs' values with the runs before the edit (`base`
/// is the run of the model the edit is of, when it simulates). The database
/// is restored whatever it finds, a panic included (`runs::Staging`).
///
/// The staged diagnostics and each run are units of the call's work: it
/// stops before each when other work waits for the project, and the staging
/// guard restores the database as it stops.
fn gate(
    ws: &mut Workspace<'_>,
    edit: &Edit<'_>,
    base: Option<&crate::Results>,
) -> Result<Gate, ToolError> {
    let session_model = edit.model.name.as_str();
    let working_model = edit.working.get_model(session_model).ok_or_else(|| {
        ToolError::new(format!("the project has no model named '{session_model}'"))
    })?;
    let source_project = ws.db.current_source_project().ok_or_else(|| {
        ToolError::new(
            "the project has not been compiled; the host must sync it before calling a tool",
        )
    })?;
    let renamed = rename_map(edit.renames);
    let waiting = ws.waiting;
    let is_waited_on = || waiting.is_some_and(|waiting| waiting());

    // The models whose runs are judged: the one the edit is of, and the
    // project's root model, which is what the person simulates.
    let root = resolve_datamodel_model(ws.project, "main")
        .map(|m| m.name.as_str())
        .filter(|root| *root != session_model);
    let own = |project: &datamodel::Project, name: &str| {
        project
            .get_model(name)
            .map(|m| crate::results::Specs::from(effective_specs(project, m)))
    };

    let before = findings(ws.db, source_project, ws.project, edit.model);
    let root_base = match root.zip(root.and_then(|root| own(ws.project, root))) {
        Some((root, specs)) => {
            ws.yield_point()?;
            run_of(ws.db, source_project, ws.project, root, &specs).ok()
        }
        None => None,
    };

    ws.yield_point()?;
    let staging = Staging::new(ws.db, ws.project, edit.working);
    let staged = staging.source_project;
    let after = findings(staging.db, staged, edit.working, working_model);
    let mut judged: Vec<(Option<&str>, Option<&crate::Results>)> = vec![(None, base)];
    if let Some(root) = root {
        judged.push((Some(root), root_base.as_ref()));
    }
    let mut runs = Vec::new();
    let mut non_finite = Vec::new();
    let mut undefined = Vec::new();
    let mut too_costly = None;
    let mut simulates = false;
    for (root, base) in judged {
        if is_waited_on() {
            return Err(ToolError::interrupted());
        }
        let name = root.unwrap_or(session_model);
        let Some(specs) = own(ws.project, name) else {
            continue;
        };
        let run = run_of(staging.db, staged, edit.working, name, &specs);
        if root.is_none() {
            simulates = run.is_ok();
        }
        let error = match &run {
            Ok(results) => {
                if let Some(run_model) = edit.working.get_model(name) {
                    let found = newly_non_finite(
                        results,
                        base,
                        edit.working,
                        run_model,
                        working_model,
                        &renamed,
                    );
                    if base.is_some() {
                        non_finite.extend(found);
                    } else {
                        undefined.extend(found);
                    }
                }
                None
            }
            Err(NoRun::Cost(reason)) => {
                too_costly.get_or_insert_with(|| reason.clone());
                Some(reason.clone())
            }
            Err(NoRun::Fails(reason)) => Some(reason.clone()),
        };
        runs.push(JudgedRun {
            root: root.map(str::to_string),
            simulated: base.is_some(),
            error,
        });
    }
    drop(staging);
    non_finite.sort_by(|a, b| {
        a.time
            .total_cmp(&b.time)
            .then_with(|| (&a.variable, &a.model).cmp(&(&b.variable, &b.model)))
    });
    non_finite.dedup();
    undefined.sort_by(|a, b| {
        a.time
            .total_cmp(&b.time)
            .then_with(|| (&a.variable, &a.model).cmp(&(&b.variable, &b.model)))
    });
    undefined.dedup();

    // An arrayed variable the edit wrote some arms of, which has an error:
    // the error may be another arm's, hiding one in the arms the edit wrote.
    let has_error = |findings: &[Finding], name: &str| {
        findings.iter().any(|f| {
            f.model == session_model
                && f.described.severity == Severity::Error
                && f.described.variable.as_deref() == Some(name)
        })
    };
    let hidden: BTreeSet<String> = edit
        .authored
        .iter()
        .filter(|(name, authored)| {
            matches!(authored, Authored::Elements(_)) && has_error(&after, name)
        })
        .map(|(name, _)| name.clone())
        .collect();
    let in_written_arms = match isolated(edit.working, session_model, edit.authored, &hidden) {
        Some(isolated) => {
            ws.yield_point()?;
            let staging = Staging::new(ws.db, ws.project, &isolated);
            let isolated_model = isolated.get_model(session_model).unwrap_or(working_model);
            findings(
                staging.db,
                staging.source_project,
                &isolated,
                isolated_model,
            )
            .into_iter()
            .filter(|f| {
                f.model == session_model
                    && f.described.severity == Severity::Error
                    && f.described
                        .variable
                        .as_ref()
                        .is_some_and(|name| hidden.contains(name))
            })
            .collect()
        }
        None => Vec::new(),
    };

    // The variables the model had an error in before the edit. Their errors
    // hide others (one error per equation, one cycle per model), so the
    // comparison is made again with them set aside: before the edit, all of
    // them, which shows what they hid there; after it, those the edit does
    // not write, which shows what the edit writes and what it makes.
    let broken: BTreeSet<String> = before
        .iter()
        .filter(|f| f.model == session_model && f.described.severity == Severity::Error)
        .filter_map(|f| f.described.variable.clone())
        .map(|name| crate::canonicalize(&name).into_owned())
        .collect();
    let unwritten_broken: BTreeSet<String> = broken
        .iter()
        .filter(|name| !edit.authored.contains_key(*name))
        .cloned()
        .collect();
    let unedited_project: &datamodel::Project = ws.project;
    let mut aside_findings = |base: &datamodel::Project, aside: &BTreeSet<String>| {
        let Some(project) = set_aside(base, session_model, aside) else {
            return Ok(Vec::new());
        };
        ws.yield_point()?;
        let staging = Staging::new(ws.db, unedited_project, &project);
        let model = project.get_model(session_model).unwrap_or(working_model);
        Ok::<_, ToolError>(findings(
            staging.db,
            staging.source_project,
            &project,
            model,
        ))
    };
    let unedited = unedited_project;
    let (before_aside, after_aside) = if broken.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        (
            aside_findings(unedited, &broken)?,
            aside_findings(edit.working, &unwritten_broken)?,
        )
    };

    // A diagnostic is the same problem when the evidence module would give
    // it the same id: its variable, severity, code and reason, here with its
    // model. A reason that names a variable the edit renames changes with
    // the name, so such a diagnostic is compared without its reason.
    let renamed_names: Vec<&String> = renamed.keys().chain(renamed.values()).collect();
    type Key = (String, DiagnosticIdentity);
    let key = |f: &Finding, before_the_edit: bool| -> Key {
        let (variable, severity, code, reason) = f.described.identity();
        let variable = variable.map(|v| {
            if before_the_edit && f.model == session_model {
                renamed.get(&v).cloned().unwrap_or(v)
            } else {
                v
            }
        });
        let reason = reason.filter(|r| !renamed_names.iter().any(|name| r.contains(name.as_str())));
        (f.model.clone(), (variable, severity, code, reason))
    };
    // How many errors of a code a variable has.
    let of_code = |k: &Key| (k.0.clone(), k.1.0.clone(), k.1.2);
    let mut had: HashMap<Key, usize> = HashMap::new();
    let mut had_of_code: HashMap<_, usize> = HashMap::new();
    let mut had_unit_warning: BTreeSet<&str> = BTreeSet::new();
    for finding in &before {
        let k = key(finding, true);
        if finding.described.severity == Severity::Error {
            *had_of_code.entry(of_code(&k)).or_default() += 1;
        } else if is_unit(finding.described.category) {
            had_unit_warning.insert(finding.model.as_str());
        }
        *had.entry(k).or_default() += 1;
    }
    // An error the broken variables hid before the edit was the model's
    // already: what the project had is, problem by problem, the most either
    // way of looking at it shows.
    {
        let mut hidden: HashMap<Key, usize> = HashMap::new();
        let mut hidden_of_code: HashMap<_, usize> = HashMap::new();
        for finding in before_aside
            .iter()
            .filter(|f| f.described.severity == Severity::Error)
        {
            let k = key(finding, true);
            *hidden_of_code.entry(of_code(&k)).or_default() += 1;
            *hidden.entry(k).or_default() += 1;
        }
        for (k, n) in hidden {
            let had = had.entry(k).or_default();
            *had = (*had).max(n);
        }
        for (code, n) in hidden_of_code {
            let had = had_of_code.entry(code).or_default();
            *had = (*had).max(n);
        }
    }
    // The comparison below spends `had` problem by problem; the comparison
    // with the broken variables set aside starts from it whole.
    let had_in_full = had.clone();
    let mut has_of_code: HashMap<_, usize> = HashMap::new();
    for finding in &after {
        if finding.described.severity == Severity::Error {
            *has_of_code
                .entry(of_code(&key(finding, false)))
                .or_default() += 1;
        }
    }

    let listed = |finding: Finding, hint: bool| -> EditDiagnostic {
        let model = edit.working.models.iter().find(|m| m.name == finding.model);
        let d = finding.described;
        let mut reason = d.reason;
        if hint
            && let Some(variable) = &d.variable
            && let Some(hint) = spelling_hint(working_model, variable, edit.authored)
        {
            reason = Some(match reason {
                Some(reason) => format!("{reason}; {hint}"),
                None => hint,
            });
        }
        EditDiagnostic {
            id: None,
            severity: d.severity,
            category: d.category,
            code: d.code.to_string(),
            model: (finding.model != session_model && !finding.model.is_empty())
                .then(|| finding.model.clone()),
            variable: d
                .variable
                .map(|name| model.map_or_else(|| name.clone(), |m| display_name(m, &name))),
            reason,
        }
    };
    // What the edit wrote of a diagnostic's variable, if anything.
    let wrote = |finding: &Finding| {
        (finding.model == session_model)
            .then_some(finding.described.variable.as_ref())
            .flatten()
            .and_then(|name| edit.authored.get(name))
    };
    // Whether a diagnostic is of the equation the edit wrote whole: one of
    // the variable's units is the units string's, which the edit did not
    // write.
    let in_written_equation = |finding: &Finding| {
        wrote(finding) == Some(&Authored::Whole) && !is_unit(finding.described.category)
    };

    let mut errors: Vec<(Found, Listed)> = Vec::new();
    let mut warnings = Vec::new();
    let mut first_unit_warning = None;
    let mut report = Vec::new();
    for finding in after {
        // A read reports the model's diagnostics and the project's own.
        let at = (finding.model == session_model || finding.model.is_empty()).then(|| {
            report.push(finding.described.identity());
            report.len() - 1
        });
        let k = key(&finding, false);
        let known = had.get_mut(&k).filter(|n| **n > 0).map(|n| *n -= 1);
        let is_new = known.is_none();
        match finding.described.severity {
            Severity::Error => {
                let code = of_code(&k);
                let rose = has_of_code.get(&code).copied().unwrap_or(0)
                    > had_of_code.get(&code).copied().unwrap_or(0);
                let why = if is_new && rose {
                    Listed::New
                } else if in_written_equation(&finding) {
                    Listed::Authored
                } else if is_new {
                    Listed::Changed
                } else {
                    continue;
                };
                let hint = wrote(&finding).is_some();
                let diagnostic = listed(finding, hint);
                errors.push((Found { diagnostic, at }, why));
            }
            Severity::Warning if is_new => {
                let first = is_unit(finding.described.category)
                    && !had_unit_warning.contains(finding.model.as_str());
                let diagnostic = listed(finding, false);
                if first && first_unit_warning.is_none() {
                    first_unit_warning = Some(diagnostic.clone());
                }
                warnings.push(Found { diagnostic, at });
            }
            Severity::Warning => {}
        }
    }
    for finding in in_written_arms {
        let error = listed(finding, true);
        match errors
            .iter_mut()
            .find(|(listed, _)| listed.diagnostic == error)
        {
            Some((_, why)) => {
                if *why == Listed::Changed {
                    *why = Listed::Authored;
                }
            }
            // An error the other arms hid: no report of the model has it.
            None => errors.push((Found::by_the_gate(error), Listed::Authored)),
        }
    }
    // With the broken variables the edit does not write set aside: an error
    // in an equation the edit writes is the edit's, and so is one elsewhere
    // that the model did not have, which they hid.
    let mut aside_had = had_in_full;
    for finding in after_aside
        .into_iter()
        .filter(|f| f.described.severity == Severity::Error)
    {
        let k = key(&finding, false);
        let known = aside_had.get_mut(&k).filter(|n| **n > 0).map(|n| *n -= 1);
        let why = if in_written_equation(&finding) {
            Listed::Authored
        } else if known.is_none() {
            Listed::New
        } else {
            continue;
        };
        let hint = wrote(&finding).is_some();
        let error = listed(finding, hint);
        match errors
            .iter_mut()
            .find(|(listed, _)| listed.diagnostic == error)
        {
            Some((_, listed_why)) => {
                if *listed_why == Listed::Changed {
                    *listed_why = why;
                }
            }
            None => errors.push((Found::by_the_gate(error), why)),
        }
    }

    // A variable the edit removes that a text still names, which the
    // compiler need not report: the reader's equation may have failed
    // already, or a cycle hide it.
    let dangling: Vec<Dangling> = edit
        .gone
        .iter()
        .filter_map(|(model, name, spelled)| {
            let readers: Vec<(Option<String>, String)> =
                crate::patch::variables_naming(edit.working, model, name)
                    .into_iter()
                    .map(|(reader_model, reader)| {
                        (
                            (reader_model != session_model).then_some(reader_model),
                            reader,
                        )
                    })
                    .collect();
            (!readers.is_empty()).then(|| Dangling {
                gone: in_model(spelled, model, session_model),
                readers,
            })
        })
        .collect();

    // The errors that refuse come first, so the first listed is the one a
    // refusal names.
    errors.sort_by_key(|(_, why)| *why == Listed::Changed);

    Ok(Gate {
        errors,
        warnings,
        report,
        first_unit_warning,
        non_finite,
        undefined,
        runs,
        simulates,
        too_costly,
        dangling,
    })
}

/// The runs of words in `text`: each stretch of names with only spaces
/// between them (or the stored `\n` escape of a name's line break),
/// lowercased. Anything else -- an operator, a bracket, a comma -- ends a run.
fn word_runs(text: &str) -> Vec<Vec<String>> {
    text.replace("\\n", " ")
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c.is_whitespace()))
        .map(|run| run.split_whitespace().map(str::to_lowercase).collect())
        .collect()
}

/// When an equation the edit wrote for the variable named `variable`
/// (canonically) spells one of the model's names as the model displays it,
/// in several words with spaces between, which the equation language reads
/// as several names: how to spell it.
fn spelling_hint(
    model: &datamodel::Model,
    variable: &str,
    authored: &BTreeMap<String, Authored>,
) -> Option<String> {
    let equation = model.get_variable(variable)?.get_equation()?;
    let texts: Vec<&str> = match (authored.get(variable)?, equation) {
        (Authored::Elements(written), Equation::Arrayed(_, elements, _, _)) => elements
            .iter()
            .filter(|(element, ..)| {
                written.contains(&CanonicalElementName::from_subscript(element))
            })
            .map(|(_, text, _, _)| text.as_str())
            .collect(),
        (Authored::Whole, Equation::Arrayed(_, elements, _, _)) => elements
            .iter()
            .map(|(_, text, _, _)| text.as_str())
            .collect(),
        (_, Equation::Scalar(text)) | (_, Equation::ApplyToAll(_, text)) => vec![text.as_str()],
    };
    // A name in quotes is one name already.
    let written: Vec<Vec<String>> = texts
        .iter()
        .flat_map(|text| text.split('"').step_by(2))
        .flat_map(word_runs)
        .collect();
    model
        .variables
        .iter()
        .filter_map(|v| {
            let name = match word_runs(v.get_ident()).as_slice() {
                [name] if name.len() > 1 => name.clone(),
                _ => return None,
            };
            written
                .iter()
                .any(|run| run.windows(name.len()).any(|window| window == name))
                .then(|| (name.len(), v.get_ident()))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, ident)| {
            format!(
                "'{}' is one variable: write it {} in an equation",
                ident.replace("\\n", " "),
                crate::canonicalize(ident)
            )
        })
}

/// A series the edit makes not a number, as an answer names it: the variable
/// (and element) as its model names it, the model when it is not the one the
/// edit is of, and when it first is not a number.
#[derive(Clone, PartialEq)]
struct NonFinite {
    variable: String,
    model: Option<String>,
    time: f64,
}

/// The series of `results` not a number somewhere, whose series in `base`
/// (the run before the edit, under the name the edit's renames came from) is
/// a number throughout, or that `base` lacks (every one, with no `base`):
/// each with when it first is not.
/// `run_model` is the model the run is of (of `project`), whose variables own
/// its columns (`save_check::column_variable`). A variable of `model`, the one
/// the edit is of, is named as that model names it, also where the run reads
/// it through module instances; any other is named in the run's model, which
/// the finding names.
fn newly_non_finite(
    results: &crate::Results,
    base: Option<&crate::Results>,
    project: &datamodel::Project,
    run_model: &datamodel::Model,
    model: &datamodel::Model,
    renamed: &BTreeMap<String, String>,
) -> Vec<NonFinite> {
    let origin: HashMap<&str, &str> = renamed
        .iter()
        .map(|(from, to)| (to.as_str(), from.as_str()))
        .collect();
    let rows: Vec<&[f64]> = results.iter().collect();
    let base_rows: Vec<&[f64]> = base.map(|base| base.iter().collect()).unwrap_or_default();
    let declared: BTreeSet<String> = run_model
        .variables
        .iter()
        .map(|var| crate::canonicalize(var.get_ident()).into_owned())
        .collect();
    // The model a path of instances (`a·b·`) leads to from the run's model.
    let through = |path: &str| {
        path.split('\u{00B7}')
            .filter(|segment| !segment.is_empty())
            .try_fold(run_model, |at, instance| match at.get_variable(instance) {
                Some(Variable::Module(module)) => project.get_model(&module.model_name),
                _ => None,
            })
    };
    let mut found = Vec::new();
    for (key, &offset) in &results.offsets {
        let key = key.as_str();
        // The clock and the compiler's helpers are no variable's series; a
        // variable's name may hold `$`, `[` or the word `time`.
        let Some(owner) = crate::save_check::column_variable(key, &declared) else {
            continue;
        };
        let Some(row) = rows.iter().find(|r| !r[offset].is_finite()) else {
            continue;
        };
        let variable = if owner == key {
            key
        } else {
            crate::ltm::strip_subscript(key)
        };
        let subscript = &key[variable.len()..];
        // A variable of another model reads, in this run, as the last segment
        // of a path through module instances.
        let (path, last) = match variable.rfind('\u{00B7}') {
            Some(at) => variable.split_at(at + '\u{00B7}'.len_utf8()),
            None => ("", variable),
        };
        let of_the_edited = through(path).is_some_and(|at| at.name == model.name);
        let base_key = if of_the_edited {
            format!(
                "{path}{}{subscript}",
                origin.get(last).copied().unwrap_or(last)
            )
        } else {
            key.to_string()
        };
        let was_finite = base
            .and_then(|base| {
                base.offsets
                    .get(&crate::common::Ident::<crate::common::Canonical>::new(
                        &base_key,
                    ))
            })
            .is_none_or(|&offset| base_rows.iter().all(|r| r[offset].is_finite()));
        if was_finite {
            let (variable, model_name) = if of_the_edited {
                (format!("{}{subscript}", display_name(model, last)), None)
            } else {
                (
                    format!("{}{}", display_name(run_model, owner), &key[owner.len()..]),
                    Some(run_model.name.clone()),
                )
            };
            found.push(NonFinite {
                variable,
                model: model_name,
                time: crate::tools::series::round(row[crate::results::TIME_OFF]),
            });
        }
    }
    found
}

/// A module's input wiring by port: the source each port takes.
fn wiring(module: &datamodel::Module) -> BTreeMap<String, String> {
    let prefix = format!("{}\u{00B7}", crate::canonicalize(&module.ident));
    module
        .references
        .iter()
        .map(|reference| {
            let dst = crate::canonicalize(&reference.dst).into_owned();
            let port = dst.strip_prefix(&prefix).unwrap_or(&dst).to_string();
            let src = crate::canonicalize(&reference.src);
            (port, src.trim_start_matches('\u{00B7}').to_string())
        })
        .collect()
}

/// The inputs of the modules of the model named `model_name` that the edit
/// leaves unwired, each as a warning: an unwired input takes the value its
/// own model's equation gives it, so the model's numbers change with no
/// error to say so.
fn unwired_inputs(
    before: &datamodel::Project,
    after: &datamodel::Project,
    model_name: &str,
) -> Vec<EditDiagnostic> {
    let (Some(old), Some(new)) = (before.get_model(model_name), after.get_model(model_name)) else {
        return vec![];
    };
    let mut warnings = Vec::new();
    for var in new.variables.iter() {
        let Variable::Module(is) = var else { continue };
        // The same module before the edit: by its uid, which a rename
        // keeps, or by its name where it has none.
        let was = old.variables.iter().find_map(|v| match v {
            Variable::Module(was)
                if (was.uid.is_some() && was.uid == is.uid)
                    || crate::canonicalize(&was.ident) == crate::canonicalize(&is.ident) =>
            {
                Some(was)
            }
            _ => None,
        });
        let Some(was) = was else { continue };
        let now = wiring(is);
        for (port, src) in wiring(was) {
            if !now.contains_key(&port) {
                warnings.push(EditDiagnostic {
                    id: None,
                    severity: Severity::Warning,
                    category: DiagnosticCategoryName::Model,
                    code: "module_input_unwired".to_string(),
                    model: None,
                    variable: Some(is.ident.clone()),
                    reason: Some(format!(
                        "its input '{port}' is no longer wired from '{src}': it takes the value \
                         the equation of '{port}' in model '{}' gives it",
                        is.model_name
                    )),
                });
            }
        }
    }
    warnings
}

/// `delta` in words: one line per variable the edit changes, the model
/// named `session_model`'s and then the project's other models', each
/// model's in the order of their names, then one per loop named and one for
/// the sim specs. `after` is the project as the
/// edit leaves it, and `renames` the edit's, in the model it is of.
fn change_lines(
    delta: &Delta,
    after: &datamodel::Project,
    session_model: &str,
    renames: &[(String, String)],
) -> Vec<ChangeLine> {
    // A rename is one line, from the old name to the new: the old name's
    // record gone and the new one's there.
    let renamed = rename_map(renames);
    let arrived: BTreeSet<&String> = renamed.values().collect();
    let own = delta
        .variables
        .iter()
        .filter(|((model, _), _)| model == session_model);
    let others = delta
        .variables
        .iter()
        .filter(|((model, _), _)| model != session_model);
    let mut changes = Vec::new();
    for ((model, name), change) in own.chain(others) {
        // Names a line gives are spelled as the model spells them.
        let spelled_in = after.get_model(model);
        let own = model == session_model;
        let label = |var: &Variable| in_model(var.get_ident(), model, session_model);
        let new_name = renamed
            .get(name)
            .filter(|new_name| own && *new_name != name);
        // A record at a name a rename arrives at is the renamed variable's,
        // which its own line tells of.
        let added = change
            .after
            .as_ref()
            .filter(|_| !(own && arrived.contains(name)));
        let Some(was) = &change.before else {
            if let Some(var) = added {
                changes.push(ChangeLine {
                    variable: label(var),
                    action: ChangeAction::Added,
                    detail: added_detail(var, spelled_in),
                });
            }
            continue;
        };
        let is = match new_name {
            Some(new_name) => delta
                .variables
                .get(&(model.clone(), new_name.clone()))
                .and_then(|there| there.after.as_ref()),
            None => change.after.as_ref(),
        };
        let Some(is) = is else {
            changes.push(ChangeLine {
                variable: label(was),
                action: ChangeAction::Deleted,
                detail: format!("deleted {}", kind_name(was)),
            });
            continue;
        };
        let mut details = Vec::new();
        if new_name.is_some() {
            details.push(format!("renamed to {}", is.get_ident()));
        }
        details.extend(
            changed_fields(was, is)
                .into_iter()
                // A rename says so itself.
                .filter(|field| !(*field == ChangedField::Name && new_name.is_some()))
                .map(|field| field_detail(field, was, is, spelled_in)),
        );
        changes.push(ChangeLine {
            variable: label(was),
            action: if new_name.is_some() {
                ChangeAction::Renamed
            } else {
                ChangeAction::Changed
            },
            detail: details.join("; "),
        });
        // A variable added under the name a rename left.
        if new_name.is_some()
            && let Some(var) = added
        {
            changes.push(ChangeLine {
                variable: label(var),
                action: ChangeAction::Added,
                detail: added_detail(var, spelled_in),
            });
        }
    }
    for change in delta.loops.values() {
        if let Some(name) = &change.after {
            changes.push(ChangeLine {
                variable: name.name.clone(),
                action: ChangeAction::LoopNamed,
                detail: format!("names the loop through {}", change.through.join(", ")),
            });
        }
    }
    if let Some(old_specs) = &delta.specs {
        let new_specs = &after.sim_specs;
        let mut specs = Vec::new();
        let number = crate::results::written;
        for (field, was, is) in [
            ("start", old_specs.start, new_specs.start),
            ("stop", old_specs.stop, new_specs.stop),
            ("dt", dt_value(&old_specs.dt), dt_value(&new_specs.dt)),
        ] {
            if was != is {
                specs.push(format!("{field} {} (was {})", number(is), number(was)));
            }
        }
        // A save step finer than a new DT is dropped: the run saves every
        // step.
        match (&old_specs.save_step, &new_specs.save_step) {
            (Some(was), None) => specs.push(format!(
                "saves every step (was every {})",
                number(dt_value(was))
            )),
            (was, Some(is)) if was.as_ref().map(dt_value) != Some(dt_value(is)) => {
                specs.push(format!("saves every {}", number(dt_value(is))))
            }
            _ => {}
        }
        if old_specs.sim_method != new_specs.sim_method {
            specs.push(format!(
                "method {} (was {})",
                method_name(new_specs.sim_method),
                method_name(old_specs.sim_method)
            ));
        }
        if !specs.is_empty() {
            changes.push(ChangeLine {
                variable: "sim specs".to_string(),
                action: ChangeAction::SimSpecs,
                detail: specs.join("; "),
            });
        }
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

/// `flows`, each spelled as `model` spells the flow, or "none".
fn flow_list(flows: &[String], model: Option<&datamodel::Model>) -> String {
    if flows.is_empty() {
        return "none".to_string();
    }
    flows
        .iter()
        .map(|flow| model.map_or_else(|| flow.clone(), |m| display_name(m, flow)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A variable's lookup, as a line says it: how it reads between and past
/// its points, how many, and the range of their x and y values.
fn lookup_detail(var: &Variable) -> Option<String> {
    let gf = match var {
        Variable::Flow(flow) => flow.gf.as_ref(),
        Variable::Aux(aux) => aux.gf.as_ref(),
        Variable::Stock(_) | Variable::Module(_) => None,
    }?;
    let points = gf.y_points.len();
    let number = crate::results::written;
    let range = |values: &mut dyn Iterator<Item = f64>| {
        let (low, high) = values.fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), v| {
            (low.min(v), high.max(v))
        });
        if low <= high {
            format!("{} to {}", number(low), number(high))
        } else {
            "none".to_string()
        }
    };
    let xs = match &gf.x_points {
        Some(xs) => range(&mut xs.iter().copied()),
        None => format!("{} to {}", number(gf.x_scale.min), number(gf.x_scale.max)),
    };
    let kind = match gf.kind {
        datamodel::GraphicalFunctionKind::Continuous => "a lookup",
        datamodel::GraphicalFunctionKind::Extrapolate => "an extrapolating lookup",
        datamodel::GraphicalFunctionKind::Discrete => "a discrete lookup",
    };
    Some(format!(
        "{kind} of {points} point{}, x {xs}, y {}",
        if points == 1 { "" } else { "s" },
        range(&mut gf.y_points.iter().copied())
    ))
}

fn added_detail(var: &Variable, model: Option<&datamodel::Model>) -> String {
    let mut detail = match var {
        Variable::Stock(stock) => {
            let mut detail = format!("stock, initial value {}", equation_of(var));
            if !stock.inflows.is_empty() {
                detail += &format!(", filled by {}", flow_list(&stock.inflows, model));
            }
            if !stock.outflows.is_empty() {
                detail += &format!(", drained by {}", flow_list(&stock.outflows, model));
            }
            detail
        }
        Variable::Flow(_) => format!("flow = {}", equation_of(var)),
        Variable::Aux(_) => format!("variable = {}", equation_of(var)),
        Variable::Module(_) => "module".to_string(),
    };
    if let Some(lookup) = lookup_detail(var) {
        detail += &format!(", through {lookup}");
    }
    if let Some(units) = var.get_units() {
        detail += &format!(" ({units})");
    }
    detail
}

/// What changed of a module: the model it instantiates, and each input
/// whose wiring differs.
fn module_detail(was: &datamodel::Module, is: &datamodel::Module) -> String {
    let mut details = Vec::new();
    if was.model_name != is.model_name {
        details.push(format!(
            "an instance of {} (was {})",
            is.model_name, was.model_name
        ));
    }
    let (old, new) = (wiring(was), wiring(is));
    // An input the model the module instantiates renamed is the old port
    // gone and a new one wired from the same source.
    let mut renamed_from: BTreeSet<&String> = BTreeSet::new();
    for (port, src) in &new {
        match old.get(port) {
            Some(was) if was != src => details.push(format!("input {port} from {src} (was {was})")),
            Some(_) => {}
            None => {
                let before = old.iter().find(|(old_port, old_src)| {
                    *old_src == src
                        && !new.contains_key(*old_port)
                        && !renamed_from.contains(old_port)
                });
                match before {
                    Some((old_port, _)) => {
                        renamed_from.insert(old_port);
                        details.push(format!("input {port} from {src} (was input {old_port})"));
                    }
                    None => details.push(format!("input {port} from {src}")),
                }
            }
        }
    }
    for (port, src) in &old {
        if !new.contains_key(port) && !renamed_from.contains(port) {
            details.push(format!("input {port} unwired (was from {src})"));
        }
    }
    if details.is_empty() {
        "module wiring".to_string()
    } else {
        details.join("; ")
    }
}

fn field_detail(
    field: ChangedField,
    was: &Variable,
    is: &Variable,
    model: Option<&datamodel::Model>,
) -> String {
    let flows = |var: &Variable, inflows: bool| match var {
        Variable::Stock(stock) => flow_list(
            if inflows {
                &stock.inflows
            } else {
                &stock.outflows
            },
            model,
        ),
        _ => "none".to_string(),
    };
    // A stock's equation is its initial value.
    let what = |var: &Variable| {
        if matches!(var, Variable::Stock(_)) {
            "initial value"
        } else {
            "equation"
        }
    };
    match field {
        // A change of kind is reported alone ([`changed_fields`]), so it
        // says what the variable is now computed from.
        ChangedField::Kind => format!(
            "now a {} (was a {}), {} {} (was {} {})",
            kind_name(is),
            kind_name(was),
            what(is),
            equation_of(is),
            what(was),
            equation_of(was)
        ),
        ChangedField::Equation => {
            // One equation in place of per-element ones applies to them all.
            let every = matches!(was.get_equation(), Some(Equation::Arrayed(..)))
                && matches!(is.get_equation(), Some(Equation::ApplyToAll(..)));
            format!(
                "{} {}{} (was {})",
                what(is),
                equation_of(is),
                if every { " for every element" } else { "" },
                equation_of(was)
            )
        }
        ChangedField::Units => format!(
            "units {} (was {})",
            is.get_units().map_or("none", String::as_str),
            was.get_units().map_or("none", String::as_str)
        ),
        ChangedField::Documentation => "notes".to_string(),
        ChangedField::Lookup => {
            let (is, was) = (
                lookup_detail(is).unwrap_or_else(|| "no lookup".to_string()),
                lookup_detail(was).unwrap_or_else(|| "no lookup".to_string()),
            );
            if is == was {
                format!("{is}, its points moved")
            } else {
                format!("{is} (was {was})")
            }
        }
        ChangedField::Inflows => format!("inflows {} (was {})", flows(is, true), flows(was, true)),
        ChangedField::Outflows => {
            format!("outflows {} (was {})", flows(is, false), flows(was, false))
        }
        ChangedField::NonNegative => "non-negative".to_string(),
        ChangedField::Module => match (was, is) {
            (Variable::Module(was), Variable::Module(is)) => module_detail(was, is),
            _ => "module wiring".to_string(),
        },
        ChangedField::Name => format!("written {} (was {})", is.get_ident(), was.get_ident()),
        ChangedField::Other => "other settings".to_string(),
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "edit_reach_tests.rs"]
mod reach_tests;

#[cfg(test)]
#[path = "edit_property_tests.rs"]
mod property_tests;
