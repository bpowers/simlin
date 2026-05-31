// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the dt-phase dependency-graph cycle-relation primitive and
//! the `#[cfg(test)]` SCC accessor. Live in their own file alongside the
//! production code in `db/dep_graph.rs` to keep both `db.rs` and
//! `db/tests.rs` under the per-file line cap.

use super::*;
use crate::datamodel;
use crate::db::{SimlinDb, sync_from_datamodel};
use crate::test_common::TestProject;

// ── dt-phase cycle introspection ────────────────────────────────────────
//
// `walk_successors(.., SccPhase::Dt)` is the single shared dt-phase
// cycle-successor relation consumed by both the production cycle detector
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

/// Build a bare `VarInfo` for the pure-unit dt-phase `walk_successors`
/// tests.
/// The dep names are already canonical (lowercase, underscore-joined), so
/// `from_str_unchecked` (intern, no re-canonicalization) is sound.
fn vi_for_test(is_stock: bool, is_module: bool, dt_deps: &[&str]) -> VarInfo {
    VarInfo {
        is_stock,
        is_module,
        is_table_only: false,
        dt_deps: dt_deps
            .iter()
            .map(|s| Ident::from_str_unchecked(s))
            .collect(),
        initial_deps: BTreeSet::new(),
    }
}

/// Insert a `VarInfo` keyed by the interned canonical `name`.
fn vi_insert(map: &mut FxHashMap<Ident<Canonical>, VarInfo>, name: &str, info: VarInfo) {
    map.insert(Ident::from_str_unchecked(name), info);
}

/// Compare a `walk_successors` result (now
/// `Vec<&Ident<Canonical>>`) against the expected canonical names, preserving
/// the exact ordered-equality assertions the tests had against `Vec<&str>`.
fn succ_strs<'a>(succ: &[&'a Ident<Canonical>]) -> Vec<&'a str> {
    succ.iter().map(|i| i.as_str()).collect()
}

#[test]
fn dt_walk_successors_stock_is_dt_sink() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(&mut vinfo, "s", vi_for_test(true, false, &["a", "b"]));
    vi_insert(&mut vinfo, "a", vi_for_test(false, false, &[]));
    vi_insert(&mut vinfo, "b", vi_for_test(false, false, &[]));
    // A Stock breaks the dt dependency chain: no cycle successors even
    // though its dt_deps are non-empty.
    assert!(walk_successors(&vinfo, "s", SccPhase::Dt).is_empty());
}

#[test]
fn dt_walk_successors_module_has_no_cycle_successors() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(&mut vinfo, "m", vi_for_test(false, true, &["a"]));
    vi_insert(&mut vinfo, "a", vi_for_test(false, false, &[]));
    // A Module returns before `processing.insert`, so it is never on the
    // DFS stack and can never carry a cycle: empty cycle-successor set.
    assert!(walk_successors(&vinfo, "m", SccPhase::Dt).is_empty());
}

#[test]
fn dt_walk_successors_aux_filters_stock_and_unknown_keeps_module() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(
        &mut vinfo,
        "x",
        vi_for_test(false, false, &["aux2", "the_stock", "the_mod", "ghost"]),
    );
    vi_insert(&mut vinfo, "aux2", vi_for_test(false, false, &[]));
    vi_insert(&mut vinfo, "the_stock", vi_for_test(true, false, &[]));
    vi_insert(&mut vinfo, "the_mod", vi_for_test(false, true, &[]));
    // "ghost" is intentionally absent from var_info (an unknown dep).
    let succ = walk_successors(&vinfo, "x", SccPhase::Dt);
    // Stock-targeted dep dropped (a stock breaks the dt chain), unknown
    // dep dropped, module-targeted dep KEPT (a module node has no
    // successors so Tarjan cannot route a cycle through it -- this
    // matches `compute_inner`, whose `!dep_info.is_module` guard only
    // controls transitive absorption, not iteration).
    assert_eq!(succ_strs(&succ), vec!["aux2", "the_mod"]);
}

#[test]
fn dt_walk_successors_absent_name_is_empty() {
    let vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    // A malformed/absent var_info entry must not panic; it yields no
    // successors.
    assert!(walk_successors(&vinfo, "nope", SccPhase::Dt).is_empty());
}

#[test]
fn dt_walk_successors_order_is_btreeset_sorted() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(
        &mut vinfo,
        "x",
        vi_for_test(false, false, &["zeta", "alpha", "mid"]),
    );
    vi_insert(&mut vinfo, "zeta", vi_for_test(false, false, &[]));
    vi_insert(&mut vinfo, "alpha", vi_for_test(false, false, &[]));
    vi_insert(&mut vinfo, "mid", vi_for_test(false, false, &[]));
    // dt_deps is a BTreeSet; the successor list preserves its sorted
    // iteration order. This is what makes the cycle-detection
    // first-back-edge and the SCC adjacency byte-stable across runs.
    assert_eq!(
        succ_strs(&walk_successors(&vinfo, "x", SccPhase::Dt)),
        vec!["alpha", "mid", "zeta"]
    );
}

// ── init-phase cycle relation (`walk_successors(.., SccPhase::Initial)`) ─
//
// `walk_successors(.., SccPhase::Initial)` is the single shared init-phase
// cycle-successor relation, the exact analogue of the dt phase for the init
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
/// init-phase `walk_successors` tests (the dt-only helper `vi_for_test`
/// leaves `initial_deps` empty).
fn vi_init_for_test(is_stock: bool, is_module: bool, initial_deps: &[&str]) -> VarInfo {
    VarInfo {
        is_stock,
        is_module,
        is_table_only: false,
        dt_deps: BTreeSet::new(),
        initial_deps: initial_deps
            .iter()
            .map(|s| Ident::from_str_unchecked(s))
            .collect(),
    }
}

#[test]
fn init_walk_successors_module_has_no_cycle_successors() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(&mut vinfo, "m", vi_init_for_test(false, true, &["a"]));
    vi_insert(&mut vinfo, "a", vi_init_for_test(false, false, &[]));
    // The module early-return in `compute_inner` fires before
    // `processing.insert` in BOTH phases, so a module is never on the
    // DFS stack and can never carry a cycle in the init phase either:
    // empty cycle-successor set (mirrors the dt phase).
    assert!(walk_successors(&vinfo, "m", SccPhase::Initial).is_empty());
}

#[test]
fn init_walk_successors_stock_is_not_an_init_sink() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(&mut vinfo, "s", vi_init_for_test(true, false, &["s", "a"]));
    vi_insert(&mut vinfo, "a", vi_init_for_test(false, false, &[]));
    // A Stock is NOT an init-phase sink: the dt stock sink in
    // `compute_inner` is `!is_initial`-gated, so in the init phase a
    // stock's `initial_deps` ARE its cycle successors. A stock whose
    // init equation references itself (`s` in its own init deps) is a
    // genuine init self-loop, so `s` MUST appear in its own successor
    // set (this is exactly what an init-phase recurrence behind a stock
    // relies on).
    assert_eq!(
        succ_strs(&walk_successors(&vinfo, "s", SccPhase::Initial)),
        vec!["a", "s"]
    );
}

#[test]
fn init_walk_successors_keeps_stock_targeted_deps() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(
        &mut vinfo,
        "x",
        vi_init_for_test(false, false, &["the_stock", "aux2"]),
    );
    vi_insert(&mut vinfo, "the_stock", vi_init_for_test(true, false, &[]));
    vi_insert(&mut vinfo, "aux2", vi_init_for_test(false, false, &[]));
    // Unlike the dt phase (which drops stock-targeted deps because a stock
    // breaks the dt chain), the init relation KEEPS a stock-targeted dep:
    // a stock's initial value is a real init-phase dependency. NO stock
    // filter on the deps.
    assert_eq!(
        succ_strs(&walk_successors(&vinfo, "x", SccPhase::Initial)),
        vec!["aux2", "the_stock"]
    );
}

#[test]
fn init_walk_successors_filters_unknown_deps() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(
        &mut vinfo,
        "x",
        vi_init_for_test(false, false, &["known", "ghost"]),
    );
    vi_insert(&mut vinfo, "known", vi_init_for_test(false, false, &[]));
    // "ghost" is intentionally absent from var_info.
    // This is exactly the inlined `compute_inner` init semantics
    // (`info.initial_deps.iter().filter(|dep|
    // var_info.contains_key(dep))`): unknown deps dropped, no other
    // filter.
    assert_eq!(
        succ_strs(&walk_successors(&vinfo, "x", SccPhase::Initial)),
        vec!["known"]
    );
}

#[test]
fn init_walk_successors_absent_name_is_empty() {
    let vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    // A malformed/absent var_info entry must not panic; it yields no
    // successors (mirrors the dt phase; `compute_inner` likewise
    // early-returns `Ok(())` for an unknown name).
    assert!(walk_successors(&vinfo, "nope", SccPhase::Initial).is_empty());
}

#[test]
fn init_walk_successors_order_is_btreeset_sorted() {
    let mut vinfo: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    vi_insert(
        &mut vinfo,
        "x",
        vi_init_for_test(false, false, &["zeta", "alpha", "mid"]),
    );
    vi_insert(&mut vinfo, "zeta", vi_init_for_test(false, false, &[]));
    vi_insert(&mut vinfo, "alpha", vi_init_for_test(false, false, &[]));
    vi_insert(&mut vinfo, "mid", vi_init_for_test(false, false, &[]));
    // initial_deps is a BTreeSet; the successor list preserves its
    // sorted iteration order, so init cycle detection and the init SCC
    // adjacency are byte-stable across runs (same discipline as the dt
    // phase).
    assert_eq!(
        succ_strs(&walk_successors(&vinfo, "x", SccPhase::Initial)),
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
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
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
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
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
fn consistency_violation_some_when_resolved_dt_scc_not_instrumented() {
    // A `phase: Dt` `ResolvedScc` whose members the dt instrumentation
    // never surfaced as an SCC => the refinement resolved a DT cycle the
    // shared dt relation did not even see => the two relations drifted
    // => STOP (the whole point of the cross-check). This must stay
    // flagged even after Task 3 -- the dt cross-check is unweakened for
    // the dt path.
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: BTreeSet::new(),
    };
    assert!(dt_cycle_sccs_consistency_violation(&sccs, &resolved_a(), false).is_some());
}

#[test]
fn consistency_violation_none_for_init_only_resolved_scc_not_dt_instrumented() {
    // Phase 2 Task 3 generalization: a `phase: Initial` `ResolvedScc`
    // for an init-only recurrence (a per-element forward recurrence in a
    // stock's initial value) is BY DESIGN absent from the dt
    // instrumentation -- a stock breaks the dt chain, so the dt
    // `walk_successors` relation reports no dt SCC for it. The dt-phase
    // consistency cross-check must therefore NOT treat a `phase:
    // Initial` resolved SCC as a "dt relation drifted" orphan (check 2
    // is scoped to `phase: Dt` SCCs; the dt instrumentation does not and
    // should not surface init-only cycles). The contrast with
    // `consistency_violation_some_when_resolved_dt_scc_not_instrumented`
    // is exactly the phase: a non-dt-instrumented `phase: Dt` SCC is a
    // genuine drift; a non-dt-instrumented `phase: Initial` SCC is
    // correct.
    let sccs = DtCycleSccs {
        multi: vec![],
        self_loops: BTreeSet::new(),
    };
    let init_only: Vec<crate::db::ResolvedScc> = vec![crate::db::ResolvedScc {
        members: [crate::common::Ident::new("s")].into_iter().collect(),
        element_order: vec![
            (crate::common::Ident::new("s"), 0usize),
            (crate::common::Ident::new("s"), 1usize),
        ],
        phase: crate::db::SccPhase::Initial,
    }];
    assert!(
        dt_cycle_sccs_consistency_violation(&sccs, &init_only, false).is_none(),
        "a phase:Initial ResolvedScc that is (correctly) not dt-instrumented \
         must NOT trip the dt-phase consistency cross-check"
    );
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
    // `crate::db::var_fragment::lower_var_fragment`), partitioned by
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

// ── Per-element dt SCC resolution (the cycle-gate refinement) ───────────
//
// `resolve_recurrence_sccs(.., SccPhase::Dt)` identifies the offending
// dt SCC(s) over the same shared `walk_successors(.., SccPhase::Dt)`
// relation the
// engine uses, refines each into an exact `(member, element-offset)`
// graph from the engine's own production-lowered per-element exprs, and
// renders a verdict:
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

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
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
    use crate::db::SccPhase;

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

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
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
    use crate::db::SccPhase;

    // `a=b+1; b=a+1`: a scalar 2-cycle / multi-variable SCC. Phase 1
    // routes multi-variable SCCs to unresolved (Phase 2 resolves them),
    // and it is also a genuine element 2-cycle => unresolved (AC4.1).
    let project = single_model_project(vec![aux_var("a", "b + 1"), aux_var("b", "a + 1")]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
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
    use crate::db::SccPhase;

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

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(!res.has_unresolved, "a clean DAG has no unresolved SCC");
    assert!(
        res.resolved.is_empty(),
        "a clean DAG has no resolved SCC either (zero extra work)"
    );
}

#[test]
fn resolve_dt_recurrence_sccs_is_byte_stable_across_runs() {
    use crate::db::SccPhase;

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
        resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt).resolved
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
        crate::db::model_dependency_graph(
            &db,
            model,
            result.project,
            crate::db::ModuleInputSet::empty(&db),
        )
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
        crate::db::model_dependency_graph(
            &db,
            model,
            result.project,
            crate::db::ModuleInputSet::empty(&db),
        )
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

// ── Per-element INIT SCC resolution (the init-phase cycle gate) ─────────
//
// `resolve_recurrence_sccs(.., SccPhase::Initial)` is the init-phase
// analogue of the dt resolution: it identifies the offending init SCC(s)
// over the shared `walk_successors(.., SccPhase::Initial)` relation
// (Task 2), refines each
// into its exact per-element graph from the engine's own production
// *init*-phase symbolic fragment
// (`var_phase_symbolic_fragment_prod(.., Initial)`, reused via the
// phase-parameterized `symbolic_phase_element_order`), and renders
// the verdict: element-acyclic + element-sourceable single-variable init
// self-recurrence => `ResolvedScc { phase: Initial }`; a genuine init
// element cycle (same-element self-loop) => unresolved (loud-safe, keep
// `CircularDependency`).
//
// The init relation is structurally DISTINCT from dt only for a stock: a
// stock's dt-equation is its flow (the stock breaks the dt chain --
// `walk_successors(.., SccPhase::Dt)` returns `[]`), while its
// init-equation is its
// initial value, so a stock whose initial value is a per-element forward
// recurrence has an init self-loop with NO corresponding dt cycle. This
// is the case Phase 1's dt path cannot reach (Phase 1's
// `refine_scc_to_element_verdict` only verifies init-acyclicity as a
// *precondition* of resolving a dt self-loop whose self-edge is in BOTH
// relations -- the aux self-recurrence case). Task 3 generalizes that to
// an independent init verdict for the init-only (stock-backed) case
// WITHOUT regressing the aux case (a `{ecc}` already in the dt-resolved
// set is not re-resolved as a duplicate `phase: Initial` SCC).

/// A single-model datamodel project whose only stateful variable is an
/// arrayed stock `s[t]` (over a 3-element named dimension `t`) with a
/// per-element forward INIT recurrence and a trivial constant inflow.
///
/// `s`'s init AST references `s` (`s[t2]=s[t1]+1`, ...), so `s` is in its
/// own `initial_deps` (an init self-loop). Its dt-equation is the flow
/// `inflow` (a stock breaks the dt chain), so there is NO dt cycle. The
/// induced per-element INIT graph `(s,0)->(s,1)->(s,2)` is acyclic.
fn arrayed_init_recurrence_stock_project(init_eqs: Vec<(&str, &str)>) -> datamodel::Project {
    use crate::datamodel::{Dimension, Equation, Flow, Stock, Variable};
    let dims = vec!["t".to_string()];
    let arrayed = init_eqs
        .into_iter()
        .map(|(elem, eq)| (elem.to_string(), eq.to_string(), None, None))
        .collect();
    datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![Dimension::named(
            "t".to_string(),
            vec!["t1".to_string(), "t2".to_string(), "t3".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                Variable::Stock(Stock {
                    ident: "s".to_string(),
                    equation: Equation::Arrayed(dims.clone(), arrayed, None, false),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["inflow".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                Variable::Flow(Flow {
                    ident: "inflow".to_string(),
                    equation: Equation::ApplyToAll(dims, "0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
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
fn resolve_init_forward_recurrence_behind_stock_is_resolved() {
    use crate::db::SccPhase;

    // A stock whose INIT equation is a per-element forward recurrence
    // (`s[t1]=1; s[t2]=s[t1]+1; s[t3]=s[t2]+1`). The stock breaks the dt
    // chain, so this is an init-ONLY recurrence: it has NO dt cycle, yet
    // the init relation has a forward element recurrence. Phase 1's dt
    // path can never reach it (a stock has no dt self-edge); Task 3's
    // init verdict resolves it.
    let project = arrayed_init_recurrence_stock_project(vec![
        ("t1", "1"),
        ("t2", "s[t1] + 1"),
        ("t3", "s[t2] + 1"),
    ]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    // The DT relation must have NO cycle (the stock breaks the dt
    // chain), so the dt resolution finds nothing.
    let dt_res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(
        !dt_res.has_unresolved && dt_res.resolved.is_empty(),
        "a stock breaks the dt chain: the dt relation is acyclic, so the \
         dt resolution must find neither resolved nor unresolved SCCs \
         (got resolved={:?}, has_unresolved={})",
        dt_res.resolved,
        dt_res.has_unresolved
    );

    // The INIT relation has a single-variable forward element recurrence
    // on `s` whose induced per-element graph is acyclic and
    // element-sourceable => resolved with phase == Initial.
    let init_res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Initial);
    assert!(
        !init_res.has_unresolved,
        "the init recurrence is element-acyclic and element-sourceable: \
         there must be NO unresolved init SCC"
    );
    assert_eq!(
        init_res.resolved.len(),
        1,
        "exactly one resolved init SCC (s)"
    );
    let scc = &init_res.resolved[0];
    assert_eq!(
        scc.phase,
        SccPhase::Initial,
        "an init-phase recurrence must carry phase == Initial"
    );
    assert_eq!(
        scc.members,
        [crate::common::Ident::new("s")]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    // Deterministic per-element init topological order: t1 has no in-SCC
    // reader edges; t2 reads t1; t3 reads t2.
    assert_eq!(
        scc.element_order,
        vec![
            (crate::common::Ident::new("s"), 0usize),
            (crate::common::Ident::new("s"), 1usize),
            (crate::common::Ident::new("s"), 2usize),
        ],
        "init element_order must be the per-element topological order"
    );
}

#[test]
fn init_recurrence_behind_stock_model_dep_graph_resolves_no_circular() {
    // End-to-end through the production `model_dependency_graph`: the
    // init-only forward recurrence behind a stock must NOT raise
    // `CircularDependency`, and the emitted `resolved_sccs` must carry
    // exactly one `ResolvedScc { phase: Initial }` for `{s}`. dt has no
    // cycle (stock breaks it).
    let project = arrayed_init_recurrence_stock_project(vec![
        ("t1", "1"),
        ("t2", "s[t1] + 1"),
        ("t3", "s[t2] + 1"),
    ]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "an element-acyclic init-only recurrence behind a stock must NOT \
         set has_cycle"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one ResolvedScc for the resolved init recurrence"
    );
    assert_eq!(
        dep_graph.resolved_sccs[0].phase,
        crate::db::SccPhase::Initial,
        "the resolved SCC is an init-phase recurrence"
    );
    assert_eq!(
        dep_graph.resolved_sccs[0].members,
        [crate::common::Ident::new("s")]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );

    // No CircularDependency diagnostic was accumulated.
    let diags = crate::db::model_dependency_graph::accumulated::<crate::db::CompilationDiagnostic>(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !diags.iter().any(|d| matches!(
            d.0.error,
            crate::db::DiagnosticError::Model(crate::common::Error {
                code: crate::common::ErrorCode::CircularDependency,
                ..
            })
        )),
        "no CircularDependency must be raised for the resolved init-only \
         recurrence"
    );
}

#[test]
fn init_same_element_self_cycle_behind_stock_is_unresolved() {
    use crate::db::SccPhase;

    // A stock whose INIT equation is a genuine same-element self-cycle
    // (`s[t] = s[t] + 1` -- every element reads ITSELF). This is a real
    // init element self-loop and MUST stay unresolved (loud-safe: keep
    // `CircularDependency`), exactly as the dt path keeps
    // `x[dimA]=x[dimA]+1` unresolved.
    let project = arrayed_init_recurrence_stock_project(vec![
        ("t1", "s[t1] + 1"),
        ("t2", "s[t2] + 1"),
        ("t3", "s[t3] + 1"),
    ]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let init_res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Initial);
    assert!(
        init_res.has_unresolved,
        "s[t]=s[t]+1 in the init equation is a genuine element self-loop \
         and MUST be unresolved (a real cycle stays rejected)"
    );
    assert!(
        init_res.resolved.is_empty(),
        "a genuine init element self-loop yields no ResolvedScc"
    );

    // End-to-end: the genuine init cycle is still flagged.
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        dep_graph.has_cycle,
        "a genuine init element self-cycle still sets has_cycle"
    );
    assert!(
        dep_graph.resolved_sccs.is_empty(),
        "a genuine init element self-cycle resolves nothing"
    );
}

#[test]
fn dt_self_recurrence_not_double_resolved_as_init_scc() {
    // REGRESSION GUARD for Phase 1's existing init handling. The aux
    // self-recurrence `ecc[t1]=1; ecc[t2]=ecc[t1]+1; ecc[t3]=ecc[t2]+1`
    // has its `ecc -> ecc` self-edge in BOTH the dt and the init
    // relation. Phase 1's dt path already resolves it (verifying init
    // element-acyclicity as a precondition) and emits exactly ONE
    // `ResolvedScc { phase: Dt }`; the shared resolvable set breaks its
    // init self-edge in the init gate. Task 3's init verdict must NOT
    // additionally emit a duplicate `ResolvedScc { phase: Initial }` for
    // `{ecc}` -- the emitted `resolved_sccs` must stay length 1, phase
    // Dt. (This is the exact "extends, not duplicates Phase 1" contract.)
    let project = TestProject::new("dt_init_no_double_resolve")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ecc[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        );
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "the aux self-recurrence must still resolve (no CircularDependency)"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "the both-relations aux self-recurrence must emit EXACTLY ONE \
         ResolvedScc -- Task 3's init verdict must not add a duplicate \
         phase:Initial SCC for a member the dt path already resolved \
         (got {:?})",
        dep_graph.resolved_sccs
    );
    assert_eq!(
        dep_graph.resolved_sccs[0].phase,
        crate::db::SccPhase::Dt,
        "the single resolved SCC stays the dt-path verdict (phase == Dt), \
         not re-attributed to Initial"
    );
}

// ── Phase 2 Task 9 (AC2.4): MULTI-member init-only recurrence SCC ───────
//
// Subcomponent A's `init_recurrence_behind_stock_*` tests cover a
// SINGLE-variable (`{s}`) init recurrence behind a stock. AC2.4's
// combined-fragment init path (the synthetic-ident `SymbolicCompiledInitial`
// in `assemble_module`, Task 6) is only exercised by a MULTI-member init
// SCC -- two variables whose INIT relation forms a `ref.mdl`-shaped
// inter-element recurrence, with a stock breaking the dt chain so the
// cycle is init-ONLY (no dt SCC). This is the empirical confirmation that
// the chosen fixture shape produces a `phase: Initial` `ResolvedScc` with
// >= 2 members (the precondition the `init_recurrence.mdl` end-to-end
// simulation test in `tests/simulate.rs` relies on), and a permanent
// regression guard at the datamodel/verdict level.

/// Two arrayed stocks `cs[t]` / `ecs[t]` over a 3-element named dimension
/// `t`, whose per-element INIT (INTEG initial-value) equations form a
/// `ref.mdl`-shaped inter-element recurrence ACROSS the two variables:
///
///   cs[t1] = 1            ecs[t1] = cs[t1] + 1
///   cs[t2] = ecs[t1] + 1  ecs[t2] = cs[t2] + 1
///   cs[t3] = ecs[t2] + 1  ecs[t3] = cs[t3] + 1
///
/// Both stocks have a trivial constant inflow `g[t] = 0`, so each stock's
/// dt-equation is its (acyclic) flow and the stock BREAKS the dt chain
/// (`walk_successors(.., SccPhase::Dt)` returns `[]` for a stock): there is
/// NO dt cycle.
/// The INIT relation, however, has `cs`'s init referencing `ecs` and
/// `ecs`'s init referencing `cs` -> a whole-variable init 2-cycle
/// `{cs,ecs}` whose induced per-element INIT graph
///   (cs,0)->(ecs,0); (cs,1)->(ecs,1); (cs,2)->(ecs,2);
///   (ecs,0)->(cs,1); (ecs,1)->(cs,2)
/// is acyclic. So this is an init-ONLY MULTI-member element-acyclic
/// recurrence -- exactly AC2.4's combined-fragment init path.
fn two_stock_init_recurrence_project(
    cs_init: Vec<(&str, &str)>,
    ecs_init: Vec<(&str, &str)>,
) -> datamodel::Project {
    use crate::datamodel::{Dimension, Equation, Flow, Stock, Variable};
    let dims = vec!["t".to_string()];
    let arrayed = |eqs: Vec<(&str, &str)>| {
        Equation::Arrayed(
            dims.clone(),
            eqs.into_iter()
                .map(|(elem, eq)| (elem.to_string(), eq.to_string(), None, None))
                .collect(),
            None,
            false,
        )
    };
    let stock = |ident: &str, eq: Equation| {
        Variable::Stock(Stock {
            ident: ident.to_string(),
            equation: eq,
            documentation: String::new(),
            units: None,
            inflows: vec!["g".to_string()],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    };
    datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![Dimension::named(
            "t".to_string(),
            vec!["t1".to_string(), "t2".to_string(), "t3".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                stock("cs", arrayed(cs_init)),
                stock("ecs", arrayed(ecs_init)),
                Variable::Flow(Flow {
                    ident: "g".to_string(),
                    equation: Equation::ApplyToAll(dims, "0".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
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
fn two_stock_init_recurrence_is_resolved_init_only_multi_member() {
    use crate::db::SccPhase;

    // `ref.mdl`-shaped init recurrence behind two stocks. The stocks
    // break the dt chain (their dt-equation is the constant flow `g`), so
    // dt is acyclic; the INIT relation has the multi-member element-acyclic
    // `{cs,ecs}` recurrence. This is the AC2.4 init-phase, MULTI-member
    // path (Subcomponent A only covered the single-variable case).
    let project = two_stock_init_recurrence_project(
        vec![("t1", "1"), ("t2", "ecs[t1] + 1"), ("t3", "ecs[t2] + 1")],
        vec![
            ("t1", "cs[t1] + 1"),
            ("t2", "cs[t2] + 1"),
            ("t3", "cs[t3] + 1"),
        ],
    );
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    // DT relation: the stocks break the dt chain, the flow `g` is
    // constant -> NO dt SCC at all.
    let dt_res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(
        !dt_res.has_unresolved && dt_res.resolved.is_empty(),
        "the stocks break the dt chain: the dt relation must have NO SCC \
         (got resolved={:?}, has_unresolved={})",
        dt_res.resolved,
        dt_res.has_unresolved
    );

    // INIT relation: the `{cs,ecs}` cluster is a MULTI-member init
    // recurrence whose induced per-element init graph is acyclic and
    // element-sourceable -> exactly one resolved `phase: Initial` SCC
    // with BOTH members.
    let init_res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Initial);
    assert!(
        !init_res.has_unresolved,
        "the init recurrence is element-acyclic and element-sourceable: \
         there must be NO unresolved init SCC (got has_unresolved=true)"
    );
    assert_eq!(
        init_res.resolved.len(),
        1,
        "exactly one resolved init SCC (the {{cs,ecs}} cluster) -- got {:?}",
        init_res.resolved
    );
    let scc = &init_res.resolved[0];
    assert_eq!(
        scc.phase,
        SccPhase::Initial,
        "an init-phase recurrence must carry phase == Initial"
    );
    assert_eq!(
        scc.members,
        [
            crate::common::Ident::new("cs"),
            crate::common::Ident::new("ecs"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "the resolved init SCC must be MULTI-member ({{cs,ecs}}) -- this \
         is what exercises AC2.4's combined-fragment init path (a \
         1-member SCC is already covered by Subcomponent A)"
    );
    assert!(
        scc.members.len() >= 2,
        "AC2.4 requires a MULTI-member init SCC; got {} member(s)",
        scc.members.len()
    );
    // Deterministic interleaved per-element init topological order. cs[0]
    // is the only in-SCC source; ecs[0] reads cs[0]; cs[1] reads ecs[0];
    // ecs[1] reads cs[1]; cs[2] reads ecs[1]; ecs[2] reads cs[2]. The
    // Kahn tie-break sorts `cs` before `ecs`, so the unique order is the
    // strict interleave (same discipline as the dt ref.mdl case).
    assert_eq!(
        scc.element_order,
        vec![
            (crate::common::Ident::new("cs"), 0usize),
            (crate::common::Ident::new("ecs"), 0usize),
            (crate::common::Ident::new("cs"), 1usize),
            (crate::common::Ident::new("ecs"), 1usize),
            (crate::common::Ident::new("cs"), 2usize),
            (crate::common::Ident::new("ecs"), 2usize),
        ],
        "init element_order must be the per-element topological interleave"
    );
}

#[test]
fn two_stock_init_recurrence_model_dep_graph_resolves_no_circular() {
    // End-to-end through the production `model_dependency_graph`: the
    // MULTI-member init-only recurrence behind stocks must NOT raise
    // `CircularDependency`, and the emitted `resolved_sccs` must carry
    // exactly one `ResolvedScc { phase: Initial }` for `{cs,ecs}`. This
    // is the precondition the `init_recurrence.mdl` end-to-end simulation
    // test relies on, asserted at the production-payload level.
    let project = two_stock_init_recurrence_project(
        vec![("t1", "1"), ("t2", "ecs[t1] + 1"), ("t3", "ecs[t2] + 1")],
        vec![
            ("t1", "cs[t1] + 1"),
            ("t2", "cs[t2] + 1"),
            ("t3", "cs[t3] + 1"),
        ],
    );
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "an element-acyclic MULTI-member init-only recurrence behind \
         stocks must NOT set has_cycle"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one ResolvedScc for the resolved {{cs,ecs}} init \
         recurrence (got {:?})",
        dep_graph.resolved_sccs
    );
    assert_eq!(
        dep_graph.resolved_sccs[0].phase,
        crate::db::SccPhase::Initial,
        "the resolved SCC is an init-phase recurrence"
    );
    assert_eq!(
        dep_graph.resolved_sccs[0].members,
        [
            crate::common::Ident::new("cs"),
            crate::common::Ident::new("ecs"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
}

#[test]
fn two_stock_init_genuine_element_cycle_is_unresolved() {
    use crate::db::SccPhase;

    // Loud-safe (AC4): a GENUINE multi-variable init element cycle
    // (`cs[t] = ecs[t] + 1; ecs[t] = cs[t] + 1` -- every element of each
    // stock's init reads the SAME element of the other) behind stocks
    // must STAY unresolved (keep `CircularDependency`). The stocks still
    // break the dt chain, so the only cycle is the genuine init element
    // 2-cycle; the symbolic verdict must reject it.
    let project = two_stock_init_recurrence_project(
        vec![
            ("t1", "ecs[t1] + 1"),
            ("t2", "ecs[t2] + 1"),
            ("t3", "ecs[t3] + 1"),
        ],
        vec![
            ("t1", "cs[t1] + 1"),
            ("t2", "cs[t2] + 1"),
            ("t3", "cs[t3] + 1"),
        ],
    );
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let init_res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Initial);
    assert!(
        init_res.has_unresolved,
        "a genuine multi-variable init element 2-cycle MUST be unresolved \
         (loud-safe: real cycles stay rejected)"
    );
    assert!(
        init_res.resolved.is_empty(),
        "a genuine init element cycle yields no ResolvedScc (got {:?})",
        init_res.resolved
    );

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        dep_graph.has_cycle,
        "a genuine multi-variable init element cycle still sets has_cycle"
    );
    assert!(
        dep_graph.resolved_sccs.is_empty(),
        "a genuine init element cycle resolves nothing"
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

// ── Multi-member symbolic element-graph verdict (GH #575) ───────────────
//
// Phase 1's `phase_element_order` built the SCC element graph from raw
// per-variable mini-slots and is structurally incapable of cross-member
// edges (GH #575): for any multi-member SCC it produced a wrong order
// *and* resolved a genuine multi-variable element cycle as acyclic
// (unsound -- violates the AC4 loud-safe hard rule). Phase 2 Subcomponent
// B rebuilds the verdict on the cross-member-comparable SYMBOLIC
// representation (`SymVarRef { name, element_offset }`) and removes the
// `members.len() != 1` mask, so `resolve_recurrence_sccs` refines
// multi-member SCCs too. These tests pin the rebuilt behavior; the
// genuine-element-2-cycle test (`b`) is the load-bearing GH #575
// unsoundness assertion -- it MUST fail RED before the rebuild (the
// element cycle was resolved) and pass GREEN after.

/// A two-element named-dim project whose only stateful structure is two
/// arrayed auxes `ce`/`ecc` in an inter-element recurrence shaped like
/// `test/sdeverywhere/models/ref/ref.mdl`:
///   ce[t1]=1; ce[t2]=ecc[t1]+1; ce[t3]=ecc[t2]+1
///   ecc[t1]=ce[t1]+1; ecc[t2]=ce[t2]+1; ecc[t3]=ce[t3]+1
/// Whole-variable `ce`<->`ecc` is a 2-cycle, but the induced element
/// graph
///   (ce,0) -> (ecc,0); (ce,1) -> (ecc,1); (ce,2) -> (ecc,2);
///   (ecc,0) -> (ce,1); (ecc,1) -> (ce,2)
/// is acyclic, so the SCC must resolve in the interleaved per-element
/// order ce[0],ecc[0],ce[1],ecc[1],ce[2],ecc[2].
fn ce_ecc_ref_shaped_project() -> TestProject {
    TestProject::new("ce_ecc_ref_shaped")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ce[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        )
        .array_with_ranges(
            "ecc[t]",
            vec![
                ("t1", "ce[t1] + 1"),
                ("t2", "ce[t2] + 1"),
                ("t3", "ce[t3] + 1"),
            ],
        )
}

#[test]
fn resolve_dt_two_member_ref_shaped_scc_resolves_interleaved() {
    use crate::db::SccPhase;

    // RED before the symbolic rebuild: `resolve_recurrence_sccs`
    // short-circuits every multi-member SCC to `has_unresolved` (the
    // `members.len() != 1` mask + the `multi` short-circuit), so the
    // `{ce,ecc}` SCC is Unresolved with no `ResolvedScc`. GREEN after: it
    // resolves with the correct INTERLEAVED element order.
    let project = ce_ecc_ref_shaped_project();
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(
        !res.has_unresolved,
        "the ce/ecc SCC is element-acyclic and element-sourceable: there \
         must be NO unresolved SCC (GH #575 -- the multi-member mask is \
         removed and the symbolic builder proves acyclicity)"
    );
    assert_eq!(
        res.resolved.len(),
        1,
        "exactly one resolved SCC (the {{ce,ecc}} cluster)"
    );
    let scc = &res.resolved[0];
    assert_eq!(scc.phase, SccPhase::Dt);
    assert_eq!(
        scc.members,
        [
            crate::common::Ident::new("ce"),
            crate::common::Ident::new("ecc"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "the resolved SCC's members are exactly {{ce,ecc}}"
    );
    // The correct INTERLEAVED per-element topological order: ce[0] has no
    // in-SCC reader; ecc[0] reads ce[0]; ce[1] reads ecc[0]; ecc[1] reads
    // ce[1]; ce[2] reads ecc[1]; ecc[2] reads ce[2]. Kahn tie-break sorts
    // ce before ecc, so the unique order is the strict interleave.
    let ce = |i: usize| (crate::common::Ident::new("ce"), i);
    assert_eq!(
        scc.element_order,
        vec![ce(0), ecc(0), ce(1), ecc(1), ce(2), ecc(2)],
        "element_order must be the INTERLEAVED per-element topological \
         order across the two members (GH #575: zero cross-member edges \
         would have produced a wrong non-interleaved order)"
    );
}

#[test]
fn resolve_dt_genuine_element_two_cycle_is_unresolved() {
    use crate::db::SccPhase;

    // THE GH #575 UNSOUNDNESS FIX (load-bearing). `a[d]=b[d]; b[d]=a[d]`
    // is a genuine multi-variable element 2-cycle: every element `a[i]`
    // reads `b[i]` and every `b[i]` reads `a[i]`, so the induced element
    // graph has the 2-cycles (a,i)<->(b,i). It MUST be Unresolved (keep
    // `CircularDependency`). RED before the rebuild: the old mini-slot
    // builder built ZERO cross-member edges and resolved this genuine
    // cycle as acyclic (masked only by the multi-member short-circuit;
    // with the mask gone and no symbolic rebuild it would silently
    // miscompile). GREEN after: the symbolic builder finds the element
    // 2-cycle and returns Unresolved.
    let project = TestProject::new("genuine_elem_two_cycle")
        .named_dimension("d", &["d1", "d2"])
        .array_aux("a[d]", "b[d]")
        .array_aux("b[d]", "a[d]");
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(
        res.has_unresolved,
        "a[d]=b[d];b[d]=a[d] is a genuine element 2-cycle and MUST be \
         unresolved (GH #575 -- the symbolic builder must NOT resolve a \
         real circular dependency as acyclic)"
    );
    assert!(
        res.resolved.is_empty(),
        "a genuine element 2-cycle yields no ResolvedScc"
    );

    // End-to-end: the genuine cycle still raises CircularDependency.
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        dep_graph.has_cycle,
        "a genuine multi-variable element 2-cycle still sets has_cycle"
    );
    assert!(
        dep_graph.resolved_sccs.is_empty(),
        "a genuine multi-variable element 2-cycle resolves nothing"
    );
}

#[test]
fn resolve_dt_genuine_scalar_two_cycle_is_unresolved_via_symbolic_builder() {
    use crate::db::SccPhase;

    // `a=b+1; b=a+1`: a genuine scalar 2-cycle. This is the N>=2 scalar
    // case of the symbolic builder: `a` (element 0) reads `b` (element
    // 0), `b` (element 0) reads `a` (element 0), so the element graph has
    // the 2-cycle (a,0)<->(b,0) and the verdict MUST be Unresolved. It
    // must stay rejected by the SAME symbolic builder that resolves the
    // acyclic ce/ecc case (sibling to the existing Phase 1 guard
    // `resolve_dt_scalar_two_cycle_is_unresolved`, which must also stay
    // green unchanged).
    let project = single_model_project(vec![aux_var("a", "b + 1"), aux_var("b", "a + 1")]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(
        res.has_unresolved,
        "a=b+1;b=a+1 is a genuine 2-cycle and MUST be unresolved even \
         under the symbolic multi-member builder"
    );
    assert!(
        res.resolved.is_empty(),
        "a genuine scalar 2-cycle yields no ResolvedScc"
    );
}

#[test]
fn resolve_dt_single_var_recurrence_byte_identical_to_phase1() {
    use crate::db::SccPhase;

    // N=1 REGRESSION: the single-variable forward self-recurrence is just
    // the N=1 case of the symbolic builder and MUST resolve with the
    // byte-identical `element_order` Phase 1 produced
    // ([(ecc,0),(ecc,1),(ecc,2)]). This pins that the rebuild did not
    // change N=1 behavior (the existing Phase 1 guard
    // `resolve_dt_forward_recurrence_is_resolved_in_declared_order`
    // asserts the same thing and must also stay green unchanged).
    let project = TestProject::new("n1_symbolic_byte_identical")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ecc[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        );
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(!res.has_unresolved, "the N=1 self-recurrence resolves");
    assert_eq!(res.resolved.len(), 1, "exactly one resolved SCC (ecc)");
    let scc = &res.resolved[0];
    assert_eq!(scc.phase, SccPhase::Dt);
    assert_eq!(
        scc.members,
        [crate::common::Ident::new("ecc")]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        scc.element_order,
        vec![ecc(0), ecc(1), ecc(2)],
        "the N=1 element_order must be byte-identical to Phase 1's \
         declared-order topological order (no N=1 behavior change)"
    );
}

#[test]
fn resolve_dt_interleaved_shaped_element_acyclic_through_two_cycle_resolves() {
    use crate::db::SccPhase;

    // `interleaved.mdl`-shaped: x=1; a[A1]=x; a[A2]=y; y=a[A1];
    // b[DimA]=a[DimA]. Whole-variable `a`<->`y` is a 2-cycle, but element-
    // wise x -> a[A1] -> y -> a[A2] is acyclic. The symbolic builder must
    // resolve `{a,y}` with the per-element order a[0],y[0],a[1].
    let project = TestProject::new("interleaved_shaped")
        .named_dimension("dima", &["a1", "a2"])
        .aux("x", "1", None)
        .array_with_ranges("a[dima]", vec![("a1", "x"), ("a2", "y")])
        .aux("y", "a[a1]", None)
        .array_aux("b[dima]", "a[dima]");
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(
        !res.has_unresolved,
        "x -> a[A1] -> y -> a[A2] is element-acyclic through the a<->y \
         whole-variable 2-cycle: there must be NO unresolved SCC"
    );
    assert_eq!(res.resolved.len(), 1, "exactly one resolved SCC ({{a,y}})");
    let scc = &res.resolved[0];
    assert_eq!(scc.phase, SccPhase::Dt);
    assert_eq!(
        scc.members,
        [
            crate::common::Ident::new("a"),
            crate::common::Ident::new("y"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
    // Element graph among {a,y}: (a,0)->(y,0) [y=a[A1]],
    // (y,0)->(a,1) [a[A2]=y]. (a,0) has indegree 0. Unique topo order:
    // a[0], y[0], a[1].
    assert_eq!(
        scc.element_order,
        vec![
            (crate::common::Ident::new("a"), 0usize),
            (crate::common::Ident::new("y"), 0usize),
            (crate::common::Ident::new("a"), 1usize),
        ],
        "element_order must be the interleaved per-element topological \
         order a[0],y[0],a[1]"
    );
}

#[test]
fn resolve_dt_unsourceable_member_is_unresolved_no_panic() {
    use crate::db::SccPhase;

    // Loud-safe: if a member's symbolic fragment cannot be sourced the
    // verdict must be Unresolved WITHOUT panicking (production code
    // reachable from the cycle gate must never panic). A genuine scalar
    // 2-cycle is element-cyclic anyway, so this primarily asserts the
    // no-panic + Unresolved contract on the symbolic-accessor path. (The
    // accessor's own `None`-on-absent-var no-panic contract is pinned by
    // `var_phase_symbolic_fragment_prod_none_for_absent_var_no_panic`.)
    let project = single_model_project(vec![aux_var("a", "b + 1"), aux_var("b", "a + 1")]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    // Must not panic.
    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(res.has_unresolved);
    assert!(res.resolved.is_empty());
}

#[test]
fn var_phase_symbolic_fragment_prod_none_for_absent_var_no_panic() {
    use crate::db::SccPhase;

    // The new symbolic accessor's loud-safe contract: a name with no
    // `SourceVariable` returns `None` and crucially must NOT panic the
    // way the `#[cfg(test)]` `var_noninitial_lowered_exprs` does, because
    // it is production code reachable from the cycle gate.
    let project = TestProject::new("vpsf_prod_absent")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges("arr[t]", vec![("t1", "1"), ("t2", "2"), ("t3", "3")]);
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    assert!(
        crate::db::var_phase_symbolic_fragment_prod(
            &db,
            model,
            result.project,
            "definitely_not_a_var",
            SccPhase::Dt,
        )
        .is_none(),
        "an absent variable must return None (loud-safe), never panic"
    );

    // And it must return Some for a real arrayed variable (sanity: the
    // accessor's happy path produces a symbolic fragment).
    assert!(
        crate::db::var_phase_symbolic_fragment_prod(
            &db,
            model,
            result.project,
            "arr",
            SccPhase::Dt,
        )
        .is_some(),
        "a real arrayed SourceVariable must yield a symbolic fragment"
    );
}

// ── AC3.2: a genuinely unsourceable in-SCC node => loud-safe fallback ────
//
// element-cycle-resolution.AC3.2: an SCC with an in-cycle node that
// genuinely cannot be element-sourced falls back to `CircularDependency`
// -- NO panic, NO silent miscompile (the other members are NOT partially
// resolved). The canonical organic case is an orphan node that is neither
// in `source_vars` nor resolvable via `model_implicit_var_info`; the
// reliable trigger (design deviation 5) is a `#[cfg(test)]` override that
// forces `var_phase_symbolic_fragment_prod` to yield `None` for a chosen
// in-SCC node, so the loud-safe path (`CircularDependency`, no panic) is
// exercised through the PRODUCTION symbolic path (`model_dependency_graph`
// / `symbolic_phase_element_order` -> `var_phase_symbolic_fragment_prod`),
// not a unit-level shim.

#[test]
fn unsourceable_in_scc_node_falls_back_to_circular_no_panic() {
    use crate::db::SccPhase;
    use crate::db::dep_graph::UnsourceableVarsGuard;

    // `ecc[t1]=1; ecc[t2]=ecc[t1]+1; ecc[t3]=ecc[t2]+1`: a single-variable
    // self-recurrence whose induced element graph (ecc,0)->(ecc,1)->(ecc,2)
    // is acyclic and well-founded -- WITHOUT a forced-unsourceable node it
    // resolves cleanly (the positive control below proves the SCC is not
    // independently cyclic / rejected earlier by an unrelated diagnostic,
    // so the fallback under the guard is caused ONLY by the forced
    // unsourceable node).
    let project = TestProject::new("ac32_unsourceable")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ecc[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        );
    let dm = project.build_datamodel();

    // Positive control: WITHOUT the guard the recurrence SCC resolves and
    // the model is NOT rejected (proves the rejection under the guard is
    // attributable to the forced-unsourceable node, not a pre-existing
    // genuine cycle or an unrelated diagnostic).
    {
        let db = SimlinDb::default();
        let result = sync_from_datamodel(&db, &dm);
        let model = result.models["main"].source;
        let dep_graph = crate::db::model_dependency_graph(
            &db,
            model,
            result.project,
            crate::db::ModuleInputSet::empty(&db),
        );
        assert!(
            !dep_graph.has_cycle,
            "positive control: the well-founded self-recurrence must \
             resolve when nothing forces a node unsourceable"
        );
        assert!(
            !dep_graph.resolved_sccs.is_empty(),
            "positive control: the recurrence SCC must be in resolved_sccs"
        );
        let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
        assert!(
            !res.has_unresolved,
            "positive control: no unresolved SCC without the guard"
        );
    }

    // Now force the single in-SCC member `ecc` unsourceable through the
    // production symbolic accessor. Driving the full production
    // `model_dependency_graph` exercises the real loud-safe chain:
    // `var_phase_symbolic_fragment_prod` -> `?`-None ->
    // `symbolic_phase_element_order` None -> `refine_scc_to_element_verdict`
    // Unresolved -> `resolve_recurrence_sccs` has_unresolved ->
    // `model_dependency_graph_impl` keeps `has_cycle` + accumulates
    // `CircularDependency`, resolved_sccs stays empty (the other members,
    // if any, are NOT partially resolved).
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    // The guard must outlive every `model_dependency_graph` call it
    // controls (salsa memoizes the result; a later call on the same `db`
    // would otherwise return the memoized with-guard result regardless of
    // guard state -- mirrors the `AggLoopBudgetGuard` salsa caveat).
    let _guard = UnsourceableVarsGuard::new(&["ecc"]);

    // Must NOT panic.
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        dep_graph.has_cycle,
        "an unsourceable in-SCC node must fall back to CircularDependency \
         (has_cycle), never silently miscompile"
    );
    assert!(
        dep_graph.resolved_sccs.is_empty(),
        "loud-safe: the SCC is NOT partially resolved -- no member is in \
         resolved_sccs when any in-SCC node is unsourceable"
    );

    // The fallback must surface the loud `CircularDependency` diagnostic
    // (the model is rejected, not silently miscompiled).
    let diags = crate::db::model_dependency_graph::accumulated::<crate::db::CompilationDiagnostic>(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        diags.iter().any(|d| matches!(
            &d.0.error,
            crate::db::DiagnosticError::Model(e)
                if e.code == crate::common::ErrorCode::CircularDependency
        )),
        "the loud-safe fallback must accumulate a CircularDependency \
         diagnostic (model rejected, not silently miscompiled)"
    );

    // And the focused-accessor contract: the forced-unsourceable member
    // yields `None` (the `?` loud-safe signal), never a panic.
    assert!(
        crate::db::var_phase_symbolic_fragment_prod(
            &db,
            model,
            result.project,
            "ecc",
            SccPhase::Dt,
        )
        .is_none(),
        "the forced-unsourceable in-SCC node must yield None (loud-safe), \
         never panic"
    );
}

// ── AC3.1: a synthetic helper in the recurrence SCC is parent-sourced ────
//
// element-cycle-resolution.AC3.1: a well-founded recurrence whose SCC
// includes a synthetic helper (here an INIT-expr-arg helper synthesized by
// `make_temp_arg`, `$\u{205A}{parent}\u{205A}{n}\u{205A}arg0\u{205A}{sub}`
// per design deviation 2) is resolvable when the helper's symbolic
// `PerVarBytecodes` is sourced from its parent variable's `implicit_vars`,
// mirroring the production `compile_implicit_var_fragment` chain.
//
// This is the Task 2 unit-level proof (the end-to-end simulate proof is
// Task 3): the focused accessor `var_phase_symbolic_fragment_prod` must
// return `Some` for the no-`SourceVariable` synthetic-helper node (it is
// absent from `model.variables` but resolves in `model_implicit_var_info`),
// and the consumer `symbolic_phase_element_order` must then build the SCC's
// element graph with the helper node present. RED before the Task 2
// extension: the helper has no `SourceVariable`, so the accessor returns
// `None` and `symbolic_phase_element_order`'s `?` short-circuits to `None`
// (the SCC is unresolved). GREEN after: parent-sourced `Some`, helper node
// in the element order.

/// `ecc[t]` over `t=[t1,t2,t3]` with `ecc[t1] = INIT(seed * 2)` (an
/// expression INIT arg -> `make_temp_arg` synthesizes a scalar helper aux,
/// the canonical `$\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t1` form) and a
/// well-founded forward element recurrence `ecc[t2]=ecc[t1]+1`,
/// `ecc[t3]=ecc[t2]+1`. `seed` is an external constant the helper reads.
/// The induced element graph over `{ecc, helper}` in the Initial phase is
/// `(helper,0) -> (ecc,0) -> (ecc,1) -> (ecc,2)` -- acyclic and
/// element-sourceable iff the helper fragment is parent-sourced.
fn ecc_with_init_helper_project() -> TestProject {
    TestProject::new("ecc_init_helper")
        .named_dimension("t", &["t1", "t2", "t3"])
        .aux("seed", "1", None)
        .array_with_ranges(
            "ecc[t]",
            vec![
                ("t1", "INIT(seed * 2)"),
                ("t2", "ecc[t1] + 1"),
                ("t3", "ecc[t2] + 1"),
            ],
        )
}

/// The single synthetic-helper canonical name for the
/// `ecc_with_init_helper_project` fixture: the entry in
/// `model_implicit_var_info` that is absent from `model.variables` and
/// carries the `$\u{205A}` synthetic-helper prefix (design deviation 2).
/// Derived from the engine's own `model_implicit_var_info` rather than
/// hard-coded so the test pins the real synthesized name.
fn sole_synthetic_helper_name(
    db: &SimlinDb,
    model: crate::db::SourceModel,
    project: crate::db::SourceProject,
) -> String {
    let info = crate::db::model_implicit_var_info(db, model, project);
    let source_vars = model.variables(db);
    let helpers: Vec<&String> = info
        .keys()
        .filter(|name| {
            source_vars.get(name.as_str()).is_none()
                && name.starts_with('$')
                && name.contains('\u{205A}')
        })
        .collect();
    assert_eq!(
        helpers.len(),
        1,
        "fixture must synthesize exactly one `$\u{205A}` helper absent from \
         model.variables; got {helpers:?} (all implicit: {:?})",
        info.keys().collect::<Vec<_>>()
    );
    helpers[0].clone()
}

#[test]
fn synthetic_helper_symbolic_fragment_is_parent_sourced() {
    use crate::db::SccPhase;

    let dm = ecc_with_init_helper_project().build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let helper = sole_synthetic_helper_name(&db, model, result.project);

    // Sanity: the helper genuinely has NO `SourceVariable` (it is the
    // no-`SourceVariable` arm Task 2 extends) yet IS the parent's
    // `implicit_vars` entry the parent-sourcing chain must reach.
    assert!(
        model.variables(&db).get(helper.as_str()).is_none(),
        "the synthetic helper {helper:?} must have no SourceVariable \
         (it is the parent-sourced arm, not a model variable)"
    );

    // CORE Task 2 deliverable: the no-`SourceVariable` synthetic-helper
    // node is sourced from the parent variable's `implicit_vars` and
    // yields a symbolic `PerVarBytecodes` (the same SymVarRef
    // layout-independent form every real SCC member produces). RED before
    // the Task 2 extension: no `SourceVariable` -> `None`.
    let frag = crate::db::var_phase_symbolic_fragment_prod(
        &db,
        model,
        result.project,
        helper.as_str(),
        SccPhase::Initial,
    );
    assert!(
        frag.is_some(),
        "var_phase_symbolic_fragment_prod must source the synthetic \
         helper {helper:?} from the parent's implicit_vars and return \
         Some(PerVarBytecodes) (AC3.1); RED before Task 2: None"
    );
    let frag = frag.unwrap();
    // The fragment must contain a per-element write of the helper itself
    // (it is the helper's own scalar aux body `seed * 2`), so the
    // element-graph segmenter can define the helper's element node.
    use crate::compiler::symbolic::SymbolicOpcode;
    assert!(
        frag.symbolic.code.iter().any(|op| matches!(
            op,
            SymbolicOpcode::AssignCurr { var }
                | SymbolicOpcode::AssignConstCurr { var, .. }
                | SymbolicOpcode::BinOpAssignCurr { var, .. }
                if var.name == helper.as_str()
        )),
        "the parent-sourced helper fragment must write the helper's own \
         per-element value (so symbolic_phase_element_order can define its \
         node); ops: {:?}",
        frag.symbolic.code
    );

    // CONSUMER: with the helper in the member set alongside the recurrence
    // variable, `symbolic_phase_element_order` must build the SCC's
    // element graph WITH the helper node present (the helper is parent-
    // sourced exactly like a real member). RED before Task 2: the helper
    // fragment is `None`, so the `?` at the member loop short-circuits the
    // whole builder to `None` (SCC unresolved).
    let members: BTreeSet<crate::common::Ident<crate::common::Canonical>> = [
        crate::common::Ident::new("ecc"),
        crate::common::Ident::new(helper.as_str()),
    ]
    .into_iter()
    .collect();
    let order =
        symbolic_phase_element_order(&db, model, result.project, &members, SccPhase::Initial);
    assert!(
        order.is_some(),
        "symbolic_phase_element_order must build the SCC element graph \
         once the helper {helper:?} is parent-sourced (RED before Task 2: \
         helper fragment None -> `?` -> None -> SCC unresolved)"
    );
    let order = order.unwrap();
    let helper_ident = crate::common::Ident::new(helper.as_str());
    assert!(
        order.iter().any(|(m, _)| *m == helper_ident),
        "the helper node {helper:?} must be present in the SCC's \
         per-element topological order; got {order:?}"
    );
    // The order is well-founded: the helper (reading only the external
    // `seed`) precedes every `ecc` element; the forward recurrence then
    // orders ecc[0] < ecc[1] < ecc[2].
    let ecc_ident = crate::common::Ident::new("ecc");
    let helper_pos = order
        .iter()
        .position(|(m, _)| *m == helper_ident)
        .expect("helper node present (asserted above)");
    let first_ecc_pos = order
        .iter()
        .position(|(m, _)| *m == ecc_ident)
        .expect("ecc has element nodes in the recurrence SCC");
    assert!(
        helper_pos < first_ecc_pos,
        "the parent-sourced helper (reading only external `seed`) must be \
         ordered before any `ecc` element; got {order:?}"
    );
}

// ── Task 5b: multi-member resolved SCC survives the dependency-graph ─────
// cycle gate (SCC-aware back-edge break + contiguous placement, GH #575)
//
// Phase 1 / Subcomponent A only ever taught `compute_transitive` to break
// a SELF-edge (`dep == name && resolvable_self_loops.contains(dep)`). For a
// multi-member resolved SCC the intra-SCC edges are CROSS-edges
// (`dep != name`), so the old guard is false, `compute_transitive` returns
// `Err`, `.unwrap_or_else` sets `has_cycle = true` and clears
// `resolved_sccs`, and `assemble_module` early-returns -- Task 4/5's
// correct verdict is unreachable. These tests pin the SCC-aware break:
// `resolve_recurrence_sccs` already resolves `{ce,ecc}` / `{a,y}` (Task 4,
// green), but `model_dependency_graph` must now ALSO report
// `has_cycle == false` with the SCC in `resolved_sccs` and the SCC's
// EXTERNAL deps propagated onto every member (the SCC treated as one
// collapsed node so the topo sort never re-sees the cycle). The
// genuine-cycle tests are the load-bearing AC4 loud-safe assertions: they
// MUST fail RED only if the generalization wrongly suppressed a real
// back-edge.

/// `ce`/`ecc` ref-shaped multi-member recurrence SCC with an EXTERNAL
/// dependency (`base`, an acyclic upstream aux every `ce` element reads)
/// and an EXTERNAL consumer (`sink`, reads `ecc`). The `{ce,ecc}` SCC is
/// element-acyclic (Task 4 resolves it). After the SCC-aware break the
/// dependency graph must treat `{ce,ecc}` as one collapsed node: both
/// members end with `base` (and nothing of each other) in their transitive
/// set, and `sink` (an external consumer) transitively depends on the
/// whole SCC + `base`.
fn ce_ecc_with_external_dep_project() -> TestProject {
    TestProject::new("ce_ecc_with_external_dep")
        .named_dimension("t", &["t1", "t2", "t3"])
        .aux("base", "7", None)
        .array_with_ranges(
            "ce[t]",
            vec![
                ("t1", "base"),
                ("t2", "ecc[t1] + base"),
                ("t3", "ecc[t2] + base"),
            ],
        )
        .array_with_ranges(
            "ecc[t]",
            vec![
                ("t1", "ce[t1] + 1"),
                ("t2", "ce[t2] + 1"),
                ("t3", "ce[t3] + 1"),
            ],
        )
        .aux("sink", "ecc[t1] + ecc[t2] + ecc[t3]", None)
}

#[test]
fn model_dep_graph_two_member_ref_scc_resolves_with_external_deps() {
    // RED before the SCC-aware break: `compute_transitive(false,
    // &resolvable)` hits the `ce -> ecc` CROSS back-edge (`dep != name`),
    // the self-edge-only guard is false, it returns `Err`, the
    // `.unwrap_or_else` sets `has_cycle = true` and clears `resolved_sccs`.
    // GREEN after: `{ce,ecc}` is one collapsed node, the back-edge is
    // suppressed (same SCC), so `has_cycle == false`, `resolved_sccs`
    // carries the `{ce,ecc}` SCC, and every member carries the SCC's
    // EXTERNAL dep `base` (not each other) in `dt_dependencies`.
    let project = ce_ecc_with_external_dep_project();
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );

    assert!(
        !dep_graph.has_cycle,
        "an element-acyclic multi-member recurrence SCC must NOT set \
         has_cycle (the intra-SCC cross-edges are not real \
         variable-granularity ordering constraints once the SCC is one \
         collapsed node)"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one ResolvedScc for the resolved {{ce,ecc}} cluster (got \
         {:?})",
        dep_graph.resolved_sccs
    );
    assert_eq!(dep_graph.resolved_sccs[0].phase, crate::db::SccPhase::Dt);
    assert_eq!(
        dep_graph.resolved_sccs[0].members,
        [
            crate::common::Ident::new("ce"),
            crate::common::Ident::new("ecc"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "the resolved SCC's members are exactly {{ce,ecc}}"
    );

    // The SCC-as-collapsed-node transitive accumulation: every member ends
    // with the SAME external transitive set (`base`), and NO member may
    // appear in another member's transitive set (else the topo sort
    // re-sees the cycle).
    let ce_deps = dep_graph
        .dt_dependencies
        .get("ce")
        .expect("ce must have a dt_dependencies entry");
    let ecc_deps = dep_graph
        .dt_dependencies
        .get("ecc")
        .expect("ecc must have a dt_dependencies entry");
    assert!(
        ce_deps.contains("base"),
        "ce must carry the SCC's external dep `base` transitively (got \
         {ce_deps:?})"
    );
    assert!(
        ecc_deps.contains("base"),
        "ecc must carry the SCC's external dep `base` transitively even \
         though only `ce` reads `base` directly -- the SCC is one \
         collapsed node so every member shares the union of external deps \
         (got {ecc_deps:?})"
    );
    assert!(
        !ce_deps.contains("ce") && !ce_deps.contains("ecc"),
        "no SCC member may appear in `ce`'s transitive set (else the topo \
         sort re-sees the cycle) (got {ce_deps:?})"
    );
    assert!(
        !ecc_deps.contains("ce") && !ecc_deps.contains("ecc"),
        "no SCC member may appear in `ecc`'s transitive set (got \
         {ecc_deps:?})"
    );

    // An external consumer carries the directly-referenced member plus
    // the SCC's external deps (the SCC being one collapsed node, depending
    // on any member transitively pulls in the SCC's member-free external
    // set). It does NOT carry the non-referenced member `ce`: the
    // SCC-as-condensed-node runlist placement (asserted below) is what
    // guarantees the WHOLE SCC precedes `sink`, not injecting every member
    // into the consumer's dep set. (`sink` references only `ecc`.)
    let sink_deps = dep_graph
        .dt_dependencies
        .get("sink")
        .expect("sink must have a dt_dependencies entry");
    assert!(
        sink_deps.contains("ecc") && sink_deps.contains("base"),
        "the external consumer `sink` must transitively depend on the \
         referenced member `ecc` and the SCC's external dep `base` (got \
         {sink_deps:?})"
    );
    assert!(
        !sink_deps.contains("ce"),
        "`sink` references only `ecc`, so `ce` (the other SCC member) must \
         NOT appear in its dep set -- the SCC is one collapsed node and \
         contiguity is enforced by the runlist condensation, not by \
         leaking every member into the consumer (got {sink_deps:?})"
    );
    let pos = |n: &str| {
        dep_graph
            .runlist_flows
            .iter()
            .position(|v| v == n)
            .unwrap_or_else(|| panic!("{n} missing from runlist_flows"))
    };
    // Deterministic CONTIGUOUS placement (Task 6 prerequisite): the two
    // SCC members must be adjacent in the flows runlist, in the SCC's
    // byte-stable sorted order (`ce` before `ecc`), so Task 6 can inject
    // ONE combined fragment at the first member's slot and skip the rest.
    assert_eq!(
        (pos("ecc") as i64) - (pos("ce") as i64),
        1,
        "the SCC members must be CONTIGUOUS in the flows runlist (ce \
         immediately followed by ecc, the byte-stable sorted order) so \
         Task 6's inject-at-first-skip-the-rest lands cleanly \
         (runlist_flows = {:?})",
        dep_graph.runlist_flows
    );
    assert!(
        pos("base") < pos("ce") && pos("base") < pos("ecc"),
        "the SCC must be positioned AFTER its external dep `base` \
         (runlist_flows = {:?})",
        dep_graph.runlist_flows
    );
    assert!(
        pos("ce") < pos("sink") && pos("ecc") < pos("sink"),
        "the SCC must be positioned BEFORE its external consumer `sink` \
         (runlist_flows = {:?})",
        dep_graph.runlist_flows
    );
}

#[test]
fn model_dep_graph_scc_members_contiguous_with_interposing_external_var() {
    // CONTIGUITY is load-bearing here: an UNRELATED external var (`mmm`)
    // sorts alphabetically strictly BETWEEN the two SCC members
    // (`aaa` < `mmm` < `zzz`). A non-SCC-aware topological sort visits
    // names in sorted order and would emit `aaa, mmm, zzz` -- the SCC
    // members NOT adjacent. Only the SCC-condensation in `topo_sort_str`
    // (emit the whole `{aaa,zzz}` block when either member is first
    // reached) yields a contiguous `aaa, zzz` run, which Task 6's
    // inject-combined-fragment-at-first-member-slot requires. `mmm` has no
    // relation to the SCC, so it may sort before or after the block, but
    // it must NOT split it.
    let project = TestProject::new("mdg_scc_contiguity")
        .named_dimension("t", &["t1", "t2", "t3"])
        .aux("mmm", "42", None)
        .array_with_ranges(
            "aaa[t]",
            vec![("t1", "1"), ("t2", "zzz[t1] + 1"), ("t3", "zzz[t2] + 1")],
        )
        .array_with_ranges(
            "zzz[t]",
            vec![
                ("t1", "aaa[t1] + 1"),
                ("t2", "aaa[t2] + 1"),
                ("t3", "aaa[t3] + 1"),
            ],
        );
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "the element-acyclic {{aaa,zzz}} SCC must NOT set has_cycle"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one ResolvedScc ({{aaa,zzz}})"
    );
    assert_eq!(
        dep_graph.resolved_sccs[0].members,
        [
            crate::common::Ident::new("aaa"),
            crate::common::Ident::new("zzz"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
    let pos = |n: &str| {
        dep_graph
            .runlist_flows
            .iter()
            .position(|v| v == n)
            .unwrap_or_else(|| panic!("{n} missing from runlist_flows"))
    };
    // The SCC members are CONTIGUOUS in byte-stable sorted order despite
    // `mmm` sorting alphabetically between them: proves the SCC-
    // condensation, not mere sorted iteration, drives placement.
    assert_eq!(
        (pos("zzz") as i64) - (pos("aaa") as i64),
        1,
        "the SCC members must be CONTIGUOUS even though the unrelated \
         external var `mmm` sorts alphabetically between them -- a \
         non-SCC-aware topo would emit aaa,mmm,zzz (runlist_flows = {:?})",
        dep_graph.runlist_flows
    );
}

#[test]
fn model_dep_graph_two_member_ref_scc_resolves_no_external_deps() {
    // The pure ref.mdl shape (members reference only each other +
    // constants, no external dep): still `has_cycle == false` with the
    // `{ce,ecc}` SCC resolved. RED before: the cross back-edge errs.
    let project = ce_ecc_ref_shaped_project();
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "the bare ref-shaped {{ce,ecc}} SCC (no external dep) must NOT set \
         has_cycle"
    );
    assert_eq!(dep_graph.resolved_sccs.len(), 1);
    assert_eq!(
        dep_graph.resolved_sccs[0].members,
        [
            crate::common::Ident::new("ce"),
            crate::common::Ident::new("ecc"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
    // dt_dependencies must be populated (Ok, not the HashMap::new() the
    // loud-safe error path returns).
    assert!(
        !dep_graph.dt_dependencies.is_empty(),
        "a resolved SCC must yield a populated dt_dependencies map, not \
         the empty map the loud-safe CircularDependency fallback returns"
    );
}

#[test]
fn model_dep_graph_interleaved_shaped_multi_member_scc_resolves() {
    // `interleaved.mdl`-shaped: x=1; a[A1]=x; a[A2]=y; y=a[A1];
    // b[DimA]=a[DimA]. Whole-variable `a`<->`y` is a 2-cycle, but
    // element-wise x -> a[A1] -> y -> a[A2] is acyclic. The SCC `{a,y}`
    // has external dep `x` and external consumer `b`. After the SCC-aware
    // break `model_dependency_graph` must report `has_cycle == false`,
    // resolve `{a,y}`, and propagate `x` onto both `a` and `y`.
    let project = TestProject::new("mdg_interleaved_shaped")
        .named_dimension("dima", &["a1", "a2"])
        .aux("x", "1", None)
        .array_with_ranges("a[dima]", vec![("a1", "x"), ("a2", "y")])
        .aux("y", "a[a1]", None)
        .array_aux("b[dima]", "a[dima]");
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "x -> a[A1] -> y -> a[A2] is element-acyclic through the a<->y \
         whole-variable 2-cycle: model_dependency_graph must NOT set \
         has_cycle"
    );
    assert_eq!(dep_graph.resolved_sccs.len(), 1);
    assert_eq!(
        dep_graph.resolved_sccs[0].members,
        [
            crate::common::Ident::new("a"),
            crate::common::Ident::new("y"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>()
    );
    let a_deps = dep_graph.dt_dependencies.get("a").expect("a deps");
    let y_deps = dep_graph.dt_dependencies.get("y").expect("y deps");
    assert!(
        a_deps.contains("x") && y_deps.contains("x"),
        "both members of the {{a,y}} SCC must carry the external dep `x` \
         (a={a_deps:?}, y={y_deps:?})"
    );
    assert!(
        !a_deps.contains("a")
            && !a_deps.contains("y")
            && !y_deps.contains("a")
            && !y_deps.contains("y"),
        "no SCC member may appear in another member's transitive set \
         (a={a_deps:?}, y={y_deps:?})"
    );
}

#[test]
fn model_dep_graph_genuine_element_two_cycle_stays_circular() {
    // LOUD-SAFE AC4 (load-bearing). `a[d]=b[d]; b[d]=a[d]` is a genuine
    // multi-variable element 2-cycle. Task 4 returns it Unresolved, so it
    // is NOT in `resolution.resolved`, so it is absent from the SCC map,
    // so its cross back-edge still `Err`s => `has_cycle == true`, empty
    // `resolved_sccs`. This must NOT regress to a silent resolve when the
    // self-edge-only break is generalized.
    let project = TestProject::new("mdg_genuine_elem_two_cycle")
        .named_dimension("d", &["d1", "d2"])
        .array_aux("a[d]", "b[d]")
        .array_aux("b[d]", "a[d]");
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        dep_graph.has_cycle,
        "a genuine multi-variable element 2-cycle MUST still set \
         has_cycle (loud-safe: it is absent from the resolved-SCC map so \
         its back-edge still errs)"
    );
    assert!(
        dep_graph.resolved_sccs.is_empty(),
        "a genuine element 2-cycle resolves nothing"
    );
}

#[test]
fn model_dep_graph_genuine_scalar_two_cycle_stays_circular() {
    // LOUD-SAFE AC4 (load-bearing). `a=b+1; b=a+1` is a genuine scalar
    // 2-cycle. Unresolved by Task 4 => absent from the SCC map => its
    // cross back-edge still errs => `has_cycle == true`, empty
    // `resolved_sccs`.
    let project = single_model_project(vec![aux_var("a", "b + 1"), aux_var("b", "a + 1")]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        dep_graph.has_cycle,
        "a genuine scalar 2-cycle MUST still set has_cycle (loud-safe)"
    );
    assert!(
        dep_graph.resolved_sccs.is_empty(),
        "a genuine scalar 2-cycle resolves nothing"
    );
}

#[test]
fn model_dep_graph_acyclic_control_unaffected_by_scc_aware_break() {
    // An ordinary acyclic model: no back-edge ever fires, so the SCC-aware
    // generalization is inert -- `has_cycle == false`, `resolved_sccs`
    // empty, dt_dependencies the normal transitive closure. Pins that the
    // generalization does ZERO extra work / no behavior change on the
    // happy path.
    let project = single_model_project(vec![
        aux_var("a", "1"),
        aux_var("b", "a + 1"),
        aux_var("c", "a + b"),
    ]);
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(!dep_graph.has_cycle, "an acyclic model has no cycle");
    assert!(
        dep_graph.resolved_sccs.is_empty(),
        "an acyclic model resolves no SCC (zero refinement work)"
    );
    let c_deps = dep_graph.dt_dependencies.get("c").expect("c deps");
    assert!(
        c_deps.contains("a") && c_deps.contains("b"),
        "the normal transitive closure is unchanged (c={c_deps:?})"
    );
}

#[test]
fn model_dep_graph_single_var_self_recurrence_byte_identical_to_phase1() {
    // N=1 REGRESSION (byte-identical). The single-variable self-recurrence
    // is the 1-member-SCC case of the generalized mechanism. Its emitted
    // `resolved_sccs` / `element_order` / `has_cycle` from
    // `model_dependency_graph` MUST be byte-identical to the Phase 1
    // shape: exactly one `ResolvedScc { phase: Dt, members: {ecc},
    // element_order: [(ecc,0),(ecc,1),(ecc,2)] }`, `has_cycle == false`.
    // The existing Phase 1 guards
    // (`dt_cycle_sccs_resolved_self_recurrence_has_no_circular`,
    // `model_dependency_graph_resolved_sccs_is_byte_stable_across_runs`)
    // assert the same N=1 contract and must ALSO stay green unchanged.
    let project = TestProject::new("mdg_n1_byte_identical")
        .named_dimension("t", &["t1", "t2", "t3"])
        .array_with_ranges(
            "ecc[t]",
            vec![("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
        );
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "the N=1 self-recurrence (1-member SCC) must NOT set has_cycle"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one ResolvedScc (the 1-member {{ecc}} SCC)"
    );
    let scc = &dep_graph.resolved_sccs[0];
    assert_eq!(scc.phase, crate::db::SccPhase::Dt);
    assert_eq!(
        scc.members,
        [crate::common::Ident::new("ecc")]
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    assert_eq!(
        scc.element_order,
        vec![ecc(0), ecc(1), ecc(2)],
        "the N=1 element_order must be byte-identical to Phase 1 (the \
         1-member-SCC case must not change N=1 behavior)"
    );
}

// ── Element-level lagged-read strip (the C-LEARN compile blocker) ───────
//
// C-LEARN's `SAMPLE IF TRUE(cond,input,init)` expands
// (`mdl/xmile_compat.rs:358-366`) to
// `( IF cond THEN input ELSE PREVIOUS(SELF, init) )`. The hard-coded
// literal `SELF` is a *bare* `Var` inside `PREVIOUS()`, so the
// `self_allowed` Var arm (`builtins_visitor.rs`) un-rewrites it to the
// enclosing variable's bare name; arg0 is a bare non-module Var, so
// `previous_needs_temp_arg` is false and it stays `PREVIOUS(<self>, init)`
// -> a direct `LoadPrev` -> `SymLoadPrev` reading THIS element's own slot
// (verified by dumping the production symbolic fragment: the `sit_x[1]`
// segment contains `SymLoadPrev(sit_x[1])`). A *subscripted* self-ref
// (`PREVIOUS(x[e],..)`) would instead be temp-arg-rewritten into a
// synthetic helper whose read is a CURRENT-value edge -- a structurally
// different (and already-resolving) shape; the bare form is the genuine
// C-LEARN shape and the one this fixture uses.
//
// When such a variable sits in a genuinely identified multi-variable
// recurrence SCC (the un-lagged cross-member chain closes the cluster, so
// `resolve_recurrence_sccs` IS reached), the same-element `SymLoadPrev`
// is, before Part A, mis-collected by `symbolic_phase_element_order`'s
// read-opcode arm as a current-value edge -> a spurious
// `(member,e)->(member,e)` element self-loop -> `self_loop` -> `None` -> a
// false `CircularDependency` (the verified C-LEARN root cause: 105
// element-self-loops, every one a `SymLoadPrev`). After Part A the element
// graph inherits `build_var_info`'s per-phase PREVIOUS/INIT strip
// (`db/dep_graph.rs` real-var :261-264 / implicit :283-287; `SymLoadPrev`
// is the element-level analogue of `dt_previous_referenced_vars` /
// `initial_previous_referenced_vars`, stripped in BOTH phases), so the
// lagged self-read contributes no edge and the (acyclic, current-value)
// element graph resolves -- exactly as the variable-level relation already
// does for the whole-variable PREVIOUS self-edge.

/// A two-element `ref.mdl`-shaped project whose only stateful structure is
/// the minimized C-LEARN `SAMPLE IF TRUE` blocker: a 2-member
/// whole-variable recurrence SCC `{sit_x, closer}` where `sit_x` carries
/// the `SAMPLE IF TRUE`-expanded body with the hard-coded bare `SELF`
/// (rendered here as the bare self-name `PREVIOUS(sit_x, 0)`, exactly what
/// `SELF` un-rewrites to):
///
///   cond[t1]=1; cond[t2]=1                         (constant; NOT in SCC)
///   sit_x[t1] = cond[t1]                           (base element)
///   sit_x[t2] = IF cond[t2] THEN closer[t1] ELSE PREVIOUS(sit_x, 0)
///   closer[t1] = sit_x[t1] + 1
///   closer[t2] = sit_x[t2] + 1
///
/// Whole-variable `sit_x`<->`closer` is a genuine 2-cycle (`sit_x` reads
/// `closer` via the THEN branch; `closer` reads `sit_x`), so the SCC IS
/// identified. The production symbolic fragment for `sit_x` (verified) is
///   [LoadVar(cond[0]), AssignCurr(sit_x[0]),
///    LoadVar(closer[0]), LoadConstant, SymLoadPrev(sit_x[1]),
///      LoadVar(cond[1]), SetCond, If, AssignCurr(sit_x[1]), Ret]
/// and for `closer`
///   [LoadVar(sit_x[0]), .., BinOpAssignCurr(closer[0]),
///    LoadVar(sit_x[1]), .., BinOpAssignCurr(closer[1]), Ret].
/// The CURRENT-VALUE induced element graph is therefore
///   (sit_x,0) -> (closer,0); (sit_x,1) -> (closer,1);
///   (closer,0) -> (sit_x,1)
/// which is acyclic (a well-founded staggered recurrence). The ONLY thing
/// that makes it appear cyclic before Part A is the `SymLoadPrev(sit_x[1])`
/// SAME-element lagged self-read in the `sit_x[1]` segment being
/// mis-collected as a current-value edge -> spurious
/// `(sit_x,1)->(sit_x,1)` self-loop. After Part A that lagged read
/// contributes no edge and the SCC resolves in the per-element
/// topological order sit_x[0],closer[0],sit_x[1],closer[1].
fn sample_if_true_shaped_scc_project() -> TestProject {
    TestProject::new("sample_if_true_shaped_scc")
        .named_dimension("t", &["t1", "t2"])
        .array_with_ranges("cond[t]", vec![("t1", "1"), ("t2", "1")])
        .array_with_ranges(
            "sit_x[t]",
            vec![
                ("t1", "cond[t1]"),
                ("t2", "IF cond[t2] THEN closer[t1] ELSE PREVIOUS(sit_x, 0)"),
            ],
        )
        .array_with_ranges(
            "closer[t]",
            vec![("t1", "sit_x[t1] + 1"), ("t2", "sit_x[t2] + 1")],
        )
}

#[test]
fn resolve_dt_sample_if_true_shaped_scc_resolves_despite_previous_self_read() {
    use crate::db::SccPhase;

    // RED before Part A: the `SymLoadPrev(sit_x[1])` same-element lagged
    // self-read in the `sit_x[1]` segment is mis-collected as a
    // current-value element edge, minting a spurious
    // `(sit_x,1)->(sit_x,1)` self-loop, so `symbolic_phase_element_order`
    // returns `None` via the `self_loop` branch and
    // `resolve_recurrence_sccs` reports `has_unresolved` with no
    // `ResolvedScc` (the false C-LEARN `CircularDependency`). GREEN after
    // Part A: `SymLoadPrev` contributes no element edge, so the (acyclic,
    // current-value-only) element graph resolves.
    let project = sample_if_true_shaped_scc_project();
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(
        !res.has_unresolved,
        "the SAMPLE IF TRUE-shaped {{sit_x,closer}} SCC is element-acyclic \
         once the PREVIOUS same-element lagged self-read is excluded from \
         the element graph (it is a prior-timestep snapshot, not a \
         current-timestep ordering edge -- the element-level analogue of \
         `build_var_info` stripping `dt_previous_referenced_vars`): there \
         MUST be no unresolved SCC"
    );
    assert_eq!(
        res.resolved.len(),
        1,
        "exactly one resolved SCC (the {{sit_x,closer}} cluster)"
    );
    let scc = &res.resolved[0];
    assert_eq!(scc.phase, SccPhase::Dt);
    assert_eq!(
        scc.members,
        [
            crate::common::Ident::new("closer"),
            crate::common::Ident::new("sit_x"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "the resolved SCC's members are exactly {{sit_x,closer}}"
    );
    // Current-value element edges (PREVIOUS self-read excluded):
    // (sit_x,0)->(closer,0); (sit_x,1)->(closer,1); (closer,0)->(sit_x,1).
    // Only (sit_x,0) has indegree 0. Kahn drains
    // sit_x[0] -> closer[0] -> sit_x[1] -> closer[1] (the unique order;
    // the encoded-key tie-break never has to choose -- the frontier is a
    // single node at every step).
    let closer = |i: usize| (crate::common::Ident::new("closer"), i);
    let sit_x = |i: usize| (crate::common::Ident::new("sit_x"), i);
    assert_eq!(
        scc.element_order,
        vec![sit_x(0), closer(0), sit_x(1), closer(1)],
        "element_order must be the per-element topological order with the \
         PREVIOUS same-element lagged self-read excluded (a spurious \
         self-loop would have produced `None`/unresolved instead)"
    );

    // End-to-end: the SCC survives the production dependency-graph gate
    // with no `CircularDependency` (the C-LEARN blocker, minimized).
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "the SAMPLE IF TRUE-shaped SCC must NOT set has_cycle once the \
         element-level lagged-read strip lands (the false C-LEARN \
         CircularDependency)"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one ResolvedScc survives the gate"
    );
}

// ── Self-loop subsumed by a multi-member SCC (the true C-LEARN blocker) ─
//
// `resolve_recurrence_sccs` builds `self_loops` from a *direct* whole-
// variable self-edge `v -> v` INDEPENDENTLY of the `scc_components`
// partition. A variable that BOTH self-references on a current path AND
// participates in a >= 2 SCC via cross-member edges lands in `multi` (its
// >= 2 component) *and* in `self_loops`. Tarjan does NOT make it a
// standalone size-1 SCC -- its self-edge is an intra-SCC edge of the
// larger SCC, already evaluated in that SCC's verified per-element
// `element_order`. Before the disjointness filter, `resolve_recurrence_sccs`
// emitted TWO overlapping `ResolvedScc`s (the >= 2 SCC and the bogus
// 1-member self-loop SCC). `scc_map_from_resolved`'s last-write-wins
// `map.insert` then remapped the shared member to the 1-member SCC's id,
// so `same_resolved_scc` no longer suppressed the genuine intra-cluster
// back-edges incident to it, and `model_dependency_graph` reported a
// false residual `CircularDependency` with `resolved_sccs` cleared. This
// is C-LEARN's actual compile blocker (`emissions_with_cumulative_constraints`
// is exactly this shape), unmasked once the element-level lagged-read
// strip let the 22-member SCC's element graph resolve. This fixture is
// that shape minimized -- WITHOUT PREVIOUS, isolating the disjointness
// bug from the lagged-read strip.

/// A two-element project with a 2-member recurrence SCC `{a, b}` where `a`
/// ALSO has a direct whole-variable self-edge (a current self-reference),
/// so `a` is in BOTH the `multi` >= 2 SCC and the `self_loops` set:
///
///   a[t1] = 1
///   a[t2] = a[t1] + b[t1]      (current self-ref `a[t1]` -> a in dt_deps(a);
///                               cross-member `b[t1]` closes the cluster)
///   b[t1] = a[t1] + 1
///   b[t2] = a[t2] + 1
///
/// Whole-variable `a`<->`b` is a 2-cycle (`a` reads `b`, `b` reads `a`) so
/// `{a,b}` is a `multi` SCC; `a` additionally has the direct self-edge
/// `a -> a` (from `a[t2] = a[t1] + ...`) so it is also a `self_loops`
/// entry. The current-value induced element graph
///   (a,0)->(a,1); (b,0)->(a,1); (a,0)->(b,0); (a,1)->(b,1)
/// is acyclic, so the `{a,b}` SCC resolves. The bug is purely the
/// double-emission of `a` as a separate 1-member self-loop SCC corrupting
/// `scc_map_from_resolved`.
fn self_loop_inside_multi_scc_project() -> TestProject {
    TestProject::new("self_loop_inside_multi_scc")
        .named_dimension("t", &["t1", "t2"])
        .array_with_ranges("a[t]", vec![("t1", "1"), ("t2", "a[t1] + b[t1]")])
        .array_with_ranges("b[t]", vec![("t1", "a[t1] + 1"), ("t2", "a[t2] + 1")])
}

#[test]
fn resolve_dt_self_loop_subsumed_by_multi_scc_resolves_no_duplicate() {
    use crate::db::SccPhase;

    // RED before the disjointness filter: `a` is emitted as BOTH a member
    // of the resolved 2-member `{a,b}` SCC and a separate resolved
    // 1-member `{a}` self-loop SCC, so `res.resolved.len() == 2` with a
    // shared member, `scc_map_from_resolved` corrupts, and
    // `model_dependency_graph` reports `has_cycle == true` with
    // `resolved_sccs` cleared (the false C-LEARN residual
    // `CircularDependency`). GREEN after: `a` is filtered out of
    // `self_loops` because it is already a `multi` member, so exactly one
    // `ResolvedScc` `{a,b}` is produced and the gate resolves it.
    let project = self_loop_inside_multi_scc_project();
    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &dm);
    let model = result.models["main"].source;

    let res = resolve_recurrence_sccs(&db, model, result.project, SccPhase::Dt);
    assert!(
        !res.has_unresolved,
        "the {{a,b}} SCC is element-acyclic and element-sourceable: no \
         unresolved SCC"
    );
    assert_eq!(
        res.resolved.len(),
        1,
        "EXACTLY ONE resolved SCC: the 2-member {{a,b}} cluster. `a`'s \
         direct self-edge is an intra-SCC edge of {{a,b}}, NOT a separate \
         1-member self-loop SCC -- emitting both double-resolves `a` and \
         corrupts the pairwise-disjoint invariant `scc_map_from_resolved` \
         relies on"
    );
    let scc = &res.resolved[0];
    assert_eq!(scc.phase, SccPhase::Dt);
    assert_eq!(
        scc.members,
        [
            crate::common::Ident::new("a"),
            crate::common::Ident::new("b"),
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "the single resolved SCC's members are exactly {{a,b}}"
    );

    // End-to-end: the production gate resolves it with NO residual
    // `CircularDependency` (the minimized C-LEARN blocker).
    let dep_graph = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    assert!(
        !dep_graph.has_cycle,
        "a self-edge subsumed by a resolved multi-member SCC must NOT \
         produce a residual CircularDependency: the >= 2 SCC's combined \
         per-element fragment already evaluates `a`'s self-edge in the \
         verified element_order (the C-LEARN blocker's true root cause)"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly one ResolvedScc survives the gate (no duplicate, no \
         scc_map corruption)"
    );
}

// ── Initials runlist ordering determinism (GH #595) ─────────────────────
//
// The Initials runlist is built by `topo_sort_str(init_list, ..)` where
// `init_list` is the set of init-phase variables (stocks, modules,
// INIT()-referenced vars, the empty-dt/non-empty-init `INITIAL()`-backed
// vars, plus their transitive init deps). `topo_sort_str` emits names in
// the *visit order of its `names` argument*, breaking ties (variables with
// no ordering dependency between them) by that argument's order. If
// `init_list` is materialized from a `HashSet` in iteration order, the
// runlist becomes HashMap-RandomState dependent: two compiles of the SAME
// model produce DIFFERENT init orderings, so a `PREVIOUS()`/`INITIAL()`
// variable can be evaluated before or after an unrelated init helper,
// yielding nondeterministic initial values (the Flows and Stocks runlists
// filter the pre-sorted `var_names`, so they were already deterministic).
//
// The fix: `init_list` must be a deterministic function of the model
// alone. The flows/stocks runlists achieve this by filtering the sorted
// `var_names`; the initials runlist must likewise sort its names before
// `topo_sort_str`. These tests pin that property without relying on
// probability: every fresh `SimlinDb` (fresh per-HashMap RandomState
// seeds) must produce a BYTE-IDENTICAL `runlist_initials`, and the order
// must equal the topological sort with a stable (sorted-name) tie-break.

/// A model whose Initials runlist contains many variables with NO ordering
/// dependency between them: independent constant-init stocks, an
/// `INITIAL()`-backed aux that pins several constants into the init runlist,
/// and a `PREVIOUS()` aux (whose lagged dep is stripped from both phases, so
/// it too is unordered relative to its input). With this many unordered init
/// nodes a HashMap-iteration-order-dependent `init_list` essentially never
/// repeats the same order across fresh databases.
fn init_runlist_determinism_fixture() -> datamodel::Project {
    single_model_project(vec![
        aux_var("zeta", "1"),
        aux_var("yankee", "2"),
        aux_var("xray", "3"),
        aux_var("whiskey", "4"),
        aux_var("victor", "5"),
        aux_var("uniform", "6"),
        // `INITIAL()` pins each referenced constant into the initials runlist
        // (an `INIT()`-referenced var) with no ordering edge between them.
        aux_var(
            "init_sum",
            "INIT(zeta) + INIT(yankee) + INIT(xray) + INIT(whiskey) + INIT(victor) + INIT(uniform)",
        ),
        // A PREVIOUS aux: its lagged input dep is stripped from dt AND init
        // deps, so `prev_alpha` is unordered relative to `victor` in the init
        // runlist -- the exact symptom shape of C-LEARN's
        // `previous_emissions_for_rate`.
        aux_var("prev_alpha", "PREVIOUS(victor, 0)"),
        datamodel::Variable::Stock(datamodel::Stock {
            ident: "stock_bravo".to_string(),
            equation: datamodel::Equation::Scalar("10".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
        datamodel::Variable::Stock(datamodel::Stock {
            ident: "stock_charlie".to_string(),
            equation: datamodel::Equation::Scalar("20".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
        datamodel::Variable::Stock(datamodel::Stock {
            ident: "stock_delta".to_string(),
            equation: datamodel::Equation::Scalar("30".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
    ])
}

/// Build `runlist_initials` for `init_runlist_determinism_fixture` from a
/// freshly-seeded database.
fn init_runlist_once(project: &datamodel::Project) -> Vec<String> {
    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, project);
    let model = result.models["main"].source;
    let dep = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    dep.runlist_initials.clone()
}

#[test]
fn initials_runlist_is_deterministic_across_fresh_databases() {
    // Each `SimlinDb::default()` gets fresh per-HashMap RandomState seeds. A
    // HashMap-iteration-order-dependent `init_list` would yield different
    // runlist orders across these builds; a deterministic build yields one.
    let project = init_runlist_determinism_fixture();
    let baseline = init_runlist_once(&project);
    // Sanity: the fixture must actually populate the initials runlist with
    // many independent nodes, else the test could pass vacuously.
    assert!(
        baseline.len() >= 8,
        "fixture must put many independent vars in the initials runlist \
         (got {}): {baseline:?}",
        baseline.len()
    );
    for i in 0..32 {
        let again = init_runlist_once(&project);
        assert_eq!(
            baseline, again,
            "runlist_initials must be a deterministic function of the model \
             (fresh-database build {i} diverged -- HashMap-iteration-order \
             dependence reintroduced in the initials runlist build)"
        );
    }
}

#[test]
fn initials_runlist_is_sorted_topological_order() {
    // Pin the deterministic order directly (not just self-consistency): the
    // initials runlist must equal a stable topological sort with a
    // sorted-name tie-break. We reconstruct that reference order from the
    // engine's own `initial_dependencies` so the assertion tracks the real
    // dependency edges rather than a hard-coded list, and confirm the engine
    // emits exactly it. A HashMap-order-dependent build would (almost surely)
    // disagree with this canonical order.
    let project = init_runlist_determinism_fixture();

    let db = SimlinDb::default();
    let result = sync_from_datamodel(&db, &project);
    let model = result.models["main"].source;
    let dep = crate::db::model_dependency_graph(
        &db,
        model,
        result.project,
        crate::db::ModuleInputSet::empty(&db),
    );
    let actual = dep.runlist_initials.clone();

    // Reference: visit the runlist's members in sorted order, emitting each
    // member's (sorted) init deps that are themselves in the runlist before
    // the member -- a stable Kahn-style topological order with an alphabetical
    // tie-break. This mirrors `topo_sort_str` fed a *sorted* `names` list.
    let member_set: std::collections::BTreeSet<String> = actual.iter().cloned().collect();
    let mut expected: Vec<String> = Vec::new();
    let mut emitted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // ASSUMES this fixture's initials runlist is acyclic (no *resolved* init
    // SCC among the members): a plain DFS post-order matches `topo_sort_str`
    // only on a DAG. A resolved init SCC would be emitted SCC-contiguous at the
    // SCC's slot by `topo_sort_str`'s condensation, and this reference
    // reconstruction would diverge -- such a fixture needs SCC-aware expected
    // order, not this helper.
    fn emit(
        name: &str,
        deps: &std::collections::HashMap<
            Ident<Canonical>,
            std::collections::BTreeSet<Ident<Canonical>>,
        >,
        members: &std::collections::BTreeSet<String>,
        emitted: &mut std::collections::BTreeSet<String>,
        out: &mut Vec<String>,
    ) {
        if emitted.contains(name) {
            return;
        }
        emitted.insert(name.to_string());
        // `deps` is interned-keyed now; probe by `&str` (Borrow) and recurse
        // on each dep's `as_str()`. Behavior is unchanged from the former
        // `String`-keyed map (same `BTreeSet` lexicographic dep order).
        if let Some(ds) = deps.get(name) {
            for d in ds.iter() {
                if members.contains(d.as_str()) {
                    emit(d.as_str(), deps, members, emitted, out);
                }
            }
        }
        if members.contains(name) {
            out.push(name.to_string());
        }
    }
    let mut sorted_members: Vec<&String> = member_set.iter().collect();
    sorted_members.sort();
    for m in sorted_members {
        emit(
            m,
            &dep.initial_dependencies,
            &member_set,
            &mut emitted,
            &mut expected,
        );
    }

    assert_eq!(
        actual, expected,
        "runlist_initials must be the stable (sorted-name tie-break) \
         topological order; a divergence means the initials runlist build \
         is not sorting its candidate set before topo_sort_str"
    );
}
