// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Acceptance-criteria tests for incremental compilation (AC3.x).

/// AC3.1: apply_patch followed by sim_new triggers only one compilation
/// pass (not two), because the salsa DB is shared and sim_new's
/// compilation is a salsa cache hit from the patch application.
///
/// We verify this by checking that apply_patch + sim_new succeeds (the
/// shared DB path works), and that the sim produces correct results
/// consistent with the post-patch state.
#[test]
fn test_ac3_1_one_compilation_patch_then_sim() {
    let datamodel = TestProject::new("ac3_1")
        .aux("rate", "0.1", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    // Apply a patch that changes rate's equation
    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "rate",
                                "equation": "0.5"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected,
            &mut out_error,
        );
        assert!(out_error.is_null(), "patch should succeed");
        if !collected.is_null() {
            simlin_error_free(collected);
        }

        // Create sim from model -- should use salsa cache from patch
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null(), "sim_new should succeed after patch");
        assert!(out_error.is_null());

        // Run the simulation to verify it works
        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null(), "simulation should run successfully");

        // Verify the patched value is reflected: "rate" should be 0.5
        let name = std::ffi::CString::new("rate").unwrap();
        let mut value: f64 = 0.0;
        simlin_sim_get_value(sim, name.as_ptr(), &mut value, &mut out_error);
        assert!(out_error.is_null());
        assert!(
            (value - 0.5).abs() < 1e-10,
            "rate should be 0.5 after patch, got {}",
            value
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// AC3.3: Sequential patches each trigger only incremental recomputation.
/// The second patch should only recompute affected portions, and both
/// patches should produce correct simulation results.
#[test]
fn test_ac3_3_sequential_patches() {
    let datamodel = TestProject::new("ac3_3")
        .aux("alpha", "10", None)
        .aux("beta", "20", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let mut collected: *mut SimlinError = ptr::null_mut();

        // Patch 1: change alpha to 42
        let patch1 = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "alpha", "equation": "42" } }
                }]
            }]
        }"#;
        let p1 = patch1.as_bytes();
        simlin_project_apply_patch(
            proj,
            p1.as_ptr(),
            p1.len(),
            false,
            true,
            &mut collected,
            &mut out_error,
        );
        assert!(out_error.is_null(), "patch 1 should succeed");
        if !collected.is_null() {
            simlin_error_free(collected);
            collected = ptr::null_mut();
        }

        // Patch 2: change beta to 99
        let patch2 = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "beta", "equation": "99" } }
                }]
            }]
        }"#;
        let p2 = patch2.as_bytes();
        simlin_project_apply_patch(
            proj,
            p2.as_ptr(),
            p2.len(),
            false,
            true,
            &mut collected,
            &mut out_error,
        );
        assert!(out_error.is_null(), "patch 2 should succeed");
        if !collected.is_null() {
            simlin_error_free(collected);
        }

        // Create a simulation after both patches
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null(), "sim should succeed after two patches");
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        // Verify both patches took effect
        let alpha_name = std::ffi::CString::new("alpha").unwrap();
        let mut alpha_val: f64 = 0.0;
        simlin_sim_get_value(sim, alpha_name.as_ptr(), &mut alpha_val, &mut out_error);
        assert!(out_error.is_null());
        assert!(
            (alpha_val - 42.0).abs() < 1e-10,
            "alpha should be 42, got {}",
            alpha_val
        );

        let beta_name = std::ffi::CString::new("beta").unwrap();
        let mut beta_val: f64 = 0.0;
        simlin_sim_get_value(sim, beta_name.as_ptr(), &mut beta_val, &mut out_error);
        assert!(out_error.is_null());
        assert!(
            (beta_val - 99.0).abs() < 1e-10,
            "beta should be 99, got {}",
            beta_val
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Dry-run patches must not affect the db state visible to subsequent
/// simlin_sim_new calls. After a dry_run patch that changes alpha from
/// 100 to 999, a simulation created afterwards must still see 100.
#[test]
fn test_dry_run_does_not_leak_staged_state() {
    let datamodel = TestProject::new("dry_run_leak")
        .aux("alpha", "100", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let mut collected: *mut SimlinError = ptr::null_mut();

        // Apply a dry_run patch that changes alpha to 999
        let patch = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "alpha", "equation": "999" } }
                }]
            }]
        }"#;
        let p = patch.as_bytes();
        simlin_project_apply_patch(
            proj,
            p.as_ptr(),
            p.len(),
            true, // dry_run = true
            true,
            &mut collected,
            &mut out_error,
        );
        assert!(out_error.is_null(), "dry_run patch should succeed");
        if !collected.is_null() {
            simlin_error_free(collected);
        }

        // sim_new after the dry_run must see the ORIGINAL value (100)
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null(), "sim_new should succeed after dry_run patch");
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        let name = std::ffi::CString::new("alpha").unwrap();
        let mut value: f64 = 0.0;
        simlin_sim_get_value(sim, name.as_ptr(), &mut value, &mut out_error);
        assert!(out_error.is_null());
        assert!(
            (value - 100.0).abs() < 1e-10,
            "dry_run should not change alpha; expected 100, got {}",
            value
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Rejected patches (due to errors, when allow_errors=false) must not
/// leak staged state to the db. After a rejected patch that introduces
/// an unknown dependency, sim_new must still see the original value.
#[test]
fn test_rejected_patch_does_not_leak_staged_state() {
    let datamodel = TestProject::new("rejected_leak")
        .aux("alpha", "100", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let mut collected: *mut SimlinError = ptr::null_mut();

        // Apply a patch that introduces an error (unknown_var dependency).
        // With allow_errors=false, this should be rejected.
        let patch = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "alpha", "equation": "unknown_var + 1" } }
                }]
            }]
        }"#;
        let p = patch.as_bytes();
        simlin_project_apply_patch(
            proj,
            p.as_ptr(),
            p.len(),
            false,
            false, // allow_errors = false -- patch should be rejected
            &mut collected,
            &mut out_error,
        );
        // The patch should be rejected (out_error set)
        assert!(
            !out_error.is_null(),
            "patch with unknown dependency should be rejected when allow_errors=false"
        );
        simlin_error_free(out_error);
        out_error = ptr::null_mut();
        if !collected.is_null() {
            simlin_error_free(collected);
        }

        // sim_new after the rejected patch must see the ORIGINAL value (100)
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null(), "sim_new should succeed after rejected patch");
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        let name = std::ffi::CString::new("alpha").unwrap();
        let mut value: f64 = 0.0;
        simlin_sim_get_value(sim, name.as_ptr(), &mut value, &mut out_error);
        assert!(out_error.is_null());
        assert!(
            (value - 100.0).abs() < 1e-10,
            "rejected patch should not change alpha; expected 100, got {}",
            value
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Concurrent stress test: one thread repeatedly applies dry_run patches
/// while other threads create simulations. No thread should ever observe
/// the staged (dry_run) value.
#[test]
fn test_concurrent_dry_run_never_leaks_to_sim_new() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    let datamodel = TestProject::new("concurrent_dry_run")
        .aux("alpha", "100", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let stop = Arc::new(AtomicBool::new(false));
    let leaked = Arc::new(AtomicBool::new(false));

    // Reader threads: repeatedly create sims and check the value
    let mut handles = vec![];
    for _ in 0..4 {
        let stop = Arc::clone(&stop);
        let leaked = Arc::clone(&leaked);
        let proj_addr = proj as usize;
        let handle = thread::spawn(move || unsafe {
            let proj_ptr = proj_addr as *mut SimlinProject;
            while !stop.load(Ordering::Relaxed) {
                let mut err: *mut SimlinError = ptr::null_mut();
                let model = simlin_project_get_model(proj_ptr, ptr::null(), &mut err);
                if model.is_null() || !err.is_null() {
                    if !err.is_null() {
                        simlin_error_free(err);
                    }
                    continue;
                }

                let sim = simlin_sim_new(model, false, &mut err);
                if sim.is_null() || !err.is_null() {
                    if !err.is_null() {
                        simlin_error_free(err);
                    }
                    simlin_model_unref(model);
                    continue;
                }

                simlin_sim_run_to_end(sim, &mut err);
                if !err.is_null() {
                    simlin_error_free(err);
                    simlin_sim_unref(sim);
                    simlin_model_unref(model);
                    continue;
                }

                let name = std::ffi::CString::new("alpha").unwrap();
                let mut value: f64 = 0.0;
                simlin_sim_get_value(sim, name.as_ptr(), &mut value, &mut err);
                if err.is_null() && (value - 100.0).abs() > 1e-10 {
                    leaked.store(true, Ordering::SeqCst);
                }
                if !err.is_null() {
                    simlin_error_free(err);
                }

                simlin_sim_unref(sim);
                simlin_model_unref(model);
            }
        });
        handles.push(handle);
    }

    // Writer thread: repeatedly apply dry_run patches
    let proj_addr = proj as usize;
    let stop_writer = Arc::clone(&stop);
    let writer = thread::spawn(move || unsafe {
        let proj_ptr = proj_addr as *mut SimlinProject;
        let patch = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "alpha", "equation": "999" } }
                }]
            }]
        }"#;
        let p = patch.as_bytes();

        for _ in 0..50 {
            if stop_writer.load(Ordering::Relaxed) {
                break;
            }
            let mut err: *mut SimlinError = ptr::null_mut();
            let mut collected: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch(
                proj_ptr,
                p.as_ptr(),
                p.len(),
                true, // dry_run
                true,
                &mut collected,
                &mut err,
            );
            if !err.is_null() {
                simlin_error_free(err);
            }
            if !collected.is_null() {
                simlin_error_free(collected);
            }
        }
    });

    writer.join().unwrap();
    stop.store(true, Ordering::SeqCst);
    for handle in handles {
        handle.join().unwrap();
    }

    assert!(
        !leaked.load(Ordering::SeqCst),
        "a reader thread observed the staged dry_run value (999) instead of the committed value (100)"
    );

    unsafe {
        simlin_project_unref(proj);
    }
}

/// AC3.4: Running simulations are isolated from subsequent patches.
/// A sim created before a patch should produce results from the
/// pre-patch model state (snapshot isolation), because sim_new clones
/// the compiled simulation at creation time.
#[test]
fn test_ac3_4_snapshot_isolation() {
    let datamodel = TestProject::new("ac3_4")
        .aux("constant", "100", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let mut collected: *mut SimlinError = ptr::null_mut();

        // Create a simulation BEFORE the patch
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null(), "pre-patch sim should succeed");
        assert!(out_error.is_null());

        // Now apply a patch that changes the constant
        let patch = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "constant", "equation": "999" } }
                }]
            }]
        }"#;
        let p = patch.as_bytes();
        simlin_project_apply_patch(
            proj,
            p.as_ptr(),
            p.len(),
            false,
            true,
            &mut collected,
            &mut out_error,
        );
        assert!(out_error.is_null(), "patch should succeed");
        if !collected.is_null() {
            simlin_error_free(collected);
        }

        // Run the pre-patch simulation -- it should use the OLD value (100)
        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        let name = std::ffi::CString::new("constant").unwrap();
        let mut value: f64 = 0.0;
        simlin_sim_get_value(sim, name.as_ptr(), &mut value, &mut out_error);
        assert!(out_error.is_null());
        assert!(
            (value - 100.0).abs() < 1e-10,
            "pre-patch sim should use old value 100, got {}",
            value
        );

        // Create a NEW simulation -- this one should see the patched value (999)
        let sim2 = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim2.is_null(), "post-patch sim should succeed");
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim2, &mut out_error);
        assert!(out_error.is_null());

        let mut value2: f64 = 0.0;
        simlin_sim_get_value(sim2, name.as_ptr(), &mut value2, &mut out_error);
        assert!(out_error.is_null());
        assert!(
            (value2 - 999.0).abs() < 1e-10,
            "post-patch sim should use new value 999, got {}",
            value2
        );

        simlin_sim_unref(sim);
        simlin_sim_unref(sim2);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// When incremental compilation fails (e.g., model references an undefined
/// variable), the actual error must be preserved and surfaced when the
/// caller tries to run the simulation.  Previously the error was dropped,
/// resulting in a generic "not initialised" message.
#[test]
fn test_incremental_compile_error_preserved_in_sim() {
    use engine::datamodel;

    let project = datamodel::Project {
        name: "bad_model".to_string(),
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
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("undefined_var + 1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        source: None,
        ai_information: None,
    };

    let proj = open_project_from_datamodel(&project);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(!model.is_null());

        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err);
        // Sim creation returns a handle even on compile failure
        assert!(!sim.is_null());

        // Attempting to run should surface the compile error, not a generic message
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err);
        assert!(
            !err.is_null(),
            "run_to_end should fail for a model with undefined reference"
        );

        let msg_ptr = simlin_error_get_message(err);
        let msg = if !msg_ptr.is_null() {
            CStr::from_ptr(msg_ptr).to_str().unwrap_or("")
        } else {
            ""
        };
        // The error should NOT be the generic "not initialised" message
        assert!(
            !msg.contains("not been initialised"),
            "error should be the actual compile failure, not generic 'not initialised': {msg}"
        );
        simlin_error_free(err);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
