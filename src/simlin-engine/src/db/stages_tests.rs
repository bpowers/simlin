// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the two cached model-compilation stage queries (`db::stages`).
//!
//! Three claims are pinned here:
//!
//!   1. **Value**: the cached `model_stage0` / `model_stage1` equal what the
//!      independently-written, salsa-free `ModelStage0::new_in_project` (plus
//!      `ModelStage1::new` over a whole-project scope of those) builds for the
//!      same models -- across every model shape, up to and including the
//!      combined [`every_shape_project`].
//!   2. **Cost (GH #966)**: whole-project unit diagnostics build each model's
//!      two stages AT MOST ONCE per revision, not once per (model, model) pair
//!      -- and `Project::from_salsa` READS those same memos rather than building
//!      a second set.
//!   3. **Diagnostics do not move**: every diagnostic that reached a harvest
//!      point before the stage construction moved out of `check_model_units`
//!      still reaches it.
//!
//! The oracles used to be `ModelStage0::new_cached` (a test-only third copy of
//! the salsa-cached construction) and `Project::from_salsa`'s own inline
//! lowering. Both are gone: the first was deleted, and the second became
//! circular once `from_salsa` started reading these queries. A datamodel-driven
//! constructor that touches no database is the oracle that cannot go circular.
//!
//! # Compare stages with `PartialEq`, never with Debug text
//!
//! `Ast::Arrayed` holds its per-element equations in a `HashMap`, so a stage's
//! Debug rendering is ORDER-UNSTABLE run to run, while the derived `PartialEq`
//! is order-independent. A Debug-text oracle over a fixture with per-element
//! equations therefore reports spurious inequality -- including a stage
//! "differing from itself" -- for a reason that has nothing to do with the code
//! under test.
//!
//! This has cost time twice: once in a throwaway harness written to design the
//! scope-narrowing fixtures, and once in the temporary oracle used to verify
//! that `Project::from_salsa`'s deleted inline build equalled these queries --
//! which sorted Debug strings and was sound only by the accident that its
//! fixtures had no `Equation::Arrayed`.
//!
//! The rule: compare with `PartialEq`. If you must reach for Debug text because
//! a NaN literal defeats `PartialEq` (GH #987/#981 -- `ModelStage0` compares
//! parsed `f64` constants, and every SMOOTH/DELAY/TREND template declares
//! `initial_value = NAN`, so such a stage is not even equal to itself), then it
//! is sound ONLY for fixtures with no per-element equations, and the test must
//! say so.

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

/// An apply-to-all `datamodel::Aux` over one dimension.
fn x_apply_to_all(ident: &str, dim: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::ApplyToAll(vec![dim.to_string()], equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

/// The same shape plus a dimension and apply-to-all variables, so Stage0's
/// dimension resolution and Stage1's array lowering are exercised.
///
/// `sub_a` gains an ARRAYED output that `main` reduces over, which is the one
/// construct that makes the whole-project scope map load-bearing:
/// `ArrayContext::get_variable` resolves `sub_a·out_by_region`'s dimensions by
/// following the module variable into `sub_a`'s Stage0, so with the other
/// models missing from the scope `get_dimensions` returns `None` and the
/// reference lowers as though it were scalar. That silent mis-lowering is
/// exactly what `model_stage1`'s self-insert comment describes -- and, until
/// this fixture, nothing in the crate's tests could see it: emptying the scope
/// map entirely left the whole engine suite green.
fn arrayed_module_project() -> datamodel::Project {
    let mut project = three_model_project();
    project.dimensions = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec!["north".to_string(), "south".to_string()],
    )];
    let sub_a = project
        .models
        .iter_mut()
        .find(|m| m.name == "sub_a")
        .expect("fixture has a sub_a model");
    sub_a
        .variables
        .push(x_apply_to_all("out_by_region", "Region", "input * 3"));

    let main = project
        .models
        .iter_mut()
        .find(|m| m.name == "main")
        .expect("fixture has a main model");
    main.variables
        .push(x_apply_to_all("pop", "Region", "driver * 2"));
    main.variables.push(x_aux("total_pop", "SUM(pop[*])", None));
    main.variables.push(x_aux(
        "sub_region_total",
        "SUM(sub_a.out_by_region[*])",
        None,
    ));
    project
}

/// A TWO-LEVEL module chain, `main -> sub_a -> sub_c`, where `main` makes an
/// arrayed reference that resolves through BOTH hops.
///
/// This separates the two narrowings of `model_stage1`'s scope map that
/// [`arrayed_module_project`] cannot tell apart. `ArrayContext::get_variable`
/// resolves `sub_a·sub_c·out_by_region` by recursing once per hop, looking up
/// each intermediate model in `scope.models`: `main` (self), then `sub_a`
/// (a DIRECT module target of main), then `sub_c` (reachable only THROUGH
/// `sub_a`, and not a module target of `main` at all).
///
/// So a scope narrowed to "self + direct module targets" drops `sub_c`,
/// `get_dimensions` returns `None`, and `SUM(...[*])` lowers as though its
/// argument were scalar -- silently, with no error. Only the TRANSITIVE
/// module-reachable closure is correct, and this fixture is what says so.
fn chain_project() -> datamodel::Project {
    let sub_c = x_model(
        "sub_c",
        vec![
            x_aux("input", "0", None),
            x_apply_to_all("out_by_region", "Region", "input * 3"),
        ],
    );
    let sub_a = x_model(
        "sub_a",
        vec![
            x_aux("input", "0", None),
            x_module("sub_c", &[("input", "sub_c.input")], None),
        ],
    );
    let main = x_model(
        "main",
        vec![
            x_aux("driver", "5", None),
            x_module("sub_a", &[("driver", "sub_a.input")], None),
            x_aux("deep_total", "SUM(sub_a.sub_c.out_by_region[*])", None),
        ],
    );
    let mut project = x_project(sim_specs_with_units("month"), &[main, sub_a, sub_c]);
    project.dimensions = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec!["north".to_string(), "south".to_string()],
    )];
    project
}

/// Every shape `Project::from_salsa`'s deleted inline Stage0 build had to
/// handle, in one project:
///
///   - an IMPLICIT stdlib module instance (`SMTH1`), whose expansion synthesizes
///     module and argument variables that have no `SourceVariable` of their own
///     and so are parsed inside the stage rather than by the per-variable memo;
///   - an EXPLICIT user sub-model instance that `main` READS through
///     (`SUM(sub.out_by_region[*])`), so Stage1 lowering genuinely follows a
///     `module·output` reference into another model's Stage0 and depends on
///     the answer -- instantiating `sub` without reading it would exercise
///     only `resolve_module_input`'s self-entry path;
///   - a MACRO-marked model (`is_macro` / `macro_params` non-default) together
///     with a caller of it, which only classifies correctly under the
///     PROJECT-wide macro registry;
///   - two variables whose names canonicalize identically, the one case where
///     Stage0 records a model-level error.
fn every_shape_project() -> datamodel::Project {
    let sub = x_model(
        "sub",
        vec![
            x_aux("input", "0", None),
            x_aux("out", "input * 2", None),
            x_apply_to_all("out_by_region", "Region", "input * 4"),
        ],
    );
    let scaled_macro = datamodel::Model {
        name: "scaled".to_string(),
        sim_specs: None,
        variables: vec![
            x_aux("scaled", "p1 * p2", None),
            x_aux("p1", "0", None),
            x_aux("p2", "0", None),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(datamodel::MacroSpec {
            parameters: vec!["p1".to_string(), "p2".to_string()],
            primary_output: "scaled".to_string(),
            additional_outputs: vec![],
        }),
    };
    let main = x_model(
        "main",
        vec![
            x_aux("driver", "5", None),
            x_aux("smoothed", "SMTH1(driver, 3)", None),
            x_module("sub", &[("driver", "sub.input")], None),
            x_aux("sub_total", "SUM(sub.out_by_region[*])", None),
            x_aux("scaled_driver", "scaled(driver, 2)", None),
            x_aux("net flow", "1", None),
            x_aux("net_flow", "2", None),
        ],
    );
    let mut project = x_project(sim_specs_with_units("month"), &[main, sub, scaled_macro]);
    project.dimensions = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec!["north".to_string(), "south".to_string()],
    )];
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
/// `ModelStage0::new_in_project` builds for the same model.
///
/// That constructor is the independently written twin: it derives the
/// module-ident set (`collect_module_idents` over the `datamodel::Model`), the
/// macro registry and the duplicate-ident errors (the raw declared-ident list)
/// along completely different routes than the query, which reads interned salsa
/// contexts and memoized groups. So this is a real cross-check, not a
/// restatement of the query body.
///
/// It replaced `ModelStage0::new_cached` as this oracle when that test-only
/// third copy of the salsa-cached construction was deleted.
#[test]
fn cached_stage0_equals_datamodel_driven_constructor() {
    for (fixture, project, names) in [
        (
            "three_model",
            three_model_project(),
            &["main", "sub_a", "sub_b"][..],
        ),
        (
            "arrayed_module",
            arrayed_module_project(),
            &["main", "sub_a", "sub_b"][..],
        ),
        ("chain", chain_project(), &["main", "sub_a", "sub_c"][..]),
    ] {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let dims = project_datamodel_dims(&db, sync.project);
        let units_ctx = project_units_context(&db, sync.project);

        for name in names {
            let source = sync.models[*name].source;
            let oracle = ModelStage0::new_in_project(
                &project.models,
                dm_model(&project, name),
                dims,
                units_ctx,
                false,
            );
            assert!(
                *model_stage0(&db, source, sync.project) == oracle,
                "cached model_stage0 for `{name}` in fixture `{fixture}` must equal the \
                 datamodel-driven build"
            );
        }
    }
}

/// Every model a synced project holds -- the datamodel's own models plus the
/// stdlib models `db::sync` splices into every project -- staged by the
/// datamodel-driven constructor.
///
/// This is the whole-project Stage0 map `Project::from_salsa` used to build
/// inline, reconstructed through the salsa-free constructor so it remains an
/// independent oracle now that `from_salsa` reads these queries instead. The
/// stdlib half is here because the scope map holds it, not because any shipped
/// template can change a lowering -- see
/// [`omitting_stdlib_models_from_the_lowering_scope_is_inert_today`].
///
/// BOTH halves pass the same `project.models` as the macro-registry source, so
/// one oracle scope cannot contain two models staged under different registries
/// -- the hazard `ModelStage0::new_in_project`'s rustdoc describes, which would
/// otherwise be live here since [`every_shape_project`] declares a macro. A
/// stdlib model neither declares nor calls a macro, so this changes no stdlib
/// stage today; it removes the split, not a bug.
fn datamodel_driven_stage0s(
    project: &datamodel::Project,
    dims: &[datamodel::Dimension],
    units_ctx: &crate::units::Context,
) -> Vec<ModelStage0> {
    let mut all: Vec<ModelStage0> = project
        .models
        .iter()
        .map(|m| ModelStage0::new_in_project(&project.models, m, dims, units_ctx, false))
        .collect();
    all.extend(crate::stdlib::MODEL_NAMES.iter().map(|name| {
        let dm = crate::stdlib::get(name).expect("MODEL_NAMES only names real stdlib models");
        ModelStage0::new_in_project(&project.models, &dm, dims, units_ctx, true)
    }));
    all
}

/// Lower [`datamodel_driven_stage0s`] the way `Project::from_salsa` used to:
/// one whole-project `ScopeStage0` per model, keyed by canonical model name.
fn datamodel_driven_stage1s(
    all_s0: &[ModelStage0],
    dims_ctx: &crate::dimensions::DimensionsContext,
) -> HashMap<Ident<Canonical>, ModelStage1> {
    let models_s0: HashMap<Ident<Canonical>, &ModelStage0> =
        all_s0.iter().map(|m| (m.ident.clone(), m)).collect();
    all_s0
        .iter()
        .map(|ms0| {
            let scope = crate::model::ScopeStage0 {
                models: &models_s0,
                dimensions: dims_ctx,
                model_name: ms0.ident.as_str(),
            };
            (ms0.ident.clone(), ModelStage1::new(&scope, ms0))
        })
        .collect()
}

/// The cached `model_stage1` must equal the datamodel-driven whole-project
/// lowering of the same models.
///
/// This used to compare against `Project::from_salsa`'s output. That became
/// circular the moment `from_salsa` started reading this query -- it would have
/// compared a memo against a clone of itself and passed unconditionally -- so
/// the oracle moved to the salsa-free constructors, which is what `from_salsa`
/// was standing in for all along.
///
/// Because the oracle is now a freshly built `ModelStage1` rather than one
/// `from_salsa` has already post-processed, `model_deps` and `errors` are
/// compared too; the old version had to skip them (`model_deps.take()` and
/// `set_dependencies` had already consumed and rewritten them). `instantiations`
/// is `None` on both sides -- neither construction fills it.
#[test]
fn cached_stage1_lowering_equals_datamodel_driven_lowering() {
    for (fixture, project, names) in [
        (
            "three_model",
            three_model_project(),
            &["main", "sub_a", "sub_b"][..],
        ),
        (
            "arrayed_module",
            arrayed_module_project(),
            &["main", "sub_a", "sub_b"][..],
        ),
        ("chain", chain_project(), &["main", "sub_a", "sub_c"][..]),
    ] {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let all_s0 = datamodel_driven_stage0s(
            &project,
            project_datamodel_dims(&db, sync.project),
            project_units_context(&db, sync.project),
        );
        let oracles =
            datamodel_driven_stage1s(&all_s0, project_dimensions_context(&db, sync.project));

        for name in names {
            let ident: Ident<Canonical> = Ident::new(name);
            let cached: &ModelStage1 = model_stage1(&db, sync.models[*name].source, sync.project);
            let oracle = &oracles[&ident];

            assert_eq!(cached.name, oracle.name);
            assert_eq!(cached.display_name, oracle.display_name);
            assert_eq!(cached.implicit, oracle.implicit);
            assert_eq!(cached.is_macro, oracle.is_macro);
            assert_eq!(cached.macro_params, oracle.macro_params);
            assert_eq!(cached.model_deps, oracle.model_deps);
            assert_eq!(cached.errors, oracle.errors);
            assert!(cached.instantiations.is_none() && oracle.instantiations.is_none());
            assert!(
                cached.variables == oracle.variables,
                "cached model_stage1 lowering for `{name}` in fixture `{fixture}` must equal \
                 the datamodel-driven one"
            );
        }
    }
}

/// Omitting the stdlib models from `model_stage1`'s lowering scope changes no
/// lowering TODAY -- and this is the property that makes that true, asserted
/// directly so it becomes a tripwire rather than an assumption.
///
/// Written for whoever does the scope narrowing (the follow-on to GH #966).
/// Four plausible narrowings were probed against the full workspace; "whole
/// project MINUS every `stdlib⁚` model" came back green, and the reason is not
/// missing coverage. The scope map has exactly two consumers, and neither can
/// observe a stdlib model:
///
///   - `ArrayContext::get_variable` follows a `module·output` reference into the
///     target model's Stage0 to read its DIMENSIONS. Every shipped stdlib
///     variable is scalar, so the answer is `None` whether the model is in the
///     map or not.
///   - `resolve_relative` recurses through intermediate models named by a
///     dotted reference. No stdlib model instantiates a module, so none can ever
///     be an intermediate hop.
///
/// One route DOES reach a stdlib entry, and it is deliberately out of scope
/// here: `resolve_relative`'s TERMINAL lookup. A module-input `src` spelled
/// `stdlib⁚<template>·<var>` resolves against the stdlib model itself, so
/// dropping those entries turns that input into a `BadModuleInputSrc`. That is
/// a LOUD error rather than a silent mis-lowering -- the opposite of the failure
/// class this tripwire guards -- and it needs an imported model whose input
/// `src` literally names a stdlib template (ordinary model creation never
/// produces the `⁚` separator). A narrowing that keeps the closure's stdlib
/// targets, rather than skipping `stdlib⁚*` wholesale, avoids it entirely.
///
/// If a future stdlib template gains an arrayed variable or a module instance,
/// this test fails -- and at that moment a closure that skips `stdlib⁚*` becomes
/// a silent mis-lowering, exactly like the one
/// [`arrayed_module_project`] and [`chain_project`] catch for user models. The
/// assertion runs over the SYNCED Stage0s, which is what the scope map actually
/// holds, rather than over the generated stdlib source.
#[test]
fn omitting_stdlib_models_from_the_lowering_scope_is_inert_today() {
    let db = SimlinDb::default();
    let project = x_project(
        sim_specs_with_units("month"),
        &[x_model("main", vec![x_aux("x", "1", None)])],
    );
    let sync = sync_from_datamodel(&db, &project);

    let mut checked = 0usize;
    for source in sync.project.models(&db).values() {
        if !crate::db::source_model_is_stdlib(&db, *source) {
            continue;
        }
        let s0 = model_stage0(&db, *source, sync.project);
        for (ident, var) in s0.variables.iter() {
            assert!(
                var.get_dimensions().is_none(),
                "stdlib variable {}·{ident} is arrayed; a lowering scope that omits stdlib \
                 models can now silently lower a reference to it as scalar",
                s0.ident
            );
            assert!(
                !matches!(var, crate::variable::Variable::Module { .. }),
                "stdlib model {} instantiates a module ({ident}); it can now be an intermediate \
                 hop in `resolve_relative`, so a lowering scope that omits stdlib models can \
                 silently break a chained reference",
                s0.ident
            );
        }
        checked += 1;
    }
    assert_eq!(
        checked,
        crate::stdlib::MODEL_NAMES.len(),
        "every stdlib model must be reached by this check"
    );
}

/// Both cached stages equal the datamodel-driven build for EVERY model shape
/// `Project::from_salsa`'s deleted inline copy handled -- see
/// [`every_shape_project`] for the four.
///
/// The two oracle tests above cover a plain multi-model project; this one is the
/// combined fixture, and it is where the field-by-field equality argument for
/// deleting that copy is actually pinned. Every `ModelStage0` field is compared
/// (the `==` is the derived `PartialEq` over the whole struct): `ident`,
/// `display_name`, `variables` (including the implicit SMOOTH expansion),
/// `errors` (the duplicate-ident pair), `implicit`, `is_macro` and
/// `macro_params`.
///
/// The comparison ranges over the USER models only. The stdlib templates are in
/// the project -- `main` instantiates `stdlib⁚smth1` and both scopes hold all of
/// them -- but SMOOTH/DELAY/TREND bodies declare `initial_value = NAN`, and
/// `ModelStage0` derives `PartialEq`, so a stdlib stage carrying a NaN literal
/// does not even compare equal to ITSELF. Asserting on one would fail for a
/// reason unrelated to what is being tested (GH #987/#981);
/// `cached_stdlib_stage0_equals_implicit_datamodel_build` covers the stdlib side
/// on `npv`, the one template with no NaN.
#[test]
fn cached_stages_equal_the_datamodel_driven_build_for_every_model_shape() {
    let db = SimlinDb::default();
    let project = every_shape_project();
    let sync = sync_from_datamodel(&db, &project);
    let dims = project_datamodel_dims(&db, sync.project);
    let units_ctx = project_units_context(&db, sync.project);

    let all_s0 = datamodel_driven_stage0s(&project, dims, units_ctx);
    let oracle_s1 =
        datamodel_driven_stage1s(&all_s0, project_dimensions_context(&db, sync.project));
    let oracle_s0: HashMap<&Ident<Canonical>, &ModelStage0> =
        all_s0.iter().map(|m| (&m.ident, m)).collect();

    for name in ["main", "sub", "scaled"] {
        let ident: Ident<Canonical> = Ident::new(name);
        let source = sync.models[name].source;
        assert!(
            *model_stage0(&db, source, sync.project) == **oracle_s0.get(&ident).unwrap(),
            "cached model_stage0 for `{name}` must equal the datamodel-driven build"
        );
        assert!(
            *model_stage1(&db, source, sync.project) == oracle_s1[&ident],
            "cached model_stage1 for `{name}` must equal the datamodel-driven lowering"
        );
    }

    // The fixture really does exercise all four shapes -- otherwise the
    // equalities above would be comparing an ordinary project twice.
    let main_s0 = model_stage0(&db, sync.models["main"].source, sync.project);
    assert!(
        main_s0.variables.values().any(
            |v| matches!(v, crate::variable::Variable::Module { model_name, .. }
                if model_name.as_str() == "stdlib\u{205A}smth1")
        ),
        "SMTH1 must have expanded into an implicit stdlib module instance: {:?}",
        main_s0.variables.keys().collect::<Vec<_>>()
    );
    assert!(
        main_s0.variables.values().any(
            |v| matches!(v, crate::variable::Variable::Module { model_name, .. }
                if model_name.as_str() == "scaled")
        ),
        "the macro call must have expanded into a synthetic module targeting the macro model"
    );
    assert!(
        main_s0
            .variables
            .contains_key(&Ident::<Canonical>::new("sub")),
        "the explicit user sub-model instance must be staged"
    );
    assert!(
        main_s0
            .errors
            .as_ref()
            .is_some_and(|errs| errs.iter().any(|e| e.code == ErrorCode::DuplicateVariable)),
        "the duplicate canonical idents must record a model-level error"
    );
    let macro_s0 = model_stage0(&db, sync.models["scaled"].source, sync.project);
    assert!(
        macro_s0.is_macro,
        "the macro-marked model must stage as one"
    );
    assert_eq!(
        macro_s0.macro_params,
        vec![Ident::<Canonical>::new("p1"), Ident::<Canonical>::new("p2")],
        "the macro's formal parameters must reach Stage0"
    );
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
    let oracle = ModelStage0::new(
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

/// `Project::from_salsa` READS these queries; it does not build stages of its
/// own.
///
/// The evidence is execution counts, not values. A value oracle cannot see the
/// difference at all: the copy this commit deleted from `from_salsa` produced
/// stages that were *equal* to the cached ones (that was the point -- B2 adopted
/// `from_salsa`'s semantics field by field so this step would be a deletion
/// rather than a behaviour change), so an equality assertion passes just as
/// happily against a second inline build. The counters distinguish them: with
/// the inline build `from_salsa` entered neither query body and both counts read
/// ZERO here.
///
/// The second half is the "one construction site" claim made observable. Running
/// the whole-project unit pass afterwards on the SAME database rebuilds nothing,
/// because `from_salsa` and `check_model_units` now share one set of memos. With
/// two construction sites the same sequence paid for both.
#[test]
fn project_from_salsa_reads_the_cached_stages() {
    let db = SimlinDb::default();
    let project = three_model_project();
    let sync = sync_from_datamodel(&db, &project);
    let n_models = sync.project.models(&db).len();
    let expected = StageExecutions {
        stage0: n_models,
        stage1: n_models,
    };

    reset_stage_executions();
    let built =
        crate::project::Project::from_salsa(project.clone(), &db, sync.project, |_, _, _| {});
    assert_eq!(
        stage_executions(),
        expected,
        "from_salsa must build each model's stages through the cached queries, once each"
    );
    assert_eq!(
        built.models.len(),
        n_models,
        "every project model must reach the built Project"
    );

    let diagnostics = collect_all_diagnostics(&db, sync.project);
    assert_eq!(
        stage_executions(),
        expected,
        "the unit pass must reuse the stages from_salsa already demanded, not rebuild them \
         (diagnostics: {diagnostics:?})"
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
