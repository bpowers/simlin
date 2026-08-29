// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// Salsa-tracked orchestration borrows the per-variable lowering memos into a
// stack-local `UnitModel`, runs the pure inference (`units_infer`) and
// consistency (`units_check`) cores, and accumulates their diagnostics. No
// whole-model equation value crosses a query boundary.
//
// Conveyor parameter expressions are the one transient augmentation. They are
// parsed and lowered after the ordinary unit model is built, then borrowed only
// by that parameter check; they must never join the cached per-variable inputs
// or contribute constraints to ordinary inference.

//! Per-model unit inference and checking as a salsa-tracked query.
//!
//! `check_model_units` is the single salsa-tracked entry point that runs unit
//! inference + consistency checking for one model and accumulates unit
//! warnings. It is invoked by `db::model_all_diagnostics`.
//!
//! Stdlib and macro-marked models are skipped: both are generic templates
//! whose formal parameters are unitless, so checking them in isolation only
//! produces noise; their unit correctness is validated at each instantiation
//! through the cross-module constraints `units_infer` generates.
//!
//! This is a submodule of `db` (a child of `db.rs`, like `macro_registry`
//! and `dep_graph`) kept in its own file purely to keep `db.rs` under the
//! per-file line cap (`scripts/lint-project.sh` rule 2);
//! `db::model_all_diagnostics` reaches it via `crate::db::units::...`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::common::{Canonical, Ident};
use crate::datamodel;
#[cfg(test)]
use crate::db::DiagnosticCategory;
use crate::db::{
    Db, Diagnostic, DiagnosticSeverity, SourceModel, SourceProject, SourceVariable,
    lowered_implicit_variable, lowered_source_variable, model_implicit_var_info,
    model_scope_models, project_dimensions_context, project_units_context, source_model_is_stdlib,
};

/// Build one transient unit-analysis view from memo-owned per-variable lowered
/// values. The view clones only `Arc` handles and is never cached itself.
pub(crate) fn unit_model(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> crate::model::UnitModel {
    let mut variables = HashMap::new();
    for source_var in model.variables(db).values() {
        let lowered = lowered_source_variable(db, *source_var, model, project);
        variables.insert(lowered.ident.clone(), std::sync::Arc::clone(lowered));
    }
    let mut implicit_names: Vec<_> = model_implicit_var_info(db, model, project)
        .keys()
        .cloned()
        .collect();
    implicit_names.sort_unstable();
    for name in implicit_names {
        if let Some(lowered) = lowered_implicit_variable(db, model, project, name).as_ref() {
            variables.insert(lowered.ident.clone(), std::sync::Arc::clone(lowered));
        }
    }
    crate::model::UnitModel {
        name: Ident::new(model.name(db)),
        variables,
        is_macro: model.macro_spec(db).is_some(),
        macro_params: crate::model::macro_param_idents(model.macro_spec(db).as_ref()),
    }
}

/// Collect the identifiers that must share units because they sit in the
/// "value branches" of an `if isModuleInput(x) then x else y` conditional.
///
/// Every stdlib delay/smooth module's stock-init equation selects between
/// a caller-supplied `initial_value` and the module's `input`; the stdlib
/// marks this choice with an `isModuleInput(initial_value)` predicate, so
/// `initial_value` (then-branch) and `input` (else-branch) are the pair
/// whose units must agree.  Other identifiers that appear elsewhere in
/// the init AST -- notably `delay_time` in `delay1`/`delay3`, which is
/// multiplied against the value-branch result -- are *coefficients*, not
/// value-equivalents, and their units legitimately differ.  Grabbing every
/// identifier with a whole-AST name collection conflates these roles.
///
/// We walk the AST looking for any `If(App(IsModuleInput(_)), t, f)` subtree
/// and, on the first match, record *only the bare `Var` identifiers* that
/// appear directly as `t` or `f`.  An identifier embedded in arithmetic --
/// for example `trend`'s then-branch `input / (1 + delay_time *
/// initial_value)` -- is playing a coefficient or rate role, not a
/// value-equivalence role, and must NOT be collapsed into the equivalence
/// group.  If the matched branches are both non-bare (or no `isModuleInput`
/// subtree exists), we return an empty set and the pairwise-compatibility
/// check in `check_model_units` skips this stock entirely.  That is the
/// correct conservative behaviour: without a structural value-swap marker
/// we have no basis for unit equivalence.
fn init_value_equivalence_group(
    ast: &crate::ast::Ast<crate::ast::Expr2>,
) -> HashSet<Ident<Canonical>> {
    use crate::ast::{Ast, Expr2};
    use crate::builtins::BuiltinFn;

    /// If `expr` is a bare `Var`, insert its identifier into `out`.  A bare
    /// reference directly under the if-then-else is the stdlib's signal
    /// that a module-input slot is interchangeable with its sibling
    /// branch's bare reference; anything wrapped in arithmetic (or a
    /// builtin call, subscript, nested conditional, etc.) means the
    /// identifier is playing a different role and should be left out of
    /// the equivalence group.
    fn try_insert_bare_var(expr: &Expr2, out: &mut HashSet<Ident<Canonical>>) {
        if let Expr2::Var(id, _, _) = expr {
            out.insert(id.clone());
        }
    }

    /// Walk the AST looking for an `If(App(IsModuleInput(_)), t, f, ..)`
    /// subtree; on the first match record the bare-Var idents (if any)
    /// from both branches and return.  We match only the first such
    /// subtree -- stdlib modules use this pattern at most once per init
    /// equation, and a second isModuleInput inside a branch would
    /// indicate a different constraint we do not want to collapse.
    fn find_value_branches(expr: &Expr2, out: &mut HashSet<Ident<Canonical>>) -> bool {
        match expr {
            Expr2::If(cond, t, f, _, _) => {
                if let Expr2::App(BuiltinFn::IsModuleInput(_, _), _, _) = cond.as_ref() {
                    try_insert_bare_var(t, out);
                    try_insert_bare_var(f, out);
                    return true;
                }
                find_value_branches(cond, out)
                    || find_value_branches(t, out)
                    || find_value_branches(f, out)
            }
            Expr2::Op2(_, l, r, _, _) => find_value_branches(l, out) || find_value_branches(r, out),
            Expr2::Op1(_, e, _, _) => find_value_branches(e, out),
            Expr2::App(builtin, _, _) => {
                use crate::builtins::{BuiltinContents, walk_builtin_expr};
                let mut found = false;
                walk_builtin_expr(builtin, |c| {
                    if let BuiltinContents::Expr(inner) | BuiltinContents::LookupTable(inner) = c
                        && !found
                    {
                        found = find_value_branches(inner, &mut *out);
                    }
                });
                found
            }
            Expr2::Subscript(_, args, _, _) => {
                for arg in args {
                    if let crate::ast::IndexExpr2::Expr(e) = arg
                        && find_value_branches(e, out)
                    {
                        return true;
                    }
                }
                false
            }
            Expr2::Const(_, _, _) | Expr2::Var(_, _, _) => false,
        }
    }

    let mut out = HashSet::new();
    match ast {
        Ast::Scalar(expr) => {
            find_value_branches(expr, &mut out);
        }
        Ast::ApplyToAll(_, expr) => {
            find_value_branches(expr, &mut out);
        }
        Ast::Arrayed(_, elements, default_expr, _) => {
            for expr in elements.values() {
                find_value_branches(expr, &mut out);
            }
            if let Some(default_expr) = default_expr {
                find_value_branches(default_expr, &mut out);
            }
        }
    }
    out
}

/// Per-model tracked function that performs unit inference and checking,
/// accumulating unit warnings/errors through the salsa accumulator.
///
/// Borrows the per-variable lowered values for this model and its transitive
/// module targets into stack-local views. This is exactly the graph
/// `units_infer` can consult while following module inputs and outputs; an
/// unrelated model is outside the query's dependency cone (GH #966).
/// Unit mismatches accumulate as warnings because they do not block simulation.
///
/// Stdlib (implicit) models are skipped because they are generic
/// templates that only make sense when instantiated with specific inputs.
#[salsa::tracked]
pub fn check_model_units(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use crate::common::ErrorCode;

    // Skip stdlib models -- they are generic and unit checking doesn't
    // apply until instantiated with concrete inputs.
    //
    // The shared gate requires the suffix to name a real stdlib model. An
    // imported `stdlib\u{205A}<unknown>` model is therefore treated as a user
    // model and checked rather than mistaken for a generic template (GH #988).
    if source_model_is_stdlib(db, model) {
        return;
    }

    // Skip macro-marked models for the same reason: a macro is a generic
    // template whose formal parameters are unitless, so unit-checking its body
    // in isolation only produces spurious errors (e.g. C-LEARN's
    // `ramp_from_to`/`sshape`). Macro correctness is validated at each
    // instantiation through the cross-module unit constraints in `units_infer`.
    if model.macro_spec(db).is_some() {
        return;
    }

    let model_name = model.name(db).clone();
    let units_ctx = project_units_context(db, project);

    // Borrow each model in this one's module-reachable scope, so that
    // cross-module unit inference constraints (module
    // inputs/outputs) can resolve submodel variable types. A stdlib model is in
    // the map when this model instantiates one, on the same rule as any other
    // module target.
    //
    // The map is built from `model_scope_models`' resolved handles. What can be
    // missing is the NAME, not this handle. `model_scope_models` seeds the
    // scope with `project.models(db)`'s entry for the root's canonical name
    // whenever the project holds that name, so
    //
    //   - the name is absent only when the project holds NO model under it (this
    //     handle was renamed or deleted while a caller kept it), and the lookup
    //     below then returns `None` -- the signal that there is nothing to check;
    //   - if a DIFFERENT handle occupies the name, the map holds that other
    //     model's unit view and `target_model` is that model, not this one.
    let unit_models: HashMap<Ident<Canonical>, crate::model::UnitModel> =
        model_scope_models(db, model, project)
            .values()
            .map(|src_model| {
                let unit_model = unit_model(db, *src_model, project);
                (unit_model.name.clone(), unit_model)
            })
            .collect();

    // Find the target model in the lowered map. A `SourceModel` whose canonical
    // name the project no longer holds has nothing to check here.
    let target_ident = Ident::<Canonical>::new(&model_name);
    let target_model = match unit_models.get(&target_ident) {
        Some(m) => m,
        None => return,
    };

    // Check whether the model declares units on any variable. If not,
    // skip surfacing inference errors (the model wasn't designed with
    // dimensional analysis in mind).
    let has_declared_units = target_model
        .variables
        .values()
        .any(|var| var.units().is_some());

    // Run unit inference. Inference is partial: it returns the units it could
    // resolve together with any dimensional conflicts it found. We keep the
    // resolved units -- so the rest of the model is still unit-checked even when
    // one equation conflicts, rather than discarding the whole inferred-units
    // map on the first conflict (GH #614).
    //
    // Conflicts are surfaced as a single umbrella model-level warning rather
    // than one diagnostic per conflict: a large macro-instantiated model can
    // produce hundreds of internal constraint contradictions, and emitting one
    // warning each would flood the report. The full conflict list remains
    // available on the `InferenceResult` for callers that want it.
    let inference = crate::units_infer::infer(&unit_models, units_ctx, target_model);
    if has_declared_units && !inference.conflicts.is_empty() {
        // The diagnostic detail is user-facing (it reaches the GUI's error
        // panel): a plain-language sentence naming the involved variables,
        // not the raw `1 == unit-expression` constraint dump. The full
        // conflict list (with constraint text) remains available on the
        // `InferenceResult` for callers that want it.
        let friendly = match &inference.conflicts[0] {
            crate::common::UnitError::InferenceError {
                sources, details, ..
            } => crate::errors::unit_inference_reason(sources, details.as_deref()),
            other => format!("{other}"),
        };
        let detail = if inference.conflicts.len() == 1 {
            friendly
        } else {
            format!(
                "{friendly} ({} unit problems found in total)",
                inference.conflicts.len()
            )
        };
        Diagnostic::unit(inference.conflicts[0].clone(), DiagnosticSeverity::Warning)
            .with_display_details(detail)
            .with_context(model_name.clone(), None)
            .emit(db);
    }
    let inferred_units = inference.resolved;

    // Check stdlib module argument unit compatibility.
    //
    // The unit inference handles cross-module constraints recursively, but
    // implicit module variables (from SMOOTH/DELAY expansion) may not be
    // fully processed by the inference when the sub-model's internal
    // constraints aren't yet resolved. We do an explicit check here: for
    // each implicit Module variable in the target model, verify that
    // arguments bound to the same internal variable have compatible units.
    //
    // For stdlib modules like SMTH1, the first argument (input) and third
    // argument (initial_value) must have the same units because they both
    // feed into the stock's init equation. We check this by looking up
    // each argument's units (declared or inferred) and comparing.
    if has_declared_units {
        // Sorted for deterministic diagnostic emission order (GH #999),
        // matching `units_check::check` and `units_infer::gen_all_constraints`.
        let mut sorted_vars: Vec<_> = target_model.variables.iter().collect();
        sorted_vars.sort_unstable_by_key(|(id, _)| id.as_str());
        for (var_ident, var) in sorted_vars {
            if let crate::variable::VarKind::Module {
                model_name: sub_model_name,
                inputs,
            } = &var.kind
            {
                // Only check stdlib modules where we know the constraint structure
                if !sub_model_name.as_str().starts_with("stdlib\u{205A}") {
                    continue;
                }
                let submodel = match unit_models.get(sub_model_name) {
                    Some(m) => m,
                    None => continue,
                };
                // Find groups of inputs that must have compatible units.
                //
                // In smth1/smth3 the stock's init equation is
                // `if isModuleInput(initial_value) then initial_value else input`,
                // constraining `input` and `initial_value` (and nothing else) to
                // share units.  In delay1/delay3 the same conditional is
                // multiplied by `delay_time`, which is a coefficient whose units
                // are independent.  We specifically extract the identifiers that
                // sit in the value branches of the `if isModuleInput(...)` test
                // (see `init_value_equivalence_group`); a whole-AST name walk
                // would also return `delay_time` and create spurious unit
                // equivalences.
                let stock_init_deps: Vec<HashSet<Ident<Canonical>>> = submodel
                    .variables
                    .values()
                    .filter_map(|sv| {
                        if sv.is_stock() {
                            sv.ast().map(init_value_equivalence_group)
                        } else {
                            None
                        }
                    })
                    .collect();

                for init_dep_set in &stock_init_deps {
                    // Collect (src_units, input) pairs for inputs that bind to
                    // variables in this stock's init dep set.
                    let mut group_units: Vec<(Ident<Canonical>, &crate::datamodel::UnitMap)> =
                        Vec::new();
                    for input in inputs {
                        if !init_dep_set.contains(&input.dst) {
                            continue;
                        }
                        let src_units = target_model
                            .variables
                            .get(&input.src)
                            .and_then(|v| v.units())
                            .or_else(|| inferred_units.get(&input.src));
                        if let Some(units) = src_units {
                            group_units.push((input.src.clone(), units));
                        }
                    }
                    // Check pairwise compatibility
                    if group_units.len() >= 2 {
                        let (first_src, first_units) = &group_units[0];
                        for (other_src, other_units) in &group_units[1..] {
                            if first_units != other_units {
                                Diagnostic::unit(
                                    crate::common::UnitError::ConsistencyError(
                                        ErrorCode::UnitMismatch,
                                        crate::builtins::Loc::default(),
                                        Some(format!(
                                            "module '{}': argument '{}' has units '{}' \
                                                 but argument '{}' has units '{}' \
                                                 (both feed the same internal variable)",
                                            var_ident,
                                            first_src,
                                            first_units,
                                            other_src,
                                            other_units,
                                        )),
                                    ),
                                    DiagnosticSeverity::Warning,
                                )
                                .with_context(model_name.clone(), Some(var_ident.to_string()))
                                .emit(db);
                            }
                        }
                    }
                }
            }
        }
    }

    // Run unit checking. Dimensional analysis is opt-in by declaring units:
    // a model that declares NO units on any variable (a purely numeric
    // model) gets no consistency diagnostics either -- without this gate the
    // arrayed element-consistency pass still fired off sim_specs'
    // time_units alone, flooding unit-less fixtures (e.g. the test-models
    // arithmetics suite) with warnings about equations the modeler never
    // claimed were dimensional. Mirrors the `has_declared_units` gate on
    // inference conflicts above.
    if !has_declared_units {
        return;
    }
    match crate::units_check::check(units_ctx, &inferred_units, target_model) {
        Ok(()) => {}
        Err(errors) => {
            for (ident, err) in errors.into_iter() {
                Diagnostic::unit(err, DiagnosticSeverity::Warning)
                    .with_context(model_name.clone(), Some(ident.to_string()))
                    .emit(db);
            }
        }
    }

    // Conveyor block parameter unit checks (docs/design/conveyors.md §9.8).
    // These expressions live as datamodel strings on the stock/flow Compat, so
    // `units_check::check`'s per-variable loop never sees them; we reach them
    // here with the datamodel + lowering machinery already in hand. Runs after
    // the ordinary unit check so a conveyor model's ordinary variables are still
    // checked exactly as before.
    check_conveyor_param_units(
        db,
        model,
        project,
        &model_name,
        target_model,
        units_ctx,
        &inferred_units,
    );
}

/// Unit-check a model's conveyor block parameters (docs/design/conveyors.md
/// §9.8). Best-effort/advisory: every mismatch is a `Warning`, never a hard
/// error, so the model still simulates through `queue_compile::build_vm`.
///
/// A conveyor's `<len>`/`<capacity>`/`<in_limit>` and its leak flows' fractions
/// are expression strings on the stock/flow `datamodel::Compat`, not ordinary
/// variables, so they must be parsed and lowered here to be
/// unit-checked. We synthesize one hidden aux per parameter expression, lower
/// them together in the target model's context (so their variable references
/// resolve to real declared-or-inferred units), then compare each computed unit
/// against the unit the block position requires, with the conveyor stock's
/// declared units `S` and the model time unit `t`:
///
///   - `<len>`      : `t`   (transit time)
///   - `<capacity>` : `S`   (max material on the belt)
///   - `<in_limit>` : `S/t` (max inflow per time unit)
///   - a LINEAR leak fraction      : dimensionless
///   - an EXPONENTIAL leak rate     : `1/t`
///
/// `<sample>` and `<arrest>` are intentionally NOT checked: they are CONDITIONS
/// evaluated for nonzero (a predicate like `TIME > 10` is dimensionless by
/// design), so requiring `t` would reject valid expressions. Conveyor-driven
/// flows (primary outflow, leaks, admitted inflows) carry `S/t` like any flow
/// and are already unit-checked by the ordinary stock/flow path.
///
/// A conveyor stock with no declared units `S` skips ALL of its parameter
/// checks -- `S` is the yardstick for capacity/in_limit -- consistent with the
/// "unknown units are skipped" rule elsewhere in unit checking. Likewise a
/// parameter whose expression reads a variable with unknown units is skipped
/// (a `DoesNotExist` verdict), never reported as a mismatch.
// The lowered target model, its model name and the inferred-units map are all
// already resolved by the single caller (`check_model_units`), so they are
// passed rather than re-derived. The transient parsed scope and dimension query
// are built only past the early return below, so a model with no conveyor pays
// nothing for them.
fn check_conveyor_param_units(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    model_name: &str,
    target_model: &crate::model::UnitModel,
    units_ctx: &crate::units::Context,
    inferred_units: &HashMap<Ident<Canonical>, crate::datamodel::UnitMap>,
) {
    use crate::ast::Ast;
    use crate::common::{ErrorCode, UnitError};
    use crate::datamodel::UnitMap;
    use crate::units::{UnitOp, Units, combine};

    // One synthesized parameter-expression aux awaiting unit checking.
    struct SynthParam {
        ident: Ident<Canonical>,
        expected: UnitMap,
        stock: String,
        label: String,
    }

    let time_units = crate::units_check::model_time_units(units_ctx);

    // Index the source variables by canonical ident so we can resolve a
    // conveyor stock's declared units and follow its outflow names to the leak
    // flows carrying leak fractions. Salsa inputs are Copy.
    let src_by_canon: HashMap<Ident<Canonical>, SourceVariable> = model
        .variables(db)
        .values()
        .map(|v| (Ident::new(v.ident(db)), *v))
        .collect();

    let mut synth_params: Vec<SynthParam> = Vec::new();
    let mut synth_dm_vars: Vec<datamodel::Variable> = Vec::new();

    // A synthetic parameter aux: scalar, no declared units (we compare its
    // computed units manually), a reserved synthetic name that cannot collide
    // with a user variable.
    let push_param = |synth_params: &mut Vec<SynthParam>,
                      synth_dm_vars: &mut Vec<datamodel::Variable>,
                      key: &str,
                      expr: &str,
                      expected: UnitMap,
                      stock: &str,
                      label: &str| {
        let ident_str = format!("$\u{205a}conveyor\u{205a}{key}");
        let ident = Ident::new(&ident_str);
        synth_dm_vars.push(datamodel::Variable::Aux(datamodel::Aux {
            ident: ident_str,
            equation: datamodel::Equation::Scalar(expr.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        synth_params.push(SynthParam {
            ident,
            expected,
            stock: stock.to_string(),
            label: label.to_string(),
        });
    };

    // Sorted for deterministic diagnostic emission order (GH #999),
    // matching every other variable loop that feeds diagnostics.
    let mut sorted_svars: Vec<_> = model.variables(db).values().collect();
    sorted_svars.sort_unstable_by_key(|sv| sv.ident(db).as_str());
    for svar in sorted_svars {
        let compat = svar.compat(db);
        let Some(conv) = &compat.conveyor else {
            continue;
        };
        let stock_ident = Ident::new(svar.ident(db));
        // The conveyor stock's declared units `S`. Without it we have no
        // yardstick for capacity/in_limit, so skip this conveyor's checks.
        let Some(stock_units) = target_model
            .variables
            .get(&stock_ident)
            .and_then(|v| v.units())
            .cloned()
        else {
            continue;
        };
        let stock_name = svar.ident(db).clone();

        // `<len>` (transit time): expected `t`.
        push_param(
            &mut synth_params,
            &mut synth_dm_vars,
            &format!("{stock_name}\u{205a}len"),
            &conv.transit_time,
            time_units.clone(),
            &stock_name,
            "<len> (transit time)",
        );
        // `<capacity>`: expected `S`.
        if let Some(capacity) = &conv.capacity {
            push_param(
                &mut synth_params,
                &mut synth_dm_vars,
                &format!("{stock_name}\u{205a}capacity"),
                capacity,
                stock_units.clone(),
                &stock_name,
                "<capacity>",
            );
        }
        // `<in_limit>`: expected `S/t`.
        if let Some(in_limit) = &conv.inflow_limit {
            push_param(
                &mut synth_params,
                &mut synth_dm_vars,
                &format!("{stock_name}\u{205a}in_limit"),
                in_limit,
                combine(UnitOp::Div, stock_units.clone(), time_units.clone()),
                &stock_name,
                "<in_limit>",
            );
        }

        // Leak fractions on this conveyor's leak-marked outflows. A linear leak
        // fraction is dimensionless; an exponential leak RATE is `1/t`. A bare
        // `<leak/>` marker (fraction None) contributes zero and is not checked.
        for out_name in svar.outflows(db) {
            let Some(flow) = src_by_canon.get(&Ident::new(out_name)) else {
                continue;
            };
            let Some(leak) = &flow.compat(db).leakage else {
                continue;
            };
            let Some(fraction) = &leak.fraction else {
                continue;
            };
            let (expected, label) = if conv.exponential_leak {
                (
                    combine(UnitOp::Div, UnitMap::new(), time_units.clone()),
                    "exponential leak rate",
                )
            } else {
                (UnitMap::new(), "linear leak fraction")
            };
            push_param(
                &mut synth_params,
                &mut synth_dm_vars,
                &format!("{stock_name}\u{205a}leak\u{205a}{}", flow.ident(db)),
                fraction,
                expected,
                &stock_name,
                label,
            );
        }
    }

    if synth_params.is_empty() {
        return;
    }

    // Lower every synthesized parameter aux in the target model's context. The
    // scope borrows source parse memos and owns only the generated helpers, so
    // the expressions never enter the model's cached unit-analysis inputs.
    let dim_ctx = project_dimensions_context(db, project);
    let make_lowering_model = |source_model: SourceModel| {
        let mut variables = HashMap::new();
        for source_var in source_model.variables(db).values() {
            let parsed = crate::db::parse_source_variable(db, *source_var, project);
            variables.insert(
                parsed.variable.ident.clone(),
                std::borrow::Cow::Borrowed(&parsed.variable),
            );
            for implicit in &parsed.implicit_vars {
                let parsed_implicit = implicit.parsed_variable(dim_ctx);
                variables.insert(
                    parsed_implicit.ident.clone(),
                    std::borrow::Cow::Owned(parsed_implicit),
                );
            }
        }
        crate::model::LoweringModel { variables }
    };
    let mut lowering_models: HashMap<Ident<Canonical>, crate::model::LoweringModel<'_>> =
        model_scope_models(db, model, project)
            .values()
            .map(|source_model| {
                (
                    Ident::new(source_model.name(db)),
                    make_lowering_model(*source_model),
                )
            })
            .collect();
    lowering_models
        .entry(Ident::new(model_name))
        .or_insert_with(|| make_lowering_model(model));

    let synth_ctx = crate::variable::ParseContext::new(dim_ctx, units_ctx);
    for dm_var in &synth_dm_vars {
        let mut dummy: Vec<crate::capture::ImplicitVar> = Vec::new();
        let vs0 =
            crate::variable::parse_var(&synth_ctx, dm_var, &mut dummy, |mi| Ok(Some(mi.clone())));
        lowering_models
            .get_mut(&Ident::new(model_name))
            .expect("the target lowering scope contains itself")
            .variables
            .insert(Ident::new(vs0.ident()), std::borrow::Cow::Owned(vs0));
    }
    let scope = crate::model::LoweringScope {
        models: &lowering_models,
        dimensions: dim_ctx,
        model_name,
    };
    let lowered_params: HashMap<Ident<Canonical>, Arc<crate::variable::Variable>> = synth_params
        .iter()
        .map(|param| {
            let parsed = &lowering_models[&Ident::new(model_name)].variables[&param.ident];
            (
                param.ident.clone(),
                Arc::new(crate::model::lower_variable(&scope, parsed)),
            )
        })
        .collect();
    let mut unit_variables = target_model.variables.clone();
    unit_variables.extend(
        lowered_params
            .iter()
            .map(|(name, variable)| (name.clone(), Arc::clone(variable))),
    );
    let aug_unit_model = crate::model::UnitModel {
        name: target_model.name.clone(),
        variables: unit_variables,
        is_macro: target_model.is_macro,
        macro_params: target_model.macro_params.clone(),
    };

    for param in &synth_params {
        // Extract the lowered parameter expression. A malformed expression has
        // no AST (it surfaces as a separate compile error), so there is nothing
        // to unit-check -- skip it.
        let expr = match lowered_params.get(&param.ident).and_then(|v| v.ast()) {
            Some(Ast::Scalar(expr)) | Some(Ast::ApplyToAll(_, expr)) => expr,
            _ => continue,
        };

        let diagnostic = match crate::units_check::evaluate_expr_units(
            units_ctx,
            inferred_units,
            &aug_unit_model,
            expr,
        ) {
            // A determinate unit that disagrees with what the block requires.
            Ok(Units::Explicit(actual)) if actual != param.expected => {
                let details = format!(
                    "computed units '{}' don't match the expected units '{}'",
                    actual, param.expected
                );
                Some(
                    Diagnostic::unit(
                        UnitError::ConsistencyError(
                            ErrorCode::UnitMismatch,
                            expr.get_loc(),
                            Some(details.clone()),
                        ),
                        DiagnosticSeverity::Warning,
                    )
                    .with_display_details(format!(
                        "conveyor '{}' {}: {details}",
                        param.stock, param.label
                    )),
                )
            }
            // Matches, or a pure constant (compatible with any expected unit).
            Ok(_) => None,
            // A dependency's units are unknown -- skip, not a dimensional error.
            Err(UnitError::ConsistencyError(ErrorCode::DoesNotExist, _, _)) => None,
            // An internal dimensional inconsistency in the expression itself
            // (e.g. adding incompatible units); surface it against the conveyor.
            Err(err) => {
                let reason = err.to_string();
                Some(
                    Diagnostic::unit(err, DiagnosticSeverity::Warning).with_display_details(
                        format!("conveyor '{}' {}: {reason}", param.stock, param.label),
                    ),
                )
            }
        };

        if let Some(diagnostic) = diagnostic {
            diagnostic
                .with_context(model_name.to_string(), Some(param.stock.clone()))
                .emit(db);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

    /// A module freshly drawn in the editor carries an EMPTY model_name -- it
    /// has no target model assigned yet (see `Editor.tsx`'s `upsertModule` with
    /// `modelName: ''`). That empty name is absent from the inference
    /// `models_s1` map, so unit inference must skip the module rather than
    /// panic on the missing key. Regression for the `self.models[model_name]`
    /// index panic in `units_infer::gen_all_constraints`, which surfaced as a
    /// WASM panic ("no entry found for key") -- and a cascading recursive-mutex
    /// panic from re-entering the panic path -- when a newly drawn module was
    /// persisted.
    #[test]
    fn module_with_empty_model_name_does_not_panic() {
        let sim_specs = sim_specs_with_units("parsec");

        let mod_inst = crate::datamodel::Variable::Module(crate::datamodel::Module {
            ident: "mod_inst".to_string(),
            model_name: String::new(),
            documentation: String::new(),
            units: None,
            references: vec![],
            compat: crate::datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        });
        // A unit-declared sibling makes inference run with real constraints.
        let model = x_model("main", vec![x_aux("input", "6", Some("widget")), mod_inst]);
        let project = x_project(sim_specs, &[model]);

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);

        // Before the fix this panicked inside units_infer; now it returns
        // cleanly, skipping the dangling module's constraints.
        check_model_units(&db, sync.models["main"].source, sync.project);
    }

    /// Unit-checking a model inside a module CYCLE (`a` instantiates `b`, `b`
    /// instantiates `a`) must terminate. `units_infer::gen_all_constraints`
    /// recurses through every module instantiation, so before the recursion
    /// guard this overflowed the stack -- which, unlike a panic, is an
    /// immediate abort of the host process (libsimlin builds with
    /// `panic=abort`; the host is a WASM tab, an MCP server, or a Python
    /// session). Today nothing in the shipped product reaches here with a
    /// cyclic graph, because `collect_all_diagnostics` consults
    /// `project_module_graph` first; this pins the engine primitive itself
    /// rather than that one caller-side gate.
    ///
    /// It must also DEGRADE rather than fail: `a`'s own dimensional error is
    /// still reported. This is driven through `check_model_units` directly --
    /// the entry that diverged -- not through the gated whole-project path.
    #[test]
    fn module_cycle_unit_checks_without_diverging() {
        use crate::common::ErrorCode;
        use crate::testutils::x_module_named;

        let sim_specs = sim_specs_with_units("parsec");
        let model_a = x_model(
            "a",
            vec![
                x_aux("x", "1", Some("widget")),
                // A genuine dimensional error: widget + time.
                x_aux("bad", "x + TIME", None),
                x_module_named("to_b", "b", &[("x", "to_b.input")], None),
            ],
        );
        let model_b = x_model(
            "b",
            vec![
                x_aux("input", "0", None),
                x_module_named("to_a", "a", &[("input", "to_a.x")], None),
            ],
        );
        let project = x_project(sim_specs, &[model_a, model_b]);

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let diagnostics = check_model_units::accumulated::<Diagnostic>(
            &db,
            sync.models["a"].source,
            sync.project,
        );

        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.category == DiagnosticCategory::UnitInference
                    && diagnostic.code == ErrorCode::UnitMismatch
            }),
            "a model's own unit error must still be reported despite the cycle, got: {:?}",
            diagnostics
        );
    }

    // ── Conveyor block parameter unit checks (docs/design/conveyors.md §9.8) ──

    /// Build a conveyor stock: an ordinary `datamodel::Stock` carrying a
    /// `<conveyor>` block on its `Compat`.
    fn conveyor_stock(
        ident: &str,
        init: &str,
        inflows: &[&str],
        outflows: &[&str],
        units: Option<&str>,
        conv: crate::datamodel::Conveyor,
    ) -> crate::datamodel::Variable {
        let compat = crate::datamodel::Compat {
            conveyor: Some(conv),
            ..Default::default()
        };
        crate::datamodel::Variable::Stock(crate::datamodel::Stock {
            ident: ident.to_string(),
            equation: crate::datamodel::Equation::Scalar(init.to_string()),
            documentation: String::new(),
            units: units.map(|s| s.to_owned()),
            inflows: inflows.iter().map(|s| s.to_string()).collect(),
            outflows: outflows.iter().map(|s| s.to_string()).collect(),
            ai_state: None,
            uid: None,
            compat,
        })
    }

    /// A leak-marked outflow carrying a leak `fraction` expression.
    fn leak_flow(ident: &str, fraction: &str) -> crate::datamodel::Variable {
        let mut var = crate::testutils::x_flow(ident, "0", None);
        if let crate::datamodel::Variable::Flow(flow) = &mut var {
            flow.compat.leakage = Some(crate::datamodel::Leakage {
                fraction: Some(fraction.to_string()),
                integers: false,
                zone_start: None,
                zone_end: None,
            });
        }
        var
    }

    /// A `<conveyor>` block with the given transit time and everything else at
    /// its documented default.
    fn conveyor_with_len(transit_time: &str) -> crate::datamodel::Conveyor {
        crate::datamodel::Conveyor {
            transit_time: transit_time.to_string(),
            capacity: None,
            inflow_limit: None,
            sample: None,
            arrest: None,
            discrete: false,
            batch_integrity: false,
            one_at_a_time: true,
            exponential_leak: false,
            ignore_earlier_zone_losses: false,
        }
    }

    /// The human-readable detail text of a `Unit` diagnostic (empty otherwise).
    fn unit_detail(d: &Diagnostic) -> String {
        if d.category.is_unit() {
            d.reason().unwrap_or_default().to_string()
        } else {
            String::new()
        }
    }

    /// Drive `check_model_units` over a project's `main` model and return the
    /// conveyor unit diagnostics -- the `Unit` warnings whose detail names a
    /// conveyor (so ordinary unit warnings are excluded).
    fn conveyor_unit_warnings(project: &crate::datamodel::Project) -> Vec<Diagnostic> {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, project);
        let source = sync.models["main"].source;
        check_model_units::accumulated::<Diagnostic>(&db, source, sync.project)
            .into_iter()
            .filter(|d| unit_detail(d).contains("conveyor"))
            .cloned()
            .collect()
    }

    /// A conveyor project with one stock, an inflow, and a primary outflow.
    fn conveyor_project(
        stock: crate::datamodel::Variable,
        extra: Vec<crate::datamodel::Variable>,
    ) -> crate::datamodel::Project {
        use crate::testutils::{x_flow, x_model, x_project};
        let mut vars = vec![
            stock,
            x_flow("inflow", "250", None),
            x_flow("graduating", "0", None),
        ];
        vars.extend(extra);
        let model = x_model("main", vars);
        x_project(sim_specs_with_units("month"), &[model])
    }

    #[test]
    fn conveyor_capacity_wrong_units_warns() {
        // <capacity> must match the stock's units S (widget). Here it reads a
        // variable declared in `month`, so a mismatch warning naming the
        // conveyor is produced.
        let mut conv = conveyor_with_len("bad_len_ok");
        conv.capacity = Some("bad_cap".to_string());
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            Some("widget"),
            conv,
        );
        let extra = vec![
            x_aux("bad_len_ok", "4", Some("month")), // len IS correct (month == t)
            x_aux("bad_cap", "1200", Some("month")), // capacity WRONG (month != widget)
        ];
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one conveyor unit warning, got: {warnings:?}"
        );
        let detail = unit_detail(&warnings[0]);
        assert!(
            detail.contains("students") && detail.contains("<capacity>"),
            "warning should name the conveyor and the offending parameter: {detail}"
        );
        assert_eq!(warnings[0].category, DiagnosticCategory::UnitConsistency);
        assert_eq!(warnings[0].code, crate::common::ErrorCode::UnitMismatch);
        assert_eq!(
            warnings[0].location,
            Some(crate::builtins::Loc {
                start: 0,
                end: "bad_cap".len() as u16,
            }),
            "the diagnostic location is relative to the production conveyor parameter expression"
        );
        assert_eq!(
            warnings[0].details.as_deref(),
            Some("computed units 'month' don't match the expected units 'widget'"),
            "conveyor attribution belongs in display_details, not the raising-site payload"
        );
    }

    #[test]
    fn conveyor_parameter_internal_unit_error_keeps_typed_payload() {
        // The capacity expression itself is dimensionally inconsistent. This
        // drives UnitEvaluator's binary-op error through the transient
        // conveyor-expression lowering path; the wrapper must not replace its
        // location or raw details with a generic conveyor mismatch.
        let mut conv = conveyor_with_len("4");
        let expression = "value_count + duration";
        conv.capacity = Some(expression.to_string());
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            Some("widget"),
            conv,
        );
        let extra = vec![
            x_aux("value_count", "1200", Some("widget")),
            x_aux("duration", "3", Some("month")),
        ];

        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        let warning = &warnings[0];
        assert_eq!(warning.category, DiagnosticCategory::UnitConsistency);
        assert_eq!(warning.code, crate::common::ErrorCode::UnitMismatch);
        assert_eq!(
            warning.location,
            Some(crate::builtins::Loc {
                start: 0,
                end: expression.len() as u16,
            })
        );
        assert_eq!(
            warning.details.as_deref(),
            Some("expected left and right argument units to match, but 'widget' and 'month' don't")
        );
        let display = warning
            .display_details
            .as_deref()
            .expect("the conveyor attribution is presentation context");
        assert!(
            display.contains("conveyor 'students' <capacity>"),
            "{display}"
        );
        assert!(
            display.contains(warning.details.as_deref().unwrap()),
            "{display}"
        );
    }

    #[test]
    fn conveyor_len_wrong_units_warns() {
        // <len> is a transit time and must be in the model time unit t (month).
        // Reading a variable declared in `widget` is a mismatch.
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            Some("widget"),
            conveyor_with_len("bad_len"),
        );
        let extra = vec![x_aux("bad_len", "4", Some("widget"))];
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(unit_detail(&warnings[0]).contains("<len>"));
    }

    #[test]
    fn conveyor_in_limit_wrong_units_warns() {
        // <in_limit> is an inflow rate and must be S/t (widget/month). A plain
        // `widget`-declared variable is a mismatch.
        let mut conv = conveyor_with_len("len_ok");
        conv.inflow_limit = Some("bad_limit".to_string());
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            Some("widget"),
            conv,
        );
        let extra = vec![
            x_aux("len_ok", "4", Some("month")),
            x_aux("bad_limit", "500", Some("widget")),
        ];
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(unit_detail(&warnings[0]).contains("<in_limit>"));
    }

    #[test]
    fn conveyor_correct_units_produce_no_warning() {
        // Every parameter reads a variable whose declared units match the block
        // position exactly, so no conveyor unit warning is produced.
        let mut conv = conveyor_with_len("len_ok");
        conv.capacity = Some("cap_ok".to_string());
        conv.inflow_limit = Some("limit_ok".to_string());
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            Some("widget"),
            conv,
        );
        let extra = vec![
            x_aux("len_ok", "4", Some("month")),
            x_aux("cap_ok", "1200", Some("widget")),
            x_aux("limit_ok", "500", Some("widget/month")),
        ];
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert!(
            warnings.is_empty(),
            "correct conveyor units should not warn, got: {warnings:?}"
        );
    }

    #[test]
    fn conveyor_constant_parameters_produce_no_warning() {
        // Bare numeric literals are unit-constants, compatible with any expected
        // unit -- the common authoring case must never warn.
        let mut conv = conveyor_with_len("4");
        conv.capacity = Some("1200".to_string());
        conv.inflow_limit = Some("500".to_string());
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            Some("widget"),
            conv,
        );
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, vec![]));
        assert!(warnings.is_empty(), "got: {warnings:?}");
    }

    #[test]
    fn conveyor_sample_and_arrest_predicates_do_not_warn() {
        // <sample>/<arrest> are CONDITIONS evaluated for nonzero. `TIME > 10` is
        // dimensionless by design; requiring `t` would reject it, so they are
        // deliberately not unit-checked.
        let mut conv = conveyor_with_len("4");
        conv.sample = Some("TIME > 10".to_string());
        conv.arrest = Some("TIME > 20".to_string());
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            Some("widget"),
            conv,
        );
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, vec![]));
        assert!(
            warnings.is_empty(),
            "sample/arrest predicates must not warn, got: {warnings:?}"
        );
    }

    #[test]
    fn conveyor_without_declared_stock_units_is_skipped() {
        // With no declared units S on the conveyor stock there is no yardstick
        // for capacity/in_limit, so ALL of its parameter checks are skipped --
        // consistent with the "unknown units are skipped" rule.
        let mut conv = conveyor_with_len("bad_len");
        conv.capacity = Some("bad_cap".to_string());
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            None, // no S
            conv,
        );
        let extra = vec![
            x_aux("bad_len", "4", Some("widget")),
            x_aux("bad_cap", "1200", Some("widget")),
        ];
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert!(
            warnings.is_empty(),
            "a unitless conveyor stock skips all parameter checks, got: {warnings:?}"
        );
    }

    #[test]
    fn conveyor_unknown_parameter_dependency_is_skipped() {
        // A parameter that reads a variable with UNKNOWN units (undeclared, not
        // inferrable) yields a DoesNotExist verdict -- skipped, not a mismatch.
        let mut conv = conveyor_with_len("4");
        conv.capacity = Some("mystery".to_string());
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating"],
            Some("widget"),
            conv,
        );
        // `mystery` has no declared units and nothing constrains it.
        let extra = vec![
            x_aux("mystery", "some_input", None),
            x_aux("some_input", "3", None),
        ];
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert!(
            warnings.is_empty(),
            "unknown parameter units should be skipped, got: {warnings:?}"
        );
    }

    #[test]
    fn conveyor_linear_leak_fraction_wrong_units_warns() {
        // A LINEAR leak fraction is dimensionless; reading a `widget`-declared
        // variable is a mismatch.
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating", "attriting"],
            Some("widget"),
            conveyor_with_len("4"),
        );
        let extra = vec![
            leak_flow("attriting", "bad_frac"),
            x_aux("bad_frac", "0.1", Some("widget")),
        ];
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert_eq!(warnings.len(), 1, "got: {warnings:?}");
        assert!(unit_detail(&warnings[0]).contains("leak"));
    }

    #[test]
    fn conveyor_exponential_leak_rate_correct_units_no_warning() {
        // With exponential_leak the leak number is a RATE (1/t). A variable
        // declared as `1/month` matches, so no warning.
        let mut conv = conveyor_with_len("4");
        conv.exponential_leak = true;
        let stock = conveyor_stock(
            "students",
            "1000",
            &["inflow"],
            &["graduating", "attriting"],
            Some("widget"),
            conv,
        );
        let extra = vec![
            leak_flow("attriting", "rate_ok"),
            x_aux("rate_ok", "0.1", Some("1/month")),
        ];
        let warnings = conveyor_unit_warnings(&conveyor_project(stock, extra));
        assert!(
            warnings.is_empty(),
            "an exponential leak rate in 1/t must not warn, got: {warnings:?}"
        );
    }
}
