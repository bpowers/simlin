// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the dt-phase dependency-graph cycle-relation primitive and
//! the `#[cfg(test)]` Condition-2 SCC accessor (Task #15 / B1-design.md
//! §10b). Moved out of `db_tests.rs` -- alongside the production code in
//! `db_dep_graph.rs` -- purely to keep both `db.rs` and `db_tests.rs`
//! under the per-file line cap; the logic is byte-unchanged.

use super::*;
use crate::datamodel;
use crate::db::{SimlinDb, sync_from_datamodel};

// ── Task #15: Condition-2 dt-phase cycle introspection harness ──────────
//
// `dt_walk_successors` is the single shared dt-phase cycle-successor
// relation consumed by BOTH the production cycle detector
// (`compute_inner` inside `model_dependency_graph_impl`) AND the
// `#[cfg(test)]` SCC accessor (`dt_cycle_sccs`). Because there is exactly
// one definition of the relation, used twice, the introspection gate IS
// the engine's relation by construction (B1-design.md §10b: this ends the
// D1/D2/§19 "re-derive the relation and get it subtly wrong" footgun
// class). These tests pin its stated invariant (§10b(iii)): the successor
// set `compute_inner` iterates for every node kind --
//   Stock  => empty (dt-phase sink; db.rs stock early-return),
//   Module => empty (returns before `processing.insert`, so a module is
//             never on the DFS stack and can never carry a cycle),
//   Aux    => dt_deps filtered to known, non-stock targets
//             (module targets KEPT -- a module has no successors so
//             Tarjan cannot route a cycle through it; unknown and
//             stock-targeted deps dropped, matching `compute_inner`),
//   absent => empty (no panic; B1-primitive rigor).

/// Build a bare `VarInfo` for the pure-unit `dt_walk_successors` tests.
fn vi_for_test(is_stock: bool, is_module: bool, dt_deps: &[&str]) -> VarInfo {
    VarInfo {
        is_stock,
        is_module,
        dt_deps: dt_deps.iter().map(|s| (*s).to_string()).collect(),
        initial_deps: BTreeSet::new(),
    }
}

#[test]
fn dt_walk_successors_stock_is_dt_sink() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert("s".to_string(), vi_for_test(true, false, &["a", "b"]));
    vinfo.insert("a".to_string(), vi_for_test(false, false, &[]));
    vinfo.insert("b".to_string(), vi_for_test(false, false, &[]));
    // A Stock breaks the dt dependency chain: no cycle successors even
    // though its dt_deps are non-empty.
    assert!(dt_walk_successors(&vinfo, "s").is_empty());
}

#[test]
fn dt_walk_successors_module_has_no_cycle_successors() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert("m".to_string(), vi_for_test(false, true, &["a"]));
    vinfo.insert("a".to_string(), vi_for_test(false, false, &[]));
    // A Module returns before `processing.insert`, so it is never on the
    // DFS stack and can never carry a cycle: empty cycle-successor set.
    assert!(dt_walk_successors(&vinfo, "m").is_empty());
}

#[test]
fn dt_walk_successors_aux_filters_stock_and_unknown_keeps_module() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert(
        "x".to_string(),
        vi_for_test(false, false, &["aux2", "the_stock", "the_mod", "ghost"]),
    );
    vinfo.insert("aux2".to_string(), vi_for_test(false, false, &[]));
    vinfo.insert("the_stock".to_string(), vi_for_test(true, false, &[]));
    vinfo.insert("the_mod".to_string(), vi_for_test(false, true, &[]));
    // "ghost" is intentionally absent from var_info (an unknown dep).
    let succ = dt_walk_successors(&vinfo, "x");
    // Stock-targeted dep dropped (a stock breaks the dt chain), unknown
    // dep dropped, module-targeted dep KEPT (a module node has no
    // successors so Tarjan cannot route a cycle through it -- this
    // matches `compute_inner`, whose `!dep_info.is_module` guard only
    // controls transitive absorption, not iteration).
    assert_eq!(succ, vec!["aux2", "the_mod"]);
}

#[test]
fn dt_walk_successors_absent_name_is_empty() {
    let vinfo: HashMap<String, VarInfo> = HashMap::new();
    // B1-primitive rigor: a malformed/absent var_info entry must not
    // panic; it yields no successors.
    assert!(dt_walk_successors(&vinfo, "nope").is_empty());
}

#[test]
fn dt_walk_successors_order_is_btreeset_sorted() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert(
        "x".to_string(),
        vi_for_test(false, false, &["zeta", "alpha", "mid"]),
    );
    vinfo.insert("zeta".to_string(), vi_for_test(false, false, &[]));
    vinfo.insert("alpha".to_string(), vi_for_test(false, false, &[]));
    vinfo.insert("mid".to_string(), vi_for_test(false, false, &[]));
    // dt_deps is a BTreeSet; the successor list preserves its sorted
    // iteration order. This is what makes the cycle-detection
    // first-back-edge and the SCC adjacency byte-stable across runs.
    assert_eq!(
        dt_walk_successors(&vinfo, "x"),
        vec!["alpha", "mid", "zeta"]
    );
}

fn aux_var(ident: &str, eq: &str) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(eq.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn single_model_project(vars: Vec<datamodel::Variable>) -> datamodel::Project {
    datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vars,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

#[test]
fn dt_cycle_sccs_clean_dag_has_no_sccs_or_self_loops() {
    let db = SimlinDb::default();
    let project = single_model_project(vec![
        aux_var("rate", "0.1"),
        aux_var("growth", "rate * 100"),
    ]);
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    // `dt_cycle_sccs_engine_consistent` STOPs (panics) if the
    // instrumented dt-SCC set diverges from the engine's REAL
    // CircularDependency flagging -- so reaching the asserts already
    // proves the §10b step-4 cross-assert held (clean DAG => no cycle
    // reported AND no CircularDependency raised).
    let sccs = dt_cycle_sccs_engine_consistent(&db, model, result.project);
    assert!(sccs.multi.is_empty(), "clean DAG has no >=2 SCCs");
    assert!(sccs.self_loops.is_empty(), "clean DAG has no self-loops");
}

#[test]
fn dt_cycle_sccs_two_node_cycle_matches_circular_diagnostic() {
    let db = SimlinDb::default();
    let project = single_model_project(vec![aux_var("a", "b"), aux_var("b", "a")]);
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    // Consumes the consistency-checked accessor: the §10b step-4
    // cross-assert (reported SCC <=> engine CircularDependency) is
    // enforced INSIDE the accessor; reaching here means it held.
    let sccs = dt_cycle_sccs_engine_consistent(&db, model, result.project);
    let expected: Vec<BTreeSet<crate::common::Ident<crate::common::Canonical>>> = vec![
        [
            crate::common::Ident::new("a"),
            crate::common::Ident::new("b"),
        ]
        .into_iter()
        .collect(),
    ];
    assert_eq!(sccs.multi, expected);
    assert!(sccs.self_loops.is_empty());
}

#[test]
fn dt_cycle_sccs_self_reference_is_a_self_loop() {
    let db = SimlinDb::default();
    let project = single_model_project(vec![aux_var("a", "a + 1")]);
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    let sccs = dt_cycle_sccs_engine_consistent(&db, model, result.project);
    // A direct self-reference is a size-1 SCC that Tarjan does NOT
    // surface as `multi`; it is captured separately as a self-loop.
    // (The consistency cross-assert -- self-loop <=> CircularDependency
    // -- was already enforced inside the accessor.)
    assert!(sccs.multi.is_empty(), "a self-loop is not a >=2 SCC");
    assert!(
        sccs.self_loops.contains(&crate::common::Ident::new("a")),
        "direct self-reference must be reported as a self-loop"
    );
}

#[test]
fn dt_cycle_sccs_is_byte_stable_across_runs() {
    // §10b step 5: the harness output must be byte-stable across runs
    // (sorted Vec/BTreeSet, no HashMap-iteration nondeterminism leaking
    // out) so a diff is meaningful and the reviewer's ACCEPT is
    // reproducible. A regression that returned raw Tarjan order would
    // fail this.
    let db = SimlinDb::default();
    let project = single_model_project(vec![
        aux_var("a", "b"),
        aux_var("b", "a"),
        aux_var("c", "c + 1"),
        aux_var("d", "0"),
    ]);
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    let first = dt_cycle_sccs(&db, model, result.project);
    let second = dt_cycle_sccs(&db, model, result.project);
    assert_eq!(first, second, "dt_cycle_sccs must be byte-stable");
}

// §10b step-4 footgun-proofing as a tested invariant (functional core):
// the pure consistency predicate must flag a divergence between the
// instrumented dt-SCC set and the engine's real CircularDependency
// flagging in BOTH directions (an invented false-positive cycle, and a
// missed/false-negative cycle) and must accept every consistent pairing.

fn multi_ab() -> Vec<BTreeSet<crate::common::Ident<crate::common::Canonical>>> {
    vec![
        [
            crate::common::Ident::new("a"),
            crate::common::Ident::new("b"),
        ]
        .into_iter()
        .collect(),
    ]
}

fn self_loop_a() -> BTreeSet<crate::common::Ident<crate::common::Canonical>> {
    let mut s = BTreeSet::new();
    s.insert(crate::common::Ident::new("a"));
    s
}

#[test]
fn consistency_violation_none_when_both_clean() {
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, false).is_none());
}

#[test]
fn consistency_violation_none_when_multi_and_circular() {
    let sccs = DtCycleSccs {
        multi: multi_ab(),
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, true).is_none());
}

#[test]
fn consistency_violation_none_when_self_loop_and_circular() {
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: self_loop_a(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, true).is_none());
}

#[test]
fn consistency_violation_some_when_invented_cycle_not_flagged() {
    // Instrumentation reports a cycle the engine does NOT flag =>
    // the relation is mis-derived => STOP (do not gate).
    let sccs = DtCycleSccs {
        multi: multi_ab(),
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, false).is_some());
}

#[test]
fn consistency_violation_some_when_missed_cycle_flagged() {
    // Engine raises CircularDependency but the instrumentation reports
    // NO cycle => a missed cycle (or the init-acyclic premise broke)
    // => STOP.
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, true).is_some());
}

#[test]
fn consistency_violation_some_when_self_loop_not_flagged() {
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: self_loop_a(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, false).is_some());
}
