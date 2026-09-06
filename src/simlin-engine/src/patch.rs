// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::{Expr0, Expr2, IndexExpr0, IndexExpr2, print_eqn};
use crate::builtins::{BuiltinFn, UntypedBuiltinFn};
use crate::canonicalize;
use crate::common::{Canonical, Error, ErrorCode, ErrorKind, Ident, RawIdent, Result};
use crate::datamodel::{self, Variable};
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
    SetSimSpecs(datamodel::SimSpecs),
    SetSource(datamodel::Source),
    AddModel { name: String },
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
}

/// Returns true when the patch only touches views (UpsertView/DeleteView)
/// and has no project-level operations. View-only patches don't affect
/// equations, variables, or simulation, so callers can skip recompilation.
pub fn is_view_only_patch(patch: &ProjectPatch) -> bool {
    if !patch.project_ops.is_empty() {
        return false;
    }
    patch.models.iter().all(|mp| {
        mp.ops.iter().all(|op| {
            matches!(
                op,
                ModelOperation::UpsertView { .. } | ModelOperation::DeleteView { .. }
            )
        })
    })
}

pub fn apply_patch(project: &mut datamodel::Project, patch: ProjectPatch) -> Result<()> {
    let mut staged = project.clone();

    // Apply project-level operations first
    for project_op in patch.project_ops {
        match project_op {
            ProjectOperation::SetSimSpecs(sim_specs) => {
                staged.sim_specs = sim_specs;
            }
            ProjectOperation::SetSource(source) => {
                staged.source = Some(source);
            }
            ProjectOperation::AddModel { name } => {
                apply_add_model(&mut staged, name)?;
            }
        }
    }

    // Then apply model-level operations
    for model_patch in patch.models {
        for op in model_patch.ops {
            match op {
                ModelOperation::RenameVariable { from, to } => {
                    apply_rename_variable(&mut staged, &model_patch.name, &from, &to)?;
                }
                _ => {
                    let model = get_model_mut(&mut staged, &model_patch.name)?;
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
                        ModelOperation::RenameVariable { .. } => unreachable!(),
                    }
                }
            }
        }
    }

    *project = staged;
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
    for inflow in stock.inflows.iter_mut() {
        canonicalize_ident(inflow);
    }
    stock.inflows.sort_unstable();
    for outflow in stock.outflows.iter_mut() {
        canonicalize_ident(outflow);
    }
    stock.outflows.sort_unstable();
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
    if let Some(existing) = model.get_variable_mut(&ident) {
        // View elements reference variables by UID, so preserve the existing
        // UID when the replacement doesn't specify one.
        if get_uid(&variable).is_none() {
            set_uid(&mut variable, get_uid(existing));
        }
        *existing = variable;
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

fn get_model_mut<'a>(
    project: &'a mut datamodel::Project,
    model_name: &str,
) -> Result<&'a mut datamodel::Model> {
    project.get_model_mut(model_name).ok_or_else(|| {
        Error::new(
            ErrorKind::Model,
            ErrorCode::BadModelName,
            Some(model_name.to_string()),
        )
    })
}

fn apply_add_model(project: &mut datamodel::Project, name: String) -> Result<()> {
    // Check if a model with this name already exists.
    // Model names are stored and looked up as-is (no canonicalization),
    // consistent with XMILE/JSON import and the C FFI simlin_project_add_model.
    if project.get_model(&name).is_some() {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DuplicateVariable,
            Some(format!("model '{}' already exists", name)),
        ));
    }
    project.models.push(datamodel::Model {
        name,
        sim_specs: None,
        variables: vec![],
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

    let stock = model
        .variables
        .iter_mut()
        .find_map(|var| {
            if let Variable::Stock(stock) = var
                && canonicalize(stock.ident.as_str()) == ident
            {
                return Some(stock);
            }
            None
        })
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Model,
                ErrorCode::DoesNotExist,
                Some(format!("stock '{}' not found", ident_str)),
            )
        })?;

    stock.inflows = inflows
        .iter()
        .map(|s| canonicalize(s).into_owned())
        .collect();
    stock.outflows = outflows
        .iter()
        .map(|s| canonicalize(s).into_owned())
        .collect();
    stock.inflows.sort_unstable();
    stock.outflows.sort_unstable();

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
    let mut seen = std::collections::HashSet::new();
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
        let var = model.get_variable_mut(var_name).ok_or_else(|| {
            Error::new(
                ErrorKind::Model,
                ErrorCode::DoesNotExist,
                Some(format!("variable '{}' not found", var_name)),
            )
        })?;
        let uid = match get_uid(var) {
            Some(uid) => uid,
            None => {
                let minted = next_uid;
                set_uid(var, Some(minted));
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
    let Some(pos) = model
        .variables
        .iter()
        .position(|var| canonicalize(var.get_ident()) == ident)
    else {
        return Err(Error::new(ErrorKind::Model, ErrorCode::DoesNotExist, None));
    };

    let removed = model.variables.remove(pos);
    if let Variable::Flow(flow) = removed {
        let flow_ident = canonicalize(flow.ident.as_str());
        for var in model.variables.iter_mut() {
            if let Variable::Stock(stock) = var {
                stock
                    .inflows
                    .retain(|name| canonicalize(name.as_str()) != flow_ident);
                stock
                    .outflows
                    .retain(|name| canonicalize(name.as_str()) != flow_ident);
            }
        }
    }

    // Drop any module input wiring whose `src` named the deleted variable.
    // Mirrors the stock-flow and group-member cleanup above: a left-behind
    // `src` becomes a dependency on a non-existent variable, making the whole
    // project fail to compile with a confusing "missing variable" message. The
    // rename path already rewrites module references; the delete path was the
    // asymmetric gap.
    for var in model.variables.iter_mut() {
        if let Variable::Module(module) = var {
            module
                .references
                .retain(|reference| canonicalize(reference.src.as_str()) != ident);
        }
    }

    for group in model.groups.iter_mut() {
        group
            .members
            .retain(|name| canonicalize(name.as_str()) != ident);
    }

    Ok(())
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
    // function reprints every dependent equation, so it would persist
    // `c = "x"y" + 1` into the datamodel and the previously-valid model would
    // stop compiling with `UnclosedQuotedIdent` -- the same silent, saved
    // corruption GH #976 fixed for keyword names, through this same entry point.
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

    if old_ident == new_ident {
        // Canonically-identical rename: only the display spelling changes
        // (e.g. "students" -> "Students"). Every reference resolves through
        // canonicalization, so no equation or reference rewrites are needed --
        // just restamp the stored display name.
        let model = get_model_mut(project, model_name)?;
        let var = model
            .get_variable_mut(from)
            .ok_or_else(|| Error::new(ErrorKind::Model, ErrorCode::DoesNotExist, None))?;
        if var.get_ident() != to {
            var.set_ident(to.to_string());
        }
        return Ok(());
    }

    let model = get_model_mut(project, model_name)?;

    if model.get_variable(new_ident.as_str()).is_some() {
        return Err(Error::new(
            ErrorKind::Model,
            ErrorCode::DuplicateVariable,
            None,
        ));
    }

    let (target_index, is_flow) = model
        .variables
        .iter()
        .enumerate()
        .find_map(|(idx, var)| {
            if canonicalize(var.get_ident()) == old_ident.as_str() {
                Some((idx, matches!(var, Variable::Flow(_))))
            } else {
                None
            }
        })
        .ok_or_else(|| Error::new(ErrorKind::Model, ErrorCode::DoesNotExist, None))?;

    rename_model_equations(model, &old_ident, &new_ident);

    if is_flow {
        update_stock_flow_references(model, &old_ident, &new_ident);
    }

    rename_module_references(model, &old_ident, &new_ident);
    rename_group_members(model, &old_ident, &new_ident);

    if let Some(var) = model.variables.get_mut(target_index) {
        // Store the caller's display spelling verbatim (ident fields hold
        // display names; see the comment above `canonicalize_stock_references`).
        // References below are rewritten with the canonical `new_ident`.
        var.set_ident(to.to_string());

        // If the renamed variable is itself a module instance, its OWN input
        // references carry the module-qualified `{old}·{port}` dst prefix. The
        // engine rebuilds inputs under the new `{new}·` prefix and would drop a
        // stale `{old}·port` reference, silently unwiring every input. Reprefix
        // them so the wiring survives renaming the module variable.
        if let Variable::Module(module) = var {
            let old_prefix = format!("{}\u{00B7}", old_ident.as_str());
            let new_prefix = format!("{}\u{00B7}", new_ident.as_str());
            for reference in module.references.iter_mut() {
                let dst_canonical = canonicalize(reference.dst.as_str());
                if let Some(port) = dst_canonical.strip_prefix(old_prefix.as_str()) {
                    reference.dst = format!("{new_prefix}{port}");
                }
            }
        }
    }

    // Cross-model fix-up: the variable just renamed may be a module INPUT PORT.
    // Every PARENT module that instantiates this model wires into the OLD port
    // name via its `dst` (the module-qualified `module·port` form), and those
    // references live in OTHER models that the single-model rename above never
    // visits. Without retargeting them the parent silently feeds the renamed
    // port its default value -- wrong numbers, no error.
    retarget_parent_module_dst(project, model_name, &old_ident, &new_ident);

    Ok(())
}

/// Retarget the `dst` of every parent module reference that wires into the
/// just-renamed input port of `target_model_name`.
///
/// A module reference `dst` is the canonical `{module_ident}·{port}` form (see
/// `build_module_inputs`). For each module instance pointing at
/// `target_model_name`, rewrite a reference whose port suffix names `old_ident`
/// to name `new_ident`. Gated on the module's target model so an unrelated model
/// with a like-named variable is untouched.
fn retarget_parent_module_dst(
    project: &mut datamodel::Project,
    target_model_name: &str,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    let target_canonical = canonicalize(target_model_name);
    for model in project.models.iter_mut() {
        for var in model.variables.iter_mut() {
            let Variable::Module(module) = var else {
                continue;
            };
            if canonicalize(module.model_name.as_str()) != target_canonical {
                continue;
            }
            let prefix = format!("{}\u{00B7}", canonicalize(module.ident.as_str()));
            for reference in module.references.iter_mut() {
                let dst_canonical = canonicalize(reference.dst.as_str());
                let Some(port) = dst_canonical.strip_prefix(prefix.as_str()) else {
                    continue;
                };
                if canonicalize(port).as_ref() == old_ident.as_str() {
                    reference.dst = format!("{prefix}{}", new_ident.as_str());
                }
            }
        }
    }
}

/// Rewrite every equation of `model` that references `old_ident`.
///
/// A rename is SYNTACTIC: each equation string is parsed as written
/// (`Expr0::new`, the parser alone), renamed, and printed back, so what the
/// user wrote is what comes back with one name changed. Neither compiler tier
/// can do that: the parse memo's tree is the EXPANDED one -- a `SMTH1(x, 3)`
/// is already an instance read and a `PREVIOUS(x + 1)` a capture, so printing
/// it back would replace the call with the helper's name -- and the lowered
/// tree is absent for an equation the compiler refuses, which would leave the
/// old name in place and turn the refusal into an unknown dependency. A string
/// that does not parse, or names nothing renamed, is left exactly as written
/// (`patch::tests::rename_rewrites_an_equation_the_lowering_refuses`,
/// `rename_keeps_a_module_function_call_as_written`).
fn rename_model_equations(
    model: &mut datamodel::Model,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    for var in model.variables.iter_mut() {
        match var {
            Variable::Stock(stock) => rename_equation(&mut stock.equation, old_ident, new_ident),
            Variable::Flow(flow) => {
                rename_equation(&mut flow.equation, old_ident, new_ident);
                rename_active_initial(&mut flow.compat, old_ident, new_ident);
            }
            Variable::Aux(aux) => {
                rename_equation(&mut aux.equation, old_ident, new_ident);
                rename_active_initial(&mut aux.compat, old_ident, new_ident);
            }
            Variable::Module(_) => {}
        }
    }
}

/// Every equation string a `datamodel::Equation` holds: the scalar or
/// apply-to-all text, and an arrayed equation's per-element texts, per-element
/// initial texts and default.
fn rename_equation(
    equation: &mut datamodel::Equation,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    match equation {
        datamodel::Equation::Scalar(text) | datamodel::Equation::ApplyToAll(_, text) => {
            rename_text(text, old_ident, new_ident);
        }
        datamodel::Equation::Arrayed(_, elements, default_eq, _) => {
            for (_, text, initial, _) in elements.iter_mut() {
                rename_text(text, old_ident, new_ident);
                if let Some(initial) = initial.as_mut() {
                    rename_text(initial, old_ident, new_ident);
                }
            }
            if let Some(default_eq) = default_eq.as_mut() {
                rename_text(default_eq, old_ident, new_ident);
            }
        }
    }
}

fn rename_active_initial(
    compat: &mut datamodel::Compat,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    if let Some(text) = compat.active_initial.as_mut() {
        rename_text(text, old_ident, new_ident);
    }
}

/// One equation string: parsed as written, renamed, and printed back only if
/// a reference changed. An empty or unparseable string is left as it is; the
/// parse errors are the variable's own diagnostics, reported by the compile.
fn rename_text(text: &mut String, old_ident: &Ident<Canonical>, new_ident: &Ident<Canonical>) {
    let Ok(Some(expr)) = Expr0::new(text, LexerType::Equation) else {
        return;
    };
    let renamed = rename_expr(&expr, old_ident, new_ident);
    if renamed != expr {
        *text = print_eqn(&renamed);
    }
}

fn rename_expr(expr: &Expr0, old_ident: &Ident<Canonical>, new_ident: &Ident<Canonical>) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr.clone(),
        Expr0::Var(ident, loc) => Expr0::Var(rename_raw_ident(ident, old_ident, new_ident), *loc),
        // Every argument is an expression, a bare variable reference included
        // (`isModuleInput(x)`), so one walk covers every builtin.
        Expr0::App(UntypedBuiltinFn(name, args), loc) => Expr0::App(
            UntypedBuiltinFn(
                name.clone(),
                args.iter()
                    .map(|arg| rename_expr(arg, old_ident, new_ident))
                    .collect(),
            ),
            *loc,
        ),
        Expr0::Subscript(ident, indexes, loc) => Expr0::Subscript(
            rename_raw_ident(ident, old_ident, new_ident),
            indexes
                .iter()
                .map(|idx| rename_index_expr(idx, old_ident, new_ident))
                .collect(),
            *loc,
        ),
        Expr0::Op1(op, rhs, loc) => {
            Expr0::Op1(*op, Box::new(rename_expr(rhs, old_ident, new_ident)), *loc)
        }
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            *op,
            Box::new(rename_expr(lhs, old_ident, new_ident)),
            Box::new(rename_expr(rhs, old_ident, new_ident)),
            *loc,
        ),
        Expr0::If(cond, then_branch, else_branch, loc) => Expr0::If(
            Box::new(rename_expr(cond, old_ident, new_ident)),
            Box::new(rename_expr(then_branch, old_ident, new_ident)),
            Box::new(rename_expr(else_branch, old_ident, new_ident)),
            *loc,
        ),
    }
}

fn rename_index_expr(
    index: &IndexExpr0,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) -> IndexExpr0 {
    match index {
        IndexExpr0::Wildcard(_) | IndexExpr0::StarRange(_, _) | IndexExpr0::DimPosition(_, _) => {
            index.clone()
        }
        IndexExpr0::Range(lhs, rhs, loc) => IndexExpr0::Range(
            Box::new(rename_expr(lhs, old_ident, new_ident)),
            Box::new(rename_expr(rhs, old_ident, new_ident)),
            *loc,
        ),
        IndexExpr0::Expr(expr) => IndexExpr0::Expr(rename_expr(expr, old_ident, new_ident)),
    }
}

/// A reference as written, renamed through the canonical rule
/// (`rename_canonical_ident`); a reference the rule leaves alone keeps the
/// user's spelling.
fn rename_raw_ident(
    ident: &RawIdent,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) -> RawIdent {
    let canonical = ident.canonicalize();
    let renamed = rename_canonical_ident(&canonical, old_ident, new_ident);
    if renamed == canonical {
        ident.clone()
    } else {
        RawIdent::new(renamed.to_source_repr())
    }
}

pub(crate) fn expr2_to_string(expr: &Expr2) -> String {
    let expr0 = expr2_to_expr0(expr);
    crate::ast::print_eqn(&expr0)
}

pub(crate) fn expr2_to_expr0(expr: &Expr2) -> Expr0 {
    match expr {
        Expr2::Const(text, value, loc) => Expr0::Const(text.clone(), *value, *loc),
        Expr2::Var(ident, _, loc) => Expr0::Var(RawIdent::new(ident.to_source_repr()), *loc),
        Expr2::App(builtin, _, loc) => {
            let untyped = builtin_to_untyped(builtin);
            Expr0::App(untyped, *loc)
        }
        Expr2::Subscript(ident, indexes, _, loc) => Expr0::Subscript(
            RawIdent::new(ident.to_source_repr()),
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

fn rename_canonical_ident(
    ident: &Ident<Canonical>,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) -> Ident<Canonical> {
    if ident == old_ident {
        return new_ident.clone();
    }

    let ident_str = ident.as_str();
    if let Some(pos) = ident_str.rfind('·') {
        let prefix = &ident_str[..pos];
        let suffix = &ident_str[pos + '·'.len_utf8()..];

        // Only rename self-qualified references (self·variable)
        // Don't rename other module-qualified references as they refer to different variables
        if suffix == old_ident.as_str() && prefix == "self" {
            return Ident::from_unchecked(format!("self·{}", new_ident.as_str()));
        }
    }

    ident.clone()
}

fn rename_module_references(
    model: &mut datamodel::Model,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    for var in model.variables.iter_mut() {
        if let Variable::Module(module) = var {
            for reference in module.references.iter_mut() {
                rename_module_reference_string(&mut reference.src, old_ident, new_ident);
                rename_module_reference_string(&mut reference.dst, old_ident, new_ident);
            }
        }
    }
}

fn rename_module_reference_string(
    value: &mut String,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    let canonical = Ident::new(value.as_str());
    let renamed = rename_canonical_ident(&canonical, old_ident, new_ident);
    if renamed != canonical {
        *value = renamed.to_source_repr();
    }
}

fn rename_group_members(
    model: &mut datamodel::Model,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    for group in model.groups.iter_mut() {
        for member in group.members.iter_mut() {
            if canonicalize(member.as_str()) == old_ident.as_str() {
                *member = new_ident.to_source_repr();
            }
        }
    }
}

fn update_stock_flow_references(
    model: &mut datamodel::Model,
    old_ident: &Ident<Canonical>,
    new_ident: &Ident<Canonical>,
) {
    for var in model.variables.iter_mut() {
        if let Variable::Stock(stock) = var {
            for inflow in stock.inflows.iter_mut() {
                if canonicalize(inflow.as_str()) == old_ident.as_str() {
                    *inflow = new_ident.to_source_repr();
                }
            }
            for outflow in stock.outflows.iter_mut() {
                if canonicalize(outflow.as_str()) == old_ident.as_str() {
                    *outflow = new_ident.to_source_repr();
                }
            }
            stock.inflows.sort_unstable();
            stock.outflows.sort_unstable();
        }
    }
}

fn apply_upsert_view(
    model: &mut datamodel::Model,
    index: u32,
    view: datamodel::View,
) -> Result<()> {
    let index = index as usize;

    if index < model.views.len() {
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

    #[test]
    fn upsert_view_and_delete() {
        let mut project = TestProject::new("test").build_datamodel();
        let view = datamodel::View::StockFlow(datamodel::StockFlow {
            name: None,
            elements: vec![],
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
            })],
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
            })],
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
        // A builtin's name is lowercased by the parser (`parser/mod.rs`), so the
        // printer has only that form on either tier.
        assert_eq!(
            scalar("good"),
            "sum(aa) * 2",
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
    /// rename rewrites the argument and keeps the call (in the parser's
    /// lowercase spelling of the builtin's name).
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
        assert_eq!(scalar("y"), "smth1(w, 3)");
        assert_eq!(scalar("z"), "previous(w + 1) + init(w * 2)");
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
                ],
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
                ],
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
                variables: vec![],
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

    #[test]
    fn add_model_duplicate_returns_error() {
        let mut project = TestProject::new("test").build_datamodel();

        let patch = ProjectPatch {
            project_ops: vec![ProjectOperation::AddModel {
                name: "main".to_string(),
            }],
            models: vec![],
        };

        let err = apply_patch(&mut project, patch).unwrap_err();
        assert_eq!(err.code, ErrorCode::DuplicateVariable);
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
            })],
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
            })],
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
            ],
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
            })],
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
            variables: vec![],
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
            variables: vec![],
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
            variables: vec![],
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
                ],
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
                ],
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
        let view_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertView {
                    index: 0,
                    view: datamodel::View::StockFlow(datamodel::StockFlow {
                        name: None,
                        elements: vec![],
                        view_box: Default::default(),
                        zoom: 1.0,
                        use_lettered_polarity: false,
                        font: None,
                        sketch_compat: None,
                    }),
                }],
            }],
        };
        assert!(is_view_only_patch(&view_patch));

        let mixed_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![
                    ModelOperation::UpsertView {
                        index: 0,
                        view: datamodel::View::StockFlow(datamodel::StockFlow {
                            name: None,
                            elements: vec![],
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
        assert!(!is_view_only_patch(&mixed_patch));

        let empty_patch = ProjectPatch {
            project_ops: vec![],
            models: vec![],
        };
        assert!(is_view_only_patch(&empty_patch));

        let project_op_patch = ProjectPatch {
            project_ops: vec![ProjectOperation::AddModel {
                name: "test".to_string(),
            }],
            models: vec![],
        };
        assert!(!is_view_only_patch(&project_op_patch));
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
