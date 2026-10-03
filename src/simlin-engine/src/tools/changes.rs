// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What changed in a model since an agent last read it.
//!
//! A session keeps what its last read saw ([`ReadSnapshot`]): the model's
//! variable records and sim specs, and what they rest on -- the project's
//! dimensions and unit definitions, and its other models, which a module
//! instantiates. [`diff`] compares them with the project now. Only what a
//! simulation or an agent reads counts: a view edit (a moved stock, a
//! reshaped pipe) changes nothing an agent read, so it is no change here,
//! though it advances the project's revision.

use std::collections::BTreeMap;

use serde::Serialize;

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::datamodel::{self, Variable};

/// The most names a change list carries; the rest are counted.
pub(crate) const MAX_CHANGED_NAMES: usize = 30;

/// A variable's fields, as a change report names them.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChangedField {
    /// It became a different kind of variable (an auxiliary became a stock).
    Kind,
    /// Its equation, or for a stock its initial value.
    Equation,
    Units,
    Documentation,
    Lookup,
    Inflows,
    Outflows,
    NonNegative,
    /// A module's model or its input wiring.
    Module,
    /// How its name is written, when only case or spacing changed; a rename
    /// is a removal and an addition.
    Name,
    /// Anything else an import carries (an active initial value, a data
    /// source, a conveyor or queue marker, visibility).
    Other,
}

impl ChangedField {
    pub const ALL: [ChangedField; 11] = [
        ChangedField::Kind,
        ChangedField::Equation,
        ChangedField::Units,
        ChangedField::Documentation,
        ChangedField::Lookup,
        ChangedField::Inflows,
        ChangedField::Outflows,
        ChangedField::NonNegative,
        ChangedField::Module,
        ChangedField::Name,
        ChangedField::Other,
    ];
}

/// One variable whose record changed, and which of its fields did.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ChangedVariable {
    pub name: String,
    pub fields: Vec<ChangedField>,
}

/// What changed since a read. Each list of variables is capped at 30 names,
/// and an outline cuts the lists further to fit its budget
/// ([`Changes::fitted`]), with the total counted beside a list when either
/// binds.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Default, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Changes {
    /// The revision the agent last read at.
    pub since_revision: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_count: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed_count: Option<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<ChangedVariable>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_count: Option<usize>,
    /// Whether the start, stop, DT, save step, integration method or time
    /// units changed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub specs_changed: bool,
    /// Whether the project's dimensions changed: an element added, removed
    /// or renamed, or a dimension added or removed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub dimensions_changed: bool,
    /// Whether the project's unit definitions changed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unit_definitions_changed: bool,
    /// The project's other models whose variables or sim specs changed (the
    /// models a module instantiates among them), and those added or removed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models_changed: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models_changed_count: Option<usize>,
}

impl Changes {
    /// The report within `budget` bytes of JSON, as an outline carries it:
    /// each name cut as an answer cuts text it repeats
    /// (`evidence::window`), then the longest of its lists halved, its total
    /// counted beside it, until it fits. Never refused: a read is answered
    /// whatever the person changed, and what it leaves out is counted.
    pub(crate) fn fitted(mut self, budget: usize) -> Changes {
        let cut = |name: &mut String| {
            *name = super::evidence::window(name, 0, 0, super::evidence::ECHO_CHARS);
        };
        self.added.iter_mut().for_each(cut);
        self.removed.iter_mut().for_each(cut);
        self.changed
            .iter_mut()
            .for_each(|changed| cut(&mut changed.name));
        self.models_changed.iter_mut().for_each(cut);
        super::fit(&mut self, budget, |changes| {
            let lengths = [
                changes.added.len(),
                changes.removed.len(),
                changes.changed.len(),
                changes.models_changed.len(),
            ];
            let longest = (0..4).max_by_key(|&i| lengths[i]).unwrap_or(0);
            let keep = lengths[longest] / 2;
            match longest {
                _ if lengths[longest] == 0 => return false,
                0 => {
                    changes.added_count.get_or_insert(changes.added.len());
                    changes.added.truncate(keep);
                }
                1 => {
                    changes.removed_count.get_or_insert(changes.removed.len());
                    changes.removed.truncate(keep);
                }
                2 => {
                    changes.changed_count.get_or_insert(changes.changed.len());
                    changes.changed.truncate(keep);
                }
                _ => {
                    changes
                        .models_changed_count
                        .get_or_insert(changes.models_changed.len());
                    changes.models_changed.truncate(keep);
                }
            }
            true
        });
        self
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && !self.specs_changed
            && !self.dimensions_changed
            && !self.unit_definitions_changed
            && self.models_changed.is_empty()
    }
}

/// The part of a model an agent read, at the revision it read it.
pub(crate) struct ReadSnapshot {
    pub(crate) revision: u64,
    /// Each variable's record by canonical name.
    variables: BTreeMap<String, Variable>,
    specs: datamodel::SimSpecs,
    /// The model's loop names.
    loops: Vec<datamodel::LoopMetadata>,
    dimensions: Vec<datamodel::Dimension>,
    units: Vec<datamodel::Unit>,
    /// The project's other models by name: what of each a simulation reads.
    other_models: BTreeMap<String, ModelContents>,
}

/// What a simulation reads of a model: its variables and its own sim specs,
/// and not its views, groups or loop names.
#[derive(Clone, PartialEq)]
struct ModelContents {
    /// Shared with the model's own, element by element.
    variables: datamodel::SharedVec<Variable>,
    sim_specs: Option<datamodel::SimSpecs>,
}

impl ModelContents {
    fn of(model: &datamodel::Model) -> ModelContents {
        ModelContents {
            variables: model.variables.iter().map(without_provenance).collect(),
            sim_specs: model.sim_specs.clone(),
        }
    }
}

/// `var` with no record of who made it: what is compared when the question is
/// whether a variable changed, since provenance changes with every edit and
/// describes the edit, not the model.
pub(crate) fn without_provenance(var: &Variable) -> Variable {
    let mut var = var.clone();
    match &mut var {
        Variable::Stock(v) => v.ai_state = None,
        Variable::Flow(v) => v.ai_state = None,
        Variable::Aux(v) => v.ai_state = None,
        Variable::Module(v) => v.ai_state = None,
    }
    var
}

/// Whether two records (none: absent) are the same variable: no field a
/// change report names differs ([`changed_fields`]). Who made a variable
/// (`ai_state`) and its uid are not the variable: an edit of either changes
/// nothing an agent read or a simulation reads.
pub(crate) fn same_record(a: Option<&Variable>, b: Option<&Variable>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => changed_fields(a, b).is_empty(),
        (None, None) => true,
        _ => false,
    }
}

/// The project's models other than `model`, by name.
fn other_models(
    project: &datamodel::Project,
    model: &datamodel::Model,
) -> BTreeMap<String, ModelContents> {
    project
        .models
        .iter()
        .filter(|m| m.name != model.name)
        .map(|m| (m.name.clone(), ModelContents::of(m)))
        .collect()
}

impl ReadSnapshot {
    pub(crate) fn new(
        revision: u64,
        project: &datamodel::Project,
        model: &datamodel::Model,
    ) -> ReadSnapshot {
        ReadSnapshot {
            revision,
            variables: model
                .variables
                .iter()
                .map(|v| (crate::canonicalize(v.get_ident()).into_owned(), v.clone()))
                .collect(),
            specs: effective_specs(project, model).clone(),
            loops: model.loop_metadata.clone(),
            dimensions: project.dimensions.clone(),
            units: project.units.clone(),
            other_models: other_models(project, model),
        }
    }
}

impl ReadSnapshot {
    /// The record the read gave the variable named `name` (canonically), if
    /// the model had it then.
    pub(crate) fn record(&self, name: &str) -> Option<&Variable> {
        self.variables.get(name)
    }

    /// The sim specs the read gave.
    pub(crate) fn specs(&self) -> &datamodel::SimSpecs {
        &self.specs
    }

    /// The name the read gave the loop whose variables have the uids `uids`
    /// (sorted), if it had one.
    pub(crate) fn loop_named(&self, uids: &[i32]) -> Option<&datamodel::LoopMetadata> {
        self.loops.iter().find(|entry| {
            let mut entry_uids = entry.uids.clone();
            entry_uids.sort_unstable();
            entry_uids == uids
        })
    }

    /// The record the read gave the variable named `name` (canonically) of
    /// the project's other model `model`, if that model had it then.
    pub(crate) fn record_in(&self, model: &str, name: &str) -> Option<&Variable> {
        self.other_models
            .get(model)?
            .variables
            .iter()
            .find(|v| crate::canonicalize(v.get_ident()).as_ref() == name)
    }

    /// Hold the variable named `name` (canonically) as `record` (none:
    /// absent), as though the read had given it: what the session's own edit
    /// left of it. `model` names another model of the project; none is the
    /// session's own.
    pub(crate) fn absorb_variable(
        &mut self,
        model: Option<&str>,
        name: &str,
        record: Option<&Variable>,
    ) {
        let Some(model) = model else {
            match record {
                Some(record) => self.variables.insert(name.to_string(), record.clone()),
                None => self.variables.remove(name),
            };
            return;
        };
        let Some(contents) = self.other_models.get_mut(model) else {
            return;
        };
        let at = contents
            .variables
            .iter()
            .position(|v| crate::canonicalize(v.get_ident()).as_ref() == name);
        match (at, record.map(without_provenance)) {
            (Some(at), Some(record)) => contents.variables.replace(at, record),
            (Some(at), None) => {
                contents.variables.remove(at);
            }
            (None, Some(record)) => contents.variables.push(record),
            (None, None) => {}
        }
    }

    /// Hold `specs` as the sim specs the read gave.
    pub(crate) fn absorb_specs(&mut self, specs: &datamodel::SimSpecs) {
        self.specs = specs.clone();
    }

    /// Hold `entry` (none: unnamed) as the name the read gave the loop whose
    /// variables have the uids `uids` (sorted).
    pub(crate) fn absorb_loop(&mut self, uids: &[i32], entry: Option<&datamodel::LoopMetadata>) {
        self.loops.retain(|held| {
            let mut held_uids = held.uids.clone();
            held_uids.sort_unstable();
            held_uids != uids
        });
        self.loops.extend(entry.cloned());
    }
}

/// The sim specs a model runs under: its own, else the project's.
pub(crate) fn effective_specs<'a>(
    project: &'a datamodel::Project,
    model: &'a datamodel::Model,
) -> &'a datamodel::SimSpecs {
    model.sim_specs.as_ref().unwrap_or(&project.sim_specs)
}

/// What changed between `snapshot` and `model` as it is in `project`. A
/// session's own edit is no change: the snapshot holds what it left as read
/// (`absorb_variable`).
pub(crate) fn diff(
    snapshot: &ReadSnapshot,
    project: &datamodel::Project,
    model: &datamodel::Model,
) -> Changes {
    let now: BTreeMap<String, &Variable> = model
        .variables
        .iter()
        .map(|v| (crate::canonicalize(v.get_ident()).into_owned(), v))
        .collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for (canonical, var) in &now {
        match snapshot.variables.get(canonical) {
            None => added.push(var.get_ident().to_string()),
            Some(before) => {
                let fields = changed_fields(before, var);
                if !fields.is_empty() {
                    changed.push(ChangedVariable {
                        name: var.get_ident().to_string(),
                        fields,
                    });
                }
            }
        }
    }
    let removed: Vec<String> = snapshot
        .variables
        .iter()
        .filter(|(canonical, _)| !now.contains_key(*canonical))
        .map(|(_, var)| var.get_ident().to_string())
        .collect();
    let specs = effective_specs(project, model);
    let (added, added_count) = cap(added);
    let (removed, removed_count) = cap(removed);
    let (changed, changed_count) = cap(changed);
    let others = other_models(project, model);
    let mut models_changed: Vec<String> = others
        .iter()
        .filter(|(name, contents)| snapshot.other_models.get(*name) != Some(*contents))
        .map(|(name, _)| name.clone())
        .chain(
            snapshot
                .other_models
                .keys()
                .filter(|name| !others.contains_key(*name))
                .cloned(),
        )
        .collect();
    models_changed.sort();
    let (models_changed, _) = cap(models_changed);
    Changes {
        since_revision: snapshot.revision,
        added,
        added_count,
        removed,
        removed_count,
        changed,
        changed_count,
        specs_changed: &snapshot.specs != specs,
        dimensions_changed: snapshot.dimensions != project.dimensions,
        unit_definitions_changed: snapshot.units != project.units,
        models_changed,
        models_changed_count: None,
    }
}

/// A list capped at [`MAX_CHANGED_NAMES`], and its length when the cap bound.
fn cap<T>(mut list: Vec<T>) -> (Vec<T>, Option<usize>) {
    let total = list.len();
    if total > MAX_CHANGED_NAMES {
        list.truncate(MAX_CHANGED_NAMES);
        (list, Some(total))
    } else {
        (list, None)
    }
}

/// The fields in which two records of one variable differ, in
/// [`ChangedField`] order. A change of kind is reported alone: a stock and an
/// auxiliary have different fields, so nothing else compares.
pub(crate) fn changed_fields(before: &Variable, after: &Variable) -> Vec<ChangedField> {
    use Variable::{Aux, Flow, Module, Stock};
    let mut fields = Vec::new();
    let renamed = before.get_ident() != after.get_ident();
    let mut note = |differs: bool, field: ChangedField| {
        if differs {
            fields.push(field);
        }
    };
    match (before, after) {
        (Stock(a), Stock(b)) => {
            note(a.equation != b.equation, ChangedField::Equation);
            note(a.units != b.units, ChangedField::Units);
            note(
                a.documentation != b.documentation,
                ChangedField::Documentation,
            );
            note(
                datamodel::distinct_stock_flows(&a.inflows).flows
                    != datamodel::distinct_stock_flows(&b.inflows).flows,
                ChangedField::Inflows,
            );
            note(
                datamodel::distinct_stock_flows(&a.outflows).flows
                    != datamodel::distinct_stock_flows(&b.outflows).flows,
                ChangedField::Outflows,
            );
            note(
                a.compat.non_negative != b.compat.non_negative,
                ChangedField::NonNegative,
            );
            note(
                other_compat_differs(&a.compat, &b.compat),
                ChangedField::Other,
            );
        }
        (Flow(a), Flow(b)) => {
            note(a.equation != b.equation, ChangedField::Equation);
            note(a.units != b.units, ChangedField::Units);
            note(
                a.documentation != b.documentation,
                ChangedField::Documentation,
            );
            note(a.gf != b.gf, ChangedField::Lookup);
            note(
                a.compat.non_negative != b.compat.non_negative,
                ChangedField::NonNegative,
            );
            note(
                other_compat_differs(&a.compat, &b.compat),
                ChangedField::Other,
            );
        }
        (Aux(a), Aux(b)) => {
            note(a.equation != b.equation, ChangedField::Equation);
            note(a.units != b.units, ChangedField::Units);
            note(
                a.documentation != b.documentation,
                ChangedField::Documentation,
            );
            note(a.gf != b.gf, ChangedField::Lookup);
            note(
                a.compat.non_negative != b.compat.non_negative,
                ChangedField::NonNegative,
            );
            note(
                other_compat_differs(&a.compat, &b.compat),
                ChangedField::Other,
            );
        }
        (Module(a), Module(b)) => {
            note(a.units != b.units, ChangedField::Units);
            note(
                a.documentation != b.documentation,
                ChangedField::Documentation,
            );
            note(
                a.model_name != b.model_name || a.references != b.references,
                ChangedField::Module,
            );
            note(
                other_compat_differs(&a.compat, &b.compat),
                ChangedField::Other,
            );
        }
        _ => note(true, ChangedField::Kind),
    }
    if renamed {
        fields.push(ChangedField::Name);
        fields.sort();
    }
    fields
}

/// Whether two records differ in a compat field other than `non_negative`,
/// which is reported on its own.
fn other_compat_differs(a: &datamodel::Compat, b: &datamodel::Compat) -> bool {
    let strip = |c: &datamodel::Compat| datamodel::Compat {
        non_negative: false,
        ..c.clone()
    };
    strip(a) != strip(b)
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
