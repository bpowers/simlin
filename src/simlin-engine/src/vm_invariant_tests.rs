// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Time-invariant variable hoisting execution tests (GH #712, stage B2).
//!
//! Split out of `vm.rs` as a `#[path]`-included child module (so `super::*`
//! still reaches the parent module internals) to keep `vm.rs` under the
//! per-file line cap.
//!
//! B1 reorders the root flow runlist so run-invariant variables form a
//! contiguous prefix and records `flows_invariant_opcode_len`. B2 makes the VM
//! execute that prefix **once per `run_to`** (snapshot the written slots) and
//! re-seeds those slots into `curr` before every dynamic step. These tests are
//! the behavioral contract for that hoist: every saved step of every invariant
//! variable's series equals its constant value, stocks fed by invariant flows
//! integrate correctly, `PREVIOUS(invariant_var)` is stable, `set_value`
//! mid-run propagates, and `reset`/`clear_values` restore the baseline.
//!
//! Every test here is ALSO satisfied by the pre-B2 engine (which recomputes the
//! whole flow program every step), so they pin equivalence: they must keep
//! passing after the hoist. The genuinely new failure mode -- a stale or zeroed
//! invariant slot after step 0 because the per-step scatter is missing -- is
//! caught by `invariant_chain_constant_at_every_step` and its arrayed twin.

use super::*;
use crate::datamodel;
use crate::test_common::TestProject;

fn build_compiled(tp: &TestProject) -> CompiledSimulation {
    tp.compile_incremental()
        .expect("incremental compile should succeed")
}

/// A model with an invariant chain (`k`, `derived`, `pure`) layered on top of
/// genuine dynamics (a growing stock and a time-dependent flow). The invariant
/// vars must be hoisted; the dynamics must still step.
fn invariant_chain_model() -> TestProject {
    TestProject::new("invariant_chain")
        .with_sim_time(0.0, 10.0, 1.0)
        // Invariant chain: pure constants and pure-builtin derivations.
        .aux("k", "10", None)
        .aux("derived", "k * 3", None)
        .aux("pure", "SQRT(k)", None)
        // Dynamics: a stock fed by a constant-derived inflow, plus a
        // time-dependent aux so the dynamic suffix is non-empty.
        .flow("inflow", "derived / 10", None)
        .stock("level", "0", &["inflow"], &[], None)
        .aux("ramping", "TIME * 2", None)
}

/// THE presentation regression: every saved step of every invariant variable
/// equals its constant value. Without the per-step scatter the invariant slots
/// go stale (or zero) after step 0, so this fails on a broken B2.
#[test]
fn invariant_chain_constant_at_every_step() {
    let mut vm = Vm::new(build_compiled(&invariant_chain_model())).unwrap();
    vm.run_to_end().unwrap();

    let k = vm.get_series(&Ident::new("k")).unwrap();
    let derived = vm.get_series(&Ident::new("derived")).unwrap();
    let pure = vm.get_series(&Ident::new("pure")).unwrap();

    assert!(k.len() > 5, "expected several saved steps, got {}", k.len());
    assert_eq!(k.len(), derived.len());
    assert_eq!(k.len(), pure.len());

    for (i, &v) in k.iter().enumerate() {
        assert_eq!(v, 10.0, "k must be 10 at every step; step {i} = {v}");
    }
    for (i, &v) in derived.iter().enumerate() {
        assert_eq!(v, 30.0, "derived must be 30 at every step; step {i} = {v}");
    }
    let expected_pure = 10.0_f64.sqrt();
    for (i, &v) in pure.iter().enumerate() {
        assert_eq!(
            v, expected_pure,
            "pure must be sqrt(10) at every step; step {i} = {v}"
        );
    }
}

/// An ARRAYED invariant variable: every element's per-step series is its
/// constant value. The scatter must re-seed every element slot, not just the
/// base offset.
#[test]
fn arrayed_invariant_constant_at_every_step() {
    let tp = TestProject::new("arrayed_invariant")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Dim", &["a", "b", "c"])
        .array_const("base[Dim]", 4.0)
        .array_aux("scaled[Dim]", "base[*] * 2")
        // Genuine dynamics so the run is not all-invariant.
        .flow("inflow", "1", None)
        .stock("level", "0", &["inflow"], &[], None);

    let results = tp.run_vm().expect("VM should run");

    for elem in ["a", "b", "c"] {
        let key = format!("scaled[{elem}]");
        let series = results
            .get(&key)
            .unwrap_or_else(|| panic!("missing series for {key}"));
        assert!(series.len() > 3, "expected several steps for {key}");
        for (i, &v) in series.iter().enumerate() {
            assert_eq!(v, 8.0, "{key} must be 8 at every step; step {i} = {v}");
        }
    }
}

/// A stock fed by an invariant flow integrates correctly across all steps.
/// Catches a scatter-AFTER-eval ordering bug: if the dynamic stock phase reads
/// a stale (zero) invariant inflow slot, the stock never grows.
#[test]
fn stock_fed_by_invariant_flow_integrates() {
    let mut vm = Vm::new(build_compiled(&invariant_chain_model())).unwrap();
    vm.run_to_end().unwrap();

    // inflow = derived/10 = 3.0, dt=1, so level(t) = 3*t saved at t=0..10.
    let level = vm.get_series(&Ident::new("level")).unwrap();
    for (i, &v) in level.iter().enumerate() {
        let expected = 3.0 * i as f64;
        assert!(
            (v - expected).abs() < 1e-10,
            "level at step {i}: got {v}, expected {expected}"
        );
    }
}

/// `PREVIOUS(invariant_var)` behaves identically: it equals the constant from
/// the first step onward (PREVIOUS returns the fallback at t=0). The scatter
/// must land before the `prev_values` snapshot so PREVIOUS sees the seeded
/// invariant slot.
#[test]
fn previous_of_invariant_is_stable() {
    let tp = TestProject::new("previous_invariant")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("k", "7", None)
        .aux("derived", "k * 2", None)
        // PREVIOUS(derived) with fallback -1 at t=0, then 14 thereafter.
        .aux("prev_derived", "PREVIOUS(derived, -1)", None)
        .flow("inflow", "1", None)
        .stock("level", "0", &["inflow"], &[], None);

    let mut vm = Vm::new(build_compiled(&tp)).unwrap();
    vm.run_to_end().unwrap();

    let prev = vm.get_series(&Ident::new("prev_derived")).unwrap();
    assert!(prev.len() > 3);
    assert_eq!(prev[0], -1.0, "PREVIOUS fallback at t=0");
    for (i, &v) in prev.iter().enumerate().skip(1) {
        assert_eq!(
            v, 14.0,
            "PREVIOUS(derived) must be 14 from step 1 on; step {i} = {v}"
        );
    }
}

/// `set_value` mid-run: `run_to(5)`, override the constant, `run_to(end)`.
/// The invariant chain must reflect the new value for all subsequent steps.
/// This pins the documented "re-run the invariant phase at the next run_to"
/// semantics -- the override flows through the flow phase (not initials, which
/// run_to does not re-run once did_initials is set, matching HEAD).
#[test]
fn set_value_midrun_repropagates_invariant_chain() {
    let mut vm = Vm::new(build_compiled(&invariant_chain_model())).unwrap();

    vm.run_to(5.0).unwrap();
    vm.set_value(&Ident::new("k"), 20.0).unwrap();
    vm.run_to_end().unwrap();

    let k = vm.get_series(&Ident::new("k")).unwrap();
    let derived = vm.get_series(&Ident::new("derived")).unwrap();

    // Steps saved at t=0..5 used k=10; steps from t=6 on used k=20.
    // After the override and the second run_to, every saved step the second
    // run touched (t>=6) reflects k=20; the already-saved t<=5 rows keep k=10.
    assert!(
        k.len() >= 11,
        "expected the full run, got {} steps",
        k.len()
    );
    for (i, (&kv, &dv)) in k.iter().zip(derived.iter()).enumerate() {
        if i <= 5 {
            assert_eq!(kv, 10.0, "k at step {i} (pre-override) should be 10");
            assert_eq!(dv, 30.0, "derived at step {i} (pre-override) should be 30");
        } else {
            assert_eq!(kv, 20.0, "k at step {i} (post-override) should be 20");
            assert_eq!(dv, 60.0, "derived at step {i} (post-override) should be 60");
        }
    }
}

/// `clear_values` + `reset` + rerun restores the original invariant values.
#[test]
fn clear_values_reset_restores_invariant_baseline() {
    let compiled = build_compiled(&invariant_chain_model());

    let mut baseline = Vm::new(compiled.clone()).unwrap();
    baseline.run_to_end().unwrap();
    let base_derived = baseline.get_series(&Ident::new("derived")).unwrap();

    let mut vm = Vm::new(compiled).unwrap();
    vm.set_value(&Ident::new("k"), 99.0).unwrap();
    vm.run_to_end().unwrap();
    let overridden = vm.get_series(&Ident::new("derived")).unwrap();
    assert!(
        overridden.iter().all(|&v| (v - 297.0).abs() < 1e-9),
        "override should make derived = 99*3 = 297 everywhere"
    );

    vm.clear_values();
    vm.reset();
    vm.run_to_end().unwrap();
    let restored = vm.get_series(&Ident::new("derived")).unwrap();

    assert_eq!(restored.len(), base_derived.len());
    for (i, (&r, &b)) in restored.iter().zip(base_derived.iter()).enumerate() {
        assert_eq!(r, b, "restored derived at step {i}: {r} != baseline {b}");
    }
}

/// RK4 with an invariant chain: invariant vars stay constant across the
/// multi-stage step, and a stock fed by an invariant flow integrates exactly.
/// The scatter runs once per step (before stage 1); RK stages only mutate stock
/// + dynamic-flow slots, so invariant slots survive across stages.
#[test]
fn rk4_invariant_chain_constant_and_integrates() {
    let tp = TestProject::new("rk4_invariant")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta4)
        .aux("k", "10", None)
        .aux("derived", "k * 3", None)
        .flow("inflow", "derived / 10", None)
        .stock("level", "0", &["inflow"], &[], None);

    let mut vm = Vm::new(build_compiled(&tp)).unwrap();
    vm.run_to_end().unwrap();

    let derived = vm.get_series(&Ident::new("derived")).unwrap();
    for (i, &v) in derived.iter().enumerate() {
        assert_eq!(
            v, 30.0,
            "derived must be 30 at every RK4 step; step {i} = {v}"
        );
    }
    // Constant inflow of 3 with dt=1: level(t) = 3*t (RK4 is exact for a
    // constant derivative).
    let level = vm.get_series(&Ident::new("level")).unwrap();
    for (i, &v) in level.iter().enumerate() {
        let expected = 3.0 * i as f64;
        assert!(
            (v - expected).abs() < 1e-9,
            "RK4 level at step {i}: got {v}, expected {expected}"
        );
    }
}

/// RK2 twin of the above (the third method arm).
#[test]
fn rk2_invariant_chain_constant_and_integrates() {
    let tp = TestProject::new("rk2_invariant")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta2)
        .aux("k", "10", None)
        .aux("derived", "k * 3", None)
        .flow("inflow", "derived / 10", None)
        .stock("level", "0", &["inflow"], &[], None);

    let mut vm = Vm::new(build_compiled(&tp)).unwrap();
    vm.run_to_end().unwrap();

    let derived = vm.get_series(&Ident::new("derived")).unwrap();
    for (i, &v) in derived.iter().enumerate() {
        assert_eq!(
            v, 30.0,
            "derived must be 30 at every RK2 step; step {i} = {v}"
        );
    }
    let level = vm.get_series(&Ident::new("level")).unwrap();
    for (i, &v) in level.iter().enumerate() {
        let expected = 3.0 * i as f64;
        assert!(
            (v - expected).abs() < 1e-9,
            "RK2 level at step {i}: got {v}, expected {expected}"
        );
    }
}

/// An all-invariant model (no dynamics beyond TIME): the whole flow program is
/// the invariant prefix (split == full length). It must run correctly -- the
/// dynamic half is empty.
#[test]
fn all_invariant_model_runs() {
    let tp = TestProject::new("all_invariant")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("a", "3", None)
        .aux("b", "a * 4", None)
        .aux("c", "b + a", None);

    let mut vm = Vm::new(build_compiled(&tp)).unwrap();
    vm.run_to_end().unwrap();

    for (name, expected) in [("a", 3.0), ("b", 12.0), ("c", 15.0)] {
        let series = vm.get_series(&Ident::new(name)).unwrap();
        assert!(series.len() > 3);
        for (i, &v) in series.iter().enumerate() {
            assert_eq!(
                v, expected,
                "{name} must be {expected} at step {i}, got {v}"
            );
        }
    }
}

/// A no-invariant model (split == 0): nothing is hoisted, everything is
/// dynamic. The empty invariant program must be a no-op.
#[test]
fn no_invariant_model_runs() {
    let tp = TestProject::new("no_invariant")
        .with_sim_time(0.0, 5.0, 1.0)
        // `t2` depends on TIME (variant); `acc` is a stock (variant); the flow
        // depends on TIME (variant). Nothing is run-invariant.
        .aux("t2", "TIME * 2", None)
        .flow("inflow", "t2", None)
        .stock("acc", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    assert_eq!(
        compiled.flows_invariant_opcode_len(),
        0,
        "model has no run-invariant flow var; split must be 0"
    );

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let t2 = vm.get_series(&Ident::new("t2")).unwrap();
    for (i, &v) in t2.iter().enumerate() {
        assert_eq!(v, 2.0 * i as f64, "t2 at step {i}: got {v}");
    }
}

/// `get_value_now` of an invariant var mid-run (after `run_to(partial)`) reads
/// the seeded invariant slot, not a stale value.
#[test]
fn get_value_now_of_invariant_midrun() {
    let mut vm = Vm::new(build_compiled(&invariant_chain_model())).unwrap();
    vm.run_to(4.0).unwrap();

    let k_off = vm.get_offset(&Ident::new("k")).unwrap();
    let derived_off = vm.get_offset(&Ident::new("derived")).unwrap();
    assert_eq!(vm.get_value_now(k_off), 10.0, "k mid-run should be 10");
    assert_eq!(
        vm.get_value_now(derived_off),
        30.0,
        "derived mid-run should be 30"
    );
}
