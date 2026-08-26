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
//!      two stages AT MOST ONCE per revision, not once per (model, model) pair.
//!   3. **Diagnostics do not move**: every diagnostic that reached a harvest
//!      point before the stage construction moved out of `check_model_units`
//!      still reaches it.
//!
//! The oracle is a datamodel-driven constructor that touches no database: an
//! oracle that read the queries under test would compare a memo against a
//! clone of itself and pass unconditionally.
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
//! The rule: compare with `PartialEq`, always. The one thing that used to force
//! a Debug-text oracle is gone -- a stage carrying a NaN EQUATION LITERAL was
//! not equal even to ITSELF, because `ModelStage0` compares parsed float
//! constants and every SMOOTH/DELAY/TREND template declares
//! `initial_value = NAN`. Since GH #987/#981 those constants are
//! `ast::Literal`, compared by bit pattern, so every stage here -- stdlib
//! templates included -- equals a bit-identical rebuild. One float-bearing
//! field on these memos is still outside that: `variable::Table`'s `Vec<f64>`
//! lookup points (see `ast::Literal`'s scope note). No fixture in this file has
//! a graphical function, let alone a NaN in one, so the rule holds here.

use super::*;
use crate::common::{Canonical, Ident};
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

/// Every model shape the stage queries have to handle, in one project:
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
///   - two variables whose names canonicalize identically, which the
///     canonical-keyed `variables` map collapses last-wins on both sides.
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
/// This is the whole-project Stage0 map, built through the salsa-free
/// constructor so it is an oracle independent of the cached queries. The
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

/// Lower [`datamodel_driven_stage0s`] under one whole-project `ScopeStage0` per
/// model, keyed by canonical model name.
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
/// lowering of the same models, field by field; `variables` is compared last so
/// a mismatch names the fixture and model.
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
            assert!(
                cached.variables == oracle.variables,
                "cached model_stage1 lowering for `{name}` in fixture `{fixture}` must equal \
                 the datamodel-driven one"
            );
        }
    }
}

/// Omitting the stdlib models from `model_stage1`'s LOWERING scope changes no
/// lowering TODAY -- and this is the property that makes that true, asserted
/// directly so it becomes a tripwire rather than an assumption.
///
/// Written for whoever did the scope narrowing (the follow-on to GH #966). That
/// narrowing has landed and did NOT take the wholesale route: `model_scope_models`
/// keeps the stdlib models a lowering can actually reach, exactly as it keeps
/// any other module target, so this test now guards a road not taken.
///
/// The reason "whole project MINUS every `stdlib⁚` model" is inert for LOWERING
/// is not missing coverage. The lowering scope has exactly two consumers, and
/// neither can observe a stdlib model:
///
///   - `ArrayContext::get_variable` follows a `module·output` reference into the
///     target model's Stage0 to read its DIMENSIONS. Every shipped stdlib
///     variable is scalar, so the answer is `None` whether the model is in the
///     map or not.
///   - `resolve_relative` recurses through intermediate models named by a
///     dotted reference. No stdlib model instantiates a module, so none can ever
///     be an intermediate hop.
///
/// Two routes DO reach a stdlib entry, and both are deliberately out of scope
/// for this assertion:
///
///   - `resolve_relative`'s TERMINAL lookup. A module-input `src` spelled
///     `stdlib⁚<template>·<var>` resolves against the stdlib model itself, so
///     dropping those entries turns that input into a `BadModuleInputSrc`. That
///     is a LOUD error rather than a silent mis-lowering -- the opposite of the
///     failure class this tripwire guards -- and it needs an imported model whose
///     input `src` literally names a stdlib template (ordinary model creation
///     never produces the `⁚` separator).
///   - The INFERENCE map, which is a different consumer of the same scope:
///     `check_model_units`' stdlib-argument-group check looks a module's target
///     up by name, and `units_infer` recurses into it. Since B3 gave both halves
///     one scope, a `stdlib⁚*`-skipping closure is no longer even inert
///     end-to-end -- it reds `unit_checking_test::test_smth1_unit_mismatch_initial`,
///     measured. Keeping the closure's stdlib targets avoids both routes.
///
/// If a future stdlib template gains an arrayed variable or a module instance,
/// this test fails -- and at that moment a closure that skips `stdlib⁚*` becomes
/// a silent mis-lowering, exactly like the one
/// [`arrayed_module_project`] and [`chain_project`] catch for user models. The
/// assertion runs over the SYNCED Stage0s, which is what the scope map actually
/// holds, rather than over the generated stdlib source.
///
/// # The module half is ALSO the tripwire for module-graph cycle safety
///
/// The `Variable::Module` assertion below now carries a second, unrelated claim.
/// `MacroRegistry::build`'s Pass 4 (rejecting a module inside a macro,
/// `ErrorCode::MacroContainsModule`) closes the hole where a cycle ran through an
/// IMPLICIT module edge that `db::project_module_graph` cannot see. Its closure
/// argument needs "a stdlib model is a SINK -- it instantiates no module", so that
/// an implicit stdlib edge cannot be one hop of a longer invisible cycle. That
/// property is TEST-asserted here, not structural: nothing stops a future
/// `stdlib/*.stmx` template from instantiating another template.
///
/// So if this assertion ever fails, TWO things break at once, and the second is
/// the abort-class one: the shape a stdlib template just gained is a module edge
/// invisible to the gate, and `project_module_graph`'s "every remaining cycle is
/// explicit" invariant no longer holds. Fixing it means teaching the gate about
/// stdlib edges (they are static and known at build time, so this is cheap --
/// unlike parse-derived macro edges), not just widening the lowering scope.
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
                !var.is_module(),
                "stdlib model {} instantiates a module ({ident}). Two consequences: it can now \
                 be an intermediate hop in `resolve_relative`, so a lowering scope that omits \
                 stdlib models can silently break a chained reference; and -- the abort-class \
                 one -- `db::project_module_graph` does not record implicit module edges, so \
                 the stdlib-sink premise of `MacroRegistry::build`'s Pass 4 closure argument \
                 no longer holds and a cycle through this edge is invisible to the cycle gate",
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

/// Both cached stages equal the datamodel-driven build for EVERY model shape in
/// [`every_shape_project`].
///
/// The two oracle tests above cover a plain multi-model project; this one is the
/// combined fixture. Every `ModelStage0` field is compared (the `==` is the
/// derived `PartialEq` over the whole struct): `ident`, `display_name`,
/// `variables` (including the implicit SMOOTH expansion), `implicit`,
/// `is_macro` and `macro_params`.
///
/// The comparison ranges over EVERY model the sync produced -- the three user
/// models and all nine spliced stdlib templates. Five of the nine templates
/// declare `initial_value = NAN`; `ast::Literal` compares float literals by bit
/// pattern (GH #987/#981), which is what lets a NaN-bearing stage equal its own
/// rebuild and the oracle cover them.
///
/// The row-count assertion is not about a newly added stdlib template -- both
/// sides derive their stdlib half from `stdlib::MODEL_NAMES`, so one of those
/// is covered automatically. What it catches is `every_shape_project()` gaining
/// or losing a USER model that the hard-coded list below does not follow.
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

    let mut names: Vec<String> = vec!["main".to_string(), "sub".to_string(), "scaled".to_string()];
    names.extend(
        crate::stdlib::MODEL_NAMES
            .iter()
            .map(|n| format!("stdlib\u{205A}{n}")),
    );
    assert_eq!(
        names.len(),
        oracle_s0.len(),
        "every model the oracle builds must be asserted on"
    );
    for name in &names {
        let name = name.as_str();
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

    // The fixture really does exercise its module shapes -- otherwise the
    // equalities above would be comparing an ordinary project twice.
    let main_s0 = model_stage0(&db, sync.models["main"].source, sync.project);
    assert!(
        main_s0.variables.values().any(
            |v| matches!(&v.kind, crate::variable::VarKind::Module { model_name, .. }
                if model_name.as_str() == "stdlib\u{205A}smth1")
        ),
        "SMTH1 must have expanded into an implicit stdlib module instance: {:?}",
        main_s0.variables.keys().collect::<Vec<_>>()
    );
    assert!(
        main_s0.variables.values().any(
            |v| matches!(&v.kind, crate::variable::VarKind::Module { model_name, .. }
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
/// The rule is inert for unit checking (nothing on the units path reads
/// `implicit`), so this pins the decision at the only place it is observable.
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
/// the module-ident set), but load-bearing for cache sharing: one rule means
/// one `ModuleIdentContext` and one set of per-variable parse memos.
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
/// different key -- losing the cache sharing that is the entire stated reason
/// for the rule. `Stage0Context` is the one place that
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
/// The fixture is `smth1`, the representative SMOOTH template. It used to have
/// to be `npv` -- the one template with no NaN literal -- because
/// `ModelStage0`'s derived `PartialEq` compared bare parsed `f64` constants and
/// every SMOOTH/DELAY/TREND template declares `initial_value = NAN`, so those
/// stages were unequal even to a bit-identical rebuild. `ast::Literal` compares
/// float literals by bit pattern (GH #987/#981), so a stage whose NaN is an
/// EQUATION LITERAL is equal to itself and the natural fixture works. (A NaN
/// arriving through a graphical function's points is a different field and is
/// still non-reflexive -- see `ast::Literal`'s scope note; no stdlib template
/// has a graphical function.)
#[test]
fn cached_stdlib_stage0_equals_implicit_datamodel_build() {
    let db = SimlinDb::default();
    // Every stdlib model is spliced into every synced project, referenced or
    // not, so a bare `main` is enough to reach one.
    let main = x_model("main", vec![x_aux("input", "1", None)]);
    let project = x_project(sim_specs_with_units("month"), &[main]);
    let sync = sync_from_datamodel(&db, &project);

    let source = sync.models["stdlib\u{205A}smth1"].source;
    let cached = model_stage0(&db, source, sync.project);
    assert!(cached.implicit, "a stdlib model's Stage0 is implicit");
    assert!(
        cached.variables.len() > 1,
        "the fixture stdlib model has a body to compare"
    );
    assert!(
        cached.variables.values().any(|v| {
            v.ast().is_some_and(|ast| match ast {
                crate::ast::Ast::Scalar(e) => {
                    matches!(e, crate::ast::Expr0::Const(_, n, _) if n.value().is_nan())
                }
                _ => false,
            })
        }),
        "the fixture must actually carry the NaN literal this test is here to cover"
    );

    let stdlib_dm = crate::stdlib::get("smth1").expect("smth1 is a stdlib model");
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

// ── 3. the GH #966 cost claim ───────────────────────────────────────────

/// Collecting whole-project diagnostics BUILDS each model's Stage0 and Stage1
/// at most once, and only for the models something actually reaches.
///
/// What the execution counts prove: the two query BODIES ran once per USER model
/// across a whole-project diagnostic collection that runs the unit pass on all
/// of them. Before GH #966 the same collection constructed `n_models` of each
/// PER checked model; after it, one per project model; and now one per model
/// that is in some checked model's module-reachable scope. The stdlib templates
/// are spliced into every project but this fixture instantiates none of them, so
/// they are never staged at all -- the difference between `n_user` and
/// `n_models` is the narrowing, measured.
///
/// The second half re-reads every model's stages once per model -- the exact
/// pre-#966 access pattern -- and shows that once the stdlib stages have been
/// demanded once, nothing rebuilds: the linearity is a property of the cache and
/// not of the order `collect_all_diagnostics` happens to walk in.
///
/// What they do NOT prove: anything about wall-clock time, and anything about a
/// LATER revision (the incrementality claims are
/// [`an_unrelated_models_edit_invalidates_neither_stage_nor_unit_check`] and
/// [`a_module_targets_edit_invalidates_its_instantiators_stage_and_unit_check`]).
///
/// What makes the measurement sound: `reset_query_executions()` immediately
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
    let n_user = project.models.len();
    // Three user models plus the spliced stdlib set: the pre-#966 cost was
    // 3 x n_models of each stage, so n_user and 3 x n_models are far apart.
    assert!(
        n_models > n_user,
        "fixture should have stdlib models beyond its {n_user} user models: {n_models}"
    );

    reset_query_executions();
    let diagnostics = collect_all_diagnostics(&db, sync.project);
    let after_collect = query_executions();
    assert_eq!(
        after_collect,
        QueryExecutions {
            stage0: n_user,
            stage1: n_user,
            // The unit pass is entered for every model, stdlib included; the
            // stdlib ones return at the skip gate before reading a stage.
            unit_check: n_models,
        },
        "only the reachable models' stages may be built, once each, got {after_collect:?} \
         for {n_user} user models of {n_models} (diagnostics: {diagnostics:?})"
    );

    // The pre-#966 access pattern, made explicit: every model's stages, once
    // per model. n_models^2 reads, and -- after the first pass has demanded the
    // stdlib stages nothing had reached -- still n_models builds.
    let sources: Vec<SourceModel> = sync.project.models(&db).values().copied().collect();
    for (round, _target) in sources.iter().enumerate() {
        for m in &sources {
            let _ = model_stage1(&db, *m, sync.project);
            let _ = model_stage0(&db, *m, sync.project);
        }
        if round == 0 {
            assert_eq!(
                query_executions(),
                QueryExecutions {
                    stage0: n_models,
                    stage1: n_models,
                    unit_check: n_models,
                },
                "demanding every model's stages must build only the ones the diagnostic \
                 collection did not reach"
            );
        }
    }
    assert_eq!(
        query_executions(),
        QueryExecutions {
            stage0: n_models,
            stage1: n_models,
            unit_check: n_models,
        },
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

// ── 5. the scope narrowing ──────────────────────────────────────────────

/// The canonical model names in `model`'s lowering/inference scope.
fn scope_names(db: &SimlinDb, sync: &SyncResult, model: &str) -> Vec<String> {
    model_scope_models(db, sync.models[model].source, sync.project)
        .keys()
        .map(|ident| ident.as_str().to_string())
        .collect()
}

/// The scope is the TRANSITIVE module-reachable closure, and nothing else.
///
/// [`chain_project`] is `main -> sub_a -> sub_c`, so each model's scope shrinks
/// as you descend, and `sub_c` -- reachable from `main` only THROUGH `sub_a` --
/// proves the closure is transitive rather than one hop. The stdlib templates
/// are spliced into every synced project and this fixture instantiates none of
/// them, so their absence is what says the scope is a closure and not a filter
/// on the project's model list.
#[test]
fn the_lowering_scope_is_the_transitive_module_reachable_closure() {
    let db = SimlinDb::default();
    let project = chain_project();
    let sync = sync_from_datamodel(&db, &project);

    assert_eq!(scope_names(&db, &sync, "main"), ["main", "sub_a", "sub_c"]);
    assert_eq!(scope_names(&db, &sync, "sub_a"), ["sub_a", "sub_c"]);
    assert_eq!(scope_names(&db, &sync, "sub_c"), ["sub_c"]);
    assert!(
        sync.project.models(&db).len() > 3,
        "the fixture must carry models outside every scope (the spliced stdlib set)"
    );
}

/// An IMPLICIT module -- one that builtin/macro expansion synthesized rather
/// than the modeller declaring -- is a scope edge like any other.
///
/// This is the constraint that rules out deriving the scope from
/// `db::project_module_graph`, which records only EXPLICIT `Variable::Module`
/// edges. A `SMTH1` call reaches a stdlib template, which is harmless today; a
/// MACRO call reaches an ordinary user model, which is not. Both are asserted
/// here, together with the sibling stdlib templates the model does NOT
/// instantiate, so "the scope holds every stdlib model" cannot pass this test.
#[test]
fn implicit_and_macro_modules_are_scope_edges() {
    let db = SimlinDb::default();
    let project = every_shape_project();
    let sync = sync_from_datamodel(&db, &project);

    assert_eq!(
        scope_names(&db, &sync, "main"),
        ["main", "scaled", "stdlib\u{205A}smth1", "sub"],
        "the SMTH1 expansion's stdlib target and the macro call's model must both be edges"
    );
    // The macro model itself calls nothing, so its scope is just itself -- the
    // macro-call edge above is real reachability, not a project-wide splice.
    assert_eq!(scope_names(&db, &sync, "scaled"), ["scaled"]);
}

/// A project whose `main` reduces over the output of a MACRO call, where the
/// macro's primary output is arrayed (`arrayed`) or scalar (the control).
///
/// A macro call expands into a synthetic module targeting the macro's own model
/// plus a reference to `<synthetic module>·<primary_output>`
/// (`builtins_visitor::expand_module_function`), so `SUM(scaled(driver))` is the
/// macro-side twin of [`arrayed_module_project`]'s
/// `SUM(sub_a.out_by_region[*])`: resolving its dimensions means following an
/// IMPLICIT module edge into another model's Stage0.
fn macro_output_project(arrayed: bool) -> datamodel::Project {
    let out = if arrayed {
        x_apply_to_all("out", "Region", "p1 * 2")
    } else {
        x_aux("out", "p1 * 2", None)
    };
    let scaled_macro = datamodel::Model {
        name: "scaled".to_string(),
        sim_specs: None,
        variables: vec![out, x_aux("p1", "0", None)],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(datamodel::MacroSpec {
            parameters: vec!["p1".to_string()],
            primary_output: "out".to_string(),
            additional_outputs: vec![],
        }),
    };
    let main = x_model(
        "main",
        vec![
            x_aux("driver", "5", None),
            x_aux("total", "SUM(scaled(driver))", None),
        ],
    );
    let mut project = x_project(sim_specs_with_units("month"), &[main, scaled_macro]);
    project.dimensions = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec!["north".to_string(), "south".to_string()],
    )];
    project
}

/// Lower `main` against its own scope with the named models REMOVED, as a
/// closure that dropped those edges would.
fn lower_main_without(
    db: &SimlinDb,
    sync: &SyncResult,
    omit: &[&str],
) -> crate::model::ModelStage1 {
    let source = sync.models["main"].source;
    let main_s0 = model_stage0(db, source, sync.project);
    let models: HashMap<Ident<Canonical>, &ModelStage0> =
        model_scope_models(db, source, sync.project)
            .values()
            .map(|m| {
                let s0 = model_stage0(db, *m, sync.project);
                (s0.ident.clone(), s0)
            })
            .filter(|(ident, _)| !omit.contains(&ident.as_str()))
            .collect();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: project_dimensions_context(db, sync.project),
        model_name: main_s0.ident.as_str(),
    };
    ModelStage1::new(&scope, main_s0)
}

/// Dropping the macro model's scope edge changes the lowered VALUE, and the
/// mechanism is dimension resolution.
///
/// [`implicit_and_macro_modules_are_scope_edges`] pins the macro edge
/// STRUCTURALLY -- it asserts a name list -- which cannot show that losing the
/// edge would mis-lower anything. This is the macro-side equivalent of what
/// [`arrayed_module_project`] and [`chain_project`] do for user models: it
/// demonstrates the CONSEQUENCE.
///
/// Two halves, and the second is what makes the first mean something:
///
///   - ARRAYED macro output: `main`'s `total` lowers differently with and
///     without `scaled` in scope, and it is the ONLY variable that differs.
///     Without the macro model, `ArrayContext::get_variable` cannot follow the
///     synthetic module into `scaled`, `get_dimensions` returns `None`, and
///     `SUM(...)` reduces over what it now believes is a scalar -- no error, a
///     plausible wrong answer.
///   - SCALAR macro output (the control): removing `scaled` changes NOTHING,
///     because `get_dimensions` answers `None` either way. So the difference
///     above is dimension resolution through the macro edge specifically, not
///     some incidental sensitivity to the map's contents.
///
/// What a mis-lowering here actually corrupts is worth stating, because it is
/// narrower than "the model simulates wrong": `model_stage1`'s consumer is
/// unit checking. Simulation compiles from the per-variable fragment path (its
/// own mini Stage0s), so it does not read this stage at all.
#[test]
fn dropping_a_macro_models_scope_edge_changes_the_lowered_value() {
    let total: Ident<Canonical> = Ident::new("total");

    let db = SimlinDb::default();
    let project = macro_output_project(true);
    let sync = sync_from_datamodel(&db, &project);
    let cached = model_stage1(&db, sync.models["main"].source, sync.project);

    // The fixture must actually route through a macro-expansion module. This is
    // read off the model's STAGE0, deliberately, not off its scope: a scope-name
    // assertion would fire first under a narrowing that drops macro edges, and
    // then this test would be red for the same structural reason
    // `implicit_and_macro_modules_are_scope_edges` already covers, instead of on
    // the lowered value it exists to protect.
    assert!(
        model_stage0(&db, sync.models["main"].source, sync.project)
            .variables
            .values()
            .any(
                |v| matches!(&v.kind, crate::variable::VarKind::Module { model_name, .. }
                if model_name.as_str() == "scaled")
            ),
        "the macro call must have expanded into a synthetic module targeting `scaled`"
    );

    let without_macro = lower_main_without(&db, &sync, &["scaled"]);
    assert!(
        cached.variables[&total] != without_macro.variables[&total],
        "the arrayed macro output's dimensions must resolve through the macro \
         scope edge, so dropping it must change how `total` lowers"
    );
    let differing: Vec<&str> = cached
        .variables
        .iter()
        .filter(|(ident, var)| without_macro.variables.get(*ident) != Some(*var))
        .map(|(ident, _)| ident.as_str())
        .collect();
    assert_eq!(
        differing,
        ["total"],
        "only the reducing caller should be affected"
    );

    // Control: with a SCALAR macro output the same removal changes nothing.
    let db = SimlinDb::default();
    let scalar_project = macro_output_project(false);
    let sync = sync_from_datamodel(&db, &scalar_project);
    let cached = model_stage1(&db, sync.models["main"].source, sync.project);
    let without_macro = lower_main_without(&db, &sync, &["scaled"]);
    assert!(
        cached.variables == without_macro.variables,
        "a scalar macro output resolves to no dimensions either way, so the \
         difference above is dimension resolution and not the map's contents"
    );
}

/// `resolve_relative` keys a dotted module-input `src` on each component as a
/// MODEL name, so a module's own IDENT is a scope edge too when it names one.
///
/// This is the narrowing's one non-obvious edge, and the fixture is the shape
/// that needs it: `main` instantiates model `real_target` under the ident
/// `helper`, while a model named `helper` also exists, and a second module's
/// input reads `helper·something`. `resolve_relative` predates a module ident
/// being allowed to differ from its target's name, so it resolves that `src`
/// against MODEL `helper` -- which is reachable from `main` only through the
/// ident. Dropping the ident edge turns a project that lowered cleanly into one
/// reporting `BadModuleInputSrc`.
///
/// That resolution is itself questionable (the reference means "the module
/// `helper`'s output", not "model `helper`'s variable"), and its TODO has been
/// in `model.rs` for years. This test pins BEHAVIOUR PRESERVATION across the
/// narrowing, not the rule: whoever fixes the underlying resolution should
/// delete this along with the ident edge.
#[test]
fn a_module_ident_that_names_a_model_is_a_scope_edge() {
    use crate::testutils::x_module_named;

    let db = SimlinDb::default();
    let project = x_project(
        sim_specs_with_units("month"),
        &[
            x_model(
                "main",
                vec![
                    x_aux("driver", "5", None),
                    x_module_named("helper", "real_target", &[("driver", "helper.input")], None),
                    x_module_named("m2", "consumer", &[("helper.something", "m2.input")], None),
                ],
            ),
            x_model("real_target", vec![x_aux("input", "0", None)]),
            x_model("helper", vec![x_aux("something", "1", None)]),
            x_model("consumer", vec![x_aux("input", "0", None)]),
        ],
    );
    let sync = sync_from_datamodel(&db, &project);

    assert!(
        scope_names(&db, &sync, "main").contains(&"helper".to_string()),
        "the module ident `helper` names a model that `resolve_relative` reaches: {:?}",
        scope_names(&db, &sync, "main")
    );

    let m2 = model_stage1(&db, sync.models["main"].source, sync.project)
        .variables
        .get(&Ident::<Canonical>::new("m2"))
        .expect("the second module must be staged")
        .equation_errors();
    assert!(
        m2.is_none(),
        "the module input must still resolve, as it does under a whole-project scope: {m2:?}"
    );
}

/// A module CYCLE yields a finite scope containing the whole cycle.
///
/// The walk is iterative over a visited set precisely because this project is
/// one a user can draw: a recursive tracked query on this graph is salsa's
/// unrecoverable dependency-graph panic, not a diagnostic (GH #806), and a
/// recursive plain function is a stack overflow, which under `panic=abort` takes
/// the host process with it. Both members' stages must also LOWER, since
/// `check_model_units` reaches them on a cyclic project (the engine primitive is
/// reachable even though `collect_all_diagnostics` gates on the cycle first).
#[test]
fn a_module_cycle_yields_a_finite_scope() {
    use crate::testutils::x_module_named;

    let db = SimlinDb::default();
    let project = x_project(
        sim_specs_with_units("month"),
        &[
            x_model(
                "a",
                vec![
                    x_aux("x", "1", None),
                    x_module_named("to_b", "b", &[("x", "to_b.input")], None),
                ],
            ),
            x_model(
                "b",
                vec![
                    x_aux("input", "0", None),
                    x_module_named("to_a", "a", &[("input", "to_a.x")], None),
                ],
            ),
        ],
    );
    let sync = sync_from_datamodel(&db, &project);

    assert_eq!(scope_names(&db, &sync, "a"), ["a", "b"]);
    assert_eq!(scope_names(&db, &sync, "b"), ["a", "b"]);
    for name in ["a", "b"] {
        assert!(
            !model_stage1(&db, sync.models[name].source, sync.project)
                .variables
                .is_empty(),
            "a cyclic project's models must still lower"
        );
    }
}

/// A cross-module unit mismatch that only closes through TWO module hops.
///
/// `main.x` is declared `widget` and feeds `sub_a.input`, which is undeclared
/// and feeds `sub_c.input`, which is declared `gadget`. The contradiction exists
/// only once `units_infer` has walked BOTH hops: hop one binds
/// `@main·x = @sub_a·input`, hop two binds `@sub_a·input = @sub_c·input`, and
/// only `sub_c`'s declaration closes it against `widget`. `sub_a` declares
/// nothing, so no one-hop conflict can stand in for it.
fn two_hop_unit_mismatch_project() -> datamodel::Project {
    let sub_c = x_model("sub_c", vec![x_aux("input", "0", Some("gadget"))]);
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
            x_aux("x", "1", Some("widget")),
            x_module("sub_a", &[("x", "sub_a.input")], None),
        ],
    );
    x_project(sim_specs_with_units("month"), &[main, sub_a, sub_c])
}

/// The unit guard for the TRANSITIVE row of the narrowing matrix.
///
/// The stdlib row has `unit_checking_test::test_smth1_unit_mismatch_initial` as
/// its guard: narrow the closure past stdlib targets and a unit diagnostic
/// disappears. The transitive row had no twin -- the direct-targets-only probe
/// red-lit only LOWERING tests, so a closure that stopped at direct targets
/// would have silently dropped every grandchild's unit constraints with nothing
/// to say so. `units_infer` recurses through module instantiations, so a model
/// missing from the inference map is declined exactly like a dangling
/// `model_name`: partial results, no error, one fewer diagnostic.
///
/// This is a diagnostic-DISAPPEARS test, so it asserts the warning is present.
/// Under a direct-targets-only closure `sub_c` is absent from `main`'s inference
/// map and the mismatch goes unreported.
#[test]
fn a_two_hop_cross_module_unit_mismatch_is_still_reported() {
    use crate::common::ErrorCode;

    let db = SimlinDb::default();
    let project = two_hop_unit_mismatch_project();
    let sync = sync_from_datamodel(&db, &project);

    // The fixture depends on `sub_c` being reachable only THROUGH `sub_a`, and
    // that precondition is read off the models' STAGE0s rather than off `main`'s
    // scope. A scope-name assertion here would fire first under the very
    // narrowing this test exists to catch, and the test would then be red for a
    // structural reason `the_lowering_scope_is_the_transitive_module_reachable_closure`
    // already covers instead of on the diagnostic it is protecting.
    let targets = |model: &str| -> Vec<String> {
        model_stage0(&db, sync.models[model].source, sync.project)
            .variables
            .values()
            .filter_map(|v| match &v.kind {
                crate::variable::VarKind::Module { model_name, .. } => {
                    Some(model_name.as_str().to_string())
                }
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        targets("main"),
        ["sub_a"],
        "main must not reach sub_c directly"
    );
    assert_eq!(
        targets("sub_a"),
        ["sub_c"],
        "sub_a must be the only route to sub_c"
    );

    let diagnostics = unit_pass_diagnostics(&db, sync.models["main"].source, sync.project);
    assert!(
        diagnostics.iter().any(|cd| matches!(
            &cd.0.error,
            DiagnosticError::Model(e) if e.code == ErrorCode::UnitMismatch
        )),
        "the two-hop widget/gadget contradiction must still be reported: {:?}",
        diagnostics.iter().map(|cd| &cd.0).collect::<Vec<_>>()
    );
}

/// Editing a model with NO module relationship to `main` re-executes neither
/// `main`'s lowered stage nor its unit check.
///
/// This is the point of the narrowing, and it needs execution counts rather than
/// memo pointer identity: salsa backdates a re-executed query whose value
/// compares equal and can keep the memo's address, so a pointer-equal memo does
/// not show that the body did not run. With the whole-project scope map both of
/// `main`'s queries re-executed on this edit; `other`'s still must, or the test
/// would pass against a database that simply answered nothing.
///
/// The sync trap this is written around: incremental sync REUSES the
/// `SourceProject` handle, so `sync1.project` and `sync2.project` are the same
/// salsa input and a value read through the older `SyncResult` after the edit is
/// the POST-edit value. Every read here is therefore made against the sync that
/// is current at that moment.
#[test]
fn an_unrelated_models_edit_invalidates_neither_stage_nor_unit_check() {
    let unrelated_pair = |other_eqn: &str| {
        x_project(
            sim_specs_with_units("month"),
            &[
                x_model(
                    "main",
                    vec![
                        x_aux("driver", "5", Some("widget")),
                        x_aux("scaled", "driver * 2", Some("widget")),
                    ],
                ),
                x_model("other", vec![x_aux("y", other_eqn, Some("gadget"))]),
            ],
        )
    };

    let mut db = SimlinDb::default();
    let state1 = sync_from_datamodel_incremental(&mut db, &unrelated_pair("1"), None);
    let sync1 = state1.to_sync_result();
    for name in ["main", "other"] {
        let source = sync1.models[name].source;
        let _ = model_stage1(&db, source, sync1.project);
        crate::db::units::check_model_units(&db, source, sync1.project);
    }

    // Control: re-syncing the identical project re-executes nothing, so a count
    // below is attributable to the edit rather than to the re-sync.
    reset_query_executions();
    let state2 = sync_from_datamodel_incremental(&mut db, &unrelated_pair("1"), Some(&state1));
    let sync2 = state2.to_sync_result();
    for name in ["main", "other"] {
        let source = sync2.models[name].source;
        let _ = model_stage1(&db, source, sync2.project);
        crate::db::units::check_model_units(&db, source, sync2.project);
    }
    assert_eq!(
        query_executions(),
        QueryExecutions::default(),
        "re-syncing an unchanged project must re-execute nothing"
    );

    reset_query_executions();
    let state3 = sync_from_datamodel_incremental(&mut db, &unrelated_pair("2"), Some(&state2));
    let sync3 = state3.to_sync_result();
    let main = sync3.models["main"].source;
    let _ = model_stage1(&db, main, sync3.project);
    crate::db::units::check_model_units(&db, main, sync3.project);
    assert_eq!(
        query_executions(),
        QueryExecutions::default(),
        "an unrelated model's edit must not invalidate main's stage or unit check"
    );

    // The edit really did land: the edited model's own queries re-execute.
    let other = sync3.models["other"].source;
    let _ = model_stage1(&db, other, sync3.project);
    crate::db::units::check_model_units(&db, other, sync3.project);
    assert_eq!(
        query_executions(),
        QueryExecutions {
            stage0: 1,
            stage1: 1,
            unit_check: 1,
        },
        "the EDITED model's stage and unit check must re-execute, or the fixture proves nothing"
    );
}

/// A model carrying a USER-AUTHORED NaN literal backdates its stage exactly
/// like one without: a revision bump that re-executes `model_stage0` and
/// rebuilds a bit-identical value must NOT re-execute `model_stage1`.
///
/// This is the reachable half of GH #987's EQUATION-LITERAL path. `ModelStage0`
/// derives `PartialEq` and holds parsed float constants; with a bare `f64` a
/// NaN-bearing stage was never equal to its own rebuild, so salsa could never
/// backdate it and every downstream query in the cone re-ran on every revision
/// bump -- on the interactive diagnostics path, per keystroke. The issue's own
/// reachability argument rests on the stdlib `initial_value = NAN`, but that
/// half is inert (those inputs never change after sync), so the fixture here is
/// a user equation, which is the shape that actually pays.
///
/// A NaN reaching the same memo through a GRAPHICAL FUNCTION's points is a
/// different, still-unfixed field (`variable::Table`'s `Vec<f64>`); this lever
/// measures that one identically, which is how it was confirmed. See
/// `ast::Literal`'s scope note.
///
/// The lever is the project's UNIT table: `model_stage0` reads
/// `project_units_context` and `model_stage1` does not, so adding an unrelated
/// unit re-executes stage0 and leaves stage1's re-execution decided purely by
/// whether the rebuilt stage compares equal to the old one.
///
/// Two rows, derived from the one axis the change is about -- whether the
/// model's equations carry a NaN literal -- and they must produce identical
/// counts. The control row is what makes the NaN row attributable: under the
/// mutation probe (bare `f64` equality inside `ast::Literal`) the control stays
/// green and the NaN row reds.
#[test]
fn a_nan_bearing_models_stage_backdates_like_any_other() {
    // The control row runs FIRST so that a mutation probe fails on the NaN row
    // with the control already green -- attribution, not just a red test.
    for (label, eqn) in [("control", "1 + 2"), ("nan literal", "1 + nan")] {
        let build = |extra_unit: bool| {
            let mut project = x_project(
                sim_specs_with_units("month"),
                &[x_model("main", vec![x_aux("x", eqn, Some("widget"))])],
            );
            if extra_unit {
                project.units.push(datamodel::Unit {
                    name: "gizmo".to_owned(),
                    equation: None,
                    disabled: false,
                    aliases: vec![],
                });
            }
            project
        };

        let mut db = SimlinDb::default();
        let state1 = sync_from_datamodel_incremental(&mut db, &build(false), None);
        let sync1 = state1.to_sync_result();
        let main = sync1.models["main"].source;
        let _ = model_stage1(&db, main, sync1.project);

        // Guard against a vacuous pass: the NaN row's stage really does hold a
        // NaN literal, so the equality being measured is the one at issue.
        let holds_nan = model_stage0(&db, main, sync1.project)
            .variables
            .values()
            .any(|v| {
                v.ast().is_some_and(|ast| match ast {
                    crate::ast::Ast::Scalar(e) => expr0_holds_nan(e),
                    _ => false,
                })
            });
        assert_eq!(
            holds_nan,
            label == "nan literal",
            "{label}: the fixture must carry a NaN literal iff it is the NaN row"
        );

        reset_query_executions();
        let state2 = sync_from_datamodel_incremental(&mut db, &build(true), Some(&state1));
        let sync2 = state2.to_sync_result();
        let _ = model_stage1(&db, sync2.models["main"].source, sync2.project);
        assert_eq!(
            query_executions(),
            QueryExecutions {
                stage0: 1,
                stage1: 0,
                unit_check: 0,
            },
            "{label}: the units edit must re-execute stage0 and, because the rebuilt \
             stage is bit-identical, backdate it so stage1 is reused"
        );
    }
}

/// Does this `Expr0` tree contain a NaN literal? Used only by the fixture guard
/// above.
fn expr0_holds_nan(expr: &crate::ast::Expr0) -> bool {
    use crate::ast::Expr0;
    match expr {
        Expr0::Const(_, n, _) => n.value().is_nan(),
        Expr0::Var(_, _) => false,
        Expr0::App(crate::builtins::UntypedBuiltinFn(_, args), _) => {
            args.iter().any(expr0_holds_nan)
        }
        Expr0::Subscript(_, _, _) => false,
        Expr0::Op1(_, inner, _) => expr0_holds_nan(inner),
        Expr0::Op2(_, l, r, _) => expr0_holds_nan(l) || expr0_holds_nan(r),
        Expr0::If(c, t, f, _) => expr0_holds_nan(c) || expr0_holds_nan(t) || expr0_holds_nan(f),
    }
}

/// Editing a model that `main` instantiates DOES re-execute `main`'s lowered
/// stage and unit check.
///
/// This is the direction a too-narrow scope breaks, and it breaks silently: the
/// stale memo still answers, with the previous revision's lowering. A scope of
/// "self only" leaves this test red, which is the property that makes the
/// narrowing testable at all rather than merely plausible.
///
/// The two queries are demanded SEPARATELY, because a combined count cannot tell
/// `main`'s stage1 re-executing from `sub`'s: the unit check demands `sub`'s
/// stage1 itself. Demanding `model_stage1(main)` alone can only re-execute
/// `main`'s -- that query reads Stage0s, never another model's Stage1 -- so the
/// first count attributes the invalidation to `main`.
#[test]
fn a_module_targets_edit_invalidates_its_instantiators_stage_and_unit_check() {
    let parent_child = |child_eqn: &str| {
        x_project(
            sim_specs_with_units("month"),
            &[
                x_model(
                    "main",
                    vec![
                        x_aux("driver", "5", Some("widget")),
                        x_module("sub", &[("driver", "sub.input")], None),
                        x_aux("combined", "sub.out", Some("widget")),
                    ],
                ),
                x_model(
                    "sub",
                    vec![
                        x_aux("input", "0", Some("widget")),
                        x_aux("out", child_eqn, Some("widget")),
                    ],
                ),
            ],
        )
    };

    let mut db = SimlinDb::default();
    let state1 = sync_from_datamodel_incremental(&mut db, &parent_child("input * 2"), None);
    let sync1 = state1.to_sync_result();
    let _ = model_stage1(&db, sync1.models["main"].source, sync1.project);
    crate::db::units::check_model_units(&db, sync1.models["main"].source, sync1.project);

    // Control: re-syncing the IDENTICAL project re-executes nothing, so the
    // counts below are attributable to the edit and not to the re-sync. Without
    // it this test would still pass if a no-op re-sync had started re-executing
    // everything -- it asserts re-execution, so a spurious cause satisfies it.
    reset_query_executions();
    let state2 =
        sync_from_datamodel_incremental(&mut db, &parent_child("input * 2"), Some(&state1));
    let sync2 = state2.to_sync_result();
    let _ = model_stage1(&db, sync2.models["main"].source, sync2.project);
    crate::db::units::check_model_units(&db, sync2.models["main"].source, sync2.project);
    assert_eq!(
        query_executions(),
        QueryExecutions::default(),
        "re-syncing an unchanged project must re-execute nothing"
    );

    reset_query_executions();
    let state3 =
        sync_from_datamodel_incremental(&mut db, &parent_child("input * 3"), Some(&state2));
    let sync3 = state3.to_sync_result();
    let main = sync3.models["main"].source;

    let _ = model_stage1(&db, main, sync3.project);
    assert_eq!(
        query_executions(),
        QueryExecutions {
            // `sub`'s Stage0 rebuilt because its equation changed; `main`'s did
            // not (its own variables are untouched), and `main`'s Stage1 rebuilt
            // because that Stage0 is in its lowering scope.
            stage0: 1,
            stage1: 1,
            unit_check: 0,
        },
        "a module target's edit must re-execute the INSTANTIATOR's lowered stage"
    );

    reset_query_executions();
    crate::db::units::check_model_units(&db, main, sync3.project);
    assert_eq!(
        query_executions(),
        QueryExecutions {
            stage0: 0,
            // the target's own Stage1, which only the unit check demanded
            stage1: 1,
            unit_check: 1,
        },
        "a module target's edit must re-execute the instantiator's unit check"
    );
}
