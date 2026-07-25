// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the two cached model-compilation stage queries (`db::stages`).
//!
//! Three claims are pinned here:
//!
//!   1. **Value**: the cached `model_stage0` / `model_stage1` equal what the
//!      independently-written constructors build -- the datamodel-driven
//!      `ModelStage0::new_cached` for Stage0, and `Project::from_salsa`'s own
//!      lowering for Stage1.
//!   2. **Cost (GH #966)**: whole-project unit diagnostics build each model's
//!      two stages AT MOST ONCE per revision, not once per (model, model) pair.
//!   3. **Diagnostics do not move**: every diagnostic that reached a harvest
//!      point before the stage construction moved out of `check_model_units`
//!      still reaches it.

use super::*;
use crate::common::{Canonical, ErrorCode, Ident};
use crate::datamodel;
use crate::model::{ModelStage0, ModelStage1};
use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

// ── fixtures ────────────────────────────────────────────────────────────

/// A project with three user models: a `main` that instantiates two
/// sub-models, so Stage1 lowering actually walks the scope map.
fn three_model_project() -> datamodel::Project {
    let sub_a = x_model(
        "sub_a",
        vec![x_aux("input", "0", None), x_aux("out", "input * 2", None)],
    );
    let sub_b = x_model(
        "sub_b",
        vec![x_aux("input", "0", None), x_aux("out", "input + 1", None)],
    );
    let main = x_model(
        "main",
        vec![
            x_aux("driver", "5", None),
            x_module("sub_a", &[("driver", "sub_a.input")], None),
            x_module("sub_b", &[("driver", "sub_b.input")], None),
            x_aux("combined", "sub_a.out + sub_b.out", None),
        ],
    );
    x_project(sim_specs_with_units("month"), &[main, sub_a, sub_b])
}

/// The same shape plus a dimension and an apply-to-all variable, so Stage0's
/// dimension resolution and Stage1's array lowering are exercised.
fn arrayed_module_project() -> datamodel::Project {
    let mut project = three_model_project();
    project.dimensions = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec!["north".to_string(), "south".to_string()],
    )];
    let main = project
        .models
        .iter_mut()
        .find(|m| m.name == "main")
        .expect("fixture has a main model");
    main.variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "pop".to_string(),
            equation: datamodel::Equation::ApplyToAll(
                vec!["Region".to_string()],
                "driver * 2".to_string(),
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    main.variables.push(x_aux("total_pop", "SUM(pop[*])", None));
    project
}

/// The datamodel model with the given name.
fn dm_model<'a>(project: &'a datamodel::Project, name: &str) -> &'a datamodel::Model {
    project
        .models
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("fixture has no model named {name}"))
}

/// Everything the unit pass accumulates for one model, drained through the
/// direct `check_model_units::accumulated` harvest point -- the one the
/// conveyor parameter tests in `db::units` use.
fn unit_pass_diagnostics(
    db: &SimlinDb,
    model: SourceModel,
    project: SourceProject,
) -> Vec<&CompilationDiagnostic> {
    crate::db::units::check_model_units::accumulated::<CompilationDiagnostic>(db, model, project)
}

// ── 1. value oracles ────────────────────────────────────────────────────

/// The cached `model_stage0` must equal what the datamodel-driven
/// `ModelStage0::new_cached` builds for the same model.
///
/// `new_cached` is an independently written constructor (it derives the
/// module-ident set and the duplicate-ident errors from the `datamodel::Model`
/// rather than from the salsa inputs), so this is a real cross-check and not a
/// restatement of the query body. It is only a valid oracle for a macro-free
/// project: `new_cached` builds a single-model `MacroRegistry`, while the query
/// reads the project-wide one.
#[test]
fn cached_stage0_equals_datamodel_driven_constructor() {
    for project in [three_model_project(), arrayed_module_project()] {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let dims = project_datamodel_dims(&db, sync.project);
        let units_ctx = project_units_context(&db, sync.project);

        for name in ["main", "sub_a", "sub_b"] {
            let source = sync.models[name].source;
            let oracle = ModelStage0::new_cached(
                &db,
                source,
                sync.project,
                dm_model(&project, name),
                dims,
                units_ctx,
                false,
            );
            assert!(
                *model_stage0(&db, source, sync.project) == oracle,
                "cached model_stage0 for `{name}` must equal the datamodel-driven build"
            );
        }
    }
}

/// The cached `model_stage1`'s lowered variables must equal the ones
/// `Project::from_salsa` produces for the same model.
///
/// Only the fields `from_salsa` leaves alone are compared: it post-processes
/// each `ModelStage1` after construction (`model_deps.take()`, then
/// `set_dependencies`, which fills `instantiations` and extends `errors`), so
/// those three fields legitimately differ. `variables` -- the lowering output
/// this query exists to cache -- is the part that must match exactly.
#[test]
fn cached_stage1_lowering_equals_project_from_salsa() {
    for project in [three_model_project(), arrayed_module_project()] {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let oracle_project = crate::project::Project::from(project.clone());

        for name in ["main", "sub_a", "sub_b"] {
            let ident: Ident<Canonical> = Ident::new(name);
            let cached: &ModelStage1 = model_stage1(&db, sync.models[name].source, sync.project);
            let oracle = &oracle_project.models[&ident];

            assert_eq!(cached.name, oracle.name);
            assert_eq!(cached.display_name, oracle.display_name);
            assert_eq!(cached.implicit, oracle.implicit);
            assert_eq!(cached.is_macro, oracle.is_macro);
            assert_eq!(cached.macro_params, oracle.macro_params);
            assert!(
                cached.variables == oracle.variables,
                "cached model_stage1 lowering for `{name}` must equal Project::from_salsa's"
            );
        }
    }
}

// ── 2. the three Stage0 semantics decisions ─────────────────────────────

/// A model is implicit/stdlib only when the `stdlib⁚` prefix is followed by a
/// name that is actually in `stdlib::MODEL_NAMES`.
///
/// `db/units.rs`'s deleted `build_model_s0` used the bare prefix; the unified
/// query takes `Project::from_salsa`'s stricter rule. Both are inert for unit
/// checking today (nothing on the units path reads `implicit`), so this pins
/// the decision at the only place it is observable.
#[test]
fn model_is_stdlib_requires_a_known_stdlib_suffix() {
    assert!(crate::db::stages::model_is_stdlib("stdlib\u{205A}smth1"));
    assert!(crate::db::stages::model_is_stdlib("stdlib\u{205A}trend"));
    // Prefix present, suffix names no stdlib model: a user model, not a
    // generic template.
    assert!(!crate::db::stages::model_is_stdlib("stdlib\u{205A}bogus"));
    assert!(!crate::db::stages::model_is_stdlib("main"));
    assert!(!crate::db::stages::model_is_stdlib("smth1"));
}

/// Every model `db::sync` splices in satisfies the strict predicate, so
/// tightening the rule cannot orphan a real stdlib template.
///
/// `build_stdlib_models` walks `stdlib::MODEL_NAMES` and names each model
/// `format!("stdlib\u{{205A}}{name}")`, so a spliced model's suffix is in
/// `MODEL_NAMES` by construction. This asserts that end-to-end over a synced
/// project rather than trusting the construction, since `source_model_is_stdlib`
/// now gates the unit-check skip too (GH #988).
#[test]
fn every_spliced_stdlib_model_satisfies_the_strict_predicate() {
    let db = SimlinDb::default();
    let project = x_project(
        sim_specs_with_units("month"),
        &[x_model("main", vec![x_aux("x", "1", None)])],
    );
    let sync = sync_from_datamodel(&db, &project);

    let mut seen = 0usize;
    for (canonical, source) in sync.project.models(&db) {
        let prefixed = canonical.starts_with("stdlib\u{205A}");
        assert_eq!(
            prefixed,
            crate::db::source_model_is_stdlib(&db, *source),
            "spliced model '{canonical}' must be classified by the strict predicate"
        );
        seen += usize::from(prefixed);
    }
    assert_eq!(
        seen,
        crate::stdlib::MODEL_NAMES.len(),
        "every stdlib model should be spliced into a synced project"
    );
}

/// The DISPLAY name is canonicalized before the predicate sees it: the
/// project's model map is canonically keyed, so an imported `Stdlib⁚Smth1` is
/// the same model as `stdlib⁚smth1` and must classify the same way.
#[test]
fn source_model_is_stdlib_canonicalizes_the_display_name() {
    let db = SimlinDb::default();
    let project = x_project(
        sim_specs_with_units("month"),
        &[
            x_model("main", vec![x_aux("x", "1", None)]),
            x_model("Stdlib\u{205A}Smth1", vec![x_aux("input", "1", None)]),
        ],
    );
    let sync = sync_from_datamodel(&db, &project);
    let shadow = sync.models["stdlib\u{205A}smth1"].source;
    assert_eq!(
        shadow.name(&db),
        "Stdlib\u{205A}Smth1",
        "display name is raw"
    );
    assert!(
        crate::db::source_model_is_stdlib(&db, shadow),
        "the predicate must canonicalize before testing the prefix and suffix"
    );
}

/// A stdlib model's variable parses treat EVERY variable name as
/// module-backed; a user model's extra set is empty.
///
/// Inert today (no stdlib body calls `PREVIOUS`/`INIT`, the only consumer of
/// the module-ident set), but load-bearing for cache sharing: the same rule in
/// both construction paths means one `ModuleIdentContext` and one set of
/// per-variable parse memos.
#[test]
fn stdlib_models_add_every_variable_name_to_the_module_ident_set() {
    let db = SimlinDb::default();
    let project = three_model_project();
    let sync = sync_from_datamodel(&db, &project);

    let smth1 = sync.models["stdlib\u{205A}smth1"].source;
    let mut stdlib_extra = crate::db::stages::extra_module_idents(&db, smth1, true);
    stdlib_extra.sort();
    let mut expected: Vec<String> = smth1.variables(&db).keys().cloned().collect();
    expected.sort();
    assert_eq!(
        stdlib_extra, expected,
        "a stdlib model contributes all of its variable names"
    );
    assert!(
        !expected.is_empty(),
        "the fixture stdlib model has variables"
    );

    let main = sync.models["main"].source;
    assert!(
        crate::db::stages::extra_module_idents(&db, main, false).is_empty(),
        "a user model contributes no extra module idents"
    );
}

/// `model_stage0` actually PASSES the stdlib extra idents on -- pinned on the
/// interned `ModuleIdentContext` identity, not on a parse result.
///
/// This is the wiring the previous version of these tests could not see. The
/// rule is inert in every stage VALUE (no stdlib body calls `PREVIOUS`/`INIT`),
/// so reverting the query's call site to `vec![]` left the whole engine suite
/// green while quietly minting a SECOND set of per-variable parse memos under a
/// different key -- losing the cache sharing with `Project::from_salsa` that is
/// the entire stated reason for the rule. `Stage0Context` is the one place that
/// wiring can now go wrong, and interned contexts compare by identity, so the
/// difference is directly observable even though the parse is not.
#[test]
fn model_stage0_parses_a_stdlib_model_under_the_extended_module_context() {
    let db = SimlinDb::default();
    let project = three_model_project();
    let sync = sync_from_datamodel(&db, &project);

    let smth1 = sync.models["stdlib\u{205A}smth1"].source;
    let stdlib_ctx = crate::db::stages::model_stage0_context(&db, smth1, sync.project);
    assert!(stdlib_ctx.is_stdlib);
    assert_ne!(
        stdlib_ctx.module_idents,
        model_module_ident_context(&db, smth1, sync.project, vec![]),
        "a stdlib model must be parsed under an EXTENDED module-ident context, \
         not the bare one a user model gets"
    );

    // A user model adds nothing, so it must land on exactly the bare context --
    // that shared identity is what keeps one set of parse memos.
    let main = sync.models["main"].source;
    let main_ctx = crate::db::stages::model_stage0_context(&db, main, sync.project);
    assert!(!main_ctx.is_stdlib);
    assert_eq!(
        main_ctx.module_idents,
        model_module_ident_context(&db, main, sync.project, vec![]),
        "a user model's stage context must be the bare module-ident context"
    );

    // And the query reads its `implicit` flag from that same derivation, so the
    // context cannot be derived correctly while the query uses another one.
    assert_eq!(
        model_stage0(&db, smth1, sync.project).implicit,
        stdlib_ctx.is_stdlib
    );
    assert_eq!(
        model_stage0(&db, main, sync.project).implicit,
        main_ctx.is_stdlib
    );
}

/// A genuine stdlib model's cached Stage0 equals the datamodel-driven
/// constructor's IMPLICIT build -- which independently applies both of the
/// decisions above (`implicit: true`, and every variable name in the
/// module-ident set).
///
/// `npv` is the fixture rather than the more representative `smth1` because
/// `ModelStage0`'s derived `PartialEq` compares the parsed `f64` constants, and
/// every SMOOTH/DELAY/TREND template declares `initial_value = NAN`: `NaN !=
/// NaN` makes those stages unequal even to a bit-identical rebuild. `npv`
/// carries no NaN literal, so equality is meaningful there.
#[test]
fn cached_stdlib_stage0_equals_implicit_datamodel_build() {
    let db = SimlinDb::default();
    // Every stdlib model is spliced into every synced project, referenced or
    // not, so a bare `main` is enough to reach one.
    let main = x_model("main", vec![x_aux("input", "1", None)]);
    let project = x_project(sim_specs_with_units("month"), &[main]);
    let sync = sync_from_datamodel(&db, &project);

    let source = sync.models["stdlib\u{205A}npv"].source;
    let cached = model_stage0(&db, source, sync.project);
    assert!(cached.implicit, "a stdlib model's Stage0 is implicit");
    assert!(
        cached.variables.len() > 1,
        "the fixture stdlib model has a body to compare"
    );

    let stdlib_dm = crate::stdlib::get("npv").expect("npv is a stdlib model");
    let oracle = ModelStage0::new_cached(
        &db,
        source,
        sync.project,
        &stdlib_dm,
        project_datamodel_dims(&db, sync.project),
        project_units_context(&db, sync.project),
        true,
    );
    assert!(
        *cached == oracle,
        "a stdlib model's cached Stage0 must equal the implicit datamodel-driven build"
    );
}

/// Two variables whose names canonicalize identically collapse last-wins on
/// the canonical-keyed `variables` map, so Stage0 records the collision as a
/// model-level `DuplicateVariable` error (GH #885/#891).
///
/// `db/units.rs`'s deleted builder set `errors: None`; the unified query takes
/// `Project::from_salsa`'s recording behaviour. Nothing on the units path reads
/// `ModelStage0::errors`, so the change adds no unit diagnostic -- asserted
/// below so a future reader of the field cannot start emitting one silently.
#[test]
fn stage0_records_duplicate_canonical_ident_errors() {
    let db = SimlinDb::default();
    let model = x_model(
        "main",
        vec![x_aux("net flow", "1", None), x_aux("net_flow", "2", None)],
    );
    let project = x_project(sim_specs_with_units("month"), &[model]);
    let sync = sync_from_datamodel(&db, &project);
    let source = sync.models["main"].source;

    let errors = model_stage0(&db, source, sync.project)
        .errors
        .as_ref()
        .expect("duplicate canonical idents must record a model-level error");
    let dup = errors
        .iter()
        .find(|e| e.code == ErrorCode::DuplicateVariable)
        .unwrap_or_else(|| panic!("expected a DuplicateVariable error, got: {errors:?}"));
    let msg = dup.details.as_deref().unwrap_or("");
    assert!(
        msg.contains("'net flow'") && msg.contains("'net_flow'"),
        "message should name both colliding spellings, got: {msg}"
    );
    // The lowered stage carries it forward, and the unit pass still ignores it.
    assert!(model_stage1(&db, source, sync.project).errors.is_some());
    let unit_diagnostics = unit_pass_diagnostics(&db, source, sync.project);
    assert!(
        !unit_diagnostics
            .iter()
            .any(|cd| matches!(&cd.0.error, DiagnosticError::Model(e)
                if e.code == ErrorCode::DuplicateVariable)),
        "the unit pass must not start reporting Stage0's model errors: {unit_diagnostics:?}"
    );
}

// ── 3. the GH #966 cost claim ───────────────────────────────────────────

/// Collecting whole-project diagnostics BUILDS each model's Stage0 and Stage1
/// exactly once, not once per (checked model, project model) pair.
///
/// What the execution counts prove: the two query BODIES ran `models.len()`
/// times each across a whole-project diagnostic collection that runs the unit
/// pass on three models. Before this change the same collection constructed
/// `3 x models.len()` of each. The second half re-reads every model's stages
/// once per model -- the exact pre-#966 access pattern -- and shows the build
/// count does not move, so the linearity is a property of the cache and not of
/// the order `collect_all_diagnostics` happens to walk in.
///
/// What they do NOT prove: anything about wall-clock time; anything about a
/// LATER revision (no incrementality claim is made here); and nothing about the
/// number of memo READS, which is still quadratic in the model count until the
/// lowering scope is narrowed to the module-reachable closure.
///
/// What makes the measurement sound: `reset_stage_executions()` immediately
/// before the measured region. The counters are thread-local, which keeps a
/// PARALLEL run's other tests off this thread, but that is not on its own
/// enough -- under `--test-threads=1` libtest runs every test on this same
/// thread, so an earlier test's counts would otherwise still be sitting there.
/// The `SimlinDb` is local to this test, so no memo predates the reset either.
#[test]
fn whole_project_diagnostics_build_each_models_stages_once() {
    let db = SimlinDb::default();
    let project = three_model_project();
    let sync = sync_from_datamodel(&db, &project);
    let n_models = sync.project.models(&db).len();
    // Three user models plus the spliced stdlib set: the pre-#966 cost was
    // 3 x n_models of each stage, so n_models and 3 x n_models are far apart.
    assert!(
        n_models > 3,
        "fixture should have several models: {n_models}"
    );

    reset_stage_executions();
    let diagnostics = collect_all_diagnostics(&db, sync.project);
    let after_collect = stage_executions();
    assert_eq!(
        after_collect,
        StageExecutions {
            stage0: n_models,
            stage1: n_models,
        },
        "each model's stages must be built exactly once per revision, got {after_collect:?} \
         for {n_models} models (diagnostics: {diagnostics:?})"
    );

    // The pre-#966 access pattern, made explicit: every model's stages, once
    // per model. n_models^2 reads, still n_models builds.
    let sources: Vec<SourceModel> = sync.project.models(&db).values().copied().collect();
    for _target in &sources {
        for m in &sources {
            let _ = model_stage1(&db, *m, sync.project);
            let _ = model_stage0(&db, *m, sync.project);
        }
    }
    assert_eq!(
        stage_executions(),
        after_collect,
        "re-reading every model's stages must not rebuild any of them"
    );
}

// ── 4. diagnostics did not move ─────────────────────────────────────────

/// A project whose `main` model has a genuine dimensional inconsistency.
fn unit_mismatch_project() -> datamodel::Project {
    let main = x_model(
        "main",
        vec![
            x_aux("a", "1", Some("widget")),
            x_aux("b", "2", Some("gadget")),
            x_aux("c", "a + b", Some("widget")),
        ],
    );
    x_project(sim_specs_with_units("month"), &[main])
}

/// A unit warning still reaches BOTH harvest points after the stage
/// construction moved out of `check_model_units`: the direct
/// `check_model_units::accumulated` drain (what the conveyor parameter tests
/// use) and `collect_all_diagnostics` (via `model_all_diagnostics`).
#[test]
fn unit_warning_reaches_both_harvest_points() {
    let db = SimlinDb::default();
    let project = unit_mismatch_project();
    let sync = sync_from_datamodel(&db, &project);
    let source = sync.models["main"].source;

    let direct = unit_pass_diagnostics(&db, source, sync.project);
    assert!(
        direct
            .iter()
            .any(|cd| matches!(&cd.0.error, DiagnosticError::Unit(_))),
        "check_model_units::accumulated must still carry the unit warning: {direct:?}"
    );

    let all = collect_all_diagnostics(&db, sync.project);
    assert!(
        all.iter().any(|d| d.model == "main"
            && d.severity == DiagnosticSeverity::Warning
            && matches!(&d.error, DiagnosticError::Unit(_))),
        "collect_all_diagnostics must still carry the unit warning: {all:?}"
    );
}

/// GH #988: a model carrying the stdlib PREFIX but an unknown suffix is a user
/// model, so the unit pass checks it instead of skipping it.
///
/// The skip gate used to accept the bare prefix, which meant an imported model
/// under the stdlib namespace silently lost unit checking while the stage query
/// -- using the strict rule -- staged it as an ordinary user model. Both now
/// call `source_model_is_stdlib`; a real stdlib model is still skipped.
#[test]
fn a_stdlib_prefixed_model_with_an_unknown_suffix_is_unit_checked() {
    let db = SimlinDb::default();
    let namespaced = x_model(
        "stdlib\u{205A}bogus",
        vec![
            x_aux("a", "1", Some("widget")),
            x_aux("b", "2", Some("gadget")),
            x_aux("c", "a + b", Some("widget")),
        ],
    );
    let project = x_project(
        sim_specs_with_units("month"),
        &[x_model("main", vec![x_aux("x", "1", None)]), namespaced],
    );
    let sync = sync_from_datamodel(&db, &project);

    let source = sync.models["stdlib\u{205A}bogus"].source;
    let direct = unit_pass_diagnostics(&db, source, sync.project);
    assert!(
        direct
            .iter()
            .any(|cd| matches!(&cd.0.error, DiagnosticError::Unit(_))),
        "a prefix-only model must be unit-checked, not skipped: {direct:?}"
    );

    // A genuine stdlib model is still skipped: it is a generic template whose
    // formal parameters are unitless until instantiated.
    let real = sync.models["stdlib\u{205A}smth1"].source;
    assert!(
        unit_pass_diagnostics(&db, real, sync.project).is_empty(),
        "a real stdlib model must still be skipped by the unit pass"
    );
}
