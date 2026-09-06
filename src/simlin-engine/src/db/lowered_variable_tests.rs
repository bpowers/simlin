// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The per-variable lowered memos (`lowered_source_variable`,
//! `lowered_implicit_variable`) are the ONE owner of a variable's `Expr2`
//! form: the fragment compiler borrows the memo, unit checking and the LTM
//! describers hold its `Arc`, and every consumer together lowers each variable
//! exactly once. Pinned with pointer identity against the production memos and
//! with `ProbedDb` body counts, since salsa backdating keeps a memo's address
//! whether or not its body re-ran.

use std::borrow::Cow;
use std::sync::Arc;

use super::*;
use crate::common::{Canonical, Ident};
use crate::db::exec_probe::ProbedDb;
use crate::db::fragment_compile::implicit_fragment_input;
use crate::db::var_fragment::{ExplicitFragment, explicit_fragment_input};
use crate::test_common::TestProject;

/// Every variable kind the memos serve: a stock, a flow, an aux, an arrayed
/// aux, an explicit module instance, a stdlib call (an implicit module plus a
/// hoisted constant argument), a structural apply-to-all capture, and a
/// module-bearing apply-to-all body whose hoisted argument is minted per
/// element (an element-scoped helper).
fn shared_fixture() -> datamodel::Project {
    let mut project = TestProject::new("main")
        .named_dimension("region", &["north", "south"])
        .stock("level", "7", &["fill"], &["drain"], Some("widgets"))
        .flow("fill", "1", Some("widgets/time"))
        .flow("drain", "level * 0.1", None)
        .aux("rate", "level / 2", Some("widgets"))
        .aux("smoothed", "SMTH1(rate, 3)", None)
        .array_aux("pop[region]", "level * 2")
        .array_aux("lagged[region]", "PREVIOUS(pop + 1)")
        .array_aux("per_elem[region]", "SMTH1(pop * 2, 3)")
        .build_datamodel();
    project.models[0]
        .variables
        .push(crate::testutils::x_module_named(
            "inst",
            "sub",
            &[("rate", "inst.port")],
            None,
        ));
    project.models.push(crate::testutils::x_model(
        "sub",
        vec![
            crate::testutils::x_aux("port", "1", None),
            crate::testutils::x_aux("out", "port * 2", None),
        ],
    ));
    project
}

/// The production fragment input of one explicit variable, or a panic naming
/// the diagnostics that stopped it.
fn ready_input<'db>(
    db: &'db SimlinDb,
    sync: &SyncResult,
    model: &str,
    var: &str,
) -> Box<crate::compiler::fragment::FragmentInput<'db>> {
    let source = sync.models[model].variables[var].source;
    let ExplicitFragment { diagnostics, input } = explicit_fragment_input(
        db,
        source,
        sync.models[model].source,
        sync.project,
        &[],
        crate::db::LtmOverlay::Off,
    );
    input.unwrap_or_else(|| panic!("{model}.{var} must lower for this fixture: {diagnostics:?}"))
}

/// The fragment compiler borrows the memo, and the LTM map and the unit view
/// hold the memo's `Arc`, for every explicit variable of every kind.
#[test]
fn fragments_units_and_the_ltm_map_share_one_lowered_source_variable() {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &shared_fixture());
    let main = sync.models["main"].source;
    let map = model_lowered_variables(&db, main, sync.project);
    let units = crate::db::units::unit_model(&db, main, sync.project);

    let mut seen = 0usize;
    for (name, synced) in &sync.models["main"].variables {
        let memo = &lowered_source_variable(&db, synced.source, main, sync.project).variable;
        let ident: Ident<Canonical> = Ident::new(name);
        let input = ready_input(&db, &sync, "main", name);
        assert!(
            matches!(input.target, Cow::Borrowed(_)),
            "{name}: the fragment must borrow the memo, not clone it"
        );
        assert!(
            std::ptr::eq(input.target.as_ref(), &**memo),
            "{name}: the fragment's target must be the memo's variable"
        );
        assert!(
            Arc::ptr_eq(&map[&ident], memo),
            "{name}: the LTM map must hold the memo's Arc"
        );
        assert!(
            Arc::ptr_eq(&units.variables[&ident], memo),
            "{name}: the unit view must hold the memo's Arc"
        );
        seen += 1;
    }
    // level, fill, drain, rate, smoothed, pop, lagged, per_elem, inst.
    assert_eq!(seen, 9, "every variable of the fixture is asserted on");
}

/// The fragment compiler borrows a helper's memo and the LTM map holds its
/// `Arc`, except for an element-scoped helper, whose map entry is the memo's
/// element-pinned projection -- the describers classify reads by spelling, so
/// they see the static index the compiled fragment reads.
#[test]
fn fragments_and_the_ltm_map_share_one_lowered_implicit_variable() {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &shared_fixture());
    let main = sync.models["main"].source;
    let map = model_lowered_variables(&db, main, sync.project);
    let units = crate::db::units::unit_model(&db, main, sync.project);

    let mut plain = 0usize;
    let mut element_scoped = 0usize;
    for (name, meta) in model_implicit_var_info(&db, main, sync.project) {
        let memo = &lowered_implicit_variable(&db, main, sync.project, name.clone())
            .as_ref()
            .unwrap_or_else(|| panic!("{name}: the parse synthesized this helper"))
            .variable;
        let input = implicit_fragment_input(
            &db,
            meta,
            main,
            sync.project,
            &[],
            crate::db::LtmOverlay::Off,
        )
        .unwrap_or_else(|_| panic!("{name}: the helper must lower for this fixture"));
        assert!(
            matches!(input.target, Cow::Borrowed(_))
                && std::ptr::eq(input.target.as_ref(), &**memo),
            "{name}: the fragment must borrow the memo"
        );
        let ident: Ident<Canonical> = Ident::new(name);
        if memo.element_scope().is_some() {
            element_scoped += 1;
            let pinned = &map[&ident];
            assert!(
                !Arc::ptr_eq(pinned, memo),
                "{name}: the map holds the pinned projection, not the memo"
            );
            assert!(
                pinned.element_scope().is_none(),
                "{name}: the pinned projection carries no scope"
            );
            assert_eq!(
                **pinned,
                input.element_pinned_target(),
                "{name}: the map entry is the fragment input's pinned target"
            );
            assert!(
                Arc::ptr_eq(&units.variables[&ident], pinned),
                "{name}: the unit view reads the same map"
            );
        } else {
            plain += 1;
            assert!(
                Arc::ptr_eq(&map[&ident], memo),
                "{name}: the LTM map must hold the memo's Arc"
            );
            assert!(
                Arc::ptr_eq(&units.variables[&ident], memo),
                "{name}: the unit view must hold the memo's Arc"
            );
        }
    }
    assert!(plain >= 3, "the fixture holds plain helpers: {plain}");
    assert!(
        element_scoped >= 2,
        "the fixture holds per-element helpers: {element_scoped}"
    );
}

/// The whole pipeline -- diagnostics (fragments and units), assembly, and the
/// LTM describers -- lowers each explicit variable and each helper exactly
/// once, across every model the project holds.
#[test]
fn every_consumer_lowers_each_variable_once() {
    let mut probed = ProbedDb::new();
    let state = sync_from_datamodel_incremental(probed.db_mut(), &shared_fixture(), None);
    let sync = state.to_sync_result();
    let db = probed.db();

    let n_explicit: usize = sync
        .project
        .models(db)
        .values()
        .map(|m| m.variables(db).len())
        .sum();
    let n_helpers: usize = sync
        .project
        .models(db)
        .values()
        .map(|m| model_implicit_var_info(db, *m, sync.project).len())
        .sum();
    assert!(
        n_explicit > 12 && n_helpers >= 5,
        "{n_explicit} / {n_helpers}"
    );

    probed.reset();
    let db = probed.db();
    let diagnostics = collect_all_diagnostics(db, sync.project, crate::db::LtmOverlay::On);
    assert!(
        !diagnostics
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Error),
        "the fixture compiles: {diagnostics:?}"
    );
    compile_project_incremental(db, sync.project, "main", crate::db::LtmOverlay::On)
        .expect("assembly");
    let main = sync.models["main"].source;
    let _ = model_lowered_variables(db, main, sync.project);
    let _ = crate::db::ltm_ir::model_ltm_reference_sites(db, main, sync.project);
    let _ = crate::ltm_agg::enumerate_agg_nodes(db, main, sync.project);
    let _ = lowered_variable_by_name(db, main, sync.project, "rate");
    let _ = lowered_variable_by_name(db, main, sync.project, "$⁚smoothed⁚0⁚smth1");
    let _ = crate::db::units::unit_model(db, main, sync.project);

    let counts = probed.counts();
    assert_eq!(
        counts.get("lowered_source_variable"),
        Some(&(n_explicit, n_explicit)),
        "one lowering per explicit variable: {counts:?}"
    );
    assert_eq!(
        counts.get("lowered_implicit_variable"),
        Some(&(n_helpers, n_helpers)),
        "one lowering per helper: {counts:?}"
    );
}

/// An equation edit re-lowers the edited variable and nothing else: a
/// dependent lowers under the edited variable's SHAPE, which the edit leaves
/// alone, and a helper of another variable reads nothing of it.
#[test]
fn an_equation_edit_relowers_only_the_edited_variable() {
    let with_rate = |eqn: &str| {
        let mut project = shared_fixture();
        for var in &mut project.models[0].variables {
            if let datamodel::Variable::Aux(aux) = var
                && aux.ident == "rate"
            {
                aux.equation = datamodel::Equation::Scalar(eqn.to_string());
            }
        }
        project
    };

    let mut probed = ProbedDb::new();
    let state1 = sync_from_datamodel_incremental(probed.db_mut(), &with_rate("level / 2"), None);
    let sync1 = state1.to_sync_result();
    compile_project_incremental(
        probed.db(),
        sync1.project,
        "main",
        crate::db::LtmOverlay::Off,
    )
    .expect("first compile");
    let _ = collect_all_diagnostics(probed.db(), sync1.project, crate::db::LtmOverlay::Off);

    // Control: an identical re-sync re-lowers nothing.
    probed.reset();
    let state2 =
        sync_from_datamodel_incremental(probed.db_mut(), &with_rate("level / 2"), Some(&state1));
    let sync2 = state2.to_sync_result();
    compile_project_incremental(
        probed.db(),
        sync2.project,
        "main",
        crate::db::LtmOverlay::Off,
    )
    .expect("re-sync compile");
    let _ = collect_all_diagnostics(probed.db(), sync2.project, crate::db::LtmOverlay::Off);
    let counts = probed.counts();
    assert_eq!(counts.get("lowered_source_variable"), None, "{counts:?}");
    assert_eq!(counts.get("lowered_implicit_variable"), None, "{counts:?}");

    probed.reset();
    let state3 =
        sync_from_datamodel_incremental(probed.db_mut(), &with_rate("level / 3"), Some(&state2));
    let sync3 = state3.to_sync_result();
    compile_project_incremental(
        probed.db(),
        sync3.project,
        "main",
        crate::db::LtmOverlay::Off,
    )
    .expect("edited compile");
    let _ = collect_all_diagnostics(probed.db(), sync3.project, crate::db::LtmOverlay::Off);
    let counts = probed.counts();
    assert_eq!(
        counts.get("lowered_source_variable"),
        Some(&(1, 1)),
        "only `rate` re-lowers: {counts:?}"
    );
    assert_eq!(
        counts.get("lowered_implicit_variable"),
        None,
        "no helper reads `rate`'s equation: {counts:?}"
    );
}

/// The `main` rule of module wiring -- a parent-scope `·x` source is stripped
/// in the root model -- compares CANONICAL model names, so a root model
/// spelled `Main` wires its instances exactly as one spelled `main`.
#[test]
fn module_wiring_strips_the_parent_scope_prefix_under_a_display_cased_main() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module_named, x_project};

    for root in ["main", "Main"] {
        let project = x_project(
            sim_specs_with_units("month"),
            &[
                x_model(
                    root,
                    vec![
                        x_aux("x", "5", None),
                        x_module_named("inst", "sub", &[(".x", "inst.port")], None),
                        x_aux("echo", "inst.out", None),
                    ],
                ),
                x_model(
                    "sub",
                    vec![x_aux("port", "0", None), x_aux("out", "port * 2", None)],
                ),
            ],
        );
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let main = sync.models["main"].source;
        let inst = sync.models["main"].variables["inst"].source;
        let lowered = &lowered_source_variable(&db, inst, main, sync.project).variable;
        let crate::variable::VarKind::Module { inputs, .. } = &lowered.kind else {
            panic!("{root}: `inst` is a module instance");
        };
        let wiring: Vec<(&str, &str)> = inputs
            .iter()
            .map(|mi| (mi.src.as_str(), mi.dst.as_str()))
            .collect();
        assert_eq!(
            wiring,
            vec![("x", "port")],
            "{root}: the `·` prefix is stripped"
        );

        let compiled =
            compile_project_incremental(&db, sync.project, "main", crate::db::LtmOverlay::Off)
                .unwrap_or_else(|e| panic!("{root}: the wired project compiles: {e}"));
        let mut vm = crate::Vm::new(compiled).expect("vm");
        vm.run_to_end().expect("run");
        let results = crate::test_common::collect_results(&vm.into_results());
        assert_eq!(
            results["echo"][0], 10.0,
            "{root}: `inst` reads `x` through the port"
        );
    }
}

/// A dependency's graphical-function tables reach a fragment through a tracked
/// projection (`variable_tables`), so an edit of the GF-bearing `k` recompiles
/// its reader `probe` only when the table changes: an equation-text edit
/// re-lowers and recompiles `k` alone, while an edit of the table's values or
/// of its x-range recompiles `probe` too.
#[test]
fn a_dependency_tables_projection_backdates_on_an_equation_edit() {
    let gf = |y_points: Vec<f64>, x_max: f64| datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: None,
        y_points,
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: x_max,
        },
        y_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
    };
    let with_k = |eqn: &str, gf: datamodel::GraphicalFunction| {
        TestProject::new("main")
            .with_sim_time(0.0, 1.0, 1.0)
            .aux_with_gf("k", eqn, gf)
            .scalar_aux("probe", "k * 2")
            .build_datamodel()
    };
    let mut probed = ProbedDb::new();
    let mut state = sync_from_datamodel_incremental(
        probed.db_mut(),
        &with_k("3", gf(vec![0.0, 5.0], 10.0)),
        None,
    );
    compile_project_incremental(
        probed.db(),
        state.to_sync_result().project,
        "main",
        crate::db::LtmOverlay::Off,
    )
    .expect("first compile");

    // (edit, the project after it, fragments recompiled)
    let rows = [
        ("an equation edit", with_k("4", gf(vec![0.0, 5.0], 10.0)), 1),
        (
            "a table values edit",
            with_k("4", gf(vec![0.0, 6.0], 10.0)),
            2,
        ),
        (
            "a table x-range edit",
            with_k("4", gf(vec![0.0, 6.0], 20.0)),
            2,
        ),
    ];
    for (edit, project, recompiled) in rows {
        probed.reset();
        state = sync_from_datamodel_incremental(probed.db_mut(), &project, Some(&state));
        compile_project_incremental(
            probed.db(),
            state.to_sync_result().project,
            "main",
            crate::db::LtmOverlay::Off,
        )
        .expect(edit);
        let counts = probed.counts();
        assert_eq!(
            counts.get("variable_tables"),
            Some(&(1, 1)),
            "{edit}: `k`'s tables are re-derived: {counts:?}"
        );
        assert_eq!(
            counts.get("compile_var_fragment"),
            Some(&(recompiled, recompiled)),
            "{edit}: {recompiled} fragment(s) recompile: {counts:?}"
        );
    }
}
