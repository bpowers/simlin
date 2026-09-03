// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The structured dependency relation (`DepRef`): every read of a variable
//! classified once on its typed tree, each `·` hop proven against the model,
//! and one set of projections every scheduling consumer reads. Rows are one
//! per spelling a consumer resolves (a parent-scope source, a module read to
//! a stock or an aux output, a nested path, a helper's instance, an element
//! beside a like-named module, a flat dotted aux, an unproven spelling), the
//! phase x lag enumeration, and one per consumer of the relation -- the
//! dependency graph, the LTM gates, the exit-port selection, the firewall --
//! all through the production queries.

use std::collections::BTreeSet;

use super::*;
use crate::common::{Canonical, ErrorCode, Ident};
use crate::db::exec_probe::ProbedDb;
use crate::test_common::TestProject;
use crate::testutils::{x_aux, x_flow, x_model, x_module_named, x_stock};
use crate::variable::DepLag;

/// The input-agnostic dependency memo of one variable of `main`.
fn deps<'db>(db: &'db SimlinDb, sync: &SyncResult, var: &str) -> &'db VariableDeps {
    variable_direct_dependencies(
        db,
        sync.models["main"].variables[var].source,
        sync.project,
        ModuleInputSet::empty(db),
    )
}

/// One read as a row: `(phase, lag, module path, variable, stock output)`.
type Row = (DepPhase, DepLag, Vec<String>, String, bool);

fn rows(deps: &DepRefs) -> BTreeSet<Row> {
    deps.iter()
        .map(|dep| {
            (
                dep.phase,
                dep.lag,
                dep.target
                    .module_path
                    .iter()
                    .map(|i| i.as_str().to_string())
                    .collect(),
                dep.target.variable.as_str().to_string(),
                dep.target.stock_output,
            )
        })
        .collect()
}

/// A read in both phases (an aux without an initial equation reads the same
/// names in each).
fn both(lag: DepLag, path: &[&str], variable: &str, stock_output: bool) -> Vec<Row> {
    [DepPhase::Dt, DepPhase::Init]
        .into_iter()
        .map(|phase| {
            (
                phase,
                lag,
                path.iter().map(|s| s.to_string()).collect(),
                variable.to_string(),
                stock_output,
            )
        })
        .collect()
}

/// The fixture every consumer arm reads: `main` with an explicit instance of
/// `mid` (which instantiates `leaf`) and of `leaf` itself, a stdlib call, a
/// dimension `d` beside a module `d`, and a Stella-style flat `child.output`
/// aux beside a module `child`.
fn consumer_arms_project() -> datamodel::Project {
    let mut project = TestProject::new("main")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("d", &["e", "f"])
        .aux("area", "100", None)
        .aux("rate", "2", None)
        .aux("smoothed", "SMTH1(rate, 3)", None)
        .aux("reads_stock", "m.level", None)
        .aux("reads_aux", "m.out", None)
        .aux("nested", "outer.inner.out", None)
        .array_aux("arr[d]", "1")
        .aux("beside_dimension", "d.e + arr[d.e]", None)
        .aux("child.output", "99", None)
        .aux("flat_and_module", "child.output", None)
        .build_datamodel();
    project.models[0].variables.extend([
        // A parent-scope source spelled `.area`, as XMILE writes one.
        x_module_named("m", "leaf", &[(".area", "m.port")], None),
        x_module_named("outer", "mid", &[], None),
        x_module_named("d", "leaf", &[], None),
        x_module_named("child", "leaf", &[], None),
    ]);
    project.models.push(x_model(
        "leaf",
        vec![
            x_aux("port", "1", None),
            x_aux("e", "2", None),
            x_aux("out", "port * 2", None),
            x_aux("output", "port", None),
            x_stock("level", "port", &[], &[], None),
        ],
    ));
    project.models.push(x_model(
        "mid",
        vec![x_module_named("inner", "leaf", &[], None)],
    ));
    project
}

/// One row per spelling a consumer resolves: the leading `·` a module
/// source strips, a module read to a stock output and to an aux output, a
/// nested `m·n·x`, a `$⁚` helper's instance, a `dimension·element` beside a
/// module named like the dimension, and a flat `child·output` aux beside a
/// module `child`.
#[test]
fn every_consumer_arm_reads_one_target_relation() {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &consumer_arms_project());

    // A module instance's parent-scope source is the bare local name.
    assert_eq!(
        rows(&deps(&db, &sync, "m").deps),
        both(DepLag::Current, &[], "area", false)
            .into_iter()
            .collect()
    );
    // A stock of the instance's sub-model: a prior-step read in the dt phase.
    assert_eq!(
        rows(&deps(&db, &sync, "reads_stock").deps),
        both(DepLag::Current, &["m"], "level", true)
            .into_iter()
            .collect()
    );
    // An aux of the sub-model: a current read the instance orders.
    assert_eq!(
        rows(&deps(&db, &sync, "reads_aux").deps),
        both(DepLag::Current, &["m"], "out", false)
            .into_iter()
            .collect()
    );
    // Two proven hops.
    assert_eq!(
        rows(&deps(&db, &sync, "nested").deps),
        both(DepLag::Current, &["outer", "inner"], "out", false)
            .into_iter()
            .collect()
    );
    // The stdlib instance the parse synthesized is a proven head, and its
    // output is the template's stock.
    let smoothed = deps(&db, &sync, "smoothed");
    assert_eq!(
        rows(&smoothed.deps),
        both(
            DepLag::Current,
            &["$\u{205A}smoothed\u{205A}0\u{205A}smth1"],
            "output",
            true
        )
        .into_iter()
        .collect()
    );
    let instance = smoothed
        .implicit_vars
        .iter()
        .find(|iv| iv.is_module)
        .expect("SMTH1 synthesizes an instance");
    assert!(
        rows(&instance.deps).contains(&(
            DepPhase::Dt,
            DepLag::Current,
            vec![],
            "rate".to_string(),
            false
        )),
        "the instance reads its input source: {:?}",
        rows(&instance.deps)
    );
    // `d·e` beside a dimension `d` is the element's constant, never a read,
    // even though a module `d` exists.
    assert_eq!(
        rows(&deps(&db, &sync, "beside_dimension").deps),
        both(DepLag::Current, &[], "arr", false)
            .into_iter()
            .collect()
    );
    // A module `child` proves the hop: `child.output` is the module read,
    // not the flat aux spelled the same.
    assert_eq!(
        rows(&deps(&db, &sync, "flat_and_module").deps),
        both(DepLag::Current, &["child"], "output", false)
            .into_iter()
            .collect()
    );

    // The dt ordering follows the stock bit: `reads_stock` orders after
    // nothing in the dt phase and after `m` in the init phase, `reads_aux`
    // after `m` in both, `smoothed` after its instance only in init.
    let graph = model_dependency_graph(
        &db,
        sync.models["main"].source,
        sync.project,
        ModuleInputSet::empty(&db),
    );
    let dt = |name: &str| -> Vec<&str> {
        graph.dt_dependencies[name]
            .iter()
            .map(|d| d.as_str())
            .collect()
    };
    let init = |name: &str| -> Vec<&str> {
        graph.initial_dependencies[name]
            .iter()
            .map(|d| d.as_str())
            .collect()
    };
    // (A module is a sink of the transitive relation, so its own source
    // `area` is not absorbed into its readers' sets.)
    assert_eq!(dt("reads_stock"), Vec::<&str>::new());
    assert_eq!(init("reads_stock"), vec!["m"]);
    assert_eq!(dt("reads_aux"), vec!["m"]);
    assert_eq!(dt("nested"), vec!["outer"]);
    assert_eq!(dt("smoothed"), Vec::<&str>::new());
    assert!(init("smoothed").contains(&"$\u{205A}smoothed\u{205A}0\u{205A}smth1"));

    // The nested read names its port inside the first instance.
    let ports = model_module_output_ports(&db, sync.models["main"].source, sync.project);
    assert_eq!(ports["outer"], vec!["inner\u{00B7}out".to_string()]);
    assert_eq!(ports["m"], vec!["level".to_string(), "out".to_string()]);
    assert_eq!(ports["child"], vec!["output".to_string()]);
    assert!(
        !ports.contains_key("d"),
        "a dimension element is not a port"
    );
}

/// The other side of the flat-aux arm: with no module `child`, the
/// `child.output` spelling is one local name the model declares, and a
/// spelling the model proves no hop of is one local name it does not, which
/// its fragment reports as the unknown dependency. (The compiler's own
/// resolver reads every `·` as a hop, so a flat aux spelled `child.output`
/// is refused at lowering in either role -- the dependency listing is what
/// this pins, and `simlin_model_get_incoming_links` reads that listing.)
#[test]
fn an_unproven_qualified_spelling_is_one_local_name() {
    let mut project = TestProject::new("main")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("child.output", "99", None)
        .aux("flat", "child.output", None)
        .aux("bad", "ghost.x", None)
        .build_datamodel();
    // A module source under the parent-scope spelling: the leading `·` is
    // not part of the local name the unproven spelling becomes.
    project.models[0].variables.push(x_module_named(
        "wired",
        "leaf",
        &[(".ghost.x", "wired.port")],
        None,
    ));
    project
        .models
        .push(x_model("leaf", vec![x_aux("port", "1", None)]));
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    assert_eq!(
        rows(&deps(&db, &sync, "flat").deps),
        both(DepLag::Current, &[], "child\u{00B7}output", false)
            .into_iter()
            .collect()
    );
    assert_eq!(
        rows(&deps(&db, &sync, "bad").deps),
        both(DepLag::Current, &[], "ghost\u{00B7}x", false)
            .into_iter()
            .collect()
    );
    assert_eq!(
        rows(&deps(&db, &sync, "wired").deps),
        both(DepLag::Current, &[], "ghost\u{00B7}x", false)
            .into_iter()
            .collect()
    );
    let errors = TestProject::from_datamodel(project).error_diagnostics();
    assert!(
        errors
            .iter()
            .any(|(loc, code)| loc == "main.bad" && *code == ErrorCode::UnknownDependency),
        "{errors:?}"
    );
}

/// Every phase x lag cell through production: an aux reads the same names
/// in both phases, a stock's initial equation is its only equation, and an
/// initial equation of its own splits the phases.
#[test]
fn every_phase_and_lag_arm_from_production() {
    let project = TestProject::new("main")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("a", "1", None)
        .aux("b", "2", None)
        .aux("c", "3", None)
        .aux("reader", "a + PREVIOUS(b, 0) + INIT(c)", None)
        .stock("level", "a + INIT(b)", &[], &[], None)
        .build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let mut expected: Vec<Row> = both(DepLag::Current, &[], "a", false);
    expected.extend(both(DepLag::Previous, &[], "b", false));
    expected.extend(both(DepLag::Initial, &[], "c", false));
    assert_eq!(
        rows(&deps(&db, &sync, "reader").deps),
        expected.into_iter().collect()
    );
    let mut expected: Vec<Row> = both(DepLag::Current, &[], "a", false);
    expected.extend(both(DepLag::Initial, &[], "b", false));
    assert_eq!(
        rows(&deps(&db, &sync, "level").deps),
        expected.into_iter().collect()
    );
}

/// The previous-only projection is `Previous - Current`: a snapshot read of
/// the same name -- `INIT(x)`, or the fallback of the `PREVIOUS` itself --
/// does not cancel the lag ("Phase 8.5 semantic divergences" 1). XMILE 1.0
/// 3.5.6: `PREVIOUS(price, 0)` "returns the value of price in the last DT,
/// or zero in the first DT", and INIT is the "initial value (i.e., value at
/// STARTTIME) of a variable"; so `x = PREVIOUS(y, INIT(y)) + 1` over
/// `y = x * 2` carries one DT of memory around a stockless cycle, which LTM
/// scores as a loop. The cycle compiles only with its init phase broken --
/// the fallback is an init-phase read of `y`, whose initial value otherwise
/// reads `x` -- so `y` carries an initial equation of its own.
#[test]
fn an_initial_read_does_not_cancel_a_previous_only_lag() {
    let mut project = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("y", "x * 2", None)
        .aux("x", "PREVIOUS(y, INIT(y)) + 1", None)
        .aux("mixed", "PREVIOUS(y, 0) + INIT(y)", None)
        .aux("instantaneous", "PREVIOUS(y, 0) + y", None)
        .build_datamodel();
    let datamodel::Variable::Aux(y) = &mut project.models[0].variables[0] else {
        panic!("y is the first aux");
    };
    assert_eq!(y.ident, "y");
    y.compat.active_initial = Some("5".to_string());

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let previous_only = |name: &str| -> Vec<String> {
        deps(&db, &sync, name)
            .deps
            .dt_previous_only()
            .iter()
            .map(|t| t.variable.as_str().to_string())
            .collect()
    };
    assert_eq!(previous_only("x"), vec!["y"]);
    assert_eq!(previous_only("mixed"), vec!["y"]);
    assert_eq!(previous_only("instantaneous"), Vec::<String>::new());

    // The lagged cycle `x -> y -> x` is state: LTM scores it, and the
    // scores read 1 from the first step the lag carries a value.
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .unwrap_or_else(|e| panic!("the lagged cycle compiles: {e:?}"));
    let mut vm = crate::vm::Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("run");
    let series = crate::test_common::collect_results(&vm.into_results());
    assert_eq!(series["x"], vec![6.0, 13.0, 27.0, 55.0]);
    assert_eq!(series["y"], vec![12.0, 26.0, 54.0, 110.0]);
    for key in [
        "$\u{205A}ltm\u{205A}link_score\u{205A}x\u{2192}y",
        "$\u{205A}ltm\u{205A}link_score\u{205A}y\u{2192}x",
        "$\u{205A}ltm\u{205A}loop_score\u{205A}u1",
    ] {
        let Some(score) = series.get(key) else {
            let mut keys: Vec<&String> = series.keys().collect();
            keys.sort();
            panic!("{key} is a series of the run: {keys:?}");
        };
        assert_eq!(score, &vec![0.0, 1.0, 1.0, 1.0], "{key}");
    }
}

/// A qualified read of a sub-model STOCK is a prior-step read at any depth
/// (`ordering_edges`, "Phase 8.5 semantic divergences" 5): `stock_read =
/// m.n.level` does not order `stock_read` after `m`, so `feeder =
/// stock_read * 2`, wired into `m`, runs before the instance and `m` reads
/// the current `feeder` each step. Ordering the reader after the instance
/// would close `stock_read -> m -> feeder -> stock_read`, a cycle the cycle
/// relation cannot see (a module is a sink there) and the sort breaks by
/// emitting `m` before its input, which then reads an unwritten slot -- the
/// #591-c1 stale-input class, here at depth two. A nested stock reader with
/// no other dt read joins the initials runlist, as a one-hop reader does.
#[test]
fn a_nested_stock_read_is_a_prior_step_read_at_any_depth() {
    let leaf = || {
        x_model(
            "leaf",
            vec![
                x_stock("level", "1", &["growth"], &[], None),
                x_flow("growth", "level * 0.1", None),
                x_aux("inp", "0", None),
                x_aux("out", "level + inp", None),
            ],
        )
    };
    // Inside a sub-model a connect source is spelled bare; only `main`
    // strips the parent-scope `.`.
    let mid = || {
        x_model(
            "mid",
            vec![
                x_aux("feed", "0", None),
                x_module_named("n", "leaf", &[("feed", "n.inp")], None),
                x_aux("mout", "n.out", None),
            ],
        )
    };
    let graph_of = |project: &datamodel::Project| {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, project);
        model_dependency_graph(
            &db,
            sync.models["main"].source,
            sync.project,
            ModuleInputSet::empty(&db),
        )
        .clone()
    };

    let mut feedback = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("stock_read", "m.n.level", None)
        .aux("feeder", "stock_read * 2", None)
        .aux("top", "m.mout", None)
        .build_datamodel();
    feedback.models[0]
        .variables
        .push(x_module_named("m", "mid", &[(".feeder", "m.feed")], None));
    feedback.models.extend([mid(), leaf()]);
    let graph = graph_of(&feedback);
    let position = |name: &str| {
        graph
            .runlist_flows
            .iter()
            .position(|v| v == name)
            .unwrap_or_else(|| panic!("{name} in {:?}", graph.runlist_flows))
    };
    assert!(
        position("feeder") < position("m"),
        "the instance runs after its input: {:?}",
        graph.runlist_flows
    );
    assert!(
        !graph.dt_dependencies[&Ident::new("stock_read")].contains(&Ident::new("m")),
        "a nested stock read orders nothing in the dt phase: {:?}",
        graph.dt_dependencies[&Ident::new("stock_read")]
    );
    let series = TestProject::from_datamodel(feedback).run_vm_expecting_success();
    let close = |a: &[f64], b: &[f64]| {
        a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9)
    };
    assert!(
        close(&series["feeder"], &[2.0, 2.2, 2.42, 2.662]),
        "{:?}",
        series["feeder"]
    );
    assert!(
        close(&series["m\u{00B7}feed"], &series["feeder"]),
        "the instance reads the current feeder: {:?} vs {:?}",
        series["m\u{00B7}feed"],
        series["feeder"]
    );

    let mut plain = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("stock_read", "m.n.level", None)
        .aux("aux_read", "m.n.out", None)
        .build_datamodel();
    plain.models[0]
        .variables
        .push(x_module_named("m", "mid", &[], None));
    plain.models.extend([mid(), leaf()]);
    assert_eq!(graph_of(&plain).runlist_initials, vec!["m", "stock_read"]);
}

/// A loop's exit port at a reader is the one output of the module the
/// reader reads, recorded by the causal-edge builder for an equation and for
/// an instance wired from it alike; two distinct outputs are ambiguous, and
/// the same output read twice is not.
#[test]
fn module_exit_selection_reads_the_recorded_outputs() {
    let mut project = TestProject::new("main")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("one", "m.a", None)
        .aux("two", "m.a + m.b", None)
        .aux("same_twice", "m.a * m.a", None)
        .aux("none", "one", None)
        .build_datamodel();
    project.models[0].variables.extend([
        x_module_named("m", "child", &[], None),
        x_module_named("wired", "sink", &[("m.b", "wired.input")], None),
        x_module_named(
            "wired_twice",
            "sink",
            &[("m.a", "wired_twice.input"), ("m.b", "wired_twice.other")],
            None,
        ),
    ]);
    project.models.push(x_model(
        "child",
        vec![x_aux("a", "1", None), x_aux("b", "2", None)],
    ));
    project.models.push(x_model(
        "sink",
        vec![
            x_aux("input", "0", None),
            x_aux("other", "0", None),
            x_aux("out", "input + other", None),
        ],
    ));
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let edges = model_causal_edges(&db, sync.models["main"].source, sync.project);
    let m = Ident::<Canonical>::new("m");
    let exit = |reader: &str| {
        edges
            .unique_module_output(reader, &m)
            .map(|p| p.to_string())
    };
    assert_eq!(exit("one"), Some("a".to_string()));
    assert_eq!(exit("two"), None);
    assert_eq!(exit("same_twice"), Some("a".to_string()));
    assert_eq!(exit("none"), None);
    assert_eq!(exit("wired"), Some("b".to_string()));
    assert_eq!(exit("wired_twice"), None);
    // The graph built from the edges answers identically, with no database.
    let graph = causal_graph_with_modules(&db, sync.models["main"].source, sync.project);
    assert_eq!(
        graph
            .unique_module_output(&Ident::new("wired"), &m)
            .map(|p| p.to_string()),
        Some("b".to_string())
    );
}

/// Resolving a module read costs the reader a dependency on the instances
/// it crosses and nothing else of the model: an unrelated edit to the owning
/// model, to an intermediate model or to the project re-executes no
/// dependency query of the unchanged reader.
#[test]
fn qualified_reads_are_firewalled_by_name() {
    let project_with = |owner_extra: bool, intermediate_extra: bool, project_extra: bool| {
        let mut project = TestProject::new("main")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux(
                "reader",
                "outer.inner.out + SMTH1(outer.inner.out, 2)",
                None,
            )
            .build_datamodel();
        project.models[0]
            .variables
            .push(x_module_named("outer", "middle", &[], None));
        if owner_extra {
            project.models[0]
                .variables
                .push(x_aux("unrelated_owner", "1", None));
        }
        let mut middle_vars = vec![x_module_named("inner", "leaf", &[], None)];
        if intermediate_extra {
            middle_vars.push(x_aux("unrelated_middle", "1", None));
        }
        project.models.push(x_model("middle", middle_vars));
        project
            .models
            .push(x_model("leaf", vec![x_aux("out", "TIME", None)]));
        if project_extra {
            project.models.push(x_model(
                "unrelated project model",
                vec![x_aux("value", "1", None)],
            ));
        }
        project
    };

    for (label, owner_extra, intermediate_extra, project_extra) in [
        ("owner", true, false, false),
        ("intermediate", false, true, false),
        ("project", false, false, true),
    ] {
        let mut probed = ProbedDb::new();
        let state1 = sync_from_datamodel_incremental(
            probed.db_mut(),
            &project_with(false, false, false),
            None,
        );
        let sync1 = state1.to_sync_result();
        let reader1 = sync1.models["main"].variables["reader"].source;
        let before = variable_direct_dependencies(
            probed.db(),
            reader1,
            sync1.project,
            ModuleInputSet::empty(probed.db()),
        )
        .clone();
        assert!(before.deps.iter().any(|dep| {
            dep.target
                .module_path
                .iter()
                .map(Ident::as_str)
                .eq(["outer", "inner"])
                && dep.target.variable.as_str() == "out"
        }));

        probed.reset();
        let state2 = sync_from_datamodel_incremental(
            probed.db_mut(),
            &project_with(owner_extra, intermediate_extra, project_extra),
            Some(&state1),
        );
        let sync2 = state2.to_sync_result();
        let reader2 = sync2.models["main"].variables["reader"].source;
        assert!(reader1 == reader2, "{label}: the reader keeps its input");
        let after = variable_direct_dependencies(
            probed.db(),
            reader2,
            sync2.project,
            ModuleInputSet::empty(probed.db()),
        );
        assert_eq!(&before, after, "{label}");
        let counts = probed.counts();
        assert_eq!(
            counts.get("variable_direct_dependencies"),
            None,
            "{label}: an unrelated structural edit may revalidate the per-name \
             projections but must not execute the unchanged reader's dependency \
             query: {counts:#?}"
        );
    }
}
