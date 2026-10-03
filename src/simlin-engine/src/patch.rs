// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};

use crate::ast::{Expr0, Expr2, IndexExpr0, IndexExpr2, print_eqn};
use crate::builtins::{BuiltinFn, Loc, UntypedBuiltinFn};
use crate::canonicalize;
use crate::common::{Canonical, Error, ErrorCode, ErrorKind, Ident, RawIdent, Result};
use crate::datamodel::{self, NameRole, Variable};
use crate::dimensions::DimensionsContext;
use crate::lexer::LexerType;

/// A patch to apply to a project. Contains project-level operations
/// (like changing sim specs or adding models) and per-model patches
/// (like upserting variables or views).
#[derive(Clone)]
pub struct ProjectPatch {
    pub project_ops: Vec<ProjectOperation>,
    pub models: Vec<ModelPatch>,
}

/// A project-level operation.
#[derive(Clone)]
pub enum ProjectOperation {
    /// Sets the sim specs the project runs under.
    ///
    /// A run takes its specs from the model it runs when that model has specs
    /// of its own, and from the project otherwise (XMILE 1.0 section 2.3: a
    /// model's `<sim_specs>` override the file's). So the edit sets the
    /// project's specs and takes the root model's own away: afterwards the
    /// project's are the one statement of what the root model runs under, and
    /// every reader of either agrees with the run. The root model is the one a
    /// host runs when it names none (`Project::default_model`). Another
    /// model's own specs are that model's and stay, so a run of another model
    /// that has its own still runs under them.
    SetSimSpecs(datamodel::SimSpecs),
    SetSource(datamodel::Source),
    AddModel {
        name: String,
    },
}

/// A patch targeting a specific model within the project.
#[derive(Clone)]
pub struct ModelPatch {
    pub name: String,
    pub ops: Vec<ModelOperation>,
}

/// An operation on a single model.
#[derive(Clone)]
pub enum ModelOperation {
    UpsertStock(datamodel::Stock),
    UpsertFlow(datamodel::Flow),
    UpsertAux(datamodel::Aux),
    UpsertModule(datamodel::Module),
    DeleteVariable {
        ident: String,
    },
    /// Renames a variable and respells every reference to it, in every model
    /// that can name it (its own and every model instantiating that one), and
    /// every other place a model holds its name, but for a diagram's labels:
    /// every view's label keeps naming the old name until a view edit
    /// (`EditView`, as `editing::plan_rename` plans one, or an upserted view)
    /// or a diagram sync that carries the patch relabels it. A host that
    /// draws the model sends the rename that way, or syncs its diagrams after
    /// it.
    ///
    /// Refused, changing nothing, where carrying it out would decide what an
    /// identifier inside a subscript's brackets names, which the compiler
    /// does not answer one way (`patch::Reader`): when the old or the new name
    /// is a dimension's, and when the variable is written alone in a
    /// subscript (or at an end of a range there) and its old or new name is an
    /// element of a dimension. Refused too when the new name is one of the
    /// run's clock slots (`db::is_implicit_global`: the time, the time step,
    /// the initial and the final time): a variable of that name takes the
    /// slot's results key, so a rename does not give a variable one, though
    /// the compiler resolves the quoted name; a model that already declares
    /// one keeps it.
    RenameVariable {
        from: String,
        to: String,
    },
    UpsertView {
        index: u32,
        view: datamodel::View,
    },
    DeleteView {
        index: u32,
    },
    UpdateStockFlows {
        ident: String,
        inflows: Vec<String>,
        outflows: Vec<String>,
    },
    SetLoopName {
        variables: Vec<String>,
        name: String,
        description: Option<String>,
    },
    /// Edit one stock-and-flow view by upserting elements (by uid: substituting
    /// or appending) and removing uids, and apply every model operation the
    /// edit implies -- renames, deletes, creates, stock list updates -- derived
    /// against the model as it is when this operation applies
    /// (`editing::derived_operations`). A diagram host describes an edit only by
    /// the elements it changed; the model follows.
    EditView {
        index: u32,
        upsert: Vec<datamodel::ViewElement>,
        remove: Vec<i32>,
    },
}

/// Whether applying the patch to `project` changes only views: no project-level
/// operations, and every model operation an `UpsertView`, a `DeleteView`, or an
/// `EditView` that implies no model operation (a moved element, a rerouted flow,
/// a curved link). Such a patch cannot change equations, variables, or
/// simulation, so callers can skip recompilation. Each `EditView` is judged
/// against the view the patch's earlier view operations leave, the view it will
/// actually edit; a patch that would fail to apply is not view-only, so the
/// caller's full path reports the failure.
pub fn is_view_only_patch(project: &datamodel::Project, patch: &ProjectPatch) -> bool {
    if !patch.project_ops.is_empty() {
        return false;
    }
    let mut views: HashMap<&str, Vec<datamodel::View>> = HashMap::new();
    for model_patch in &patch.models {
        let Some(model) = project.get_model(&model_patch.name) else {
            return false;
        };
        let working = views
            .entry(model_patch.name.as_str())
            .or_insert_with(|| model.views.clone());
        for op in &model_patch.ops {
            match op {
                ModelOperation::UpsertView { index, view } => {
                    let i = *index as usize;
                    if i < working.len() {
                        working[i] = view.clone();
                    } else if i == working.len() {
                        working.push(view.clone());
                    } else {
                        return false;
                    }
                }
                ModelOperation::DeleteView { index } => {
                    if (*index as usize) >= working.len() {
                        return false;
                    }
                    working.remove(*index as usize);
                }
                ModelOperation::EditView {
                    index,
                    upsert,
                    remove,
                } => {
                    let Some(datamodel::View::StockFlow(base)) = working.get(*index as usize)
                    else {
                        return false;
                    };
                    let next = crate::editing::edited_view(base, upsert, remove);
                    if !crate::editing::derived_operations(model, base, &next)
                        .is_ok_and(|ops| ops.is_empty())
                    {
                        return false;
                    }
                    working[*index as usize] = datamodel::View::StockFlow(next);
                }
                _ => return false,
            }
        }
    }
    true
}

pub fn apply_patch(project: &mut datamodel::Project, patch: ProjectPatch) -> Result<()> {
    let mut staged = project.clone();
    apply_patch_in_place(&mut staged, patch)?;
    *project = staged;
    Ok(())
}

/// Applies `patch` to `project` itself. A patch that fails leaves `project`
/// holding the operations before the one that failed, so a caller that must
/// keep the project as it was stages a copy, as [`apply_patch`] does.
pub(crate) fn apply_patch_in_place(
    project: &mut datamodel::Project,
    patch: ProjectPatch,
) -> Result<()> {
    // Apply project-level operations first
    for project_op in patch.project_ops {
        match project_op {
            ProjectOperation::SetSimSpecs(sim_specs) => {
                project.sim_specs = sim_specs;
                if let Some(root) = project.default_model_mut() {
                    root.sim_specs = None;
                }
            }
            ProjectOperation::SetSource(source) => {
                project.source = Some(source);
            }
            ProjectOperation::AddModel { name } => {
                apply_add_model(project, name)?;
            }
        }
    }

    // Then apply model-level operations
    for model_patch in patch.models {
        for op in model_patch.ops {
            apply_model_operation(project, &model_patch.name, op)?;
        }
    }

    Ok(())
}

fn apply_model_operation(
    project: &mut datamodel::Project,
    model_name: &str,
    op: ModelOperation,
) -> Result<()> {
    // Renames reach every model's equations through the project, and a view
    // edit applies renames of its own, so both take the project.
    if let ModelOperation::RenameVariable { from, to } = &op {
        return apply_rename_variable(project, model_name, from, to);
    }
    if let ModelOperation::EditView {
        index,
        upsert,
        remove,
    } = &op
    {
        return apply_edit_view(project, model_name, *index, upsert, remove);
    }
    let model = get_model_mut(project, model_name)?;
    match op {
        ModelOperation::UpsertStock(mut stock) => {
            canonicalize_stock_references(&mut stock);
            upsert_variable(model, Variable::Stock(stock));
        }
        ModelOperation::UpsertFlow(flow) => {
            upsert_variable(model, Variable::Flow(flow));
        }
        ModelOperation::UpsertAux(aux) => {
            upsert_variable(model, Variable::Aux(aux));
        }
        ModelOperation::UpsertModule(mut module) => {
            canonicalize_module_references(&mut module);
            upsert_variable(model, Variable::Module(module));
        }
        ModelOperation::DeleteVariable { ident } => {
            apply_delete_variable(model, &ident)?;
        }
        ModelOperation::UpsertView { index, view } => {
            apply_upsert_view(model, index, view)?;
        }
        ModelOperation::DeleteView { index } => {
            apply_delete_view(model, index)?;
        }
        ModelOperation::UpdateStockFlows {
            ident,
            inflows,
            outflows,
        } => {
            apply_update_stock_flows(model, &ident, &inflows, &outflows)?;
        }
        ModelOperation::SetLoopName {
            variables,
            name,
            description,
        } => {
            apply_set_loop_name(model, variables, name, description)?;
        }
        ModelOperation::RenameVariable { .. } | ModelOperation::EditView { .. } => {
            unreachable!("applied through the project above")
        }
    }
    Ok(())
}

/// Apply a view edit: the model operations the edit implies, derived against
/// the model as it is now, then the edited view in place of the old one.
fn apply_edit_view(
    project: &mut datamodel::Project,
    model_name: &str,
    index: u32,
    upsert: &[datamodel::ViewElement],
    remove: &[i32],
) -> Result<()> {
    let model = get_model_mut(project, model_name)?;
    let Some(datamodel::View::StockFlow(base)) = model.views.get(index as usize) else {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DoesNotExist,
            Some(format!("view index {index} out of range")),
        ));
    };
    let next = crate::editing::edited_view(base, upsert, remove);
    let ops = crate::editing::derived_operations(model, base, &next)?;
    for op in ops {
        apply_model_operation(project, model_name, op)?;
    }
    let model = get_model_mut(project, model_name)?;
    model.views[index as usize] = datamodel::View::StockFlow(next);
    Ok(())
}

fn canonicalize_ident(ident: &mut String) {
    *ident = canonicalize(ident.as_str()).into_owned();
}

// The variable's own `ident` is deliberately NOT canonicalized on upsert:
// datamodel ident fields hold the human-facing display name (casing, spaces,
// XMILE `\n` line breaks -- see the module comment in serde.rs and the XMILE
// reader, which stores `name` attributes verbatim), and every consumer
// canonicalizes at lookup time (`Model::get_variable`, `db/sync.rs`,
// `layout/mod.rs`, ...). Canonicalizing in place destroyed the display name
// on every upsert (GH #890). REFERENCE lists (stock inflows/outflows, module
// `src`/`dst`) ARE canonicalized, mirroring the XMILE reader's convention for
// those fields.

fn canonicalize_stock_references(stock: &mut datamodel::Stock) {
    stock.inflows = canonical_flow_list(&stock.inflows);
    stock.outflows = canonical_flow_list(&stock.outflows);
}

/// The stored form of a stock's inflow or outflow list, for every op that sets
/// one: the set the engine integrates (`datamodel::distinct_stock_flows`, so
/// `"Flow A"` and `"flow_a"` are one member), as canonical idents, in the
/// order given. Storing the set keeps a patch from writing a repeat the sync
/// would then have to warn about.
///
/// The order is the modeler's and is kept: XMILE 1.0 section 4.2
/// (`docs/reference/xmile-v1.0.html`, "Stocks") says inflows "appear with
/// multiple tags in inflow-priority order (if the order of inflow to the stock
/// is important)", and that a queue's "outflows have a priority order and MAY
/// have the `<overflow/>` option set on all but the first outflow". The engine
/// reads both: a queue serves its outflows in list order
/// (`docs/design/queues.md` section 3.3), and a conveyor admits its coupled
/// inflows in list order.
fn canonical_flow_list(flows: &[String]) -> Vec<String> {
    datamodel::distinct_stock_flows(flows)
        .flows
        .iter()
        .map(|flow| canonicalize(flow).into_owned())
        .collect()
}

fn canonicalize_module_references(module: &mut datamodel::Module) {
    // Canonicalize the reference endpoints, mirroring
    // `canonicalize_stock_references`. `src`/`dst` are variable idents (`dst`
    // is the module-qualified `module·port` form); leaving them verbatim lets
    // a non-canonical `src`/`dst` arriving via the FFI `apply_patch`
    // (pysimlin `upsert_module`) disagree with the canonical idents every
    // UI/engine consumer compares against. Empty placeholder endpoints
    // canonicalize to empty and are preserved.
    for reference in module.references.iter_mut() {
        canonicalize_ident(&mut reference.src);
        canonicalize_ident(&mut reference.dst);
    }
}

fn get_uid(var: &Variable) -> Option<i32> {
    variable_uid(var)
}

/// The datamodel UID of a variable, or `None` if it has not been assigned
/// one yet. Pinned-loop sync (`db::sync`) reads this to resolve a
/// `LoopMetadata`'s UID references back to canonical variable names.
pub fn variable_uid(var: &Variable) -> Option<i32> {
    match var {
        Variable::Stock(s) => s.uid,
        Variable::Flow(f) => f.uid,
        Variable::Aux(a) => a.uid,
        Variable::Module(m) => m.uid,
    }
}

fn set_uid(var: &mut Variable, uid: Option<i32>) {
    match var {
        Variable::Stock(s) => s.uid = uid,
        Variable::Flow(f) => f.uid = uid,
        Variable::Aux(a) => a.uid = uid,
        Variable::Module(m) => m.uid = uid,
    }
}

/// The next UID safe to assign in `model`: one past the maximum over both
/// variable UIDs and view element UIDs.  Considering view UIDs too avoids
/// collisions when variables lack UIDs but views already carry them (view
/// elements reference variables by UID).
fn next_available_uid(model: &datamodel::Model) -> i32 {
    let max_var_uid = model
        .variables
        .iter()
        .filter_map(get_uid)
        .max()
        .unwrap_or(0);
    let max_view_uid = model
        .views
        .iter()
        .flat_map(|v| match v {
            datamodel::View::StockFlow(sf) => sf.elements.iter(),
        })
        .map(|e| e.get_uid())
        .max()
        .unwrap_or(0);
    max_var_uid.max(max_view_uid) + 1
}

fn upsert_variable(model: &mut datamodel::Model, mut variable: Variable) {
    let ident = canonicalize(variable.get_ident());
    if let Some(index) = model.variable_index(&ident) {
        // View elements reference variables by UID, so preserve the existing
        // UID when the replacement doesn't specify one.
        if get_uid(&variable).is_none() {
            set_uid(&mut variable, get_uid(&model.variables[index]));
        }
        // An upsert of the variable as it is changes nothing, so the model
        // keeps the allocation it has and a copy of the project keeps sharing
        // it.
        if model.variables[index] != variable {
            model.variables.replace(index, variable);
        }
    } else {
        // New variables created via patch (e.g., from MCP EditModel) may arrive
        // without a UID. Assign one so that SetLoopName can later reference them
        // by UID.
        if get_uid(&variable).is_none() {
            set_uid(&mut variable, Some(next_available_uid(model)));
        }
        model.variables.push(variable);
    }
}

/// The `DoesNotExist` an operation on a variable the model does not hold
/// fails with, naming the variable as the operation spelled it.
fn no_such_variable(ident: &str) -> Error {
    Error::new(
        ErrorKind::Model,
        ErrorCode::DoesNotExist,
        Some(format!("there is no variable named '{ident}'")),
    )
}

fn get_model_mut<'a>(
    project: &'a mut datamodel::Project,
    model_name: &str,
) -> Result<&'a mut datamodel::Model> {
    project.get_model_mut(model_name).ok_or_else(|| {
        Error::new(
            ErrorKind::Model,
            ErrorCode::BadModelName,
            Some(format!("there is no model named '{model_name}'")),
        )
    })
}

fn apply_add_model(project: &mut datamodel::Project, name: String) -> Result<()> {
    // A model's name is stored as written, and a patch addresses it so, but
    // the engine knows every model by its canonical name, the empty name
    // being `main`: a project with two models of one such name is one it
    // refuses to compile (`db::diagnostic::project_duplicate_models`). So a
    // name is taken when the name it is known by is.
    let known_as = datamodel::canonical_model_name(&name);
    if let Some(taken) = project
        .models
        .iter()
        .find(|model| datamodel::canonical_model_name(&model.name) == known_as)
    {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DuplicateVariable,
            Some(if taken.name == name {
                format!("model '{name}' already exists")
            } else {
                format!(
                    "model '{name}' already exists as '{}' (model names are case-, \
                     whitespace-, and underscore-insensitive, and an unnamed model is \
                     'main')",
                    taken.name
                )
            }),
        ));
    }
    // The stdlib's models are named under a prefix of their own, and a
    // project model of such a name stands in the stdlib model's place
    // (`db::sync`): an empty one would answer every call of that builtin. The
    // one way a project holds a stdlib model is the stdlib's own definition,
    // which `Project::ensure_referenced_stdlib_models` adds.
    if known_as.starts_with(datamodel::STDLIB_PREFIX) {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::BadModelName,
            Some(format!(
                "cannot add model '{name}': names beginning '{}' are the stdlib's",
                datamodel::STDLIB_PREFIX
            )),
        ));
    }
    project.models.push(datamodel::Model {
        name,
        sim_specs: None,
        variables: vec![].into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    Ok(())
}

fn apply_update_stock_flows(
    model: &mut datamodel::Model,
    ident_str: &str,
    inflows: &[String],
    outflows: &[String],
) -> Result<()> {
    let ident = canonicalize(ident_str);

    let index = model
        .variables
        .iter()
        .position(
            |var| matches!(var, Variable::Stock(stock) if canonicalize(stock.ident.as_str()) == ident),
        )
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Model,
                ErrorCode::DoesNotExist,
                Some(format!("stock '{}' not found", ident_str)),
            )
        })?;

    let inflows = canonical_flow_list(inflows);
    let outflows = canonical_flow_list(outflows);
    // Lists the stock already holds leave it as it is, in the allocation a
    // copy of the project shares.
    let holds_them = matches!(&model.variables[index], Variable::Stock(stock)
        if stock.inflows == inflows && stock.outflows == outflows);
    if !holds_them && let Some(Variable::Stock(stock)) = model.variables.get_mut(index) {
        stock.inflows = inflows;
        stock.outflows = outflows;
    }

    Ok(())
}

fn apply_set_loop_name(
    model: &mut datamodel::Model,
    variables: Vec<String>,
    name: String,
    description: Option<String>,
) -> Result<()> {
    if variables.is_empty() {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::Generic,
            Some("SetLoopName requires at least one variable".to_owned()),
        ));
    }

    // ReadModel returns loops with the first variable repeated at the end to
    // close the cycle (e.g., ["a", "b", "a"]). Deduplicate before resolving
    // UIDs so that a client passing the ReadModel output directly doesn't
    // produce duplicate entries in the sorted UID list.
    let mut seen = HashSet::new();
    let unique_vars: Vec<&String> = variables
        .iter()
        .filter(|v| seen.insert(v.as_str()))
        .collect();

    // Resolve each variable's UID, minting fresh ones for variables that lack
    // them.  Vensim/MDL- and SD-AI-imported models carry no variable UIDs at
    // all, and pinning a loop is exactly the operation that needs them -- so
    // assign on demand rather than failing, mirroring `upsert_variable`'s
    // precedent for patch-created variables.
    let mut next_uid = next_available_uid(model);
    let mut uids: Vec<i32> = Vec::with_capacity(unique_vars.len());
    for var_name in &unique_vars {
        let index = model.variable_index(var_name).ok_or_else(|| {
            Error::new(
                ErrorKind::Model,
                ErrorCode::DoesNotExist,
                Some(format!("variable '{}' not found", var_name)),
            )
        })?;
        // Only a variable that takes a new uid is written: one that has a uid
        // stays in the allocation a copy of the project shares.
        let uid = match get_uid(&model.variables[index]) {
            Some(uid) => uid,
            None => {
                let minted = next_uid;
                if let Some(var) = model.variables.get_mut(index) {
                    set_uid(var, Some(minted));
                }
                next_uid += 1;
                minted
            }
        };
        uids.push(uid);
    }
    uids.sort();

    if let Some(existing) = model.loop_metadata.iter_mut().find(|lm| {
        let mut existing_uids = lm.uids.clone();
        existing_uids.sort();
        existing_uids == uids
    }) {
        existing.name = name;
        existing.description = description.unwrap_or_default();
        // SetLoopName means "name/pin this loop", so revive a previously
        // soft-deleted entry -- otherwise the consumers that filter out deleted
        // entries (pinned-loop scoring, loop-name display) would silently ignore
        // the re-pin.
        existing.deleted = false;
    } else {
        model.loop_metadata.push(datamodel::LoopMetadata {
            uids,
            deleted: false,
            name,
            description: description.unwrap_or_default(),
        });
    }
    Ok(())
}

fn apply_delete_variable(model: &mut datamodel::Model, ident_str: &str) -> Result<()> {
    let ident = canonicalize(ident_str);
    let Some(pos) = model.variable_index(ident_str) else {
        return Err(no_such_variable(ident_str));
    };

    // The instances a reference can hop through, the deleted variable among
    // them when it is one.
    let instances: HashSet<String> = model
        .variables
        .iter()
        .filter_map(|var| match var {
            Variable::Module(module) => Some(canonicalize(&module.ident).into_owned()),
            Variable::Stock(_) | Variable::Flow(_) | Variable::Aux(_) => None,
        })
        .collect();

    let removed = model.variables.remove(pos);
    if let Variable::Flow(flow) = removed {
        let flow_ident = canonicalize(flow.ident.as_str());
        let names_flow = |name: &String| canonicalize(name.as_str()) == flow_ident;
        model.variables.edit_where(
            |var| {
                matches!(var, Variable::Stock(stock)
                    if stock.inflows.iter().chain(&stock.outflows).any(names_flow))
            },
            |var| {
                if let Variable::Stock(stock) = var {
                    stock.inflows.retain(|name| !names_flow(name));
                    stock.outflows.retain(|name| !names_flow(name));
                }
            },
        );
    }

    // Drop the module wiring that runs through the deleted variable: a
    // left-behind `src` is a dependency on a variable the model no longer has,
    // and the whole project stops compiling with a "missing variable" message;
    // a left-behind `dst` is an input written into an instance that is gone.
    // An end runs through the variable in any spelling whose path starts at
    // it: the bare name, XMILE's parent-scope `·name` (the form the XMILE
    // reader stores), `self·name`, and a port of a deleted module instance
    // (`name·output`, `name·input`). A path starts at its first segment only
    // when that segment is a module instance; otherwise the whole spelling is
    // one local name, as `db::DepScope::resolve` reads it, so `x·foo` beside
    // an auxiliary `x` starts at the variable named `x.foo`, never at `x`.
    let starts_at_deleted = |end: &str| {
        let end = canonicalize(end);
        let (_, path) = split_scope_prefix(&end);
        let start = match path.split_once(MODULE_SEPARATOR) {
            Some((head, _)) if instances.contains(head) => head,
            Some(_) | None => path,
        };
        start == ident.as_ref()
    };
    let runs_through_deleted = |reference: &datamodel::ModuleReference| {
        starts_at_deleted(&reference.src) || starts_at_deleted(&reference.dst)
    };
    model.variables.edit_where(
        |var| matches!(var, Variable::Module(module) if module.references.iter().any(runs_through_deleted)),
        |var| {
            if let Variable::Module(module) = var {
                module
                    .references
                    .retain(|reference| !runs_through_deleted(reference));
            }
        },
    );

    for group in model.groups.iter_mut() {
        group
            .members
            .retain(|name| canonicalize(name.as_str()) != ident);
    }

    Ok(())
}

/// The separator between a module instance and what is read through it in a
/// canonical identifier (`hares·births`).
const MODULE_SEPARATOR: char = '\u{00B7}';

/// A canonical reference as it is written in a model, split into its scope
/// prefix and the path it walks.
///
/// The prefix names no variable: `self·` is the model itself (a module's port
/// is spelled `self·port`), and a bare `·` is XMILE's parent-scope spelling
/// `.x`, which the reader stores canonicalized and every consumer reads as the
/// bare name (`db::DepScope::resolve`). The path's first segment is a variable
/// of the model, and each later one a variable of the model the segment before
/// it instantiates.
fn split_scope_prefix(reference: &str) -> (&str, &str) {
    const SELF_PREFIX: &str = "self\u{00B7}";
    if reference.starts_with(SELF_PREFIX) {
        return reference.split_at(SELF_PREFIX.len());
    }
    if reference.starts_with(MODULE_SEPARATOR) {
        return reference.split_at(MODULE_SEPARATOR.len_utf8());
    }
    ("", reference)
}

fn apply_rename_variable(
    project: &mut datamodel::Project,
    model_name: &str,
    from: &str,
    to: &str,
) -> Result<()> {
    let old_ident = Ident::new(from);
    let new_ident = Ident::new(to);

    // Refuse a target name that has NO spelling in the equation language.
    //
    // `Lexer::quoted_identifier` terminates on the first `"` and the grammar has
    // no escape of any kind, so a canonical name containing `"` can be written
    // neither bare nor quoted. Renaming TO one is not merely useless: this
    // function respells every reference, so it would persist `c = "x"y" + 1`
    // into the datamodel and the previously-valid model would stop compiling
    // with `UnclosedQuotedIdent` -- the same silent, saved corruption GH #976
    // fixed for keyword names, through this same entry point.
    //
    // Rejecting at the front door rather than teaching the lexer an escape is
    // deliberate: nothing is lost by refusing a name that nothing could ever
    // reference, and the alternative is a grammar change. Note the check is on
    // `to` only -- renaming AWAY from such a name is how a model that already
    // has one gets repaired. The error code is the one recompiling would have
    // produced, so the rejection and the failure it prevents read alike.
    if new_ident.as_str().contains('"') {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::UnclosedQuotedIdent,
            Some(format!(
                "cannot rename to `{to}`: a name containing a double quote cannot be \
                 referenced in an equation"
            )),
        ));
    }

    let model_index = project.model_index(model_name).ok_or_else(|| {
        Error::new(
            ErrorKind::Model,
            ErrorCode::BadModelName,
            Some(format!("there is no model named '{model_name}'")),
        )
    })?;

    if old_ident == new_ident {
        // Canonically-identical rename: only the display spelling changes
        // (e.g. "students" -> "Students"). Every reference resolves through
        // canonicalization, so no equation or reference rewrites are needed --
        // just restamp the stored display name.
        let model = &mut project.models[model_index];
        let index = model
            .variable_index(from)
            .ok_or_else(|| no_such_variable(from))?;
        if model.variables[index].get_ident() != to
            && let Some(var) = model.variables.get_mut(index)
        {
            var.set_ident(to.to_string());
        }
        return Ok(());
    }

    // A rename does not give a variable the name of one of the run's clock
    // slots -- the time, the time step, the initial and the final time
    // (`db::is_implicit_global`); a model that already declares one keeps it.
    // The compiler resolves such a name written quoted, so the rule is
    // stricter than it: a variable of that name takes the slot's key in the
    // results, where every host reads the clock. Another builtin's name
    // (`pi`, `if`) is not refused; the printer quotes it.
    if crate::db::is_implicit_global(new_ident.as_str()) {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DuplicateVariable,
            Some(format!(
                "cannot rename '{from}' to '{to}': '{to}' is one of the simulation's own \
                 clock names (time, the time step, the initial and the final time), which a \
                 rename does not give a variable"
            )),
        ));
    }

    let model = &project.models[model_index];
    if model.get_variable(new_ident.as_str()).is_some() {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DuplicateVariable,
            Some(format!(
                "cannot rename '{from}' to '{to}': a variable named '{to}' already exists"
            )),
        ));
    }
    if model.get_variable(old_ident.as_str()).is_none() {
        return Err(no_such_variable(from));
    }

    let rename = Rename::new(project, model_index, &old_ident, &new_ident);
    // A name a dimension has, before the rename or after it, can be the
    // dimension or the variable wherever an equation writes it, and the
    // compiler reads it both ways (`Reader`). XMILE 1.0 section 3.7.1 says
    // dimension names "must be distinct from model variables names".
    let shared_with_dimension = |name: &str| {
        Error::new(
            ErrorKind::Model,
            ErrorCode::DuplicateVariable,
            Some(format!(
                "cannot rename '{from}' to '{to}': '{name}' is also the name of a dimension, \
                 so an equation that writes it may mean the dimension or the variable, and the \
                 rename cannot tell which (XMILE requires a variable's name to differ from \
                 every dimension's)"
            )),
        )
    };
    if rename.dimensions.is_dimension_name(old_ident.as_str()) {
        return Err(shared_with_dimension(from));
    }
    if rename.dimensions.is_dimension_name(new_ident.as_str()) {
        return Err(shared_with_dimension(to));
    }

    // A reference reaches the renamed variable from its own model and from
    // every model that instantiates that model, however deep; no other model
    // can name it. Every text is respelled before any is written, so a
    // rename refused for an ambiguous reference in one model leaves every
    // model as it was.
    let reaches = rename.models_reaching();
    let mut respelled: Vec<(usize, Vec<(usize, Variable)>)> = Vec::new();
    for (index, model) in project.models.iter().enumerate() {
        if !reaches[index] {
            continue;
        }
        let mut variables = Vec::new();
        for (position, var) in model.variables.iter().enumerate() {
            match rename.renamed_texts(index, var) {
                Ok(Some(renamed)) => variables.push((position, renamed)),
                Ok(None) => {}
                Err(Ambiguity(why)) => {
                    return Err(Error::new(
                        ErrorKind::Model,
                        ErrorCode::DuplicateVariable,
                        Some(format!(
                            "cannot rename '{from}' to '{to}': the equation of '{}' in model \
                             '{}' {why}",
                            var.get_ident(),
                            model.name
                        )),
                    ));
                }
            }
        }
        respelled.push((index, variables));
    }
    for (index, variables) in respelled {
        let model = &mut project.models[index];
        for (position, renamed) in variables {
            model.variables.replace(position, renamed);
        }
        model.map_variable_names(|role, name| rename.renamed_name(index, role, name, to));
    }

    Ok(())
}

/// The variables whose expression texts name the variable `name` (canonical)
/// of the model `model_name`: in that model, and through module instances in
/// every model that reaches it, found as a rename finds what it respells
/// (`Rename`, over `Variable::expression_texts`), each as `(model, variable)`
/// by its display spelling. A text that names it where a rename could not
/// tell whether it is a reference counts as naming it. The variable itself,
/// when the model has it, is left out. What a delete or a rename leaves
/// dangling is what this finds after it: the one owner of "who still names a
/// variable", so a reference the compiler's report does not show (an
/// equation that failed already, a cycle hidden behind another) is found as
/// the rename would find it.
#[cfg(feature = "agent_tools")]
pub(crate) fn variables_naming(
    project: &datamodel::Project,
    model_name: &str,
    name: &str,
) -> Vec<(String, String)> {
    let Some(model_index) = project.model_index(model_name) else {
        return Vec::new();
    };
    let old = Ident::<Canonical>::new(name);
    let rename = Rename::new(project, model_index, &old, &old);
    let reaches = rename.models_reaching();
    let mut naming = Vec::new();
    for (index, model) in project.models.iter().enumerate() {
        if !reaches[index] {
            continue;
        }
        for var in model.variables.iter() {
            if index == model_index && canonicalize(var.get_ident()) == old.as_str() {
                continue;
            }
            // Renaming the variable to its own name respells every reference
            // to it, so a text it gives a respelling, or finds ambiguous,
            // names it.
            if !matches!(rename.renamed_texts(index, var), Ok(None)) {
                naming.push((model.name.clone(), var.get_ident().to_string()));
            }
        }
    }
    naming
}

/// What a module instance instantiates.
enum Instantiates {
    /// A model of the project, by its index in `project.models`.
    Model(usize),
    /// A stdlib model, which the db holds for every project and no patch
    /// reaches.
    Stdlib,
    /// A model nothing holds: a read through the instance resolves to nothing.
    Nothing,
}

/// Where a reference leads from the model it is written in.
enum Target<'r> {
    /// One name of the model the reference is written in.
    Local(&'r str),
    /// A variable read through module instances.
    Through {
        /// Each instance on the way, with the model it is a variable of.
        instances: Vec<(usize, &'r str)>,
        /// The model the last instance instantiates; `None` for a stdlib
        /// model.
        model: Option<usize>,
        variable: &'r str,
    },
}

/// How a spelling is read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ReadAs {
    /// As a variable: a reference in an equation, a module reference's `src`,
    /// a stock's flow. It is resolved as `db::DepScope::resolve` resolves it,
    /// so a hop through an instance of a model nothing holds fails, and the
    /// spelling is one local name.
    Variable,
    /// As a module reference's `dst`: a port, through the instance the
    /// spelling starts at (`db::assemble::port_of`), whatever model that
    /// instance instantiates, so the wiring follows a renamed instance even
    /// before its model is in the project.
    Port,
}

/// Where in an expression a reference is written, which decides what the
/// compiler can read it as.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Site {
    /// An identifier anywhere but alone in a subscript's brackets: an
    /// operand, a call's argument, a part of an index expression.
    Value,
    /// The variable a subscript indexes.
    Subscripted,
    /// Alone in a subscript's brackets, or an end of a range there.
    Index,
}

/// A reference a rename would have to respell without knowing what the
/// compiler reads it as, and why: a phrase completing "the equation of
/// `reader` ...".
struct Ambiguity(String);

/// What a rename makes of a reference in an expression, by where it is
/// written: the name it reads after the rename, `None` for one the rename
/// leaves, or the ambiguity that refuses the rename.
type Respelled = std::result::Result<Option<Ident<Canonical>>, Ambiguity>;

/// What a rename makes of each reference in an expression ([`Respelled`]).
type Respelling<'f> = dyn Fn(Site, &Ident<Canonical>) -> Respelled + 'f;

/// One rename, and what each name of the renamed variable reads after it.
///
/// A reference names the variable from the model it is written in: bare in
/// the variable's own model (or under a scope prefix, `split_scope_prefix`),
/// and through a chain of module instances from a model that instantiates it
/// (`hares·births` in the model holding the instance `hares`). The chain is
/// resolved the way the compiler resolves it (`Rename::target`). One rule
/// therefore respells an equation's references, a module's `src` and `dst`,
/// and a stock's flow lists, in every model that reaches the renamed variable
/// -- a renamed input port in its parents' `dst`, a renamed output in its
/// parents' equations and `src`, and a renamed module instance in everything
/// read or wired through it.
///
/// Inside an equation a name spelled like the variable is not always a
/// reference to it, and where the rename cannot tell, it refuses
/// (`Reader`).
struct Rename<'a> {
    /// The renamed variable's model, by its index in `project.models`.
    model: usize,
    old: &'a Ident<Canonical>,
    new: &'a Ident<Canonical>,
    /// For each model, by its index in `project.models`, the module
    /// instances a reference can hop through, by canonical ident, with what
    /// each instantiates, as the project declares them before the rename.
    instances: Vec<HashMap<String, Instantiates>>,
    dimensions: DimensionsContext,
    /// The project's dimensions by name, in declaration order, so the
    /// dimension a refusal names is the same on every run.
    dimension_names: Vec<String>,
}

impl<'a> Rename<'a> {
    fn new(
        project: &datamodel::Project,
        model: usize,
        old: &'a Ident<Canonical>,
        new: &'a Ident<Canonical>,
    ) -> Self {
        let dimensions = DimensionsContext::from(&project.dimensions);
        // Of two models whose names canonicalize alike (a project the engine
        // refuses to compile) a name is the later's, the one the db files
        // under it.
        let models: HashMap<String, usize> = project
            .models
            .iter()
            .enumerate()
            .map(|(index, model)| (canonicalize(&model.name).into_owned(), index))
            .collect();
        let instantiates = |model_name: &str| {
            let name = canonicalize(model_name);
            match models.get(name.as_ref()) {
                Some(&index) => Instantiates::Model(index),
                None if crate::stdlib::MODEL_NAMES
                    .iter()
                    .any(|stdlib| name.strip_prefix(datamodel::STDLIB_PREFIX) == Some(*stdlib)) =>
                {
                    Instantiates::Stdlib
                }
                None => Instantiates::Nothing,
            }
        };
        let instances = project
            .models
            .iter()
            .map(|model| {
                model
                    .variables
                    .iter()
                    .filter_map(|var| match var {
                        Variable::Module(module) => Some((
                            canonicalize(&module.ident).into_owned(),
                            instantiates(&module.model_name),
                        )),
                        Variable::Stock(_) | Variable::Flow(_) | Variable::Aux(_) => None,
                    })
                    .collect()
            })
            .collect();
        Rename {
            model,
            old,
            new,
            instances,
            dimensions,
            dimension_names: project
                .dimensions
                .iter()
                .map(|dimension| dimension.name().to_string())
                .collect(),
        }
    }

    /// The first dimension of the project, in declaration order, that has an
    /// element named `name`: the membership test the compiler resolves a
    /// subscript's index against (`Dimension::canonical_element`).
    fn dimension_with_element(&self, name: &str) -> Option<&str> {
        self.dimension_names
            .iter()
            .find(|dimension| {
                self.dimensions
                    .get_by_raw_name(dimension)
                    .is_some_and(|dimension| dimension.canonical_element(name).is_some())
            })
            .map(String::as_str)
    }

    /// Which models, by index, can name the renamed variable: its own, and
    /// every model with an instance of a model that can.
    fn models_reaching(&self) -> Vec<bool> {
        let mut reaches = vec![false; self.instances.len()];
        reaches[self.model] = true;
        loop {
            let mut grew = false;
            for (from, instances) in self.instances.iter().enumerate() {
                let instantiates_one = || {
                    instances
                        .values()
                        .any(|instance| matches!(instance, Instantiates::Model(to) if reaches[*to]))
                };
                if !reaches[from] && instantiates_one() {
                    reaches[from] = true;
                    grew = true;
                }
            }
            if !grew {
                return reaches;
            }
        }
    }

    /// Where `path`, a canonical spelling written in the model at index
    /// `model` with its scope prefix taken off, leads when read as `read_as`
    /// says. Read as a variable it is `db::DepScope::resolve` on the
    /// datamodel: each segment but the last must be a module instance of the
    /// model the segment before it instantiates, and the last is a variable
    /// of the model the last instance instantiates. A spelling that fails a
    /// hop is one local name, the whole of it: `x·foo` beside an auxiliary
    /// `x` reads the variable named `x.foo`, never `x`. Read as a port, an
    /// instance of a model nothing holds is a hop too, to a model no rename
    /// reaches.
    fn target<'r>(&self, model: usize, path: &'r str, read_as: ReadAs) -> Target<'r> {
        let mut instances = Vec::new();
        let (mut at, mut rest) = (model, path);
        while let Some((segment, tail)) = rest.split_once(MODULE_SEPARATOR) {
            match self.instances[at].get(segment) {
                Some(Instantiates::Model(next)) => {
                    instances.push((at, segment));
                    (at, rest) = (*next, tail);
                }
                Some(Instantiates::Stdlib) => {
                    instances.push((at, segment));
                    return Target::Through {
                        instances,
                        model: None,
                        variable: tail,
                    };
                }
                Some(Instantiates::Nothing) if read_as == ReadAs::Port => {
                    instances.push((at, segment));
                    return Target::Through {
                        instances,
                        model: None,
                        variable: tail,
                    };
                }
                Some(Instantiates::Nothing) | None => {
                    return Target::Local(path);
                }
            }
        }
        if instances.is_empty() {
            Target::Local(path)
        } else {
            Target::Through {
                instances,
                model: Some(at),
                variable: rest,
            }
        }
    }

    /// What `reference`, written in the model at index `model` and read as
    /// `read_as` says, reads after the rename, or `None` when it does not
    /// name the renamed variable.
    fn renamed(
        &self,
        model: usize,
        reference: &Ident<Canonical>,
        read_as: ReadAs,
    ) -> Option<Ident<Canonical>> {
        let (prefix, path) = split_scope_prefix(reference.as_str());
        let is_renamed = |at: usize, name: &str| at == self.model && name == self.old.as_str();
        let mut renamed = String::from(prefix);
        match self.target(model, path, read_as) {
            Target::Local(name) => {
                if !is_renamed(model, name) {
                    return None;
                }
                renamed.push_str(self.new.as_str());
            }
            Target::Through {
                instances,
                model: holder,
                variable,
            } => {
                let renames_variable = holder.is_some_and(|holder| is_renamed(holder, variable));
                if !renames_variable && !instances.iter().any(|(at, name)| is_renamed(*at, name)) {
                    return None;
                }
                for (at, name) in instances {
                    renamed.push_str(if is_renamed(at, name) {
                        self.new.as_str()
                    } else {
                        name
                    });
                    renamed.push(MODULE_SEPARATOR);
                }
                renamed.push_str(if renames_variable {
                    self.new.as_str()
                } else {
                    variable
                });
            }
        }
        Some(Ident::from_unchecked(renamed))
    }

    /// `var`, a variable of the model at index `model`, with every reference
    /// to the renamed variable in its expression texts respelled
    /// (`Variable::expression_texts`), or `None` when there is none; or the
    /// first ambiguity that refuses the rename.
    fn renamed_texts(
        &self,
        model: usize,
        var: &Variable,
    ) -> std::result::Result<Option<Variable>, Ambiguity> {
        let reader = Reader {
            rename: self,
            model,
        };
        let mut ambiguity = None;
        let renamed = var.map_expression_texts(|_, text| {
            match renamed_text(text, &|site, reference| reader.renamed(site, reference)) {
                Ok(renamed) => renamed,
                Err(found) => {
                    ambiguity.get_or_insert(found);
                    None
                }
            }
        });
        match ambiguity {
            Some(ambiguity) => Err(ambiguity),
            None => Ok(renamed),
        }
    }

    /// What a name the model at index `model` holds outside an expression
    /// (`Model::map_variable_names`) becomes, or `None` when the rename
    /// leaves it. `to` is the new name as the caller spelled it.
    fn renamed_name(&self, model: usize, role: NameRole, name: &str, to: &str) -> Option<String> {
        // A name of the renamed variable's own model, compared canonically.
        let is_renamed = || model == self.model && canonicalize(name) == self.old.as_str();
        match role {
            // The variable's own name takes the caller's display spelling
            // verbatim (ident fields hold display names; see the comment
            // above `canonicalize_stock_references`).
            NameRole::Ident => is_renamed().then(|| to.to_string()),
            // A flow keeps its place in its list (the order is the flows'
            // priority, `canonical_flow_list`), and a reference's end is a
            // path. Both are stored canonical, the form the upserts store
            // them in (`canonicalize_stock_references`,
            // `canonicalize_module_references`).
            NameRole::Inflow | NameRole::Outflow | NameRole::ModuleSource => self
                .renamed(model, &Ident::new(name), ReadAs::Variable)
                .map(|renamed| renamed.as_str().to_string()),
            NameRole::ModuleDestination => self
                .renamed(model, &Ident::new(name), ReadAs::Port)
                .map(|renamed| renamed.as_str().to_string()),
            // Names of the model's own variables, which their readers
            // canonicalize: the distribution's by `conveyor_compile`, a
            // macro's by the registry, a group's by the patch operations.
            NameRole::Distribution
            | NameRole::MacroParameter
            | NameRole::MacroOutput
            | NameRole::GroupMember => is_renamed().then(|| self.new.as_str().to_string()),
            // A diagram's label is the view's own, and a rename leaves it:
            // every view's label names the old name until a view edit
            // (`ModelOperation::EditView`, an upserted view) or a diagram
            // sync that carries the patch (the incremental layout) relabels
            // it, wrapping the label as it does.
            NameRole::ViewLabel => None,
        }
    }
}

/// The expression texts of one model as a rename reads them: which names
/// written in them are references to the renamed variable, and where the
/// rename cannot tell.
///
/// The compiler has no single statement of what an identifier inside a
/// subscript's brackets names. The dependency walk skips one that is a
/// dimension or an element of any dimension
/// (`variable::ClassifyVisitor::is_dimension_or_element`). A static subscript
/// reads an element of the axis it indexes (`compiler::subscript`, through
/// `dimensions::resolve_axis_index_name`), and a range reads its ends as
/// elements only when both are elements of the axis, as variables otherwise.
/// A dimension's name is the active axis's element where an axis supplies it,
/// and the like-named variable where none does
/// (`compiler::Context::lower_from_expr3`). A rename that chose among those
/// readings would be a resolver of its own replaying a rule that is not one,
/// so it chooses none: wherever the reading decides what a respelled
/// reference means, the rename is refused.
///
/// - A variable whose name is a dimension's, before or after the rename
///   (`apply_rename_variable`, before any text is read). XMILE 1.0 section
///   3.7.1 says dimension names "must be distinct from model variables
///   names", and in an equation such a name can be either.
/// - A reference alone in a subscript's brackets, or at an end of a range
///   there, whose name before or after the rename is an element of some
///   dimension (`Dimension::canonical_element`, the membership test a
///   subscript is resolved by). XMILE 1.0 section 2.1 lets element names be
///   the same as variable names.
/// - A reference whose new spelling is a `dimension·element`, which the
///   compiler reads as that element's position.
///
/// What is left has one reading. A name that is no dimension and no element
/// is the variable wherever it is written, in brackets too (a dynamic index),
/// and is respelled. A `dimension·element` written as an identifier is that
/// element's position (`Expr1::constify_dimensions` over
/// `DimensionsContext::lookup`, which this asks rather than restates), so a
/// rename leaves it.
///
/// `patch::rename_tests` holds the rule to the run: every rename there is
/// refused, or leaves the model computing what it computed.
struct Reader<'a> {
    rename: &'a Rename<'a>,
    /// The model the texts are in, by its index in `project.models`.
    model: usize,
}

impl Reader<'_> {
    /// What `reference`, written at `site`, reads after the rename: `None`
    /// when it does not name the renamed variable, an [`Ambiguity`] when the
    /// compiler could read it, or its new spelling, as something else.
    fn renamed(&self, site: Site, reference: &Ident<Canonical>) -> Respelled {
        let dimensions = &self.rename.dimensions;
        let is_identifier = site != Site::Subscripted;
        if is_identifier && dimensions.lookup(reference.as_str()).is_some() {
            return Ok(None);
        }
        let Some(renamed) = self.rename.renamed(self.model, reference, ReadAs::Variable) else {
            return Ok(None);
        };
        let written = crate::ast::print_ident(reference.as_str());
        let spelled = crate::ast::print_ident(renamed.as_str());
        if is_identifier && dimensions.lookup(renamed.as_str()).is_some() {
            return Err(Ambiguity(format!(
                "reads `{written}`, which the rename would spell `{spelled}`, the \
                 position of a dimension's element"
            )));
        }
        if site == Site::Index {
            let element = |name: &Ident<Canonical>| {
                let (_, path) = split_scope_prefix(name.as_str());
                self.rename.dimension_with_element(path)
            };
            if let Some(dimension) = element(reference) {
                return Err(Ambiguity(format!(
                    "writes `{written}` alone in a subscript, and `{written}` is also an \
                     element of the dimension '{dimension}': the rename cannot tell whether \
                     it names the element or the variable there"
                )));
            }
            if let Some(dimension) = element(&renamed) {
                return Err(Ambiguity(format!(
                    "writes `{written}` alone in a subscript, and `{spelled}` is an element \
                     of the dimension '{dimension}': renamed, it would name the element \
                     there rather than the variable"
                )));
            }
        }
        Ok(Some(renamed))
    }
}

/// The longest text a span addresses: a span is a pair of `u16`s
/// (`builtins::Loc`), and a position past it wraps.
const SPAN_REACH: usize = u16::MAX as usize;

/// One expression text with every reference `renamed` gives a new name
/// respelled, or `None` when it gives none: the text stays as it is. An empty
/// or unparseable text is one of those; its parse errors are the variable's
/// own diagnostics, reported by the compile. A reference `renamed` finds
/// ambiguous refuses the whole text with the first such one.
///
/// The text is parsed as written (`Expr0::new`, the parser alone) only to
/// find its references. Neither compiler tier can find them: the parse memo's
/// tree is the EXPANDED one -- a `SMTH1(x, 3)` is already an instance read and
/// a `PREVIOUS(x + 1)` a capture -- and the lowered tree is absent for an
/// equation the compiler refuses, which must still be renamed or the refusal
/// turns into an unknown dependency. Each renamed reference is then spliced
/// into the text at its own span, so everything else the modeler wrote -- the
/// spacing, a builtin's case, every other name's spelling -- comes back byte
/// for byte (`patch::rename_tests`).
///
/// A text longer than `SPAN_REACH` has no spans to splice at: one past the
/// reach has wrapped onto another place in the text, where the same letters
/// may well stand. It goes through the printer instead, which spells the same
/// expression without the modeler's layout, and that is decided on the text's
/// length, before any span is read.
fn renamed_text(
    text: &str,
    renamed: &Respelling<'_>,
) -> std::result::Result<Option<String>, Ambiguity> {
    let Ok(Some(expr)) = Expr0::new(text, LexerType::Equation) else {
        return Ok(None);
    };
    let mut walk = Walk {
        renamed,
        splices: Vec::new(),
        ambiguity: None,
    };
    let respelled = walk.expr(&expr);
    let Walk {
        mut splices,
        ambiguity,
        ..
    } = walk;
    if let Some(ambiguity) = ambiguity {
        return Err(ambiguity);
    }
    if splices.is_empty() {
        return Ok(None);
    }
    if text.len() > SPAN_REACH {
        return Ok(Some(print_eqn(&respelled)));
    }
    splices.sort_by_key(|splice| std::cmp::Reverse(splice.start));
    let mut text = text.to_string();
    for splice in splices {
        text.replace_range(splice.start..splice.start + splice.old_len, &splice.new);
    }
    Ok(Some(text))
}

/// Every reference `text` writes, in the order written, with the [`Site`] it
/// is written at: the walk a rename makes of a text (`renamed_text`), asked
/// to respell nothing. `None` for a text that does not parse, whose
/// references no reader can know; an empty text writes none.
pub(crate) fn text_references(text: &str) -> Option<Vec<(Site, Ident<Canonical>)>> {
    let expr = match Expr0::new(text, LexerType::Equation) {
        Ok(Some(expr)) => expr,
        Ok(None) => return Some(Vec::new()),
        Err(_) => return None,
    };
    let found = std::cell::RefCell::new(Vec::new());
    let record = |site: Site, reference: &Ident<Canonical>| -> Respelled {
        found.borrow_mut().push((site, reference.clone()));
        Ok(None)
    };
    let mut walk = Walk {
        renamed: &record,
        splices: Vec::new(),
        ambiguity: None,
    };
    walk.expr(&expr);
    Some(found.into_inner())
}

/// One reference a rename respells: where it starts in the text, how long it
/// is written there, and how it reads after.
struct Splice {
    start: usize,
    old_len: usize,
    new: String,
}

/// The one walk of an expression's references a rename makes: each reference
/// is put to `renamed` with the [`Site`] it is written at, and the walk
/// returns the expression with every new name in place -- the tree the
/// printer spells when the text is past a span's reach -- while recording
/// each new name in `splices` with its place in the text, which is what the
/// text takes otherwise, and the first ambiguity met.
struct Walk<'r, 'f> {
    renamed: &'r Respelling<'f>,
    splices: Vec<Splice>,
    ambiguity: Option<Ambiguity>,
}

impl Walk<'_, '_> {
    fn expr(&mut self, expr: &Expr0) -> Expr0 {
        match expr {
            Expr0::Const(..) => expr.clone(),
            Expr0::Var(ident, loc) => Expr0::Var(self.ident(ident, *loc, Site::Value), *loc),
            // Every argument is an expression, a bare variable reference
            // included (`isModuleInput(x)`), so one walk covers every builtin.
            Expr0::App(UntypedBuiltinFn(name, args), loc) => Expr0::App(
                UntypedBuiltinFn(
                    name.clone(),
                    args.iter().map(|arg| self.expr(arg)).collect(),
                ),
                *loc,
            ),
            // A subscripted reference's span covers its brackets; the name is
            // at its start.
            Expr0::Subscript(ident, indexes, loc) => Expr0::Subscript(
                self.ident(ident, *loc, Site::Subscripted),
                indexes.iter().map(|index| self.index(index)).collect(),
                *loc,
            ),
            Expr0::Op1(op, rhs, loc) => Expr0::Op1(*op, Box::new(self.expr(rhs)), *loc),
            Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
                *op,
                Box::new(self.expr(lhs)),
                Box::new(self.expr(rhs)),
                *loc,
            ),
            Expr0::If(cond, then_branch, else_branch, loc) => Expr0::If(
                Box::new(self.expr(cond)),
                Box::new(self.expr(then_branch)),
                Box::new(self.expr(else_branch)),
                *loc,
            ),
        }
    }

    /// One index of a subscript. An identifier alone in an index, or at an
    /// end of a range, is at `Site::Index`; any other index is an expression
    /// like another.
    fn index(&mut self, index: &IndexExpr0) -> IndexExpr0 {
        let mut alone = |expr: &Expr0| match expr {
            Expr0::Var(ident, loc) => Expr0::Var(self.ident(ident, *loc, Site::Index), *loc),
            Expr0::Const(..)
            | Expr0::App(..)
            | Expr0::Subscript(..)
            | Expr0::Op1(..)
            | Expr0::Op2(..)
            | Expr0::If(..) => self.expr(expr),
        };
        match index {
            // A star range names a dimension, never a variable.
            IndexExpr0::Wildcard(_)
            | IndexExpr0::StarRange(_, _)
            | IndexExpr0::DimPosition(_, _) => index.clone(),
            IndexExpr0::Range(lhs, rhs, loc) => {
                IndexExpr0::Range(Box::new(alone(lhs)), Box::new(alone(rhs)), *loc)
            }
            IndexExpr0::Expr(expr) => IndexExpr0::Expr(alone(expr)),
        }
    }

    /// A reference as written at `site`, respelled when `renamed` gives it a
    /// new name (and then recorded in `splices`); a reference the rename
    /// leaves alone, or finds ambiguous, keeps the modeler's spelling. The
    /// new spelling is `ast::print_ident`'s of the canonical name, the one
    /// spelling of a name inside equation text, which quotes a name the lexer
    /// cannot read bare: a name holding a literal period is written `"a.b"`,
    /// never as the path `a.b`.
    fn ident(&mut self, ident: &RawIdent, loc: Loc, site: Site) -> RawIdent {
        match (self.renamed)(site, &ident.canonicalize()) {
            Ok(Some(renamed)) => {
                let spelled = crate::ast::print_ident(renamed.as_str());
                self.splices.push(Splice {
                    start: loc.start as usize,
                    old_len: ident.as_str().len(),
                    new: spelled.clone(),
                });
                RawIdent::new(spelled)
            }
            Ok(None) => ident.clone(),
            Err(ambiguity) => {
                self.ambiguity.get_or_insert(ambiguity);
                ident.clone()
            }
        }
    }
}

pub(crate) fn expr2_to_string(expr: &Expr2) -> String {
    let expr0 = expr2_to_expr0(expr);
    crate::ast::print_eqn(&expr0)
}

/// A canonical name as the identifier a parsed expression holds for it. The
/// canonical form is carried over as it is: it reads back as itself, where
/// the source form (`Ident::to_source_repr`) would read a literal period back
/// as a module separator and name another variable.
fn raw_ident(ident: &Ident<Canonical>) -> RawIdent {
    RawIdent::new(ident.as_str().to_string())
}

pub(crate) fn expr2_to_expr0(expr: &Expr2) -> Expr0 {
    match expr {
        Expr2::Const(text, value, loc) => Expr0::Const(text.clone(), *value, *loc),
        Expr2::Var(ident, _, loc) => Expr0::Var(raw_ident(ident), *loc),
        Expr2::App(builtin, _, loc) => {
            let untyped = builtin_to_untyped(builtin);
            Expr0::App(untyped, *loc)
        }
        Expr2::Subscript(ident, indexes, _, loc) => Expr0::Subscript(
            raw_ident(ident),
            indexes.iter().map(index_expr2_to_index_expr0).collect(),
            *loc,
        ),
        Expr2::Op1(op, rhs, _, loc) => Expr0::Op1(*op, Box::new(expr2_to_expr0(rhs)), *loc),
        Expr2::Op2(op, lhs, rhs, _, loc) => Expr0::Op2(
            *op,
            Box::new(expr2_to_expr0(lhs)),
            Box::new(expr2_to_expr0(rhs)),
            *loc,
        ),
        Expr2::If(cond, then_branch, else_branch, _, loc) => Expr0::If(
            Box::new(expr2_to_expr0(cond)),
            Box::new(expr2_to_expr0(then_branch)),
            Box::new(expr2_to_expr0(else_branch)),
            *loc,
        ),
    }
}

pub(crate) fn index_expr2_to_index_expr0(index: &IndexExpr2) -> crate::ast::IndexExpr0 {
    use crate::ast::IndexExpr0;
    match index {
        IndexExpr2::Wildcard(loc) => IndexExpr0::Wildcard(*loc),
        IndexExpr2::StarRange(dim, loc) => {
            IndexExpr0::StarRange(RawIdent::new(dim.as_str().to_string()), *loc)
        }
        IndexExpr2::Range(lhs, rhs, loc) => IndexExpr0::Range(
            Box::new(expr2_to_expr0(lhs)),
            Box::new(expr2_to_expr0(rhs)),
            *loc,
        ),
        IndexExpr2::DimPosition(pos, loc) => IndexExpr0::DimPosition(*pos, *loc),
        IndexExpr2::Expr(expr) => IndexExpr0::Expr(expr2_to_expr0(expr)),
    }
}

pub(crate) fn builtin_to_untyped(builtin: &BuiltinFn<Expr2>) -> UntypedBuiltinFn<Expr0> {
    use crate::builtins::BuiltinFn;
    let args: Box<[Expr0]> = match builtin {
        // The identifier payload prints as the bare variable reference it was
        // parsed from.
        BuiltinFn::IsModuleInput(ident, _) => {
            Box::new([Expr0::Var(RawIdent::new(ident.clone()), Default::default())])
        }
        other => other.args().into_iter().map(expr2_to_expr0).collect(),
    };
    UntypedBuiltinFn(builtin.name().to_string(), args)
}

fn apply_upsert_view(
    model: &mut datamodel::Model,
    index: u32,
    view: datamodel::View,
) -> Result<()> {
    let index = index as usize;

    if index < model.views.len() {
        // A replacement view arrives with every element freshly allocated;
        // each one it keeps as it was shares the replaced view's allocation.
        let mut view = view;
        let (datamodel::View::StockFlow(new), datamodel::View::StockFlow(old)) =
            (&mut view, &model.views[index]);
        new.elements
            .share_identical(&old.elements, datamodel::ViewElement::get_uid);
        model.views[index] = view;
        Ok(())
    } else if index == model.views.len() {
        // Allow appending at the end
        model.views.push(view);
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DoesNotExist,
            Some(format!("view index {index} out of range")),
        ))
    }
}

fn apply_delete_view(model: &mut datamodel::Model, index: u32) -> Result<()> {
    let index = index as usize;
    if index < model.views.len() {
        model.views.remove(index);
        Ok(())
    } else {
        Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DoesNotExist,
            Some(format!("view index {index} out of range")),
        ))
    }
}

#[cfg(test)]
#[path = "patch_flow_order_tests.rs"]
mod flow_order_tests;

#[cfg(test)]
#[path = "patch_rename_tests.rs"]
mod rename_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::{self, Equation, Visibility};
    use crate::test_common::TestProject;

    #[test]
    fn upsert_aux_adds_variable() {
        let mut project = TestProject::new("test").build_datamodel();
        let aux = datamodel::Aux {
            ident: "new_aux".to_string(),
            equation: Equation::Scalar("1".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertAux(aux.clone())],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        let var = model.get_variable("new_aux").unwrap();
        match var {
            Variable::Aux(actual) => assert_eq!(actual.equation, aux.equation),
            _ => panic!("expected aux"),
        }
    }

    #[test]
    fn upsert_stock_replaces_existing() {
        let mut project = TestProject::new("test")
            .stock("stock", "1", &[], &[], None)
            .build_datamodel();
        let stock = datamodel::Stock {
            ident: "stock".to_string(),
            equation: Equation::Scalar("5".to_string()),
            documentation: "docs".to_string(),
            units: Some("people".to_string()),
            inflows: vec!["flow".to_string()],
            outflows: vec![],
            ai_state: None,
            uid: Some(10),
            compat: datamodel::Compat {
                non_negative: true,
                can_be_module_input: true,
                visibility: Visibility::Public,
                ..datamodel::Compat::default()
            },
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertStock(stock.clone())],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        let var = model.get_variable("stock").unwrap();
        match var {
            Variable::Stock(actual) => {
                assert_eq!(actual.equation, stock.equation);
                assert_eq!(actual.inflows, stock.inflows);
                assert_eq!(actual.compat.non_negative, stock.compat.non_negative);
                assert_eq!(actual.compat.visibility, stock.compat.visibility);
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn delete_flow_removes_references() {
        let mut project = TestProject::new("test")
            .flow("flow", "1", None)
            .stock("stock", "stock", &["flow"], &["flow"], None)
            .build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::DeleteVariable {
                    ident: "flow".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert!(model.get_variable("flow").is_none());
        match model.get_variable("stock").unwrap() {
            Variable::Stock(stock) => {
                assert!(stock.inflows.is_empty());
                assert!(stock.outflows.is_empty());
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn rename_flow_updates_stock_references() {
        let mut project = TestProject::new("test")
            .flow("flow", "1", None)
            .stock("stock", "stock", &["flow"], &["flow"], None)
            .build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "flow".to_string(),
                    to: "new_flow".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert!(model.get_variable("flow").is_none());
        match model.get_variable("new_flow").unwrap() {
            Variable::Flow(_) => {}
            _ => panic!("expected flow"),
        }
        match model.get_variable("stock").unwrap() {
            Variable::Stock(stock) => {
                assert_eq!(stock.inflows, vec!["new_flow".to_string()]);
                assert_eq!(stock.outflows, vec!["new_flow".to_string()]);
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn set_sim_specs() {
        let mut project = TestProject::new("test").build_datamodel();
        let new_specs = datamodel::SimSpecs {
            start: 5.0,
            stop: project.sim_specs.stop,
            dt: datamodel::Dt::Dt(0.5),
            save_step: None,
            sim_method: datamodel::SimMethod::RungeKutta4,
            time_units: Some("days".to_string()),
        };
        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation::SetSimSpecs(new_specs)],
            models: vec![],
        };

        apply_patch(&mut project, patch).unwrap();
        assert_eq!(project.sim_specs.start, 5.0);
        assert_eq!(project.sim_specs.dt, datamodel::Dt::Dt(0.5));
        assert!(project.sim_specs.save_step.is_none());
        assert_eq!(
            project.sim_specs.sim_method,
            datamodel::SimMethod::RungeKutta4
        );
        assert_eq!(project.sim_specs.time_units, Some("days".to_string()));
    }

    /// A lowered expression turned back into a parsed one names each variable
    /// as itself, a name with a literal period in it included: the link scores
    /// LTM generates from one are the scores of the same model under a plain
    /// name, with nothing declined.
    #[test]
    fn a_lowered_expression_names_a_variable_with_a_period_as_itself() {
        use crate::db::{LtmOverlay, SimlinDb, collect_all_diagnostics};

        let link_score = |rate: &str| {
            let project = TestProject::new("period")
                .aux(rate, "0.1", None)
                .aux("cap", "100", None)
                .flow(
                    "inflow",
                    &format!("level * {rate} * (1 - level / cap)"),
                    None,
                )
                .stock("level", "10", &["inflow"], &[], None)
                .build_datamodel();
            let mut db = SimlinDb::default();
            let source = db.sync(&project);
            let rows: Vec<String> = collect_all_diagnostics(&db, source, LtmOverlay::On)
                .iter()
                .map(|d| format!("{:?}: {:?}", d.variable, d.reason()))
                .collect();
            assert!(rows.is_empty(), "{rate}: {rows:#?}");
            let compiled =
                crate::db::compile_project_incremental(&db, source, "main", LtmOverlay::On)
                    .expect("the model compiles under LTM");
            let mut vm = crate::vm::Vm::new((*compiled).clone()).expect("a vm");
            vm.run_to_end().expect("it runs");
            let results = crate::test_common::collect_results(&vm.into_results());
            results["$\u{205A}ltm\u{205A}link_score\u{205A}level\u{2192}inflow"].clone()
        };
        let plain = link_score("rate_x");
        assert!(plain.iter().any(|score| *score != 0.0), "{plain:?}");
        assert_eq!(link_score("\"rate.x\""), plain);
    }

    /// The specs an edit sets are the specs the root model runs under, whether
    /// or not the model carried specs of its own, which override the
    /// project's, and whatever the root is called. Another model's own specs
    /// are not the edit's to change.
    #[test]
    fn set_sim_specs_is_what_the_root_model_runs_under() {
        let specs = |stop: f64| datamodel::SimSpecs {
            start: 0.0,
            stop,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        };
        let last_time = |project: &datamodel::Project, model: &str| {
            let mut vm = crate::queue_compile::build_vm(project, model).expect("it builds");
            vm.run_to_end().expect("it runs");
            let results = crate::test_common::collect_results(&vm.into_results());
            results["time"].last().copied()
        };
        let set = ProjectPatch {
            project_ops: vec![ProjectOperation::SetSimSpecs(specs(20.0))],
            models: vec![],
        };
        // A model of its own specs, `stop` at 5 plus its place, so each
        // model's run says whose specs it ran under.
        let model = |name: &str, place: usize, is_macro: bool| datamodel::Model {
            name: name.to_string(),
            sim_specs: Some(specs(5.0 + place as f64)),
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: name.to_string(),
                equation: Equation::Scalar("TIME".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })]
            .into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: is_macro.then(|| datamodel::MacroSpec {
                parameters: vec![],
                primary_output: name.to_string(),
                additional_outputs: vec![],
            }),
        };

        // The models, each a name and whether it is a macro, and which is
        // the root.
        let rows: &[(&[(&str, bool)], usize)] = &[
            (&[("main", false), ("other", false)], 0),
            (&[("Main", false), ("other", false)], 0),
            (&[("", false), ("other", false)], 0),
            (&[("other", false), ("main", false)], 1),
            (&[("root model", false), ("other", false)], 0),
            (&[("helper", true), ("simulation", false)], 1),
        ];
        for (models, root) in rows {
            let mut project = TestProject::new("specs").build_datamodel();
            project.sim_specs = specs(10.0);
            project.models = models
                .iter()
                .enumerate()
                .map(|(place, (name, is_macro))| model(name, place, *is_macro))
                .collect();
            assert_eq!(project.default_model_index(), Some(*root), "{models:?}");

            apply_patch(&mut project, set.clone()).unwrap();
            assert!(project.sim_specs == specs(20.0));
            for (place, (name, is_macro)) in models.iter().enumerate() {
                let own = project.models[place].sim_specs.clone();
                if place == *root {
                    assert!(own.is_none(), "{models:?}: the root's own specs go");
                    assert_eq!(last_time(&project, name), Some(20.0), "{models:?}");
                } else {
                    assert!(
                        own == Some(specs(5.0 + place as f64)),
                        "{models:?}: {name} keeps its own"
                    );
                    if !is_macro {
                        assert_eq!(
                            last_time(&project, name),
                            Some(5.0 + place as f64),
                            "{models:?}: {name}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn upsert_view_and_delete() {
        let mut project = TestProject::new("test").build_datamodel();
        let view = datamodel::View::StockFlow(datamodel::StockFlow {
            name: None,
            elements: vec![].into(),
            view_box: datamodel::Rect::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        });
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertView {
                    index: 0,
                    view: view.clone(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert_eq!(model.views.len(), 1);

        let delete_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::DeleteView { index: 0 }],
            }],
        };

        apply_patch(&mut project, delete_patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert!(model.views.is_empty());
    }

    #[test]
    fn set_source() {
        let mut project = TestProject::new("test").build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation::SetSource(datamodel::Source {
                extension: datamodel::Extension::Xmile,
                content: "hello".to_string(),
            })],
            models: vec![],
        };

        apply_patch(&mut project, patch).unwrap();
        assert!(project.source.is_some());
        assert_eq!(project.source.as_ref().unwrap().content, "hello");
    }

    #[test]
    fn rename_duplicate_returns_error() {
        let mut project = TestProject::new("test")
            .flow("flow", "1", None)
            .flow("flow2", "2", None)
            .build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "flow".to_string(),
                    to: "flow2".to_string(),
                }],
            }],
        };

        let err = apply_patch(&mut project, patch).unwrap_err();
        assert_eq!(err.code, ErrorCode::DuplicateVariable);
        assert_eq!(err.kind, ErrorKind::Model);
    }

    #[test]
    fn rename_aux_updates_equations() {
        let mut project = TestProject::new("test")
            .aux("foo", "bar + 1", None)
            .aux("bar", "foo + 2", None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "bar".to_string(),
                    to: "baz".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("foo").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn) => assert_eq!(eqn, "baz + 1"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }

        match model.get_variable("baz").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn) => assert_eq!(eqn, "foo + 2"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected renamed auxiliary"),
        }

        assert!(model.get_variable("bar").is_none());
    }

    #[test]
    fn rename_updates_module_references() {
        let mut project = TestProject::new("test")
            .aux("input", "1", None)
            .aux("consumer", "input * 2", None)
            .build_datamodel();

        let model = project
            .models
            .iter_mut()
            .find(|m| m.name == "main")
            .expect("main model");

        model
            .variables
            .push(datamodel::Variable::Module(datamodel::Module {
                ident: "child".to_string(),
                model_name: "child".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: "input".to_string(),
                    dst: "self.target".to_string(),
                }],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }));

        project.models.push(datamodel::Model {
            name: "child".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "target".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })]
            .into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "input".to_string(),
                    to: "new_input".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("consumer").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn) => assert_eq!(eqn, "new_input * 2"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }

        let module = model
            .variables
            .iter()
            .find_map(|var| match var {
                Variable::Module(module) => Some(module),
                _ => None,
            })
            .expect("module variable");

        assert_eq!(module.references.len(), 1);
        assert_eq!(module.references[0].src, "new_input");
        assert_eq!(module.references[0].dst, "self.target");
    }

    #[test]
    fn rename_rewrites_a_parent_scope_module_source() {
        // XMILE spells a module input's source in the enclosing model as `.x`
        // (`<connect to="hares.area" from=".area"/>`), which the reader stores
        // canonicalized, as `·x`, and which every consumer reads as the bare
        // name. Renaming x must rewrite that source, or the module silently
        // reads its input port's default.
        let mut project = TestProject::new("test")
            .aux("input", "1", None)
            .build_datamodel();
        let parent_scope_source = canonicalize(".input").into_owned();
        let model = project.get_model_mut("main").expect("main model");
        model
            .variables
            .push(datamodel::Variable::Module(datamodel::Module {
                ident: "child".to_string(),
                model_name: "child".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: parent_scope_source,
                    dst: canonicalize("child.target").into_owned(),
                }],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }));

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "input".to_string(),
                    to: "new_input".to_string(),
                }],
            }],
        };
        apply_patch(&mut project, patch).unwrap();

        let Some(Variable::Module(module)) =
            project.get_model("main").unwrap().get_variable("child")
        else {
            panic!("child is a module");
        };
        // A rename writes a reference's source spelling (`self.target`, `.x`);
        // what it names is the canonical form every consumer reads.
        assert_eq!(
            canonicalize(&module.references[0].src),
            canonicalize(".new_input")
        );
    }

    #[test]
    fn rename_does_not_affect_unrelated_module_variables() {
        let mut project = TestProject::new("test")
            .aux("foo", "1", None)
            .aux("bar", "2", None)
            .aux("consumer", "foo + child·foo + bar", None)
            .build_datamodel();

        let model = project
            .models
            .iter_mut()
            .find(|m| m.name == "main")
            .expect("main model");

        model
            .variables
            .push(datamodel::Variable::Module(datamodel::Module {
                ident: "child".to_string(),
                model_name: "child_model".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![datamodel::ModuleReference {
                    src: "bar".to_string(),
                    dst: "child·foo".to_string(),
                }],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }));

        project.models.push(datamodel::Model {
            name: "child_model".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "foo".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })]
            .into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "foo".to_string(),
                    to: "renamed_foo".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("consumer").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn) => {
                    assert_eq!(eqn, "renamed_foo + child·foo + bar");
                }
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }

        match model.get_variable("renamed_foo").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn) => assert_eq!(eqn, "1"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected renamed auxiliary"),
        }

        assert!(model.get_variable("foo").is_none());
    }

    /// A rename is syntactic: every equation that parses is rewritten, whether
    /// or not it lowers. `bad = a + b` mismatches its axes (`a[d]`, `b[p]`),
    /// which the compiler refuses; renaming `a` must still rewrite it, or the
    /// stale name turns the refusal into an `unknown_dependency` on a product
    /// surface (MCP `edit_model`, libsimlin `apply_patch`). An equation that
    /// does not parse is left as written.
    #[test]
    fn rename_rewrites_an_equation_the_lowering_refuses() {
        let mut project = TestProject::new("test")
            .named_dimension("d", &["d1", "d2"])
            .named_dimension("p", &["p1", "p2"])
            .array_aux("a[d]", "1")
            .array_aux("b[p]", "2")
            .aux("bad", "a + b", None)
            .aux("good", "SUM(a) * 2", None)
            .aux("unparsed", "a +", None)
            .build_datamodel();

        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::RenameVariable {
                        from: "a".to_string(),
                        to: "aa".to_string(),
                    }],
                }],
            },
        )
        .unwrap();
        let model = project.get_model("main").unwrap();
        let scalar = |name: &str| match model.get_variable(name).unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn) => eqn.clone(),
                _ => panic!("{name}: expected a scalar equation"),
            },
            _ => panic!("{name}: expected an aux"),
        };
        assert_eq!(scalar("bad"), "aa + b", "a mismatched equation is renamed");
        assert_eq!(
            scalar("good"),
            "SUM(aa) * 2",
            "a compiling equation is renamed"
        );
        assert_eq!(
            scalar("unparsed"),
            "a +",
            "an unparseable equation is left as written"
        );
        assert!(model.get_variable("aa").is_some());
    }

    /// An arrayed equation is renamed string by string: every per-element
    /// text, every per-element initial and the EXCEPT default, in place, with
    /// the elements and the `except` flag kept.
    #[test]
    fn rename_renames_an_arrayed_equations_elements_initials_and_default() {
        let mut project = TestProject::new("test")
            .named_dimension("d", &["d1", "d2", "d3"])
            .aux("k", "1", None)
            .build_datamodel();
        project.models[0]
            .variables
            .push(Variable::Aux(datamodel::Aux {
                ident: "arr".to_string(),
                equation: datamodel::Equation::Arrayed(
                    vec!["d".to_string()],
                    vec![
                        (
                            "d1".to_string(),
                            "k * 2".to_string(),
                            Some("k + 1".to_string()),
                            None,
                        ),
                        ("d2".to_string(), "5".to_string(), None, None),
                    ],
                    Some("k * 3".to_string()),
                    true,
                ),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }));

        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::RenameVariable {
                        from: "k".to_string(),
                        to: "kk".to_string(),
                    }],
                }],
            },
        )
        .unwrap();

        let model = project.get_model("main").unwrap();
        let Variable::Aux(arr) = model.get_variable("arr").unwrap() else {
            panic!("expected an aux");
        };
        let datamodel::Equation::Arrayed(dims, elements, default, except) = &arr.equation else {
            panic!("expected an arrayed equation");
        };
        assert_eq!(dims, &["d".to_string()]);
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].0, "d1");
        assert_eq!(elements[0].1, "kk * 2", "the element's text is renamed");
        assert_eq!(
            elements[0].2.as_deref(),
            Some("kk + 1"),
            "the element's initial is renamed"
        );
        assert!(elements[0].3.is_none());
        assert_eq!(elements[1].0, "d2");
        assert_eq!(
            elements[1].1, "5",
            "an element naming nothing renamed is as written"
        );
        assert_eq!(elements[1].2, None);
        assert_eq!(
            default.as_deref(),
            Some("kk * 3"),
            "the EXCEPT default is renamed in place"
        );
        assert!(*except, "the EXCEPT flag is kept");
    }

    /// A module-function call and a snapshot argument are the user's text, not
    /// the instance read and the capture the parse rewrites them into: a
    /// rename rewrites the argument and keeps the call, in the case it was
    /// written in.
    #[test]
    fn rename_keeps_a_module_function_call_as_written() {
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .aux("y", "SMTH1(x, 3)", None)
            .aux("z", "PREVIOUS(x + 1) + INIT(x * 2)", None)
            .aux("untouched", "SMTH1(y, 2)", None)
            .build_datamodel();

        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::RenameVariable {
                        from: "x".to_string(),
                        to: "w".to_string(),
                    }],
                }],
            },
        )
        .unwrap();
        let model = project.get_model("main").unwrap();
        let scalar = |name: &str| match model.get_variable(name).unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn) => eqn.clone(),
                _ => panic!("{name}: expected a scalar equation"),
            },
            _ => panic!("{name}: expected an aux"),
        };
        assert_eq!(scalar("y"), "SMTH1(w, 3)");
        assert_eq!(scalar("z"), "PREVIOUS(w + 1) + INIT(w * 2)");
        assert_eq!(
            scalar("untouched"),
            "SMTH1(y, 2)",
            "an equation naming nothing renamed is left exactly as written"
        );
    }

    #[test]
    fn rename_self_qualified_references() {
        let mut project = TestProject::new("test")
            .aux("foo", "1", None)
            .aux("consumer", "foo + self·foo", None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "foo".to_string(),
                    to: "bar".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("consumer").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(eqn) => {
                    assert_eq!(eqn, "bar + self·bar");
                }
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }
    }

    #[test]
    fn rename_arrayed_equation() {
        let mut project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![datamodel::Dimension::named(
                "Region".to_string(),
                vec!["North".to_string(), "South".to_string()],
            )],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "base_value".to_string(),
                        equation: datamodel::Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "regional_growth".to_string(),
                        equation: datamodel::Equation::Arrayed(
                            vec!["Region".to_string()],
                            vec![
                                (
                                    "North".to_string(),
                                    "base_value * 1.5".to_string(),
                                    None,
                                    None,
                                ),
                                (
                                    "South".to_string(),
                                    "base_value * 2".to_string(),
                                    None,
                                    None,
                                ),
                            ],
                            None,
                            false,
                        ),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ]
                .into(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "base_value".to_string(),
                    to: "initial_value".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("regional_growth").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Arrayed(dims, elements, _default_eq, _) => {
                    assert_eq!(dims, &vec!["Region".to_string()]);
                    assert_eq!(elements[0].0, "North");
                    assert_eq!(elements[0].1, "initial_value * 1.5");
                    assert_eq!(elements[1].0, "South");
                    assert_eq!(elements[1].1, "initial_value * 2");
                }
                _ => panic!("expected arrayed equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }
    }

    #[test]
    fn rename_apply_to_all_equation() {
        let mut project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![datamodel::Dimension::named(
                "Product".to_string(),
                vec!["A".to_string(), "B".to_string(), "C".to_string()],
            )],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "price".to_string(),
                        equation: datamodel::Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "revenue".to_string(),
                        equation: datamodel::Equation::ApplyToAll(
                            vec!["Product".to_string()],
                            "price * quantity".to_string(),
                        ),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: "quantity".to_string(),
                        equation: datamodel::Equation::Scalar("5".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ]
                .into(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "price".to_string(),
                    to: "unit_price".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("revenue").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::ApplyToAll(dims, eqn) => {
                    assert_eq!(dims, &vec!["Product".to_string()]);
                    assert_eq!(eqn, "unit_price * quantity");
                }
                _ => panic!("expected apply-to-all equation"),
            },
            _ => panic!("expected auxiliary variable"),
        }
    }

    #[test]
    fn rename_stock_with_initial_value() {
        let mut project = TestProject::new("test")
            .aux("initial_stock", "100", None)
            .stock("inventory", "initial_stock * 2", &[], &[], None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "initial_stock".to_string(),
                    to: "starting_inventory".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("inventory").unwrap() {
            Variable::Stock(stock) => match &stock.equation {
                datamodel::Equation::Scalar(main) => {
                    assert_eq!(main, "starting_inventory * 2");
                }
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected stock variable"),
        }
    }

    #[test]
    fn upsert_stock_to_model_with_empty_name() {
        let mut project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "".to_string(),
                sim_specs: None,
                variables: vec![].into(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        let stock = datamodel::Stock {
            ident: "inventory".to_string(),
            equation: Equation::Scalar("100".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertStock(stock.clone())],
            }],
        };

        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        let var = model.get_variable("inventory").unwrap();
        match var {
            Variable::Stock(actual) => assert_eq!(actual.equation, stock.equation),
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn update_stock_flows_preserves_all_fields() {
        let mut project = TestProject::new("test")
            .flow("birth_rate", "10", None)
            .stock_with_options(
                "population",
                "1000",
                &["birth_rate"],
                &[],
                Some("people"),
                "Total population",
                true,
                true,
                Visibility::Public,
                Some(42),
            )
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpdateStockFlows {
                    ident: "population".to_string(),
                    inflows: vec![],
                    outflows: vec![],
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        match model.get_variable("population").unwrap() {
            Variable::Stock(stock) => {
                assert!(stock.inflows.is_empty());
                assert!(stock.outflows.is_empty());
                assert_eq!(stock.equation, Equation::Scalar("1000".to_string()));
                assert_eq!(stock.documentation, "Total population");
                assert_eq!(stock.units, Some("people".to_string()));
                assert!(stock.compat.non_negative);
                assert!(stock.compat.can_be_module_input);
                assert_eq!(stock.compat.visibility, Visibility::Public);
                assert_eq!(stock.uid, Some(42));
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn update_stock_flows_nonexistent_stock_returns_error() {
        let mut project = TestProject::new("test").build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpdateStockFlows {
                    ident: "nonexistent".to_string(),
                    inflows: vec![],
                    outflows: vec![],
                }],
            }],
        };

        let err = apply_patch(&mut project, patch).unwrap_err();
        assert_eq!(err.code, ErrorCode::DoesNotExist);
    }

    /// Both ops that set a stock's flow lists store each flow once, judged
    /// after canonicalization (`"Flow A"` and `"flow_a"` name one flow). The
    /// compiler sums a stock's lists, so a stored duplicate would integrate the
    /// flow twice.
    #[test]
    fn every_op_that_sets_stock_flow_lists_stores_each_flow_once() {
        let base = TestProject::new("test")
            .flow("flow_a", "1", None)
            .flow("drain", "1", None)
            .stock("population", "0", &[], &[], None)
            .build_datamodel();

        let update = ModelOperation::UpdateStockFlows {
            ident: "population".to_string(),
            inflows: vec![
                "Flow A".to_string(),
                "flow_a".to_string(),
                "flow_a".to_string(),
            ],
            outflows: vec!["drain".to_string(), "Drain".to_string()],
        };
        let upsert = ModelOperation::UpsertStock(datamodel::Stock {
            ident: "population".to_string(),
            equation: Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec!["flow_a".to_string(), "Flow_A".to_string()],
            outflows: vec!["Drain".to_string(), "drain".to_string()],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        });

        for (label, op) in [("updateStockFlows", update), ("upsertStock", upsert)] {
            let mut project = base.clone();
            let patch = ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![op],
                }],
            };
            apply_patch(&mut project, patch).unwrap();
            match project
                .get_model("main")
                .unwrap()
                .get_variable("population")
                .unwrap()
            {
                Variable::Stock(stock) => {
                    assert_eq!(stock.inflows, vec!["flow_a".to_string()], "{label}");
                    assert_eq!(stock.outflows, vec!["drain".to_string()], "{label}");
                }
                _ => panic!("{label}: expected stock"),
            }
        }
    }

    /// Deduplication does not turn an upsert into a merge: a later upsert's
    /// lists replace the earlier ones outright.
    #[test]
    fn upsert_stock_still_replaces_flow_lists_outright() {
        let mut project = TestProject::new("test")
            .flow("flow_a", "1", None)
            .flow("flow_b", "1", None)
            .stock("population", "0", &["flow_a", "flow_a"], &[], None)
            .build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertStock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["flow_b".to_string(), "flow_b".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
            }],
        };
        apply_patch(&mut project, patch).unwrap();
        match project
            .get_model("main")
            .unwrap()
            .get_variable("population")
            .unwrap()
        {
            Variable::Stock(stock) => {
                assert_eq!(stock.inflows, vec!["flow_b".to_string()]);
                assert!(stock.outflows.is_empty());
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn rename_updates_group_members() {
        let mut project = TestProject::new("test")
            .aux("alpha", "1", None)
            .aux("beta", "2", None)
            .build_datamodel();

        let model = project
            .models
            .iter_mut()
            .find(|m| m.name == "main")
            .unwrap();

        model.groups.push(datamodel::ModelGroup {
            name: "my_group".to_string(),
            doc: None,
            parent: None,
            members: vec!["alpha".to_string(), "beta".to_string()],
            run_enabled: true,
        });

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "alpha".to_string(),
                    to: "gamma".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        assert_eq!(model.groups.len(), 1);
        assert_eq!(model.groups[0].members, vec!["gamma", "beta"]);
    }

    #[test]
    fn delete_removes_from_group_members() {
        let mut project = TestProject::new("test")
            .aux("alpha", "1", None)
            .aux("beta", "2", None)
            .build_datamodel();

        let model = project
            .models
            .iter_mut()
            .find(|m| m.name == "main")
            .unwrap();

        model.groups.push(datamodel::ModelGroup {
            name: "my_group".to_string(),
            doc: None,
            parent: None,
            members: vec!["alpha".to_string(), "beta".to_string()],
            run_enabled: true,
        });

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::DeleteVariable {
                    ident: "alpha".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        assert_eq!(model.groups.len(), 1);
        assert_eq!(model.groups[0].members, vec!["beta"]);
    }

    // --- New tests for module support and AddModel ---

    #[test]
    fn add_model_creates_empty_model() {
        let mut project = TestProject::new("test").build_datamodel();
        assert_eq!(project.models.len(), 1);

        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation::AddModel {
                name: "submodel".to_string(),
            }],
            models: vec![],
        };

        apply_patch(&mut project, patch).unwrap();
        assert_eq!(project.models.len(), 2);
        let submodel = project.get_model("submodel").unwrap();
        assert!(submodel.variables.is_empty());
        assert!(submodel.views.is_empty());
    }

    /// A model's name is taken when its canonical form is: the engine files
    /// models under their canonical names, and two of one name are a project
    /// it refuses to compile. The project is left as it was.
    #[test]
    fn add_model_refuses_a_name_whose_canonical_form_is_taken() {
        let mut base = TestProject::new("test").build_datamodel();
        apply_patch(
            &mut base,
            ProjectPatch {
                project_ops: vec![ProjectOperation::AddModel {
                    name: "Customer Growth".to_string(),
                }],
                models: vec![],
            },
        )
        .unwrap();

        for taken in [
            "main",
            "Main",
            "MAIN",
            "Customer Growth",
            "customer_growth",
            "customer growth",
            "Customer_Growth",
            // A name whose canonical form is empty is the unnamed model's.
            "   ",
            "\t",
            "\"\"",
        ] {
            let mut project = base.clone();
            let err = apply_patch(
                &mut project,
                ProjectPatch {
                    project_ops: vec![ProjectOperation::AddModel {
                        name: taken.to_string(),
                    }],
                    models: vec![],
                },
            )
            .expect_err(taken);
            assert_eq!(err.code, ErrorCode::DuplicateVariable, "{taken}");
            assert!(project == base, "{taken}: the project is as it was");
        }

        // An unnamed model is `main`, from either side.
        let add = |name: &str| ProjectPatch {
            project_ops: vec![ProjectOperation::AddModel {
                name: name.to_string(),
            }],
            models: vec![],
        };
        let mut project = base.clone();
        let err = apply_patch(&mut project, add("")).expect_err("the empty name is main");
        assert_eq!(err.code, ErrorCode::DuplicateVariable);
        let mut unnamed = base.clone();
        unnamed.models[0].name = String::new();
        let mut project = unnamed.clone();
        let err = apply_patch(&mut project, add("Main")).expect_err("main is the unnamed model");
        assert_eq!(err.code, ErrorCode::DuplicateVariable);
        assert!(project == unnamed);
        let err = apply_patch(&mut project, add("  ")).expect_err("a blank name is unnamed too");
        assert_eq!(err.code, ErrorCode::DuplicateVariable);
        assert!(project == unnamed);

        // A name under the stdlib's prefix is the stdlib's, whether or not a
        // stdlib model has it.
        for reserved in [
            "stdlib\u{205A}smth1",
            "Stdlib\u{205A}SMTH1",
            "stdlib\u{205A}mine",
        ] {
            let mut project = base.clone();
            let err = apply_patch(&mut project, add(reserved)).expect_err(reserved);
            assert_eq!(err.code, ErrorCode::BadModelName, "{reserved}");
            assert!(project == base, "{reserved}: the project is as it was");
        }

        // A name whose canonical form is free is added, as written.
        let mut project = base.clone();
        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![ProjectOperation::AddModel {
                    name: "Customer Growth 2".to_string(),
                }],
                models: vec![],
            },
        )
        .unwrap();
        assert_eq!(project.models[2].name, "Customer Growth 2");
    }

    #[test]
    fn upsert_module_adds_module_variable() {
        let mut project = TestProject::new("test").build_datamodel();

        // First add the submodel
        project.models.push(datamodel::Model {
            name: "submodel".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "output".to_string(),
                equation: Equation::Scalar("42".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat {
                    visibility: Visibility::Public,
                    ..datamodel::Compat::default()
                },
            })]
            .into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });

        let module = datamodel::Module {
            ident: "my_module".to_string(),
            model_name: "submodel".to_string(),
            documentation: "A test module".to_string(),
            units: None,
            references: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(100),
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertModule(module.clone())],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        match model.get_variable("my_module").unwrap() {
            Variable::Module(m) => {
                assert_eq!(m.model_name, "submodel");
                assert_eq!(m.documentation, "A test module");
                assert_eq!(m.uid, Some(100));
            }
            _ => panic!("expected module"),
        }
    }

    #[test]
    fn upsert_module_with_references() {
        let mut project = TestProject::new("test")
            .aux("local_input", "10", None)
            .build_datamodel();

        project.models.push(datamodel::Model {
            name: "submodel".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "input_var".to_string(),
                equation: Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat {
                    can_be_module_input: true,
                    visibility: Visibility::Public,
                    ..datamodel::Compat::default()
                },
            })]
            .into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });

        let module = datamodel::Module {
            ident: "my_module".to_string(),
            model_name: "submodel".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "local_input".to_string(),
                dst: "input_var".to_string(),
            }],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertModule(module)],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        match model.get_variable("my_module").unwrap() {
            Variable::Module(m) => {
                assert_eq!(m.references.len(), 1);
                assert_eq!(m.references[0].src, "local_input");
                assert_eq!(m.references[0].dst, "input_var");
            }
            _ => panic!("expected module"),
        }
    }

    /// A parent `main` with `local_input`, a `submodel` exposing an input port
    /// (`input_var`) and a public `output = input_var * 2`, and `main` holding
    /// `my_module` wired `local_input -> input_var` plus a `reader` of the
    /// module output. With the wiring intact `reader` simulates to 20.
    fn project_with_wired_module() -> datamodel::Project {
        let mut project = TestProject::new("test")
            .aux("local_input", "10", None)
            .build_datamodel();

        project.models[0]
            .variables
            .push(Variable::Aux(datamodel::Aux {
                ident: "reader".to_string(),
                equation: Equation::Scalar("my_module·output".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }));

        project.models.push(datamodel::Model {
            name: "submodel".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Aux(datamodel::Aux {
                    ident: "input_var".to_string(),
                    equation: Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        can_be_module_input: true,
                        visibility: Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                }),
                Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
                    equation: Equation::Scalar("input_var * 2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat {
                        visibility: Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                }),
            ]
            .into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });

        let module = datamodel::Module {
            ident: "my_module".to_string(),
            model_name: "submodel".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "local_input".to_string(),
                dst: "my_module·input_var".to_string(),
            }],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        };
        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::UpsertModule(module)],
                }],
            },
        )
        .unwrap();
        project
    }

    fn reader_value(project: &datamodel::Project) -> f64 {
        let series = TestProject::from_datamodel(project.clone()).vm_result("reader");
        *series.last().expect("reader produced no samples")
    }

    /// Regression for the asymmetric delete cleanup: `apply_delete_variable`
    /// pruned deleted flows from stock in/outflows and group members, but left
    /// module references whose `src` named the deleted variable -- a dangling
    /// dependency on a non-existent variable that made the whole project fail to
    /// compile with a confusing "missing variable" message.
    #[test]
    fn delete_variable_prunes_dangling_module_src() {
        let mut project = project_with_wired_module();
        assert!((reader_value(&project) - 20.0).abs() < 1e-6);

        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::DeleteVariable {
                        ident: "local_input".to_string(),
                    }],
                }],
            },
        )
        .unwrap();

        match project
            .get_model("main")
            .unwrap()
            .get_variable("my_module")
            .unwrap()
        {
            Variable::Module(m) => assert!(
                m.references.is_empty(),
                "deleted variable still wired as a module src: {:?}",
                m.references
            ),
            _ => panic!("expected module"),
        }

        // The project must still compile and simulate (the module is now
        // unwired, so its input falls back to the port default of 0).
        TestProject::from_datamodel(project.clone()).assert_compiles_incremental();
        assert!((reader_value(&project) - 0.0).abs() < 1e-6);
    }

    /// Regression for the cross-model rename gap: renaming a child model's input
    /// port left every parent module's `dst` pointing at the old name, so the
    /// parent silently fed the renamed port its default value (wrong numbers, no
    /// error). The rename must retarget parent module references too.
    #[test]
    fn rename_child_input_port_retargets_parent_module_dst() {
        let mut project = project_with_wired_module();
        assert!((reader_value(&project) - 20.0).abs() < 1e-6);

        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "submodel".to_string(),
                    ops: vec![ModelOperation::RenameVariable {
                        from: "input_var".to_string(),
                        to: "renamed_port".to_string(),
                    }],
                }],
            },
        )
        .unwrap();

        match project
            .get_model("main")
            .unwrap()
            .get_variable("my_module")
            .unwrap()
        {
            Variable::Module(m) => {
                assert_eq!(m.references.len(), 1);
                assert_eq!(
                    m.references[0].dst, "my_module·renamed_port",
                    "parent module dst did not follow the child input-port rename"
                );
            }
            _ => panic!("expected module"),
        }

        // The wiring must still carry local_input(10) -> renamed_port -> 20.
        assert!((reader_value(&project) - 20.0).abs() < 1e-6);
    }

    /// Renaming the MODULE VARIABLE itself must reprefix its own input
    /// references: `dst` is the module-qualified `{moduleIdent}·{port}` form, so
    /// after `my_module` -> `renamed_module` the engine rebuilds inputs under the
    /// `renamed_module·` prefix and would drop a stale `my_module·input_var`
    /// reference, silently unwiring the input. Regression for the Codex review
    /// finding on PR #807.
    #[test]
    fn rename_module_variable_retargets_its_own_dst_prefix() {
        // A minimal module-with-input fixture (no output reader, so the test is
        // not entangled with module-output reference renaming).
        let mut project = TestProject::new("test")
            .aux("local_input", "10", None)
            .build_datamodel();
        project.models.push(datamodel::Model {
            name: "submodel".to_string(),
            sim_specs: None,
            variables: vec![Variable::Aux(datamodel::Aux {
                ident: "input_var".to_string(),
                equation: Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat {
                    can_be_module_input: true,
                    visibility: Visibility::Public,
                    ..datamodel::Compat::default()
                },
            })]
            .into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });
        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::UpsertModule(datamodel::Module {
                        ident: "my_module".to_string(),
                        model_name: "submodel".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "local_input".to_string(),
                            dst: "my_module·input_var".to_string(),
                        }],
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    })],
                }],
            },
        )
        .unwrap();

        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::RenameVariable {
                        from: "my_module".to_string(),
                        to: "renamed_module".to_string(),
                    }],
                }],
            },
        )
        .unwrap();

        match project
            .get_model("main")
            .unwrap()
            .get_variable("renamed_module")
            .unwrap()
        {
            Variable::Module(m) => {
                assert_eq!(m.references.len(), 1);
                assert_eq!(
                    m.references[0].dst, "renamed_module·input_var",
                    "the module's own dst prefix did not follow the module-variable rename"
                );
            }
            _ => panic!("expected module"),
        }

        // The input must still resolve (the engine strips the new prefix and
        // wires local_input into the child port), so the project still compiles.
        TestProject::from_datamodel(project).assert_compiles_incremental();
    }

    /// `canonicalize_module` canonicalized only the module ident, leaving the
    /// reference endpoints verbatim -- so a non-canonical `src`/`dst` arriving
    /// via the FFI `apply_patch` (pysimlin `upsert_module`) disagreed with the
    /// canonical idents every UI/engine consumer compares against. Mirror
    /// `canonicalize_stock`'s inflow/outflow canonicalization.
    #[test]
    fn upsert_module_canonicalizes_references() {
        let mut project = TestProject::new("test").build_datamodel();
        project.models.push(datamodel::Model {
            name: "submodel".to_string(),
            sim_specs: None,
            variables: vec![].into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });

        let module = datamodel::Module {
            ident: "My Module".to_string(),
            model_name: "submodel".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "Local Input".to_string(),
                dst: "My Module·Input Var".to_string(),
            }],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        };

        apply_patch(
            &mut project,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::UpsertModule(module)],
                }],
            },
        )
        .unwrap();

        match project
            .get_model("main")
            .unwrap()
            .get_variable("my_module")
            .unwrap()
        {
            Variable::Module(m) => {
                assert_eq!(m.references[0].src, "local_input");
                assert_eq!(m.references[0].dst, "my_module·input_var");
            }
            _ => panic!("expected module"),
        }
    }

    #[test]
    fn upsert_module_replaces_existing() {
        let mut project = TestProject::new("test").build_datamodel();
        project.models.push(datamodel::Model {
            name: "submodel".to_string(),
            sim_specs: None,
            variables: vec![].into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });

        // Add initial module
        let initial_module = datamodel::Module {
            ident: "my_module".to_string(),
            model_name: "submodel".to_string(),
            documentation: "initial".to_string(),
            units: None,
            references: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        };
        let patch1 = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertModule(initial_module)],
            }],
        };
        apply_patch(&mut project, patch1).unwrap();

        // Now upsert with updated data
        let updated_module = datamodel::Module {
            ident: "my_module".to_string(),
            model_name: "submodel".to_string(),
            documentation: "updated".to_string(),
            units: Some("widgets".to_string()),
            references: vec![],
            compat: datamodel::Compat {
                can_be_module_input: true,
                visibility: Visibility::Public,
                ..datamodel::Compat::default()
            },
            ai_state: None,
            uid: Some(1),
        };
        let patch2 = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertModule(updated_module)],
            }],
        };
        apply_patch(&mut project, patch2).unwrap();

        let model = project.get_model("main").unwrap();
        match model.get_variable("my_module").unwrap() {
            Variable::Module(m) => {
                assert_eq!(m.documentation, "updated");
                assert_eq!(m.units, Some("widgets".to_string()));
                assert!(m.compat.can_be_module_input);
                assert_eq!(m.compat.visibility, Visibility::Public);
            }
            _ => panic!("expected module"),
        }
    }

    #[test]
    fn delete_module_variable() {
        let mut project = TestProject::new("test").build_datamodel();
        project.models.push(datamodel::Model {
            name: "submodel".to_string(),
            sim_specs: None,
            variables: vec![].into(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        });

        let module = datamodel::Module {
            ident: "my_module".to_string(),
            model_name: "submodel".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        };
        let add_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertModule(module)],
            }],
        };
        apply_patch(&mut project, add_patch).unwrap();
        assert!(
            project
                .get_model("main")
                .unwrap()
                .get_variable("my_module")
                .is_some()
        );

        let delete_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::DeleteVariable {
                    ident: "my_module".to_string(),
                }],
            }],
        };
        apply_patch(&mut project, delete_patch).unwrap();
        assert!(
            project
                .get_model("main")
                .unwrap()
                .get_variable("my_module")
                .is_none()
        );
    }

    #[test]
    fn add_model_and_module_in_same_patch() {
        let mut project = TestProject::new("test")
            .aux("driver", "100", None)
            .build_datamodel();

        let module = datamodel::Module {
            ident: "sub".to_string(),
            model_name: "new_submodel".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "driver".to_string(),
                dst: "input".to_string(),
            }],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        };

        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation::AddModel {
                name: "new_submodel".to_string(),
            }],
            models: vec![
                // Add a variable to the new submodel
                ModelPatch {
                    name: "new_submodel".to_string(),
                    ops: vec![ModelOperation::UpsertAux(datamodel::Aux {
                        ident: "input".to_string(),
                        equation: Equation::Scalar("0".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            can_be_module_input: true,
                            visibility: Visibility::Public,
                            ..datamodel::Compat::default()
                        },
                    })],
                },
                // Add the module reference to main
                ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::UpsertModule(module)],
                },
            ],
        };

        apply_patch(&mut project, patch).unwrap();

        // Verify submodel was created with variable
        let submodel = project.get_model("new_submodel").unwrap();
        assert!(submodel.get_variable("input").is_some());

        // Verify module was added to main
        let main = project.get_model("main").unwrap();
        match main.get_variable("sub").unwrap() {
            Variable::Module(m) => {
                assert_eq!(m.model_name, "new_submodel");
                assert_eq!(m.references.len(), 1);
            }
            _ => panic!("expected module"),
        }
    }

    #[test]
    fn patch_rollback_on_error() {
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .build_datamodel();

        // Try a patch that adds a variable then operates on a nonexistent model
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![
                ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::UpsertAux(datamodel::Aux {
                        ident: "y".to_string(),
                        equation: Equation::Scalar("2".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    })],
                },
                ModelPatch {
                    name: "nonexistent_model".to_string(),
                    ops: vec![ModelOperation::DeleteVariable {
                        ident: "z".to_string(),
                    }],
                },
            ],
        };

        let result = apply_patch(&mut project, patch);
        assert!(result.is_err());

        // Project should be unchanged (rollback)
        let model = project.get_model("main").unwrap();
        assert!(
            model.get_variable("y").is_none(),
            "y should not have been added on error"
        );
        assert!(model.get_variable("x").is_some(), "x should still exist");
    }

    #[test]
    fn add_model_preserves_display_name() {
        let mut project = TestProject::new("test").build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation::AddModel {
                name: "Customer Growth".to_string(),
            }],
            models: vec![],
        };

        apply_patch(&mut project, patch).unwrap();
        assert_eq!(project.models.len(), 2);
        // The model should be stored with its display name, not canonicalized
        assert_eq!(project.models[1].name, "Customer Growth");
        // And we should be able to find it by its display name
        assert!(project.get_model("Customer Growth").is_some());
    }

    #[test]
    fn add_model_and_operate_on_it_in_same_patch_with_display_name() {
        let mut project = TestProject::new("test").build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation::AddModel {
                name: "Customer Growth".to_string(),
            }],
            models: vec![ModelPatch {
                name: "Customer Growth".to_string(),
                ops: vec![ModelOperation::UpsertAux(datamodel::Aux {
                    ident: "growth_rate".to_string(),
                    equation: Equation::Scalar("0.05".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                })],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("Customer Growth").unwrap();
        assert!(model.get_variable("growth_rate").is_some());
    }

    #[test]
    fn rename_updates_compat_active_initial_on_aux() {
        let mut project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    Variable::Aux(datamodel::Aux {
                        ident: "base_rate".to_string(),
                        equation: Equation::Scalar("10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    Variable::Aux(datamodel::Aux {
                        ident: "adjusted".to_string(),
                        equation: Equation::Scalar("base_rate * 2".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            active_initial: Some("base_rate * 3".to_string()),
                            ..datamodel::Compat::default()
                        },
                    }),
                ]
                .into(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "base_rate".to_string(),
                    to: "initial_rate".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("adjusted").unwrap() {
            Variable::Aux(aux) => {
                match &aux.equation {
                    Equation::Scalar(eqn) => assert_eq!(eqn, "initial_rate * 2"),
                    _ => panic!("expected scalar equation"),
                }
                assert_eq!(
                    aux.compat.active_initial.as_deref(),
                    Some("initial_rate * 3"),
                );
            }
            _ => panic!("expected auxiliary variable"),
        }
    }

    #[test]
    fn rename_updates_compat_active_initial_on_flow() {
        let mut project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: vec![],
            units: vec![],
            models: vec![datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    Variable::Aux(datamodel::Aux {
                        ident: "capacity".to_string(),
                        equation: Equation::Scalar("100".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    Variable::Flow(datamodel::Flow {
                        ident: "production".to_string(),
                        equation: Equation::Scalar("capacity / 10".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat {
                            active_initial: Some("capacity / 5".to_string()),
                            ..datamodel::Compat::default()
                        },
                    }),
                ]
                .into(),
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            }],
            source: None,
            ai_information: None,
        };

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "capacity".to_string(),
                    to: "max_capacity".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("production").unwrap() {
            Variable::Flow(flow) => {
                match &flow.equation {
                    Equation::Scalar(eqn) => assert_eq!(eqn, "max_capacity / 10"),
                    _ => panic!("expected scalar equation"),
                }
                assert_eq!(
                    flow.compat.active_initial.as_deref(),
                    Some("max_capacity / 5"),
                );
            }
            _ => panic!("expected flow variable"),
        }
    }

    #[test]
    fn rename_preserves_none_compat_active_initial() {
        let mut project = TestProject::new("test")
            .aux("old_name", "42", None)
            .aux("consumer", "old_name + 1", None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "old_name".to_string(),
                    to: "new_name".to_string(),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        match model.get_variable("consumer").unwrap() {
            Variable::Aux(aux) => {
                match &aux.equation {
                    Equation::Scalar(eqn) => assert_eq!(eqn, "new_name + 1"),
                    _ => panic!("expected scalar equation"),
                }
                assert!(aux.compat.active_initial.is_none());
            }
            _ => panic!("expected auxiliary variable"),
        }
    }

    #[test]
    fn upsert_preserves_existing_uid_when_replacement_has_none() {
        let mut project = TestProject::new("test")
            .stock_with_options(
                "population",
                "100",
                &[],
                &[],
                None,
                "",
                false,
                false,
                Visibility::Private,
                Some(42),
            )
            .build_datamodel();

        let replacement = datamodel::Stock {
            ident: "population".to_string(),
            equation: Equation::Scalar("200".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertStock(replacement)],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        match model.get_variable("population").unwrap() {
            Variable::Stock(stock) => {
                assert_eq!(stock.equation, Equation::Scalar("200".to_string()));
                assert_eq!(stock.uid, Some(42));
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn upsert_new_variable_gets_uid_assigned() {
        // New variables that arrive without a UID must have one assigned so that
        // SetLoopName can reference them later. With no existing variables, the
        // first assigned UID should be 1 (0 + 1).
        let mut project = TestProject::new("test").build_datamodel();

        let stock = datamodel::Stock {
            ident: "brand_new".to_string(),
            equation: Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertStock(stock)],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        match model.get_variable("brand_new").unwrap() {
            Variable::Stock(stock) => {
                assert!(stock.uid.is_some(), "new variable must receive a UID");
                assert_eq!(stock.uid, Some(1));
            }
            _ => panic!("expected stock"),
        }
    }

    #[test]
    fn upsert_new_variable_uid_increments_past_existing_max() {
        // When the model already has variables with UIDs, the new UID must be
        // max_existing_uid + 1 to avoid collisions.
        let mut project = TestProject::new("test")
            .aux("existing", "1", None)
            .build_datamodel();

        // Give the existing variable a high UID (99) so we can verify the next
        // inserted variable gets uid = 100.
        assign_uids(&mut project, "main", &[("existing", 99)]);

        let aux = datamodel::Aux {
            ident: "new_aux".to_string(),
            equation: Equation::Scalar("42".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertAux(aux)],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        match model.get_variable("new_aux").unwrap() {
            Variable::Aux(a) => {
                assert_eq!(a.uid, Some(100), "new UID should be max+1 = 100");
            }
            _ => panic!("expected aux"),
        }
    }

    #[test]
    fn set_loop_name_with_duplicate_variable_names_deduplicates() {
        // ReadModel returns loops with the first variable repeated at the end
        // (e.g., ["population", "births", "population"]). When the client passes
        // that list directly to SetLoopName, the duplicate must be stripped.
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .aux("y", "x", None)
            .build_datamodel();
        assign_uids(&mut project, "main", &[("x", 1), ("y", 2)]);

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["x".to_string(), "y".to_string(), "x".to_string()],
                    name: "loop".to_string(),
                    description: None,
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert_eq!(model.loop_metadata.len(), 1);
        let lm = &model.loop_metadata[0];
        // UIDs should be [1, 2] (sorted), not [1, 1, 2]
        assert_eq!(lm.uids, vec![1, 2]);
    }

    #[test]
    fn set_loop_name_duplicate_and_non_duplicate_match_same_entry() {
        // ["x", "y", "x"] (with duplicate) and ["x", "y"] (without) must resolve
        // to the same LoopMetadata entry so that re-calling SetLoopName updates
        // rather than creates a second entry.
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .aux("y", "x", None)
            .build_datamodel();
        assign_uids(&mut project, "main", &[("x", 1), ("y", 2)]);

        // First call with closing duplicate
        let patch1 = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["x".to_string(), "y".to_string(), "x".to_string()],
                    name: "first name".to_string(),
                    description: None,
                }],
            }],
        };
        apply_patch(&mut project, patch1).unwrap();

        // Second call without duplicate
        let patch2 = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["x".to_string(), "y".to_string()],
                    name: "second name".to_string(),
                    description: None,
                }],
            }],
        };
        apply_patch(&mut project, patch2).unwrap();

        let model = project.get_model("main").unwrap();
        assert_eq!(
            model.loop_metadata.len(),
            1,
            "should update the same entry, not create a second one"
        );
        assert_eq!(model.loop_metadata[0].name, "second name");
    }

    #[test]
    fn set_loop_name_revives_a_previously_deleted_entry() {
        // A LoopMetadata can be soft-deleted (a user removing a loop name, or a
        // deserialized project carrying `deleted: true`). Re-naming the same
        // variable set via SetLoopName means "name/pin this loop" and must REVIVE
        // the entry (clear `deleted`); otherwise the consumers that filter out
        // deleted entries -- pinned-loop scoring (`pinned_loops_from_datamodel`)
        // and the loop-name display (`build_uid_to_loop_name`) -- silently ignore
        // it, so the user's re-pin has no effect.
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .aux("y", "x", None)
            .build_datamodel();
        assign_uids(&mut project, "main", &[("x", 1), ("y", 2)]);

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["x".to_string(), "y".to_string()],
                    name: "loop".to_string(),
                    description: None,
                }],
            }],
        };
        apply_patch(&mut project, patch.clone()).unwrap();

        // Soft-delete the entry, as a serialized/UI deletion would.
        let model = project
            .models
            .iter_mut()
            .find(|m| m.name == "main")
            .unwrap();
        model.loop_metadata[0].deleted = true;

        // Re-pinning the same variable set must revive the entry.
        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        assert_eq!(
            model.loop_metadata.len(),
            1,
            "should update the existing entry, not create a second one"
        );
        assert!(
            !model.loop_metadata[0].deleted,
            "SetLoopName must revive a previously-deleted entry so it is scored/displayed again"
        );
    }

    #[test]
    fn set_loop_name_on_uid_assigned_variable_works() {
        // Variables added via upsert without explicit UIDs now get UIDs assigned.
        // SetLoopName must be able to reference them.
        let mut project = TestProject::new("test").build_datamodel();

        // Add two variables without UIDs; upsert_variable should assign them.
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![
                    ModelOperation::UpsertAux(datamodel::Aux {
                        ident: "population".to_string(),
                        equation: Equation::Scalar("births".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    ModelOperation::UpsertAux(datamodel::Aux {
                        ident: "births".to_string(),
                        equation: Equation::Scalar("population * 0.03".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                    ModelOperation::SetLoopName {
                        variables: vec!["population".to_string(), "births".to_string()],
                        name: "growth loop".to_string(),
                        description: None,
                    },
                ],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert_eq!(
            model.loop_metadata.len(),
            1,
            "SetLoopName should succeed on patch-added variables"
        );
        let lm = &model.loop_metadata[0];
        assert_eq!(lm.name, "growth loop");
        assert_eq!(lm.uids.len(), 2, "loop should reference both variable UIDs");
    }

    #[test]
    fn set_loop_name_mints_uids_for_uidless_variables() {
        // Vensim/MDL- and SD-AI-imported models carry no variable UIDs at all.
        // Pinning a loop is exactly the operation that needs UIDs, so SetLoopName
        // must mint them on demand instead of failing with "has no UID" -- without
        // this, loop pinning is unusable on every imported model.
        let mut project = TestProject::new("test")
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * 0.02", None)
            .build_datamodel();
        // Deliberately NO assign_uids call: every variable has uid == None,
        // exactly like an MDL import.

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["population".to_string(), "births".to_string()],
                    name: "growth".to_string(),
                    description: None,
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        assert_eq!(model.loop_metadata.len(), 1);
        let lm = &model.loop_metadata[0];
        assert_eq!(lm.name, "growth");
        assert_eq!(lm.uids.len(), 2);

        // The referenced variables must now carry the minted UIDs, and the
        // metadata entry must reference exactly those UIDs (sorted).
        let pop_uid = variable_uid(model.get_variable("population").unwrap())
            .expect("population should have a minted UID");
        let births_uid = variable_uid(model.get_variable("births").unwrap())
            .expect("births should have a minted UID");
        assert_ne!(pop_uid, births_uid, "minted UIDs must be unique");
        let mut expected = vec![pop_uid, births_uid];
        expected.sort_unstable();
        assert_eq!(lm.uids, expected);
    }

    #[test]
    fn set_loop_name_minted_uids_skip_existing_max() {
        // When some variables already carry UIDs, freshly-minted ones must not
        // collide with them (or with each other).
        let mut project = TestProject::new("test")
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * 0.02", None)
            .aux("rate", "0.02", None)
            .build_datamodel();
        assign_uids(&mut project, "main", &[("rate", 7)]);

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["population".to_string(), "births".to_string()],
                    name: "growth".to_string(),
                    description: None,
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        let pop_uid = variable_uid(model.get_variable("population").unwrap()).unwrap();
        let births_uid = variable_uid(model.get_variable("births").unwrap()).unwrap();
        let rate_uid = variable_uid(model.get_variable("rate").unwrap()).unwrap();
        assert_eq!(rate_uid, 7, "pre-existing UID must be preserved");
        assert!(
            pop_uid > 7 && births_uid > 7,
            "minted UIDs ({pop_uid}, {births_uid}) must be greater than the existing max (7)"
        );
        assert_ne!(pop_uid, births_uid);

        let lm = &model.loop_metadata[0];
        let mut expected = vec![pop_uid, births_uid];
        expected.sort_unstable();
        assert_eq!(lm.uids, expected);
    }

    #[test]
    fn test_is_view_only_patch() {
        let project = TestProject::new("test").build_datamodel();
        let view_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertView {
                    index: 0,
                    view: datamodel::View::StockFlow(datamodel::StockFlow {
                        name: None,
                        elements: vec![].into(),
                        view_box: Default::default(),
                        zoom: 1.0,
                        use_lettered_polarity: false,
                        font: None,
                        sketch_compat: None,
                    }),
                }],
            }],
        };
        assert!(is_view_only_patch(&project, &view_patch));

        let mixed_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![
                    ModelOperation::UpsertView {
                        index: 0,
                        view: datamodel::View::StockFlow(datamodel::StockFlow {
                            name: None,
                            elements: vec![].into(),
                            view_box: Default::default(),
                            zoom: 1.0,
                            use_lettered_polarity: false,
                            font: None,
                            sketch_compat: None,
                        }),
                    },
                    ModelOperation::UpsertAux(datamodel::Aux {
                        ident: "x".to_string(),
                        equation: datamodel::Equation::Scalar("1".to_string()),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: Default::default(),
                    }),
                ],
            }],
        };
        assert!(!is_view_only_patch(&project, &mixed_patch));

        let empty_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![],
        };
        assert!(is_view_only_patch(&project, &empty_patch));

        let project_op_patch = ProjectPatch {
            project_ops: vec![ProjectOperation::AddModel {
                name: "test".to_string(),
            }],
            models: vec![],
        };
        assert!(!is_view_only_patch(&project, &project_op_patch));
    }

    /// Helper to set UIDs on variables in a built datamodel, since TestProject
    /// doesn't assign them.
    fn assign_uids(project: &mut datamodel::Project, model_name: &str, uids: &[(&str, i32)]) {
        let model = project.get_model_mut(model_name).unwrap();
        for (ident, uid) in uids {
            let var = model.get_variable_mut(ident).unwrap();
            set_uid(var, Some(*uid));
        }
    }

    #[test]
    fn set_loop_name_creates_loop_metadata() {
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .aux("y", "x", None)
            .build_datamodel();
        assign_uids(&mut project, "main", &[("x", 10), ("y", 20)]);

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["x".to_string(), "y".to_string()],
                    name: "reinforcing loop".to_string(),
                    description: Some("test loop".to_string()),
                }],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();
        assert_eq!(model.loop_metadata.len(), 1);
        let lm = &model.loop_metadata[0];
        assert_eq!(lm.uids, vec![10, 20]);
        assert_eq!(lm.name, "reinforcing loop");
        assert_eq!(lm.description, "test loop");
        assert!(!lm.deleted);
    }

    #[test]
    fn set_loop_name_updates_existing_loop() {
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .aux("y", "x", None)
            .build_datamodel();
        assign_uids(&mut project, "main", &[("x", 10), ("y", 20)]);

        // First SetLoopName creates the entry
        let patch1 = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["x".to_string(), "y".to_string()],
                    name: "old name".to_string(),
                    description: None,
                }],
            }],
        };
        apply_patch(&mut project, patch1).unwrap();

        // Second SetLoopName with same variables (different order) updates
        let patch2 = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["y".to_string(), "x".to_string()],
                    name: "new name".to_string(),
                    description: Some("updated".to_string()),
                }],
            }],
        };
        apply_patch(&mut project, patch2).unwrap();

        let model = project.get_model("main").unwrap();
        assert_eq!(model.loop_metadata.len(), 1, "should update, not duplicate");
        let lm = &model.loop_metadata[0];
        assert_eq!(lm.name, "new name");
        assert_eq!(lm.description, "updated");
    }

    #[test]
    fn set_loop_name_unknown_variable_returns_error() {
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .build_datamodel();
        assign_uids(&mut project, "main", &[("x", 10)]);

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec!["x".to_string(), "nonexistent".to_string()],
                    name: "loop".to_string(),
                    description: None,
                }],
            }],
        };

        let err = apply_patch(&mut project, patch).unwrap_err();
        assert_eq!(err.code, ErrorCode::DoesNotExist);
    }

    #[test]
    fn set_loop_name_empty_variables_returns_error() {
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .build_datamodel();
        assign_uids(&mut project, "main", &[("x", 10)]);

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: vec![],
                    name: "loop".to_string(),
                    description: None,
                }],
            }],
        };

        let err = apply_patch(&mut project, patch).unwrap_err();
        assert_eq!(err.code, ErrorCode::Generic);
    }

    /// Datamodel `ident` fields hold the human-facing display name; every
    /// consumer canonicalizes at lookup time. Upserting must therefore store
    /// the caller's spelling verbatim -- casing, spaces, and XMILE `\n` line
    /// breaks included (GH #890).
    #[test]
    fn upsert_preserves_display_name_spelling() {
        let mut project = TestProject::new("test").build_datamodel();
        let aux = datamodel::Aux {
            ident: "Total Students".to_string(),
            equation: Equation::Scalar("1".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        };
        let flow = datamodel::Flow {
            ident: "testing\\nassymptomatic".to_string(),
            equation: Equation::Scalar("2".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![
                    ModelOperation::UpsertAux(aux),
                    ModelOperation::UpsertFlow(flow),
                ],
            }],
        };

        apply_patch(&mut project, patch).unwrap();
        let model = project.get_model("main").unwrap();

        // Stored spelling is the caller's display form...
        let var = model.get_variable("total_students").unwrap();
        assert_eq!(var.get_ident(), "Total Students");
        let var = model.get_variable("testing_assymptomatic").unwrap();
        assert_eq!(var.get_ident(), "testing\\nassymptomatic");

        // ...and lookups by any spelling variant still resolve.
        assert!(model.get_variable("Total Students").is_some());
        assert!(model.get_variable("TOTAL_STUDENTS").is_some());
        assert!(model.get_variable("testing\\nassymptomatic").is_some());
    }

    /// Upserting a variable whose name canonicalizes to an existing variable's
    /// ident replaces that variable (no duplicate), and the stored spelling
    /// follows the upsert payload -- the payload is authoritative for the
    /// display form, just as it is for every other field.
    #[test]
    fn upsert_matches_existing_by_canonical_ident() {
        let mut project = TestProject::new("test")
            .stock("Students", "100", &[], &[], None)
            .build_datamodel();

        let make_stock = |ident: &str| datamodel::Stock {
            ident: ident.to_string(),
            equation: Equation::Scalar("100".to_string()),
            documentation: "cohort pipeline".to_string(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        };
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertStock(make_stock("Students"))],
            }],
        };
        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        assert_eq!(model.variables.len(), 1, "upsert must not duplicate");
        match model.get_variable("students").unwrap() {
            Variable::Stock(s) => {
                assert_eq!(s.ident, "Students");
                assert_eq!(s.documentation, "cohort pipeline");
            }
            _ => panic!("expected stock"),
        }

        // A canonically-equal but differently-spelled upsert also matches,
        // and restamps the display form from the payload.
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertStock(make_stock("STUDENTS"))],
            }],
        };
        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        assert_eq!(model.variables.len(), 1);
        assert_eq!(
            model.get_variable("students").unwrap().get_ident(),
            "STUDENTS"
        );
    }

    /// Renaming stores the new name's display form verbatim while every
    /// reference (equations, stock in/outflows) is rewritten canonically.
    #[test]
    fn rename_stores_display_form() {
        let mut project = TestProject::new("test")
            .flow("flow", "1", None)
            .aux("watcher", "flow * 2", None)
            .stock("stock", "0", &["flow"], &["flow"], None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "flow".to_string(),
                    to: "Enrollment Rate".to_string(),
                }],
            }],
        };
        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        assert!(model.get_variable("flow").is_none());
        assert_eq!(
            model.get_variable("enrollment_rate").unwrap().get_ident(),
            "Enrollment Rate"
        );
        match model.get_variable("watcher").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                Equation::Scalar(eqn) => assert_eq!(eqn, "enrollment_rate * 2"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected aux"),
        }
        match model.get_variable("stock").unwrap() {
            Variable::Stock(stock) => {
                assert_eq!(stock.inflows, vec!["enrollment_rate".to_string()]);
                assert_eq!(stock.outflows, vec!["enrollment_rate".to_string()]);
            }
            _ => panic!("expected stock"),
        }
    }

    /// A rename whose old and new names canonicalize identically only changes
    /// the display spelling: no equation or reference rewrites are needed
    /// because every reference resolves through canonicalization.
    #[test]
    fn rename_case_only_updates_display_name() {
        let mut project = TestProject::new("test")
            .aux("students", "1", None)
            .aux("watcher", "students * 2", None)
            .build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "students".to_string(),
                    to: "Students".to_string(),
                }],
            }],
        };
        apply_patch(&mut project, patch).unwrap();

        let model = project.get_model("main").unwrap();
        assert_eq!(
            model.get_variable("students").unwrap().get_ident(),
            "Students"
        );
        match model.get_variable("watcher").unwrap() {
            Variable::Aux(aux) => match &aux.equation {
                Equation::Scalar(eqn) => assert_eq!(eqn, "students * 2"),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected aux"),
        }
    }

    /// A same-name rename of an existing variable is an accepted no-op.
    #[test]
    fn rename_identity_is_noop() {
        let mut project = TestProject::new("test")
            .aux("x", "1", None)
            .build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "x".to_string(),
                    to: "x".to_string(),
                }],
            }],
        };
        apply_patch(&mut project, patch).unwrap();
        assert!(
            project
                .get_model("main")
                .unwrap()
                .get_variable("x")
                .is_some()
        );
    }

    /// A display-only rename of a variable that does not exist is an error,
    /// consistent with every other rename.
    #[test]
    fn rename_case_only_missing_variable_errors() {
        let mut project = TestProject::new("test").build_datamodel();
        let patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::RenameVariable {
                    from: "nope".to_string(),
                    to: "Nope".to_string(),
                }],
            }],
        };
        let err = apply_patch(&mut project, patch).unwrap_err();
        assert_eq!(err.code, ErrorCode::DoesNotExist);
    }
}
