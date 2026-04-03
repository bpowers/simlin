// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for Runge-Kutta integration methods (RK2 and RK4).
//! Verifies accuracy, convergence order, energy conservation, and
//! correct interaction with PREVIOUS(), INIT(), reset(), and save_step.

use crate::common::Ident;
use crate::datamodel;
use crate::test_common::TestProject;
use crate::vm::{CompiledSimulation, Vm};

fn build_compiled(tp: &TestProject) -> CompiledSimulation {
    tp.compile_incremental()
        .expect("incremental compile should succeed")
}

/// Build a harmonic oscillator model: x'=v, v'=-x.
/// Exact solution: x(t)=cos(t), v(t)=-sin(t), energy x^2+v^2=1.
fn harmonic_oscillator(method: datamodel::SimMethod) -> TestProject {
    TestProject::new("harmonic_oscillator")
        .with_sim_time(0.0, 50.0, 0.5)
        .with_sim_method(method)
        .flow("v_flow", "v", None)
        .flow("x_flow", "x", None)
        .stock("x", "1", &["v_flow"], &[], None)
        .stock("v", "0", &[], &["x_flow"], None)
}

/// Build an exponential decay model: s'=-0.1*s, s(0)=100.
/// Exact solution: s(t) = 100*exp(-0.1*t).
fn exponential_decay(method: datamodel::SimMethod) -> TestProject {
    TestProject::new("exp_decay")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_sim_method(method)
        .flow("drain", "s * 0.1", None)
        .stock("s", "100", &[], &["drain"], None)
}

// ── Euler baseline ────────────────────────────────────────────────

#[test]
fn euler_harmonic_oscillator_energy_grows() {
    let tp = harmonic_oscillator(datamodel::SimMethod::Euler);
    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let x_series = vm.get_series(&Ident::new("x")).unwrap();
    let v_series = vm.get_series(&Ident::new("v")).unwrap();
    let final_energy = {
        let x = x_series.last().unwrap();
        let v = v_series.last().unwrap();
        x * x + v * v
    };
    // Euler amplifies energy by (1+dt^2)^N per step.
    // With dt=0.5, N=100 steps: (1.25)^100 ~ 5e9.
    assert!(
        final_energy > 1000.0,
        "Euler should cause energy to grow dramatically, got {final_energy}"
    );
}

// ── RK4 tests ─────────────────────────────────────────────────────

#[test]
fn rk4_harmonic_oscillator_energy_conserved() {
    let tp = harmonic_oscillator(datamodel::SimMethod::RungeKutta4);
    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let x_series = vm.get_series(&Ident::new("x")).unwrap();
    let v_series = vm.get_series(&Ident::new("v")).unwrap();

    let mut max_energy: f64 = 0.0;
    for (&x, &v) in x_series.iter().zip(v_series.iter()) {
        let e = x * x + v * v;
        max_energy = max_energy.max(e);
    }
    assert!(
        max_energy < 1.01,
        "RK4 should conserve energy, max energy was {max_energy}"
    );

    // Check final x against analytical cos(50)
    let x_final = *x_series.last().unwrap();
    let expected = 50.0_f64.cos();
    assert!(
        (x_final - expected).abs() < 0.5,
        "RK4 x(50) = {x_final}, expected ~{expected}"
    );
}

#[test]
fn rk4_exponential_decay_accuracy() {
    let analytical = 100.0 * (-1.0_f64).exp(); // s(10) = 100*e^(-1)

    // Euler
    let euler_tp = exponential_decay(datamodel::SimMethod::Euler);
    let mut euler_vm = Vm::new(build_compiled(&euler_tp)).unwrap();
    euler_vm.run_to_end().unwrap();
    let euler_final = *euler_vm
        .get_series(&Ident::new("s"))
        .unwrap()
        .last()
        .unwrap();
    let euler_err = (euler_final - analytical).abs();

    // RK4
    let rk4_tp = exponential_decay(datamodel::SimMethod::RungeKutta4);
    let mut rk4_vm = Vm::new(build_compiled(&rk4_tp)).unwrap();
    rk4_vm.run_to_end().unwrap();
    let rk4_final = *rk4_vm.get_series(&Ident::new("s")).unwrap().last().unwrap();
    let rk4_err = (rk4_final - analytical).abs();

    assert!(
        rk4_err < euler_err,
        "RK4 error ({rk4_err}) should be less than Euler error ({euler_err})"
    );
    let rk4_rel_err = rk4_err / analytical;
    assert!(
        rk4_rel_err < 1e-5,
        "RK4 relative error should be < 0.001%, got {:.6}%",
        rk4_rel_err * 100.0
    );
}

#[test]
fn rk4_linear_growth_exact() {
    let tp = TestProject::new("linear_growth")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta4)
        .flow("inflow", "5", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let series = vm.get_series(&Ident::new("s")).unwrap();
    // s(t) = 5*t, saved at t=0,1,...,10
    for (i, &val) in series.iter().enumerate() {
        let expected = 5.0 * i as f64;
        assert!(
            (val - expected).abs() < 1e-10,
            "step {i}: got {val}, expected {expected}"
        );
    }
}

#[test]
fn rk4_reset_produces_identical_results() {
    let tp = exponential_decay(datamodel::SimMethod::RungeKutta4);
    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();

    vm.run_to_end().unwrap();
    let first_run = vm.get_series(&Ident::new("s")).unwrap();

    vm.reset();
    vm.run_to_end().unwrap();
    let second_run = vm.get_series(&Ident::new("s")).unwrap();

    assert_eq!(first_run.len(), second_run.len());
    for (i, (&a, &b)) in first_run.iter().zip(second_run.iter()).enumerate() {
        assert!(a == b, "step {i}: first run {a} != second run {b}");
    }
}

#[test]
fn rk4_with_save_step() {
    let tp = TestProject::new_with_specs(
        "rk4_save_step",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(0.25),
            save_step: Some(datamodel::Dt::Dt(1.0)),
            sim_method: datamodel::SimMethod::RungeKutta4,
            time_units: None,
        },
    )
    .flow("inflow", "5", None)
    .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let series = vm.get_series(&Ident::new("s")).unwrap();
    // save_step=1.0, dt=0.25: saves at t=0,1,...,10 -> 11 points
    assert_eq!(
        series.len(),
        11,
        "expected 11 save points, got {}",
        series.len()
    );
    // Linear growth is exact regardless of method
    for (i, &val) in series.iter().enumerate() {
        let expected = 5.0 * i as f64;
        assert!(
            (val - expected).abs() < 1e-10,
            "step {i}: got {val}, expected {expected}"
        );
    }
}

#[test]
fn rk4_previous_reads_previous_timestep() {
    let tp = TestProject::new("rk4_previous")
        .with_sim_time(0.0, 5.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta4)
        .flow("inflow", "10", None)
        .stock("s", "0", &["inflow"], &[], None)
        .aux("prev_s", "PREVIOUS(s, 0)", None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let s_series = vm.get_series(&Ident::new("s")).unwrap();
    let prev_series = vm.get_series(&Ident::new("prev_s")).unwrap();

    // s = [0, 10, 20, 30, 40, 50]
    // prev_s should lag by one step: [0, 0, 10, 20, 30, 40]
    assert_eq!(prev_series[0], 0.0, "PREVIOUS at t=0 should be fallback 0");
    for i in 1..s_series.len() {
        assert!(
            (prev_series[i] - s_series[i - 1]).abs() < 1e-10,
            "step {i}: prev_s={}, expected s[{}]={}",
            prev_series[i],
            i - 1,
            s_series[i - 1]
        );
    }
}

#[test]
fn rk4_no_stocks_matches_euler() {
    // A model with a constant stock (no flows) and aux computations
    let euler_tp = TestProject::new("euler_no_flow")
        .with_sim_time(0.0, 5.0, 1.0)
        .with_sim_method(datamodel::SimMethod::Euler)
        .stock("s", "42", &[], &[], None)
        .aux("doubled", "s * 2", None);

    let rk4_tp = TestProject::new("rk4_no_flow")
        .with_sim_time(0.0, 5.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta4)
        .stock("s", "42", &[], &[], None)
        .aux("doubled", "s * 2", None);

    let mut euler_vm = Vm::new(build_compiled(&euler_tp)).unwrap();
    euler_vm.run_to_end().unwrap();
    let euler_s = euler_vm.get_series(&Ident::new("s")).unwrap();
    let euler_d = euler_vm.get_series(&Ident::new("doubled")).unwrap();

    let mut rk4_vm = Vm::new(build_compiled(&rk4_tp)).unwrap();
    rk4_vm.run_to_end().unwrap();
    let rk4_s = rk4_vm.get_series(&Ident::new("s")).unwrap();
    let rk4_d = rk4_vm.get_series(&Ident::new("doubled")).unwrap();

    assert_eq!(euler_s, rk4_s, "constant stock should be identical");
    assert_eq!(euler_d, rk4_d, "derived aux should be identical");
}

// ── RK2 tests ─────────────────────────────────────────────────────

#[test]
fn rk2_harmonic_oscillator_better_than_euler() {
    let tp = harmonic_oscillator(datamodel::SimMethod::RungeKutta2);
    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let x_series = vm.get_series(&Ident::new("x")).unwrap();
    let v_series = vm.get_series(&Ident::new("v")).unwrap();
    let final_energy = {
        let x = x_series.last().unwrap();
        let v = v_series.last().unwrap();
        x * x + v * v
    };
    // RK2 should be vastly better than Euler (~5e9) but not as
    // tight as RK4 (<1.01). Energy stays bounded.
    assert!(
        final_energy < 100.0,
        "RK2 should keep energy much lower than Euler, got {final_energy}"
    );
}

#[test]
fn rk2_exponential_decay_accuracy() {
    let analytical = 100.0 * (-1.0_f64).exp();

    let euler_tp = exponential_decay(datamodel::SimMethod::Euler);
    let mut euler_vm = Vm::new(build_compiled(&euler_tp)).unwrap();
    euler_vm.run_to_end().unwrap();
    let euler_final = *euler_vm
        .get_series(&Ident::new("s"))
        .unwrap()
        .last()
        .unwrap();
    let euler_err = (euler_final - analytical).abs();

    let rk2_tp = exponential_decay(datamodel::SimMethod::RungeKutta2);
    let mut rk2_vm = Vm::new(build_compiled(&rk2_tp)).unwrap();
    rk2_vm.run_to_end().unwrap();
    let rk2_final = *rk2_vm.get_series(&Ident::new("s")).unwrap().last().unwrap();
    let rk2_err = (rk2_final - analytical).abs();

    let rk4_tp = exponential_decay(datamodel::SimMethod::RungeKutta4);
    let mut rk4_vm = Vm::new(build_compiled(&rk4_tp)).unwrap();
    rk4_vm.run_to_end().unwrap();
    let rk4_final = *rk4_vm.get_series(&Ident::new("s")).unwrap().last().unwrap();
    let rk4_err = (rk4_final - analytical).abs();

    // RK2 should be between Euler and RK4 in accuracy
    assert!(
        rk2_err < euler_err,
        "RK2 error ({rk2_err}) should be less than Euler error ({euler_err})"
    );
    assert!(
        rk4_err < rk2_err,
        "RK4 error ({rk4_err}) should be less than RK2 error ({rk2_err})"
    );
}

#[test]
fn rk2_linear_growth_exact() {
    let tp = TestProject::new("rk2_linear")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta2)
        .flow("inflow", "5", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let series = vm.get_series(&Ident::new("s")).unwrap();
    for (i, &val) in series.iter().enumerate() {
        let expected = 5.0 * i as f64;
        assert!(
            (val - expected).abs() < 1e-10,
            "step {i}: got {val}, expected {expected}"
        );
    }
}

#[test]
fn rk2_reset_produces_identical_results() {
    let tp = exponential_decay(datamodel::SimMethod::RungeKutta2);
    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();

    vm.run_to_end().unwrap();
    let first_run = vm.get_series(&Ident::new("s")).unwrap();

    vm.reset();
    vm.run_to_end().unwrap();
    let second_run = vm.get_series(&Ident::new("s")).unwrap();

    assert_eq!(first_run.len(), second_run.len());
    for (i, (&a, &b)) in first_run.iter().zip(second_run.iter()).enumerate() {
        assert!(a == b, "step {i}: first run {a} != second run {b}");
    }
}

// ── Convergence order test ────────────────────────────────────────

/// Helper: run exponential decay with given method and dt, return final error.
fn exp_decay_error(method: datamodel::SimMethod, dt: f64) -> f64 {
    let analytical = 100.0 * (-1.0_f64).exp();
    let tp = TestProject::new_with_specs(
        "conv",
        datamodel::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: datamodel::Dt::Dt(dt),
            save_step: None,
            sim_method: method,
            time_units: None,
        },
    )
    .flow("drain", "s * 0.1", None)
    .stock("s", "100", &[], &["drain"], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let final_val = *vm.get_series(&Ident::new("s")).unwrap().last().unwrap();
    (final_val - analytical).abs()
}

#[test]
fn convergence_order_euler_rk2_rk4() {
    // When halving dt, error should decrease by:
    // Euler: ~2x (1st order)
    // RK2:   ~4x (2nd order)
    // RK4:   ~16x (4th order)
    let dt_coarse = 1.0;
    let dt_fine = 0.5;

    let euler_coarse = exp_decay_error(datamodel::SimMethod::Euler, dt_coarse);
    let euler_fine = exp_decay_error(datamodel::SimMethod::Euler, dt_fine);
    let euler_ratio = euler_coarse / euler_fine;
    assert!(
        (1.5..=2.5).contains(&euler_ratio),
        "Euler convergence ratio should be ~2, got {euler_ratio:.2}"
    );

    let rk2_coarse = exp_decay_error(datamodel::SimMethod::RungeKutta2, dt_coarse);
    let rk2_fine = exp_decay_error(datamodel::SimMethod::RungeKutta2, dt_fine);
    let rk2_ratio = rk2_coarse / rk2_fine;
    assert!(
        (3.0..=5.0).contains(&rk2_ratio),
        "RK2 convergence ratio should be ~4, got {rk2_ratio:.2}"
    );

    let rk4_coarse = exp_decay_error(datamodel::SimMethod::RungeKutta4, dt_coarse);
    let rk4_fine = exp_decay_error(datamodel::SimMethod::RungeKutta4, dt_fine);
    let rk4_ratio = rk4_coarse / rk4_fine;
    assert!(
        (10.0..=25.0).contains(&rk4_ratio),
        "RK4 convergence ratio should be ~16, got {rk4_ratio:.2}"
    );
}

// ── Edge case tests ───────────────────────────────────────────────

#[test]
fn rk4_partial_run_then_continue() {
    let tp = exponential_decay(datamodel::SimMethod::RungeKutta4);

    // Full run
    let mut vm_full = Vm::new(build_compiled(&tp)).unwrap();
    vm_full.run_to_end().unwrap();
    let full_series = vm_full.get_series(&Ident::new("s")).unwrap();

    // Partial + continue
    let mut vm_partial = Vm::new(build_compiled(&tp)).unwrap();
    vm_partial.run_to(5.0).unwrap();
    vm_partial.run_to(10.0).unwrap();
    let partial_series = vm_partial.get_series(&Ident::new("s")).unwrap();

    assert_eq!(full_series.len(), partial_series.len());
    for (i, (&a, &b)) in full_series.iter().zip(partial_series.iter()).enumerate() {
        assert!((a - b).abs() < 1e-10, "step {i}: full={a}, partial={b}");
    }
}

#[test]
fn rk4_quadratic_growth() {
    // inflow = TIME, so s(t) = integral(TIME, 0, t) = t^2/2.
    // RK4 integrates polynomials up to degree 4 exactly.
    let tp = TestProject::new("quadratic")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta4)
        .flow("inflow", "TIME", None)
        .stock("s", "0", &["inflow"], &[], None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let series = vm.get_series(&Ident::new("s")).unwrap();
    for (i, &val) in series.iter().enumerate() {
        let t = i as f64;
        let expected = t * t / 2.0;
        assert!(
            (val - expected).abs() < 1e-10,
            "step {i}: got {val}, expected {expected}"
        );
    }
}

#[test]
fn rk4_with_init_builtin() {
    let tp = TestProject::new("rk4_init")
        .with_sim_time(0.0, 5.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta4)
        .flow("inflow", "10", None)
        .stock("s", "42", &["inflow"], &[], None)
        .aux("init_s", "INIT(s)", None);

    let compiled = build_compiled(&tp);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();

    let init_series = vm.get_series(&Ident::new("init_s")).unwrap();
    // INIT(s) should always return 42 (the initial value)
    for (i, &val) in init_series.iter().enumerate() {
        assert!(
            (val - 42.0).abs() < 1e-10,
            "step {i}: INIT(s) = {val}, expected 42"
        );
    }
}
