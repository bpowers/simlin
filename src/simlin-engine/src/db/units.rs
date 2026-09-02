// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// Salsa-tracked unit-checking orchestration: it assembles each scope model's
// `UnitModel` view over the per-variable lowering memos, runs the pure unit
// inference (`units_infer`) and consistency checking (`units_check`) cores,
// and accumulates the resulting diagnostics. The dimensional-analysis logic
// itself is the Functional Core in `units.rs`/`units_infer.rs`/`units_check.rs`;
// this module only wires it into the salsa graph.
//
// The one lowering it performs is per expression: `check_conveyor_param_units`
// parses and lowers its synthetic `<len>`/`<capacity>`/`<in_limit>`/leak-
// fraction auxes one at a time under the model's variable shapes and
// unit-checks each against the model's `UnitModel`. Those auxes exist only to
// be unit-checked, so they never enter a memo, where they would feed their
// constraints into every other reader -- inference and the ordinary unit
// check included.

//! Per-model unit inference and checking as a salsa-tracked query.
//!
//! `check_model_units` is the single salsa-tracked entry point that runs unit
//! inference + consistency checking for one model and accumulates unit
//! warnings. It is invoked by `db::model_all_diagnostics`. The pass reads a
//! SCOPE, not the project: [`model_scope_models`] is the model plus the models
//! it can reach through module instantiation, the map inference runs over.
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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use salsa::Accumulator;

use crate::common::{Canonical, Ident};
use crate::datamodel;
use crate::db::var_fragment::{implicit_dep_shape, source_dep_shape};
use crate::db::{
    CompilationDiagnostic, Db, Diagnostic, DiagnosticError, DiagnosticSeverity, SourceModel,
    SourceProject, SourceVariable, SourceVariableKind, model_implicit_var_info,
    model_lowered_variables, project_dimensions_context, project_units_context,
    source_model_is_stdlib,
};
use crate::units_check::UnitModel;

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
/// identifier (as an earlier version of this code did via `identifier_set`
/// and then `collect_idents`) conflates these roles.
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

/// The models `model`'s unit inference can reach: itself, plus the transitive
/// closure of the models its module variables instantiate. Keyed by canonical
/// model name, valued by the project's handle for that name.
///
/// This is the SCOPE of `check_model_units`' inference map, and it is
/// narrower than the project on purpose: a unit check that read every model
/// would depend on every model's lowered variables, so any edit anywhere would
/// re-run every unit check. The closure is a SUPERSET of what its consumers
/// can consult: `units_infer::gen_all_constraints` follows `model_name` edges
/// into module targets (declining a back edge already on its
/// `InstantiationPath`), and `check_model_units`' stdlib-argument check looks a
/// module's target up by name. Both follow `model_name` edges and nothing
/// else, which is what the walk below follows.
///
/// Implicit modules are IN, and that is load bearing: the edges come from the
/// explicit `Module` variables AND from `model_implicit_var_info`'s module
/// entries -- the SMOOTH/DELAY/TREND and macro-call instances builtin expansion
/// synthesized. `db::project_module_graph` deliberately omits those (it only
/// needs the edges that can close a user cycle), so it is the wrong source
/// here: a macro call expands into a module targeting the macro's own model,
/// and `units_infer` binds the call's argument units to that model's
/// parameters by recursing through the edge -- dropping it drops the
/// constraint and, with it, the diagnostic. Stdlib templates are in the
/// closure on the same rule: a model that instantiates `smth1` gets exactly
/// `stdlib⁚smth1`, and one that instantiates none gets no template.
///
/// The walk is an iterative worklist over a visited set, NOT a recursive
/// tracked query: `a` instantiating `b` and `b` instantiating `a` is a project
/// a user can draw, and a recursive salsa query on that graph is an
/// unrecoverable dependency-graph panic rather than a diagnostic (GH #806). A
/// model inside a cycle yields its full REACHABLE set. The walk reads only
/// names -- variable kinds and target model names -- so an equation edit that
/// synthesizes no new instance backdates it.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_scope_models(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> BTreeMap<Ident<Canonical>, SourceModel> {
    let project_models = project.models(db);
    let root_ident: Ident<Canonical> = Ident::new(model.name(db));

    let mut scope: BTreeMap<Ident<Canonical>, SourceModel> = BTreeMap::new();
    if let Some(src_model) = project_models.get(root_ident.as_str()) {
        scope.insert(root_ident.clone(), *src_model);
    }

    // The root is walked through the handle the CALLER passed, not through the
    // project's entry for its name: a renamed-or-deleted model's handle outlives
    // its place in that map (`PersistentSyncState` threads handles across syncs),
    // and its own module targets are still what its unit check consults.
    let mut visited: BTreeSet<Ident<Canonical>> = [root_ident].into_iter().collect();
    let mut queue: Vec<SourceModel> = vec![model];
    while let Some(src_model) = queue.pop() {
        let mut targets: BTreeSet<Ident<Canonical>> = src_model
            .variables(db)
            .values()
            .filter(|sv| sv.kind(db) == SourceVariableKind::Module)
            .map(|sv| Ident::new(sv.model_name(db)))
            .collect();
        targets.extend(
            model_implicit_var_info(db, src_model, project)
                .values()
                .filter(|meta| meta.is_module)
                .filter_map(|meta| meta.model_name.as_deref())
                .map(Ident::new),
        );
        for target in targets {
            if !visited.insert(target.clone()) {
                continue;
            }
            if let Some(next) = project_models.get(target.as_str()) {
                scope.insert(target.clone(), *next);
                queue.push(*next);
            }
        }
    }

    scope
}

/// One model as the unit pass reads it: its lowered variables
/// (`model_lowered_variables`, the handle map shared with the LTM describers)
/// plus the model-level facts inference needs. Built on the stack, per unit
/// check and per scope model; it owns no lowered tree.
pub(crate) fn unit_model(db: &dyn Db, model: SourceModel, project: SourceProject) -> UnitModel {
    let macro_spec = model.macro_spec(db);
    UnitModel {
        name: Ident::new(model.name(db)),
        variables: model_lowered_variables(db, model, project),
        is_macro: macro_spec.is_some(),
        macro_params: macro_spec
            .as_ref()
            .map(|spec| spec.parameters.iter().map(|p| Ident::new(p)).collect())
            .unwrap_or_default(),
    }
}

/// Per-model tracked function that performs unit inference and checking,
/// accumulating unit warnings/errors through the salsa accumulator.
///
/// Reads the `UnitModel` view of every model in this one's module-reachable
/// scope ([`model_scope_models`]), then runs unit inference and consistency
/// checking over the target. Unit mismatches are accumulated as
/// `DiagnosticSeverity::Warning`: unit issues do not block simulation.
///
/// Stdlib (implicit) models are skipped because they are generic
/// templates that only make sense when instantiated with concrete inputs.
#[salsa::tracked]
pub fn check_model_units(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use crate::common::{ErrorCode, ErrorKind};

    // Skip stdlib models -- they are generic and unit checking doesn't
    // apply until instantiated with concrete inputs. The gate is the crate's
    // one stdlib predicate (GH #988): a model carrying the prefix but an
    // unknown suffix is a user model, and IS unit-checked.
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

    // The view of every model in this one's module-reachable scope, so that
    // cross-module unit inference constraints (module inputs/outputs) can
    // resolve submodel variable types. A stdlib model is in the map when this
    // model instantiates one, on the same rule as any other module target.
    //
    // What can be missing from the map is the NAME, not this handle:
    // `model_scope_models` seeds the scope with `project.models(db)`'s entry
    // for the root's canonical name whenever the project holds that name, so
    //
    //   - the name is absent only when the project holds NO model under it
    //     (this handle was renamed or deleted while a caller kept it), and the
    //     lookup below then returns `None` -- the signal that there is nothing
    //     to check;
    //   - if a DIFFERENT handle occupies the name, the map holds that other
    //     model's view and `target_model` is that model, not this one.
    let models: HashMap<Ident<Canonical>, UnitModel> = model_scope_models(db, model, project)
        .values()
        .map(|src_model| {
            let view = unit_model(db, *src_model, project);
            (view.name.clone(), view)
        })
        .collect();

    // Find the target model in the map. A `SourceModel` whose canonical name
    // the project no longer holds has nothing to check here.
    let target_ident = Ident::<Canonical>::new(&model_name);
    let Some(target_model) = models.get(&target_ident) else {
        return;
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
    let inference = crate::units_infer::infer(&models, units_ctx, target_model);
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
        CompilationDiagnostic(Diagnostic {
            model: model_name.clone(),
            variable: None,
            error: DiagnosticError::Model(crate::common::Error {
                kind: ErrorKind::Model,
                code: ErrorCode::UnitMismatch,
                details: Some(detail),
            }),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
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
                let Some(submodel) = models.get(sub_model_name) else {
                    continue;
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
                // (see `init_value_equivalence_group`); the simple textual
                // `identifier_set` would also return `delay_time`, which is what
                // produced the spurious delay3 mismatches in World3.
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
                                CompilationDiagnostic(Diagnostic {
                                    model: model_name.clone(),
                                    variable: Some(var_ident.to_string()),
                                    error: DiagnosticError::Unit(
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
                                    ),
                                    severity: DiagnosticSeverity::Warning,
                                })
                                .accumulate(db);
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
        Ok(Ok(())) => {}
        Ok(Err(errors)) => {
            for (ident, err) in errors.into_iter() {
                CompilationDiagnostic(Diagnostic {
                    model: model_name.clone(),
                    variable: Some(ident.to_string()),
                    error: DiagnosticError::Unit(err),
                    severity: DiagnosticSeverity::Warning,
                })
                .accumulate(db);
            }
        }
        Err(err) => {
            CompilationDiagnostic(Diagnostic {
                model: model_name.clone(),
                variable: None,
                error: DiagnosticError::Model(crate::common::Error {
                    kind: ErrorKind::Model,
                    code: ErrorCode::Generic,
                    details: Some(format!("unit checking failed: {}", err)),
                }),
                severity: DiagnosticSeverity::Warning,
            })
            .accumulate(db);
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
/// are expression STRINGS on the stock/flow `datamodel::Compat`, not variables
/// of the model, so they must be parsed and lowered here to be unit-checked. We
/// synthesize one hidden aux per parameter expression, lower each under the
/// model's variable shapes (so their variable references resolve to real
/// declared-or-inferred units), then compare each computed unit against the
/// unit the block position requires, with the conveyor stock's declared units
/// `S` and the model time unit `t`:
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
// The target model's view, its model name and the inferred-units map are all
// already resolved by the single caller (`check_model_units`), so they are
// passed rather than re-derived. The model's variable shapes and the dimension
// context are read here instead, ONCE, and only past the early return below: a
// model with no conveyor -- almost every model -- then pays nothing for them at
// all.
fn check_conveyor_param_units(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    model_name: &str,
    target_model: &UnitModel,
    units_ctx: &crate::units::Context,
    inferred_units: &HashMap<Ident<Canonical>, crate::datamodel::UnitMap>,
) {
    use crate::ast::Ast;
    use crate::common::{ErrorCode, UnitError};
    use crate::datamodel::UnitMap;
    use crate::units::{UnitOp, Units, combine};

    // One synthesized parameter-expression aux awaiting unit checking.
    struct SynthParam {
        aux: datamodel::Variable,
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

    // A synthetic parameter aux: scalar, no declared units (we compare its
    // computed units manually), a reserved synthetic name that cannot collide
    // with a user variable.
    let push_param = |synth_params: &mut Vec<SynthParam>,
                      key: &str,
                      expr: &str,
                      expected: UnitMap,
                      stock: &str,
                      label: &str| {
        synth_params.push(SynthParam {
            aux: datamodel::Variable::Aux(datamodel::Aux {
                ident: format!("$\u{205a}conveyor\u{205a}{key}"),
                equation: datamodel::Equation::Scalar(expr.to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
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

    // Parse and lower each synthesized parameter aux on its own, under the
    // shapes of the model's variables -- each explicit variable's and each
    // helper's, through the same two per-name shape functions the fragment
    // constructors resolve a dependency with, so a parameter expression
    // resolves a reference exactly as an ordinary equation does -- and
    // unit-check it against the model's view, which is what its references
    // resolve their units through. A module instance is left out: the `Expr2`
    // tier reads only a shape's dimensions, which an instance has none of.
    // Nothing synthesized here enters a memo: the auxes must not add
    // constraints to the model under analysis, or reach another reader.
    let dim_ctx = project_dimensions_context(db, project);
    let mut shapes: crate::common::IdentMap<Ident<Canonical>, crate::compiler::fragment::DepShape> =
        Default::default();
    for (name, sv) in model.variables(db) {
        if sv.kind(db) != SourceVariableKind::Module {
            shapes.insert(Ident::new(name), source_dep_shape(db, *sv, project));
        }
    }
    // An explicit variable wins a name collision with a helper, as
    // `DeclaredName::resolve` decides it (unreachable today: helper names
    // carry the reserved `$⁚` prefix).
    for (name, meta) in model_implicit_var_info(db, model, project) {
        if !meta.is_module {
            shapes
                .entry(Ident::new(name))
                .or_insert_with(|| implicit_dep_shape(db, project, meta));
        }
    }
    let scope = crate::ast::LoweringScope {
        dimensions: dim_ctx,
        shapes: &shapes,
        model_name: target_model.name.as_str(),
    };
    let synth_ctx = crate::variable::ParseContext::new(dim_ctx, units_ctx);

    for param in &synth_params {
        let mut dummy: Vec<crate::capture::ImplicitVar> = Vec::new();
        let parsed = crate::variable::parse_var(&synth_ctx, &param.aux, &mut dummy, |mi| {
            Ok(Some(mi.clone()))
        });
        let lowered = crate::model::lower_variable(&scope, &parsed);
        // A malformed expression has no AST (it surfaces as a separate compile
        // error), so there is nothing to unit-check -- skip it.
        let expr = match lowered.ast() {
            Some(Ast::Scalar(expr)) | Some(Ast::ApplyToAll(_, expr)) => expr,
            _ => continue,
        };

        let diagnostic_detail = match crate::units_check::evaluate_expr_units(
            units_ctx,
            inferred_units,
            target_model,
            expr,
        ) {
            // A determinate unit that disagrees with what the block requires.
            Ok(Units::Explicit(actual)) if actual != param.expected => Some(format!(
                "conveyor '{}' {}: computed units '{}' don't match the expected units '{}'",
                param.stock, param.label, actual, param.expected
            )),
            // Matches, or a pure constant (compatible with any expected unit).
            Ok(_) => None,
            // A dependency's units are unknown -- skip, not a dimensional error.
            Err(UnitError::ConsistencyError(ErrorCode::DoesNotExist, _, _)) => None,
            // An internal dimensional inconsistency in the expression itself
            // (e.g. adding incompatible units); surface it against the conveyor.
            Err(err) => Some(format!(
                "conveyor '{}' {}: {}",
                param.stock, param.label, err
            )),
        };

        if let Some(detail) = diagnostic_detail {
            CompilationDiagnostic(Diagnostic {
                model: model_name.to_string(),
                variable: Some(param.stock.clone()),
                error: DiagnosticError::Unit(UnitError::ConsistencyError(
                    ErrorCode::UnitMismatch,
                    crate::builtins::Loc::default(),
                    Some(detail),
                )),
                severity: DiagnosticSeverity::Warning,
            })
            .accumulate(db);
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
        let diagnostics = check_model_units::accumulated::<CompilationDiagnostic>(
            &db,
            sync.models["a"].source,
            sync.project,
        );

        assert!(
            diagnostics.iter().any(|cd| matches!(
                &cd.0.error,
                DiagnosticError::Model(e) if e.code == ErrorCode::UnitMismatch
            )),
            "a model's own unit error must still be reported despite the cycle, got: {:?}",
            diagnostics.iter().map(|cd| &cd.0).collect::<Vec<_>>()
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
        match &d.error {
            DiagnosticError::Unit(unit_err) => unit_err.to_string(),
            _ => String::new(),
        }
    }

    /// Drive `check_model_units` over a project's `main` model and return the
    /// conveyor unit diagnostics -- the `Unit` warnings whose detail names a
    /// conveyor (so ordinary unit warnings are excluded).
    fn conveyor_unit_warnings(project: &crate::datamodel::Project) -> Vec<Diagnostic> {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, project);
        let source = sync.models["main"].source;
        check_model_units::accumulated::<CompilationDiagnostic>(&db, source, sync.project)
            .into_iter()
            .map(|cd| cd.0.clone())
            .filter(|d| unit_detail(d).contains("conveyor"))
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
