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
    let mut all = Vec::new();
    for source_model in project.models(db).values() {
        let diags = collect_model_diagnostics(db, *source_model, project);
        all.extend(diags);
    }
    all
}
