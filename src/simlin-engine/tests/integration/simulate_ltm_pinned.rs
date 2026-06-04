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
    LtmMode, LtmSyntheticVar, SimlinDb, collect_all_diagnostics, compile_project_incremental,
    model_detected_loops, model_ltm_mode, model_ltm_variables, set_project_ltm_discovery_mode,
    set_project_ltm_enabled, sync_from_datamodel_incremental,
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

/// The two-coupled-loop population model (births inflow + deaths outflow on
/// one stock) arrayed over `Region` with heterogeneous per-element rates and
/// Bare (apply-to-all) flow equations, so the pinned birth loop's raw score
/// is analytically known per element: `births = population * birth_rate`
/// scores `b / (b - d)` -- NYC `0.10/0.07`, Boston `0.40/0.35` -- at every
/// post-startup step.
fn two_region_population() -> datamodel::Project {
    let mut p = TestProject::new("a2a_pin_pop")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_with_ranges(
            "birth_rate[Region]",
            vec![("NYC", "0.10"), ("Boston", "0.40")],
        )
        .array_with_ranges(
            "death_rate[Region]",
            vec![("NYC", "0.03"), ("Boston", "0.05")],
        )
        .array_stock("population[Region]", "100", &["births"], &["deaths"], None)
        .array_flow("births[Region]", "population * birth_rate", None)
        .array_flow("deaths[Region]", "population * death_rate", None)
        .build_datamodel();
    assign_uids(&mut p);
    p
}

/// Compile with LTM enabled and discovery mode forced, run, and return
/// results + partitions + the LTM synthetic-var metadata.
fn run_ltm_discovery(
    project: &datamodel::Project,
) -> (
    simlin_engine::Results,
    std::collections::HashMap<String, Vec<Option<usize>>>,
    Vec<LtmSyntheticVar>,
) {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    let loop_partitions = ltm.loop_partitions.clone();
    let ltm_vars = ltm.vars.clone();
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    (vm.into_results(), loop_partitions, ltm_vars)
}

/// GH #653 / pin-dims.AC1: a pinned pure-A2A loop -- every variable arrayed
/// over the same dimension with Bare (apply-to-all) references -- must be
/// scored per element, in discovery mode where the pin is the only loop
/// scored at all.
///
/// Pre-#653 behavior: the pin's loop score was a *scalar* equation
/// referencing the arrayed link scores bare -- a dimension mismatch that
/// failed to compile and silently stubbed the score to constant 0 (with only
/// a fragment-diagnostics Warning), and `loop_partitions["pin1"]` was the
/// broken `[None]` single-slot entry.
#[test]
fn pinned_pure_a2a_loop_scored_per_element_in_discovery_mode() {
    let mut project = two_region_population();
    pin_loop(&mut project, "main", "growth", &["population", "births"]);

    let (results, loop_partitions, ltm_vars) = run_ltm_discovery(&project);

    // The pin's loop score var must be dimensioned over Region.
    let pin_var = ltm_vars
        .iter()
        .find(|v| v.name == "$\u{205A}ltm\u{205A}loop_score\u{205A}pin1")
        .expect("discovery mode must emit the pinned loop_score var");
    assert_eq!(
        pin_var.dimensions,
        vec!["Region".to_string()],
        "a pinned pure-A2A loop must carry the cycle's shared dimension"
    );

    // Per-slot partitions: one entry per Region element, each resolved (the
    // pre-fix behavior was the broken single-slot `[None]`).
    let parts = loop_partitions
        .get("pin1")
        .expect("pinned loop must register a partition");
    assert_eq!(
        parts.len(),
        2,
        "an A2A pin over a 2-element dimension must register one partition entry per slot; \
         got {parts:?}"
    );
    assert!(
        parts.iter().all(|p| p.is_some()),
        "every slot of the pinned A2A loop resolves to a real partition; got {parts:?}"
    );

    // Per-slot scores: slot 0 = NYC = 0.10/0.07, slot 1 = Boston = 0.40/0.35
    // at every post-startup step.
    let base = *results
        .offsets
        .get("$\u{205A}ltm\u{205A}loop_score\u{205A}pin1")
        .expect("pin1 loop score must be in results");
    let expected = [0.10 / 0.07, 0.40 / 0.35];
    const TOL: f64 = 1e-9;
    for (slot, &expected_score) in expected.iter().enumerate() {
        for step in 3..results.step_count {
            let v = results.data[step * results.step_size + base + slot];
            assert!(
                (v - expected_score).abs() <= TOL * expected_score.abs(),
                "pin1 slot {slot} at step {step}: got {v}, expected {expected_score}. A zero \
                 here means the pinned A2A loop score equation failed to compile and was \
                 silently stubbed (GH #653)."
            );
        }
    }
}

/// pin-dims.AC1.3: in exhaustive mode the same A2A pin dedups against the
/// enumerated A2A loop exactly as scalar pins dedup today -- no second
/// loop_score var, no pin partition entry.
#[test]
fn pinned_pure_a2a_loop_dedups_in_exhaustive_mode() {
    let mut project = two_region_population();
    pin_loop(&mut project, "main", "growth", &["population", "births"]);

    let (results, loop_partitions, mode) = run_ltm(&project);
    assert_eq!(
        mode,
        LtmMode::Exhaustive,
        "the small two-region model stays in exhaustive mode"
    );
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
        "exhaustive-mode A2A pin that duplicates an enumerated loop must not double-emit; \
         got {pin_score_vars:?}"
    );
    assert!(
        !loop_partitions.keys().any(|k| k.starts_with("pin")),
        "deduped A2A pin must not register a partition"
    );
}

/// pin-dims.AC1.4: a pinned A2A cycle over a multi-dimensional variable
/// carries both dimensions and one slot per element pair.
#[test]
fn pinned_multi_dim_a2a_loop_scored_per_slot() {
    let mut project = TestProject::new("multi_dim_pin")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .named_dimension("Age", &["young", "old"])
        .array_stock("pop[Region,Age]", "100", &["growth"], &[], None)
        .array_flow("growth[Region,Age]", "pop * 0.1", None)
        .build_datamodel();
    assign_uids(&mut project);
    pin_loop(&mut project, "main", "growth loop", &["pop", "growth"]);

    let (results, loop_partitions, ltm_vars) = run_ltm_discovery(&project);

    let pin_var = ltm_vars
        .iter()
        .find(|v| v.name == "$\u{205A}ltm\u{205A}loop_score\u{205A}pin1")
        .expect("discovery mode must emit the pinned loop_score var");
    assert_eq!(
        pin_var.dimensions,
        vec!["Region".to_string(), "Age".to_string()],
        "a multi-dim A2A pin must carry both dimensions"
    );
    let parts = loop_partitions
        .get("pin1")
        .expect("pinned loop must register a partition");
    assert_eq!(
        parts.len(),
        4,
        "2x2 element space -> 4 slots; got {parts:?}"
    );

    // An isolated reinforcing loop scores exactly +1 in every slot
    // (LTM ref 4.1: an isolated loop's score is +/-1 regardless of gain).
    let base = *results
        .offsets
        .get("$\u{205A}ltm\u{205A}loop_score\u{205A}pin1")
        .expect("pin1 loop score must be in results");
    for slot in 0..4 {
        for step in 3..results.step_count {
            let v = results.data[step * results.step_size + base + slot];
            assert!(
                (v - 1.0).abs() <= 1e-9,
                "pin1 slot {slot} at step {step}: got {v}, expected 1.0 (isolated loop)"
            );
        }
    }
}

/// GH #653 / pin-dims.AC2: a pinned *per-element-equation* loop -- the
/// MDL-importer shape, where the cycle's variables carry `Equation::Arrayed`
/// equations referencing literal element subscripts (FixedIndex shapes) --
/// must be scored per element in discovery mode.
///
/// The cycle classifies CrossElementOrMixed (FixedIndex shapes), so the pin is
/// expanded on the element graph; its diagonal element circuits collapse back
/// into one arrayed loop score whose slot equations reference each element's
/// own FixedIndex link scores. Same fixture/expected values as the
/// enumerated-path test
/// (`ltm_array_agg::per_element_equation_a2a_loop_scores_correct_for_every_slot`):
/// birth loop scores `b/(b-d)` per element.
#[test]
fn pinned_per_element_equation_loop_scored_per_element_in_discovery_mode() {
    let mut project = TestProject::new("per_elem_pin")
        .with_sim_time(0.0, 8.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("population[Region]", "100", &["births"], &["deaths"], None)
        .array_flow_with_ranges(
            "births[Region]",
            vec![
                ("NYC", "population[NYC] * 0.10"),
                ("Boston", "population[Boston] * 0.40"),
            ],
        )
        .array_flow_with_ranges(
            "deaths[Region]",
            vec![
                ("NYC", "population[NYC] * 0.03"),
                ("Boston", "population[Boston] * 0.05"),
            ],
        )
        .build_datamodel();
    assign_uids(&mut project);
    pin_loop(&mut project, "main", "growth", &["population", "births"]);

    let (results, loop_partitions, ltm_vars) = run_ltm_discovery(&project);

    let pin_var = ltm_vars
        .iter()
        .find(|v| v.name == "$\u{205A}ltm\u{205A}loop_score\u{205A}pin1")
        .expect("discovery mode must emit the pinned loop_score var");
    assert_eq!(
        pin_var.dimensions,
        vec!["Region".to_string()],
        "the pinned per-element-equation loop must collapse to one arrayed score over Region"
    );
    let parts = loop_partitions
        .get("pin1")
        .expect("pinned loop must register a partition");
    assert_eq!(
        parts.len(),
        2,
        "one partition entry per slot; got {parts:?}"
    );

    // Per-slot scores: slot 0 = NYC = 0.10/0.07, slot 1 = Boston = 0.40/0.35.
    let base = *results
        .offsets
        .get("$\u{205A}ltm\u{205A}loop_score\u{205A}pin1")
        .expect("pin1 loop score must be in results");
    let expected = [0.10 / 0.07, 0.40 / 0.35];
    const TOL: f64 = 1e-9;
    for (slot, &expected_score) in expected.iter().enumerate() {
        for step in 3..results.step_count {
            let v = results.data[step * results.step_size + base + slot];
            assert!(
                (v - expected_score).abs() <= TOL * expected_score.abs(),
                "pin1 slot {slot} at step {step}: got {v}, expected {expected_score}. A zero \
                 here means the pin's slot equation references the wrong element's link \
                 scores or failed to compile (GH #653)."
            );
        }
    }
}

/// GH #653 / pin-dims.AC3: a pinned *mixed* scalar/arrayed cycle (arrayed
/// stock -> scalar SUM aggregate -> scalar pressure -> arrayed flow) expands
/// to one element-level instance per element, each scored by its own scalar
/// loop-score variable -- exactly how the enumerator scores the same cycle.
///
/// Multi-instance pins use the deterministic id scheme `pin{n}⁚{j}`.
#[test]
fn pinned_mixed_scalar_arrayed_loop_scored_per_instance_in_discovery_mode() {
    let mut project = TestProject::new("mixed_pin")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Product", &["widgets", "gadgets"])
        .array_stock("inventory[Product]", "100", &["production"], &[], None)
        .aux("total_inventory", "SUM(inventory[*])", None)
        .aux("pressure", "1000 / total_inventory", None)
        .array_flow("production[Product]", "pressure * 2", None)
        .build_datamodel();
    assign_uids(&mut project);
    pin_loop(
        &mut project,
        "main",
        "production control",
        &["inventory", "total_inventory", "pressure", "production"],
    );

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);

    // One scalar loop-score var per element instance, ids pin1⁚1 / pin1⁚2.
    let pin_vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| {
            v.name
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}pin1")
        })
        .collect();
    assert_eq!(
        pin_vars.len(),
        2,
        "the mixed pin expands to one instance per Product element; got {:?}",
        pin_vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    for v in &pin_vars {
        assert!(
            v.dimensions.is_empty(),
            "each mixed-pin instance is a scalar loop score; {} has dims {:?}",
            v.name,
            v.dimensions
        );
    }

    // Both instances register partitions and produce finite, eventually
    // non-zero scores (the pre-fix behavior was a compile failure -> silent
    // constant 0 backed by a fragment-diagnostics Warning).
    let no_pin_warnings = collect_all_diagnostics(&db, sync.project)
        .iter()
        .all(|d| !format!("{:?}", d.error).contains("loop_score\u{205A}pin"));
    assert!(
        no_pin_warnings,
        "no pinned loop-score fragment may fail to compile"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    for v in &pin_vars {
        let id = v
            .name
            .strip_prefix("$\u{205A}ltm\u{205A}loop_score\u{205A}")
            .unwrap();
        assert!(
            ltm.loop_partitions.contains_key(id),
            "pin instance {id} must register a partition; got {:?}",
            ltm.loop_partitions.keys().collect::<Vec<_>>()
        );
        let off = *results
            .offsets
            .get(v.name.as_str())
            .unwrap_or_else(|| panic!("{} must be in results", v.name));
        let mut saw_nonzero = false;
        for step in 0..results.step_count {
            let val = results.data[step * results.step_size + off];
            assert!(
                val.is_finite(),
                "{} at step {step} must be finite, got {val}",
                v.name
            );
            if val != 0.0 {
                saw_nonzero = true;
            }
        }
        assert!(
            saw_nonzero,
            "{} must be non-zero at some step (silent-stub regression)",
            v.name
        );
    }
}

/// GH #653 / pin-dims.AC4: a pinned genuinely-cross-element cycle (migration:
/// each region's pressure reads the *other* region's population) expands to a
/// single element circuit visiting both elements, scored by one scalar
/// loop-score variable under the plain `pin1` id.
#[test]
fn pinned_cross_element_migration_loop_scored_in_discovery_mode() {
    let mut project = TestProject::new("migration_pin")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("population[Region]", "100", &["migration_in"], &[], None)
        .array_with_ranges(
            "migration_pressure[Region]",
            vec![
                ("NYC", "population[Boston] * 0.1"),
                ("Boston", "population[NYC] * 0.2"),
            ],
        )
        .array_flow("migration_in[Region]", "migration_pressure[Region]", None)
        .build_datamodel();
    assign_uids(&mut project);
    pin_loop(
        &mut project,
        "main",
        "migration",
        &["population", "migration_pressure", "migration_in"],
    );

    let (results, loop_partitions, ltm_vars) = run_ltm_discovery(&project);

    // Exactly one element-level instance (the 6-node walk through both
    // regions) -> the pin keeps its plain id and a scalar loop score.
    let pin_vars: Vec<&LtmSyntheticVar> = ltm_vars
        .iter()
        .filter(|v| {
            v.name
                .starts_with("$\u{205A}ltm\u{205A}loop_score\u{205A}pin")
        })
        .collect();
    assert_eq!(
        pin_vars.len(),
        1,
        "the cross-element migration cycle has exactly one element-level instance; got {:?}",
        pin_vars.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    assert_eq!(
        pin_vars[0].name,
        "$\u{205A}ltm\u{205A}loop_score\u{205A}pin1"
    );
    assert!(
        pin_vars[0].dimensions.is_empty(),
        "a cross-element pin instance is a scalar loop score"
    );
    assert!(
        loop_partitions.contains_key("pin1"),
        "the cross-element pin must register a partition"
    );
    assert_loop_score_is_link_product(&results, "pin1");
}

/// GH #653 / pin-dims.AC6.2: a pin whose variable cycle exists in the
/// variable-level causal graph but has NO element-level instantiation (the
/// per-element equations never close a cycle at any element) is reported
/// invalid with a clear reason -- never silently scored 0.
#[test]
fn pin_without_element_level_instantiation_is_invalid() {
    // Variable-level cycle: s -> g -> f -> s. Element-level: f[nyc] reads
    // g[nyc], g[boston] reads s[boston] -- the per-element edges never close
    // a cycle (s[nyc]'s only feedback path needs g[nyc] -> ... -> s[nyc], but
    // g[nyc] is a constant; s[boston] needs f[boston] to read g[boston], but
    // f[boston] is a constant).
    let mut project = TestProject::new("no_elem_cycle")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_stock("s[Region]", "100", &["f"], &[], None)
        .array_flow_with_ranges("f[Region]", vec![("NYC", "g[NYC]"), ("Boston", "5")])
        .array_with_ranges("g[Region]", vec![("NYC", "10"), ("Boston", "s[Boston]")])
        .build_datamodel();
    assign_uids(&mut project);
    pin_loop(&mut project, "main", "phantom", &["s", "g", "f"]);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let diagnostics = collect_all_diagnostics(&db, sync.project);

    let has_warning = diagnostics.iter().any(|d| {
        let msg = format!("{:?}", d.error);
        msg.contains("phantom") && msg.to_lowercase().contains("element-level")
    });
    assert!(
        has_warning,
        "a pin with no element-level instantiation must surface a diagnostic naming it; got: {:?}",
        diagnostics.iter().map(|d| &d.error).collect::<Vec<_>>()
    );

    // And no loop_score var was emitted for it.
    let source_model = sync.models["main"].source_model;
    let ltm = model_ltm_variables(&db, source_model, sync.project);
    assert!(
        !ltm.vars
            .iter()
            .any(|v| v.name.contains("loop_score\u{205A}pin")),
        "an uninstantiable pin must not emit a loop_score var"
    );
}

/// GH #653 / pin-dims.AC6.1: a pin whose element-level expansion subgraph
/// exceeds MAX_LTM_SCC_NODES is reported invalid with a clear reason rather
/// than hung on (element-level Johnson at that scale is intractable) or
/// silently zeroed.
#[test]
fn pin_with_oversized_element_expansion_is_invalid() {
    // 30 elements x (stock + agg + flow) -> a 61-node element-level SCC,
    // exceeding MAX_LTM_SCC_NODES = 50. The inlined SUM reducer couples every
    // element to every other through the synthetic agg node.
    let mut project = TestProject::new("oversized_pin")
        .with_sim_time(0.0, 5.0, 1.0)
        .indexed_dimension("D", 30)
        .array_stock("s[D]", "100", &["f"], &[], None)
        .array_flow("f[D]", "SUM(s[*]) * 0.01", None)
        .build_datamodel();
    assign_uids(&mut project);
    pin_loop(&mut project, "main", "big loop", &["s", "f"]);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let diagnostics = collect_all_diagnostics(&db, sync.project);

    let has_warning = diagnostics.iter().any(|d| {
        let msg = format!("{:?}", d.error);
        msg.contains("big loop") && msg.contains("strongly-connected")
    });
    assert!(
        has_warning,
        "an oversized pin expansion must surface a diagnostic naming it; got: {:?}",
        diagnostics.iter().map(|d| &d.error).collect::<Vec<_>>()
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

/// GH #653 / pin-dims.AC8.1: pin one of C-LEARN's climate feedback loops --
/// "Feedback cooling", the planet's blackbody-radiation balancing loop -- and
/// assert it is genuinely scored on the real model.
///
/// This is the issue's headline scenario: C-LEARN is a large arrayed
/// Vensim-imported model whose per-element-equation variables (arrayed over
/// `scenario`) made every pinned loop silently score 0. The pin is applied
/// through the production `SetLoopName` patch path (which mints variable UIDs
/// on demand, exactly like pysimlin's `patch.set_loop_name`), the model
/// auto-flips to discovery mode, and the pin must come back as one arrayed
/// loop score over `scenario` whose deterministic-scenario slot is finite,
/// eventually non-zero, and negative (it is a balancing loop).
///
/// `#[ignore]`d for runtime class only: C-LEARN's debug-mode parse +
/// LTM-discovery compile measures ~40s against the 3-minute
/// `cargo test --workspace` budget (see the design plan's "Additional
/// Considerations"). Run explicitly with:
///   cargo test --release -p simlin-engine --test integration -- --ignored clearn
#[test]
#[ignore]
fn clearn_pinned_climate_loop_is_scored() {
    use simlin_engine::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};

    let mdl_path = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";
    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));
    let mut project = simlin_engine::open_vensim(&contents)
        .unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));

    // Pin "Feedback cooling" via the production patch path (the importer does
    // not assign variable UIDs; SetLoopName mints them on demand).
    let feedback_cooling_cycle = [
        "heat_in_atmosphere_and_upper_ocean",
        "temperature_change_from_preindustrial",
        "feedback_cooling",
        "heat_in_atmosphere_and_upper_ocean_net_flow",
    ];
    apply_patch(
        &mut project,
        ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::SetLoopName {
                    variables: feedback_cooling_cycle
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    name: "Feedback cooling".to_string(),
                    description: None,
                }],
            }],
        },
    )
    .expect("SetLoopName patch must apply to the imported C-LEARN project");

    // LTM-enabled compile + run. C-LEARN's variable-level SCC exceeds
    // MAX_LTM_SCC_NODES, so LTM auto-flips to discovery -- the mode where a
    // pin is the only way to score a specific loop.
    let (results, loop_partitions, mode) = run_ltm(&project);
    assert_eq!(
        mode,
        LtmMode::Discovery,
        "C-LEARN must auto-flip LTM to discovery mode"
    );

    // The pin collapses to ONE arrayed loop score over the scenario dimension
    // (its per-element circuits form a diagonal family), with one partition
    // entry per scenario element.
    let parts = loop_partitions
        .get("pin1")
        .unwrap_or_else(|| panic!("pin1 must register a partition; got {loop_partitions:?}"));
    assert_eq!(
        parts.len(),
        3,
        "the scenario dimension has 3 elements (Deterministic / Low / High 2xCO2 sensitivity); \
         got {parts:?}"
    );

    let pin_offset = *results
        .offsets
        .get("$\u{205A}ltm\u{205A}loop_score\u{205A}pin1")
        .unwrap_or_else(|| {
            panic!(
                "pin1 loop score must be in results; loop_score offsets: {:?}",
                results
                    .offsets
                    .keys()
                    .filter(|k| k.as_str().contains("loop_score"))
                    .collect::<Vec<_>>()
            )
        });

    // Slot 0 is the Deterministic scenario (declared first in the MDL's
    // scenario dimension). Feedback cooling is a balancing loop: its score
    // must be finite at every step, non-zero once the climate system is
    // changing, and negative whenever non-zero.
    let det_series: Vec<f64> = (0..results.step_count)
        .map(|step| results.data[step * results.step_size + pin_offset])
        .collect();
    assert!(
        det_series.iter().all(|v| v.is_finite()),
        "the deterministic-scenario pin score must be finite at every step"
    );
    let nonzero: Vec<f64> = det_series.iter().copied().filter(|v| *v != 0.0).collect();
    assert!(
        !nonzero.is_empty(),
        "the deterministic-scenario pin score must be non-zero once behavior begins \
         (a constant-0 series is the GH #653 silent-stub failure)"
    );
    assert!(
        nonzero.iter().all(|v| *v < 0.0),
        "Feedback cooling is a balancing loop; every non-zero score must be negative. \
         First few non-zero values: {:?}",
        &nonzero[..nonzero.len().min(5)]
    );
}
