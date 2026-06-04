// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for `Vm` reset / `run_initials` / `run_to` behavior and bytecode
//! fusion. Split out of `vm.rs` to keep that file under the project
//! line-count lint; this is the `#[cfg(test)] mod vm_reset_and_run_initials_tests`
//! body, included via `#[path]` so `use super::*` still resolves `vm`'s private
//! items.
use super::*;
use crate::canonicalize;
use crate::test_common::TestProject;

fn pop_model() -> TestProject {
    TestProject::new("pop_model")
        .with_sim_time(0.0, 100.0, 1.0)
        .aux("birth_rate", "0.1", None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population / 80", None)
        .stock("population", "100", &["births"], &["deaths"], None)
}

fn build_compiled(tp: &TestProject) -> CompiledSimulation {
    tp.compile_incremental()
        .expect("incremental compile should succeed")
}

/// End-to-end guard for the 3-address fusion (R2), which is applied to the
/// Vm's flow/stock bytecode at construction. Uses subtraction and division
/// (non-commutative) so a swapped operand encoding in any fused form is a
/// loud failure rather than a silent miscompile. `a`, `b`, `c` are distinct
/// variables (not foldable into a literal).
///
/// Post-peephole + post-fusion, a *top-level* leaf binop assignment `x = a
/// op b` collapses all the way to one register-style leaf-assign op (the R2
/// extension), while the inner subexpression of a nested expression stays a
/// pushing `BinVarVar` whose result the outer op consumes from the stack.
#[test]
fn test_fused_binops_preserve_operand_order() {
    let tp = TestProject::new("fusion_order")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("a", "20", None)
        .aux("b", "5", None)
        .aux("c", "2", None)
        .aux("vv", "a - b", None) // AssignSubVarVarCurr (leaf assign)
        .aux("dvv", "a / b", None) // AssignDivVarVarCurr (leaf assign, division)
        .aux("vc", "a - 3", None) // AssignSubVarConstCurr (leaf assign)
        .aux("cv", "10 - a", None) // AssignSubConstVarCurr (leaf assign)
        .aux("sv", "(a - b) - c", None) // BinVarVar then AssignStackVarCurr
        .aux("sc", "(a - b) - 4", None); // BinVarVar then AssignStackConstCurr

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let val = |name: &str| -> f64 {
        let off = *results
            .offsets
            .get(&*canonicalize(name))
            .unwrap_or_else(|| panic!("missing {name}"));
        results.data[off] // step 0
    };

    assert_eq!(val("vv"), 15.0, "a - b");
    assert_eq!(val("dvv"), 4.0, "a / b");
    assert_eq!(val("vc"), 17.0, "a - 3");
    assert_eq!(val("cv"), -10.0, "10 - a");
    assert_eq!(val("sv"), 13.0, "(a - b) - c");
    assert_eq!(val("sc"), 11.0, "(a - b) - 4");
}

/// Regression (#632 review): `run_to(target)` with `target` past FINAL_TIME
/// exits the integration loop via the chunk-ring exhaustion break in
/// `save_advance!`, which sets `self.curr_chunk = self.next_chunk` *before*
/// breaking. The post-loop flow re-eval (added for #625) must not then call
/// `borrow_two` with two equal chunk indices -- that slices
/// `left[a*n_slots..(a+1)*n_slots]` out of a `left` of length `a*n_slots` and
/// panics. Such a target is a supported clamp case (the FFI `simlin_sim_run_to`
/// forwards `time` unclamped), so it must return `Ok` gracefully, not abort
/// across the C boundary.
#[test]
fn run_to_past_final_time_does_not_panic() {
    let tp = TestProject::new("past_end")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "rate", None);
    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();

    // 10x past FINAL_TIME: the loop fills the chunk ring and exits via the
    // exhaustion break (curr_chunk == next_chunk), the aliasing case.
    vm.run_to(30.0)
        .expect("run_to past the end must clamp gracefully, not panic");

    // The live curr chunk is still well-formed and readable (the integrated
    // stock, finite -- no out-of-bounds slice).
    let level_off = vm
        .get_offset(&Ident::<Canonical>::from_str_unchecked("level"))
        .expect("level offset must exist");
    assert!(
        vm.get_value_now(level_off).is_finite(),
        "level must be finite after clamping past the end"
    );
}

/// End-to-end guard for the global-operand and two-constant fused binops
/// (the R2 extension capturing the leaf-operand loads the original fusion
/// missed). All operators are Sub, Div, or Exp (non-commutative) so a swapped
/// operand encoding in any new fused form is a loud failure, not a silent
/// miscompile. Each global appears as a leaf operand of a *pushing*
/// subexpression (the outer `- c` is the assigning op) so the fusion emits
/// the pushing `BinGlobal*` / `BinConstConst` forms rather than a leaf
/// assign. The opcode each equation produces is pinned by a sibling
/// `bytecode_profile().fused_histogram` assertion so the test cannot pass
/// vacuously through the un-fused path.
///
/// `cc` uses `^` because compile-time constant folding (`compiler::fold`)
/// collapses every other `literal op literal` pair before codegen -- `^` is
/// the one operator folding deliberately leaves at runtime (platform-libm
/// `powf`), so it is the only way a `BinConstConst` still reaches the VM.
///
/// At step 0: TIME=10, DT=2 (the sim start time and dt -- both globals,
/// loaded via LoadGlobalVar), b=5, c=2, e=1.
#[test]
fn test_fused_global_and_const_binops_preserve_operand_order() {
    let tp = TestProject::new("global_fusion_order")
        .with_sim_time(10.0, 20.0, 2.0)
        .aux("b", "5", None)
        .aux("c", "2", None)
        .aux("e", "1", None)
        .aux("gv", "(TIME - b) - c", None) // BinGlobalVar(Sub) then stack-leaf
        .aux("vg", "(b / DT) - c", None) // BinVarGlobal(Div)
        .aux("gc", "(TIME - 3) - c", None) // BinGlobalConst(Sub)
        .aux("cg", "(3 / DT) - c", None) // BinConstGlobal(Div)
        .aux("gg", "(TIME - DT) - c", None) // BinGlobalGlobal(Sub)
        .aux("cc", "(7 ^ 3) - c", None) // BinConstConst(Exp) -- `^` survives folding
        .aux("sg", "((b - c) / DT) - e", None); // BinVarVar then BinStackGlobal(Div)

    let compiled = build_compiled(&tp);

    // Pin the fused shape: every new form must actually be emitted, so the
    // numeric asserts below exercise the fused dispatch arms (not a fallback).
    let fused = &compiled.bytecode_profile().fused_histogram;
    for name in [
        "BinGlobalVar",
        "BinVarGlobal",
        "BinGlobalConst",
        "BinConstGlobal",
        "BinGlobalGlobal",
        "BinConstConst",
        "BinStackGlobal",
    ] {
        assert!(
            fused.get(name).copied().unwrap_or(0) >= 1,
            "expected at least one {name} in the fused stream; got {fused:?}"
        );
    }

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    let val = |name: &str| -> f64 {
        let off = *results
            .offsets
            .get(&*canonicalize(name))
            .unwrap_or_else(|| panic!("missing {name}"));
        results.data[off] // step 0
    };

    assert_eq!(val("gv"), 3.0, "(TIME - b) - c = (10 - 5) - 2");
    assert_eq!(val("vg"), 0.5, "(b / DT) - c = (5 / 2) - 2");
    assert_eq!(val("gc"), 5.0, "(TIME - 3) - c = (10 - 3) - 2");
    assert_eq!(val("cg"), -0.5, "(3 / DT) - c = (3 / 2) - 2");
    assert_eq!(val("gg"), 6.0, "(TIME - DT) - c = (10 - 2) - 2");
    assert_eq!(val("cc"), 341.0, "(7 ^ 3) - c = 343 - 2");
    assert_eq!(val("sg"), 0.5, "((b - c) / DT) - e = ((5 - 2) / 2) - 1");
}

/// The single most dangerous risk for the global-operand fused binops: a
/// `_global` operand MUST read `curr[g]` (an absolute global slot), NOT
/// `curr[module_off + g]`. They are the same slot only at the root
/// (`module_off == 0`); inside a submodule they differ, so a `module_off + g`
/// miscompile is invisible at the root but corrupts every submodule.
///
/// This builds a real two-model project: `main` instantiates `sub`, whose
/// body computes `gv = (TIME - b) - c` (a `BinGlobalVar`) and
/// `vg = (b / DT) - c` (a `BinVarGlobal`) where `module_off > 0`. With
/// TIME=10, DT=2 (the sim start/dt globals at curr[0]/curr[1]) and the
/// submodule's own b=5, c=2, the correct values are gv=3, vg=0.5. The
/// submodule's slots 0/1 hold its own variables (curr[module_off+0/1]), whose
/// values differ from the globals, so a `module_off`-relative read would
/// produce different numbers.
#[test]
fn test_global_operand_binop_reads_global_not_module_relative() {
    use crate::datamodel;
    use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
    use crate::testutils::{x_aux, x_model, x_module, x_project};

    let sim_specs = datamodel::SimSpecs {
        start: 10.0,
        stop: 12.0,
        dt: datamodel::Dt::Dt(2.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };

    // `sub` is instantiated by `main`, so its variables live at module_off>0.
    // `b`/`c` are plain auxes (module_off-relative LoadVar operands); TIME/DT
    // are globals (LoadGlobalVar operands at curr[0]/curr[1]).
    let main = x_model("main", vec![x_module("sub", &[], None)]);
    let sub = x_model(
        "sub",
        vec![
            x_aux("b", "5", None),
            x_aux("c", "2", None),
            x_aux("gv", "(TIME - b) - c", None),
            x_aux("vg", "(b / DT) - c", None),
        ],
    );
    let datamodel = x_project(sim_specs, &[main, sub]);

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("two-model project should compile");

    // Confirm the fused global ops were actually emitted (inside the
    // submodule), so the dispatch path under test is exercised.
    let fused = &compiled.bytecode_profile().fused_histogram;
    assert!(
        fused.get("BinGlobalVar").copied().unwrap_or(0) >= 1,
        "expected a BinGlobalVar in the submodule's fused stream; got {fused:?}"
    );
    assert!(
        fused.get("BinVarGlobal").copied().unwrap_or(0) >= 1,
        "expected a BinVarGlobal in the submodule's fused stream; got {fused:?}"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();
    let series = crate::test_common::collect_results(&results);

    // Submodule variables are reported under their qualified (middot-joined)
    // `sub·<name>` names. A module_off-relative read of TIME/DT would yield
    // other numbers.
    let gv = series
        .get("sub\u{b7}gv")
        .unwrap_or_else(|| panic!("missing sub\u{b7}gv; have {:?}", series.keys()));
    let vg = series.get("sub\u{b7}vg").expect("missing sub\u{b7}vg");
    assert_eq!(
        gv[0], 3.0,
        "(TIME - b) - c = (10 - 5) - 2 -- TIME must be the global, not curr[module_off]"
    );
    assert_eq!(
        vg[0], 0.5,
        "(b / DT) - c = (5 / 2) - 2 -- DT must be the global, not curr[module_off+1]"
    );
}

#[test]
fn test_vm_reset_produces_identical_results() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    // First run
    let mut vm1 = Vm::new(compiled.clone()).unwrap();
    vm1.run_to_end().unwrap();
    let results1 = vm1.into_results();

    // Second fresh VM from same compiled
    let mut vm2 = Vm::new(compiled.clone()).unwrap();
    vm2.run_to_end().unwrap();
    let results2 = vm2.into_results();

    // Third: run, reset, run again
    let mut vm3 = Vm::new(compiled).unwrap();
    vm3.run_to_end().unwrap();
    vm3.reset();
    vm3.run_to_end().unwrap();
    let results3 = vm3.into_results();

    let pop_off = *results1.offsets.get(&*canonicalize("population")).unwrap();
    for step in 0..results1.step_count {
        let idx = step * results1.step_size + pop_off;
        let v1 = results1.data[idx];
        let v2 = results2.data[idx];
        let v3 = results3.data[idx];
        assert!(
            (v1 - v2).abs() < 1e-10,
            "fresh VMs should match at step {step}: {v1} vs {v2}"
        );
        assert!(
            (v1 - v3).abs() < 1e-10,
            "reset VM should match fresh at step {step}: {v1} vs {v3}"
        );
    }
}

#[test]
fn test_vm_reset_after_partial_run() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    // Full run for reference
    let mut vm_ref = Vm::new(compiled.clone()).unwrap();
    vm_ref.run_to_end().unwrap();
    let ref_results = vm_ref.into_results();

    // Partial run, then reset and full run
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to(50.0).unwrap();
    vm.reset();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let pop_off = *ref_results
        .offsets
        .get(&*canonicalize("population"))
        .unwrap();
    for step in 0..ref_results.step_count {
        let idx = step * ref_results.step_size + pop_off;
        let v_ref = ref_results.data[idx];
        let v = results.data[idx];
        assert!(
            (v_ref - v).abs() < 1e-10,
            "reset-after-partial should match fresh at step {step}: {v_ref} vs {v}"
        );
    }
}

#[test]
fn test_vm_reset_clears_previous_snapshot() {
    let tp = TestProject::new("reset_prev_snapshot")
        .with_sim_time(0.0, 3.0, 1.0)
        .aux("x", "1", None)
        .aux("prev_x", "PREVIOUS(x)", None);
    let compiled = build_compiled(&tp);

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    vm.reset();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let prev_off = *results.offsets.get(&*canonicalize("prev_x")).unwrap();
    let t0_prev = results.data[prev_off];
    assert!(
        (t0_prev - 0.0).abs() < 1e-10,
        "PREVIOUS(x) at t=0 after reset should be 0, got {t0_prev}"
    );
}

#[test]
fn test_previous_in_initials_vm() {
    let tp = TestProject::new("previous_in_initials")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("x", "5", None)
        .stock("s", "PREVIOUS(x)", &[], &[], None);

    let vm = tp.run_vm().expect("vm should run");
    let vm_s = vm.get("s").expect("vm missing s");

    // PREVIOUS(x) returns 0 at the initial timestep (the default value)
    assert!(
        (vm_s[0] - 0.0).abs() < 1e-10,
        "stock initial PREVIOUS(x) should be 0.0 at t=0, got {}",
        vm_s[0]
    );
}

#[test]
fn test_init_on_module_backed_var_freezes_initial_value() {
    let tp = TestProject::new("init_module_backed")
        .with_sim_time(0.0, 4.0, 1.0)
        .aux("x", "TIME", None)
        .aux("delayed", "PREVIOUS(x, 99)", None)
        .aux("frozen", "INIT(delayed)", None);

    let vm = tp.run_vm().expect("VM should run");
    let frozen_vals = vm.get("frozen").expect("frozen not in results");
    for (step, val) in frozen_vals.iter().enumerate() {
        assert!(
            (val - 99.0).abs() < 1e-10,
            "frozen should be 99.0 at every step, got {val} at step {step}"
        );
    }
}

#[test]
fn test_compiled_simulation_clone_produces_equivalent_vm() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);
    let compiled_clone = compiled.clone();

    let mut vm1 = Vm::new(compiled).unwrap();
    vm1.run_to_end().unwrap();
    let results1 = vm1.into_results();

    let mut vm2 = Vm::new(compiled_clone).unwrap();
    vm2.run_to_end().unwrap();
    let results2 = vm2.into_results();

    let pop_off = *results1.offsets.get(&*canonicalize("population")).unwrap();
    for step in 0..results1.step_count {
        let idx = step * results1.step_size + pop_off;
        assert!(
            (results1.data[idx] - results2.data[idx]).abs() < 1e-10,
            "cloned compiled should produce identical results at step {step}"
        );
    }
}

#[test]
fn test_run_initials_then_run_to_end_matches_single_call() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    // VM A: single run_to_end
    let mut vm_a = Vm::new(compiled.clone()).unwrap();
    vm_a.run_to_end().unwrap();
    let results_a = vm_a.into_results();

    // VM B: run_initials then run_to_end
    let mut vm_b = Vm::new(compiled).unwrap();
    vm_b.run_initials().unwrap();
    vm_b.run_to_end().unwrap();
    let results_b = vm_b.into_results();

    let pop_off = *results_a.offsets.get(&*canonicalize("population")).unwrap();
    for step in 0..results_a.step_count {
        let idx = step * results_a.step_size + pop_off;
        assert!(
            (results_a.data[idx] - results_b.data[idx]).abs() < 1e-10,
            "run_initials+run_to_end should match single run_to_end at step {step}"
        );
    }
}

#[test]
fn test_run_initials_is_idempotent() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_initials().unwrap();
    vm.run_initials().unwrap(); // second call should be no-op
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let pop_off = *results.offsets.get(&*canonicalize("population")).unwrap();
    let initial_pop = results.data[pop_off];
    assert_eq!(initial_pop, 100.0, "population initial should be 100");
}

#[test]
fn test_run_initials_sets_correct_values() {
    // Use a model where the aux is a stock dependency so it's in initials
    let tp = TestProject::new("initials_check")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("rate", "0.1", None)
        .flow("inflow", "0", None)
        .stock("s", "rate * 1000", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_initials().unwrap();

    let s_off = vm.get_offset(&Ident::new("s")).unwrap();
    let rate_off = vm.get_offset(&Ident::new("rate")).unwrap();

    assert_eq!(
        vm.get_value_now(s_off),
        100.0,
        "stock initial = rate*1000 = 100"
    );
    assert_eq!(
        vm.get_value_now(rate_off),
        0.1,
        "rate is a stock dependency, so it's in initials"
    );
    assert_eq!(
        vm.get_value_now(TIME_OFF),
        0.0,
        "time should be 0 after initials"
    );
}

#[test]
fn test_get_series_after_run_to_end() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let series = vm.get_series(&Ident::new("population")).unwrap();
    // With start=0, stop=100, save_step=1: 101 steps (0,1,...,100)
    assert_eq!(series.len(), 101, "should have 101 data points");
    assert_eq!(series[0], 100.0, "initial population should be 100");
    // Population should grow (birth_rate > death_rate for pop=100)
    assert!(
        series[100] > series[0],
        "population should grow: final={} > initial={}",
        series[100],
        series[0]
    );
}

#[test]
fn test_get_series_after_partial_run() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to(50.0).unwrap();

    let series = vm.get_series(&Ident::new("population")).unwrap();
    // With start=0, stop=100 but run_to(50): should have 51 steps (0..=50)
    assert_eq!(
        series.len(),
        51,
        "should have 51 data points for run_to(50)"
    );
    assert_eq!(series[0], 100.0, "initial population should be 100");

    // After reset, the VM should still work
    vm.reset();
    vm.run_to_end().unwrap();
    let full_series = vm.get_series(&Ident::new("population")).unwrap();
    assert_eq!(
        full_series.len(),
        101,
        "full run after reset should have 101 points"
    );
}

#[test]
fn test_get_series_after_run_initials_only() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_initials().unwrap();

    let series = vm.get_series(&Ident::new("population")).unwrap();
    assert_eq!(
        series.len(),
        1,
        "after run_initials only, series should have 1 element"
    );
    assert_eq!(
        series[0], 100.0,
        "the single element should be the initial value"
    );
}

#[test]
fn test_get_series_unknown_variable() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    assert!(
        vm.get_series(&Ident::new("nonexistent_var")).is_none(),
        "unknown variable should return None"
    );
}

#[test]
fn test_get_series_before_any_run() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    let vm = Vm::new(compiled).unwrap();
    let series = vm.get_series(&Ident::new("population")).unwrap();
    assert!(series.is_empty(), "before any run, series should be empty");
}
