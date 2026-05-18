// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the dt-phase dependency-graph cycle-relation primitive and
//! the `#[cfg(test)]` SCC accessor. Live in their own file alongside the
//! production code in `db_dep_graph.rs` to keep both `db.rs` and
//! `db_tests.rs` under the per-file line cap.

use super::*;
use crate::datamodel;
use crate::db::{SimlinDb, sync_from_datamodel};
use crate::test_common::TestProject;

// ── dt-phase cycle introspection ────────────────────────────────────────
//
// `dt_walk_successors` is the single shared dt-phase cycle-successor
// relation consumed by both the production cycle detector
// (`compute_inner` inside `model_dependency_graph_impl`) and the
// `#[cfg(test)]` SCC accessor (`dt_cycle_sccs`). Because there is exactly
// one definition of the relation, used twice, the introspection accessor
// is the engine's relation by construction -- no re-derivation can drift.
// These tests pin its invariant: the successor set `compute_inner`
// iterates for every node kind --
//   Stock  => empty (dt-phase sink; db.rs stock early-return),
//   Module => empty (returns before `processing.insert`, so a module is
//             never on the DFS stack and can never carry a cycle),
//   Aux    => dt_deps filtered to known, non-stock targets
//             (module targets KEPT -- a module has no successors so
//             Tarjan cannot route a cycle through it; unknown and
//             stock-targeted deps dropped, matching `compute_inner`),
//   absent => empty (no panic).

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
    // A malformed/absent var_info entry must not panic; it yields no
    // successors.
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

// ── init-phase cycle relation (`init_walk_successors`) ──────────────────
//
// `init_walk_successors` is the single shared init-phase cycle-successor
// relation, the exact analogue of `dt_walk_successors` for the init
// phase. It is consumed by both the production cycle detector
// (`compute_inner` inside `model_dependency_graph_impl`, init branch)
// and the init-phase per-element recurrence resolution. These tests pin
// its invariant per node kind:
//   Module => empty (the module early-return in `compute_inner` applies
//             to BOTH phases -- a module is never on the DFS stack so it
//             can never carry a cycle in either phase),
//   Stock  => initial_deps filtered to known vars -- a stock is NOT an
//             init sink (the dt stock sink is `!is_initial`-gated; a
//             stock is a valid init-relation node, so its init deps and
//             stock-targeted init deps are KEPT),
//   Aux    => initial_deps filtered to known vars (unknown deps dropped,
//             matching the inlined `compute_inner` init logic),
//   absent => empty (no panic).

/// Build a bare `VarInfo` carrying `initial_deps` for the pure-unit
/// `init_walk_successors` tests (the dt-only helper `vi_for_test` leaves
/// `initial_deps` empty).
fn vi_init_for_test(is_stock: bool, is_module: bool, initial_deps: &[&str]) -> VarInfo {
    VarInfo {
        is_stock,
        is_module,
        dt_deps: BTreeSet::new(),
        initial_deps: initial_deps.iter().map(|s| (*s).to_string()).collect(),
    }
}

#[test]
fn init_walk_successors_module_has_no_cycle_successors() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert("m".to_string(), vi_init_for_test(false, true, &["a"]));
    vinfo.insert("a".to_string(), vi_init_for_test(false, false, &[]));
    // The module early-return in `compute_inner` fires before
    // `processing.insert` in BOTH phases, so a module is never on the
    // DFS stack and can never carry a cycle in the init phase either:
    // empty cycle-successor set (mirrors `dt_walk_successors`).
    assert!(init_walk_successors(&vinfo, "m").is_empty());
}

#[test]
fn init_walk_successors_stock_is_not_an_init_sink() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert("s".to_string(), vi_init_for_test(true, false, &["s", "a"]));
    vinfo.insert("a".to_string(), vi_init_for_test(false, false, &[]));
    // A Stock is NOT an init-phase sink: the dt stock sink in
    // `compute_inner` is `!is_initial`-gated, so in the init phase a
    // stock's `initial_deps` ARE its cycle successors. A stock whose
    // init equation references itself (`s` in its own init deps) is a
    // genuine init self-loop, so `s` MUST appear in its own successor
    // set (this is exactly what an init-phase recurrence behind a stock
    // relies on).
    assert_eq!(init_walk_successors(&vinfo, "s"), vec!["a", "s"]);
}

#[test]
fn init_walk_successors_keeps_stock_targeted_deps() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert(
        "x".to_string(),
        vi_init_for_test(false, false, &["the_stock", "aux2"]),
    );
    vinfo.insert("the_stock".to_string(), vi_init_for_test(true, false, &[]));
    vinfo.insert("aux2".to_string(), vi_init_for_test(false, false, &[]));
    // Unlike `dt_walk_successors` (which drops stock-targeted deps
    // because a stock breaks the dt chain), the init relation KEEPS a
    // stock-targeted dep: a stock's initial value is a real init-phase
    // dependency. NO stock filter on the deps.
    assert_eq!(init_walk_successors(&vinfo, "x"), vec!["aux2", "the_stock"]);
}

#[test]
fn init_walk_successors_filters_unknown_deps() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert(
        "x".to_string(),
        vi_init_for_test(false, false, &["known", "ghost"]),
    );
    vinfo.insert("known".to_string(), vi_init_for_test(false, false, &[]));
    // "ghost" is intentionally absent from var_info.
    // This is exactly the inlined `compute_inner` init semantics
    // (`info.initial_deps.iter().filter(|dep|
    // var_info.contains_key(dep))`): unknown deps dropped, no other
    // filter.
    assert_eq!(init_walk_successors(&vinfo, "x"), vec!["known"]);
}

#[test]
fn init_walk_successors_absent_name_is_empty() {
    let vinfo: HashMap<String, VarInfo> = HashMap::new();
    // A malformed/absent var_info entry must not panic; it yields no
    // successors (mirrors `dt_walk_successors`; `compute_inner` likewise
    // early-returns `Ok(())` for an unknown name).
    assert!(init_walk_successors(&vinfo, "nope").is_empty());
}

#[test]
fn init_walk_successors_order_is_btreeset_sorted() {
    let mut vinfo: HashMap<String, VarInfo> = HashMap::new();
    vinfo.insert(
        "x".to_string(),
        vi_init_for_test(false, false, &["zeta", "alpha", "mid"]),
    );
    vinfo.insert("zeta".to_string(), vi_init_for_test(false, false, &[]));
    vinfo.insert("alpha".to_string(), vi_init_for_test(false, false, &[]));
    vinfo.insert("mid".to_string(), vi_init_for_test(false, false, &[]));
    // initial_deps is a BTreeSet; the successor list preserves its
    // sorted iteration order, so init cycle detection and the init SCC
    // adjacency are byte-stable across runs (same discipline as
    // `dt_walk_successors`).
    assert_eq!(
        init_walk_successors(&vinfo, "x"),
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
    // `dt_cycle_sccs_engine_consistent` panics if the instrumented
    // dt-SCC set diverges from the engine's real CircularDependency
    // flagging -- so reaching the asserts already proves the cross-check
    // held (clean DAG => no cycle reported AND no CircularDependency
    // raised).
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
    // Consumes the consistency-checked accessor: the cross-check
    // (reported SCC <=> engine CircularDependency) is enforced inside
    // the accessor; reaching here means it held.
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
    // (The consistency cross-check -- self-loop <=> CircularDependency
    // -- was already enforced inside the accessor.)
    assert!(sccs.multi.is_empty(), "a self-loop is not a >=2 SCC");
    assert!(
        sccs.self_loops.contains(&crate::common::Ident::new("a")),
        "direct self-reference must be reported as a self-loop"
    );
}

#[test]
fn dt_cycle_sccs_is_byte_stable_across_runs() {
    // The accessor output must be byte-stable across runs (sorted
    // Vec/BTreeSet, no HashMap-iteration nondeterminism leaking out) so
    // a diff is meaningful. A regression that returned raw Tarjan order
    // would fail this.
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

#[test]
fn dt_cycle_sccs_resolved_self_recurrence_has_no_circular() {
    // The new invariant's headline case: a single-variable
    // self-recurrence (`ecc[t1]=1; ecc[t2]=ecc[t1]+1; ecc[t3]=ecc[t2]+1`)
    // is an instrumented dt self-loop (the whole-variable dt relation has
    // a `ecc -> ecc` self-edge), yet its induced element graph is
    // element-acyclic and element-sourceable, so the engine resolves it
    // and does NOT raise `CircularDependency`. The OLD XNOR invariant
    // would have panicked here ("instrumented self-loop but no
    // CircularDependency"); the re-pointed invariant treats this as
    // consistent because the instrumented SCC `{ecc}` is in
    // `resolved_sccs`. Reaching the asserts proves the cross-check held.
    let db = SimlinDb::default();
    let project = TestProject::new("dt_resolved_self_recurrence")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ecc[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        );
    let dm = project.build_datamodel();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;
    let sccs = dt_cycle_sccs_engine_consistent(&db, model, result.project);
    // The whole-variable dt relation flags `ecc` as a self-loop...
    assert!(
        sccs.self_loops.contains(&crate::common::Ident::new("ecc")),
        "the single-variable self-recurrence must still be an \
         instrumented dt self-loop (the whole-variable relation is \
         unchanged)"
    );
    assert!(
        sccs.multi.is_empty(),
        "no >=2 SCC for a single-variable case"
    );
    // ...but the engine resolves it (no CircularDependency). Confirm the
    // diagnostic is absent and a `ResolvedScc` for `{ecc}` was emitted.
    let dep_graph = crate::db::model_dependency_graph(&db, model, result.project);
    assert!(
        !dep_graph.has_cycle,
        "an element-acyclic single-variable self-recurrence must NOT set \
         has_cycle"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one ResolvedScc for the resolved self-recurrence"
    );
    assert_eq!(
        dep_graph.resolved_sccs[0].members,
        [crate::common::Ident::new("ecc")]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
}

#[test]
fn dt_cycle_sccs_genuine_two_cycle_still_circular() {
    // `a=b+1; b=a+1`: a genuine scalar 2-cycle / multi-variable SCC.
    // Phase 1 does not resolve multi-variable SCCs, so it is absent from
    // `resolved_sccs` and the engine raises `CircularDependency` -- the
    // re-pointed invariant treats "instrumented multi-SCC, not resolved,
    // engine raises CircularDependency" as consistent (unchanged genuine-
    // cycle behavior). Reaching the asserts proves the cross-check held.
    let db = SimlinDb::default();
    let project = single_model_project(vec![aux_var("a", "b + 1"), aux_var("b", "a + 1")]);
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    let sccs = dt_cycle_sccs_engine_consistent(&db, model, result.project);
    let expected: Vec<BTreeSet<crate::common::Ident<crate::common::Canonical>>> = vec![
        [
            crate::common::Ident::new("a"),
            crate::common::Ident::new("b"),
        ]
        .into_iter()
        .collect(),
    ];
    assert_eq!(
        sccs.multi, expected,
        "the 2-cycle is an instrumented >=2 SCC"
    );
    assert!(sccs.self_loops.is_empty());
    let dep_graph = crate::db::model_dependency_graph(&db, model, result.project);
    assert!(
        dep_graph.has_cycle,
        "a genuine 2-cycle still sets has_cycle (Phase 1 does not resolve \
         multi-variable SCCs)"
    );
    assert!(
        dep_graph.resolved_sccs.is_empty(),
        "a genuine 2-cycle resolves nothing"
    );
}

// The pure consistency predicate as a tested invariant (functional
// core), re-pointed to the element-level invariant: an instrumented SCC
// whose induced element graph is acyclic AND element-sourceable is in
// `resolved_sccs` and the engine does NOT raise `CircularDependency`; an
// instrumented SCC that is element-cyclic OR not element-sourceable is
// absent from `resolved_sccs` and the engine DOES raise
// `CircularDependency`. The predicate must accept every consistent
// pairing and flag every divergence: a resolved SCC the engine still
// flagged, an unresolved instrumented SCC the engine did NOT flag (a
// missed cycle), and a `ResolvedScc` whose members the instrumentation
// never surfaced (the shared dt relation drifted from the refinement).

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

/// A `ResolvedScc` for the single-variable self-recurrence `{a}` (the
/// element-acyclic resolved shape Phase 1 produces).
fn resolved_a() -> Vec<crate::db::ResolvedScc> {
    vec![crate::db::ResolvedScc {
        members: self_loop_a(),
        element_order: vec![(crate::common::Ident::new("a"), 0usize)],
        phase: crate::db::SccPhase::Dt,
    }]
}

#[test]
fn consistency_violation_none_when_both_clean() {
    // No instrumented SCC, nothing resolved, no diagnostic: consistent.
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &[], false).is_none());
}

#[test]
fn consistency_violation_none_when_multi_and_circular() {
    // A multi-variable SCC is not resolved in Phase 1: absent from
    // `resolved_sccs` AND the engine raises `CircularDependency` =>
    // consistent (element-cyclic/unresolved => diagnostic).
    let sccs = DtCycleSccs {
        multi: multi_ab(),
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &[], true).is_none());
}

#[test]
fn consistency_violation_none_when_unresolved_self_loop_and_circular() {
    // An instrumented self-loop NOT in `resolved_sccs` (genuine
    // same-element self-cycle or not element-sourceable) + the engine
    // raises `CircularDependency` => consistent.
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: self_loop_a(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &[], true).is_none());
}

#[test]
fn consistency_violation_none_when_resolved_self_loop_and_no_circular() {
    // The new invariant's positive case: an instrumented self-loop that
    // IS in `resolved_sccs` (element-acyclic single-variable
    // self-recurrence) AND the engine does NOT raise
    // `CircularDependency` => consistent (this is exactly the case the
    // OLD XNOR invariant wrongly rejected).
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: self_loop_a(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &resolved_a(), false).is_none());
}

#[test]
fn consistency_violation_some_when_invented_cycle_not_flagged() {
    // Instrumentation reports a multi-SCC the engine does NOT flag and
    // did NOT resolve => the relation is mis-derived => STOP.
    let sccs = DtCycleSccs {
        multi: multi_ab(),
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &[], false).is_some());
}

#[test]
fn consistency_violation_some_when_missed_cycle_flagged() {
    // Engine raises CircularDependency but the instrumentation reports
    // NO cycle => a missed cycle => STOP.
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &[], true).is_some());
}

#[test]
fn consistency_violation_some_when_unresolved_self_loop_not_flagged() {
    // An instrumented self-loop that is NOT resolved AND the engine does
    // NOT raise `CircularDependency` => the instrumented cycle went
    // neither resolved nor flagged => a missed cycle => STOP.
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: self_loop_a(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &[], false).is_some());
}

#[test]
fn consistency_violation_some_when_resolved_self_loop_still_flagged() {
    // The instrumented self-loop IS in `resolved_sccs`, yet the engine
    // ALSO raised `CircularDependency` for it => the resolution verdict
    // and the diagnostic disagree on the SAME compiled model => STOP.
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: self_loop_a(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &resolved_a(), true).is_some());
}

#[test]
fn consistency_violation_some_when_resolved_scc_not_instrumented() {
    // A `ResolvedScc` whose members the dt instrumentation never
    // surfaced as an SCC => the refinement resolved something the shared
    // dt relation did not even see as a cycle => the two relations
    // drifted => STOP (the whole point of the cross-check).
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &resolved_a(), false).is_some());
}

// `array_producing_vars` membership over four cases. Both positive cases
// are independently required: each defends `array_producing_vars` against
// a different way of under-counting.
//   case-1 (POS) top-level-scalar `= VECTOR ELM MAP(...)` -- the shape
//     the scalar path does NOT hoist (`App` lives in `AssignCurr`);
//     guards against an impl narrowed to only the compiler hoist-set.
//   case-2 (POS) array-producing builtin nested so the compiler hoists
//     it into a separate `AssignTemp` (`App` lives ONLY in the hoisted
//     `AssignTemp`, NOT in `AssignCurr`/main); guards against an impl
//     that sources only `AssignCurr` and drops hoisted temps.
//   case-3 (NEG) plain scalar -- soundness.
//   case-4 (NEG) merely references the VEM var -- its OWN lowered `Expr`
//     has only a `Var` slot read, no `App`; soundness.
// VEM shapes are the known-good `tests/compiler_vector.rs` fixtures
// (`vector_elm_map(source[*], offsets[*])` and the nested
// `max(vector_elm_map(...), 15)` hoisting form) so the model compiles and
// the only thing under test is the membership set.
#[test]
fn array_producing_vars_flags_exactly_the_two_positive_cases() {
    let project = TestProject::new("array_producing_vars_fixture")
        .indexed_dimension("D", 3)
        .array_with_ranges("source[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("offsets[D]", vec![("1", "0"), ("2", "2"), ("3", "1")])
        // case-1 POSITIVE: top-level array-producing (scalar path does
        // NOT hoist; `App` in `AssignCurr`).
        .aux("case1_vem", "vector_elm_map(source[*], offsets[*])", None)
        // case-2 POSITIVE: VEM nested inside `max(...)` -> the compiler
        // hoists the VEM into a separate `AssignTemp`; `App` lives ONLY
        // in the hoisted temp, `AssignCurr` reads it back.
        .array_aux(
            "case2_hoisted[D]",
            "max(vector_elm_map(source[*], offsets[*]), 15)",
        )
        // case-3 NEGATIVE: plain scalar, no array-producing builtin.
        .aux("case3_plain", "1 + 2", None)
        // case-4 NEGATIVE: merely references the VEM var element-wise;
        // case4_ref's OWN lowered Expr is `Op2(+, Var(off), 1)` -- a slot
        // read another var filled, NOT an inline array-producing `App`.
        .array_aux("case4_ref[D]", "case1_vem + 1");
    let dm = project.build_datamodel();

    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let got = array_producing_vars(&db, model, result.project);

    let expected: BTreeSet<crate::common::Ident<crate::common::Canonical>> = [
        crate::common::Ident::new("case1_vem"),
        crate::common::Ident::new("case2_hoisted"),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        got, expected,
        "array_producing_vars must be EXACTLY {{case1_vem, case2_hoisted}}: \
         both positives flagged (top-level-scalar AND hoisted-AssignTemp), \
         both negatives not"
    );

    // Lowered-shape regression guards. Sourced from the same engine
    // per-variable production lowering `array_producing_vars` consumes
    // (`var_noninitial_lowered_exprs` ->
    // `crate::db_var_fragment::lower_var_fragment`), partitioned by
    // top-level element kind, then the same production predicate
    // (`exprs_contain_array_producing_builtin`) applied per partition --
    // no re-implementation of the array-producing recursion.
    use crate::compiler::Expr;
    let vem_in = |exprs: &[Expr], want_temp: bool| -> bool {
        let part: Vec<Expr> = exprs
            .iter()
            .filter(|e| {
                if want_temp {
                    matches!(e, Expr::AssignTemp(..))
                } else {
                    matches!(e, Expr::AssignCurr(..))
                }
            })
            .cloned()
            .collect();
        crate::compiler::exprs_contain_array_producing_builtin(&part)
    };

    // case-1 (inverse of case-2): the array-producing `App` must live
    // inside an `AssignCurr`/main element AND NOT inside any hoisted
    // `AssignTemp`. The bare-scalar declaration lowers to
    // `[AssignCurr(off, App(VEM,…))]` -- the scalar path does NOT hoist a
    // top-level array-producing builtin -- so this is distinct from
    // case-2 by construction. If the `App` is instead in a hoisted
    // `AssignTemp`, the scalar lowering changed unexpectedly: surface it
    // immediately, do NOT paper over.
    let c1 = var_noninitial_lowered_exprs(&db, model, result.project, "case1_vem");
    let c1_in_curr = vem_in(&c1, false);
    let c1_in_temp = vem_in(&c1, true);
    assert!(
        c1_in_curr && !c1_in_temp,
        "case-1 shape (inverse of case-2): the VECTOR ELM MAP App must \
         be in an AssignCurr/main element AND NOT in any hoisted \
         AssignTemp (in_curr={c1_in_curr}, in_temp={c1_in_temp}, \
         elems={}). in_temp=true means the scalar lowering changed \
         unexpectedly -- surface immediately, do not work around.",
        c1.len()
    );

    // case-2: the array-producing `App` must live ONLY in the hoisted
    // `AssignTemp`, never directly in `AssignCurr` (the `AssignCurr`
    // reads the temp back).
    let c2 = var_noninitial_lowered_exprs(&db, model, result.project, "case2_hoisted");
    let c2_in_curr = vem_in(&c2, false);
    let c2_in_temp = vem_in(&c2, true);
    assert!(
        c2_in_temp && !c2_in_curr,
        "case-2 shape: the VECTOR ELM MAP App must be ONLY in a hoisted \
         AssignTemp and NEVER in AssignCurr \
         (in_curr={c2_in_curr}, in_temp={c2_in_temp}, elems={})",
        c2.len()
    );
}

// ── var_phase_lowered_exprs_prod (production lowering accessor) ──────────
//
// The production, non-panicking sibling of `var_noninitial_lowered_exprs`:
// `Some(per-element Vec<Expr>)` for a real-`SourceVariable` (so the
// element-cycle refinement can source the per-element relation), `None`
// (loud-safe -- caller keeps `CircularDependency`) when it cannot be
// element-sourced. A name absent from `model.variables` must return
// `None`, NOT panic: production code cannot panic, and Phase 1 does not
// parent-source (Phase 3 does).

#[test]
fn var_phase_lowered_exprs_prod_some_for_real_arrayed_var() {
    use crate::compiler::Expr;
    use crate::db::SccPhase;

    // A simple arrayed real-SourceVariable: 3 declared elements, each a
    // plain constant. The dt-phase lowering is one `AssignCurr` slot per
    // element, in declared `SubscriptIterator` order.
    let project = TestProject::new("vpl_prod_fixture")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges("arr[t]", vec![("t1", "1"), ("t2", "2"), ("t3", "3")]);
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let got = var_phase_lowered_exprs_prod(&db, model, result.project, "arr", SccPhase::Dt)
        .expect("a real arrayed SourceVariable must be element-sourceable");
    let assign_curr = got
        .iter()
        .filter(|e| matches!(e, Expr::AssignCurr(..)))
        .count();
    assert_eq!(
        assign_curr, 3,
        "one Expr::AssignCurr slot per declared element (declared order); \
         got {got:#?}"
    );
}

#[test]
fn var_phase_lowered_exprs_prod_none_for_absent_var_no_panic() {
    use crate::db::SccPhase;

    let project = TestProject::new("vpl_prod_absent")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges("arr[t]", vec![("t1", "1"), ("t2", "2"), ("t3", "3")]);
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    // A name with no `SourceVariable` must return `None` (Phase 1 does
    // not parent-source) -- and crucially must NOT panic the way the
    // `#[cfg(test)]` `var_noninitial_lowered_exprs` does, because this is
    // production code reachable from the cycle gate.
    assert!(
        var_phase_lowered_exprs_prod(
            &db,
            model,
            result.project,
            "definitely_not_a_var",
            SccPhase::Dt
        )
        .is_none(),
        "an absent variable must return None (loud-safe), never panic"
    );
}

// ── Per-element dt SCC resolution (the cycle-gate refinement) ───────────
//
// `resolve_dt_recurrence_sccs` identifies the offending dt SCC(s) over
// the same shared `dt_walk_successors` relation the engine uses, refines
// each into an exact `(member, element-offset)` graph from the engine's
// own production-lowered per-element exprs, and renders a verdict:
// element-acyclic + element-sourceable single-variable self-recurrence =>
// resolved (`ResolvedScc`, no `CircularDependency`); a genuine element
// cycle (same-element self-loop or multi-var element 2-cycle) or a not-
// element-sourceable / multi-variable SCC => unresolved (loud-safe, keep
// `CircularDependency`).

fn ecc(i: usize) -> (crate::common::Ident<crate::common::Canonical>, usize) {
    (crate::common::Ident::new("ecc"), i)
}

#[test]
fn resolve_dt_forward_recurrence_is_resolved_in_declared_order() {
    use crate::db::SccPhase;

    // `ecc[t1]=1; ecc[t2]=ecc[t1]+1; ecc[t3]=ecc[t2]+1`: a single-variable
    // self-recurrence whose induced element graph
    // (ecc,0)->(ecc,1)->(ecc,2) is acyclic and well-founded.
    let project = TestProject::new("fwd_recurrence")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ecc[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        );
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let res = resolve_dt_recurrence_sccs(&db, model, result.project);
    assert!(
        !res.has_unresolved,
        "the single-variable self-recurrence is element-acyclic and \
         element-sourceable: there must be NO unresolved SCC"
    );
    assert_eq!(res.resolved.len(), 1, "exactly one resolved SCC (ecc)");
    let scc = &res.resolved[0];
    assert_eq!(scc.phase, SccPhase::Dt);
    assert_eq!(
        scc.members,
        [crate::common::Ident::new("ecc")]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    // Deterministic per-element topological order: t1 has no in-SCC
    // reader edges; t2 reads t1; t3 reads t2.
    assert_eq!(
        scc.element_order,
        vec![ecc(0), ecc(1), ecc(2)],
        "element_order must be the per-element topological order"
    );
}

#[test]
fn resolve_dt_same_element_self_cycle_is_unresolved() {
    // `x[dimA]=x[dimA]+1`: every element reads ITSELF => element
    // self-loop => element-cyclic => unresolved (AC1.5/AC4.2). Must stay
    // rejected by construction.
    let project = TestProject::new("same_elem_self")
        .named_dimension("dima", &["a1", "a2"])
        .array_aux("x[dima]", "x[dima] + 1");
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let res = resolve_dt_recurrence_sccs(&db, model, result.project);
    assert!(
        res.has_unresolved,
        "x[dimA]=x[dimA]+1 is a genuine element self-loop and MUST be \
         unresolved (AC4.2 -- a real cycle stays rejected)"
    );
    assert!(
        res.resolved.is_empty(),
        "a genuine element self-loop yields no ResolvedScc"
    );
}

#[test]
fn resolve_dt_scalar_two_cycle_is_unresolved() {
    // `a=b+1; b=a+1`: a scalar 2-cycle / multi-variable SCC. Phase 1
    // routes multi-variable SCCs to unresolved (Phase 2 resolves them),
    // and it is also a genuine element 2-cycle => unresolved (AC4.1).
    let project = single_model_project(vec![aux_var("a", "b + 1"), aux_var("b", "a + 1")]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let res = resolve_dt_recurrence_sccs(&db, model, result.project);
    assert!(
        res.has_unresolved,
        "a=b+1;b=a+1 is a genuine 2-cycle and MUST be unresolved (AC4.1)"
    );
    assert!(
        res.resolved.is_empty(),
        "a genuine 2-cycle yields no ResolvedScc"
    );
}

#[test]
fn resolve_dt_acyclic_model_has_no_sccs() {
    // The AC1.3 happy path: a clean DAG has no offending dt SCC, so the
    // refinement does zero work and reports nothing (no resolved, none
    // unresolved).
    let project = single_model_project(vec![
        aux_var("rate", "0.1"),
        aux_var("growth", "rate * 100"),
    ]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let res = resolve_dt_recurrence_sccs(&db, model, result.project);
    assert!(!res.has_unresolved, "a clean DAG has no unresolved SCC");
    assert!(
        res.resolved.is_empty(),
        "a clean DAG has no resolved SCC either (zero extra work)"
    );
}

#[test]
fn resolve_dt_recurrence_sccs_is_byte_stable_across_runs() {
    // The emitted per-element run order must be byte-identical across
    // repeated computations on fresh databases (AC1.4 discipline): the
    // element graph reuses the sorted Tarjan + BTreeSet ordering, so a
    // regression that leaked HashMap iteration order would fail here.
    let build = || {
        let project = TestProject::new("byte_stable_fwd")
            .named_dimension("t", &["t1", "t2", "t3"])
            .array_with_ranges(
                "ecc[t]",
                vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
            );
        let dm = project.build_datamodel();
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &dm);
        let model = result.models["main"].source;
        resolve_dt_recurrence_sccs(&db, model, result.project).resolved
    };
    assert_eq!(
        build(),
        build(),
        "resolved_sccs (members + element_order) must be byte-stable \
         across repeated compiles"
    );
}

#[test]
fn model_dependency_graph_resolved_sccs_is_byte_stable_across_runs() {
    // AC1.4 at the PRODUCTION-PAYLOAD level. The sibling
    // `resolve_dt_recurrence_sccs_is_byte_stable_across_runs` pins the
    // internal Task 5 builder (`DtSccResolution.resolved`); this pins the
    // *emitted* `model_dependency_graph().resolved_sccs` -- the value that
    // actually rides on the salsa-tracked `ModelDepGraphResult` and drives
    // downstream consumers -- proving `ResolvedScc.element_order` (and
    // `members`) inherits the determinism discipline end-to-end through
    // the production query, not merely inside the builder. A regression
    // that leaked HashMap iteration order anywhere on the
    // identification -> refinement -> emission path would fail here even
    // if the isolated builder stayed stable.
    //
    // Single-variable self-recurrence (`ecc[t1]=1; ecc[t2]=ecc[t1]+1;
    // ecc[t3]=ecc[t2]+1`): an instrumented dt self-loop whose induced
    // element graph (ecc,0)->(ecc,1)->(ecc,2) is acyclic and
    // element-sourceable, so it is emitted as exactly one `ResolvedScc`.
    let resolved = || {
        let project = TestProject::new("mdg_byte_stable_fwd")
            .named_dimension("t", &["t1", "t2", "t3"])
            .array_with_ranges(
                "ecc[t]",
                vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
            );
        let dm = project.build_datamodel();
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &dm);
        let model = result.models["main"].source;
        crate::db::model_dependency_graph(&db, model, result.project)
            .resolved_sccs
            .clone()
    };
    let first = resolved();
    let second = resolved();
    // The emitted payload is non-empty for the resolved self-recurrence;
    // assert that explicitly so the byte-stability check below cannot pass
    // vacuously on two empty vectors (which would defeat the obligation).
    assert_eq!(
        first.len(),
        1,
        "the resolved self-recurrence must emit exactly one ResolvedScc \
         (a vacuous empty-vs-empty comparison would not prove AC1.4)"
    );
    assert_eq!(
        first.first().map(|scc| scc.element_order.clone()),
        Some(vec![ecc(0), ecc(1), ecc(2)]),
        "the emitted per-element run order must be the per-element \
         topological order"
    );
    assert_eq!(
        first, second,
        "the emitted model_dependency_graph().resolved_sccs (members + \
         element_order + phase) must be byte-identical across repeated \
         compiles on fresh databases"
    );

    // AC1.3 happy path unaffected: an acyclic CONTROL model has no
    // offending dt SCC, so the emitted payload is empty -- and it is empty
    // BOTH times (the determinism discipline must not regress the
    // zero-extra-work acyclic path into emitting spurious SCCs).
    let acyclic = || {
        let project = single_model_project(vec![
            aux_var("rate", "0.1"),
            aux_var("growth", "rate * 100"),
        ]);
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &project);
        let model = result.models["main"].source;
        crate::db::model_dependency_graph(&db, model, result.project)
            .resolved_sccs
            .clone()
    };
    let acyclic_first = acyclic();
    let acyclic_second = acyclic();
    assert!(
        acyclic_first.is_empty(),
        "an acyclic model emits no ResolvedScc (AC1.3 happy path \
         unaffected, zero extra work)"
    );
    assert_eq!(
        acyclic_first, acyclic_second,
        "the acyclic control's empty resolved_sccs must be byte-identical \
         across repeated compiles (no spurious nondeterministic emission)"
    );
}

// ── ResolvedScc / SccPhase salsa-equality wiring ────────────────────────
//
// `ResolvedScc` rides on `ModelDepGraphResult`, which is a salsa return
// value (`#[salsa::tracked(returns(ref))]`). salsa decides whether a
// downstream query must be re-run by *structural equality* of the
// returned value, so the new `resolved_sccs` field MUST participate in
// `PartialEq`/`Eq`/`salsa::Update`. If the derive silently skipped the
// field (or the field were not wired into the struct), two results that
// differ ONLY in `resolved_sccs` would compare equal and a model whose
// only change is its resolved-SCC set would not invalidate the cache --
// a correctness bug, not a cosmetic one. This test pins that the field
// is wired and participates in equality; it deliberately does NOT
// re-test what the compiler already verifies (field presence/types).

/// A `ResolvedScc` with a non-empty member set and a non-trivial
/// per-element order constructs, and equality distinguishes the two
/// `SccPhase` variants.
#[test]
fn resolved_scc_constructs_and_phase_distinguishes() {
    use crate::common::Ident;
    use crate::db::{ResolvedScc, SccPhase};

    let members: BTreeSet<Ident<_>> = [Ident::new("ecc")].into_iter().collect();
    let element_order = vec![
        (Ident::new("ecc"), 0usize),
        (Ident::new("ecc"), 1usize),
        (Ident::new("ecc"), 2usize),
    ];

    let dt = ResolvedScc {
        members: members.clone(),
        element_order: element_order.clone(),
        phase: SccPhase::Dt,
    };
    let dt_again = ResolvedScc {
        members: members.clone(),
        element_order: element_order.clone(),
        phase: SccPhase::Dt,
    };
    let initial = ResolvedScc {
        members,
        element_order,
        phase: SccPhase::Initial,
    };

    // Structurally-identical `ResolvedScc`s compare equal.
    assert_eq!(dt, dt_again);
    // The phase is part of identity (it routes dt vs init resolution).
    assert_ne!(dt, initial);
}

/// `resolved_sccs` participates in `ModelDepGraphResult` equality, so
/// salsa cache invalidation reacts to a change in the resolved-SCC set.
#[test]
fn model_dep_graph_result_equality_observes_resolved_sccs() {
    use crate::common::Ident;
    use crate::db::{ModelDepGraphResult, ResolvedScc, SccPhase};

    let base = ModelDepGraphResult {
        dt_dependencies: HashMap::new(),
        initial_dependencies: HashMap::new(),
        runlist_initials: Vec::new(),
        runlist_flows: Vec::new(),
        runlist_stocks: Vec::new(),
        has_cycle: false,
        resolved_sccs: Vec::new(),
    };

    // A clone with no other change is equal.
    assert_eq!(base, base.clone());

    // Pushing a `ResolvedScc` makes the result compare unequal: proof
    // that `resolved_sccs` is wired into the derived `PartialEq`/`Eq`
    // (and therefore the salsa equality salsa uses for invalidation),
    // not skipped.
    let mut with_scc = base.clone();
    with_scc.resolved_sccs.push(ResolvedScc {
        members: [Ident::new("ecc")].into_iter().collect(),
        element_order: vec![(Ident::new("ecc"), 0usize)],
        phase: SccPhase::Dt,
    });
    assert_ne!(base, with_scc);

    // Two results carrying an identical `resolved_sccs` are equal again
    // (equality is structural over the field, not identity).
    assert_eq!(with_scc, with_scc.clone());
}
