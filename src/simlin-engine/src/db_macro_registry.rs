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
//! (`macro_registry_build_error`) and its typed `(ErrorCode, message)`
//! stored on the `SourceProject::macro_registry_build_error` input; this
//! query reads it back, surfaces it as a project-level diagnostic carrying
//! `MacroRegistry::build`'s own `ErrorCode`, and returns it so
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
/// empty) resolution registry plus the build error, if any.
/// Decoupled from `crate::common::Error` so this can be a salsa-tracked
/// return without adding `salsa::Update` to the crate-wide error type; the
/// (typed `ErrorCode`, message) pair is everything the compile entry and the
/// project-level diagnostic need (`sim_err!(NotSimulatable, msg)` plus the
/// diagnostic's `ErrorCode`).
// `Debug` is feature-gated to match `module_functions::MacroRegistry`,
// whose `Debug` is only derived under `debug-derive`; an unconditional
// `Debug` here would fail the default (non-`debug-derive`) build.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, salsa::Update)]
pub(crate) struct MacroRegistryResult {
    pub registry: crate::module_functions::MacroRegistry,
    /// `Some((code, message))` when `MacroRegistry::build` rejected the macro
    /// set (duplicate name, macro/model collision, recursion cycle). `code`
    /// is `MacroRegistry::build`'s own typed `ErrorCode` -- carried through
    /// rather than re-derived from the message prose -- so the diagnostic is
    /// tagged with the authoritative code. The registry is then empty.
    pub build_error: Option<(crate::common::ErrorCode, String)>,
}

/// Run `MacroRegistry::build` over the project's *datamodel* models and
/// return its typed `(ErrorCode, message)`, if it failed. Done on the
/// datamodel `Vec<Model>` (not the synced name-keyed map) so duplicate /
/// colliding model names -- which collapse in the
/// `HashMap<String, SourceModel>` -- are still detectable (macros.AC5.3).
/// Returns `None` for a valid macro set, including every macro-free project
/// (the build short-circuits when no model carries a `macro_spec`).
///
/// The `ErrorCode` is taken directly from `MacroRegistry::build`'s `Err`
/// (`CircularDependency` for an AC5.2 recursion cycle, `DuplicateMacroName`
/// for an AC5.3 duplicate name / macro-model collision) and threaded through
/// `SourceProject::macro_registry_build_error` so `project_macro_registry`
/// can tag the diagnostic with the authoritative code instead of re-deriving
/// it from the message prose.
///
/// Called from `sync_from_datamodel` / `sync_from_datamodel_incremental`,
/// which then store the result on `SourceProject::macro_registry_build_error`.
pub(crate) fn macro_registry_build_error(
    project: &datamodel::Project,
) -> Option<(crate::common::ErrorCode, String)> {
    match crate::module_functions::MacroRegistry::build(&project.models) {
        Ok(_) => None,
        Err(err) => Some((
            err.code,
            err.get_details()
                .unwrap_or_else(|| "invalid macro definitions".to_string()),
        )),
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
/// `SourceProject::macro_registry_build_error` as the typed
/// `(ErrorCode, message)` pair `MacroRegistry::build` produced. When that
/// error is `Some`, it is accumulated as a project-level `Diagnostic`
/// carrying that exact `ErrorCode` (so it surfaces through
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
    // `Vec<Model>`. The `ErrorCode` is `MacroRegistry::build`'s own typed
    // code (`CircularDependency` for an AC5.2 cycle, `DuplicateMacroName`
    // for an AC5.3 duplicate / collision), threaded through verbatim -- not
    // re-derived from the message prose -- so a future reword of the build
    // error message cannot silently mis-tag this diagnostic.
    if let Some((code, message)) = project.macro_registry_build_error(db).clone() {
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
            build_error: Some((code, message)),
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

#[cfg(test)]
mod tests {
    //! Lock-in for the typed-`ErrorCode` propagation: `macro_registry_build_error`
    //! must surface `MacroRegistry::build`'s *own* `Err.code`, not a code
    //! re-derived from the message prose. The oracle assertion compares the
    //! stored code against `MacroRegistry::build`'s actual returned `Err.code`
    //! on the same models, so it can never drift from the producer -- and a
    //! future reword of either build error message cannot silently mis-tag
    //! the downstream diagnostic (the original Phase 3 Minor #4 hazard).

    use super::macro_registry_build_error;
    use crate::common::ErrorCode;
    use crate::datamodel::{Aux, Equation, MacroSpec, Model, Variable};
    use crate::module_functions::MacroRegistry;
    use crate::testutils::x_project;

    /// A non-macro scalar aux body variable.
    fn aux(ident: &str, equation: &str) -> Variable {
        Variable::Aux(Aux {
            ident: ident.to_string(),
            equation: Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: crate::datamodel::Compat::default(),
        })
    }

    /// An ordinary (non-macro) model with the given name.
    fn plain_model(name: &str) -> Model {
        Model {
            name: name.to_string(),
            sim_specs: None,
            variables: vec![aux("x", "1")],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }
    }

    /// A macro-marked model `name(params...)` whose body variable is
    /// `name = <body_equation>` (mirrors `module_functions::tests::macro_model`).
    fn macro_model(name: &str, params: &[&str], body_equation: &str) -> Model {
        let mut variables = vec![aux(name, body_equation)];
        for p in params {
            variables.push(aux(p, "0"));
        }
        Model {
            name: name.to_string(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: Some(MacroSpec {
                parameters: params.iter().map(|s| s.to_string()).collect(),
                primary_output: name.to_string(),
                additional_outputs: vec![],
            }),
        }
    }

    /// Oracle: whatever `ErrorCode` `MacroRegistry::build` returns in its
    /// `Err`, `macro_registry_build_error` stores *that exact code* -- never
    /// one heuristically recovered from the message text.
    fn assert_propagates_build_code(models: &[Model]) {
        let project = x_project(Default::default(), models);
        let build_err = MacroRegistry::build(&project.models)
            .expect_err("fixture is expected to fail registry build");
        let stored =
            macro_registry_build_error(&project).expect("a build failure must be surfaced");
        assert_eq!(
            stored.0, build_err.code,
            "stored ErrorCode must equal MacroRegistry::build's own Err.code, \
             not a code re-derived from the message",
        );
        assert_eq!(
            stored.1,
            build_err.get_details().unwrap_or_default(),
            "the message text must be carried through verbatim",
        );
    }

    #[test]
    fn valid_macro_set_has_no_build_error() {
        let models = vec![
            plain_model("main"),
            macro_model("mymacro", &["a", "b"], "a * b"),
        ];
        assert!(
            macro_registry_build_error(&x_project(Default::default(), &models)).is_none(),
            "a valid macro set must not surface a build error",
        );
    }

    #[test]
    fn self_recursive_macro_propagates_circular_dependency_code() {
        // macros.AC5.2: a self-recursive macro -> CircularDependency, carried
        // through verbatim (not recovered from "recursive macro:" prose).
        let models = vec![macro_model("a", &["x"], "a(x) + 1")];
        assert_propagates_build_code(&models);
        let stored = macro_registry_build_error(&x_project(Default::default(), &models)).unwrap();
        assert_eq!(stored.0, ErrorCode::CircularDependency);
    }

    #[test]
    fn mutually_recursive_macros_propagate_circular_dependency_code() {
        // macros.AC5.2: a -> b -> a.
        let models = vec![
            macro_model("a", &["x"], "b(x)"),
            macro_model("b", &["y"], "a(y)"),
        ];
        assert_propagates_build_code(&models);
        let stored = macro_registry_build_error(&x_project(Default::default(), &models)).unwrap();
        assert_eq!(stored.0, ErrorCode::CircularDependency);
    }

    #[test]
    fn duplicate_macro_name_propagates_duplicate_macro_name_code() {
        // macros.AC5.3: two macros named `foo` -> DuplicateMacroName, carried
        // through verbatim (not the `else` fallthrough of a prose check).
        let models = vec![
            macro_model("foo", &["a"], "a"),
            macro_model("foo", &["b"], "b + 1"),
        ];
        assert_propagates_build_code(&models);
        let stored = macro_registry_build_error(&x_project(Default::default(), &models)).unwrap();
        assert_eq!(stored.0, ErrorCode::DuplicateMacroName);
    }

    #[test]
    fn macro_model_name_collision_propagates_duplicate_macro_name_code() {
        // macros.AC5.3: a macro named `main` colliding with the `main` model.
        let models = vec![plain_model("main"), macro_model("main", &["a"], "a")];
        assert_propagates_build_code(&models);
        let stored = macro_registry_build_error(&x_project(Default::default(), &models)).unwrap();
        assert_eq!(stored.0, ErrorCode::DuplicateMacroName);
    }
}
