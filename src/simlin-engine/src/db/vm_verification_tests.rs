// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! VM-level verification of PREVIOUS/INIT opcodes and LTM synthetic
//! variables compiled through the incremental pipeline, split out of
//! `tests.rs` to keep that file under the 6000-line per-file lint cap
//! (GH #645).

use super::*;
use crate::datamodel;
use crate::testutils::feedback_loop_project;
// ── PREVIOUS/INIT opcode verification tests ──────────────────────────

/// 1-arg PREVIOUS(x) compiles to the LoadPrev opcode. Verify that
/// PREVIOUS returns 0 at the first timestep (matching the old module
/// behavior where initial_value defaults to 0) and tracks the prior
/// timestep value thereafter.
#[test]
fn test_previous_opcode_vm() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("previous_parity")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("level", "100", &["inflow"], &[], None)
        .flow("inflow", "10", None)
        .aux("prev_level", "PREVIOUS(level)", None);

    let vm = tp.run_vm().expect("VM should run successfully");

    let vm_vals = vm.get("prev_level").expect("prev_level not in VM results");

    // LoadPrev reads from prev_values which is initialized to zeros,
    // so at t=0, PREVIOUS(level) returns 0 (not level's initial value).
    let level_vals = vm.get("level").expect("level not in VM results");
    assert!(
        (vm_vals[0] - 0.0).abs() < 1e-10,
        "prev_level at t=0 should be 0 (prev_values initialized to zeros), got {}",
        vm_vals[0]
    );
    // At subsequent steps, prev_level[t] == level[t-1]
    for step in 1..vm_vals.len() {
        assert!(
            (vm_vals[step] - level_vals[step - 1]).abs() < 1e-10,
            "prev_level at step {step} should equal level at step {}: expected {}, got {}",
            step - 1,
            level_vals[step - 1],
            vm_vals[step]
        );
    }
}

/// INIT(x) compiles to the LoadInitial opcode. Verify that INIT
/// freezes the t=0 value correctly even in an aux-only model (no
/// stocks).
#[test]
fn test_init_opcode_vm() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("init_parity")
        .with_sim_time(1.0, 5.0, 1.0)
        .aux("rate", "TIME", None)
        .aux("init_rate", "INIT(rate)", None);

    let vm = tp.run_vm().expect("VM should run successfully");

    let vm_vals = vm.get("init_rate").expect("init_rate not in VM results");

    // INIT(rate) should freeze rate's t=0 value (rate=TIME, TIME starts
    // at 1.0) and return 1.0 at every timestep even as TIME advances.
    for (step, val) in vm_vals.iter().enumerate() {
        assert!(
            (val - 1.0).abs() < 1e-10,
            "init_rate should be 1.0 at every step, got {val} at step {step}"
        );
    }
}

/// PREVIOUS and INIT are both intrinsic now and should not appear in the
/// stdlib model registry.
#[test]
fn test_previous_removed_from_stdlib_model_names() {
    let names = crate::stdlib::MODEL_NAMES;
    assert!(
        !names.contains(&"previous"),
        "'previous' should no longer be in MODEL_NAMES"
    );
    assert!(
        !names.contains(&"init"),
        "'init' should no longer be in MODEL_NAMES"
    );
}

/// PREVIOUS of a flow (not just a stock) works correctly. The flow
/// is recomputed each timestep; PREVIOUS(flow) should return the prior
/// timestep's computed flow value.
///
/// Like stocks, the 1-arg PREVIOUS(flow) form returns its desugared
/// fallback `0` at t=0.
#[test]
fn test_previous_of_flow_vm() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("previous_flow")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("level", "100", &["growth"], &[], None)
        .flow("growth", "level * 0.1", None)
        .aux("prev_growth", "PREVIOUS(growth)", None);

    let vm = tp.run_vm().expect("VM should run successfully");

    let vm_vals = vm
        .get("prev_growth")
        .expect("prev_growth not in VM results");

    // Unary PREVIOUS desugars to PREVIOUS(growth, 0). At t=0 it returns 0,
    // and at subsequent steps it returns growth's prior-timestep value.
    let growth_vals = vm.get("growth").expect("growth not in VM results");
    assert!(
        (vm_vals[0] - 0.0).abs() < 1e-10,
        "prev_growth at t=0 should be 0 (stdlib default), got {}",
        vm_vals[0]
    );
    for step in 1..vm_vals.len() {
        assert!(
            (vm_vals[step] - growth_vals[step - 1]).abs() < 1e-10,
            "prev_growth at step {step} should equal growth at step {}: expected {}, got {}",
            step - 1,
            growth_vals[step - 1],
            vm_vals[step]
        );
    }
}

/// AC1.2: PREVIOUS(x[DimA]) in an arrayed equation emits per-element
/// LoadPrev with correct offsets. Each array element should track the
/// previous value of its own slot independently.
///
/// Model: DimA = {a1, a2}
///   base_val[DimA] = apply-to-all with different values per element:
///     a1 = 10, a2 = 20
///   prev_val[DimA] = PREVIOUS(base_val[DimA])
///
/// At t=0: prev_val[a1] = 0, prev_val[a2] = 0  (LoadPrev reads zeros)
/// At t=1: prev_val[a1] = 10, prev_val[a2] = 20 (prior step values)
#[test]
fn test_arrayed_1arg_previous_loadprev_per_element() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("arrayed_prev_1arg")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("DimA", &["a1", "a2"])
        .array_with_ranges("base_val[DimA]", vec![("a1", "10"), ("a2", "20")])
        .array_aux("prev_val[DimA]", "PREVIOUS(base_val[DimA])");

    tp.assert_compiles_incremental();

    let vm = tp.run_vm().expect("VM should run successfully");

    let vm_a1 = vm
        .get("prev_val[a1]")
        .expect("prev_val[a1] not in VM results");
    let vm_a2 = vm
        .get("prev_val[a2]")
        .expect("prev_val[a2] not in VM results");

    // At t=0, unary PREVIOUS uses its desugared fallback of 0.
    assert!(
        (vm_a1[0] - 0.0).abs() < 1e-10,
        "prev_val[a1] at t=0 should be 0, got {}",
        vm_a1[0]
    );
    assert!(
        (vm_a2[0] - 0.0).abs() < 1e-10,
        "prev_val[a2] at t=0 should be 0, got {}",
        vm_a2[0]
    );

    // At t=1+, each element returns its own prior value (10 and 20 respectively).
    // base_val is constant so prev_val converges to the constant value after step 1.
    for step in 1..vm_a1.len() {
        assert!(
            (vm_a1[step] - 10.0).abs() < 1e-10,
            "prev_val[a1] at step {step} should be 10, got {}",
            vm_a1[step]
        );
        assert!(
            (vm_a2[step] - 20.0).abs() < 1e-10,
            "prev_val[a2] at step {step} should be 20, got {}",
            vm_a2[step]
        );
    }
}

/// AC3.2: PREVIOUS(arrayed_var, init_val) (2-arg) compiles per element with
/// the explicit fallback. Each element uses the shared init_val at t=0 and
/// tracks that element's previous value thereafter.
///
/// Model: DimA = {a1, a2}
///   base_val[DimA]: a1 = 10, a2 = 20
///   prev_val[DimA] = PREVIOUS(base_val[DimA], 99)
///
/// At t=0: prev_val[a1] = 99, prev_val[a2] = 99  (explicit fallback)
/// At t=1: prev_val[a1] = 10, prev_val[a2] = 20  (prior step values)
#[test]
fn test_arrayed_2arg_previous_per_element() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("arrayed_prev_2arg")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("DimA", &["a1", "a2"])
        .array_with_ranges("base_val[DimA]", vec![("a1", "10"), ("a2", "20")])
        .array_aux("prev_val[DimA]", "PREVIOUS(base_val[DimA], 99)");

    tp.assert_compiles_incremental();

    let vm = tp.run_vm().expect("VM should run successfully");

    let vm_a1 = vm
        .get("prev_val[a1]")
        .expect("prev_val[a1] not in VM results");
    let vm_a2 = vm
        .get("prev_val[a2]")
        .expect("prev_val[a2] not in VM results");

    // The explicit fallback is returned at t=0.
    assert!(
        (vm_a1[0] - 99.0).abs() < 1e-10,
        "2-arg PREVIOUS[a1] at t=0 should be init_val=99, got {}",
        vm_a1[0]
    );
    assert!(
        (vm_a2[0] - 99.0).abs() < 1e-10,
        "2-arg PREVIOUS[a2] at t=0 should be init_val=99, got {}",
        vm_a2[0]
    );

    // At t=1, each element returns its corresponding base_val from t=0.
    // base_val[a1]=10, base_val[a2]=20, so previous values are 10 and 20.
    assert!(
        (vm_a1[1] - 10.0).abs() < 1e-10,
        "2-arg PREVIOUS[a1] at t=1 should be base_val[a1] from t=0 = 10, got {}",
        vm_a1[1]
    );
    assert!(
        (vm_a2[1] - 20.0).abs() < 1e-10,
        "2-arg PREVIOUS[a2] at t=1 should be base_val[a2] from t=0 = 20, got {}",
        vm_a2[1]
    );

    // At t=2+, base_val is constant so previous values remain 10 and 20.
    for step in 2..vm_a1.len() {
        assert!(
            (vm_a1[step] - 10.0).abs() < 1e-10,
            "2-arg PREVIOUS[a1] at step {step} should be 10, got {}",
            vm_a1[step]
        );
        assert!(
            (vm_a2[step] - 20.0).abs() < 1e-10,
            "2-arg PREVIOUS[a2] at step {step} should be 20, got {}",
            vm_a2[step]
        );
    }
}

// --- LTM incremental compilation verification tests (Phase 2 Task 6) ---

/// A linear chain model with no feedback loops: aux -> flow -> stock.
/// Used to verify AC1.4 (no feedback loops = zero LTM overhead).
fn no_loop_project() -> datamodel::Project {
    datamodel::Project {
        name: "no_loop".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "growth_rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.05".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "inflow".to_string(),
                    equation: datamodel::Equation::Scalar("growth_rate".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "level".to_string(),
                    equation: datamodel::Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["inflow".to_string()],
                    outflows: vec![],
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

/// AC1.4: Models with no feedback loops incur zero LTM overhead under
/// `LtmOverlay::On`. The layout should have no LTM variable slots and
/// no LTM fragments should be compiled.
#[test]
fn test_ltm_no_loops_zero_overhead() {
    let db = SimlinDb::default();
    let project = no_loop_project();
    // Extract Copy types from sync before needing &mut db.
    // Salsa tracked return values borrow &db, so we extract scalar
    // data (n_slots, len()) before each mutation point.
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };

    // Layout slot count with LTM enabled
    let n_slots_with_ltm =
        compute_layout(&db, source_model, source_project, crate::db::LtmOverlay::On).n_slots;

    // Layout slot count without LTM
    let n_slots_without_ltm = compute_layout(
        &db,
        source_model,
        source_project,
        crate::db::LtmOverlay::Off,
    )
    .n_slots;

    // Both layouts should have the same number of slots because there
    // are no feedback loops and thus no LTM synthetic variables
    assert_eq!(
        n_slots_with_ltm, n_slots_without_ltm,
        "no-loop model should have identical slot count with/without LTM: ltm={}, no_ltm={}",
        n_slots_with_ltm, n_slots_without_ltm
    );

    // Verify LTM synthetic variables are empty for this model
    let ltm_var_count = model_ltm_variables(&db, source_model, source_project)
        .vars
        .len();
    assert_eq!(
        ltm_var_count, 0,
        "no-loop model should have zero LTM synthetic variables"
    );

    // Compilation should succeed with identical results
    let compiled_ltm =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compilation should succeed for no-loop model");
    let ltm_root_slots = compiled_ltm.modules[&compiled_ltm.root].n_slots;

    // The plain compile: with no loop there is nothing for the overlay to
    // add, so the two programs must agree.
    let compiled_no_ltm =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::Off)
            .expect("non-LTM compilation should succeed for no-loop model");
    let no_ltm_root_slots = compiled_no_ltm.modules[&compiled_no_ltm.root].n_slots;

    assert_eq!(
        ltm_root_slots, no_ltm_root_slots,
        "root module slot count should be identical for no-loop model with/without LTM"
    );
}

/// AC1.5: an `On` assembly leaks nothing into the plain program. The plain
/// program of a db that has assembled the overlay is the plain program of a
/// db that never has -- the same `CompiledSimulation`, bytecode included.
///
/// The two arms are two DERIVATIONS: the "never" arm lives in its own db, so
/// it is not the `assemble_simulation(.., Off)` memo read back (with the
/// overlay an argument, a repeat compile on one db is a salsa hit, which would
/// compare the memo with itself). What the second db pins is that the
/// per-model memos an `On` assembly populates -- layouts, fragments, the LTM
/// derivation -- are keyed so the plain assembly never reads one of them.
#[test]
fn test_ltm_disabled_identical_bytecode() {
    let project = feedback_loop_project();

    // The "never" arm: a db that only ever assembles the plain program.
    let compiled_never_ltm = {
        let db = SimlinDb::default();
        let source_project = sync_from_datamodel(&db, &project).project;
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::Off)
            .expect("compilation without LTM should succeed")
    };

    // The "after" arm: the overlay is assembled first, then the plain program
    // is derived beside it in the same db.
    let db = SimlinDb::default();
    let source_project = sync_from_datamodel(&db, &project).project;
    let compiled_ltm =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("compilation with LTM should succeed");
    let compiled_after_disable =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::Off)
            .expect("compilation beside the overlay should succeed");

    // Non-vacuity: the overlay did add something that could leak.
    assert!(
        compiled_ltm.offsets.len() > compiled_never_ltm.offsets.len(),
        "the feedback-loop fixture must gain LTM offsets under `On`: on={}, off={}",
        compiled_ltm.offsets.len(),
        compiled_never_ltm.offsets.len()
    );

    // The granular checks first, so a regression names what moved...
    let root_never = &compiled_never_ltm.modules[&compiled_never_ltm.root];
    let root_after = &compiled_after_disable.modules[&compiled_after_disable.root];
    assert_eq!(
        root_never.n_slots, root_after.n_slots,
        "slot count should be identical when LTM is disabled"
    );
    assert_eq!(
        compiled_never_ltm.modules.len(),
        compiled_after_disable.modules.len(),
        "module count should be identical when LTM is disabled"
    );
    assert_eq!(
        compiled_never_ltm.offsets.len(),
        compiled_after_disable.offsets.len(),
        "offset count should be identical when LTM is disabled"
    );
    for (name, &off) in &compiled_never_ltm.offsets {
        assert_eq!(
            compiled_after_disable.offsets.get(name),
            Some(&off),
            "offset for '{}' should be identical when LTM is disabled",
            name.as_str()
        );
    }
    // ...and then the whole program: every module's bytecode, contexts, specs.
    assert!(
        *compiled_never_ltm == *compiled_after_disable,
        "the plain program derived beside an `On` assembly must be the plain \
         program of a db that never assembled the overlay"
    );
}

/// AC1.1: LTM synthetic variables appear in compiled output with correct
/// offsets when compiling through the incremental path.
#[test]
fn test_ltm_incremental_produces_synthetic_variables() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };

    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM incremental compilation should succeed");

    // The feedback loop project has: population -> births -> population
    // LTM should produce at least one loop score and one relative loop score
    let has_ltm_offset = compiled.offsets.keys().any(|k| k.as_str().starts_with('$'));
    assert!(
        has_ltm_offset,
        "compiled output should contain LTM variable offsets (starting with '$')"
    );

    // Verify LTM increases the layout slot count: the two overlays' layouts
    // are separate memos, so both are read from the same db.
    let n_slots_ltm =
        compute_layout(&db, source_model, source_project, crate::db::LtmOverlay::On).n_slots;

    let n_slots_no_ltm = compute_layout(
        &db,
        source_model,
        source_project,
        crate::db::LtmOverlay::Off,
    )
    .n_slots;

    assert!(
        n_slots_ltm > n_slots_no_ltm,
        "layout with LTM should have more slots than without: ltm={}, no_ltm={}",
        n_slots_ltm,
        n_slots_no_ltm
    );
}

/// AC1.6: Discovery mode compiles through the same incremental path.
/// model_ltm_variables in discovery mode produces score variables for
/// ALL causal links, not just those in feedback loops.
#[test]
fn test_ltm_discovery_mode_all_links() {
    use super::model_ltm_variables;
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };

    source_project.set_ltm_discovery_mode(&mut db).to(true);

    // Discovery mode produces per-link score variables for ALL causal
    // edges (not just those in feedback loops). Normal mode produces
    // per-link + loop-level + relative loop scores, but only for links
    // in detected loops. Both should produce non-zero var counts for a
    // model with feedback.
    let discovery_var_count = model_ltm_variables(&db, source_model, source_project)
        .vars
        .len();
    assert!(
        discovery_var_count > 0,
        "discovery mode should produce at least one link score variable"
    );

    source_project.set_ltm_discovery_mode(&mut db).to(false);
    let normal_var_count = model_ltm_variables(&db, source_model, source_project)
        .vars
        .len();
    assert!(
        normal_var_count > 0,
        "normal mode should produce at least one synthetic variable for a feedback model"
    );

    // Compilation should succeed in discovery mode
    source_project.set_ltm_discovery_mode(&mut db).to(true);
    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM discovery mode compilation should succeed");

    // Verify the compiled output has LTM offsets
    let has_ltm_offset = compiled.offsets.keys().any(|k| k.as_str().starts_with('$'));
    assert!(
        has_ltm_offset,
        "discovery mode should produce LTM offsets in compiled output"
    );
}

/// AC1.1 runtime verification: Run a simulation through the incremental
/// LTM path and verify loop scores are non-trivial (not all zero).
#[test]
fn test_ltm_incremental_simulation_produces_scores() {
    let db = SimlinDb::default();
    let project = feedback_loop_project();
    let source_project = {
        let sync = sync_from_datamodel(&db, &project);
        sync.project
    };

    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM incremental compilation should succeed");

    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");

    // Find a (non-relative) loop score in the offsets.  Relative loop
    // scores are no longer compile-time synthetic variables -- they are
    // computed post-simulation via `ltm_post::compute_rel_loop_scores`
    // from the raw `loop_score` timeseries, which is still emitted.
    let score_entry = compiled.offsets.iter().find(|(k, _)| {
        let s = k.as_str();
        s.contains("\u{205A}loop_score\u{205A}")
    });

    assert!(
        score_entry.is_some(),
        "should have at least one loop_score variable"
    );

    let (_, &offset) = score_entry.unwrap();

    // Read the score values from the simulation data
    let results = vm.into_results();
    let mut has_nonzero = false;
    for row in results.iter() {
        let val = row[offset];
        assert!(val.is_finite(), "loop score should be finite, got {val}");
        if val != 0.0 {
            has_nonzero = true;
        }
    }
    assert!(
        has_nonzero,
        "loop scores should have at least one non-zero value for a feedback model"
    );
}

/// GH #527 end-to-end: a feedback loop that crosses a DIMENSION MAPPING
/// (`stock`/`inflow` over `State`, `x` over `Region`, positional
/// `State↔Region` mappings declared both ways) produces exactly the
/// mapping-diagonal loops, with link scores that compile (arrayed over
/// each edge's TARGET dims, resolving their per-slot loop-score
/// references) and loop scores that are finite and sustained non-zero.
///
/// Before #527 the element graph emitted the `State × Region`
/// cross-product (6 enumerated loops, 4 spurious), and the mapped edges'
/// link scores were emitted as SCALAR variables whose equations
/// referenced arrayed variables in scalar context -- a fragment compile
/// failure silently stubbed to constant 0, so every loop score was 0.
#[test]
fn test_ltm_mapped_dimension_loop_scores_diagonal_and_nonzero() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("mapped_loop_e2e")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension_with_mapping("Region", &["r1", "r2"], "State")
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_stock("stock[State]", "100", &["inflow"], &[], None)
        .array_flow("inflow[State]", "x[State] * 0.1", None)
        .array_aux_direct("x", vec!["Region".into()], "stock[Region] * 2", None);
    let project = tp.build_datamodel();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;

    // The mapped Bare edges' link scores carry the TARGET's dimensions
    // (the mapped pair counts as corresponding -- `link_score_dimensions`
    // consults `db::bare_axis_pairing`), so the per-slot references in the
    // loop-score equations resolve.
    let ltm_vars = crate::db::model_ltm_variables(&db, source_model, source_project);
    let dims_of = |name: &str| -> &[String] {
        &ltm_vars
            .vars
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("missing LTM var {name}"))
            .dimensions
    };
    assert_eq!(
        dims_of("$\u{205A}ltm\u{205A}link_score\u{205A}stock\u{2192}x"),
        &["Region".to_string()],
        "mapped edge stock[State]→x[Region] gets the target's dims"
    );
    assert_eq!(
        dims_of("$\u{205A}ltm\u{205A}link_score\u{205A}x\u{2192}inflow"),
        &["State".to_string()],
        "mapped edge x[Region]→inflow[State] gets the target's dims"
    );

    // Exactly the two mapping-diagonal loops (s1↔r1, s2↔r2) -- not the 6
    // loops the pre-#527 cross-product element graph enumerated.
    let loop_score_names: Vec<&str> = ltm_vars
        .vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}loop_score\u{205A}"))
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(
        loop_score_names.len(),
        2,
        "expected exactly the two diagonal loops, got {loop_score_names:?}"
    );

    // No LTM fragment-compile warnings: the arrayed link-score equations
    // genuinely compile (their references resolve through the same
    // dimension mapping the model's own equations use). Before #527 the
    // scalar forms failed to compile and were silently stubbed to 0.
    let diags = crate::db::collect_model_diagnostics(
        &db,
        source_model,
        source_project,
        crate::db::LtmOverlay::On,
    );
    assert!(
        diags.is_empty(),
        "expected no diagnostics for the mapped-loop fixture, got {diags:?}"
    );

    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the mapped-dim loop model should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();

    let score_offsets: Vec<(String, usize)> = compiled
        .offsets
        .iter()
        .filter(|(k, _)| k.as_str().contains("\u{205A}loop_score\u{205A}"))
        .map(|(k, &off)| (k.to_string(), off))
        .collect();
    assert_eq!(score_offsets.len(), 2, "two loop scores in the layout");

    for (name, offset) in &score_offsets {
        let series: Vec<f64> = results.iter().map(|row| row[*offset]).collect();
        assert!(
            series.iter().all(|v| v.is_finite()),
            "loop score {name} must be finite everywhere: {series:?}"
        );
        // The loop-score machinery needs two steps of history (PREVIOUS of
        // PREVIOUS); from t=2 on the reinforcing loop must score non-zero
        // at every step (sustained, not a transient blip).
        assert!(
            series.iter().skip(2).all(|v| v.abs() > 1e-6),
            "loop score {name} must be sustained non-zero from t=2: {series:?}"
        );
    }
}

/// GH #757 (flipped from the GH #758-era loud-skip pin): `inflow[State] =
/// x[State] * 0.1` over `x[Region]` with the mapping declared in the
/// REVERSE direction (on `Region` toward `State`) now classifies `Bare` --
/// `classify_iterated_dim_shape` gates its mapped arm on the same
/// correspondence data `expand_same_element` consults
/// (both declaration directions), matching the compiler's
/// `translate_via_mapping`. The element graph emits the mapping DIAGONAL,
/// `link_score_dimensions`' Bare-site gate passes, so the `x→inflow` score
/// is the arrayed (per-slot diagonal) A2A form and the loops through it
/// are genuinely scored -- ending the conservatism where every loop
/// through the edge was dropped with a Warning.
#[test]
fn test_ltm_reverse_declared_subscripted_link_score_is_diagonal() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("reverse_mapped_subscripted_e2e")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension_with_mapping("Region", &["r1", "r2"], "State")
        .named_dimension("State", &["s1", "s2"])
        .array_stock("stock[State]", "100", &["inflow"], &[], None)
        .array_flow("inflow[State]", "x[State] * 0.1", None)
        .array_aux_direct("x", vec!["Region".into()], "stock[Region] * 2", None);
    let project = tp.build_datamodel();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;

    let ltm_vars = crate::db::model_ltm_variables(&db, source_model, source_project);
    // The reverse-declared subscripted edge now classifies Bare: the score
    // is the arrayed diagonal form over the TARGET's dims.
    assert_eq!(
        ltm_vars
            .vars
            .iter()
            .find(|v| v.name == "$\u{205A}ltm\u{205A}link_score\u{205A}x\u{2192}inflow")
            .expect("missing LTM var x→inflow (the GH #757 diagonal score)")
            .dimensions,
        vec!["State".to_string()],
        "reverse-declared subscripted x[State] must keep the target's dims"
    );
    // The Bare-classified forward edge keeps the arrayed (diagonal) score.
    assert_eq!(
        ltm_vars
            .vars
            .iter()
            .find(|v| v.name == "$\u{205A}ltm\u{205A}link_score\u{205A}stock\u{2192}x")
            .expect("missing LTM var stock→x")
            .dimensions,
        vec!["Region".to_string()],
        "forward-declared Bare edge stock[Region]→x keeps the target's dims"
    );

    // The loops through the edge are scored (pre-#757 every one was
    // dropped with a Warning).
    assert!(
        ltm_vars
            .vars
            .iter()
            .any(|v| v.name.contains("\u{205A}loop_score\u{205A}")),
        "loops through the now-diagonal edge must be scored; got: {:?}",
        ltm_vars
            .vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>()
    );

    // No unscoreable-edge Warning fires for the edge anymore.
    let diags = crate::db::collect_all_diagnostics(&db, source_project, crate::db::LtmOverlay::On);
    assert!(
        !diags.iter().any(|d| {
            d.severity == crate::db::DiagnosticSeverity::Warning
                && matches!(&d.error, crate::db::DiagnosticError::Assembly(msg)
                    if msg.contains("x -> inflow"))
        }),
        "the x→inflow edge is scoreable now; no Warning may fire: {diags:?}"
    );

    // The model compiles and simulates with LTM enabled, and the loop score
    // carries real non-zero values past the startup guard.
    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the reverse-declared mapped model should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    let loop_offset = results
        .offsets
        .iter()
        .find(|(k, _)| k.as_str().contains("\u{205A}loop_score\u{205A}"))
        .map(|(_, &off)| off)
        .expect("a loop-score series must exist");
    let series: Vec<f64> = results.iter().map(|row| row[loop_offset]).collect();
    assert!(
        series.iter().all(|v| v.is_finite()),
        "loop score must stay finite; got {series:?}"
    );
    assert!(
        series.iter().skip(3).any(|&v| v != 0.0),
        "loop score must carry real non-zero values; got {series:?}"
    );
}

#[test]
fn compute_link_polarities_stock_flow_model() {
    // A stock-flow model where "births" feeds into "population" (positive)
    // and "population" drives "deaths" (positive dependency).
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec!["deaths".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("population * 0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "deaths".to_string(),
                    equation: datamodel::Equation::Scalar("population * 0.05".to_string()),
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
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models.get("main").unwrap().source;

    let polarities = compute_link_polarities(&db, source_model, sync.project);

    // births -> population: positive (inflow)
    let births_to_pop = polarities.get(&("births".to_string(), "population".to_string()));
    assert_eq!(
        births_to_pop,
        Some(&crate::ltm::LinkPolarity::Positive),
        "inflow should have positive polarity"
    );

    // deaths -> population: negative (outflow)
    let deaths_to_pop = polarities.get(&("deaths".to_string(), "population".to_string()));
    assert_eq!(
        deaths_to_pop,
        Some(&crate::ltm::LinkPolarity::Negative),
        "outflow should have negative polarity"
    );

    // population -> births: positive (appears positively in births equation)
    let pop_to_births = polarities.get(&("population".to_string(), "births".to_string()));
    assert_eq!(
        pop_to_births,
        Some(&crate::ltm::LinkPolarity::Positive),
        "population appears positively in births equation"
    );

    // population -> deaths: positive (appears positively in deaths equation)
    let pop_to_deaths = polarities.get(&("population".to_string(), "deaths".to_string()));
    assert_eq!(
        pop_to_deaths,
        Some(&crate::ltm::LinkPolarity::Positive),
        "population appears positively in deaths equation"
    );
}

#[test]
fn compute_link_polarities_negative_dependency() {
    // "effect" = 100 - "cause", so cause has a negative effect on effect.
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "cause".to_string(),
                    equation: datamodel::Equation::Scalar("50".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "effect".to_string(),
                    equation: datamodel::Equation::Scalar("100 - cause".to_string()),
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
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models.get("main").unwrap().source;

    let polarities = compute_link_polarities(&db, source_model, sync.project);

    let cause_to_effect = polarities.get(&("cause".to_string(), "effect".to_string()));
    assert_eq!(
        cause_to_effect,
        Some(&crate::ltm::LinkPolarity::Negative),
        "subtracted variable should have negative polarity"
    );
}

/// GH #910 end-to-end: an implicit WITH-LOOKUP variable (`effect` carries
/// BOTH a real input equation and a gf, so the compiler lowers it to
/// `LOOKUP(effect_gf, input)`).
///
/// (a) Structural polarity: the gf is monotone DECREASING, so the
/// `input -> effect` link polarity must be Negative even though the raw
/// equation text (`input`) reads as Positive.
///
/// (b) Runtime link score: the ceteris-paribus partial must be fed
/// through the same gf, so partial and actual deltas are commensurable.
/// `input` is a stock ramping 0, 1, 2, ... (LTM bails early on a fully
/// stateless model) and `gf(x) = 10 - x` over [0, 10], so
/// `effect_t = 10 - t`, `Δeffect = -1`, and the partial (the only dep is
/// the live source, nothing to freeze) equals `effect_t` exactly -- the
/// score is `SAFEDIV(-1, |-1|) * SIGN(+1) = -1` at every step >= 1 (and 0
/// at the initial step by the guard). The pre-fix unwrapped partial
/// (`input_t` -- gf-INPUT units against gf-OUTPUT deltas) scored
/// `2t - 11` instead (-9 at t=1).
#[test]
fn test_with_lookup_link_polarity_and_score_gf_aware() {
    use crate::test_common::TestProject;
    use salsa::Setter;

    let decreasing_gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![10.0, 0.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
    };
    let tp = TestProject::new("with_lookup_ltm")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("input", "0", &["one"], &[], None)
        .flow("one", "1", None)
        .aux_with_gf("effect", "input", decreasing_gf);
    let project = tp.build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;

    // (a) Structural polarity composes the decreasing gf.
    let polarities = compute_link_polarities(&db, source_model, source_project);
    assert_eq!(
        polarities.get(&("input".to_string(), "effect".to_string())),
        Some(&crate::ltm::LinkPolarity::Negative),
        "a decreasing with-lookup gf must flip the input -> effect polarity"
    );

    // (b) Runtime link score: discovery mode scores every causal edge
    // (this two-variable model has no loops).
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the with-lookup model should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();

    let score_name = "$\u{205A}ltm\u{205A}link_score\u{205A}input\u{2192}effect";
    let offset = *compiled
        .offsets
        .get(&crate::common::Ident::<crate::common::Canonical>::new(
            score_name,
        ))
        .unwrap_or_else(|| {
            panic!(
                "missing {score_name} in compiled offsets: {:?}",
                compiled.offsets.keys().collect::<Vec<_>>()
            )
        });
    let series: Vec<f64> = results.iter().map(|row| row[offset]).collect();

    assert!(
        (series[0] - 0.0).abs() < 1e-10,
        "guard: the link score is 0 at the initial step; got {series:?}"
    );
    for (step, val) in series.iter().enumerate().skip(1) {
        assert!(
            (val - (-1.0)).abs() < 1e-10,
            "gf-aware score must be exactly -1 at step {step} (the link fully \
             explains the target's change); got {val} in {series:?}"
        );
    }
}

/// GH #910 arrayed end-to-end: both arrayed WITH-LOOKUP shapes produce
/// compiling, gf-aware link scores.
///
/// (a) A2A target with ONE shared variable-level decreasing gf: the score
/// is the ApplyToAll form whose partial pins table 0 (`LOOKUP(effect[1],
/// ...)`); every element scores exactly -1 from step 1 on (same algebra
/// as the scalar twin, per element).
///
/// (b) Per-element-equation target where r1's gf is DECREASING and r2's
/// is INCREASING: r1's slot scores -1 and r2's +1 -- each slot wrapped by
/// its OWN element's table (`LOOKUP(effect2[r1], ...)`).
#[test]
fn test_with_lookup_arrayed_link_scores_gf_aware() {
    use crate::test_common::TestProject;
    use salsa::Setter;

    let gf = |y0: f64, y1: f64| datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![y0, y1],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: datamodel::GraphicalFunctionScale {
            min: y0.min(y1),
            max: y0.max(y1),
        },
    };

    let tp = TestProject::new("with_lookup_arrayed_ltm")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .array_stock("input[Region]", "0", &["one"], &[], None)
        .array_flow("one[Region]", "1", None);
    let mut project = tp.build_datamodel();
    // (a) A2A effect with a shared variable-level decreasing gf.
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "effect".to_string(),
            equation: datamodel::Equation::ApplyToAll(
                vec!["Region".to_string()],
                "input[Region]".to_string(),
            ),
            documentation: String::new(),
            units: None,
            gf: Some(gf(10.0, 0.0)),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    // (b) Per-element-equation effect2 with per-element gfs: r1
    // decreasing, r2 increasing.
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "effect2".to_string(),
            equation: datamodel::Equation::Arrayed(
                vec!["Region".to_string()],
                vec![
                    (
                        "r1".to_string(),
                        "input[r1]".to_string(),
                        None,
                        Some(gf(10.0, 0.0)),
                    ),
                    (
                        "r2".to_string(),
                        "input[r2]".to_string(),
                        None,
                        Some(gf(0.0, 10.0)),
                    ),
                ],
                None,
                false,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the arrayed with-lookup model should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();

    // (a) The A2A score is arrayed over Region: each element's slot is
    // laid out contiguously after the base offset.
    let base = "$\u{205A}ltm\u{205A}link_score\u{205A}input\u{2192}effect";
    for (elem_off, elem) in ["r1", "r2"].iter().enumerate() {
        let offset = *compiled
            .offsets
            .get(&crate::common::Ident::<crate::common::Canonical>::new(base))
            .unwrap_or_else(|| panic!("missing {base} in compiled offsets"))
            + elem_off;
        let series: Vec<f64> = results.iter().map(|row| row[offset]).collect();
        for (step, val) in series.iter().enumerate().skip(1) {
            assert!(
                (val - (-1.0)).abs() < 1e-10,
                "shared-gf A2A score[{elem}] must be -1 at step {step}; got {val} in {series:?}"
            );
        }
    }

    // (b) The per-element-equation target's slots reference `input[r1]` /
    // `input[r2]` (FixedIndex shapes), so the emitted scores carry the
    // bracketed-from names, each arrayed over the target's dims with only
    // the matching slot live: r1's decreasing gf slot scores -1, r2's
    // increasing gf slot scores +1.
    for (elem_off, (elem, expected)) in [("r1", -1.0f64), ("r2", 1.0f64)].iter().enumerate() {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}input[{elem}]\u{2192}effect2");
        let base_off = *compiled
            .offsets
            .get(&crate::common::Ident::<crate::common::Canonical>::new(
                &name,
            ))
            .unwrap_or_else(|| {
                panic!(
                    "missing {name} in compiled offsets: {:?}",
                    compiled.offsets.keys().collect::<Vec<_>>()
                )
            });
        let series: Vec<f64> = results.iter().map(|row| row[base_off + elem_off]).collect();
        for (step, val) in series.iter().enumerate().skip(1) {
            assert!(
                (val - expected).abs() < 1e-10,
                "per-element-gf score {name}[{elem}] must be {expected} at step {step}; \
                 got {val} in {series:?}"
            );
        }
    }
}

/// A shallow DECREASING gf whose output range (`[1, 0]`) is far below its
/// input range (`[0, x_max]`). The mismatch is what makes an UNWRAPPED
/// partial (gf-input units) score with the OPPOSITE sign to the composed
/// link polarity (gf-output units) once the input outgrows the output --
/// the GH #910 sign contradiction the wrap must remove.
fn shallow_decreasing_gf(x_max: f64) -> datamodel::GraphicalFunction {
    datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, x_max]),
        y_points: vec![1.0, 0.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: x_max,
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    }
}

/// Read a link score's per-step series out of a compiled+run LTM model.
fn ltm_score_series(
    compiled: &crate::CompiledSimulation,
    results: &crate::results::Results,
    name: &str,
) -> Vec<f64> {
    let offset = *compiled
        .offsets
        .get(&crate::common::Ident::<crate::common::Canonical>::new(name))
        .unwrap_or_else(|| {
            panic!(
                "missing {name} in compiled offsets: {:?}",
                compiled.offsets.keys().collect::<Vec<_>>()
            )
        });
    results.iter().map(|row| row[offset]).collect()
}

/// GH #910 (scalar source -> arrayed WITH-LOOKUP target): the per-target-element
/// link scores `try_scalar_to_arrayed_link_scores` emits must be built from the
/// LOWERED (`LOOKUP(self, input)`) equation, not the raw input.
///
/// `drive` ramps 0, 1, 2, ... and `effect[Region] = drive` carries a shared
/// decreasing gf mapping `[0, 10] -> [1, 0]`, so `effect_t = 1 - t/10`.
/// The composed link polarity is Negative. With the gf-aware partial
/// (`LOOKUP(effect[1], drive)` == `effect_t`) the numerator is `Δeffect`
/// and the score is exactly -1 at every step >= 1.
///
/// The pre-fix unwrapped partial was `drive_t` -- gf-INPUT units measured
/// against a gf-OUTPUT anchor `PREVIOUS(effect)` -- scoring
/// `(t - effect_{t-1}) / |Δeffect|`, i.e. +11 at t=2: a POSITIVE score on a
/// Negative-polarity link. That internal contradiction is what this test
/// pins shut.
#[test]
fn test_with_lookup_scalar_to_arrayed_link_score_sign_matches_polarity() {
    use crate::test_common::TestProject;
    use salsa::Setter;

    let tp = TestProject::new("with_lookup_scalar_to_arrayed")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .stock("drive", "0", &["one"], &[], None)
        .flow("one", "1", None);
    let mut project = tp.build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "effect".to_string(),
            equation: datamodel::Equation::ApplyToAll(
                vec!["Region".to_string()],
                "drive".to_string(),
            ),
            documentation: String::new(),
            units: None,
            gf: Some(shallow_decreasing_gf(10.0)),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;

    let polarity = *compute_link_polarities(&db, source_model, source_project)
        .get(&("drive".to_string(), "effect".to_string()))
        .expect("drive -> effect edge");
    assert_eq!(
        polarity,
        crate::ltm::LinkPolarity::Negative,
        "a decreasing with-lookup gf flips the drive -> effect polarity"
    );

    source_project.set_ltm_discovery_mode(&mut db).to(true);
    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the scalar->arrayed with-lookup model should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    for elem in ["r1", "r2"] {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}drive\u{2192}effect[{elem}]");
        let series = ltm_score_series(&compiled, &results, &name);
        for (step, val) in series.iter().enumerate().skip(1) {
            assert!(
                (val - (-1.0)).abs() < 1e-10,
                "gf-aware score {name} must be -1 at step {step}; got {val} in {series:?}"
            );
            assert!(
                *val < 0.0,
                "score {name} must not contradict the Negative link polarity at step {step}: \
                 got {val}"
            );
        }
    }
}

/// GH #910 (variable-backed reducer whose owner is a WITH-LOOKUP target):
/// `total = SUM(pop[*])` carrying a decreasing gf lowers to
/// `total = gf(SUM(pop[*]))`, so the reducer emitters' per-row partial --
/// which is expressed in the reducer's own (gf-INPUT) units -- must be fed
/// through the gf before it is compared against `PREVIOUS(total)`.
///
/// `pop[r1]`, `pop[r2]` each ramp 0, 1, 2, ...; the gf maps `[0, 20] -> [1, 0]`,
/// so `total_t = 1 - 2t/20` and `Δtotal = -0.1`. The gf-aware per-row partial
/// evaluates the reducer with only `pop[r1]` live -- `pop[r1]_t +
/// PREVIOUS(pop[r2])` = `2t - 1` -- through the gf, giving a numerator of
/// `-0.05` and a score of exactly `-0.5` per row (the two rows summing to the
/// whole `Δtotal`). The composed link polarity is Negative and the score
/// agrees.
///
/// The pre-fix SUM shortcut partial (`PREVIOUS(total) + Δpop[r1]`) mixes a
/// gf-OUTPUT anchor with a gf-INPUT delta, yielding a numerator of `+1` and a
/// score of `+10` -- a POSITIVE score on a Negative-polarity link.
#[test]
fn test_with_lookup_variable_backed_reducer_link_score_sign_matches_polarity() {
    use crate::test_common::TestProject;
    use salsa::Setter;

    let tp = TestProject::new("with_lookup_variable_backed_reducer")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .array_stock("pop[Region]", "0", &["grow"], &[], None)
        .array_flow("grow[Region]", "1", None);
    let mut project = tp.build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "total".to_string(),
            equation: datamodel::Equation::Scalar("SUM(pop[*])".to_string()),
            documentation: String::new(),
            units: None,
            gf: Some(shallow_decreasing_gf(20.0)),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;

    let polarity = *compute_link_polarities(&db, source_model, source_project)
        .get(&("pop".to_string(), "total".to_string()))
        .expect("pop -> total edge");
    assert_eq!(
        polarity,
        crate::ltm::LinkPolarity::Negative,
        "a decreasing with-lookup gf flips the pop -> total polarity"
    );

    source_project.set_ltm_discovery_mode(&mut db).to(true);
    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the with-lookup reducer model should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    for elem in ["r1", "r2"] {
        let name = format!("$\u{205A}ltm\u{205A}link_score\u{205A}pop[{elem}]\u{2192}total");
        let series = ltm_score_series(&compiled, &results, &name);
        for (step, val) in series.iter().enumerate().skip(1) {
            assert!(
                (val - (-0.5)).abs() < 1e-10,
                "gf-aware reducer row score {name} must be -0.5 at step {step}; \
                 got {val} in {series:?}"
            );
            assert!(
                *val < 0.0,
                "score {name} must not contradict the Negative link polarity at step {step}: \
                 got {val}"
            );
        }
    }
}

/// GH #910: a reducer owned by a per-element-equation WITH-LOOKUP variable
/// with PER-ELEMENT graphical functions.
///
/// This shape never reaches the reducer emitters: `enumerate_agg_nodes`
/// declines the variable-backed form for a per-element-equation owner and
/// mints a SYNTHETIC agg instead, so the edge routes
/// `pop[r] -> $\u{205A}ltm\u{205A}agg\u{205A}0 -> total[e]`. The `agg -> total[e]`
/// half is a per-target-element ceteris-paribus partial, which CAN name that
/// element's own table -- so the wrap is fully expressible and the edge is
/// scored, not declined.
///
/// `total[r1]` applies a DECREASING gf and `total[r2]` an INCREASING one over
/// the same `SUM(pop[*])`, so the two slots' scores must have opposite signs
/// (-1 and +1). Before the wrap both slots' partials were the raw
/// (gf-input-units) agg value, scoring identically -- and contradicting
/// `total[r1]`'s Negative hop.
#[test]
fn test_with_lookup_per_element_gf_reducer_owner_scores_per_slot() {
    use crate::test_common::TestProject;
    use salsa::Setter;

    let increasing = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 20.0]),
        y_points: vec![0.0, 1.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 20.0,
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    let tp = TestProject::new("with_lookup_per_element_reducer")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .array_stock("pop[Region]", "0", &["grow"], &[], None)
        .array_flow("grow[Region]", "1", None);
    let mut project = tp.build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "total".to_string(),
            equation: datamodel::Equation::Arrayed(
                vec!["Region".to_string()],
                vec![
                    (
                        "r1".to_string(),
                        "SUM(pop[*])".to_string(),
                        None,
                        Some(shallow_decreasing_gf(20.0)),
                    ),
                    (
                        "r2".to_string(),
                        "SUM(pop[*])".to_string(),
                        None,
                        Some(increasing),
                    ),
                ],
                None,
                false,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    // Nothing is declined: the synthetic-agg split makes every half wrappable.
    let diags = crate::db::collect_model_diagnostics(
        &db,
        source_model,
        source_project,
        crate::db::LtmOverlay::On,
    );
    assert!(
        diags.is_empty(),
        "the per-element-gf agg split must score cleanly; got {diags:?}"
    );

    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the per-element-gf reducer owner should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    for (elem, expected) in [("r1", -1.0f64), ("r2", 1.0f64)] {
        let name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}0\u{2192}total[{elem}]"
        );
        let series = ltm_score_series(&compiled, &results, &name);
        for (step, val) in series.iter().enumerate().skip(1) {
            assert!(
                (val - expected).abs() < 1e-10,
                "per-element-gf agg->target score {name} must be {expected} at step {step}; \
                 got {val} in {series:?}"
            );
        }
    }
}

/// GH #910 (`$⁚ltm⁚agg⁚{n}` -> SCALAR with-lookup target): a scalar target
/// that holds a reducer as a SUB-expression hoists the reducer into a
/// synthetic agg, and the `agg -> target` half is built by
/// `generate_agg_to_scalar_target_equation` -- a full re-evaluation of the
/// target's (gf-input-units) equation text, which must be fed through the
/// target's own table before it is ratioed against the target's
/// (gf-output-units) deltas.
///
/// `pop[r1]`, `pop[r2]` each ramp 0, 1, 2, ...; `total = 2 * SUM(pop[*])`
/// carries a decreasing gf mapping `[0, 40] -> [1, 0]`, so `total_t = 1 - 0.1t`
/// and the composed `agg -> total` polarity is Negative. Wrapping collapses the
/// numerator to exactly `Δtotal`, giving a score of -1 at every step.
///
/// Pre-fix the unwrapped partial was `2 * agg0 = 4t`, scoring
/// `(4t - total_{t-1}) / |Δtotal|` -- a POSITIVE score 30-194x outside the
/// `[-1, 1]` normalization on a Negative-polarity link.
#[test]
fn test_with_lookup_agg_to_scalar_target_link_score_sign_matches_polarity() {
    use crate::test_common::TestProject;
    use salsa::Setter;

    let tp = TestProject::new("with_lookup_agg_to_scalar")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .array_stock("pop[Region]", "0", &["grow"], &[], None)
        .array_flow("grow[Region]", "1", None);
    let mut project = tp.build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "total".to_string(),
            equation: datamodel::Equation::Scalar("2 * SUM(pop[*])".to_string()),
            documentation: String::new(),
            units: None,
            gf: Some(shallow_decreasing_gf(40.0)),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;

    let polarity = *compute_link_polarities(&db, source_model, source_project)
        .get(&("pop".to_string(), "total".to_string()))
        .expect("pop -> total edge");
    assert_eq!(
        polarity,
        crate::ltm::LinkPolarity::Negative,
        "a decreasing with-lookup gf flips the pop -> total polarity"
    );

    source_project.set_ltm_discovery_mode(&mut db).to(true);
    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the agg->scalar with-lookup model should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    let name =
        "$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}0\u{2192}total";
    let series = ltm_score_series(&compiled, &results, name);
    for (step, val) in series.iter().enumerate().skip(1) {
        assert!(
            (val - (-1.0)).abs() < 1e-10,
            "gf-aware agg->scalar score {name} must be -1 at step {step}; got {val} in {series:?}"
        );
        assert!(
            val.abs() <= 1.0 + 1e-10,
            "a fully-explanatory hop's score must stay within [-1, 1]; got {val} at step {step}"
        );
    }
}

/// GH #910 (`RefShape::PerElement` source read into an arrayed with-lookup
/// target): `emit_per_element_link_scores` builds one scalar link score per
/// `(source row, target element)` via `generate_per_element_link_equation` --
/// another full re-evaluation of the target's (gf-input-units) equation that
/// must pass through the target's table.
///
/// `pop[Region, Age]` ramps 0, 1, 2, ...; `effect[Region] = pop[Region, young]`
/// carries a decreasing gf mapping `[0, 20] -> [1, 0]`, so `effect_t = 1 - t/20`
/// and the composed polarity is Negative. Wrapping collapses the numerator to
/// `Δeffect`, giving -1 at every step.
///
/// Pre-fix the numerator was `t - effect_{t-1}` (gf-input minus gf-output),
/// scoring +21 .. +84: a POSITIVE score on a Negative-polarity link.
#[test]
fn test_with_lookup_per_element_source_read_link_score_sign_matches_polarity() {
    use crate::test_common::TestProject;
    use salsa::Setter;

    let tp = TestProject::new("with_lookup_per_element_read")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("Age", &["young", "old"])
        .array_stock("pop[Region,Age]", "0", &["grow"], &[], None)
        .array_flow("grow[Region,Age]", "1", None);
    let mut project = tp.build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "effect".to_string(),
            equation: datamodel::Equation::ApplyToAll(
                vec!["Region".to_string()],
                "pop[Region, young]".to_string(),
            ),
            documentation: String::new(),
            units: None,
            gf: Some(shallow_decreasing_gf(20.0)),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;

    let polarity = *compute_link_polarities(&db, source_model, source_project)
        .get(&("pop".to_string(), "effect".to_string()))
        .expect("pop -> effect edge");
    assert_eq!(
        polarity,
        crate::ltm::LinkPolarity::Negative,
        "a decreasing with-lookup gf flips the pop -> effect polarity"
    );

    source_project.set_ltm_discovery_mode(&mut db).to(true);
    let compiled =
        compile_project_incremental(&db, source_project, "main", crate::db::LtmOverlay::On)
            .expect("LTM compile of the per-element with-lookup model should succeed");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();

    for elem in ["r1", "r2"] {
        let name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop[{elem},young]\u{2192}effect[{elem}]"
        );
        let series = ltm_score_series(&compiled, &results, &name);
        for (step, val) in series.iter().enumerate().skip(1) {
            assert!(
                (val - (-1.0)).abs() < 1e-10,
                "gf-aware per-element score {name} must be -1 at step {step}; \
                 got {val} in {series:?}"
            );
            assert!(
                val.abs() <= 1.0 + 1e-10,
                "a fully-explanatory hop's score must stay within [-1, 1]; \
                 got {val} at step {step}"
            );
        }
    }
}

/// GH #910 / GH #792: a per-element-equation target whose EXCEPT default holds an
/// un-hoistable reducer AND whose slots carry per-element graphical functions is
/// declined either way -- but the diagnostic must name the cause a modeler could
/// act on. Removing the gf would NOT make this edge scoreable (the per-element
/// equations' un-hoisted reducer reads still have no per-slot derivation), so
/// `decline_unhoisted_reducer_edge` must win over the with-lookup decline.
///
/// `SUM(matrix[Region,*])` inside an `Ast::Arrayed` slot reads `matrix` through a
/// DIM-NAMED index, which `enumerate_agg_nodes` refuses to hoist (each slot pins
/// the dim to its own element), so no aggregate node exists and the edge lands on
/// the cartesian arm of `try_cross_dimensional_link_scores`.
#[test]
fn test_with_lookup_per_element_owner_declines_naming_the_unhoisted_reducer() {
    use crate::test_common::TestProject;
    use salsa::Setter;

    let tp = TestProject::new("with_lookup_unhoisted_default_reducer")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("Sector", &["s1", "s2"])
        .array_stock("matrix[Region,Sector]", "0", &["grow"], &[], None)
        .array_flow("grow[Region,Sector]", "1", None);
    let mut project = tp.build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "total".to_string(),
            equation: datamodel::Equation::Arrayed(
                vec!["Region".to_string()],
                vec![(
                    "r1".to_string(),
                    "SUM(matrix[Region,*])".to_string(),
                    None,
                    Some(shallow_decreasing_gf(20.0)),
                )],
                Some("SUM(matrix[Region,*])".to_string()),
                true,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let diags = crate::db::collect_model_diagnostics(
        &db,
        source_model,
        source_project,
        crate::db::LtmOverlay::On,
    );
    let messages: Vec<String> = diags
        .iter()
        .map(|d| match &d.error {
            crate::db::DiagnosticError::Assembly(m) => m.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m.contains("that could not be hoisted into an aggregate")),
        "the decline must name the un-hoistable reducer, the cause a modeler can act on; \
         got {messages:?}"
    );
    assert!(
        !messages
            .iter()
            .any(|m| m.contains("per-element graphical function")),
        "the gf is NOT the reason this edge is unscoreable; got {messages:?}"
    );
}

/// Regression test: PREVIOUS(SELF, expr) where expr depends on another
/// variable. The initials runlist must include transitive deps of implicit
/// variables so the stdlib module's stock is initialized correctly.
#[test]
fn test_previous_self_initial_value() {
    // F = IF Time = 5 THEN 2 ELSE PREVIOUS(SELF, IF switch = 1 THEN 1 ELSE 0)
    // At step 0, PREVIOUS(SELF, 1) should return 1, not 0.
    let project = datamodel::Project {
        name: "test_previous".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("months".to_string()),
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "switch".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "f".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "IF Time = 5 THEN 2 ELSE PREVIOUS(SELF, IF switch = 1 THEN 1 ELSE 0)"
                            .to_string(),
                    ),
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
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);

    // Verify the initials runlist includes switch (transitive dep of the
    // implicit intermediate variable $:f:0:arg1).
    let source_model = sync.models.get("main").unwrap().source_model;
    let dep_graph =
        model_dependency_graph(&db, source_model, sync.project, ModuleInputSet::empty(&db));
    assert!(
        dep_graph.runlist_initials.contains(&"switch".to_string()),
        "switch must be in the initials runlist so PREVIOUS fallback helpers \
         are initialized after switch is computed"
    );

    let compiled =
        compile_project_incremental(&db, sync.project, "main", crate::db::LtmOverlay::Off).unwrap();
    let mut vm = crate::vm::Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let f_off = results
        .offsets
        .iter()
        .find(|(k, _)| k.as_ref() == "f")
        .map(|(_, v)| *v)
        .expect("f should be in results");

    assert_eq!(
        results.data[f_off], 1.0,
        "f at step 0 should be 1 (PREVIOUS initial value from IF switch=1 THEN 1 ELSE 0)"
    );

    // At step 5, F = 2 (the IF Time = 5 branch)
    let stride = results.offsets.len();
    assert_eq!(
        results.data[5 * stride + f_off],
        2.0,
        "f at step 5 should be 2 (IF Time = 5 THEN 2 branch)"
    );
}

/// Regression test: SMOOTH3 with a stock input must initialize to
/// the stock's initial value.  Previously, `module_deps` filtered
/// out stock inputs during the initial phase, breaking the
/// dependency graph.  Combined with non-deterministic HashSet
/// iteration in `build_runlist`, this caused the SMOOTH3 module to
/// sometimes be initialized before its stock input, reading 0
/// instead of the correct initial value.
#[test]
fn test_smooth3_stock_input_initialization() {
    use crate::vm::Vm;

    let project = datamodel::Project {
        name: "smooth3_stock_init".to_string(),
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "my_stock".to_string(),
                    equation: datamodel::Equation::Scalar("42".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec!["drain".to_string()],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "drain".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "delay_time".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "smoothed".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "SMTH3(my_stock, delay_time)".to_string(),
                    ),
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
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let compiled =
        compile_project_incremental(&db, sync.project, "main", crate::db::LtmOverlay::Off)
            .expect("incremental compile should succeed");
    let mut vm = Vm::new(compiled).expect("VM should build");
    vm.run_to_end().expect("VM should run");
    let vm_results = vm.into_results();

    let smoothed_ident = crate::common::Ident::new("smoothed");
    let vm_off = vm_results.offsets[&smoothed_ident];
    let vm_step0 = vm_results.data[vm_off];
    assert_eq!(
        vm_step0, 42.0,
        "SMOOTH3(stock, ...) at step 0 must equal stock initial value"
    );
}

#[test]
fn test_previous_returns_zero_at_first_timestep() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("prev_zero_first_step")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("x", "42", None)
        .aux("prev_x", "PREVIOUS(x)", None);

    let vm = tp.run_vm().expect("VM should run");
    let prev_vals = vm.get("prev_x").expect("prev_x not in results");

    assert!(
        (prev_vals[0] - 0.0).abs() < 1e-10,
        "PREVIOUS at t=0 should be 0, got {}",
        prev_vals[0]
    );
    for (step, val) in prev_vals.iter().enumerate().skip(1) {
        assert!(
            (val - 42.0).abs() < 1e-10,
            "PREVIOUS at step {step} should be 42, got {val}",
        );
    }
}
#[test]
fn test_2arg_previous_uses_explicit_fallback() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("prev_2arg")
        .with_sim_time(0.0, 3.0, 1.0)
        .stock("level", "100", &["inflow"], &[], None)
        .flow("inflow", "10", None)
        .aux("prev_level", "PREVIOUS(level, 99)", None);

    let vm = tp.run_vm().expect("VM should run");
    let prev_vals = vm.get("prev_level").expect("prev_level not in results");

    assert!(
        (prev_vals[0] - 99.0).abs() < 1e-10,
        "2-arg PREVIOUS at t=0 should be 99, got {}",
        prev_vals[0]
    );
    assert!(
        (prev_vals[1] - 100.0).abs() < 1e-10,
        "2-arg PREVIOUS at t=1 should be 100, got {}",
        prev_vals[1]
    );
}
/// `PREVIOUS` of a scalar module-call aux reads the aux's own slot: the call
/// is rewritten to a reference to a separate module instance and the aux
/// keeps one slot, so no capture helper enters any runlist.
#[test]
fn test_dependency_graph_needs_no_previous_helper_for_a_module_call_aux() {
    use crate::testutils::{x_aux, x_model};

    let project = crate::testutils::x_project(
        datamodel::SimSpecs::default(),
        &[x_model(
            "main",
            vec![
                x_aux("x", "TIME", None),
                x_aux("delayed", "SMTH1(x, 99)", None),
                x_aux("prev_delayed", "PREVIOUS(delayed, 123)", None),
            ],
        )],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let dep_graph =
        model_dependency_graph(&db, source_model, sync.project, ModuleInputSet::empty(&db));

    let has_previous_helper = dep_graph
        .runlist_initials
        .iter()
        .chain(dep_graph.runlist_flows.iter())
        .chain(dep_graph.runlist_stocks.iter())
        .any(|name| name.starts_with("$⁚prev_delayed⁚0⁚arg0"));
    assert!(
        !has_previous_helper,
        "a scalar module-call aux has a snapshot slot of its own, so no helper enters a runlist"
    );
}
#[test]
fn test_init_aux_only_model() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("init_aux_only")
        .with_sim_time(1.0, 5.0, 1.0)
        .aux("growing", "TIME * 2", None)
        .aux("frozen", "INIT(growing)", None);

    let vm = tp.run_vm().expect("VM should run");
    let frozen_vals = vm.get("frozen").expect("frozen not in results");

    for (step, val) in frozen_vals.iter().enumerate() {
        assert!(
            (val - 2.0).abs() < 1e-10,
            "frozen should be 2.0 at every step, got {val} at step {step}"
        );
    }
}

#[test]
fn test_previous_of_module_backed_variable_compiles_correctly() {
    use crate::testutils::{x_aux, x_model};
    use crate::vm::Vm;

    // `x` is a scalar variable with a snapshot slot of its own even though
    // its equation expands to a separate SMTH1 module instance, so
    // `PREVIOUS(x, x)` reads that slot directly.
    let project = datamodel::Project {
        name: "previous_of_smooth".to_string(),
        sim_specs: datamodel::SimSpecs {
            stop: 10.0,
            ..Default::default()
        },
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![
                x_aux("input", "10", None),
                x_aux("x", "SMTH1(input, 1)", None),
                x_aux("y", "PREVIOUS(x, x)", None),
            ],
        )],
        source: None,
        ai_information: None,
    };

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let compiled =
        compile_project_incremental(&db, sync.project, "main", crate::db::LtmOverlay::Off)
            .expect("PREVIOUS(SMTH1_var) should compile via incremental path");
    let mut vm = Vm::new(compiled).expect("VM should build");
    vm.run_to_end().expect("simulation should run");

    let x_series = vm
        .get_series(&crate::common::Ident::new("x"))
        .expect("x missing");
    let y_series = vm
        .get_series(&crate::common::Ident::new("y"))
        .expect("y missing");
    assert_eq!(x_series.len(), y_series.len());

    // y = PREVIOUS(x, x): at t>0, y[t] should equal x[t-1].
    for t in 1..x_series.len() {
        assert!(
            (y_series[t] - x_series[t - 1]).abs() < 1e-6,
            "step {t}: y={}, expected x_prev={}",
            y_series[t],
            x_series[t - 1],
        );
    }
}
