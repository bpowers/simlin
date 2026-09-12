// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A view edit's model operations, and the view edits a delete and a rename
//! imply.
//!
//! A host edits a view by upserting and removing elements (`ViewEdit`, applied
//! by `ModelOperation::EditView`). The model operations the edit implies are
//! derived from the difference between the committed view and the view the
//! edit produces, against the committed model, when the patch applies: a
//! payload is never built from state an earlier edit changed, and the model and
//! its diagram land together or not at all. Idents are derived from element
//! NAMES, which are what the engine stores and matches canonically.

use std::collections::{HashMap, HashSet};

use crate::common::{Error, ErrorCode, ErrorKind, Result, canonicalize};
use crate::datamodel::view_element::{Cloud, Flow};
use crate::datamodel::{self, Compat, Equation, StockFlow, Variable, ViewElement};
use crate::patch::ModelOperation;

use super::base::{VariableKind, is_named};
use super::geometry::FlowEnd;
use super::gesture::ViewEdit;

/// The view `edit` produces from `base`: removals dropped, upserts substituted
/// in place (keeping draw order) or appended.
pub(crate) fn edited_view(base: &StockFlow, upsert: &[ViewElement], remove: &[i32]) -> StockFlow {
    let removed: HashSet<i32> = remove.iter().copied().collect();
    let mut upserts: HashMap<i32, &ViewElement> = upsert.iter().map(|e| (e.get_uid(), e)).collect();
    let mut elements = Vec::with_capacity(base.elements.len() + upsert.len());
    for element in &base.elements {
        let uid = element.get_uid();
        if removed.contains(&uid) {
            continue;
        }
        match upserts.remove(&uid) {
            Some(next) => elements.push(next.clone()),
            None => elements.push(element.clone()),
        }
    }
    for element in upsert {
        let uid = element.get_uid();
        if !removed.contains(&uid) && upserts.remove(&uid).is_some() {
            elements.push(element.clone());
        }
    }
    StockFlow {
        elements,
        ..base.clone()
    }
}

fn kind_of(variable: &Variable) -> VariableKind {
    match variable {
        Variable::Stock(_) => VariableKind::Stock,
        Variable::Flow(_) => VariableKind::Flow,
        Variable::Aux(_) => VariableKind::Aux,
        Variable::Module(_) => VariableKind::Module,
    }
}

fn element_kind(element: &ViewElement) -> Option<VariableKind> {
    match element {
        ViewElement::Stock(_) => Some(VariableKind::Stock),
        ViewElement::Flow(_) => Some(VariableKind::Flow),
        ViewElement::Aux(_) => Some(VariableKind::Aux),
        ViewElement::Module(_) => Some(VariableKind::Module),
        _ => None,
    }
}

fn ident_of(element: &ViewElement) -> Option<String> {
    element
        .get_name()
        .map(|name| canonicalize(name).into_owned())
}

/// The variable a created element makes: empty equations, no flows, no
/// references, for the user to fill in.
fn create_operation(element: &ViewElement) -> Option<ModelOperation> {
    let ident = element.get_name()?.to_string();
    Some(match element {
        ViewElement::Stock(_) => ModelOperation::UpsertStock(datamodel::Stock {
            ident,
            equation: Equation::Scalar(String::new()),
            documentation: String::new(),
            units: None,
            inflows: Vec::new(),
            outflows: Vec::new(),
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }),
        ViewElement::Flow(_) => ModelOperation::UpsertFlow(datamodel::Flow {
            ident,
            equation: Equation::Scalar(String::new()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }),
        ViewElement::Aux(_) => ModelOperation::UpsertAux(datamodel::Aux {
            ident,
            equation: Equation::Scalar(String::new()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }),
        ViewElement::Module(_) => ModelOperation::UpsertModule(datamodel::Module {
            ident,
            model_name: String::new(),
            documentation: String::new(),
            units: None,
            references: Vec::new(),
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }),
        _ => return None,
    })
}

fn named_elements(view: &StockFlow) -> Vec<(i32, &ViewElement)> {
    view.elements
        .iter()
        .filter(|e| is_named(e))
        .map(|e| (e.get_uid(), e))
        .collect()
}

fn conflict(message: String) -> Error {
    Error::new(
        ErrorKind::Model,
        ErrorCode::DuplicateVariable,
        Some(message),
    )
}

#[derive(Default)]
struct ListDelta {
    add: Vec<String>,
    remove: HashSet<String>,
}

#[derive(Default)]
struct StockDelta {
    inflows: ListDelta,
    outflows: ListDelta,
}

/// The ident of the stock `flow`'s `end` is attached to, if any.
fn attached_stock_ident(
    by_uid: &HashMap<i32, &ViewElement>,
    flow: Option<&Flow>,
    end: FlowEnd,
) -> Option<String> {
    let flow = flow?;
    let p = match end {
        FlowEnd::Source => flow.points.first(),
        FlowEnd::Sink => flow.points.last(),
    }?;
    match by_uid.get(&p.attached_to_uid?) {
        Some(ViewElement::Stock(s)) => Some(canonicalize(&s.name).into_owned()),
        _ => None,
    }
}

/// The model operations replacing `base` with `next` implies, in the order one
/// patch applies them:
///
/// 1. `RenameVariable` for every named element whose uid survives under a
///    different name (`from` the committed ident, `to` the new name as typed,
///    which the engine stores verbatim);
/// 2. `DeleteVariable` for every named element removed from the view whose
///    variable exists and names no remaining element; then an upsert for every
///    created named element (a new uid). A created element naming a variable
///    that exists, or a rename onto one, is a conflict: only reachable when name
///    allocation is wrong or the model moved underneath the edit;
/// 3. one `UpdateStockFlows` per stock whose lists the edit changes: per flow end
///    whose attached stock differs between base and next, the flow leaves the
///    old stock's list and joins the new stock's. Only flows and stocks that
///    exist after the edit are touched, and each operation carries both full
///    lists from the committed model with renames applied, deleted variables
///    omitted, and the deltas applied.
pub(crate) fn derived_operations(
    model: &datamodel::Model,
    base: &StockFlow,
    next: &StockFlow,
) -> Result<Vec<ModelOperation>> {
    let variables: HashMap<String, &Variable> = model
        .variables
        .iter()
        .map(|v| (canonicalize(v.get_ident()).into_owned(), v))
        .collect();
    let base_named = named_elements(base);
    let next_named = named_elements(next);
    let base_named_by_uid: HashMap<i32, &ViewElement> = base_named.iter().copied().collect();
    let next_named_by_uid: HashMap<i32, &ViewElement> = next_named.iter().copied().collect();
    let mut ops = Vec::new();

    // Renames: committed ident -> ident after the edit.
    let mut renamed: HashMap<String, String> = HashMap::new();
    for &(uid, b) in &base_named {
        let Some(n) = next_named_by_uid.get(&uid) else {
            continue;
        };
        let (Some(b_name), Some(n_name)) = (b.get_name(), n.get_name()) else {
            continue;
        };
        if b_name == n_name {
            continue;
        }
        let from = canonicalize(b_name).into_owned();
        let to = canonicalize(n_name).into_owned();
        if !variables.contains_key(&from) || renamed.contains_key(&from) {
            continue;
        }
        if to != from && variables.contains_key(&to) {
            return Err(conflict(format!(
                "cannot rename '{b_name}' to '{n_name}': a variable with that name exists"
            )));
        }
        renamed.insert(from.clone(), to);
        ops.push(ModelOperation::RenameVariable {
            from,
            to: n_name.to_string(),
        });
    }
    let after_rename = |ident: &str| {
        renamed
            .get(ident)
            .cloned()
            .unwrap_or_else(|| ident.to_string())
    };
    let before_rename: HashMap<&str, &str> = renamed
        .iter()
        .map(|(from, to)| (to.as_str(), from.as_str()))
        .collect();

    // Deletes.
    let remaining: HashSet<String> = next_named.iter().filter_map(|(_, e)| ident_of(e)).collect();
    let mut deleted: HashSet<String> = HashSet::new();
    for &(uid, b) in &base_named {
        if next_named_by_uid.contains_key(&uid) {
            continue;
        }
        let Some(ident) = ident_of(b) else {
            continue;
        };
        let Some(variable) = variables.get(&ident) else {
            continue;
        };
        if remaining.contains(&after_rename(&ident)) || deleted.contains(&ident) {
            continue;
        }
        deleted.insert(ident);
        ops.push(ModelOperation::DeleteVariable {
            ident: variable.get_ident().to_string(),
        });
    }

    // Creates.
    let mut created: HashMap<String, VariableKind> = HashMap::new();
    for &(uid, n) in &next_named {
        if base_named_by_uid.contains_key(&uid) {
            continue;
        }
        let (Some(ident), Some(kind)) = (ident_of(n), element_kind(n)) else {
            continue;
        };
        if created.contains_key(&ident) {
            continue;
        }
        // A created name that is a rename's TARGET collides with the renamed
        // variable; one that is a rename's SOURCE is free once the rename applies.
        let existing = match before_rename.get(ident.as_str()) {
            Some(from) => variables.get(*from),
            None if renamed.contains_key(&ident) => None,
            None => variables.get(&ident),
        };
        if let Some(existing) = existing
            && !deleted.contains(canonicalize(existing.get_ident()).as_ref())
        {
            return Err(conflict(format!(
                "cannot create '{}': a variable with that name exists",
                n.get_name().unwrap_or_default()
            )));
        }
        created.insert(ident, kind);
        ops.extend(create_operation(n));
    }

    // The kind of an after-edit ident once the renames, deletes and creates
    // above have applied, `None` when no variable holds it.
    let kind_after_edit = |ident: &str| -> Option<VariableKind> {
        if let Some(kind) = created.get(ident) {
            return Some(*kind);
        }
        let committed = before_rename.get(ident).copied().unwrap_or(ident);
        if deleted.contains(committed) {
            return None;
        }
        if !before_rename.contains_key(ident) && renamed.contains_key(ident) {
            return None;
        }
        variables.get(committed).map(|v| kind_of(v))
    };

    // Stock list deltas from flow attachment changes.
    let base_by_uid: HashMap<i32, &ViewElement> =
        base.elements.iter().map(|e| (e.get_uid(), e)).collect();
    let next_by_uid: HashMap<i32, &ViewElement> =
        next.elements.iter().map(|e| (e.get_uid(), e)).collect();
    let mut flow_uids: Vec<i32> = Vec::new();
    for element in base.elements.iter().chain(&next.elements) {
        if let ViewElement::Flow(f) = element
            && !flow_uids.contains(&f.uid)
        {
            flow_uids.push(f.uid);
        }
    }
    let mut deltas: HashMap<String, StockDelta> = HashMap::new();
    for uid in flow_uids {
        let base_flow = match base_by_uid.get(&uid) {
            Some(ViewElement::Flow(f)) => Some(f),
            _ => None,
        };
        let next_flow = match next_by_uid.get(&uid) {
            Some(ViewElement::Flow(f)) => Some(f),
            _ => None,
        };
        // A next element's name is already the after-edit spelling; only a flow
        // the edit removed is named through the base view and carried through
        // renames.
        let flow_ident = match (next_flow, base_flow) {
            (Some(n), _) => canonicalize(&n.name).into_owned(),
            (None, Some(b)) => after_rename(canonicalize(&b.name).as_ref()),
            (None, None) => continue,
        };
        if kind_after_edit(&flow_ident) != Some(VariableKind::Flow) {
            continue;
        }
        for end in FlowEnd::ALL {
            let from = attached_stock_ident(&base_by_uid, base_flow, end).map(|s| after_rename(&s));
            let to = attached_stock_ident(&next_by_uid, next_flow, end);
            if from == to {
                continue;
            }
            if let Some(from) = from
                && kind_after_edit(&from) == Some(VariableKind::Stock)
            {
                let delta = deltas.entry(from).or_default();
                let list = match end {
                    FlowEnd::Source => &mut delta.outflows,
                    FlowEnd::Sink => &mut delta.inflows,
                };
                list.remove.insert(flow_ident.clone());
            }
            if let Some(to) = to
                && kind_after_edit(&to) == Some(VariableKind::Stock)
            {
                let delta = deltas.entry(to).or_default();
                let list = match end {
                    FlowEnd::Source => &mut delta.outflows,
                    FlowEnd::Sink => &mut delta.inflows,
                };
                if !list.add.contains(&flow_ident) {
                    list.add.push(flow_ident.clone());
                }
            }
        }
    }
    let mut stock_idents: Vec<&String> = deltas.keys().collect();
    stock_idents.sort();
    for stock_ident in stock_idents {
        let delta = &deltas[stock_ident];
        let committed = before_rename
            .get(stock_ident.as_str())
            .copied()
            .unwrap_or(stock_ident);
        let committed_stock = if created.contains_key(stock_ident) {
            None
        } else {
            match variables.get(committed) {
                Some(Variable::Stock(s)) => Some(s),
                _ => None,
            }
        };
        let echo = |list: &[String]| -> Vec<String> {
            list.iter()
                .filter_map(|entry| {
                    let ident = canonicalize(entry).into_owned();
                    if deleted.contains(&ident) {
                        return None;
                    }
                    Some(
                        renamed
                            .get(&ident)
                            .cloned()
                            .unwrap_or_else(|| entry.clone()),
                    )
                })
                .collect()
        };
        let apply = |list: &[String], d: &ListDelta| -> Vec<String> {
            let mut kept: Vec<String> = list
                .iter()
                .filter(|entry| !d.remove.contains(canonicalize(entry).as_ref()))
                .cloned()
                .collect();
            let mut present: HashSet<String> =
                kept.iter().map(|e| canonicalize(e).into_owned()).collect();
            for flow in &d.add {
                if present.insert(flow.clone()) {
                    kept.push(flow.clone());
                }
            }
            kept
        };
        let (inflows_before, outflows_before) = match committed_stock {
            Some(s) => (echo(&s.inflows), echo(&s.outflows)),
            None => (Vec::new(), Vec::new()),
        };
        let inflows = apply(&inflows_before, &delta.inflows);
        let outflows = apply(&outflows_before, &delta.outflows);
        if inflows == inflows_before && outflows == outflows_before {
            continue;
        }
        ops.push(ModelOperation::UpdateStockFlows {
            ident: stock_ident.clone(),
            inflows,
            outflows,
        });
    }
    Ok(ops)
}

/// The view edit a delete implies:
///
/// - every selected element except a cloud whose flow is not also removed (a
///   cloud is a flow's endpoint, not a variable; removing it alone would leave
///   the flow's end dangling, so the request is ignored);
/// - the clouds of removed flows;
/// - the aliases of removed elements;
/// - every link touching a removed element;
/// - and every endpoint of a surviving flow attached to a removed element
///   becomes a new cloud at that endpoint.
///
/// `ModelOperation::EditView` derives the variable deletes and stock list
/// updates when the edit applies.
pub fn plan_delete(view: &StockFlow, selection: &[i32]) -> ViewEdit {
    let selected: HashSet<i32> = selection.iter().copied().collect();
    let mut removed: HashSet<i32> = view
        .elements
        .iter()
        .filter(|e| selected.contains(&e.get_uid()) && !matches!(e, ViewElement::Cloud(_)))
        .map(ViewElement::get_uid)
        .collect();
    for element in &view.elements {
        if let ViewElement::Cloud(c) = element
            && removed.contains(&c.flow_uid)
        {
            removed.insert(c.uid);
        }
    }
    for element in &view.elements {
        if let ViewElement::Alias(a) = element
            && removed.contains(&a.alias_of_uid)
        {
            removed.insert(a.uid);
        }
    }
    for element in &view.elements {
        if let ViewElement::Link(l) = element
            && (removed.contains(&l.from_uid) || removed.contains(&l.to_uid))
        {
            removed.insert(l.uid);
        }
    }
    let mut next_uid = view
        .elements
        .iter()
        .map(ViewElement::get_uid)
        .max()
        .unwrap_or(0)
        + 1;
    let mut upsert = Vec::new();
    let mut clouds = Vec::new();
    for element in &view.elements {
        let ViewElement::Flow(flow) = element else {
            continue;
        };
        if removed.contains(&flow.uid) {
            continue;
        }
        let mut next = flow.clone();
        let mut changed = false;
        for p in &mut next.points {
            if let Some(uid) = p.attached_to_uid
                && removed.contains(&uid)
            {
                let cloud = next_uid;
                next_uid += 1;
                clouds.push(ViewElement::Cloud(Cloud {
                    uid: cloud,
                    flow_uid: flow.uid,
                    x: p.x,
                    y: p.y,
                    compat: None,
                }));
                p.attached_to_uid = Some(cloud);
                changed = true;
            }
        }
        if changed {
            upsert.push(ViewElement::Flow(next));
        }
    }
    upsert.extend(clouds);
    ViewEdit {
        upsert,
        remove: view
            .elements
            .iter()
            .map(ViewElement::get_uid)
            .filter(|uid| removed.contains(uid))
            .collect(),
    }
}

/// The view edit a rename implies: every named element whose name is
/// `old_name` (compared canonically) relabeled `new_name`. The new name is the
/// typed name as given, with line breaks encoded to the stored two-character
/// escape but NOT canonicalized: the engine stores display spellings verbatim
/// and matches canonically, so canonicalizing would downgrade the display name.
/// `ModelOperation::EditView` derives the `RenameVariable` from the relabeled
/// element; a variable with no element on the view has nothing to relabel, and
/// is renamed with `RenameVariable` directly.
pub fn plan_rename(view: &StockFlow, old_name: &str, new_name: &str) -> ViewEdit {
    let encode = |name: &str| name.replace("\r\n", "\\n").replace('\n', "\\n");
    let old_ident = canonicalize(&encode(old_name)).into_owned();
    let new_name = encode(new_name);
    let upsert = view
        .elements
        .iter()
        .filter(|e| {
            is_named(e)
                && e.get_name()
                    .is_some_and(|n| canonicalize(n) == old_ident.as_str())
        })
        .map(|e| {
            let mut next = e.clone();
            match &mut next {
                ViewElement::Stock(s) => s.name = new_name.clone(),
                ViewElement::Flow(f) => f.name = new_name.clone(),
                ViewElement::Aux(a) => a.name = new_name.clone(),
                ViewElement::Module(m) => m.name = new_name.clone(),
                _ => {}
            }
            next
        })
        .collect();
    ViewEdit {
        upsert,
        remove: Vec::new(),
    }
}

#[cfg(test)]
#[path = "edit_view_tests.rs"]
mod tests;
