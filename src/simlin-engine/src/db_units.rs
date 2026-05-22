// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// Salsa-tracked unit-checking orchestration: it reads salsa inputs, rebuilds
// the temporary ModelStage0/ModelStage1 representations, runs the pure unit
// inference (`units_infer`) and consistency checking (`units_check`) cores,
// and accumulates the resulting diagnostics. The dimensional-analysis logic
// itself is the Functional Core in `units.rs`/`units_infer.rs`/`units_check.rs`;
// this module only wires it into the salsa graph.

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
//! This is a top-level module (a sibling of `db`, like `db_macro_registry`
//! and `db_dep_graph`) rather than a submodule of `db.rs` purely to keep
//! `db.rs` under the per-file line cap (`scripts/lint-project.sh` rule 2);
//! `db::model_all_diagnostics` reaches it via `crate::db_units::...`.

use std::collections::{HashMap, HashSet};

use salsa::Accumulator;

use crate::common::{Canonical, Ident};
use crate::datamodel;
use crate::db::{
    CompilationDiagnostic, Db, Diagnostic, DiagnosticError, DiagnosticSeverity, SourceModel,
    SourceProject, model_module_ident_context, parse_source_variable_with_module_context,
    project_datamodel_dims, project_units_context,
};

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

/// Per-model tracked function that performs unit inference and checking,
/// accumulating unit warnings/errors through the salsa accumulator.
///
/// Builds temporary ModelStage0/ModelStage1 representations from the
/// salsa-cached parsed variables, then runs the same unit inference and
/// checking pipeline as the old `run_default_model_checks` callback.
/// Unit mismatches are accumulated as DiagnosticSeverity::Warning to
/// match the old-path behavior where unit issues don't block simulation.
///
/// Stdlib (implicit) models are skipped because they are generic
/// templates that only make sense when instantiated with specific inputs.
#[salsa::tracked]
pub fn check_model_units(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use crate::common::{ErrorCode, ErrorKind};
    use crate::dimensions::DimensionsContext;
    use crate::model::{ModelStage0, ModelStage1, ScopeStage0, VariableStage0};

    // Skip stdlib models -- they are generic and unit checking doesn't
    // apply until instantiated with concrete inputs. Stdlib model names
    // start with the "stdlib\u{205A}" prefix (two-dot punctuation separator).
    if model.name(db).starts_with("stdlib\u{205A}") {
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
    let dm_dims = project_datamodel_dims(db, project);
    let dim_context = DimensionsContext::from(dm_dims.as_slice());

    // Helper: build a ModelStage0 from a SourceModel's parsed variables.
    let build_model_s0 = |src_model: &SourceModel, is_stdlib: bool| -> ModelStage0 {
        let src_vars = src_model.variables(db);
        let module_ctx = model_module_ident_context(db, *src_model, project, vec![]);
        let mut var_list: Vec<VariableStage0> = Vec::new();
        let mut implicit_dm: Vec<datamodel::Variable> = Vec::new();
        for (_name, svar) in src_vars.iter() {
            let parsed = parse_source_variable_with_module_context(db, *svar, project, module_ctx);
            var_list.push(parsed.variable.clone());
            implicit_dm.extend(parsed.implicit_vars.iter().cloned());
        }
        // Parse implicit vars (SMOOTH/DELAY expansion).
        let mut dummy: Vec<datamodel::Variable> = Vec::new();
        var_list.extend(implicit_dm.into_iter().map(|dm_var| {
            crate::variable::parse_var(dm_dims, &dm_var, &mut dummy, units_ctx, |mi| {
                Ok(Some(mi.clone()))
            })
        }));
        let variables: HashMap<Ident<Canonical>, VariableStage0> = var_list
            .into_iter()
            .map(|v| (Ident::new(v.ident()), v))
            .collect();
        ModelStage0 {
            ident: Ident::new(src_model.name(db)),
            display_name: src_model.name(db).clone(),
            variables,
            errors: None,
            implicit: is_stdlib,
            is_macro: src_model.macro_spec(db).is_some(),
        }
    };

    // Build ModelStage0 for all project models so that cross-module unit
    // inference constraints (module inputs/outputs) can resolve submodel
    // variable types. Stdlib models are included in the map because user
    // models may reference them as modules.
    let project_models = project.models(db);
    let mut all_s0: Vec<ModelStage0> = Vec::new();
    for (name, src_model) in project_models.iter() {
        let is_stdlib = name.starts_with("stdlib\u{205A}");
        all_s0.push(build_model_s0(src_model, is_stdlib));
    }

    let models_s0: HashMap<Ident<Canonical>, &ModelStage0> =
        all_s0.iter().map(|m| (m.ident.clone(), m)).collect();

    // Lower all ModelStage0 -> ModelStage1.
    let all_s1: Vec<ModelStage1> = all_s0
        .iter()
        .map(|ms0| {
            let scope = ScopeStage0 {
                models: &models_s0,
                dimensions: &dim_context,
                model_name: ms0.ident.as_str(),
            };
            ModelStage1::new(&scope, ms0)
        })
        .collect();

    let models_s1: HashMap<Ident<Canonical>, &ModelStage1> =
        all_s1.iter().map(|m| (m.name.clone(), m)).collect();

    // Find the target model in the lowered map.
    let target_ident = Ident::<Canonical>::new(&model_name);
    let target_model = match models_s1.get(&target_ident) {
        Some(m) => *m,
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
    let inference = crate::units_infer::infer(&models_s1, units_ctx, target_model);
    if has_declared_units && !inference.conflicts.is_empty() {
        let detail = if inference.conflicts.len() == 1 {
            format!("{}", inference.conflicts[0])
        } else {
            format!(
                "{} dimensional unit conflicts found during inference; first: {}",
                inference.conflicts.len(),
                inference.conflicts[0]
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
        for (var_ident, var) in target_model.variables.iter() {
            if let crate::variable::Variable::Module {
                model_name: sub_model_name,
                inputs,
                ..
            } = var
            {
                // Only check stdlib modules where we know the constraint structure
                if !sub_model_name.as_str().starts_with("stdlib\u{205A}") {
                    continue;
                }
                let submodel = match models_s1.get(sub_model_name) {
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
                // (see `init_value_equivalence_group`); the simple textual
                // `identifier_set` would also return `delay_time`, which is what
                // produced the spurious delay3 mismatches in World3.
                let stock_init_deps: Vec<HashSet<Ident<Canonical>>> = submodel
                    .variables
                    .values()
                    .filter_map(|sv| {
                        if matches!(sv, crate::variable::Variable::Stock { .. }) {
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

    // Run unit checking.
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
}
