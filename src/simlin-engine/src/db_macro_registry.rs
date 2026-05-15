// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// Salsa-tracked plumbing around the pure `module_functions` macro registry:
// it reads salsa inputs, reconstructs datamodel models, and accumulates a
// diagnostic. The validation/resolution logic itself is the Functional Core
// in `module_functions.rs`; this module only wires it into the salsa graph.

//! The per-project macro-registry salsa query.
//!
//! `project_macro_registry` is the single salsa-tracked place the
//! per-project macro registry is materialized for the compile pipeline. It
//! wraps the pure `module_functions::MacroRegistry` (the resolver +
//! validator) and exposes `MacroRegistryResult` (the resolution registry
//! plus a build-error message).
//!
//! Registry-build *validation* (macros.AC5.2 recursion cycle, macros.AC5.3
//! duplicate macro name / macro-model name collision) cannot be re-derived
//! from `SourceProject.models`: that map is keyed by canonical model name,
//! so two same-named macros -- or a macro colliding with a model -- collapse
//! into one entry and become indistinguishable post-sync. The validation is
//! therefore run at sync time on the datamodel `Vec<Model>`
//! (`macro_registry_build_error`) and its message stored on the
//! `SourceProject::macro_registry_build_error` input; this query reads it
//! back, surfaces it as a project-level diagnostic, and returns it so
//! `compile_project_incremental` can fail with a clear message.
//!
//! This is a top-level module (a sibling of `db`, like `db_ltm_ir` and
//! `ltm_agg`) rather than a submodule of `db.rs` purely to keep `db.rs`
//! under the per-file line cap (`scripts/lint-project.sh` rule 2); callers
//! in the `db` submodules reach it via `crate::db_macro_registry::...`.

use salsa::Accumulator;

use crate::datamodel;
use crate::db::{
    CompilationDiagnostic, Db, Diagnostic, DiagnosticError, DiagnosticSeverity, SourceProject,
    reconstruct_variable,
};

/// The result of building the per-project macro registry: the (possibly
/// empty) resolution registry plus the build-error message, if any.
/// Decoupled from `crate::common::Error` so this can be a salsa-tracked
/// return without adding `salsa::Update` to the crate-wide error type; the
/// message is the only thing the compile entry needs (`sim_err!(
/// NotSimulatable, msg)`).
// `Debug` is feature-gated to match `module_functions::MacroRegistry`,
// whose `Debug` is only derived under `debug-derive`; an unconditional
// `Debug` here would fail the default (non-`debug-derive`) build.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, salsa::Update)]
pub(crate) struct MacroRegistryResult {
    pub registry: crate::module_functions::MacroRegistry,
    /// `Some(message)` when `MacroRegistry::build` rejected the macro set
    /// (duplicate name, macro/model collision, recursion cycle). The
    /// registry is then empty.
    pub build_error: Option<String>,
}

/// Run `MacroRegistry::build` over the project's *datamodel* models and
/// return the error message, if any. Done on the datamodel `Vec<Model>`
/// (not the synced name-keyed map) so duplicate / colliding model names --
/// which collapse in the `HashMap<String, SourceModel>` -- are still
/// detectable (macros.AC5.3). Returns `None` for a valid macro set,
/// including every macro-free project (the build short-circuits when no
/// model carries a `macro_spec`).
///
/// Called from `sync_from_datamodel` / `sync_from_datamodel_incremental`,
/// which then store the result on `SourceProject::macro_registry_build_error`.
pub(crate) fn macro_registry_build_error(project: &datamodel::Project) -> Option<String> {
    match crate::module_functions::MacroRegistry::build(&project.models) {
        Ok(_) => None,
        Err(err) => Some(
            err.get_details()
                .unwrap_or_else(|| "invalid macro definitions".to_string()),
        ),
    }
}

/// Build the per-project macro registry, salsa-tracked and keyed on the
/// project's `SourceModel`s. Only the macro-marked models'
/// `macro_spec`/body-equation text feed `MacroRegistry::build`, so editing a
/// non-macro variable does not invalidate this query.
///
/// Validation (macros.AC5.2 recursion cycle, macros.AC5.3 duplicate name /
/// macro-model collision) is *not* re-run here: it was already computed at
/// sync time on the datamodel `Vec<Model>` (the only representation where
/// duplicate / colliding model names survive -- `SourceProject.models` is a
/// name-keyed `HashMap` that collapses them) and is read back from
/// `SourceProject::macro_registry_build_error`. When that error is `Some`,
/// it is accumulated as a project-level `Diagnostic` (so it surfaces through
/// `collect_all_diagnostics`, mirroring `project_units_context`) and
/// returned in `build_error` (so the plain `compile_project_incremental`
/// entry can fail with a clear message), and the resolution registry is
/// returned empty so the offending macros' callers are not treated as macro
/// calls -- the compile fails with the registry error, not a confusing
/// cascade.
///
/// For a *valid* project every model name is unique, so rebuilding the
/// resolution map from the (deduplicated) `SourceModel`s here is exact.
#[salsa::tracked(returns(ref))]
pub(crate) fn project_macro_registry(db: &dyn Db, project: SourceProject) -> MacroRegistryResult {
    // Authoritative validation result, computed at sync from the datamodel
    // `Vec<Model>`.
    if let Some(message) = project.macro_registry_build_error(db).clone() {
        // Reconstruct the typed error for the diagnostic. The sync-time
        // build only kept the message; recover the code from the message
        // shape (cycle messages start with "recursive macro:").
        let code = if message.starts_with("recursive macro") {
            crate::common::ErrorCode::CircularDependency
        } else {
            crate::common::ErrorCode::DuplicateMacroName
        };
        // Surface through `collect_all_diagnostics` (project-level: no
        // specific model/variable). Accumulate directly like
        // `project_units_context` -- this query is in the call graph of
        // `model_all_diagnostics` via `module_ident_context`.
        CompilationDiagnostic(Diagnostic {
            model: String::new(),
            variable: None,
            error: DiagnosticError::Model(crate::common::Error::new(
                crate::common::ErrorKind::Model,
                code,
                Some(message.clone()),
            )),
            severity: DiagnosticSeverity::Error,
        })
        .accumulate(db);
        return MacroRegistryResult {
            registry: crate::module_functions::MacroRegistry::default(),
            build_error: Some(message),
        };
    }

    // Valid project: reconstruct the resolution registry from the
    // (unique-named) macro-marked models. Body variables drive nothing
    // here (validation already passed), but `MacroRegistry::build` walks
    // them for the -- now known-acyclic -- cycle check; reconstructing
    // them keeps a single build path.
    let project_models = project.models(db);
    let mut models: Vec<datamodel::Model> = Vec::with_capacity(project_models.len());
    for source_model in project_models.values() {
        let macro_spec = source_model.macro_spec(db).clone();
        if macro_spec.is_none() {
            continue;
        }
        let variables: Vec<datamodel::Variable> = source_model
            .variables(db)
            .values()
            .map(|sv| reconstruct_variable(db, *sv))
            .collect();
        models.push(datamodel::Model {
            name: source_model.name(db).clone(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec,
        });
    }

    let registry = crate::module_functions::MacroRegistry::build(&models)
        .unwrap_or_else(|_| crate::module_functions::MacroRegistry::default());
    MacroRegistryResult {
        registry,
        build_error: None,
    }
}
