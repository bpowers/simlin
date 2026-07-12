// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double};
use std::ptr;
use std::sync::atomic::Ordering;

use prost::Message;
use simlin::*;
use simlin_engine::serde as engine_serde;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use crate::common::{expect_error_code, expect_no_error, open_project_from_datamodel};

/// Interactive set/get against a live VM: run part-way, override a simple
/// constant, and read the new value back.
///
/// Historically this targeted the `infectious` stock, from an era when
/// `set_value` wrote any variable's current value; today `set_value` is a
/// constants-only override (BadOverride otherwise), so it targets the
/// `contact_infectivity` constant and additionally pins the stock rejection.
#[test]
fn test_interactive_set_get() {
    // Load the SIR project fixture. This must be a hard failure, not a skip:
    // a prior revision pointed at a nonexistent path and silently returned,
    // so the test passed while exercising nothing.
    let pb_path = std::path::Path::new("testdata/SIR_project.pb");
    let data = std::fs::read(pb_path).expect("SIR_project.pb fixture must exist");

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "project open");
        assert!(!proj.is_null());

        err = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
        expect_no_error(err, "get_model");
        assert!(!model.is_null());

        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "sim_new");
        assert!(!sim.is_null());

        err = ptr::null_mut();
        simlin_sim_run_to(sim, 0.125, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "run_to(0.125)");

        // The var-name listing must contain the constant we are about to set.
        err = ptr::null_mut();
        let mut count: usize = 0;
        simlin_sim_get_var_count(
            sim,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "get_var_count");
        assert!(count > 0, "expected varcount > 0");

        let mut name_ptrs: Vec<*mut c_char> = vec![std::ptr::null_mut(); count];
        err = ptr::null_mut();
        simlin_sim_get_var_names(
            sim,
            name_ptrs.as_mut_ptr(),
            name_ptrs.len(),
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "get_var_names");

        let mut names: Vec<String> = Vec::with_capacity(count);
        for &p in name_ptrs.iter().take(count) {
            assert!(!p.is_null());
            names.push(CStr::from_ptr(p).to_string_lossy().into_owned());
            simlin_free_string(p);
        }
        assert!(
            names.iter().any(|n| n == "contact_infectivity"),
            "contact_infectivity not in {names:?}"
        );

        // Override the constant on the live VM and read it back.
        let c_const = CString::new("contact_infectivity").unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_const.as_ptr(),
            0.9,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "set_value(contact_infectivity)");
        assert_sim_value(sim, "contact_infectivity", 0.9, 1e-9);

        // A stock is not a simple constant: the live-VM path must reject it.
        let c_stock = CString::new("infectious").unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_stock.as_ptr(),
            42.0,
            &mut err as *mut *mut SimlinError,
        );
        expect_error_code(err, SimlinErrorCode::BadOverride, "set_value(infectious)");

        // Cleanup
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Pins `simlin_sim_set_value` semantics across the sim lifecycle:
///
/// 1. Before any run: overrides a simple constant on the live VM.
/// 2. Mid-run (after a partial `run_to`): same.
/// 3. After `run_to_end` (the VM has been consumed into results): a constant
///    override is ACCEPTED and staged -- it does not alter the saved results,
///    but applies to the VM recreated by the next `simlin_sim_reset` (the
///    documented contract; see also `test_libsimlin_set_value_when_vm_is_none`).
///    Non-constants still reject with BadOverride and unknown names with
///    DoesNotExist on that no-VM path.
/// 4. After reset + rerun the staged override is visible in the new results.
///
/// An earlier revision asserted phase 3 fails with NotSimulatable; that
/// reflected a long-gone API and never actually ran (the fixture path was
/// stale, so the whole test silently skipped).
#[test]
fn test_set_value_phases() {
    // Load the SIR project fixture (hard failure, not a skip -- see
    // test_interactive_set_get).
    let pb_path = std::path::Path::new("testdata/SIR_project.pb");
    let data = std::fs::read(pb_path).expect("SIR_project.pb fixture must exist");

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "project open");
        assert!(!proj.is_null());

        err = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
        expect_no_error(err, "get_model");
        assert!(!model.is_null());

        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "sim_new");
        assert!(!sim.is_null());

        let c_const = CString::new("contact_infectivity").unwrap();
        let c_stock = CString::new("infectious").unwrap();

        // Phase 1: override before any run. get_value requires initials to
        // have run (the VM's data buffer is unspecified before then), so run
        // just the initials phase before reading the override back.
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_const.as_ptr(),
            0.9,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "set_value before run");
        err = ptr::null_mut();
        simlin_sim_run_initials(sim, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "run_initials");
        assert_sim_value(sim, "contact_infectivity", 0.9, 1e-9);

        // Phase 2: override mid-run.
        err = ptr::null_mut();
        simlin_sim_run_to(sim, 0.5, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "run_to(0.5)");
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_const.as_ptr(),
            0.15,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "set_value during run");
        assert_sim_value(sim, "contact_infectivity", 0.15, 1e-9);

        // Phase 3: run_to_end consumes the VM into results. A constant
        // override is still accepted -- staged for the next reset -- and the
        // already-saved results are untouched by the staging.
        run_to_end(sim);
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_const.as_ptr(),
            0.05,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "set_value after run_to_end");
        assert_sim_value(sim, "contact_infectivity", 0.15, 1e-9);

        // The no-VM path validates like the live-VM path: non-constants
        // reject with BadOverride, unknown names with DoesNotExist.
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_stock.as_ptr(),
            300.0,
            &mut err as *mut *mut SimlinError,
        );
        expect_error_code(
            err,
            SimlinErrorCode::BadOverride,
            "set_value(stock) after run_to_end",
        );
        let unknown = CString::new("unknown_variable_xyz").unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            unknown.as_ptr(),
            999.0,
            &mut err as *mut *mut SimlinError,
        );
        expect_error_code(
            err,
            SimlinErrorCode::DoesNotExist,
            "set_value(unknown) after run_to_end",
        );

        // Phase 4: reset recreates the VM with the staged override applied.
        reset_sim(sim);
        run_to_end(sim);
        assert_sim_value(sim, "contact_infectivity", 0.05, 1e-9);

        // Cleanup
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_concurrent_project_ref_unref() {
    use std::thread;

    unsafe {
        // Create a test project
        let datamodel = TestProject::new("concurrent_test").build_datamodel();
        let pb_project = engine_serde::serialize(&datamodel).unwrap();
        let encoded = pb_project.encode_to_vec();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            encoded.as_ptr(),
            encoded.len(),
            &mut err as *mut *mut SimlinError,
        );

        expect_no_error(err, "project open");
        assert!(!proj.is_null());

        // Add many references from multiple threads
        const NUM_THREADS: usize = 10;
        const REFS_PER_THREAD: usize = 100;

        let mut handles = vec![];

        // Spawn threads that will add and remove references
        for _ in 0..NUM_THREADS {
            // Cast to usize to make it Send
            let proj_addr = proj as usize;
            let handle = thread::spawn(move || {
                let proj_ptr = proj_addr as *mut SimlinProject;
                for _ in 0..REFS_PER_THREAD {
                    simlin_project_ref(proj_ptr);
                    simlin_project_unref(proj_ptr);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Reference count should be back to 1
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

        // Clean up
        simlin_project_unref(proj);
    }
}

#[test]
fn test_concurrent_model_creation() {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use std::sync::Arc;
    use std::thread;

    unsafe {
        // Create a test project
        let datamodel = TestProject::new("concurrent_model").build_datamodel();
        let pb_project = engine_serde::serialize(&datamodel).unwrap();
        let encoded = pb_project.encode_to_vec();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            encoded.as_ptr(),
            encoded.len(),
            &mut err as *mut *mut SimlinError,
        );

        expect_no_error(err, "project open");
        assert!(!proj.is_null());

        const NUM_THREADS: usize = 8;
        let success_count = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];

        // Spawn threads that create and destroy models
        for _ in 0..NUM_THREADS {
            let proj_addr = proj as usize;
            let success = Arc::clone(&success_count);
            let handle = thread::spawn(move || {
                let proj_ptr = proj_addr as *mut SimlinProject;
                for _ in 0..10 {
                    let mut err: *mut SimlinError = ptr::null_mut();
                    let model = simlin_project_get_model(
                        proj_ptr,
                        ptr::null(),
                        &mut err as *mut *mut SimlinError,
                    );

                    if !err.is_null() {
                        simlin_error_free(err);
                        continue;
                    }

                    if model.is_null() {
                        continue;
                    }

                    success.fetch_add(1, AtomicOrdering::SeqCst);

                    // Use the model briefly
                    let mut var_count: usize = 0;
                    let mut err_count: *mut SimlinError = ptr::null_mut();
                    simlin_model_get_var_count(
                        model,
                        0,
                        ptr::null(),
                        &mut var_count as *mut usize,
                        &mut err_count as *mut *mut SimlinError,
                    );
                    if !err_count.is_null() {
                        simlin_error_free(err_count);
                    }

                    simlin_model_unref(model);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Should have had successful model creations
        assert!(success_count.load(AtomicOrdering::SeqCst) > 0);

        // Clean up
        simlin_project_unref(proj);
    }
}

#[test]
fn test_concurrent_sim_operations() {
    use std::thread;

    unsafe {
        // Create a test project with a simple model
        let datamodel = TestProject::new("concurrent_sim")
            .stock("inventory", "0", &[], &[], None)
            .flow("production", "5", None)
            .build_datamodel();
        let pb_project = engine_serde::serialize(&datamodel).unwrap();
        let encoded = pb_project.encode_to_vec();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            encoded.as_ptr(),
            encoded.len(),
            &mut err as *mut *mut SimlinError,
        );

        expect_no_error(err, "project open");
        assert!(!proj.is_null());

        // Get model
        let mut err_model: *mut SimlinError = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, ptr::null(), &mut err_model as *mut *mut SimlinError);
        expect_no_error(err_model, "get_model");

        const NUM_THREADS: usize = 5;
        let mut handles = vec![];

        // Spawn threads that create and run simulations
        for _ in 0..NUM_THREADS {
            let model_addr = model as usize;
            let handle = thread::spawn(move || {
                let model_ptr = model_addr as *mut SimlinModel;
                for _ in 0..5 {
                    let mut err_sim: *mut SimlinError = ptr::null_mut();
                    let sim =
                        simlin_sim_new(model_ptr, false, &mut err_sim as *mut *mut SimlinError);

                    if !err_sim.is_null() {
                        simlin_error_free(err_sim);
                        continue;
                    }

                    if sim.is_null() {
                        continue;
                    }

                    // Run simulation
                    let mut err_run: *mut SimlinError = ptr::null_mut();
                    simlin_sim_run_to_end(sim, &mut err_run as *mut *mut SimlinError);
                    if !err_run.is_null() {
                        simlin_error_free(err_run);
                    }

                    simlin_sim_unref(sim);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Clean up
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_stress_ref_counting() {
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::thread;

    unsafe {
        // Create a test project
        let datamodel = TestProject::new("stress_test")
            .stock("s", "10", &[], &[], None)
            .build_datamodel();
        let pb_project = engine_serde::serialize(&datamodel).unwrap();
        let encoded = pb_project.encode_to_vec();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            encoded.as_ptr(),
            encoded.len(),
            &mut err as *mut *mut SimlinError,
        );

        expect_no_error(err, "project open");
        assert!(!proj.is_null());

        const NUM_THREADS: usize = 20;
        const ITERATIONS: usize = 50;
        let barrier = Arc::new(Barrier::new(NUM_THREADS));
        let mut handles = vec![];

        // Spawn threads that stress test the ref counting
        for thread_id in 0..NUM_THREADS {
            let proj_addr = proj as usize;
            let barrier = Arc::clone(&barrier);
            let handle = thread::spawn(move || {
                // Wait for all threads to be ready
                barrier.wait();

                let proj_ptr = proj_addr as *mut SimlinProject;
                for _ in 0..ITERATIONS {
                    // Create model
                    let mut err_model: *mut SimlinError = ptr::null_mut();
                    let model = simlin_project_get_model(
                        proj_ptr,
                        ptr::null(),
                        &mut err_model as *mut *mut SimlinError,
                    );

                    if !err_model.is_null() {
                        simlin_error_free(err_model);
                        continue;
                    }

                    if model.is_null() {
                        continue;
                    }

                    // Ref and unref the model multiple times
                    for _ in 0..5 {
                        simlin_model_ref(model);
                    }
                    for _ in 0..5 {
                        simlin_model_unref(model);
                    }

                    // Create sim on every other iteration
                    if thread_id % 2 == 0 {
                        let mut err_sim: *mut SimlinError = ptr::null_mut();
                        let sim =
                            simlin_sim_new(model, false, &mut err_sim as *mut *mut SimlinError);

                        if !err_sim.is_null() {
                            simlin_error_free(err_sim);
                        } else if !sim.is_null() {
                            simlin_sim_unref(sim);
                        }
                    }

                    simlin_model_unref(model);
                }
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Final ref count should be 1
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

        // Clean up
        simlin_project_unref(proj);
    }
}

/// Returns (proj, model, sim) — caller is responsible for unref'ing all three.
unsafe fn create_test_sim(
    datamodel: &engine::datamodel::Project,
) -> (*mut SimlinProject, *mut SimlinModel, *mut SimlinSim) {
    let proj = open_project_from_datamodel(datamodel);
    let mut err: *mut SimlinError = ptr::null_mut();
    let model = simlin_project_get_model(proj, ptr::null(), &mut err as *mut *mut SimlinError);
    assert!(err.is_null(), "get_model failed");
    assert!(!model.is_null());

    err = ptr::null_mut();
    let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
    assert!(err.is_null(), "sim_new failed");
    assert!(!sim.is_null());

    (proj, model, sim)
}

/// Helper: assert that `simlin_sim_get_value` returns `expected` for `name`.
unsafe fn assert_sim_value(sim: *mut SimlinSim, name: &str, expected: f64, tol: f64) {
    let c_name = CString::new(name).unwrap();
    let mut out: c_double = 0.0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_get_value(
        sim,
        c_name.as_ptr(),
        &mut out,
        &mut err as *mut *mut SimlinError,
    );
    expect_no_error(err, &format!("get_value('{name}')"));
    assert!(
        (out - expected).abs() <= tol,
        "get_value('{}') = {}, expected {} (tol={})",
        name,
        out,
        expected,
        tol,
    );
}

/// Helper: run sim to end and assert success.
unsafe fn run_to_end(sim: *mut SimlinSim) {
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
    expect_no_error(err, "run_to_end");
}

/// Helper: reset the sim and assert success.
unsafe fn reset_sim(sim: *mut SimlinSim) {
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_reset(sim, &mut err as *mut *mut SimlinError);
    expect_no_error(err, "reset");
}

/// Helper: get the time series for a variable, returning a Vec<f64>.
unsafe fn get_series_vec(sim: *mut SimlinSim, name: &str, max_len: usize) -> Vec<f64> {
    let c_name = CString::new(name).unwrap();
    let mut buf = vec![0.0f64; max_len];
    let mut written: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_get_series(
        sim,
        c_name.as_ptr(),
        buf.as_mut_ptr(),
        max_len,
        &mut written,
        &mut err as *mut *mut SimlinError,
    );
    expect_no_error(err, &format!("get_series('{name}')"));
    buf.truncate(written);
    buf
}

/// Helper: resolve a variable's data-buffer offset via `simlin_sim_get_offset`.
unsafe fn get_offset(sim: *mut SimlinSim, name: &str) -> usize {
    let c_name = CString::new(name).unwrap();
    let mut off: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_get_offset(
        sim,
        c_name.as_ptr(),
        &mut off as *mut usize,
        &mut err as *mut *mut SimlinError,
    );
    expect_no_error(err, &format!("get_offset('{name}')"));
    off
}

fn build_population_datamodel() -> engine::datamodel::Project {
    // birth_rate and lifespan feed into initial_pop, which is the stock
    // initial, so all three are "initial variables" and can be overridden.
    TestProject::new("pop_test")
        .with_sim_time(0.0, 100.0, 1.0)
        .aux("birth_rate", "0.1", None)
        .aux("lifespan", "80", None)
        .aux("initial_pop", "1000 * birth_rate", None)
        .stock("population", "initial_pop", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population / lifespan", None)
        .build_datamodel()
}

#[test]
fn test_libsimlin_reset_preserves_compilation() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // First run
        run_to_end(sim);
        let series1 = get_series_vec(sim, "population", 200);
        assert!(!series1.is_empty());

        // Reset and run again
        reset_sim(sim);
        run_to_end(sim);
        let series2 = get_series_vec(sim, "population", 200);

        assert_eq!(series1.len(), series2.len());
        for (a, b) in series1.iter().zip(series2.iter()) {
            assert!((a - b).abs() < 1e-9, "mismatch: {} vs {}", a, b,);
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_set_value_survives_run_to_end() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Set value for birth_rate
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(sim, c_name.as_ptr(), 0.2, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "set_value failed");

        // run_to_end consumes the VM
        run_to_end(sim);
        let series_overridden = get_series_vec(sim, "population", 200);

        // Reset — recreates VM from cached compiled, re-applies overrides
        reset_sim(sim);
        run_to_end(sim);
        let series_after_reset = get_series_vec(sim, "population", 200);

        assert_eq!(series_overridden.len(), series_after_reset.len());
        for (a, b) in series_overridden.iter().zip(series_after_reset.iter()) {
            assert!(
                (a - b).abs() < 1e-9,
                "override not re-applied after reset: {} vs {}",
                a,
                b,
            );
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_set_value_when_vm_is_none() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Consume the VM
        run_to_end(sim);

        // Set value while VM is None
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(sim, c_name.as_ptr(), 0.3, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "set_value with no VM should succeed");

        // Reset creates a new VM with the value applied
        reset_sim(sim);
        run_to_end(sim);

        // Verify the set value took effect by comparing against default
        let series_overridden = get_series_vec(sim, "population", 200);

        // Reset with no value set to get baseline
        err = ptr::null_mut();
        simlin_sim_clear_values(sim, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        reset_sim(sim);
        run_to_end(sim);
        let series_default = get_series_vec(sim, "population", 200);

        // With birth_rate=0.3 vs 0.1, population should grow much faster
        let final_overridden = *series_overridden.last().unwrap();
        let final_default = *series_default.last().unwrap();
        assert!(
            final_overridden > final_default,
            "override should increase final population: {} vs {}",
            final_overridden,
            final_default,
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_run_initials() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_run_initials(sim, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "run_initials failed");

        // initial_pop = 1000 * 0.1 = 100
        assert_sim_value(sim, "population", 100.0, 1e-9);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_get_series_after_partial_run() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Run to t=50
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_run_to(sim, 50.0, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "run_to(50) failed");

        let series = get_series_vec(sim, "population", 200);
        // Should have 51 points (t=0..50 inclusive with dt=1, save_step=1)
        assert_eq!(series.len(), 51);
        assert!((series[0] - 100.0).abs() < 1e-9);

        // Continue to end
        run_to_end(sim);
        let full_series = get_series_vec(sim, "population", 200);
        // Should have 101 points (t=0..100 with dt=1)
        assert_eq!(full_series.len(), 101);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_set_value_flows_through_dependents() {
    let dm = TestProject::new("override_flow")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "scaled_rate", &["growth"], &[], None)
        .flow("growth", "population * 0.01", None)
        .aux("rate", "5", None)
        .aux("scaled_rate", "rate * 10", None)
        .build_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Override rate from 5 to 20
        let c_name = CString::new("rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_name.as_ptr(),
            20.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "set_value failed");

        simlin_sim_run_initials(sim, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "run_initials failed");

        // scaled_rate should be 20*10=200, and population initial = scaled_rate = 200
        assert_sim_value(sim, "rate", 20.0, 1e-9);
        assert_sim_value(sim, "scaled_rate", 200.0, 1e-9);
        assert_sim_value(sim, "population", 200.0, 1e-9);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_clear_values_restores_defaults() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Get default series
        run_to_end(sim);
        let series_default = get_series_vec(sim, "population", 200);

        // Override, reset, run
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(sim, c_name.as_ptr(), 0.5, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        reset_sim(sim);
        run_to_end(sim);
        let series_overridden = get_series_vec(sim, "population", 200);

        // Clear overrides, reset, run — should match default
        err = ptr::null_mut();
        simlin_sim_clear_values(sim, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        reset_sim(sim);
        run_to_end(sim);
        let series_restored = get_series_vec(sim, "population", 200);

        // Overridden should differ from default
        let final_default = *series_default.last().unwrap();
        let final_overridden = *series_overridden.last().unwrap();
        assert!(
            (final_default - final_overridden).abs() > 1.0,
            "override should have changed results",
        );

        // Restored should match default
        assert_eq!(series_default.len(), series_restored.len());
        for (a, b) in series_default.iter().zip(series_restored.iter()) {
            assert!(
                (a - b).abs() < 1e-9,
                "restored should match default: {} vs {}",
                a,
                b,
            );
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_set_value_validates_without_vm() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Consume the VM so we exercise the no-VM validation path
        run_to_end(sim);

        // Setting a non-constant variable (flow) by name should fail
        let c_births = CString::new("births").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_births.as_ptr(),
            42.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "non-constant variable should fail even without a VM"
        );
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadOverride);
        simlin_error_free(err);

        // Setting a nonexistent variable should fail
        let c_nonexistent = CString::new("nonexistent_var").unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_nonexistent.as_ptr(),
            42.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "nonexistent variable should fail");
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::DoesNotExist);
        simlin_error_free(err);

        // Setting a constant variable (birth_rate) should succeed
        let c_rate = CString::new("birth_rate").unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value(sim, c_rate.as_ptr(), 0.5, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "constant variable should succeed without VM");

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// `simlin_sim_set_value_by_offset` edits the LAST SAVED RESULTS ROW, and must
/// apply the same simple-constant gate as `simlin_sim_set_value`: without it,
/// any computed column (a flow, a stock, an LTM loop score) could be silently
/// rewritten in the saved results where the by-name path rejects with
/// BadOverride.
#[test]
fn test_set_value_by_offset_gates_on_constant() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Before any results exist the call must error (there is no row to
        // edit; the live-VM constant-override path is simlin_sim_set_value).
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value_by_offset(sim, 0, 1.0, &mut err as *mut *mut SimlinError);
        assert!(!err.is_null(), "pre-run set_value_by_offset must error");
        simlin_error_free(err);

        run_to_end(sim);

        // A simple-constant offset is accepted: the final saved row changes,
        // earlier rows do not.
        let off_rate = get_offset(sim, "birth_rate");
        err = ptr::null_mut();
        simlin_sim_set_value_by_offset(sim, off_rate, 0.42, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "set_value_by_offset(birth_rate)");
        let rate_series = get_series_vec(sim, "birth_rate", 200);
        assert!(
            (rate_series[0] - 0.1).abs() < 1e-12,
            "earlier rows untouched"
        );
        assert!(
            (rate_series.last().unwrap() - 0.42).abs() < 1e-12,
            "last saved row must reflect the write"
        );

        // A computed offset (flow) rejects with BadOverride and the saved
        // value is unchanged.
        let off_births = get_offset(sim, "births");
        let births_before = *get_series_vec(sim, "births", 200).last().unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value_by_offset(sim, off_births, 1234.5, &mut err as *mut *mut SimlinError);
        expect_error_code(
            err,
            SimlinErrorCode::BadOverride,
            "set_value_by_offset(births)",
        );
        let births_after = *get_series_vec(sim, "births", 200).last().unwrap();
        assert_eq!(births_before, births_after, "rejected write must not land");

        // Out-of-bounds offsets always error.
        err = ptr::null_mut();
        simlin_sim_set_value_by_offset(sim, 1 << 20, 1.0, &mut err as *mut *mut SimlinError);
        assert!(!err.is_null(), "out-of-bounds offset must error");
        simlin_error_free(err);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// The pass-driven flows of a conveyor model compile to placeholder constants
/// but are overwritten by the conveyor pass every step; GH #871 retracts them
/// from the overridable set. `simlin_sim_set_value_by_offset` must honor that
/// retraction: `graduating` (the pass-driven primary outflow) rejects with
/// BadOverride while `matriculating` (an ordinary constant inflow) is
/// writable.
#[test]
fn test_set_value_by_offset_rejects_pass_driven_flow() {
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile");
    unsafe {
        let (proj, model, sim) = create_test_sim(&datamodel);
        run_to_end(sim);

        let off_grad = get_offset(sim, "graduating");
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value_by_offset(sim, off_grad, 999.0, &mut err as *mut *mut SimlinError);
        expect_error_code(
            err,
            SimlinErrorCode::BadOverride,
            "set_value_by_offset(graduating)",
        );
        let grad_last = *get_series_vec(sim, "graduating", 4096).last().unwrap();
        assert!(
            (grad_last - 250.0).abs() < 1e-6,
            "pass-driven flow's saved value must be unchanged, got {grad_last}"
        );

        let off_matric = get_offset(sim, "matriculating");
        err = ptr::null_mut();
        simlin_sim_set_value_by_offset(sim, off_matric, 123.0, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "set_value_by_offset(matriculating)");
        let matric_last = *get_series_vec(sim, "matriculating", 4096).last().unwrap();
        assert!(
            (matric_last - 123.0).abs() < 1e-12,
            "constant inflow's saved value must reflect the write, got {matric_last}"
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_multiple_reset_set_value_cycles() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);
        let c_name = CString::new("birth_rate").unwrap();

        let mut prev_final = 0.0;
        for i in 1..=10 {
            let rate = i as f64 * 0.02;
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_sim_set_value(
                sim,
                c_name.as_ptr(),
                rate,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());

            reset_sim(sim);
            run_to_end(sim);

            let series = get_series_vec(sim, "population", 200);
            let final_val = *series.last().unwrap();
            if i > 1 {
                assert!(
                    final_val > prev_final,
                    "final population should increase with birth_rate: rate={}, final={}, prev={}",
                    rate,
                    final_val,
                    prev_final,
                );
            }
            prev_final = final_val;
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_sim_get_var_count_and_names() {
    let datamodel = TestProject::new("sim_vars")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .aux("growth_rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut err);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Get count
        let mut count: usize = 0;
        simlin_sim_get_var_count(sim, &mut count, &mut err);
        assert!(err.is_null(), "get_var_count should succeed");
        assert!(count > 0, "expected at least one sim var");

        // Verify no internal ($-prefixed) vars are counted: the count
        // should match the number of names returned.
        let mut name_ptrs: Vec<*mut c_char> = vec![ptr::null_mut(); count];
        let mut written: usize = 0;
        simlin_sim_get_var_names(sim, name_ptrs.as_mut_ptr(), count, &mut written, &mut err);
        assert!(err.is_null(), "get_var_names should succeed");
        assert_eq!(written, count, "written count must match var count");

        let mut names: Vec<String> = Vec::with_capacity(written);
        for &p in &name_ptrs[..written] {
            assert!(!p.is_null());
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            assert!(
                !s.starts_with('$'),
                "internal var '{}' should be filtered out",
                s,
            );
            names.push(s);
            simlin_free_string(p);
        }

        // Names should be sorted
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "sim var names should be sorted");

        // All model-level variables should appear (possibly flattened)
        for expected in &["population", "births", "deaths", "growth_rate"] {
            assert!(
                names.iter().any(|n| n.contains(expected)),
                "expected '{}' in sim var names {:?}",
                expected,
                names,
            );
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

// ── Phase 7: Incremental compilation integration tests ─────────────────

#[test]
fn test_sim_lifecycle() {
    // Create a minimal valid protobuf project
    let project = engine::project_io::Project {
        name: "test".to_string(),
        sim_specs: Some(engine::project_io::SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Some(engine::project_io::Dt {
                value: 1.0,
                is_reciprocal: false,
            }),
            save_step: None,
            sim_method: engine::project_io::SimMethod::Euler as i32,
            time_units: None,
        }),
        models: vec![engine::project_io::Model {
            name: "main".to_string(),
            variables: vec![engine::project_io::Variable {
                v: Some(engine::project_io::variable::V::Aux(
                    engine::project_io::variable::Aux {
                        ident: "time".to_string(),
                        equation: Some(engine::project_io::variable::Equation {
                            equation: Some(
                                engine::project_io::variable::equation::Equation::Scalar(
                                    engine::project_io::variable::ScalarEquation {
                                        equation: "time".to_string(),
                                        initial_equation: None,
                                    },
                                ),
                            ),
                        }),
                        documentation: String::new(),
                        units: String::new(),
                        gf: None,
                        can_be_module_input: false,
                        visibility: engine::project_io::variable::Visibility::Private as i32,
                        uid: 0,
                        compat: None,
                    },
                )),
            }],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        dimensions: vec![],
        units: vec![],
        source: None,
    };
    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            buf.as_ptr(),
            buf.len(),
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "project open");
        assert!(!proj.is_null());
        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        expect_no_error(err_get_model, "get_model");
        assert!(!model.is_null());
        // Project ref count should have increased when model was created
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);

        // Test model reference counting
        simlin_model_ref(model);
        assert_eq!((*model).ref_count.load(Ordering::SeqCst), 2);
        simlin_model_unref(model);
        assert_eq!((*model).ref_count.load(Ordering::SeqCst), 1);

        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());
        // Model ref count should have increased when sim was created
        assert_eq!((*model).ref_count.load(Ordering::SeqCst), 2);

        // Test sim reference counting
        simlin_sim_ref(sim);
        assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 2);
        simlin_sim_unref(sim);
        assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 1);
        simlin_sim_unref(sim);
        // Sim should be freed now, model ref count should decrease
        assert_eq!((*model).ref_count.load(Ordering::SeqCst), 1);

        simlin_model_unref(model);
        // Model should be freed now, project ref count should decrease
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_ltm_enabled_sim() {
    // Create a project with a feedback loop
    let test_project = TestProject::new("test_ltm")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * 0.02", None);

    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project).unwrap();

    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            buf.as_ptr(),
            buf.len(),
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "project open");
        assert!(!proj.is_null());

        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        expect_no_error(err_get_model, "get_model");
        assert!(!model.is_null());

        // Create simulation with LTM enabled
        err = ptr::null_mut();
        let sim_ltm = simlin_sim_new(model, true, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim_ltm.is_null());

        // Run simulation
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim_ltm, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // Create another sim without LTM
        err = ptr::null_mut();
        let sim_no_ltm = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim_no_ltm.is_null());

        // Run this one too
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim_no_ltm, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // Clean up
        simlin_sim_unref(sim_ltm);
        simlin_sim_unref(sim_no_ltm);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// GH #486: enabling LTM on a model with a non-Euler integration method must
/// fail `simlin_sim_new` cleanly with a readable error referencing the Euler
/// assumption -- not silently produce mathematically-wrong link scores. The
/// same model with LTM disabled must still compile and simulate.
#[test]
fn test_ltm_non_euler_sim_fails_cleanly() {
    let datamodel = TestProject::new("ltm_rk4")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_sim_method(engine::datamodel::SimMethod::RungeKutta4)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * 0.02", None)
        .build_datamodel();

    let proj = open_project_from_datamodel(&datamodel);
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!model.is_null());

        // LTM enabled on an RK4 model: the compile failure is deferred to run
        // time (the established `simlin_sim_new` contract returns a non-null
        // handle carrying the compile error and surfaces it on run), and the
        // surfaced error references the Euler assumption.
        err = ptr::null_mut();
        let sim_ltm = simlin_sim_new(model, true, &mut err as *mut *mut SimlinError);
        assert!(
            err.is_null(),
            "simlin_sim_new defers the compile error to run"
        );
        assert!(!sim_ltm.is_null());
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim_ltm, &mut err as *mut *mut SimlinError);
        assert!(
            !err.is_null(),
            "running an LTM + RK4 sim must surface an error"
        );
        let msg_ptr = simlin_error_get_message(err);
        assert!(!msg_ptr.is_null(), "the error must carry a message");
        let msg = CStr::from_ptr(msg_ptr).to_str().unwrap();
        assert!(
            msg.contains("Euler"),
            "the error must reference the Euler assumption, got: {msg}"
        );
        simlin_error_free(err);
        simlin_sim_unref(sim_ltm);

        // The same model without LTM compiles and runs as before.
        err = ptr::null_mut();
        let sim_no_ltm = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "RK4 without LTM must compile");
        assert!(!sim_no_ltm.is_null());
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim_no_ltm, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "RK4 without LTM must simulate");

        simlin_sim_unref(sim_no_ltm);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_mark2_mdl_simulates_through_ffi() {
    let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
    let data = std::fs::read(mdl_path).unwrap_or_else(|e| panic!("read {mdl_path}: {e}"));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_vensim(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "open_vensim");
        assert!(!proj.is_null());

        err = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!model.is_null());

        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "sim_new");
        assert!(!sim.is_null());

        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "run_to_end");

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Regression test: systems format models use stdlib modules (systems_rate,
/// systems_conversion, etc.) whose internal variables must be accessible
/// via getSeries after simulation. The flattened offsets must use canonical
/// middle-dot separators (e.g. "module·var") so that Ident::new() lookups
/// match the stored keys.
#[test]
fn test_systems_format_module_var_get_series() {
    let hiring_txt =
        std::fs::read_to_string("testdata/hiring.txt").expect("hiring.txt fixture must exist");
    let datamodel = engine::open_systems(&hiring_txt).unwrap();

    // Round-trip through JSON (same path as the web UI: server stores
    // sd.json, browser opens it via Project.openJson)
    let json_project: engine::json::Project = (&datamodel).into();
    let json_bytes = serde_json::to_vec(&json_project).unwrap();
    let reopened: engine::json::Project = serde_json::from_slice(&json_bytes).unwrap();
    let reopened_dm: engine::datamodel::Project = reopened.into();

    let proj = open_project_from_datamodel(&reopened_dm);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut err);
        assert!(err.is_null(), "sim creation must succeed");
        assert!(!sim.is_null());

        simlin_sim_run_to_end(sim, &mut err);
        assert!(err.is_null(), "run_to_end must succeed");

        // Get all var names from the simulation
        let mut count: usize = 0;
        simlin_sim_get_var_count(sim, &mut count, &mut err);
        assert!(err.is_null());
        assert!(count > 0);

        let mut name_ptrs: Vec<*mut c_char> = vec![ptr::null_mut(); count];
        let mut written: usize = 0;
        simlin_sim_get_var_names(sim, name_ptrs.as_mut_ptr(), count, &mut written, &mut err);
        assert!(err.is_null());

        let mut names: Vec<String> = Vec::with_capacity(written);
        for &p in &name_ptrs[..written] {
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            names.push(s);
            simlin_free_string(p);
        }

        // Every name returned by getVarNames must be retrievable via getSeries.
        // This was the original bug: module sub-variable names like
        // "candidates_outflows.actual" were stored with "." in offsets but
        // looked up with "\u{00B7}" (middle dot) after canonicalization, causing
        // "series not found" errors.
        let step_count = 20; // generous buffer
        let mut buf: Vec<c_double> = vec![0.0; step_count];
        for name in &names {
            let c_name = CString::new(name.as_str()).unwrap();
            let mut out_written: usize = 0;
            simlin_sim_get_series(
                sim,
                c_name.as_ptr(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut out_written,
                &mut err,
            );
            assert!(
                err.is_null(),
                "getSeries({name:?}) must succeed (was the canonical offset bug)"
            );
            assert!(out_written > 0, "getSeries({name:?}) must return data");
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// End-to-end FFI proof that a conveyor model simulates through the production
/// `simlin_sim_new` path (which routes conveyors through the conveyor build
/// path). The minimal fixture is at steady state (init 1000 == inflow 250 *
/// transit 4), so Students holds flat at 1000 and graduating is a constant 250.
#[test]
fn test_conveyor_model_simulates_via_ffi() {
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile");
    unsafe {
        let (proj, model, sim) = create_test_sim(&datamodel);
        run_to_end(sim);
        let students = get_series_vec(sim, "students", 4096);
        assert!(
            students.len() > 40,
            "expected many steps, got {}",
            students.len()
        );
        for (i, &s) in students.iter().enumerate() {
            assert!(
                (s - 1000.0).abs() < 1e-6,
                "step {i}: Students={s} (want 1000)"
            );
        }
        let graduating = get_series_vec(sim, "graduating", 4096);
        for (i, &g) in graduating.iter().enumerate().skip(1) {
            assert!(
                (g - 250.0).abs() < 1e-6,
                "step {i}: graduating={g} (want 250)"
            );
        }
        // reset + rerun must reproduce identical steady state (belts rebuilt).
        reset_sim(sim);
        run_to_end(sim);
        let students2 = get_series_vec(sim, "students", 4096);
        assert_eq!(students, students2, "reset+rerun diverged");
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// End-to-end FFI proof that a queue model simulates through the production
/// `simlin_sim_new` path (which routes queues through the unified special-stock
/// build path). The fixture is a scalar queue -> stock with a constant inflow and
/// an unconstrained outflow, so the queue is a faithful pass-through: `waiting`
/// holds ~0 and `into_service` equals the constant inflow (10) every step.
#[test]
fn test_queue_model_simulates_via_ffi() {
    let xml = include_str!("../../../../test/queues/queue_drain.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse queue_drain.xmile");
    unsafe {
        let (proj, model, sim) = create_test_sim(&datamodel);
        run_to_end(sim);
        let waiting = get_series_vec(sim, "waiting", 4096);
        assert!(
            waiting.len() > 10,
            "expected many steps, got {}",
            waiting.len()
        );
        for (i, &w) in waiting.iter().enumerate() {
            assert!(w.abs() < 1e-9, "step {i}: waiting={w} (want ~0)");
        }
        let into_service = get_series_vec(sim, "into_service", 4096);
        for (i, &o) in into_service.iter().enumerate() {
            assert!(
                (o - 10.0).abs() < 1e-9,
                "step {i}: into_service={o} (want 10)"
            );
        }
        // reset + rerun must reproduce the run (queue side table re-seeded).
        reset_sim(sim);
        run_to_end(sim);
        let waiting2 = get_series_vec(sim, "waiting", 4096);
        assert_eq!(waiting, waiting2, "reset+rerun diverged");
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Mid-run inspection of pass-driven flows via the FFI: after a partial
/// `simlin_sim_run_to`, `simlin_sim_get_value` reads the live VM's resting
/// curr chunk. That chunk's #625 Flows-only re-eval used to re-execute each
/// pass-driven flow's placeholder `AssignConstCurr 0`, so a conveyor primary
/// outflow or queue outflow read 0 mid-run even though the saved series held
/// the pass-computed rate. Both fixtures are at steady state, so the expected
/// resting values are unambiguous: graduating == 250, into_service == 10.
#[test]
fn test_mid_run_get_value_reads_pass_driven_rates() {
    // Conveyor: minimal_conveyor is at steady state (init 1000 == 250 * 4).
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile");
    unsafe {
        let (proj, model, sim) = create_test_sim(&datamodel);
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_run_to(sim, 6.0, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "run_to(6.0) failed");
        assert_sim_value(sim, "graduating", 250.0, 1e-6);
        assert_sim_value(sim, "students", 1000.0, 1e-6);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }

    // Queue: queue_drain is a faithful pass-through (into_service == 10).
    let xml = include_str!("../../../../test/queues/queue_drain.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse queue_drain.xmile");
    unsafe {
        let (proj, model, sim) = create_test_sim(&datamodel);
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_run_to(sim, 2.0, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "run_to(2.0) failed");
        assert_sim_value(sim, "into_service", 10.0, 1e-9);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// GH #871: `simlin_sim_set_value` on a pass-driven conveyor flow must be
/// rejected with `BadOverride` on BOTH validation paths -- the live-VM path
/// (`Vm::set_value`) and the post-`run_to_end` path (which validates against
/// the cached `CompiledSimulation`) -- instead of being silently accepted and
/// then overwritten by the conveyor pass every step. Because both paths
/// reject, no override is ever recorded in `SimState.overrides`, so a
/// subsequent reset cannot smuggle a stale one back in.
#[test]
fn test_set_value_on_conveyor_driven_flow_rejected() {
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile");
    unsafe {
        let (proj, model, sim) = create_test_sim(&datamodel);
        let c_name = CString::new("graduating").unwrap();

        // Live-VM path.
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_name.as_ptr(),
            999.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "override of a belt-driven flow must be rejected"
        );
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadOverride);
        simlin_error_free(err);

        // The rejected override leaves no trace: the run is belt-driven (the
        // fixture is at steady state, graduating == 250 every step).
        run_to_end(sim);
        let graduating = get_series_vec(sim, "graduating", 4096);
        for (i, &g) in graduating.iter().enumerate().skip(1) {
            assert!(
                (g - 250.0).abs() < 1e-6,
                "step {i}: graduating={g} (want 250)"
            );
        }

        // No-VM path: run_to_end consumed the VM, so set_value now validates
        // against the cached CompiledSimulation -- it must reject identically.
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_name.as_ptr(),
            999.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "post-run override of a belt-driven flow must be rejected"
        );
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadOverride);
        simlin_error_free(err);

        // Reset recreates the VM and re-applies recorded overrides; none were
        // recorded, so it succeeds and a re-run reproduces the belt-driven run.
        reset_sim(sim);
        run_to_end(sim);
        let graduating2 = get_series_vec(sim, "graduating", 4096);
        assert_eq!(graduating, graduating2, "reset+rerun diverged");

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// GH #871 (queue side): `simlin_sim_set_value` on a queue-driven outflow must
/// be rejected with `BadOverride`, and the run stays queue-driven (the fixture
/// is a pass-through queue, `into_service` == the constant inflow 10).
#[test]
fn test_set_value_on_queue_driven_outflow_rejected() {
    let xml = include_str!("../../../../test/queues/queue_drain.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse queue_drain.xmile");
    unsafe {
        let (proj, model, sim) = create_test_sim(&datamodel);
        let c_name = CString::new("into_service").unwrap();

        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_name.as_ptr(),
            999.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "override of a queue-driven outflow must be rejected"
        );
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadOverride);
        simlin_error_free(err);

        run_to_end(sim);
        let into_service = get_series_vec(sim, "into_service", 4096);
        for (i, &o) in into_service.iter().enumerate() {
            assert!(
                (o - 10.0).abs() < 1e-9,
                "step {i}: into_service={o} (want 10)"
            );
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// End-to-end FFI proof (F12) that a stock marked as BOTH a conveyor and a queue
/// is rejected -- loudly, naming the stock -- through the production
/// `simlin_sim_new` path, rather than silently building both a conveyor and a
/// queue plan over the same stock+outflow and mis-simulating. This exercises the
/// full round-trip: the XMILE reader preserves both markers, protobuf carries them
/// side by side, and the unified special-stock build path rejects the conflict up
/// front. `simlin_sim_new` defers a compile error into the sim's `vm_error` (it
/// returns a non-null sim), so the rejection surfaces on the first run. The wire
/// error code collapses to `Generic` (the wire enum does not track the engine's
/// growing conveyor/queue tail), so we assert on the message naming the stock.
#[test]
fn test_stock_with_both_conveyor_and_queue_rejected_via_ffi() {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>both markers</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="belt">
      <eqn>10</eqn>
      <inflow>into_belt</inflow>
      <outflow>out</outflow>
      <conveyor><len>4</len></conveyor>
      <queue/>
    </stock>
    <flow name="into_belt"><eqn>5</eqn><non_negative/></flow>
    <flow name="out"><eqn>0</eqn></flow>
    <stock name="done"><eqn>0</eqn><inflow>out</inflow></stock>
  </variables></model>
</xmile>"#;
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse both-markers xmile");
    unsafe {
        let proj = open_project_from_datamodel(&datamodel);
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "get_model failed");
        assert!(!model.is_null());

        // sim_new defers the compile error; it returns a non-null sim.
        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(!sim.is_null(), "sim_new returns a sim (error is deferred)");

        // The rejection surfaces on the first run, naming the offending stock.
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
        assert!(
            !err.is_null(),
            "running a both-marked-stock model must surface the rejection"
        );
        let msg_ptr = simlin_error_get_message(err);
        let msg = if !msg_ptr.is_null() {
            CStr::from_ptr(msg_ptr).to_str().unwrap_or("")
        } else {
            ""
        };
        assert!(
            msg.contains("belt"),
            "error message names the offending stock: {msg}"
        );
        simlin_error_free(err);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
