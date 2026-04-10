// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod common;

use std::ffi::{CStr, CString};
use std::ptr;

use simlin::*;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use common::open_project_from_datamodel;

#[test]
fn test_project_apply_patch_commits() {
    let datamodel = TestProject::new("json_patch").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "json_aux",
                                "equation": "7"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors as *mut *mut SimlinError,
            &mut out_error as *mut *mut SimlinError,
        );

        assert!(out_error.is_null(), "expected no error applying json patch");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(model.get_variable("json_aux").is_some());
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_invalid() {
    let datamodel = TestProject::new("json_patch").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let patch_bytes = b"{invalid";

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors as *mut *mut SimlinError,
            &mut out_error as *mut *mut SimlinError,
        );

        assert!(collected_errors.is_null());
        assert!(!out_error.is_null(), "expected error for invalid json");
        assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
        simlin_error_free(out_error);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_upsert_stock() {
    let datamodel = TestProject::new("json_patch_stock").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertStock",
                        "payload": {
                            "stock": {
                                "name": "inventory",
                                "initialEquation": "50",
                                "inflows": [],
                                "outflows": []
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error upserting stock");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        let stock = model.get_variable("inventory");
        assert!(stock.is_some(), "stock should exist after upsert");
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_upsert_flow() {
    let datamodel = TestProject::new("json_patch_flow").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertFlow",
                        "payload": {
                            "flow": {
                                "name": "production",
                                "equation": "10",
                                "nonNegative": true
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error upserting flow");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        let flow = model.get_variable("production");
        assert!(flow.is_some(), "flow should exist after upsert");
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_upsert_module() {
    // Create a project with a submodel that the module can reference
    let mut datamodel = TestProject::new("json_patch_module").build_datamodel();
    // Add a submodel to the project
    let submodel = engine::datamodel::Model {
        name: "SubModel".to_string(),
        sim_specs: None,
        variables: vec![],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
    };
    datamodel.models.push(submodel);

    let proj = open_project_from_datamodel(&datamodel);

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertModule",
                        "payload": {
                            "module": {
                                "name": "submodel",
                                "modelName": "SubModel",
                                "references": []
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error upserting module");
        // Modules referencing other models may have compilation errors, which is ok
        // when allow_errors=true

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        let module = model.get_variable("submodel");
        assert!(module.is_some(), "module should exist after upsert");
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_delete_variable() {
    let datamodel = TestProject::new("json_patch_delete")
        .aux("to_delete", "42", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("to_delete").is_some(),
            "variable should exist before delete"
        );
    }

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "deleteVariable",
                        "payload": {
                            "ident": "to_delete"
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error deleting variable");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("to_delete").is_none(),
            "variable should not exist after delete"
        );
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_rename_variable() {
    let datamodel = TestProject::new("json_patch_rename")
        .aux("old_name", "123", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("old_name").is_some(),
            "old variable should exist before rename"
        );
    }

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "renameVariable",
                        "payload": {
                            "from": "old_name",
                            "to": "new_name"
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error renaming variable");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("old_name").is_none(),
            "old variable should not exist after rename"
        );
        assert!(
            model.get_variable("new_name").is_some(),
            "new variable should exist after rename"
        );
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_upsert_view() {
    let datamodel = TestProject::new("json_patch_view").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertView",
                        "payload": {
                            "index": 0,
                            "view": {
                                "kind": "stock_flow",
                                "elements": [],
                                "viewBox": {
                                    "x": 0,
                                    "y": 0,
                                    "width": 800,
                                    "height": 600
                                },
                                "zoom": 1.0
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error upserting view");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(!model.views.is_empty(), "view should exist after upsert");
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_delete_view() {
    let datamodel = TestProject::new("json_patch_delete_view").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    // First upsert a view
    let upsert_patch = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertView",
                        "payload": {
                            "index": 0,
                            "view": {
                                "kind": "stock_flow",
                                "elements": [],
                                "viewBox": {"x": 0, "y": 0, "width": 800, "height": 600},
                                "zoom": 1.0
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let upsert_bytes = upsert_patch.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            upsert_bytes.as_ptr(),
            upsert_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );
        assert!(out_error.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(!model.views.is_empty(), "view should exist after upsert");
    }

    // Now delete the view
    let delete_patch = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "deleteView",
                        "payload": {
                            "index": 0
                        }
                    }
                ]
            }
        ]
    }"#;
    let delete_bytes = delete_patch.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            delete_bytes.as_ptr(),
            delete_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error deleting view");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(model.views.is_empty(), "view should not exist after delete");
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_set_sim_specs() {
    let datamodel = TestProject::new("json_patch_sim_specs").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let original_start = unsafe { (*proj).datamodel.lock().unwrap().sim_specs.start };
    let original_stop = unsafe { (*proj).datamodel.lock().unwrap().sim_specs.stop };

    let patch_json = r#"{
        "projectOps": [
            {
                "type": "setSimSpecs",
                "payload": {
                    "simSpecs": {
                        "startTime": 2020.0,
                        "endTime": 2030.0,
                        "dt": "1",
                        "saveStep": 1.0,
                        "method": "euler",
                        "timeUnits": "years"
                    }
                }
            }
        ],
        "models": []
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        if !out_error.is_null() {
            let err_code = simlin_error_get_code(out_error);
            let err_msg = simlin_error_get_message(out_error);
            let msg_str = if !err_msg.is_null() {
                std::ffi::CStr::from_ptr(err_msg)
                    .to_string_lossy()
                    .into_owned()
            } else {
                "no message".to_string()
            };
            simlin_error_free(out_error);
            panic!("error setting sim specs: {:?} - {}", err_code, msg_str);
        }
        assert!(collected_errors.is_null());

        let new_start = (*proj).datamodel.lock().unwrap().sim_specs.start;
        let new_stop = (*proj).datamodel.lock().unwrap().sim_specs.stop;

        assert_ne!(
            original_start, new_start,
            "start time should have been updated"
        );
        assert_ne!(
            original_stop, new_stop,
            "stop time should have been updated"
        );
        assert_eq!(new_start, 2020.0);
        assert_eq!(new_stop, 2030.0);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_project_and_model_ops() {
    let datamodel = TestProject::new("json_patch_combined").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let patch_json = r#"{
        "projectOps": [
            {
                "type": "setSimSpecs",
                "payload": {
                    "simSpecs": {
                        "startTime": 0.0,
                        "endTime": 100.0,
                        "dt": "0.5",
                        "saveStep": 0.5,
                        "method": "euler",
                        "timeUnits": "months"
                    }
                }
            }
        ],
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "combined_test",
                                "equation": "42"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        if !out_error.is_null() {
            let err_code = simlin_error_get_code(out_error);
            let err_msg = simlin_error_get_message(out_error);
            let msg_str = if !err_msg.is_null() {
                std::ffi::CStr::from_ptr(err_msg)
                    .to_string_lossy()
                    .into_owned()
            } else {
                "no message".to_string()
            };
            simlin_error_free(out_error);
            panic!("error with combined ops: {:?} - {}", err_code, msg_str);
        }
        assert!(collected_errors.is_null());

        let new_stop = (*proj).datamodel.lock().unwrap().sim_specs.stop;
        assert_eq!(new_stop, 100.0, "sim specs should be updated");

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("combined_test").is_some(),
            "variable should exist"
        );
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_apply_patch_invalid_utf8() {
    let datamodel = TestProject::new("utf8_test").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let invalid_utf8: Vec<u8> = vec![0xFF, 0xFE, 0xFD];

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            invalid_utf8.as_ptr(),
            invalid_utf8.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(!out_error.is_null(), "expected error for invalid UTF-8");
        assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
        simlin_error_free(out_error);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_apply_patch_empty_patch() {
    let datamodel = TestProject::new("empty_patch").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let empty_patch = r#"{"models": []}"#;
    let patch_bytes = empty_patch.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "empty patch should succeed as no-op");
        assert!(collected_errors.is_null());

        simlin_project_unref(proj);
    }
}

#[test]
fn test_apply_patch_zero_length_input() {
    // Test that zero-length input is treated as a no-op (backwards compatibility)
    let datamodel = TestProject::new("zero_len_patch").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();

        // Zero-length patch with null pointer (documented as valid)
        simlin_project_apply_patch(
            proj,
            ptr::null(),
            0,
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(
            out_error.is_null(),
            "zero-length patch should succeed as no-op"
        );
        assert!(collected_errors.is_null());

        // Also test empty string (whitespace only)
        let whitespace_patch = "   \n\t  ";
        let patch_bytes = whitespace_patch.as_bytes();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(
            out_error.is_null(),
            "whitespace-only patch should succeed as no-op"
        );
        assert!(collected_errors.is_null());

        simlin_project_unref(proj);
    }
}

#[test]
fn test_apply_patch_dry_run_no_changes() {
    let datamodel = TestProject::new("dry_run_test").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("dry_run_var").is_none(),
            "variable should not exist before patch"
        );
    }

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "dry_run_var",
                                "equation": "99"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            true,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "dry run should succeed");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("dry_run_var").is_none(),
            "variable should not exist after dry run"
        );
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_apply_patch_null_data_with_length() {
    let datamodel = TestProject::new("null_test").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            ptr::null(),
            10,
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(
            !out_error.is_null(),
            "expected error for NULL patch_data with length > 0"
        );
        assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
        simlin_error_free(out_error);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_apply_patch_multiple_models() {
    // Create a project with multiple models
    let mut datamodel = TestProject::new("multi_model").build_datamodel();
    let second_model = engine::datamodel::Model {
        name: "SecondModel".to_string(),
        sim_specs: None,
        variables: vec![],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
    };
    datamodel.models.push(second_model);

    let proj = open_project_from_datamodel(&datamodel);

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "main_var",
                                "equation": "1"
                            }
                        }
                    }
                ]
            },
            {
                "name": "SecondModel",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "second_var",
                                "equation": "2"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "multi-model patch should succeed");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let main_model = project_locked.get_model("main").unwrap();
        assert!(main_model.get_variable("main_var").is_some());

        let second_model = project_locked.get_model("SecondModel").unwrap();
        assert!(second_model.get_variable("second_var").is_some());
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_apply_patch_multiple_ops_per_model() {
    let datamodel = TestProject::new("multi_ops").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "var1",
                                "equation": "10"
                            }
                        }
                    },
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "var2",
                                "equation": "20"
                            }
                        }
                    },
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "var3",
                                "equation": "30"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        assert!(out_error.is_null(), "multiple ops should succeed");
        assert!(collected_errors.is_null());

        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(model.get_variable("var1").is_some());
        assert!(model.get_variable("var2").is_some());
        assert!(model.get_variable("var3").is_some());
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

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
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
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
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
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
        assert!(
            !sim2.is_null(),
            "non-LTM simulation should work after patch"
        );
        assert!(out_error.is_null());
        simlin_sim_run_to_end(sim2, &mut out_error);
        assert!(out_error.is_null());
        simlin_sim_unref(sim2);
        simlin_model_unref(model2);

        simlin_project_unref(proj);
    }
}

/// Verify that simlin_project_get_errors produces error details with
/// snippet/squiggle formatting when a variable has an equation error.
#[test]
fn test_project_get_errors_includes_snippets() {
    let datamodel = TestProject::new("snippet-via-ffi")
        .aux("bad", "1 + bogus", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let errors = simlin_project_get_errors(proj, &mut out_error);
        assert!(out_error.is_null(), "unexpected operational error");
        assert!(!errors.is_null(), "expected errors for bad equation");

        let detail_count = simlin_error_get_detail_count(errors);
        assert!(detail_count > 0, "expected at least one error detail");

        let mut found_snippet = false;
        for i in 0..detail_count {
            let detail = simlin_error_get_detail(errors, i);
            assert!(!detail.is_null());
            let detail_ref = &*detail;
            if !detail_ref.message.is_null() {
                let msg = CStr::from_ptr(detail_ref.message).to_str().unwrap();
                if msg.contains("1 + bogus") && msg.contains("~~~~~") {
                    found_snippet = true;
                    break;
                }
            }
        }

        assert!(
            found_snippet,
            "error detail messages should contain snippet with equation text and squiggle underline"
        );

        simlin_error_free(errors);
        simlin_project_unref(proj);
    }
}

/// Verify that apply_patch error collection includes snippet formatting
/// when the patch introduces a variable with a bad equation.
#[test]
fn test_apply_patch_errors_include_snippets() {
    let datamodel = TestProject::new("patch-snippet")
        .aux("ok_var", "42", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let patch_json = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "broken", "equation": "1 + nonexistent" } }
                }]
            }]
        }"#;
        let patch_bytes = patch_json.as_bytes();
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true, // allow_errors so the patch is accepted
            &mut collected_errors as *mut *mut SimlinError,
            &mut out_error as *mut *mut SimlinError,
        );
        assert!(
            out_error.is_null(),
            "patch should succeed with allow_errors=true"
        );
        assert!(
            !collected_errors.is_null(),
            "should have collected errors for bad equation"
        );

        let detail_count = simlin_error_get_detail_count(collected_errors);
        assert!(detail_count > 0, "expected at least one error detail");

        let mut found_snippet = false;
        for i in 0..detail_count {
            let detail = simlin_error_get_detail(collected_errors, i);
            assert!(!detail.is_null());
            let detail_ref = &*detail;
            if !detail_ref.message.is_null() {
                let msg = CStr::from_ptr(detail_ref.message).to_str().unwrap();
                if msg.contains("1 + nonexistent") && msg.contains("~~~~~~~~~~~") {
                    found_snippet = true;
                    break;
                }
            }
        }

        assert!(
            found_snippet,
            "patch error details should contain snippet with equation text and squiggle underline"
        );

        simlin_error_free(collected_errors);
        simlin_project_unref(proj);
    }
}

/// View-only patches take a fast path that skips compilation.
/// Verify the patch is actually applied to the datamodel.
#[test]
fn test_apply_patch_upsert_view_fast_path() {
    let datamodel = TestProject::new("view_fast_path")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("s", "100", &["f"], &[], None)
        .flow("f", "s * 0.1", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Apply a view-only patch
        let patch_json = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertView",
                    "payload": {
                        "index": 0,
                        "view": {
                            "elements": [
                                { "type": "stock", "uid": 1, "name": "s", "x": 100, "y": 100, "labelSide": "bottom" }
                            ],
                            "viewBox": { "x": 0, "y": 0, "width": 800, "height": 600 },
                            "zoom": 1.5
                        }
                    }
                }]
            }]
        }"#;
        let patch_bytes = patch_json.as_bytes();
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
        assert!(out_error.is_null(), "view-only patch should succeed");

        // Verify the view was persisted by serializing the project
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut serialize_err: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_json(
            proj,
            0, // Native format
            false,
            &mut out_buffer,
            &mut out_len,
            &mut serialize_err,
        );
        assert!(serialize_err.is_null());
        let json = std::str::from_utf8(std::slice::from_raw_parts(out_buffer, out_len)).unwrap();
        assert!(
            json.contains("800"),
            "serialized JSON should contain the view width"
        );

        simlin_free(out_buffer);
        if !collected.is_null() {
            simlin_error_free(collected);
        }
        simlin_project_unref(proj);
    }
}

/// Regression test: upsertView patch on a real XMILE model with existing
/// views must not panic/trap. Exercises the same path as queueViewUpdate
/// in the diagram editor (panning the canvas).
#[test]
fn test_apply_patch_upsert_view_xmile_model() {
    let teacup_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../src/pysimlin/tests/fixtures/teacup.stmx");
    if !teacup_path.exists() {
        eprintln!("missing teacup.stmx fixture; skipping");
        return;
    }
    let data = std::fs::read(&teacup_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "project_open_xmile failed");
        assert!(!proj.is_null());

        // Apply an upsertView patch (simulates panning the diagram)
        let patch_json = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertView",
                    "payload": {
                        "index": 0,
                        "view": {
                            "elements": [
                                { "type": "stock", "uid": 1, "name": "Teacup Temperature", "x": 300, "y": 200, "labelSide": "bottom" },
                                { "type": "flow", "uid": 2, "name": "Heat Loss to Room", "x": 200, "y": 200, "points": [{"x": 300, "y": 200}, {"x": 100, "y": 200}], "labelSide": "bottom" },
                                { "type": "cloud", "uid": 3, "flowUid": 2, "x": 100, "y": 200 },
                                { "type": "aux", "uid": 4, "name": "Room Temperature", "x": 200, "y": 340, "labelSide": "bottom" },
                                { "type": "link", "uid": 5, "fromUid": 1, "toUid": 2, "arc": 30 },
                                { "type": "link", "uid": 6, "fromUid": 4, "toUid": 2, "arc": -30 }
                            ],
                            "viewBox": { "x": -150, "y": -100, "width": 800, "height": 600 },
                            "zoom": 1.0
                        }
                    }
                }]
            }]
        }"#;
        let patch_bytes = patch_json.as_bytes();
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

        if !out_error.is_null() {
            let code = simlin_error_get_code(out_error);
            let msg_ptr = simlin_error_get_message(out_error);
            let msg = if msg_ptr.is_null() {
                "(null)"
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(out_error);
            panic!(
                "upsertView patch on xmile model should not fail: code={:?}, msg={}",
                code, msg
            );
        }

        if !collected.is_null() {
            simlin_error_free(collected);
        }
        simlin_project_unref(proj);
    }
}

/// Regression test: upsertView patch (triggered by panning the diagram)
/// must not panic/trap. This exercises the same code path as
/// queueViewUpdate in the diagram editor.
#[test]
fn test_apply_patch_upsert_view_does_not_panic() {
    let datamodel = TestProject::new("upsert_view_pan")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.03", None)
        .flow("deaths", "population * 0.01", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Simulate the view patch that queueViewUpdate sends when the user pans.
        // The view includes stock/flow/aux/cloud/link elements and a viewBox.
        let patch_json = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertView",
                    "payload": {
                        "index": 0,
                        "view": {
                            "elements": [
                                { "type": "stock", "uid": 1, "name": "population", "x": 100, "y": 100, "labelSide": "bottom" },
                                { "type": "flow", "uid": 2, "name": "births", "x": 50, "y": 100, "points": [{"x": 0, "y": 100}, {"x": 100, "y": 100}], "labelSide": "bottom" },
                                { "type": "cloud", "uid": 3, "flowUid": 2, "x": 0, "y": 100 },
                                { "type": "flow", "uid": 4, "name": "deaths", "x": 150, "y": 100, "points": [{"x": 100, "y": 100}, {"x": 200, "y": 100}], "labelSide": "bottom" },
                                { "type": "cloud", "uid": 5, "flowUid": 4, "x": 200, "y": 100 },
                                { "type": "link", "uid": 6, "fromUid": 1, "toUid": 2, "arc": 30 }
                            ],
                            "viewBox": { "x": -50, "y": -50, "width": 800, "height": 600 },
                            "zoom": 1.0
                        }
                    }
                }]
            }]
        }"#;
        let patch_bytes = patch_json.as_bytes();
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

        if !out_error.is_null() {
            let code = simlin_error_get_code(out_error);
            let msg_ptr = simlin_error_get_message(out_error);
            let msg = if msg_ptr.is_null() {
                "(null)"
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(out_error);
            panic!(
                "upsertView patch should not fail: code={:?}, msg={}",
                code, msg
            );
        }

        if !collected.is_null() {
            simlin_error_free(collected);
        }

        // Now simulate a second pan (updated viewBox), same as a continuous pan gesture
        let patch_json2 = r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertView",
                    "payload": {
                        "index": 0,
                        "view": {
                            "elements": [
                                { "type": "stock", "uid": 1, "name": "population", "x": 100, "y": 100, "labelSide": "bottom" },
                                { "type": "flow", "uid": 2, "name": "births", "x": 50, "y": 100, "points": [{"x": 0, "y": 100}, {"x": 100, "y": 100}], "labelSide": "bottom" },
                                { "type": "cloud", "uid": 3, "flowUid": 2, "x": 0, "y": 100 },
                                { "type": "flow", "uid": 4, "name": "deaths", "x": 150, "y": 100, "points": [{"x": 100, "y": 100}, {"x": 200, "y": 100}], "labelSide": "bottom" },
                                { "type": "cloud", "uid": 5, "flowUid": 4, "x": 200, "y": 100 },
                                { "type": "link", "uid": 6, "fromUid": 1, "toUid": 2, "arc": 30 }
                            ],
                            "viewBox": { "x": -100, "y": -80, "width": 800, "height": 600 },
                            "zoom": 1.0
                        }
                    }
                }]
            }]
        }"#;
        let patch_bytes2 = patch_json2.as_bytes();
        collected = ptr::null_mut();
        out_error = ptr::null_mut();

        simlin_project_apply_patch(
            proj,
            patch_bytes2.as_ptr(),
            patch_bytes2.len(),
            false,
            true,
            &mut collected,
            &mut out_error,
        );

        if !out_error.is_null() {
            let code = simlin_error_get_code(out_error);
            let msg_ptr = simlin_error_get_message(out_error);
            let msg = if msg_ptr.is_null() {
                "(null)"
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(out_error);
            panic!(
                "second upsertView patch should not fail: code={:?}, msg={}",
                code, msg
            );
        }

        if !collected.is_null() {
            simlin_error_free(collected);
        }
        simlin_project_unref(proj);
    }
}

#[test]
fn test_patch_with_preexisting_unit_warnings_succeeds() {
    // Create a project that already has unit warnings (apples + oranges)
    let datamodel = TestProject::new("unit_test")
        .unit("apples", None)
        .unit("oranges", None)
        .aux("a", "10", Some("apples"))
        .aux("b", "20", Some("oranges"))
        .aux("c", "a + b", None) // unit mismatch: apples + oranges
        .build_datamodel();

    let proj = open_project_from_datamodel(&datamodel);

    // Verify the project has unit warnings via salsa diagnostics.
    // Unit mismatch can surface as either a DiagnosticError::Unit (from
    // units_check) or DiagnosticError::Model with UnitMismatch code (from
    // units_infer). Both indicate unit-related problems.
    {
        let db = unsafe { (*proj).db.lock().unwrap() };
        let sync_state = unsafe { (*proj).sync_state.lock().unwrap() };
        let sync = sync_state.as_ref().unwrap().to_sync_result();
        let diags = engine::db::collect_all_diagnostics(&db, &sync);
        let has_unit_diags = diags.iter().any(|d| {
            d.severity == engine::db::DiagnosticSeverity::Warning
                && (matches!(d.error, engine::db::DiagnosticError::Unit(_))
                    || matches!(
                        &d.error,
                        engine::db::DiagnosticError::Model(e)
                        if e.code == engine::common::ErrorCode::UnitMismatch
                    ))
        });
        assert!(has_unit_diags, "expected unit warnings in the model");
    }

    // Apply a patch that doesn't introduce new unit warnings
    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "d",
                                "equation": "5",
                                "units": "apples"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
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

        // Should succeed despite pre-existing unit warnings
        if !out_error.is_null() {
            let msg_ptr = simlin_error_get_message(out_error);
            let msg = if !msg_ptr.is_null() {
                CStr::from_ptr(msg_ptr).to_str().unwrap_or("")
            } else {
                ""
            };
            panic!("unexpected error: {}", msg);
        }
        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        // Verify the patch was applied
        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(model.get_variable("d").is_some(), "patch should be applied");
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_patch_introducing_new_unit_warning_rejected() {
    // Create a clean project with no unit warnings
    let datamodel = TestProject::new("unit_test")
        .unit("apples", None)
        .unit("oranges", None)
        .aux("a", "10", Some("apples"))
        .aux("b", "20", Some("oranges"))
        .build_datamodel();

    let proj = open_project_from_datamodel(&datamodel);

    // Verify no unit warnings initially via salsa diagnostics
    {
        let db = unsafe { (*proj).db.lock().unwrap() };
        let sync_state = unsafe { (*proj).sync_state.lock().unwrap() };
        let sync = sync_state.as_ref().unwrap().to_sync_result();
        let diags = engine::db::collect_all_diagnostics(&db, &sync);
        let has_unit_diags = diags.iter().any(|d| {
            matches!(d.error, engine::db::DiagnosticError::Unit(_))
                && d.severity == engine::db::DiagnosticSeverity::Warning
        });
        assert!(!has_unit_diags, "should not have unit warnings initially");
    }

    // Apply a patch that introduces a unit mismatch (apples + oranges)
    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "bad_sum",
                                "equation": "a + b"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
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

        // Should fail because patch introduces a new unit warning
        assert!(
            !out_error.is_null(),
            "expected error when introducing new unit warning"
        );
        let code = simlin_error_get_code(out_error);
        assert_eq!(
            code,
            SimlinErrorCode::UnitMismatch,
            "expected UnitMismatch error"
        );
        simlin_error_free(out_error);
        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        // Verify the patch was NOT applied
        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("bad_sum").is_none(),
            "patch should NOT be applied"
        );
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_patch_introducing_new_unit_warning_allowed_with_flag() {
    // Create a clean project with no unit warnings
    let datamodel = TestProject::new("unit_test")
        .unit("apples", None)
        .unit("oranges", None)
        .aux("a", "10", Some("apples"))
        .aux("b", "20", Some("oranges"))
        .build_datamodel();

    let proj = open_project_from_datamodel(&datamodel);

    // Apply a patch that introduces a unit mismatch, but with allow_errors = true
    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "upsertAux",
                        "payload": {
                            "aux": {
                                "name": "bad_sum",
                                "equation": "a + b"
                            }
                        }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
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
        assert!(
            out_error.is_null(),
            "expected no error when allow_errors = true"
        );
        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        // Verify the patch WAS applied
        let project_locked = (*proj).datamodel.lock().unwrap();
        let model = project_locked.get_model("main").unwrap();
        assert!(
            model.get_variable("bad_sum").is_some(),
            "patch should be applied"
        );
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_patch_then_sim_uses_incremental_compilation() {
    let datamodel = TestProject::new("incr_compile")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * 0.01", None)
        .aux("birth_rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Apply a patch that modifies birth_rate
        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsertAux",
                            "payload": { "aux": { "name": "birth_rate", "equation": "0.03" } }
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
            true,
            &mut collected_errors,
            &mut out_error,
        );
        assert!(out_error.is_null(), "patch should succeed");
        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        // Create simulation via incremental compilation
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());
        assert!(out_error.is_null());

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null());
        assert!(out_error.is_null());

        // Run simulation and verify results reflect the patched value
        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        // birth_rate should be 0.03 (patched value), so population should grow
        // faster than with 0.02
        let c_name = CString::new("population").unwrap();
        let mut step_count: usize = 0;
        simlin_sim_get_stepcount(sim, &mut step_count, &mut out_error);
        assert!(out_error.is_null());
        assert!(step_count > 0);

        let mut series = vec![0.0f64; step_count];
        let mut written: usize = 0;
        simlin_sim_get_series(
            sim,
            c_name.as_ptr(),
            series.as_mut_ptr(),
            step_count,
            &mut written,
            &mut out_error,
        );
        assert!(out_error.is_null());
        assert_eq!(written, step_count);
        assert!((series[0] - 100.0).abs() < 1e-9);
        assert!(*series.last().unwrap() > 100.0, "population should grow");

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_dry_run_patch_does_not_affect_project() {
    let datamodel = TestProject::new("dry_run_incr")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Apply a dry-run patch
        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsertAux",
                            "payload": { "aux": { "name": "growth_rate", "equation": "0.05" } }
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
            true, // dry_run
            true,
            &mut collected_errors,
            &mut out_error,
        );
        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        // Simulation should still work with original model (dry-run
        // should not have modified the project or its salsa DB state)
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null());
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_multiple_patches_then_sim() {
    let datamodel = TestProject::new("multi_patch")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * 0.01", None)
        .aux("birth_rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();

        // First patch: change birth_rate to 0.03
        let patch1 = r#"{
            "models": [{ "name": "main", "ops": [
                { "type": "upsertAux", "payload": { "aux": { "name": "birth_rate", "equation": "0.03" } } }
            ]}]
        }"#;
        let patch1_bytes = patch1.as_bytes();
        let mut collected1: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch1_bytes.as_ptr(),
            patch1_bytes.len(),
            false,
            true,
            &mut collected1,
            &mut out_error,
        );
        assert!(out_error.is_null());
        if !collected1.is_null() {
            simlin_error_free(collected1);
        }

        // Second patch: change birth_rate to 0.05
        let patch2 = r#"{
            "models": [{ "name": "main", "ops": [
                { "type": "upsertAux", "payload": { "aux": { "name": "birth_rate", "equation": "0.05" } } }
            ]}]
        }"#;
        let patch2_bytes = patch2.as_bytes();
        let mut collected2: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch2_bytes.as_ptr(),
            patch2_bytes.len(),
            false,
            true,
            &mut collected2,
            &mut out_error,
        );
        assert!(out_error.is_null());
        if !collected2.is_null() {
            simlin_error_free(collected2);
        }

        // sim_new should compile with the latest DB state (birth_rate=0.05)
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null());
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        // With birth_rate=0.05 and death_rate=0.01, net growth is 4%/period,
        // so after 10 periods: 100 * (1.04)^10 ~ 148. Verify it's above 140.
        let c_name = CString::new("population").unwrap();
        let mut step_count: usize = 0;
        simlin_sim_get_stepcount(sim, &mut step_count, &mut out_error);
        assert!(out_error.is_null());

        let mut series = vec![0.0f64; step_count];
        let mut written: usize = 0;
        simlin_sim_get_series(
            sim,
            c_name.as_ptr(),
            series.as_mut_ptr(),
            step_count,
            &mut written,
            &mut out_error,
        );
        assert!(out_error.is_null());
        let final_pop = *series.last().unwrap();
        assert!(
            final_pop > 140.0,
            "population with 5% net growth should exceed 140 after 10 periods, got {final_pop}"
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_sim_without_prior_patch_compiles_normally() {
    let datamodel = TestProject::new("no_patch_sim")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        // sim_new should compile via incremental path from fresh DB
        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null());
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        let c_name = CString::new("population").unwrap();
        let mut step_count: usize = 0;
        simlin_sim_get_stepcount(sim, &mut step_count, &mut out_error);
        assert!(out_error.is_null());
        assert!(step_count > 0);

        let mut series = vec![0.0f64; step_count];
        let mut written: usize = 0;
        simlin_sim_get_series(
            sim,
            c_name.as_ptr(),
            series.as_mut_ptr(),
            step_count,
            &mut written,
            &mut out_error,
        );
        assert!(out_error.is_null());
        assert!((series[0] - 100.0).abs() < 1e-9);
        assert!(*series.last().unwrap() > 100.0);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_patch_then_ltm_sim_compiles_normally() {
    let datamodel = TestProject::new("ltm_incr_bypass")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * 0.01", None)
        .aux("birth_rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();

        // Apply a patch
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
        assert!(out_error.is_null());
        if !collected.is_null() {
            simlin_error_free(collected);
        }

        // Create LTM simulation -- simlin_sim_new uses the incremental pipeline for both LTM and non-LTM
        let model = simlin_project_get_model(proj, ptr::null(), &mut out_error);
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, true, &mut out_error);
        assert!(!sim.is_null());
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null());

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Deleting a module variable from a model with dependent variables (flows
/// whose equations reference module outputs like `candidates_outflows.actual`)
/// must not panic.  The patch should apply successfully with `allow_errors`,
/// and the resulting project should report compilation errors for the
/// now-dangling references instead of crashing in the compiler.
#[test]
fn test_delete_module_variable_does_not_panic() {
    let json_bytes = include_bytes!("../../../test/hiring.sd.json");

    let proj = unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_json(
            json_bytes.as_ptr(),
            json_bytes.len(),
            SimlinJsonFormat::Native as u32,
            &mut err,
        );
        assert!(!proj.is_null(), "hiring.sd.json must open successfully");
        assert!(err.is_null());
        proj
    };

    // The model should be simulatable before the delete.
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut err);
        assert!(!sim.is_null(), "hiring model must simulate before delete");
        assert!(err.is_null());
        simlin_sim_unref(sim);
        simlin_model_unref(model);
    }

    // Delete a module variable.  Other variables still reference its
    // outputs (e.g. flow equation `candidates_outflows.actual`), so the
    // model will have compilation errors -- but the engine must not panic.
    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "deleteVariable",
                        "payload": { "ident": "candidates_outflows" }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true, // allow_errors -- the dependent variables will have errors
            &mut collected_errors,
            &mut out_error,
        );

        // The patch itself must not return a fatal error (no panic).
        if !out_error.is_null() {
            let msg_ptr = simlin_error_get_message(out_error);
            let msg = if !msg_ptr.is_null() {
                CStr::from_ptr(msg_ptr).to_str().unwrap_or("<non-utf8>")
            } else {
                "<no message>"
            };
            panic!(
                "delete-module patch must not produce a fatal error, got: {}",
                msg
            );
        }

        // The module should be gone.
        {
            let dm = (*proj).datamodel.lock().unwrap();
            let model = dm.get_model("main").unwrap();
            assert!(
                model.get_variable("candidates_outflows").is_none(),
                "module should be deleted"
            );
        }

        // There should be collected errors (dangling references), but no panic.
        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        simlin_project_unref(proj);
    }
}

/// Deleting a variable that is referenced as a module input source (e.g.
/// stock "candidates" which is wired into module "candidates_outflows" via
/// `src: "candidates"`) must not panic during module compilation.
/// The `.unwrap()` on `get_offset` in the `Variable::Module` arm of
/// `Var::new` would crash if the source variable is missing.
#[test]
fn test_delete_module_input_source_does_not_panic() {
    let json_bytes = include_bytes!("../../../test/hiring.sd.json");

    let proj = unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_json(
            json_bytes.as_ptr(),
            json_bytes.len(),
            SimlinJsonFormat::Native as u32,
            &mut err,
        );
        assert!(!proj.is_null(), "hiring.sd.json must open successfully");
        assert!(err.is_null());
        proj
    };

    // Delete "candidates" -- a stock that is wired as an input to the
    // module "candidates_outflows" (src: "candidates", dst:
    // "candidates_outflows.available").  The module still exists, so
    // compiling it must not panic when resolving the now-missing input.
    let patch_json = r#"{
        "models": [
            {
                "name": "main",
                "ops": [
                    {
                        "type": "deleteVariable",
                        "payload": { "ident": "candidates" }
                    }
                ]
            }
        ]
    }"#;
    let patch_bytes = patch_json.as_bytes();

    unsafe {
        let mut collected_errors: *mut SimlinError = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_bytes.as_ptr(),
            patch_bytes.len(),
            false,
            true,
            &mut collected_errors,
            &mut out_error,
        );

        if !out_error.is_null() {
            let msg_ptr = simlin_error_get_message(out_error);
            let msg = if !msg_ptr.is_null() {
                CStr::from_ptr(msg_ptr).to_str().unwrap_or("<non-utf8>")
            } else {
                "<no message>"
            };
            panic!(
                "delete-input-source patch must not produce a fatal error, got: {}",
                msg
            );
        }

        {
            let dm = (*proj).datamodel.lock().unwrap();
            let model = dm.get_model("main").unwrap();
            assert!(
                model.get_variable("candidates").is_none(),
                "candidates should be deleted"
            );
            assert!(
                model.get_variable("candidates_outflows").is_some(),
                "module should still exist"
            );
        }

        if !collected_errors.is_null() {
            simlin_error_free(collected_errors);
        }

        simlin_project_unref(proj);
    }
}
