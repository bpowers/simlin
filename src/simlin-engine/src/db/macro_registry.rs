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
//! into one entry and become indistinguishable post-sync. The query instead
//! derives the build error on demand from two salsa inputs: the ordered,
//! pre-dedup `SourceProject::macro_declarations` list (canonical name +
//! `macro_spec`, one entry per project model in declaration order) supplies
//! the duplicate/collision data Passes 1-2 need, and the macro-marked
//! models' body equations -- read from `models` -- drive Pass 3's recursion
//! check. It surfaces the result as a project-level diagnostic carrying
//! `MacroRegistry::build`'s own `ErrorCode`, and returns it so
//! `compile_project_incremental` can fail with a clear message.
//!
//! This is a submodule of `db` (a child of `db.rs`, like `ltm_ir`) kept in
//! its own file purely to keep `db.rs` under the per-file line cap
//! (`scripts/lint-project.sh` rule 2); callers in the `db` submodules reach
//! it via `crate::db::macro_registry::...`.

use std::collections::HashMap;

use salsa::Accumulator;

use crate::datamodel;
use crate::db::{
    CompilationDiagnostic, Db, Diagnostic, DiagnosticError, DiagnosticSeverity, SourceProject,
    SourceVariable, datamodel_variable_from_source,
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

/// Reconstruct the project's `datamodel::Model` list from salsa inputs in
/// declaration order, then run the UNCHANGED `MacroRegistry::build` over it
/// and return its typed `(ErrorCode, message)`, if it failed.
///
/// Strategy (a) of the salsa-pipeline cleanup: rather than store
/// `MacroRegistry::build`'s result on the input (a derived value recomputed
/// every sync), the build error is now derived on demand inside the tracked
/// query. To guarantee the produced `(ErrorCode, message)` is BYTE-IDENTICAL
/// to building over the original datamodel `Vec<Model>`, we reconstruct that
/// `Vec<Model>` from the minimal raw inputs `build` actually reads and call
/// the same function -- no logic is duplicated or re-derived:
///
/// - **Passes 1-2** (duplicate macro name, macro/model name collision) read
///   each model's name + `macro_spec` IN DECLARATION ORDER and report the
///   FIRST-detected duplicate/collision. The ordered, pre-dedup
///   `macro_declarations` input carries exactly that -- the canonical name
///   and `macro_spec` of every *project* model (stdlib excluded), preserving
///   the duplicate / colliding entries the name-keyed `models` map collapses.
///   `MacroRegistry::build` re-canonicalizes each `model.name`, and
///   canonicalization is idempotent on an already-canonical string, so the
///   error message text (which embeds the canonical name) is unchanged.
/// - **Pass 3** (recursion cycle) parses the macro-marked models' body
///   equations. Pass 3 only runs after Passes 1-2 confirm uniqueness, so for
///   any model set that reaches it every macro name is unique and the body
///   variables are fully recoverable from the (deduplicated) `models` map.
///   `find_cycle` sorts roots and uses `BTreeSet` successors, so the reported
///   cycle path is independent of model iteration order; declaration order
///   here only matters for Passes 1-2.
///
/// Non-macro models get an empty body: Passes 1-2 never read non-macro bodies
/// and Pass 3 skips non-macro models, so their absence cannot change the
/// result. Returns `None` for a valid macro set, including every macro-free
/// project (the build short-circuits when no model carries a `macro_spec`).
fn build_error_from_inputs(
    db: &dyn Db,
    project: SourceProject,
) -> Option<(crate::common::ErrorCode, String)> {
    let models = reconstruct_project_models(db, project);
    match crate::module_functions::MacroRegistry::build(&models) {
        Ok(_) => None,
        Err(err) => Some((
            err.code,
            err.get_details()
                .unwrap_or_else(|| "invalid macro definitions".to_string()),
        )),
    }
}

/// Reconstruct the project's models in declaration order from salsa inputs,
/// carrying just what `MacroRegistry::build` reads: each model's canonical
/// name + `macro_spec` (from `macro_declarations`) plus the macro-marked
/// models' body variables (from `models`, recovered by canonical name --
/// safe because a duplicate macro name is caught by Pass 1 before Pass 3's
/// body walk, so any name reaching Pass 3 is unique).
fn reconstruct_project_models(db: &dyn Db, project: SourceProject) -> Vec<datamodel::Model> {
    let source_models = project.models(db);
    project
        .macro_declarations(db)
        .iter()
        .map(|(canonical_name, macro_spec)| {
            // Pass 3 only walks macro-marked models' bodies, so reconstruct
            // body variables only for them; a non-macro model's empty body is
            // never read.
            let variables = if macro_spec.is_some() {
                source_models
                    .get(canonical_name)
                    .map(|source_model| {
                        source_model
                            .variables(db)
                            .values()
                            .map(|sv| datamodel_variable_from_source(db, *sv))
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            datamodel::Model {
                name: canonical_name.clone(),
                sim_specs: None,
                variables,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: macro_spec.clone(),
            }
        })
        .collect()
}

/// Build the per-project macro registry, salsa-tracked and keyed on the
/// project's `SourceProject`. The query reads ONLY the `macro_declarations`
/// input (Passes 1-2) and the macro-marked models' body equations (Pass 3),
/// so editing a non-macro variable's equation does not invalidate it.
///
/// Validation (macros.AC5.2 recursion cycle, macros.AC5.3 duplicate name /
/// macro-model collision) is derived on demand by `build_error_from_inputs`,
/// which reconstructs the project's `datamodel::Model` list (in declaration
/// order, from the `macro_declarations` input plus the macro bodies) and runs
/// the UNCHANGED `MacroRegistry::build` on it -- so its typed
/// `(ErrorCode, message)` is byte-identical to building over the original
/// datamodel `Vec<Model>`. When the build fails, the error is accumulated as
/// a project-level `Diagnostic` carrying that exact `ErrorCode` (so it
/// surfaces through `collect_all_diagnostics`, mirroring
/// `project_units_context`) and returned in `build_error` (so the plain
/// `compile_project_incremental` entry can fail with a clear message), and
/// the resolution registry is returned empty so the offending macros' callers
/// are not treated as macro calls -- the compile fails with the registry
/// error, not a confusing cascade.
///
/// For a *valid* project every model name is unique, so rebuilding the
/// resolution map from the (deduplicated) `SourceModel`s here is exact.
#[salsa::tracked(returns(ref))]
pub(crate) fn project_macro_registry(db: &dyn Db, project: SourceProject) -> MacroRegistryResult {
    // Derive the authoritative validation result on demand. The `ErrorCode`
    // is `MacroRegistry::build`'s own typed code (`CircularDependency` for an
    // AC5.2 cycle, `DuplicateMacroName` for an AC5.3 duplicate / collision),
    // carried through verbatim from `build`'s `Err` -- not re-derived from the
    // message prose -- so a future reword of the build error message cannot
    // silently mis-tag this diagnostic.
    if let Some((code, message)) = build_error_from_inputs(db, project) {
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
            .map(|sv| datamodel_variable_from_source(db, *sv))
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

/// Map every body `SourceVariable` of a macro-marked model to that model's
/// name (#554).
///
/// `parse_source_variable_impl` needs "is this variable a macro body, and
/// which macro?" to thread the enclosing-macro context into
/// `BuiltinVisitor` (so a macro body's renamed same-named `init`/`previous`
/// builtin resolves to the intrinsic instead of recursing into the like-named
/// macro). `SourceVariable.model_name` does NOT answer this -- it is a
/// *Module* variable's referenced target model (empty for non-Module vars,
/// see `db::source_variable_from_datamodel`), not the owning model -- and a
/// per-parse reverse scan of every model would be O(vars x models). This
/// salsa-tracked query builds the reverse map once per project (memoized;
/// only macro-marked models contribute, and they are few and small), so the
/// hot `parse_source_variable_with_module_context` does a single map lookup.
///
/// Keyed on `SourceProject`, so it is recomputed only when the project's
/// models change. Non-macro models contribute nothing, so an ordinary
/// (macro-free) project yields an empty map and zero per-parse overhead.
#[salsa::tracked(returns(ref))]
pub(crate) fn macro_body_owner(
    db: &dyn Db,
    project: SourceProject,
) -> HashMap<SourceVariable, String> {
    let mut owner: HashMap<SourceVariable, String> = HashMap::new();
    for source_model in project.models(db).values() {
        if source_model.macro_spec(db).is_none() {
            continue;
        }
        let model_name = source_model.name(db).clone();
        for svar in source_model.variables(db).values() {
            owner.insert(*svar, model_name.clone());
        }
    }
    owner
}

/// The owning macro model's name when `var` is a body variable of a
/// macro-marked model, else `None` (#554).
///
/// `parse_source_variable_impl` threads this into `BuiltinVisitor` as
/// `enclosing_model` so the same-named-opcode-intrinsic exception fires: a
/// macro body's renamed `init`/`previous` builtin (the importer's
/// `INITIAL`->`INIT` / `SAMPLE IF TRUE`->`PREVIOUS` rename) resolves to the
/// intrinsic instead of recursing into the like-named macro. This is a thin
/// reader of the salsa-cached [`macro_body_owner`] map -- `parse` already
/// memoizes, and `SourceVariable.model_name` cannot answer this (it is a
/// *Module* variable's referenced target, empty for non-Module vars).
pub(crate) fn enclosing_macro_for_var(
    db: &dyn Db,
    project: SourceProject,
    var: SourceVariable,
) -> Option<&str> {
    macro_body_owner(db, project).get(&var).map(|s| s.as_str())
}

#[cfg(test)]
mod tests {
    //! Lock-in for the typed-`ErrorCode` propagation: `project_macro_registry`
    //! must surface `MacroRegistry::build`'s *own* `Err.code`, not a code
    //! re-derived from the message prose. The oracle assertion compares the
    //! query's `build_error` against `MacroRegistry::build`'s actual returned
    //! `Err.code` on the same datamodel models, so it can never drift from the
    //! producer -- and a future reword of either build error message cannot
    //! silently mis-tag the downstream diagnostic (the original Phase 3 Minor
    //! #4 hazard). Driving the real salsa pipeline (sync -> query) also exercises
    //! the demand-driven derivation end to end (`macro_declarations` input ->
    //! `build_error_from_inputs` -> `MacroRegistry::build`).

    use super::project_macro_registry;
    use crate::common::ErrorCode;
    use crate::datamodel::{Aux, Equation, MacroSpec, Model, Variable};
    use crate::db::{SimlinDb, sync_from_datamodel, sync_from_datamodel_incremental};
    use crate::module_functions::MacroRegistry;
    use crate::testutils::x_project;

    /// Drive the production salsa pipeline -- sync the project into a fresh
    /// `SimlinDb`, then read `project_macro_registry`'s `build_error`. This is
    /// what `compile_project_incremental` and the project-level diagnostic see,
    /// so the oracle locks in the *query*'s observable behavior, not an
    /// internal helper.
    fn build_error_via_query(project: &crate::datamodel::Project) -> Option<(ErrorCode, String)> {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, project);
        project_macro_registry(&db, sync.project)
            .build_error
            .clone()
    }

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

    /// Oracle: whatever `ErrorCode`/message `MacroRegistry::build` returns in
    /// its `Err` over the ORIGINAL datamodel models, `project_macro_registry`
    /// surfaces *that exact code and text* -- never one heuristically recovered
    /// from the message. Comparing against `MacroRegistry::build` directly is
    /// what makes the byte-identity un-driftable: the demand-driven derivation
    /// reconstructs the model list, but its build error must match building the
    /// original list.
    fn assert_propagates_build_code(models: &[Model]) {
        let project = x_project(Default::default(), models);
        let build_err = MacroRegistry::build(&project.models)
            .expect_err("fixture is expected to fail registry build");
        let surfaced = build_error_via_query(&project).expect("a build failure must be surfaced");
        assert_eq!(
            surfaced.0, build_err.code,
            "surfaced ErrorCode must equal MacroRegistry::build's own Err.code, \
             not a code re-derived from the message",
        );
        assert_eq!(
            surfaced.1,
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
            build_error_via_query(&x_project(Default::default(), &models)).is_none(),
            "a valid macro set must not surface a build error",
        );
    }

    #[test]
    fn self_recursive_macro_propagates_circular_dependency_code() {
        // macros.AC5.2: a self-recursive macro -> CircularDependency, carried
        // through verbatim (not recovered from "recursive macro:" prose).
        let models = vec![macro_model("a", &["x"], "a(x) + 1")];
        assert_propagates_build_code(&models);
        let surfaced = build_error_via_query(&x_project(Default::default(), &models)).unwrap();
        assert_eq!(surfaced.0, ErrorCode::CircularDependency);
    }

    #[test]
    fn mutually_recursive_macros_propagate_circular_dependency_code() {
        // macros.AC5.2: a -> b -> a.
        let models = vec![
            macro_model("a", &["x"], "b(x)"),
            macro_model("b", &["y"], "a(y)"),
        ];
        assert_propagates_build_code(&models);
        let surfaced = build_error_via_query(&x_project(Default::default(), &models)).unwrap();
        assert_eq!(surfaced.0, ErrorCode::CircularDependency);
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
        let surfaced = build_error_via_query(&x_project(Default::default(), &models)).unwrap();
        assert_eq!(surfaced.0, ErrorCode::DuplicateMacroName);
    }

    #[test]
    fn macro_model_name_collision_propagates_duplicate_macro_name_code() {
        // macros.AC5.3: a macro named `main` colliding with the `main` model.
        let models = vec![plain_model("main"), macro_model("main", &["a"], "a")];
        assert_propagates_build_code(&models);
        let surfaced = build_error_via_query(&x_project(Default::default(), &models)).unwrap();
        assert_eq!(surfaced.0, ErrorCode::DuplicateMacroName);
    }

    /// Invalidation contract: `project_macro_registry` depends only on
    /// `macro_declarations` + the macro-marked models' bodies. Editing a
    /// NON-macro variable's equation must leave the query cached -- it is a
    /// `returns(ref)` query, so a cache hit returns the SAME memoized value
    /// (pointer-equal), mirroring `db/fragment_cache_tests.rs`. If the query
    /// instead depended on every model's variables (or recomputed every sync,
    /// as the old input-field design did), the pointer would differ.
    #[test]
    fn non_macro_equation_edit_does_not_invalidate_query() {
        let models = vec![
            plain_model("main"),
            macro_model("mymacro", &["a", "b"], "a * b"),
        ];
        let project = x_project(Default::default(), &models);

        let mut db = SimlinDb::default();
        let state1 = sync_from_datamodel_incremental(&mut db, &project, None);

        // Prime the cache and capture the memoized result's pointer.
        let (ptr_before, build_error_before) = {
            let sync1 = state1.to_sync_result();
            let result = project_macro_registry(&db, sync1.project);
            (result as *const _, result.build_error.clone())
        };
        assert!(
            build_error_before.is_none(),
            "the valid macro set must have no build error",
        );

        // Edit ONLY the non-macro `main` model's `x` variable equation.
        let mut project2 = project.clone();
        let main_idx = project2
            .models
            .iter()
            .position(|m| m.name == "main")
            .expect("main model present");
        project2.models[main_idx].variables[0] = aux("x", "42");

        let state2 = sync_from_datamodel_incremental(&mut db, &project2, Some(&state1));
        let sync2 = state2.to_sync_result();
        let result2 = project_macro_registry(&db, sync2.project);
        let ptr_after = result2 as *const _;

        assert_eq!(
            ptr_before, ptr_after,
            "project_macro_registry must be a cache hit (pointer-equal) when only \
             a non-macro variable's equation changes -- the query depends solely on \
             macro_declarations + the macro models' bodies",
        );
    }
}
