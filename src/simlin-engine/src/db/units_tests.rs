// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The unit pass over the per-variable lowered memos: the inference scope
//! (`model_scope_models`), the stdlib skip gate (`source_model_is_stdlib`),
//! the harvest points a unit warning reaches, and the invalidation each edit
//! class causes -- measured as `ProbedDb` body counts, since a backdated memo
//! keeps its address whether or not its body re-ran.

use super::*;
use crate::common::{Canonical, Ident};
use crate::datamodel;
use crate::db::exec_probe::ProbedDb;
use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};

// ── fixtures ────────────────────────────────────────────────────────────

/// A project with three user models: a `main` that instantiates two
/// sub-models, so the inference scope actually walks module edges.
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

/// [`three_model_project`] plus a dimension, an arrayed variable in `main`
/// and an arrayed output of `sub_a` that `main` reduces over.
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

/// A TWO-LEVEL module chain, `main -> sub_a -> sub_c`: `sub_c` is reachable
/// from `main` only THROUGH `sub_a`, which separates a transitive closure
/// from "self plus direct targets".
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

/// An implicit stdlib instance (`SMTH1`), an explicit sub-model instance read
/// through, and a macro-marked model with a caller: every module-edge kind
/// the inference scope must follow.
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
        ],
    );
    let mut project = x_project(sim_specs_with_units("month"), &[main, sub, scaled_macro]);
    project.dimensions = vec![datamodel::Dimension::named(
        "Region".to_string(),
        vec!["north".to_string(), "south".to_string()],
    )];
    project
}

/// Everything the unit pass accumulates for one model, drained through the
/// direct `check_model_units::accumulated` harvest point.
fn unit_pass_diagnostics(
    db: &SimlinDb,
    model: SourceModel,
    project: SourceProject,
) -> Vec<&Diagnostic> {
    crate::db::units::check_model_units::accumulated::<Diagnostic>(db, model, project)
}

/// The canonical model names in `model`'s unit-inference scope.
fn scope_names(db: &SimlinDb, sync: &SyncResult, model: &str) -> Vec<String> {
    crate::db::units::model_scope_models(db, sync.models[model].source, sync.project)
        .keys()
        .map(|ident| ident.as_str().to_string())
        .collect()
}

// ── the stdlib gate ─────────────────────────────────────────────────────

/// A model is a stdlib template only when the `stdlib⁚` prefix is followed
/// by a name that is actually in `stdlib::MODEL_NAMES`.
#[test]
fn model_is_stdlib_requires_a_known_stdlib_suffix() {
    assert!(crate::db::input::model_is_stdlib("stdlib\u{205A}smth1"));
    assert!(crate::db::input::model_is_stdlib("stdlib\u{205A}trend"));
    // Prefix present, suffix names no stdlib model: a user model, not a
    // generic template.
    assert!(!crate::db::input::model_is_stdlib("stdlib\u{205A}bogus"));
    assert!(!crate::db::input::model_is_stdlib("main"));
    assert!(!crate::db::input::model_is_stdlib("smth1"));
}

/// Every model `db::sync` splices in satisfies the strict predicate, so
/// tightening the rule cannot orphan a real stdlib template.
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

/// The DISPLAY name is canonicalized before the predicate sees it: an
/// imported `Stdlib⁚Smth1` is the same model as `stdlib⁚smth1`.
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

/// Every shipped stdlib template is scalar and instantiates no module --
/// asserted over the SYNCED parse memos (explicit variables and the helpers
/// their parses synthesize) rather than the generated source, so it is a
/// tripwire rather than an assumption. Two arguments rest on it:
///
///   - **Module-graph cycle safety**, the abort-class one. `MacroRegistry::build`'s
///     Pass 4 (rejecting a module inside a macro, `ErrorCode::MacroContainsModule`)
///     closes the hole where a cycle ran through an IMPLICIT module edge that
///     `db::project_module_graph` cannot see. Its closure argument needs "a
///     stdlib model is a SINK -- it instantiates no module", so that an implicit
///     stdlib edge cannot be one hop of a longer invisible cycle. Nothing stops
///     a future `stdlib/*.stmx` template from instantiating another template;
///     if the module assertion ever fails, the shape a template just gained is a
///     module edge invisible to the gate, and the gate must learn stdlib edges.
///   - **The unit-inference scope.** `model_scope_models` keeps the stdlib
///     templates a model instantiates rather than splicing every template in;
///     with every template scalar and module-free, a template adds nothing to
///     a scope beyond its own port constraints.
#[test]
fn stdlib_templates_are_scalar_and_instantiate_no_module() {
    let db = SimlinDb::default();
    let project = x_project(
        sim_specs_with_units("month"),
        &[x_model("main", vec![x_aux("x", "1", None)])],
    );
    let sync = sync_from_datamodel(&db, &project);

    let mut checked = 0usize;
    for (model_name, source) in sync.project.models(&db) {
        if !crate::db::source_model_is_stdlib(&db, *source) {
            continue;
        }
        for (ident, var) in source.variables(&db) {
            let parsed = parse_source_variable(&db, *var, sync.project);
            assert!(
                parsed.variable.get_dimensions().is_none(),
                "stdlib variable {model_name}·{ident} is arrayed: the template is no longer \
                 the scalar sink the inference scope assumes"
            );
            assert!(
                !parsed.variable.is_module(),
                "stdlib model {model_name} instantiates a module ({ident}): \
                 `db::project_module_graph` does not record implicit module edges, so the \
                 stdlib-sink premise of `MacroRegistry::build`'s Pass 4 closure argument no \
                 longer holds and a cycle through this edge is invisible to the cycle gate"
            );
            assert!(
                parsed.implicit_vars.iter().all(|iv| !iv.is_module()),
                "stdlib variable {model_name}·{ident} calls a module function: the template \
                 is no longer a module sink"
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

// ── diagnostics reach both harvest points ───────────────────────────────

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

/// A unit warning reaches BOTH harvest points: the direct
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
            .any(|d| matches!(&d.error, DiagnosticError::Unit(_))),
        "check_model_units::accumulated must carry the unit warning: {direct:?}"
    );

    let all = collect_all_diagnostics(&db, sync.project);
    assert!(
        all.iter().any(|d| d.model == "main"
            && d.severity == DiagnosticSeverity::Warning
            && matches!(&d.error, DiagnosticError::Unit(_))),
        "collect_all_diagnostics must carry the unit warning: {all:?}"
    );
}

/// GH #988: a model carrying the stdlib PREFIX but an unknown suffix is a user
/// model, so the unit pass checks it instead of skipping it, while a real
/// stdlib template is still skipped.
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
            .any(|d| matches!(&d.error, DiagnosticError::Unit(_))),
        "a prefix-only model must be unit-checked, not skipped: {direct:?}"
    );

    let real = sync.models["stdlib\u{205A}smth1"].source;
    assert!(
        unit_pass_diagnostics(&db, real, sync.project).is_empty(),
        "a real stdlib model must still be skipped by the unit pass"
    );
}

// ── the inference scope ─────────────────────────────────────────────────

/// The scope is the TRANSITIVE module-reachable closure, and nothing else:
/// `sub_c` is reachable from `main` only through `sub_a`, and the stdlib
/// templates spliced into every project are absent because this fixture
/// instantiates none of them.
#[test]
fn the_inference_scope_is_the_transitive_module_reachable_closure() {
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
/// than the modeller declaring -- is a scope edge like any other. This rules
/// out deriving the scope from `db::project_module_graph`, which records only
/// EXPLICIT `Variable::Module` edges: a macro call reaches an ordinary user
/// model whose parameter units inference binds to the call's arguments.
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
    // The macro model itself calls nothing, so its scope is just itself.
    assert_eq!(scope_names(&db, &sync, "scaled"), ["scaled"]);
}

/// A module CYCLE yields a finite scope containing the whole cycle, and both
/// members' variables still lower: `check_model_units` reaches them on a
/// cyclic project (the engine primitive is reachable even though
/// `collect_all_diagnostics` gates on the cycle first).
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
            !model_lowered_variables(&db, sync.models[name].source, sync.project).is_empty(),
            "a cyclic project's models must still lower"
        );
    }
}

/// The pe1 shape: `main` reaches a module cycle (`a <-> b`, when `cyclic`)
/// THROUGH the instance `m` that two per-element helpers read (`x[d] =
/// SMTH1(m.out + y[d], 1)`, `xs[d] = SMTH1(m.outarr[d] + y[d], 1)`); with
/// `cyclic` false, the same `main` over an acyclic `a`.
fn cycle_through_helper_project(cyclic: bool) -> datamodel::Project {
    use crate::testutils::x_module_named;

    let arrayed = |ident: &str, eqn: &str| x_apply_to_all(ident, "d", eqn);
    let main = x_model(
        "main",
        vec![
            x_aux("driver", "2", Some("people")),
            arrayed("y", "1"),
            x_module_named("m", "a", &[("driver", "m.inp")], None),
            arrayed("x", "SMTH1(m.out + y[d], 1)"),
            arrayed("xs", "SMTH1(m.outarr[d] + y[d], 1)"),
        ],
    );
    let mut model_a = vec![
        x_aux("inp", "1", Some("people")),
        x_aux("out", "inp * 2", Some("people")),
        arrayed("outarr", "inp * 3"),
    ];
    if cyclic {
        model_a.push(x_module_named("to_b", "b", &[("out", "to_b.binp")], None));
    }
    let mut models = vec![main, x_model("a", model_a)];
    if cyclic {
        models.push(x_model(
            "b",
            vec![
                x_aux("binp", "1", Some("people")),
                x_aux("bout", "binp * 2", Some("people")),
                x_module_named("to_a", "a", &[("binp", "to_a.inp")], None),
            ],
        ));
    }
    let mut project = x_project(sim_specs_with_units("month"), &models);
    project.dimensions.push(datamodel::Dimension::named(
        "d".to_string(),
        vec!["north".to_string(), "south".to_string()],
    ));
    project
}

/// The second arm of the cycle gate: the cycle is reached from `main` THROUGH
/// a module instance that a per-element helper reads (`x[d] = SMTH1(m.out +
/// y[d], 1)`, `xs[d] = SMTH1(m.outarr[d] + y[d], 1)`, `m` an instance of `a`,
/// `a <-> b`). The helper's element-pinned projection resolves `m` through the
/// instance's sub-model layout, a recursive query salsa cannot run under a
/// module cycle, so `model_lowered_variables` holds the memo's unpinned handle
/// where the module graph reaches a cycle and the pinned projection where it
/// does not; the unit pass reads the two identically, and the diagnostics
/// collector reports the cycle for `main`.
#[test]
fn a_module_cycle_reached_through_a_per_element_helper_still_unit_checks() {
    use crate::common::ErrorCode;
    use crate::db::lowered_implicit_variable;
    use std::sync::Arc;

    let project = cycle_through_helper_project;

    // Every element-scoped helper of `main`, with whether its map entry IS the
    // memo's handle (unpinned) rather than a projection of it (pinned).
    let element_scoped_entries = |db: &SimlinDb, sync: &SyncResult| -> Vec<(String, bool)> {
        let model = sync.models["main"].source;
        let map = model_lowered_variables(db, model, sync.project);
        let mut rows: Vec<(String, bool)> =
            crate::db::model_implicit_var_info(db, model, sync.project)
                .keys()
                .filter_map(|name| {
                    let memo = lowered_implicit_variable(db, model, sync.project, name.clone());
                    let memo = memo.as_ref()?;
                    memo.variable.element_scope().is_some().then(|| {
                        let entry = &map[&Ident::<Canonical>::new(name)];
                        (name.clone(), Arc::ptr_eq(entry, &memo.variable))
                    })
                })
                .collect();
        rows.sort();
        rows
    };
    let unit_rows = |db: &SimlinDb, sync: &SyncResult| -> Vec<(Option<String>, String)> {
        unit_pass_diagnostics(db, sync.models["main"].source, sync.project)
            .iter()
            .map(|d| (d.variable.clone(), format!("{:?}", d.error)))
            .collect()
    };
    let is_cycle = |d: &Diagnostic| matches!(&d.error, DiagnosticError::Model(err) if err.code == ErrorCode::CircularDependency);

    let db = SimlinDb::default();
    let cyclic = sync_from_datamodel(&db, &project(true));
    assert_eq!(
        scope_names(&db, &cyclic, "main"),
        ["a", "b", "main", "stdlib\u{205A}smth1"]
    );
    let entries = element_scoped_entries(&db, &cyclic);
    assert!(
        !entries.is_empty(),
        "the SMTH1 arguments are per-element helpers"
    );
    assert!(
        entries.iter().all(|(_, holds_memo)| *holds_memo),
        "under the cycle every element-scoped helper holds its memo's handle: {entries:?}"
    );
    let cyclic_units = unit_rows(&db, &cyclic);
    let main_diagnostics: Vec<Diagnostic> = collect_all_diagnostics(&db, cyclic.project)
        .into_iter()
        .filter(|d| d.model == "main")
        .collect();
    assert!(
        main_diagnostics.len() == 1 && is_cycle(&main_diagnostics[0]),
        "main reports the cycle and nothing else: {main_diagnostics:?}"
    );

    let db = SimlinDb::default();
    let acyclic = sync_from_datamodel(&db, &project(false));
    let entries = element_scoped_entries(&db, &acyclic);
    assert!(!entries.is_empty());
    assert!(
        entries.iter().all(|(_, holds_memo)| !*holds_memo),
        "with an acyclic module graph every element-scoped helper is pinned: {entries:?}"
    );
    assert_eq!(
        unit_rows(&db, &acyclic),
        cyclic_units,
        "the unit pass reads the pinned and the unpinned entry identically"
    );
    assert!(
        !collect_all_diagnostics(&db, acyclic.project)
            .iter()
            .any(is_cycle)
    );
}

/// Loop detection is an analysis entry point, so it carries the module-cycle
/// gate itself: a model the project's module graph reaches a cycle from has
/// no detected loops -- the cycle is the model error the diagnostics pass
/// reports, and enumerating its loops recurses into the instance's layout,
/// salsa's dependency-graph panic -- while the acyclic sibling of the same
/// `main` enumerates as usual.
#[test]
fn a_module_cycle_reached_through_a_per_element_helper_has_no_detected_loops() {
    use crate::db::model_detected_loops;

    let db = SimlinDb::default();
    let acyclic = sync_from_datamodel(&db, &cycle_through_helper_project(false));
    let detected = model_detected_loops(&db, acyclic.models["main"].source, acyclic.project);
    // `main` has no feedback of its own, so both arms yield the empty result;
    // what the gate decides is that the cyclic arm RETURNS it instead of
    // panicking in `compute_layout`'s dependency-graph cycle.
    assert!(
        detected.loops.is_empty() && detected.partitions.is_empty(),
        "{detected:?}"
    );

    let db = SimlinDb::default();
    let cyclic = sync_from_datamodel(&db, &cycle_through_helper_project(true));
    let detected = model_detected_loops(&db, cyclic.models["main"].source, cyclic.project);
    assert!(
        detected.loops.is_empty() && detected.partitions.is_empty(),
        "a model reaching a module cycle has no detected loops: {detected:?}"
    );
}

/// A cross-module unit mismatch that only closes through TWO module hops:
/// `main.x` (`widget`) feeds `sub_a.input` (undeclared), which feeds
/// `sub_c.input` (`gadget`). Under a direct-targets-only closure `sub_c` is
/// absent from `main`'s inference map and the mismatch goes unreported.
#[test]
fn a_two_hop_cross_module_unit_mismatch_is_still_reported() {
    use crate::common::ErrorCode;

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
    let project = x_project(sim_specs_with_units("month"), &[main, sub_a, sub_c]);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    // The precondition -- `sub_c` reachable only THROUGH `sub_a` -- is read
    // off the salsa inputs, not off `main`'s scope, which is what the test
    // protects.
    let targets = |model: &str| -> Vec<String> {
        sync.models[model]
            .source
            .variables(&db)
            .values()
            .filter(|sv| sv.kind(&db) == SourceVariableKind::Module)
            .map(|sv| sv.model_name(&db).clone())
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
        diagnostics
            .iter()
            .any(|d| d.is(DiagnosticCategory::UnitInference, ErrorCode::UnitMismatch)),
        "the two-hop widget/gadget contradiction must still be reported: {diagnostics:?}"
    );
}

/// A module-output read lowers WITHOUT bounds on the unit path exactly as on
/// the compile path (`db::lowering_scope_tests`): `main`'s reduction over
/// `sub_a`'s arrayed output carries `None` where its reduction over its own
/// arrayed `pop` carries the axis.
#[test]
fn a_module_output_read_lowers_without_bounds_on_the_unit_path() {
    use crate::ast::{Ast, Expr2};
    use crate::builtins::BuiltinFn;

    let db = SimlinDb::default();
    let project = arrayed_module_project();
    let sync = sync_from_datamodel(&db, &project);
    let units = crate::db::units::unit_model(&db, sync.models["main"].source, sync.project);

    let reduced_bounds = |var: &str| {
        let ident: Ident<Canonical> = Ident::new(var);
        let Some(Ast::Scalar(Expr2::App(BuiltinFn::Sum(arg), _, _))) =
            units.variables[&ident].ast()
        else {
            panic!("{var} is a scalar SUM");
        };
        let Expr2::Subscript(_, _, bounds, _) = &**arg else {
            panic!("{var} reduces over a subscripted reference");
        };
        bounds.clone()
    };
    assert!(
        reduced_bounds("total_pop").is_some(),
        "a wildcard over the model's own arrayed variable carries its axis"
    );
    assert!(
        reduced_bounds("sub_region_total").is_none(),
        "a wildcard over a module output carries no bounds: the compiler resolves it"
    );
}

// ── cost and invalidation, as body counts ───────────────────────────────

/// Collecting whole-project diagnostics lowers each variable ONCE -- the
/// fragment pass and the unit pass share the memo -- and assembles each
/// reachable model's variable map once, whichever model's unit check demands
/// it first (GH #966: no per-(model, model) rebuild).
#[test]
fn whole_project_diagnostics_lower_each_variable_once() {
    let mut probed = ProbedDb::new();
    let project = three_model_project();
    let state = sync_from_datamodel_incremental(probed.db_mut(), &project, None);
    let sync = state.to_sync_result();
    let db = probed.db();
    let n_models = sync.project.models(db).len();
    let n_user = project.models.len();
    let n_vars: usize = sync
        .project
        .models(db)
        .values()
        .map(|m| m.variables(db).len())
        .sum();
    assert!(
        n_models > n_user,
        "the fixture has stdlib models beyond its {n_user} user models"
    );

    probed.reset();
    let diagnostics = collect_all_diagnostics(probed.db(), sync.project);
    let counts = probed.counts();
    assert_eq!(
        counts.get("lowered_source_variable"),
        Some(&(n_vars, n_vars)),
        "every model's fragment pass lowers each variable once, and the unit pass reuses \
         it: {counts:?} (diagnostics: {diagnostics:?})"
    );
    assert_eq!(
        counts.get("model_lowered_variables"),
        Some(&(n_user, n_user)),
        "the unit pass assembles each user model's map once, though `main`'s scope reads \
         all three: {counts:?}"
    );
    assert_eq!(
        counts.get("check_model_units"),
        Some(&(n_models, n_models)),
        "the unit pass is entered for every model, stdlib included (skipped at the gate)"
    );

    // Re-reading every model's map and unit check rebuilds nothing.
    probed.reset();
    for source in sync.project.models(probed.db()).values() {
        crate::db::units::check_model_units(probed.db(), *source, sync.project);
        let _ = model_lowered_variables(probed.db(), *source, sync.project);
    }
    let counts = probed.counts();
    assert_eq!(
        counts
            .get("model_lowered_variables")
            .map(|c| c.0)
            .unwrap_or(0),
        n_models - n_user,
        "only the stdlib models' maps -- never demanded by a unit check -- are new: {counts:?}"
    );
    assert_eq!(counts.get("lowered_source_variable"), None, "{counts:?}");
    assert_eq!(counts.get("check_model_units"), None, "{counts:?}");
}

/// Editing a model with NO module relationship to `main` re-executes neither
/// a lowering of `main`'s variables nor `main`'s unit check; the edited
/// model's own do re-execute, or the fixture proves nothing.
#[test]
fn an_unrelated_models_edit_invalidates_neither_lowering_nor_unit_check() {
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

    let mut probed = ProbedDb::new();
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &unrelated_pair("1"), None);
    let sync1 = state1.to_sync_result();
    for name in ["main", "other"] {
        crate::db::units::check_model_units(probed.db(), sync1.models[name].source, sync1.project);
    }

    // Control: re-syncing the identical project re-executes nothing.
    probed.reset();
    let state2 =
        sync_from_datamodel_incremental(probed.db_mut(), &unrelated_pair("1"), Some(&state1));
    let sync2 = state2.to_sync_result();
    for name in ["main", "other"] {
        crate::db::units::check_model_units(probed.db(), sync2.models[name].source, sync2.project);
    }
    assert!(
        probed.counts().is_empty(),
        "re-syncing an unchanged project must re-execute nothing: {:?}",
        probed.counts()
    );

    probed.reset();
    let state3 =
        sync_from_datamodel_incremental(probed.db_mut(), &unrelated_pair("2"), Some(&state2));
    let sync3 = state3.to_sync_result();
    crate::db::units::check_model_units(probed.db(), sync3.models["main"].source, sync3.project);
    let counts = probed.counts();
    assert_eq!(counts.get("check_model_units"), None, "{counts:?}");
    assert_eq!(counts.get("lowered_source_variable"), None, "{counts:?}");
    assert_eq!(counts.get("model_lowered_variables"), None, "{counts:?}");

    crate::db::units::check_model_units(probed.db(), sync3.models["other"].source, sync3.project);
    let counts = probed.counts();
    assert_eq!(
        counts.get("check_model_units"),
        Some(&(1, 1)),
        "the EDITED model's unit check must re-execute: {counts:?}"
    );
    assert_eq!(
        counts.get("lowered_source_variable"),
        Some(&(1, 1)),
        "only the edited variable is re-lowered: {counts:?}"
    );
}

/// A model carrying a USER-AUTHORED NaN literal compares equal to its own
/// rebuild exactly like one without (GH #987): `ast::Literal` compares float
/// literals by bit pattern, which is what lets salsa backdate the lowered memo
/// instead of re-running every query in its cone per keystroke. Two rows,
/// derived from the one axis the property is about, and they must agree; the
/// control row runs first so a regression on the NaN row is attributable.
#[test]
fn a_nan_bearing_lowering_compares_equal_to_its_rebuild() {
    for (label, eqn) in [("control", "1 + 2"), ("nan literal", "1 + nan")] {
        let project = x_project(
            sim_specs_with_units("month"),
            &[x_model("main", vec![x_aux("x", eqn, Some("widget"))])],
        );
        // Two independent databases, so the second lowering is a genuine
        // rebuild rather than a memo read.
        let build = || {
            let db = SimlinDb::default();
            let sync = sync_from_datamodel(&db, &project);
            let x = sync.models["main"].variables["x"].source;
            (*lowered_source_variable(&db, x, sync.models["main"].source, sync.project).variable)
                .clone()
        };
        let first = build();
        let second = build();

        let holds_nan = first.ast().is_some_and(|ast| match ast {
            crate::ast::Ast::Scalar(e) => expr2_holds_nan(e),
            _ => false,
        });
        assert_eq!(
            holds_nan,
            label == "nan literal",
            "{label}: the fixture must carry a NaN literal iff it is the NaN row"
        );
        assert!(
            first == second,
            "{label}: a rebuilt lowering must compare equal to the original"
        );
    }
}

/// Does this `Expr2` tree contain a NaN literal? The fixture guard above.
fn expr2_holds_nan(expr: &crate::ast::Expr2) -> bool {
    use crate::ast::Expr2;
    match expr {
        Expr2::Const(_, n, _) => n.value().is_nan(),
        Expr2::Var(..) | Expr2::Subscript(..) => false,
        Expr2::App(builtin, _, _) => {
            let mut found = false;
            crate::builtins::walk_builtin_expr(builtin, |c| {
                if let crate::builtins::BuiltinContents::Expr(inner)
                | crate::builtins::BuiltinContents::LookupTable(inner) = c
                {
                    found |= expr2_holds_nan(inner);
                }
            });
            found
        }
        Expr2::Op1(_, inner, _, _) => expr2_holds_nan(inner),
        Expr2::Op2(_, l, r, _, _) => expr2_holds_nan(l) || expr2_holds_nan(r),
        Expr2::If(c, t, f, _, _) => expr2_holds_nan(c) || expr2_holds_nan(t) || expr2_holds_nan(f),
    }
}

/// Editing a model that `main` instantiates re-executes `main`'s UNIT CHECK
/// -- inference reads the target's variables through the scope -- and NOT
/// the lowering of `main`'s own variables: a module-output read carries no
/// bounds at the `Expr2` tier, so `combined = sub.out` lowers under nothing
/// of `sub`'s equations. A too-narrow scope breaks the first half silently
/// (the stale memo still answers); a too-wide lowering breaks the second.
#[test]
fn a_module_targets_edit_invalidates_the_unit_check_and_not_the_instantiators_lowering() {
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

    let mut probed = ProbedDb::new();
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &parent_child("input * 2"), None);
    let sync1 = state1.to_sync_result();
    crate::db::units::check_model_units(probed.db(), sync1.models["main"].source, sync1.project);

    // Control: re-syncing the IDENTICAL project re-executes nothing.
    probed.reset();
    let state2 =
        sync_from_datamodel_incremental(probed.db_mut(), &parent_child("input * 2"), Some(&state1));
    let sync2 = state2.to_sync_result();
    crate::db::units::check_model_units(probed.db(), sync2.models["main"].source, sync2.project);
    assert!(
        probed.counts().is_empty(),
        "re-syncing an unchanged project must re-execute nothing: {:?}",
        probed.counts()
    );

    probed.reset();
    let state3 =
        sync_from_datamodel_incremental(probed.db_mut(), &parent_child("input * 3"), Some(&state2));
    let sync3 = state3.to_sync_result();
    crate::db::units::check_model_units(probed.db(), sync3.models["main"].source, sync3.project);
    let counts = probed.counts();
    assert_eq!(
        counts.get("check_model_units"),
        Some(&(1, 1)),
        "a module target's edit must re-execute the instantiator's unit check: {counts:?}"
    );
    assert_eq!(
        counts.get("lowered_source_variable"),
        Some(&(1, 1)),
        "only `sub.out` is re-lowered; `main`'s variables read nothing of its equation: \
         {counts:?}"
    );
    assert_eq!(
        counts.get("model_lowered_variables"),
        Some(&(1, 1)),
        "only `sub`'s map is rebuilt: {counts:?}"
    );
}
