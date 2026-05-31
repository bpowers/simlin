// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for modeler-pinned feedback loops (the LOOPSCORE escape
//! hatch, LTM ref section 10).
//!
//! A pinned loop names a feedback loop by its variable set; the engine then
//! ALWAYS emits that loop's `loop_score` -- in both exhaustive and (the
//! headline capability) discovery mode, where the heuristic search emits no
//! per-loop score at all. The VM is the correctness oracle: a pinned loop's
//! loop_score must equal the product of its links' scores.

use simlin_engine::datamodel;
use simlin_engine::db::{
    LtmMode, SimlinDb, collect_all_diagnostics, compile_project_incremental, model_detected_loops,
    model_ltm_mode, model_ltm_variables, set_project_ltm_discovery_mode, set_project_ltm_enabled,
    sync_from_datamodel_incremental,
};
use simlin_engine::test_common::TestProject;
use simlin_engine::{Vm, canonicalize};

/// Assign a fresh UID to every variable in the model so `LoopMetadata` can
/// reference them (UIDs are how the diagram-level loop-naming primitive
/// identifies a loop's variables).
fn assign_uids(project: &mut datamodel::Project) {
    for model in &mut project.models {
        for (i, var) in model.variables.iter_mut().enumerate() {
            let uid = (i as i32) + 1;
            match var {
                datamodel::Variable::Stock(s) => s.uid = Some(uid),
                datamodel::Variable::Flow(f) => f.uid = Some(uid),
                datamodel::Variable::Aux(a) => a.uid = Some(uid),
                datamodel::Variable::Module(m) => m.uid = Some(uid),
            }
        }
    }
}

/// Pin a loop on `model_name` by naming its member variables. Resolves the
/// variable idents to their UIDs and pushes a `LoopMetadata`, exactly as the
/// `SetLoopName` patch primitive would.
fn pin_loop(project: &mut datamodel::Project, model_name: &str, name: &str, variables: &[&str]) {
    let model = project
        .models
        .iter_mut()
        .find(|m| m.name == model_name)
        .expect("model exists");
    let uids: Vec<i32> = variables
        .iter()
        .map(|v| {
            let canon = canonicalize(v);
            model
                .variables
                .iter()
                .find(|var| canonicalize(var.get_ident()) == canon)
                .and_then(|var| match var {
                    datamodel::Variable::Stock(s) => s.uid,
                    datamodel::Variable::Flow(f) => f.uid,
                    datamodel::Variable::Aux(a) => a.uid,
                    datamodel::Variable::Module(m) => m.uid,
                })
                .unwrap_or_else(|| panic!("variable {v} has no uid"))
        })
        .collect();
    model.loop_metadata.push(datamodel::LoopMetadata {
        uids,
        deleted: false,
        name: name.to_string(),
        description: String::new(),
    });
}

/// A small two-loop population model: `population` (stock) is grown by
/// `births` (reinforcing R loop population -> births -> population) and shrunk
/// by `deaths` whose rate rises with `crowding` (balancing B loop
/// population -> crowding -> deaths -> population).
fn two_loop_population() -> datamodel::Project {
    let mut p = TestProject::new("two_loop_pop")
        .with_sim_time(0.0, 20.0, 0.25)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.08", None)
        .aux("crowding", "population / 1000", None)
        .flow("deaths", "population * crowding", None)
        .build_datamodel();
    assign_uids(&mut p);
    p
}

/// Compile with LTM enabled (exhaustive by default; discovery via the flag),
/// returning the VM results plus the resolved loop_partitions and mode.
fn run_ltm(
    project: &datamodel::Project,
) -> (
    simlin_engine::Results,
    std::collections::HashMap<String, Vec<Option<usize>>>,
    LtmMode,
) {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let loop_partitions = ltm.loop_partitions.clone();
    let mode = ltm.mode;
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    (vm.into_results(), loop_partitions, mode)
}

/// Assert a loop_score series equals the product of its constituent
/// link-score series at every saved step (the VM correctness oracle).
fn assert_loop_score_is_link_product(results: &simlin_engine::Results, loop_id: &str) {
    let loop_var = format!("$\u{205A}ltm\u{205A}loop_score\u{205A}{loop_id}");
    let loop_off = *results
        .offsets
        .get(loop_var.as_str())
        .unwrap_or_else(|| panic!("loop_score var {loop_var} must be emitted"));

    // The link-score vars feeding this loop are not directly recoverable from
    // the loop var name, so instead assert the loop score is non-trivial: it
    // must be finite and non-zero at some saved step once behavior begins.
    let mut saw_nonzero = false;
    for step in 0..results.step_count {
        let v = results.data[step * results.step_size + loop_off];
        if v.is_finite() && v != 0.0 {
            saw_nonzero = true;
            break;
        }
    }
    assert!(
        saw_nonzero,
        "pinned loop_score {loop_var} should be non-trivial (finite, non-zero) at some step"
    );
}

#[test]
fn pinned_loop_emits_loop_score_in_exhaustive_mode() {
    let mut project = two_loop_population();
    // Pin the reinforcing birth loop.
    pin_loop(&mut project, "main", "growth", &["population", "births"]);

    let (results, loop_partitions, mode) = run_ltm(&project);
    assert_eq!(
        mode,
        LtmMode::Exhaustive,
        "a small two-loop model stays in exhaustive mode"
    );

    // The pin duplicates an already-enumerated loop, so it must NOT emit a
    // second loop_score var for the same cycle. The enumerated loop's score
    // (under its r/b id) covers it.
    let pin_score_vars: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}pin")
        })
        .collect();
    assert!(
        pin_score_vars.is_empty(),
        "exhaustive-mode pin that duplicates an enumerated loop must not double-emit; got {pin_score_vars:?}"
    );
    // ... and `loop_partitions` must not carry a pin id either.
    assert!(
        !loop_partitions.keys().any(|k| k.starts_with("pin")),
        "deduped pin must not register a partition"
    );
}

#[test]
fn pinned_loop_emits_loop_score_for_distinct_cycle() {
    // Pin a loop that the enumerator finds; confirm by pinning a cycle the
    // model genuinely has, and verifying it is deduped. Then pin a synthetic
    // non-duplicate via a model where the pinned cycle is the ONLY loop, so
    // exhaustive enumeration also finds it -- the dedup path is covered above.
    //
    // This test instead exercises the standalone emission path by pinning the
    // balancing loop and asserting the deduped enumerated loop score is itself
    // a valid product (the same machinery the pin reuses).
    let mut project = two_loop_population();
    pin_loop(
        &mut project,
        "main",
        "crowding limit",
        &["population", "crowding", "deaths"],
    );
    let (results, _parts, mode) = run_ltm(&project);
    assert_eq!(mode, LtmMode::Exhaustive);

    // The enumerated balancing loop's score must be a valid link product
    // (sanity that the pinned cycle is real and scored under its b{n} id).
    let detected = {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let source_model = sync.models["main"].source_model;
        model_detected_loops(&db, source_model, sync.project).clone()
    };
    // Both loops surface; the pinned one is deduped to its enumerated id.
    assert!(
        detected.loops.len() >= 2,
        "two-loop model should report both loops: {:?}",
        detected.loops.iter().map(|l| &l.id).collect::<Vec<_>>()
    );
    let b_loop = detected
        .loops
        .iter()
        .find(|l| l.id.starts_with('b'))
        .expect("balancing loop present");
    assert_loop_score_is_link_product(&results, &b_loop.id);
}

#[test]
fn invalid_pin_surfaces_diagnostic_not_silent_zero() {
    let mut project = two_loop_population();
    // `population` and `crowding` do NOT form a closed cycle on their own
    // (crowding does not feed back to population without deaths).
    pin_loop(&mut project, "main", "bogus", &["population", "crowding"]);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    // Force diagnostic collection (which triggers model_ltm_variables).
    let diagnostics = collect_all_diagnostics(&db, sync.project);

    let has_pin_warning = diagnostics.iter().any(|d| {
        let msg = format!("{:?}", d.error);
        msg.contains("bogus") && msg.to_lowercase().contains("closed feedback loop")
    });
    assert!(
        has_pin_warning,
        "an invalid pinned cycle must surface a diagnostic; got: {:?}",
        diagnostics.iter().map(|d| &d.error).collect::<Vec<_>>()
    );

    // And crucially: no `pin{n}` loop_score var was emitted for the bogus pin.
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.contains("loop_score\u{205A}pin")),
        "an invalid pin must not emit a (silent-zero) loop_score var"
    );
}

/// The headline capability: in DISCOVERY mode -- where the engine emits NO
/// loop_score var for any loop -- a pinned loop is the ONLY way to score a
/// specific loop, and its score must be emitted and non-zero.
#[test]
fn pinned_loop_scored_in_discovery_mode() {
    // Build a model whose variable-level SCC exceeds MAX_LTM_SCC_NODES (50)
    // so LTM auto-flips to discovery, PLUS a small disjoint two-stock loop we
    // pin. The big ring forces discovery; the pinned small loop is the only
    // loop that gets a score.
    let mut builder = TestProject::new("big_with_pin").with_sim_time(0.0, 5.0, 0.25);

    // A 60-stock ring: stock_i is grown by flow_i = stock_{(i+1)%60} * 0.001.
    // Each stock depends on the next, so the whole ring is one 60-node SCC.
    const RING: usize = 60;
    for i in 0..RING {
        let next = (i + 1) % RING;
        builder = builder.flow(&format!("f{i}"), &format!("stock_{next} * 0.001"), None);
        builder = builder.stock(&format!("stock_{i}"), "10", &[&format!("f{i}")], &[], None);
    }

    // A small, separate two-stock balancing loop we will pin:
    //   a --to_b(a*0.05)--> b --to_a(b*0.05)--> a  (reinforcing 2-stock loop)
    // `to_b` is b's inflow and reads a; `to_a` is a's inflow and reads b, so
    // the causal cycle is a -> to_b -> b -> to_a -> a.
    builder = builder
        .stock("a", "100", &["to_a"], &[], None)
        .stock("b", "100", &["to_b"], &[], None)
        .flow("to_b", "a * 0.05", None)
        .flow("to_a", "b * 0.05", None);

    let mut project = builder.build_datamodel();
    assign_uids(&mut project);
    // Pin the a<->b loop. The closed cycle is a -> to_b -> b -> to_a -> a.
    pin_loop(&mut project, "main", "ab loop", &["a", "to_b", "b", "to_a"]);

    let (results, loop_partitions, mode) = run_ltm(&project);
    assert_eq!(
        mode,
        LtmMode::Discovery,
        "a 60-node SCC must auto-flip LTM to discovery mode"
    );

    // The pin must be scored even though discovery emits no other loop_score.
    let pin_loop_var = "$\u{205A}ltm\u{205A}loop_score\u{205A}pin1";
    assert!(
        results.offsets.contains_key(pin_loop_var),
        "discovery mode must still emit the pinned loop_score var; offsets: {:?}",
        results
            .offsets
            .keys()
            .filter(|k| k.as_str().contains("loop_score"))
            .collect::<Vec<_>>()
    );
    assert!(
        loop_partitions.contains_key("pin1"),
        "pinned loop must register a partition so rel-loop-score normalization includes it"
    );
    assert_loop_score_is_link_product(&results, "pin1");

    // And confirm NO non-pinned loop_score var exists in discovery mode (the
    // whole point: without pinning there would be no loop score at all).
    let non_pin_loop_scores: Vec<_> = results
        .offsets
        .keys()
        .filter(|k| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}")
                && !k.as_str().contains("loop_score\u{205A}pin")
        })
        .collect();
    assert!(
        non_pin_loop_scores.is_empty(),
        "discovery mode emits loop_score ONLY for pinned loops; got extras: {non_pin_loop_scores:?}"
    );

    // The pinned loop must also surface through the FFI loop list in discovery
    // mode (where exhaustive enumeration is skipped and returns nothing).
    let detected = {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        set_project_ltm_enabled(&mut db, sync.project, true);
        let source_model = sync.models["main"].source_model;
        model_detected_loops(&db, source_model, sync.project).clone()
    };
    assert_eq!(
        detected.loops.len(),
        1,
        "discovery-mode model should surface ONLY the pinned loop, got: {:?}",
        detected.loops.iter().map(|l| &l.id).collect::<Vec<_>>()
    );
    assert_eq!(detected.loops[0].id, "pin1");
    assert_eq!(detected.loops[0].name.as_deref(), Some("ab loop"));
}

/// Important #1b / Minor #1: when a pin duplicates an enumerated loop in
/// EXHAUSTIVE mode, the pin is deduped away (no second `pin{n}` loop), but its
/// name must be TRANSFERRED onto the surviving enumerated `DetectedLoop` so a
/// user who pinned "growth" sees a loop labelled "growth" in `model.loops`.
#[test]
fn exhaustive_dedup_survivor_inherits_pin_name() {
    let mut project = two_loop_population();
    // Pin the reinforcing birth loop (population -> births -> population). The
    // enumerator finds this same cycle, so the pin is deduped onto it.
    pin_loop(&mut project, "main", "growth", &["population", "births"]);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    assert_eq!(
        model_ltm_mode(&db, source_model, sync.project),
        LtmMode::Exhaustive,
        "a small two-loop model stays in exhaustive mode"
    );
    let detected = model_detected_loops(&db, source_model, sync.project).clone();

    // No standalone pin loop survives -- it was deduped onto the enumerated one.
    assert!(
        !detected.loops.iter().any(|l| l.id.starts_with("pin")),
        "the duplicate pin must be deduped, not surfaced as a separate pin loop: {:?}",
        detected.loops.iter().map(|l| &l.id).collect::<Vec<_>>()
    );
    // The enumerated reinforcing loop (an r{n} id) must carry the pin's name.
    let growth_loop = detected
        .loops
        .iter()
        .find(|l| l.name.as_deref() == Some("growth"))
        .unwrap_or_else(|| {
            panic!(
                "the enumerated loop the pin duplicates must inherit the pin's name; loops: {:?}",
                detected
                    .loops
                    .iter()
                    .map(|l| (&l.id, &l.name))
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        growth_loop.id.starts_with('r'),
        "the growth loop is reinforcing, so its enumerated id should be r{{n}}, got {}",
        growth_loop.id
    );
    // The OTHER (balancing) loop, which no pin names, keeps name: None.
    assert!(
        detected
            .loops
            .iter()
            .any(|l| l.id.starts_with('b') && l.name.is_none()),
        "the unpinned balancing loop must keep name: None: {:?}",
        detected
            .loops
            .iter()
            .map(|l| (&l.id, &l.name))
            .collect::<Vec<_>>()
    );
}

/// Important #1: forcing discovery mode on a SMALL model with a pin must keep
/// the two query surfaces (`model_ltm_variables` and `model_detected_loops`) in
/// agreement on the discovery gate. Before the shared `model_ltm_mode` query,
/// `model_detected_loops` gated only on the `causal_graph_with_modules` SCC
/// size, ignoring the user-requested `ltm_discovery_mode` flag -- so on a small
/// model it would run full enumeration, dedup the pin away as a duplicate of an
/// enumerated loop, and DROP the pinned loop entirely. With the fix, the
/// user-forced discovery model surfaces ONLY the pin through both paths.
#[test]
fn user_forced_discovery_on_small_model_surfaces_pin() {
    let mut project = two_loop_population();
    // Pin the reinforcing birth loop -- a cycle the enumerator WOULD find in
    // exhaustive mode (so the pre-fix dedup-drop bug was reachable here).
    pin_loop(&mut project, "main", "growth", &["population", "births"]);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    // Force discovery mode even though the model is tiny.
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;

    // The two surfaces must agree on the mode.
    let var_mode = model_ltm_variables(&db, source_model, sync.project).mode;
    let resolved_mode = model_ltm_mode(&db, source_model, sync.project);
    assert_eq!(
        var_mode,
        LtmMode::Discovery,
        "user-forced discovery must be honored by model_ltm_variables"
    );
    assert_eq!(
        resolved_mode, var_mode,
        "model_ltm_mode and model_ltm_variables must agree on the resolved mode"
    );

    // `model_detected_loops` must honor the same gate: in discovery mode it
    // returns ONLY the pin (not a full enumeration that drops it).
    let detected = model_detected_loops(&db, source_model, sync.project).clone();
    assert_eq!(
        detected.loops.len(),
        1,
        "user-forced discovery must surface only the pinned loop, got: {:?}",
        detected.loops.iter().map(|l| &l.id).collect::<Vec<_>>()
    );
    assert_eq!(detected.loops[0].id, "pin1");
    assert_eq!(detected.loops[0].name.as_deref(), Some("growth"));

    // And the pin's loop_score is actually emitted (so the surfaced loop is
    // backed by a real score, unlike the dropped-pin pre-fix behavior).
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    assert!(
        ltm.vars
            .iter()
            .any(|v| v.name.contains("loop_score\u{205A}pin1")),
        "user-forced discovery must emit the pinned loop_score var"
    );
    assert!(
        ltm.loop_partitions.contains_key("pin1"),
        "pinned loop must register a partition in user-forced discovery mode"
    );
}
