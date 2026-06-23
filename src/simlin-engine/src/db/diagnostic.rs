// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compilation diagnostics: the salsa `CompilationDiagnostic` accumulator,
//! the typed `Diagnostic` value (severity + per-model/per-variable context),
//! the per-model triggering query `model_all_diagnostics`, and the
//! accumulator-drain helpers `collect_model_diagnostics` /
//! `collect_all_diagnostics`.
//!
//! `model_all_diagnostics` is the single per-model query that drives every
//! diagnostic source: it triggers `compile_var_fragment` per variable (the
//! emission half lives in `db.rs`), the unit-check pass, and -- when LTM is
//! enabled -- the LTM fragment-diagnostic pass. The two `collect_*` helpers
//! drain the accumulated `CompilationDiagnostic`s for one model or the whole
//! synced project.
//!
//! `model_all_diagnostics` performs an untracked read so it always
//! re-executes: see the in-body comment for why that is load-bearing for
//! diagnostic stability across unrelated salsa revision bumps. Without it,
//! salsa's accumulator-DFS pruning silently drops previously-collected
//! diagnostics whenever the query is validated-but-not-re-executed.

use super::*;
use crate::common::{EquationError, Error, UnitError};

#[salsa::accumulator]
pub struct CompilationDiagnostic(pub Diagnostic);

/// A single compilation diagnostic emitted by tracked functions.
/// Carries enough context (model name, optional variable name) for
/// downstream formatting without re-walking the model tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Copy)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub model: String,
    pub variable: Option<String>,
    pub error: DiagnosticError,
    pub severity: DiagnosticSeverity,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticError {
    Equation(EquationError),
    Model(Error),
    Unit(UnitError),
    Assembly(String),
}

/// Per-model tracked function that triggers diagnostic accumulation from
/// all compilation stages. The salsa accumulator is the sole error source
/// for diagnostic reporting -- this function does not read struct fields.
///
/// Triggers three diagnostic sources:
/// 1. `compile_var_fragment` for each variable -- accumulates parse-level
///    equation errors (EmptyEquation, syntax errors), unit definition
///    syntax errors (bad unit strings), and compilation-level errors
///    (BadTable, MismatchedDimensions, etc.)
/// 2. `check_model_units` -- accumulates unit inference/checking warnings
/// 3. When LTM is enabled, `model_ltm_fragment_diagnostics` -- accumulates
///    LTM assembly diagnostics: the auto-flip warning that surfaces when
///    the element-level largest SCC exceeds `MAX_LTM_SCC_NODES` (emitted
///    by `model_ltm_variables`, which the fragment-diagnostic pass drives
///    internally), and a compile-failure warning for any LTM synthetic
///    variable whose fragment fails to compile. Gated on `ltm_enabled` so
///    we don't run LTM synthesis on projects that never requested it.
#[salsa::tracked]
pub fn model_all_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    // Force this query to re-execute on every revision rather than being
    // validated-but-skipped.
    //
    // The two `collect_*` helpers drain diagnostics via
    // `model_all_diagnostics::accumulated::<CompilationDiagnostic>(..)`. salsa
    // 0.26's `accumulated_by` does a DFS that prunes any dependency subtree
    // whose root memo's `accumulated_inputs` flag is `Empty`. That flag is set
    // to `Any` only while the query *executes* (when it reads a child whose
    // memo already holds accumulated values, e.g. `check_model_units`). When an
    // UNRELATED salsa input changes (a `SetLoopName` patch touching only
    // `SourceModel.pinned_loops`, a sim-spec edit, ...) the revision bumps but
    // none of this query's tracked inputs change, so salsa validates the memo
    // without re-executing it -- and the deep-verify path recomputes the
    // pruning flag from each input's `maybe_changed_after` result, which
    // reports `Empty` for a self-accumulating child (a memo's
    // `accumulated_inputs` reflects only its *inputs*, never whether the memo
    // itself accumulated). The flag collapses to `Empty`, the DFS prunes the
    // whole subtree, and the previously-collected diagnostics silently vanish
    // on the next collection (engine `test_diagnostics_stable_across_*`;
    // libsimlin saw `get_errors` zero out after an unrelated patch). The inner
    // memos still hold their accumulated maps, so re-executing this trigger --
    // a cheap O(num_vars) walk of already-memoized children -- is enough to
    // refresh the flag to `Any` and let the DFS descend. An untracked read
    // makes this query ineligible for shallow/deep validation, so it always
    // re-executes (salsa `Database::report_untracked_read`: "queries which
    // report untracked reads will be re-executed in the next revision").
    db.report_untracked_read();

    let source_vars = model.variables(db);

    // Trigger compile_var_fragment for each variable. This is a superset
    // of parse_source_variable_with_module_context: it first accumulates
    // unit definition syntax errors from the parsed variable, then checks
    // for equation parse errors, then proceeds with compilation which can
    // surface additional errors like BadTable, MismatchedDimensions, etc.
    //
    // The symbolic fragment is role-independent (`time`/`dt` lower to
    // `LoadGlobalVar` at fixed slots, never through the layout), so this
    // diagnostic pass produces byte-identical fragments to assembly and the
    // two SHARE one salsa cache entry per variable -- the win from dropping
    // `is_root`. The module inputs are empty because we are not in an
    // assembly context: this is purely for error detection.
    let empty_inputs = ModuleInputSet::empty(db);
    for (_var_name, source_var) in source_vars.iter() {
        let _fragment = compile_var_fragment(db, *source_var, model, project, empty_inputs);
    }

    // Trigger unit checking. This is a separate tracked function so
    // that unit inference results are individually cached and
    // invalidated only when unit-relevant inputs change. It lives in the
    // `db::units` submodule (kept out of `db.rs` for the per-file line
    // cap).
    crate::db::units::check_model_units(db, model, project);

    // Validate each explicit module variable's input wiring (GH #806 sibling):
    // a reference whose `dst` names no input of the target model, or whose bare
    // `src` names no variable in this model, is silently dropped at assembly and
    // the port reads its default -- a quietly-wrong simulation. The salsa path
    // had lost the legacy `BadModuleInputDst`/`BadModuleInputSrc` check.
    model_module_wiring_diagnostics(db, model, project);

    // When LTM is enabled, also trigger the LTM diagnostic pass so that
    // diagnostics accumulated by the LTM pipeline surface through
    // `collect_all_diagnostics`: the auto-flip-to-discovery warning from
    // `model_ltm_variables` and the synthetic-fragment compile-failure
    // warning from `model_ltm_fragment_diagnostics`.
    // `model_ltm_fragment_diagnostics` drives `model_ltm_variables`
    // internally, so the auto-flip warning rides along. Without this
    // call the warnings are invisible even though the LTM pipeline
    // already emitted them. Gated on `ltm_enabled` so projects that never
    // requested LTM pay no LTM synthesis cost here. The diagnostic-
    // collection FFI path (`simlin_project_get_errors`) transiently
    // re-enables `ltm_enabled` for callers who created an LTM simulation,
    // so these warnings reach `simlin-mcp`/`libsimlin`/pysimlin (GH #466).
    if project.ltm_enabled(db) {
        model_ltm_fragment_diagnostics(db, model, project);
    }
}

/// Validate the input wiring of each explicit module variable in `model`.
///
/// A module reference is `{ src, dst }` where `dst` is the module-qualified
/// `{module}·{port}` form naming an input of the target model and `src` is a
/// variable in the enclosing model. `build_module_inputs` SILENTLY DROPS a
/// reference whose `dst` does not match an existing child input -- the port then
/// reads its default and the simulation is quietly wrong, with no error. The
/// legacy monolithic path returned `BadModuleInputDst`/`BadModuleInputSrc` here;
/// the salsa path dropped the check. Re-add it as a Warning (partial-result
/// philosophy: a mis-wired input should not block the rest of the model).
///
/// Validated conservatively to avoid false positives:
/// - empty placeholder endpoints (the new-row UI pattern) are skipped;
/// - only an EXISTING target model is checked (an empty / dangling `model_name`
///   is a separate concern and the empty name is the normal freshly-drawn state);
/// - a `src` is checked only when it is a bare ident (no `·`) and not an engine
///   synthetic (`$⁚…`) -- a qualified cross-module output or temporary is left
///   to the equation checker.
#[salsa::tracked]
pub fn model_module_wiring_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use salsa::Accumulator;

    let source_vars = model.variables(db);
    let project_models = project.models(db);
    let model_name = model.name(db);

    let mut module_names: Vec<&String> = source_vars
        .iter()
        .filter(|(_, sv)| sv.kind(db) == SourceVariableKind::Module)
        .map(|(name, _)| name)
        .collect();
    module_names.sort_unstable();

    let emit = |code: crate::common::ErrorCode, message: String| {
        CompilationDiagnostic(Diagnostic {
            model: model_name.clone(),
            variable: None,
            error: DiagnosticError::Model(Error::new(
                crate::common::ErrorKind::Model,
                code,
                Some(message),
            )),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    };

    for module_name in module_names {
        let svar = &source_vars[module_name];
        let child_canonical = crate::canonicalize(svar.model_name(db));
        let Some(child_model) = project_models.get(child_canonical.as_ref()) else {
            continue;
        };
        let child_vars = child_model.variables(db);
        let prefix = format!("{module_name}\u{00B7}");

        for reference in svar.module_refs(db).iter() {
            let dst = crate::canonicalize(&reference.dst);
            if !dst.is_empty() {
                let resolves = dst
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|port| child_vars.contains_key(port));
                if !resolves {
                    emit(
                        crate::common::ErrorCode::BadModuleInputDst,
                        format!(
                            "module '{module_name}' input wiring target '{}' does not name an input of model '{}'",
                            reference.dst, child_canonical
                        ),
                    );
                }
            }

            let src = crate::canonicalize(&reference.src);
            if !src.is_empty()
                && !src.contains('\u{00B7}')
                && !src.starts_with("$\u{205A}")
                && !source_vars.contains_key(src.as_ref())
            {
                emit(
                    crate::common::ErrorCode::BadModuleInputSrc,
                    format!(
                        "module '{module_name}' input source '{}' does not name a variable in model '{model_name}'",
                        reference.src
                    ),
                );
            }
        }
    }
}

/// Collect all `CompilationDiagnostic`s accumulated during
/// `model_all_diagnostics` for a single model.
pub fn collect_model_diagnostics(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Vec<Diagnostic> {
    model_all_diagnostics::accumulated::<CompilationDiagnostic>(db, model, project)
        .into_iter()
        .map(|cd| cd.0.clone())
        .collect()
}

/// Collect all diagnostics for every model in a synced project.
pub fn collect_all_diagnostics(db: &SimlinDb, project: SourceProject) -> Vec<Diagnostic> {
    let graph = project_module_graph(db, project);

    let mut all = Vec::new();
    for (name, source_model) in project.models(db) {
        // A model that can REACH a module cycle would drive its per-model passes
        // (compile_var_fragment recursing through the submodel) into the salsa
        // cycle panic. Report the cycle for that model and skip its passes. A
        // model that reaches no cycle is processed normally, so a valid model's
        // diagnostics are not hidden by an unrelated draft cycle elsewhere
        // (GH #806).
        if let Some((code, message)) = graph.cycle_error_from(name) {
            all.push(Diagnostic {
                model: name.clone(),
                variable: None,
                error: DiagnosticError::Model(Error::new(
                    crate::common::ErrorKind::Model,
                    code,
                    Some(message),
                )),
                severity: DiagnosticSeverity::Error,
            });
            continue;
        }
        all.extend(collect_model_diagnostics(db, *source_model, project));
    }
    all
}
