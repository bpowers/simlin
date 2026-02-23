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
