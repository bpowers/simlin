// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for VM reset/run_to/constant-override behavior ([`super`]). Split
//! out of `vm.rs` to keep that file under the project line-count lint; this
//! is the `#[cfg(test)] mod vm_reset_run_to_and_constants_tests` body,
//! included via `#[path]` so `use super::*` still resolves vm's private
//! items.

use super::*;
use crate::datamodel;
use crate::test_common::TestProject;

fn pop_model() -> TestProject {
    TestProject::new("pop_model")
        .with_sim_time(0.0, 100.0, 1.0)
        .aux("birth_rate", "0.1", None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population / 80", None)
        .stock("population", "100", &["births"], &["deaths"], None)
}

fn build_compiled(tp: &TestProject) -> std::sync::Arc<CompiledSimulation> {
    tp.compile_incremental()
        .expect("incremental compile should succeed")
}

// ================================================================
// Multiple reset cycles
// ================================================================

#[test]
fn test_multiple_reset_cycles_produce_identical_results() {
    let compiled = build_compiled(&pop_model());
    let mut vm = Vm::new(compiled).unwrap();

    vm.run_to_end().unwrap();
    let ref_series = vm.get_series(&Ident::new("population")).unwrap();

    for cycle in 1..=5 {
        vm.reset();
        vm.run_to_end().unwrap();
        let series = vm.get_series(&Ident::new("population")).unwrap();
        assert_eq!(
            series.len(),
            ref_series.len(),
            "cycle {cycle}: series length should match"
        );
        for (step, (a, b)) in ref_series.iter().zip(series.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-10,
                "cycle {cycle}, step {step}: {a} vs {b}"
            );
        }
    }
}

// ================================================================
// Reset after partial run with different dt values
// ================================================================

#[test]
fn test_reset_after_partial_run_dt_quarter() {
    let tp = TestProject::new("dt_quarter")
        .with_sim_time(0.0, 10.0, 0.25)
        .aux("rate", "0.05", None)
        .flow("inflow", "stock * rate", None)
        .stock("stock", "100", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);

    let mut vm_ref = Vm::new(compiled.clone()).unwrap();
    vm_ref.run_to_end().unwrap();
    let ref_series = vm_ref.get_series(&Ident::new("stock")).unwrap();

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to(5.0).unwrap();
    vm.reset();
    vm.run_to_end().unwrap();
    let series = vm.get_series(&Ident::new("stock")).unwrap();

    assert_eq!(series.len(), ref_series.len());
    for (step, (a, b)) in ref_series.iter().zip(series.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-10,
            "step {step}: reference {a} vs reset {b}"
        );
    }
}

#[test]
fn test_reset_after_partial_run_dt_half() {
    let tp = TestProject::new("dt_half")
        .with_sim_time(0.0, 20.0, 0.5)
        .aux("rate", "0.03", None)
        .flow("inflow", "stock * rate", None)
        .stock("stock", "50", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);

    let mut vm_ref = Vm::new(compiled.clone()).unwrap();
    vm_ref.run_to_end().unwrap();
    let ref_series = vm_ref.get_series(&Ident::new("stock")).unwrap();

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to(10.0).unwrap();
    vm.reset();
    vm.run_to_end().unwrap();
    let series = vm.get_series(&Ident::new("stock")).unwrap();

    assert_eq!(series.len(), ref_series.len());
    for (step, (a, b)) in ref_series.iter().zip(series.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-10,
            "step {step}: reference {a} vs reset {b}"
        );
    }
}

// ================================================================
// Pre-filled constants verification
// ================================================================

#[test]
fn test_prefilled_constants_after_run_initials() {
    let tp = TestProject::new("constants_check")
        .with_sim_time(5.0, 50.0, 0.5)
        .flow("inflow", "0", None)
        .stock("s", "10", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_initials().unwrap();

    assert_eq!(vm.get_value_now(TIME_OFF), 5.0);
    assert_eq!(vm.get_value_now(DT_OFF), 0.5);
    assert_eq!(vm.get_value_now(INITIAL_TIME_OFF), 5.0);
    assert_eq!(vm.get_value_now(FINAL_TIME_OFF), 50.0);

    // DT/INITIAL_TIME/FINAL_TIME are pre-filled in every chunk slot during initials
    let data = vm.data.as_ref().unwrap();
    let n_slots = vm.n_slots;
    let total_chunks = vm.n_chunks + 2;
    for chunk in 1..total_chunks {
        let base = chunk * n_slots;
        assert_eq!(data[base + DT_OFF], 0.5, "DT in chunk {chunk}");
        assert_eq!(
            data[base + INITIAL_TIME_OFF],
            5.0,
            "INITIAL_TIME in chunk {chunk}"
        );
        assert_eq!(
            data[base + FINAL_TIME_OFF],
            50.0,
            "FINAL_TIME in chunk {chunk}"
        );
    }
}

#[test]
fn test_constants_remain_correct_throughout_simulation() {
    let tp = TestProject::new("constants_during_sim")
        .with_sim_time(0.0, 10.0, 1.0)
        .flow("inflow", "1", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let data = vm.data.as_ref().unwrap();
    let n_slots = vm.n_slots;
    for chunk in 0..vm.n_chunks {
        let base = chunk * n_slots;
        assert_eq!(data[base + DT_OFF], 1.0, "DT in chunk {chunk}");
        assert_eq!(
            data[base + INITIAL_TIME_OFF],
            0.0,
            "INITIAL_TIME in chunk {chunk}"
        );
        assert_eq!(
            data[base + FINAL_TIME_OFF],
            10.0,
            "FINAL_TIME in chunk {chunk}"
        );
    }
}

// ================================================================
// TIME series correctness
// ================================================================

#[test]
fn test_time_advances_by_dt_each_step() {
    let tp = TestProject::new("time_series")
        .with_sim_time(0.0, 5.0, 1.0)
        .flow("inflow", "0", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let data = vm.data.as_ref().unwrap();
    let n_slots = vm.n_slots;
    for chunk in 0..vm.n_chunks {
        let base = chunk * n_slots;
        let expected_time = chunk as f64;
        assert!(
            (data[base + TIME_OFF] - expected_time).abs() < 1e-10,
            "chunk {chunk}: TIME={}, expected {}",
            data[base + TIME_OFF],
            expected_time
        );
    }
}

#[test]
fn test_time_series_with_fractional_dt() {
    // Use save_step=dt so every step is saved
    let tp = TestProject::new_with_specs(
        "time_frac",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 2.0,
            dt: datamodel::Dt::Dt(0.25),
            save_step: Some(datamodel::Dt::Dt(0.25)),
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
    )
    .flow("inflow", "0", None)
    .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let data = vm.data.as_ref().unwrap();
    let n_slots = vm.n_slots;
    // Expected: 0.0, 0.25, 0.5, ..., 2.0 => 9 saved steps
    let expected_steps = 9;
    assert_eq!(vm.n_chunks, expected_steps);
    for chunk in 0..vm.n_chunks {
        let base = chunk * n_slots;
        let expected_time = chunk as f64 * 0.25;
        assert!(
            (data[base + TIME_OFF] - expected_time).abs() < 1e-10,
            "chunk {chunk}: TIME={}, expected {}",
            data[base + TIME_OFF],
            expected_time
        );
    }
}

#[test]
fn test_time_series_with_nonzero_start() {
    let tp = TestProject::new("time_nonzero")
        .with_sim_time(10.0, 15.0, 1.0)
        .flow("inflow", "0", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let data = vm.data.as_ref().unwrap();
    let n_slots = vm.n_slots;
    for chunk in 0..vm.n_chunks {
        let base = chunk * n_slots;
        let expected_time = 10.0 + chunk as f64;
        assert!(
            (data[base + TIME_OFF] - expected_time).abs() < 1e-10,
            "chunk {chunk}: TIME={}, expected {}",
            data[base + TIME_OFF],
            expected_time
        );
    }
}

/// When save_step does not evenly divide (stop-start), the VM must
/// only report the save points that fall within the horizon.
/// start=0, stop=10, save_step=4 → saves at t=0,4,8 (3 steps).
/// t=12 > stop, so we must NOT report a 4th step.
#[test]
fn test_non_divisible_save_step_no_over_allocation() {
    let tp = TestProject::new_with_specs(
        "non_div_save",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: Some(datamodel::Dt::Dt(4.0)),
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
    )
    .flow("inflow", "1", None)
    .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    // 3 saved steps: t=0, t=4, t=8
    assert_eq!(
        vm.n_chunks, 3,
        "non-divisible save_step must truncate, not round"
    );

    let results = vm.into_results();
    assert_eq!(results.step_count, 3);

    // Verify saved times
    let steps: Vec<&[f64]> = results.iter().collect();
    assert_eq!(steps.len(), 3);
    assert!((steps[0][TIME_OFF] - 0.0).abs() < 1e-10);
    assert!((steps[1][TIME_OFF] - 4.0).abs() < 1e-10);
    assert!((steps[2][TIME_OFF] - 8.0).abs() < 1e-10);
}

/// When save_step < dt the VM can only save once per dt step, so
/// n_chunks must reflect the dt-based cadence, not the raw save_step.
#[test]
fn test_save_step_smaller_than_dt() {
    let tp = TestProject::new_with_specs(
        "save_lt_dt",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: Some(datamodel::Dt::Dt(0.5)),
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
    )
    .flow("inflow", "1", None)
    .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    // Effective save cadence is dt=1.0 (can't save more often than dt),
    // so we expect 11 saved steps at t=0,1,2,...,10.
    assert_eq!(vm.n_chunks, 11);

    let results = vm.into_results();
    assert_eq!(results.step_count, 11);

    let steps: Vec<&[f64]> = results.iter().collect();
    assert_eq!(steps.len(), 11);
    for (i, step) in steps.iter().enumerate() {
        assert!(
            (step[TIME_OFF] - i as f64).abs() < 1e-10,
            "step {i}: TIME={}, expected {}",
            step[TIME_OFF],
            i
        );
    }
}

/// A very small but positive dt must be accepted, not rejected by
/// an approximate-zero check.  The contract is dt > 0 (strict positivity).
#[test]
fn test_small_positive_dt_accepted() {
    let tp = TestProject::new_with_specs(
        "tiny_dt",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 1e-6,
            dt: datamodel::Dt::Dt(1e-8),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
    )
    .aux("x", "42", None);

    let compiled = tp.compile_incremental().expect("compile should succeed");
    assert!(Vm::new(compiled).is_ok(), "Vm::new must accept dt=1e-8");
}

/// dt=0 must still be rejected.
#[test]
fn test_zero_dt_rejected() {
    let tp = TestProject::new_with_specs(
        "zero_dt",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(0.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
    )
    .aux("x", "1", None);

    let compiled = tp.compile_incremental().expect("compile should succeed");
    assert!(Vm::new(compiled).is_err(), "Vm::new must reject dt=0");
}

// ================================================================
// set_value_now / get_value_now
// ================================================================

#[test]
fn test_set_and_get_value_now() {
    let tp = TestProject::new("set_get")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("rate", "0.1", None)
        .flow("inflow", "stock * rate", None)
        .stock("stock", "100", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_initials().unwrap();

    let stock_off = vm.get_offset(&Ident::new("stock")).unwrap();

    assert_eq!(vm.get_value_now(stock_off), 100.0);

    vm.set_value_now(stock_off, 42.0);
    assert_eq!(vm.get_value_now(stock_off), 42.0);

    vm.set_value_now(stock_off, -7.5);
    assert_eq!(vm.get_value_now(stock_off), -7.5);
}

#[test]
fn test_set_value_now_for_special_offsets() {
    let tp = TestProject::new("set_specials")
        .with_sim_time(0.0, 10.0, 1.0)
        .flow("inflow", "0", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_initials().unwrap();

    assert_eq!(vm.get_value_now(TIME_OFF), 0.0);
    assert_eq!(vm.get_value_now(DT_OFF), 1.0);
    assert_eq!(vm.get_value_now(INITIAL_TIME_OFF), 0.0);
    assert_eq!(vm.get_value_now(FINAL_TIME_OFF), 10.0);

    vm.set_value_now(TIME_OFF, 99.0);
    assert_eq!(vm.get_value_now(TIME_OFF), 99.0);
}

#[test]
fn test_set_value_now_after_run_initials_affects_simulation() {
    let tp = TestProject::new("set_after_init")
        .with_sim_time(0.0, 5.0, 1.0)
        .flow("inflow", "stock * 0.1", None)
        .stock("stock", "100", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);

    let mut vm1 = Vm::new(compiled.clone()).unwrap();
    vm1.run_to_end().unwrap();
    let series1 = vm1.get_series(&Ident::new("stock")).unwrap();

    let mut vm2 = Vm::new(compiled).unwrap();
    vm2.run_initials().unwrap();
    let stock_off = vm2.get_offset(&Ident::new("stock")).unwrap();
    vm2.set_value_now(stock_off, 200.0);
    vm2.run_to_end().unwrap();
    let series2 = vm2.get_series(&Ident::new("stock")).unwrap();

    assert_eq!(series1[0], 100.0);
    assert_eq!(series2[0], 200.0);
    for step in 1..series1.len() {
        assert!(
            series2[step] > series1[step],
            "step {step}: stock with init=200 ({}) should be > stock with init=100 ({})",
            series2[step],
            series1[step]
        );
    }
}

// ================================================================
// run_to with partial ranges
// ================================================================

#[test]
fn test_run_to_partial_then_continue_matches_full_run() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    let mut vm_full = Vm::new(compiled.clone()).unwrap();
    vm_full.run_to_end().unwrap();
    let full_series = vm_full.get_series(&Ident::new("population")).unwrap();

    let mut vm_partial = Vm::new(compiled).unwrap();
    vm_partial.run_to(50.0).unwrap();
    vm_partial.run_to_end().unwrap();
    let partial_series = vm_partial.get_series(&Ident::new("population")).unwrap();

    assert_eq!(full_series.len(), partial_series.len());
    for (step, (a, b)) in full_series.iter().zip(partial_series.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-10,
            "step {step}: full={a} vs partial+continue={b}"
        );
    }
}

#[test]
fn test_run_to_multiple_segments_matches_full_run() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);

    let mut vm_full = Vm::new(compiled.clone()).unwrap();
    vm_full.run_to_end().unwrap();
    let full_series = vm_full.get_series(&Ident::new("population")).unwrap();

    let mut vm_seg = Vm::new(compiled).unwrap();
    vm_seg.run_to(25.0).unwrap();
    vm_seg.run_to(50.0).unwrap();
    vm_seg.run_to(75.0).unwrap();
    vm_seg.run_to_end().unwrap();
    let seg_series = vm_seg.get_series(&Ident::new("population")).unwrap();

    assert_eq!(full_series.len(), seg_series.len());
    for (step, (a, b)) in full_series.iter().zip(seg_series.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-10,
            "step {step}: full={a} vs segmented={b}"
        );
    }
}

#[test]
fn test_run_to_segments_preserve_previous_state() {
    let tp = TestProject::new("run_to_prev_segments")
        .with_sim_time(0.0, 5.0, 1.0)
        .aux("x", "TIME", None)
        .aux("prev_x", "PREVIOUS(x)", None);
    let compiled = build_compiled(&tp);

    let mut vm_full = Vm::new(compiled.clone()).unwrap();
    vm_full.run_to_end().unwrap();
    let full_prev = vm_full.get_series(&Ident::new("prev_x")).unwrap();

    let mut vm_seg = Vm::new(compiled).unwrap();
    vm_seg.run_to(2.0).unwrap();
    vm_seg.run_to_end().unwrap();
    let seg_prev = vm_seg.get_series(&Ident::new("prev_x")).unwrap();

    assert_eq!(full_prev.len(), seg_prev.len());
    for (step, (full, seg)) in full_prev.iter().zip(seg_prev.iter()).enumerate() {
        assert!(
            (full - seg).abs() < 1e-10,
            "step {step}: full={full} vs segmented={seg}"
        );
    }
}

// ================================================================
// Non-default save_every (save_step != dt)
// ================================================================

#[test]
fn test_save_every_2_with_dt_1() {
    let tp = TestProject::new_with_specs(
        "save_every_test",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: Some(datamodel::Dt::Dt(2.0)),
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
    )
    .flow("inflow", "1", None)
    .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let series = vm.get_series(&Ident::new("s")).unwrap();

    // save_step=2, dt=1, start=0, stop=10: saved at t=0,2,4,6,8,10 => 6 points
    assert_eq!(series.len(), 6, "should have 6 saved points");
    let expected = [0.0, 2.0, 4.0, 6.0, 8.0, 10.0];
    for (i, (&actual, &exp)) in series.iter().zip(expected.iter()).enumerate() {
        assert!(
            (actual - exp).abs() < 1e-10,
            "saved point {i}: actual={actual}, expected={exp}"
        );
    }
}

#[test]
fn test_save_every_with_fractional_dt() {
    let tp = TestProject::new_with_specs(
        "save_frac",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 4.0,
            dt: datamodel::Dt::Dt(0.5),
            save_step: Some(datamodel::Dt::Dt(1.0)),
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
    )
    .flow("inflow", "2", None)
    .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let series = vm.get_series(&Ident::new("s")).unwrap();

    // save_step=1, dt=0.5, start=0, stop=4: saved at t=0,1,2,3,4 => 5 points
    assert_eq!(series.len(), 5, "should have 5 saved points");
    // s increases by inflow*dt = 2*0.5 = 1.0 per dt step.
    // At save points: t=0: 0, t=1: 2, t=2: 4, t=3: 6, t=4: 8
    let expected = [0.0, 2.0, 4.0, 6.0, 8.0];
    for (i, (&actual, &exp)) in series.iter().zip(expected.iter()).enumerate() {
        assert!(
            (actual - exp).abs() < 1e-10,
            "saved point {i}: actual={actual}, expected={exp}"
        );
    }
}

#[test]
fn test_save_every_matches_dt_gives_all_steps() {
    let tp = TestProject::new("save_all")
        .with_sim_time(0.0, 5.0, 1.0)
        .flow("inflow", "1", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let series = vm.get_series(&Ident::new("s")).unwrap();

    assert_eq!(series.len(), 6, "should have 6 saved points");
    let expected = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
    for (i, (&actual, &exp)) in series.iter().zip(expected.iter()).enumerate() {
        assert!(
            (actual - exp).abs() < 1e-10,
            "saved point {i}: actual={actual}, expected={exp}"
        );
    }
}

// ================================================================
// Reset clears temp_storage
// ================================================================

#[test]
fn test_reset_zeroes_temp_storage() {
    let tp = pop_model();
    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    vm.reset();

    for (i, &val) in vm.temp_storage.iter().enumerate() {
        assert_eq!(val, 0.0, "temp_storage[{i}] should be 0 after reset");
    }
}

// ================================================================
// Simulation produces correct numerical results
// ================================================================

#[test]
fn test_exponential_growth_euler() {
    // ds/dt = s * 0.1, s(0) = 100, dt = 1
    let tp = TestProject::new("exp_growth")
        .with_sim_time(0.0, 5.0, 1.0)
        .flow("growth", "s * 0.1", None)
        .stock("s", "100", &["growth"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let series = vm.get_series(&Ident::new("s")).unwrap();

    // Euler: s(t+1) = s(t) * 1.1
    let expected = [100.0, 110.0, 121.0, 133.1, 146.41, 161.051];
    assert_eq!(series.len(), expected.len());
    for (i, (&actual, &exp)) in series.iter().zip(expected.iter()).enumerate() {
        assert!(
            (actual - exp).abs() < 1e-6,
            "step {i}: actual={actual}, expected={exp}"
        );
    }
}

#[test]
fn test_decay_model_with_small_dt() {
    // ds/dt = -s * 0.1, dt = 0.25, save_step = 0.25 so every step is saved
    let tp = TestProject::new_with_specs(
        "decay",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: datamodel::Dt::Dt(0.25),
            save_step: Some(datamodel::Dt::Dt(0.25)),
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
    )
    .flow("decay", "s * 0.1", None)
    .stock("s", "100", &[], &["decay"], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let series = vm.get_series(&Ident::new("s")).unwrap();

    // s(t+dt) = s(t) * (1 - 0.1*0.25) = s(t) * 0.975
    assert_eq!(series.len(), 5, "5 saved points at dt=0.25 from 0 to 1");
    let mut expected = 100.0;
    assert!((series[0] - expected).abs() < 1e-10);
    for (step, &actual) in series.iter().enumerate().skip(1) {
        expected *= 0.975;
        assert!(
            (actual - expected).abs() < 1e-10,
            "step {step}: actual={actual}, expected={expected}",
        );
    }
}

// ================================================================
// Reset with save_every > 1
// ================================================================

#[test]
fn test_reset_with_save_every_produces_identical_results() {
    let tp = TestProject::new_with_specs(
        "save_reset",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(0.5),
            save_step: Some(datamodel::Dt::Dt(2.0)),
            sim_method: datamodel::SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
    )
    .flow("inflow", "s * 0.1", None)
    .stock("s", "100", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);

    let mut vm_ref = Vm::new(compiled.clone()).unwrap();
    vm_ref.run_to_end().unwrap();
    let ref_series = vm_ref.get_series(&Ident::new("s")).unwrap();

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    vm.reset();
    vm.run_to_end().unwrap();
    let series = vm.get_series(&Ident::new("s")).unwrap();

    assert_eq!(ref_series.len(), series.len());
    for (step, (a, b)) in ref_series.iter().zip(series.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-10,
            "step {step}: reference={a} vs reset={b}"
        );
    }
}

#[test]
fn test_sshape_midpoint() {
    let result = apply(BuiltinId::Sshape, 0.0, 1.0, 0.5, 0.0, 100.0);
    assert!(
        (result - 50.0).abs() < 1e-10,
        "SSHAPE(0.5, 0, 100) should be 50, got {result}"
    );
}

#[test]
fn test_sshape_endpoints() {
    let at_zero = apply(BuiltinId::Sshape, 0.0, 1.0, 0.0, 0.0, 100.0);
    assert!(
        at_zero < 2.0,
        "SSHAPE(0, 0, 100) should approach 0, got {at_zero}"
    );

    let at_one = apply(BuiltinId::Sshape, 0.0, 1.0, 1.0, 0.0, 100.0);
    assert!(
        at_one > 98.0,
        "SSHAPE(1, 0, 100) should approach 100, got {at_one}"
    );
}

#[test]
fn test_sshape_custom_range() {
    let result = apply(BuiltinId::Sshape, 0.0, 1.0, 0.5, 10.0, 20.0);
    assert!(
        (result - 15.0).abs() < 1e-10,
        "SSHAPE(0.5, 10, 20) should be 15, got {result}"
    );
}

#[test]
fn test_quantum_positive() {
    let result = apply(BuiltinId::Quantum, 0.0, 1.0, 7.3, 2.0, 0.0);
    assert!(
        (result - 6.0).abs() < 1e-10,
        "QUANTUM(7.3, 2) should be 6, got {result}"
    );
}

#[test]
fn test_quantum_negative_truncates_toward_zero() {
    let result = apply(BuiltinId::Quantum, 0.0, 1.0, -0.9, 1.0, 0.0);
    assert!(
        result.abs() < 1e-10,
        "QUANTUM(-0.9, 1) should be 0 (truncate toward zero), got {result}"
    );

    let result2 = apply(BuiltinId::Quantum, 0.0, 1.0, -2.7, 1.0, 0.0);
    assert!(
        (result2 - (-2.0)).abs() < 1e-10,
        "QUANTUM(-2.7, 1) should be -2 (truncate toward zero), got {result2}"
    );
}

#[test]
fn test_quantum_zero_quantum_returns_input() {
    let result = apply(BuiltinId::Quantum, 0.0, 1.0, 3.7, 0.0, 0.0);
    assert!(
        (result - 3.7).abs() < 1e-10,
        "QUANTUM(3.7, 0) should return 3.7, got {result}"
    );
}
