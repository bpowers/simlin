// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// ── concurrency regression tests ────────────────────────────────────────

/// Regression test for issue #303: concurrent add_model + sim_new should never
/// produce a spurious NotSimulatable error caused by sync_state being temporarily
/// None between .take() and the restore.
#[test]
fn test_concurrent_add_model_and_sim_new_no_spurious_not_simulatable() {
    use std::ffi::CString;
    use std::sync::Arc;
    use std::thread;

    let datamodel = TestProject::new("concurrent_add_model")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let thread_count = 20;
    unsafe {
        for _ in 0..thread_count {
            simlin_project_ref(proj);
        }
    }

    let proj_addr = proj as usize;
    let barrier = Arc::new(std::sync::Barrier::new(thread_count));

    let mut handles = Vec::new();

    // 10 threads doing add_model
    for i in 0..10 {
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            unsafe {
                let proj = proj_addr as *mut SimlinProject;
                let model_name = CString::new(format!("extra_model_{i}")).unwrap();
                let mut out_error: *mut SimlinError = std::ptr::null_mut();
                simlin_project_add_model(proj, model_name.as_ptr(), &mut out_error);
                if !out_error.is_null() {
                    simlin_error_free(out_error);
                }
                simlin_project_unref(proj);
            }
        }));
    }

    // 10 threads doing sim_new on the "main" model
    for _ in 0..10 {
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            unsafe {
                let proj = proj_addr as *mut SimlinProject;
                let mut out_error: *mut SimlinError = std::ptr::null_mut();
                let model = simlin_project_get_model(proj, std::ptr::null(), &mut out_error);
                if model.is_null() {
                    if !out_error.is_null() {
                        simlin_error_free(out_error);
                    }
                    simlin_project_unref(proj);
                    return;
                }

                let mut sim_error: *mut SimlinError = std::ptr::null_mut();
                let sim = simlin_sim_new(model, false, &mut sim_error);

                if !sim_error.is_null() {
                    let code = simlin_error_get_code(sim_error);
                    assert_ne!(
                        code,
                        SimlinErrorCode::NotSimulatable,
                        "sim_new should never fail with NotSimulatable due to missing sync_state"
                    );
                    simlin_error_free(sim_error);
                }

                if !sim.is_null() {
                    simlin_sim_unref(sim);
                }
                simlin_model_unref(model);
                simlin_project_unref(proj);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    unsafe {
        simlin_project_unref(proj);
    }
}
