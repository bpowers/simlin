// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Tests for patch application using XMILE models.
// These test the full FFI patch pipeline including error collection.

#[test]
fn test_apply_patch_xmile_empty_equation_allow_errors() {
    // Reproduces the WASM test scenario: open a XMILE model and apply a patch
    // with an empty equation variable, with allow_errors = true.
    let teacup_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../src/pysimlin/tests/fixtures/teacup.stmx");
    if !teacup_path.exists() {
        eprintln!("missing teacup.stmx fixture; skipping");
        return;
    }
    let data = std::fs::read(&teacup_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_xmile(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        if !err.is_null() {
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if msg_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(err);
            panic!("project_open_xmile failed: {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        // Apply a patch adding an auxiliary with an empty equation, allow_errors=true
        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsertAux",
                            "payload": { "aux": { "name": "new_var", "equation": "" } }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true, // allow_errors = true
            &mut collected_errors as *mut *mut SimlinError,
            &mut out_error as *mut *mut SimlinError,
        );

        // Should succeed because allow_errors is true
        if !out_error.is_null() {
            let code = simlin_error_get_code(out_error);
            let msg_ptr = simlin_error_get_message(out_error);
            let msg = if msg_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(out_error);
            panic!(
                "expected no error when allow_errors=true, got {:?}: {}",
                code, msg
            );
        }

        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        simlin_project_unref(proj);
    }
}

#[test]
fn test_apply_patch_xmile_empty_equation_reject() {
    // Reproduces the WASM test scenario: open a XMILE model and apply a patch
    // with an empty equation variable, with allow_errors = false (should reject).
    let teacup_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../src/pysimlin/tests/fixtures/teacup.stmx");
    if !teacup_path.exists() {
        eprintln!("missing teacup.stmx fixture; skipping");
        return;
    }
    let data = std::fs::read(&teacup_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_xmile(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "project_open_xmile failed");
        assert!(!proj.is_null());

        // Apply a patch adding an auxiliary with an empty equation, allow_errors=false
        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsertAux",
                            "payload": { "aux": { "name": "bad_var", "equation": "" } }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            false, // allow_errors = false
            &mut collected_errors as *mut *mut SimlinError,
            &mut out_error as *mut *mut SimlinError,
        );

        // Should reject because allow_errors=false and there are errors
        assert!(
            !out_error.is_null(),
            "expected error when allow_errors=false with empty equation"
        );

        // Verify the error has a message
        let msg_ptr = simlin_error_get_message(out_error);
        assert!(!msg_ptr.is_null(), "error should have a message");
        let msg = CStr::from_ptr(msg_ptr).to_str().unwrap();
        assert!(!msg.is_empty(), "error message should not be empty");

        // Verify the error has details
        let detail_count = simlin_error_get_detail_count(out_error);
        assert!(
            detail_count > 0,
            "error should have details, got count={}",
            detail_count
        );

        simlin_error_free(out_error);
        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        simlin_project_unref(proj);
    }
}

/// Regression test: simlin_sim_new with enable_ltm=true must not leave
/// the SourceProject's ltm_enabled flag set, otherwise subsequent patch
/// validation compiles in stale LTM mode.
#[test]
fn test_ltm_sim_then_patch_does_not_inherit_ltm_mode() {
    let datamodel = TestProject::new("ltm_sim_patch_ordering")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * 0.01", None)
        .aux("birth_rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();

        // First, create an LTM simulation (sets ltm_enabled=true internally)
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, true, &mut out_error);
        assert!(!sim.is_null(), "LTM simulation creation should succeed");
        assert!(out_error.is_null());
        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());
        simlin_sim_unref(sim);
        simlin_model_unref(model);

        // Now apply a patch. If ltm_enabled leaked from simlin_sim_new,
        // the patch validation compilation would run in LTM mode, which
        // could cause spurious failures or different error behavior.
        let patch_json = r#"{
            "models": [{ "name": "main", "ops": [
                { "type": "upsertAux", "payload": { "aux": { "name": "birth_rate", "equation": "0.03" } } }
            ]}]
        }"#;
        let patch_bytes = patch_json.as_bytes();
        let mut collected: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected,
            &mut out_error,
        );
        assert!(
            out_error.is_null(),
            "patch after LTM sim should not fail due to stale LTM mode"
        );
        if !collected.is_null() {
            simlin_error_free(collected);
        }

        // Verify normal simulation still works after the patch
        let model2 = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model2.is_null());
        let sim2 = simlin_sim_new(model2, false, &mut out_error);
        assert!(!sim2.is_null(), "non-LTM simulation should work after patch");
        assert!(out_error.is_null());
        simlin_sim_run_to_end(sim2, &mut out_error);
        assert!(out_error.is_null());
        simlin_sim_unref(sim2);
        simlin_model_unref(model2);

        simlin_project_unref(proj);
    }
}
