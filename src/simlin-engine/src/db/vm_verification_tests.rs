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

/// AC1.4: Models with no feedback loops incur zero LTM overhead when
/// ltm_enabled=true. The layout should have no LTM variable slots and
/// no LTM fragments should be compiled.
#[test]
fn test_ltm_no_loops_zero_overhead() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = no_loop_project();
    // Extract Copy types from sync before needing &mut db.
    // Salsa tracked return values borrow &db, so we extract scalar
    // data (n_slots, len()) before each mutation point.
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };

    // Layout slot count with LTM enabled
    source_project.set_ltm_enabled(&mut db).to(true);
    let n_slots_with_ltm = compute_layout(&db, source_model, source_project).n_slots;

    // Layout slot count without LTM
    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_without_ltm = compute_layout(&db, source_model, source_project).n_slots;

    // Both layouts should have the same number of slots because there
    // are no feedback loops and thus no LTM synthetic variables
    assert_eq!(
        n_slots_with_ltm, n_slots_without_ltm,
        "no-loop model should have identical slot count with/without LTM: ltm={}, no_ltm={}",
        n_slots_with_ltm, n_slots_without_ltm
    );

    // Verify LTM synthetic variables are empty for this model
    source_project.set_ltm_enabled(&mut db).to(true);
    let ltm_var_count = model_ltm_variables(&db, source_model, source_project)
        .vars
        .len();
    assert_eq!(
        ltm_var_count, 0,
        "no-loop model should have zero LTM synthetic variables"
    );

    // Compilation should succeed with identical results
    let compiled_ltm = compile_project_incremental(&db, source_project, "main")
        .expect("LTM compilation should succeed for no-loop model");
    let ltm_root_slots = compiled_ltm.modules[&compiled_ltm.root].n_slots;

    source_project.set_ltm_enabled(&mut db).to(false);
    let compiled_no_ltm = compile_project_incremental(&db, source_project, "main")
        .expect("non-LTM compilation should succeed for no-loop model");
    let no_ltm_root_slots = compiled_no_ltm.modules[&compiled_no_ltm.root].n_slots;

    assert_eq!(
        ltm_root_slots, no_ltm_root_slots,
        "root module slot count should be identical for no-loop model with/without LTM"
    );
}

/// AC1.5: ltm_enabled=false skips all LTM layout and assembly work;
/// compilation produces identical bytecode to a compilation that never
/// had LTM enabled.
#[test]
fn test_ltm_disabled_identical_bytecode() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let source_project = {
        let sync = sync_from_datamodel(&db, &project);
        sync.project
    };

    // Compile with LTM disabled (the default)
    let compiled_never_ltm = compile_project_incremental(&db, source_project, "main")
        .expect("compilation without LTM should succeed");

    // Enable then disable LTM -- should return to the same state.
    // compile_project_incremental returns an owned CompiledSimulation,
    // so it does not borrow db.
    source_project.set_ltm_enabled(&mut db).to(true);
    let _compiled_ltm = compile_project_incremental(&db, source_project, "main")
        .expect("compilation with LTM should succeed");

    // Disable LTM again
    source_project.set_ltm_enabled(&mut db).to(false);
    let compiled_after_disable = compile_project_incremental(&db, source_project, "main")
        .expect("compilation after disabling LTM should succeed");

    // The root module's slot count should be identical
    let root_never = &compiled_never_ltm.modules[&compiled_never_ltm.root];
    let root_after = &compiled_after_disable.modules[&compiled_after_disable.root];

    assert_eq!(
        root_never.n_slots, root_after.n_slots,
        "slot count should be identical when LTM is disabled"
    );

    // The module count should be identical (no extra LTM modules)
    assert_eq!(
        compiled_never_ltm.modules.len(),
        compiled_after_disable.modules.len(),
        "module count should be identical when LTM is disabled"
    );

    // The offset map should be identical (no extra LTM variables)
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
}

/// AC1.1: LTM synthetic variables appear in compiled output with correct
/// offsets when compiling through the incremental path.
#[test]
fn test_ltm_incremental_produces_synthetic_variables() {
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };

    source_project.set_ltm_enabled(&mut db).to(true);

    let compiled = compile_project_incremental(&db, source_project, "main")
        .expect("LTM incremental compilation should succeed");

    // The feedback loop project has: population -> births -> population
    // LTM should produce at least one loop score and one relative loop score
    let has_ltm_offset = compiled.offsets.keys().any(|k| k.as_str().starts_with('$'));
    assert!(
        has_ltm_offset,
        "compiled output should contain LTM variable offsets (starting with '$')"
    );

    // Verify LTM increases the layout slot count. Extract n_slots
    // before toggling ltm_enabled to avoid holding a salsa ref across
    // a &mut db call.
    let n_slots_ltm = compute_layout(&db, source_model, source_project).n_slots;

    source_project.set_ltm_enabled(&mut db).to(false);
    let n_slots_no_ltm = compute_layout(&db, source_model, source_project).n_slots;

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

    source_project.set_ltm_enabled(&mut db).to(true);
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
    let compiled = compile_project_incremental(&db, source_project, "main")
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
    use salsa::Setter;

    let mut db = SimlinDb::default();
    let project = feedback_loop_project();
    let source_project = {
        let sync = sync_from_datamodel(&db, &project);
        sync.project
    };

    source_project.set_ltm_enabled(&mut db).to(true);

    let compiled = compile_project_incremental(&db, source_project, "main")
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
    use salsa::Setter;

    let tp = TestProject::new("mapped_loop_e2e")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension_with_mapping("Region", &["r1", "r2"], "State")
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_stock("stock[State]", "100", &["inflow"], &[], None)
        .array_flow("inflow[State]", "x[State] * 0.1", None)
        .array_aux_direct("x", vec!["Region".into()], "stock[Region] * 2", None);
    let project = tp.build_datamodel();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let source_project = sync.project;
    let source_model = sync.models["main"].source;
    source_project.set_ltm_enabled(&mut db).to(true);

    // The mapped Bare edges' link scores carry the TARGET's dimensions
    // (the mapped pair counts as corresponding -- `link_score_dimensions`
    // consults `mapped_element_correspondence`), so the per-slot
    // references in the loop-score equations resolve.
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
    let diags = crate::db::collect_model_diagnostics(&db, source_model, source_project);
    assert!(
        diags.is_empty(),
        "expected no diagnostics for the mapped-loop fixture, got {diags:?}"
    );

    let compiled = compile_project_incremental(&db, source_project, "main")
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

    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
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
    let compiled = compile_project_incremental(&db, sync.project, "main")
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
#[test]
fn test_dependency_graph_includes_previous_helper_for_module_backed_var() {
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
        has_previous_helper,
        "dependency graph runlists should include the helper aux for PREVIOUS(module-backed var)"
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

    // PREVIOUS(x) where x = SMTH1(input, 1) must rewrite through a scalar
    // helper aux, not LoadPrev directly against the module-backed variable.
    // Module-backed variables like SMTH1 occupy multiple VM slots.
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
    let compiled = compile_project_incremental(&db, sync.project, "main")
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
