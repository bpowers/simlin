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
