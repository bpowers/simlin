#[test]
fn test_project_json_roundtrip_sdai() {
    let original_datamodel = TestProject::new("sdai_roundtrip")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&original_datamodel);

    unsafe {
        // Serialize to SDAI format
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_json(
            proj,
            ffi::SimlinJsonFormat::Sdai as u32,
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(out_error.is_null(), "serialization should succeed");
        assert!(!out_buffer.is_null());

        // Re-open from SDAI JSON
        let mut open_error: *mut SimlinError = ptr::null_mut();
        let proj2 = simlin_project_open_json(
            out_buffer,
            out_len,
            ffi::SimlinJsonFormat::Sdai as u32,
            &mut open_error,
        );

        assert!(open_error.is_null(), "open from SDAI JSON should succeed");
        assert!(!proj2.is_null());

        // Verify the model exists and has the expected variables
        let mut get_model_error: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj2, ptr::null(), &mut get_model_error);
        assert!(get_model_error.is_null());
        assert!(!model.is_null());

        // Verify variables exist
        let project2_locked = (*proj2).project.lock().unwrap();
        let roundtrip_datamodel = &project2_locked.datamodel;
        let roundtrip_model = roundtrip_datamodel.get_model("main").unwrap();

        assert!(roundtrip_model.get_variable("population").is_some());
        assert!(roundtrip_model.get_variable("births").is_some());
        assert!(roundtrip_model.get_variable("deaths").is_some());
        drop(project2_locked);

        simlin_free(out_buffer);
        simlin_model_unref(model);
        simlin_project_unref(proj2);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_serialize_json_null_out_buffer() {
    let datamodel = TestProject::new("error_test").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_json(
            proj,
            ffi::SimlinJsonFormat::Native as u32,
            ptr::null_mut(),
            &mut out_len,
            &mut out_error,
        );

        assert!(!out_error.is_null(), "expected error for NULL out_buffer");
        assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
        simlin_error_free(out_error);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_serialize_json_null_out_len() {
    let datamodel = TestProject::new("error_test").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_json(
            proj,
            ffi::SimlinJsonFormat::Native as u32,
            &mut out_buffer,
            ptr::null_mut(),
            &mut out_error,
        );

        assert!(!out_error.is_null(), "expected error for NULL out_len");
        assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
        simlin_error_free(out_error);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_serialize_json_null_project() {
    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_json(
            ptr::null_mut(),
            ffi::SimlinJsonFormat::Native as u32,
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(!out_error.is_null(), "expected error for NULL project");
        assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
        simlin_error_free(out_error);
    }
}

#[test]
fn test_serialize_json_both_formats_work() {
    let datamodel = TestProject::new("format_test").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // Test Native format
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_json(
            proj,
            ffi::SimlinJsonFormat::Native as u32,
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(out_error.is_null(), "Native format should succeed");
        assert!(!out_buffer.is_null());
        assert!(out_len > 0);
        simlin_free(out_buffer);

        // Test SDAI format
        out_buffer = ptr::null_mut();
        out_len = 0;
        simlin_project_serialize_json(
            proj,
            ffi::SimlinJsonFormat::Sdai as u32,
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(out_error.is_null(), "SDAI format should succeed");
        assert!(!out_buffer.is_null());
        assert!(out_len > 0);
        simlin_free(out_buffer);

        simlin_project_unref(proj);
    }
}

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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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
        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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
        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
        assert!(model.views.is_empty(), "view should not exist after delete");
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_apply_patch_set_sim_specs() {
    let datamodel = TestProject::new("json_patch_sim_specs").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let original_start = unsafe { (*proj).project.lock().unwrap().datamodel.sim_specs.start };
    let original_stop = unsafe { (*proj).project.lock().unwrap().datamodel.sim_specs.stop };

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

        let new_start = (*proj).project.lock().unwrap().datamodel.sim_specs.start;
        let new_stop = (*proj).project.lock().unwrap().datamodel.sim_specs.stop;

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

        let new_stop = (*proj).project.lock().unwrap().datamodel.sim_specs.stop;
        assert_eq!(new_stop, 100.0, "sim specs should be updated");

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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
        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let main_model = project_locked.datamodel.get_model("main").unwrap();
        assert!(main_model.get_variable("main_var").is_some());

        let second_model = project_locked.datamodel.get_model("SecondModel").unwrap();
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

        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
        assert!(model.get_variable("var1").is_some());
        assert!(model.get_variable("var2").is_some());
        assert!(model.get_variable("var3").is_some());
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_interactive_set_get() {
    // Load the SIR project fixture
    let pb_path = std::path::Path::new("../../src/engine/testdata/SIR_project.pb");
    if !pb_path.exists() {
        eprintln!("missing SIR_project.pb fixture; skipping");
        return;
    }
    let data = std::fs::read(pb_path).unwrap();

    unsafe {
        // Open project
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
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
            panic!("project open failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        // Get model
        err = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!model.is_null());

        // Create sim
        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Run to a partial time
        err = ptr::null_mut();
        simlin_sim_run_to(sim, 0.125, &mut err as *mut *mut SimlinError);
        if !err.is_null() {
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if msg_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(err);
            panic!("sim_run_to failed with error {:?}: {}", code, msg);
        }

        // Fetch var names from model
        err = ptr::null_mut();
        let mut count: usize = 0;
        simlin_model_get_var_count(
            model,
            &mut count as *mut usize,
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
            panic!("get_var_count failed with error {:?}: {}", code, msg);
        }
        assert!(count > 0, "expected varcount > 0");

        let mut name_ptrs: Vec<*mut c_char> = vec![std::ptr::null_mut(); count];
        let _written: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_names(
            model,
            name_ptrs.as_mut_ptr(),
            name_ptrs.len(),
            &mut count as *mut usize,
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
            panic!("get_var_names failed with error {:?}: {}", code, msg);
        }

        // Find canonical name that ends with "infectious"
        let mut infectious_name: Option<String> = None;
        for &p in &name_ptrs {
            if p.is_null() {
                continue;
            }
            let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
            // free the CString from get_var_names
            simlin_free_string(p as *mut c_char);
            if s.to_ascii_lowercase().ends_with("infectious") {
                infectious_name = Some(s);
            }
        }
        let infectious = infectious_name.expect("infectious not found in names");

        // Read current value using canonical name
        let c_infectious = CString::new(infectious.clone()).unwrap();
        let mut out: c_double = 0.0;
        err = ptr::null_mut();
        simlin_sim_get_value(
            sim,
            c_infectious.as_ptr(),
            &mut out as *mut c_double,
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
            panic!("get_value failed with error {:?}: {}", code, msg);
        }

        // Set to a new value and read it back
        let new_val: f64 = 42.0;
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_infectious.as_ptr(),
            new_val as c_double,
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
            panic!("set_value failed with error {:?}: {}", code, msg);
        }

        let mut out2: c_double = 0.0;
        err = ptr::null_mut();
        simlin_sim_get_value(
            sim,
            c_infectious.as_ptr(),
            &mut out2 as *mut c_double,
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
            panic!(
                "get_value (after set) failed with error {:?}: {}",
                code, msg
            );
        }
        assert!(
            (out2 - new_val).abs() <= 1e-9,
            "expected {new_val} got {out2}"
        );

        // Cleanup
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_set_value_phases() {
    // Load the SIR project fixture
    let pb_path = std::path::Path::new("../../src/engine/testdata/SIR_project.pb");
    if !pb_path.exists() {
        eprintln!("missing SIR_project.pb fixture; skipping");
        return;
    }
    let data = std::fs::read(pb_path).unwrap();

    unsafe {
        // Open project
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
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
            panic!("project open failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        // Get model
        err = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!model.is_null());

        // Test Phase 1: Set value before first run_to (initial value)
        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Get variable names to find a valid variable
        err = ptr::null_mut();
        let mut count: usize = 0;
        simlin_model_get_var_count(
            model,
            &mut count as *mut usize,
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
            panic!("get_var_count failed with error {:?}: {}", code, msg);
        }

        let mut name_ptrs: Vec<*mut c_char> = vec![std::ptr::null_mut(); count];
        let _written: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_names(
            model,
            name_ptrs.as_mut_ptr(),
            name_ptrs.len(),
            &mut count as *mut usize,
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
            panic!("get_var_names failed with error {:?}: {}", code, msg);
        }

        let mut test_var_name: Option<String> = None;
        for &p in &name_ptrs {
            if p.is_null() {
                continue;
            }
            let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
            simlin_free_string(p as *mut c_char);
            if s.to_ascii_lowercase().ends_with("infectious") {
                test_var_name = Some(s);
                break;
            }
        }
        let test_var = test_var_name.expect("test variable not found");
        let c_test_var = CString::new(test_var.clone()).unwrap();

        // Set initial value before any run_to
        let initial_val: f64 = 100.0;
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_test_var.as_ptr(),
            initial_val,
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
            panic!("set_value before run failed with error {:?}: {}", code, msg);
        }

        // Verify initial value is set
        let mut out: c_double = 0.0;
        err = ptr::null_mut();
        simlin_sim_get_value(
            sim,
            c_test_var.as_ptr(),
            &mut out,
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
            panic!("get_value failed with error {:?}: {}", code, msg);
        }
        assert!(
            (out - initial_val).abs() <= 1e-9,
            "initial value not set correctly"
        );

        // Test Phase 2: Set value during simulation (after partial run)
        err = ptr::null_mut();
        simlin_sim_run_to(sim, 0.5, &mut err as *mut *mut SimlinError);
        if !err.is_null() {
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if msg_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(err);
            panic!("sim_run_to failed with error {:?}: {}", code, msg);
        }

        let during_val: f64 = 200.0;
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_test_var.as_ptr(),
            during_val,
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
            panic!("set_value during run failed with error {:?}: {}", code, msg);
        }

        err = ptr::null_mut();
        simlin_sim_get_value(
            sim,
            c_test_var.as_ptr(),
            &mut out,
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
            panic!("get_value failed with error {:?}: {}", code, msg);
        }
        assert!(
            (out - during_val).abs() <= 1e-9,
            "value during run not set correctly"
        );

        // Test Phase 3: Set value after run_to_end (should fail)
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
        if !err.is_null() {
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if msg_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(err);
            panic!("sim_run_to_end failed with error {:?}: {}", code, msg);
        }

        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_test_var.as_ptr(),
            300.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "Expected an error but got success");
        let code = simlin_error_get_code(err);
        assert_eq!(
            code,
            SimlinErrorCode::NotSimulatable,
            "set_value after completion should fail with NotSimulatable"
        );
        simlin_error_free(err);

        // Test setting unknown variable (should fail)
        let unknown = CString::new("unknown_variable_xyz").unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            unknown.as_ptr(),
            999.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "Expected an error but got success");
        let code = simlin_error_get_code(err);
        assert_eq!(
            code,
            SimlinErrorCode::UnknownDependency,
            "set_value for unknown variable should fail with UnknownDependency"
        );
        simlin_error_free(err);

        // Cleanup
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
// Model-only protobufs are not supported at the ABI layer; only Projects are accepted.

#[test]
fn test_project_lifecycle() {
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
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
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
        if !err.is_null() {
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if msg_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(err);
            panic!("project open failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());
        // Test reference counting
        simlin_project_ref(proj);
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);
        simlin_project_unref(proj);
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);
        simlin_project_unref(proj);
        // Project should be freed now
    }
}

#[test]
fn test_import_xmile() {
    // Load the SIR XMILE model
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    if !xmile_path.exists() {
        eprintln!("missing SIR.stmx fixture; skipping");
        return;
    }
    let data = std::fs::read(xmile_path).unwrap();

    unsafe {
        // Import XMILE
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
            panic!("project_open_xmile failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        // Get model and verify we can create a simulation from the imported project
        err = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!model.is_null());

        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Run simulation to verify it's valid
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
        if !err.is_null() {
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if msg_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(err);
            panic!("sim_run_to_end failed with error {:?}: {}", code, msg);
        }

        // Check we have expected variables
        err = ptr::null_mut();
        let mut var_count: usize = 0;
        simlin_model_get_var_count(
            model,
            &mut var_count as *mut usize,
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
            panic!("get_var_count failed with error {:?}: {}", code, msg);
        }
        assert!(var_count > 0);

        // Clean up
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_import_mdl() {
    // Load the SIR MDL model
    let mdl_path = std::path::Path::new("testdata/SIR.mdl");
    if !mdl_path.exists() {
        eprintln!("missing SIR.mdl fixture; skipping");
        return;
    }
    let data = std::fs::read(mdl_path).unwrap();

    unsafe {
        // Import MDL
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_vensim(
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
            panic!("project_open_vensim failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        // Get model and verify we can create a simulation from the imported project
        err = ptr::null_mut();
        let model =
            simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!model.is_null());

        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Run simulation to verify it's valid
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
        if !err.is_null() {
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if msg_ptr.is_null() {
                ""
            } else {
                CStr::from_ptr(msg_ptr).to_str().unwrap()
            };
            simlin_error_free(err);
            panic!("sim_run_to_end failed with error {:?}: {}", code, msg);
        }

        // Check we have expected variables
        err = ptr::null_mut();
        let mut var_count: usize = 0;
        simlin_model_get_var_count(
            model,
            &mut var_count as *mut usize,
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
            panic!("get_var_count failed with error {:?}: {}", code, msg);
        }
        assert!(var_count > 0);

        // Clean up
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_get_incoming_links() {
    // Create a project with a flow that depends on a rate and a stock using TestProject
    let test_project = TestProject::new("test")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("Stock", "100", &["flow"], &[], None)
        .flow("flow", "rate * Stock", None)
        .aux("rate", "0.5", None);

    // Build the datamodel and serialize to protobuf
    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project);

    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            buf.as_ptr(),
            buf.len(),
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
            panic!("project open failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        if !err_get_model.is_null() {
            simlin_error_free(err_get_model);
            panic!("get_model failed");
        }
        assert!(!model.is_null());
        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Test getting incoming links for the flow
        let flow_name = CString::new("flow").unwrap();

        // Test 1: Query the number of dependencies with max=0
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            flow_name.as_ptr(),
            ptr::null_mut(), // result can be null when max=0
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(count, 2, "Expected 2 dependencies for flow when querying");

        // Test 2: Try with insufficient array size (should return error)
        let mut small_links: [*mut c_char; 1] = [ptr::null_mut(); 1];
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            flow_name.as_ptr(),
            small_links.as_mut_ptr(),
            1, // Only room for 1, but there are 2 dependencies
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "Expected error when array too small");
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::Generic);
        simlin_error_free(err);

        // Test 3: Proper usage - query then allocate
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            flow_name.as_ptr(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(count, 2);

        // Allocate exact size needed
        let mut links = vec![ptr::null_mut::<c_char>(); count];
        let mut count2: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            flow_name.as_ptr(),
            links.as_mut_ptr(),
            count,
            &mut count2 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(
            count2, count,
            "Should return same count when array is exact size"
        );

        // Collect the dependency names
        let mut dep_names = Vec::new();
        for link in links.iter().take(count2) {
            assert!(!link.is_null());
            let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
            dep_names.push(dep_name);
            simlin_free_string(*link);
        }

        // Check that we got both "rate" and "stock" (canonicalized to lowercase)
        assert!(
            dep_names.contains(&"rate".to_string()),
            "Missing 'rate' dependency"
        );
        assert!(
            dep_names.contains(&"stock".to_string()),
            "Missing 'stock' dependency"
        );

        // Test getting incoming links for rate (should have none since it's a constant)
        let rate_name = CString::new("rate").unwrap();
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            rate_name.as_ptr(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle error gracefully
        assert_eq!(count, 0, "Expected 0 dependencies for rate");

        // Test getting incoming links for stock (initial value is constant, so no deps)
        let stock_name = CString::new("Stock").unwrap();
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            stock_name.as_ptr(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle error gracefully
        assert_eq!(
            count, 0,
            "Expected 0 dependencies for Stock's initial value"
        );

        // Test error cases
        // Non-existent variable
        let nonexistent = CString::new("nonexistent").unwrap();
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            nonexistent.as_ptr(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "Expected error for non-existent variable");
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::DoesNotExist);
        simlin_error_free(err);

        // Null pointer checks
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            ptr::null_mut(),
            flow_name.as_ptr(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "Expected error for null model");
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::Generic);
        simlin_error_free(err);

        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            ptr::null(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "Expected error for null var_name");
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::Generic);
        simlin_error_free(err);

        // Test that result being null with max > 0 is an error
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            flow_name.as_ptr(),
            ptr::null_mut(),
            10,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "Expected error for null result with max > 0"
        );
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::Generic);
        simlin_error_free(err);

        // Clean up
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_get_incoming_links_with_private_variables() {
    // Test that private variables (starting with $⁚) are not exposed in incoming links
    // Create a model with a SMOOTH function which internally creates private variables
    let test_project = TestProject::new("test")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("input", "10", None)
        .aux("smooth_time", "3", None)
        // SMTH1 creates internal private variables like $⁚smoothed⁚0⁚smth1⁚output
        .aux("smoothed", "SMTH1(input, smooth_time)", None)
        // A variable that depends on the smoothed output
        .aux("result", "smoothed * 2", None);

    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project);
    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            buf.as_ptr(),
            buf.len(),
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
            panic!("project open failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        if !err_get_model.is_null() {
            simlin_error_free(err_get_model);
            panic!("get_model failed");
        }
        assert!(!model.is_null());

        // Test getting incoming links for 'smoothed' variable
        // It should show 'input' and 'smooth_time' as dependencies,
        // but NOT any private variables like $⁚smoothed⁚0⁚smooth⁚output
        let smoothed_name = CString::new("smoothed").unwrap();
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            smoothed_name.as_ptr(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle error gracefully

        // Get the actual dependencies
        let mut links = vec![ptr::null_mut::<c_char>(); count];
        let mut count2: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            smoothed_name.as_ptr(),
            links.as_mut_ptr(),
            count,
            &mut count2 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(count2, count);

        // Collect dependency names
        let mut dep_names = Vec::new();
        for link in links.iter().take(count2) {
            assert!(!link.is_null());
            let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
            dep_names.push(dep_name.clone());

            simlin_free_string(*link);
        }

        // Assert that no private variable is exposed
        for dep_name in &dep_names {
            assert!(
                !dep_name.starts_with("$"),
                "Private variable '{}' should not be exposed in incoming links",
                dep_name
            );
        }

        // Should have input and smooth_time as dependencies
        assert!(
            dep_names.contains(&"input".to_string()),
            "Missing 'input' dependency"
        );
        assert!(
            dep_names.contains(&"smooth_time".to_string()),
            "Missing 'smooth_time' dependency"
        );

        // Clean up
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_get_incoming_links_nested_private_vars() {
    // Test that nested private variables are resolved transitively
    // Create a model with chained SMTH1 functions which create nested private variables
    let test_project = TestProject::new("test")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("base_input", "TIME", None)
        .aux("delay1", "2", None)
        .aux("delay2", "3", None)
        // First smoothing creates private variables
        .aux("smooth1", "SMTH1(base_input, delay1)", None)
        // Second smoothing uses first, creating more private variables
        .aux("smooth2", "SMTH1(smooth1, delay2)", None)
        // Final result uses the second smoothed value
        .aux("final_output", "smooth2 * 1.5", None);

    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project);
    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            buf.as_ptr(),
            buf.len(),
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
            panic!("project open failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        if !err_get_model.is_null() {
            simlin_error_free(err_get_model);
            panic!("get_model failed");
        }
        assert!(!model.is_null());

        // Test smooth1 dependencies - should resolve to base_input and delay1
        let smooth1_name = CString::new("smooth1").unwrap();
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            smooth1_name.as_ptr(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle error gracefully

        let mut links = vec![ptr::null_mut::<c_char>(); count];
        let mut count2: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            smooth1_name.as_ptr(),
            links.as_mut_ptr(),
            count,
            &mut count2 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());

        let mut smooth1_deps = Vec::new();
        for link in links.iter().take(count2) {
            let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
            smooth1_deps.push(dep_name.clone());
            assert!(
                !dep_name.starts_with("$"),
                "No private vars in smooth1 deps"
            );
            simlin_free_string(*link);
        }

        assert!(smooth1_deps.contains(&"base_input".to_string()));
        assert!(smooth1_deps.contains(&"delay1".to_string()));

        // Test smooth2 dependencies - should transitively resolve through smooth1's private vars
        let smooth2_name = CString::new("smooth2").unwrap();
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            smooth2_name.as_ptr(),
            ptr::null_mut(),
            0,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle error gracefully

        let mut links = vec![ptr::null_mut::<c_char>(); count];
        let mut count2: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_incoming_links(
            model,
            smooth2_name.as_ptr(),
            links.as_mut_ptr(),
            count,
            &mut count2 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());

        let mut smooth2_deps = Vec::new();
        for link in links.iter().take(count2) {
            let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
            smooth2_deps.push(dep_name.clone());
            assert!(
                !dep_name.starts_with("$"),
                "No private vars in smooth2 deps"
            );
            simlin_free_string(*link);
        }

        // smooth2 depends on smooth1's module output, which transitively depends on
        // base_input, delay1, plus smooth2's own delay2
        assert!(
            smooth2_deps.contains(&"smooth1".to_string()),
            "Should depend on smooth1"
        );
        assert!(
            smooth2_deps.contains(&"delay2".to_string()),
            "Should depend on delay2"
        );

        // Clean up
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_add_model() {
    use prost::Message;

    // Create a minimal project with just one model
    let project = engine::project_io::Project {
        name: "test_project".to_string(),
        sim_specs: Some(engine::project_io::SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: Some(engine::project_io::Dt {
                value: 0.25,
                is_reciprocal: false,
            }),
            save_step: None,
            sim_method: engine::project_io::SimMethod::Euler as i32,
            time_units: None,
        }),
        models: vec![engine::project_io::Model {
            name: "main".to_string(),
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        }],
        dimensions: vec![],
        units: vec![],
        source: None,
    };
    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    unsafe {
        // Open the project
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            buf.as_ptr(),
            buf.len(),
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
            panic!("project open failed with error {:?}: {}", code, msg);
        }
        assert!(!proj.is_null());

        // Verify initial model count
        let mut initial_count: usize = 0;
        err = ptr::null_mut();
        simlin_project_get_model_count(
            proj,
            &mut initial_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(initial_count, 1);

        // Test adding a model
        let model_name = CString::new("new_model").unwrap();
        err = ptr::null_mut();
        simlin_project_add_model(proj, model_name.as_ptr(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // Verify model count increased
        let mut new_count: usize = 0;
        err = ptr::null_mut();
        simlin_project_get_model_count(
            proj,
            &mut new_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(new_count, 2);

        // Verify we can get the new model
        err = ptr::null_mut();
        let new_model = simlin_project_get_model(
            proj,
            model_name.as_ptr(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(!new_model.is_null());
        assert!(err.is_null());
        assert_eq!((*new_model).model_name.as_str(), "new_model");

        // Verify the new model can be used to create a simulation
        err = ptr::null_mut();
        let sim = simlin_sim_new(new_model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Clean up
        simlin_sim_unref(sim);
        simlin_model_unref(new_model);

        // Test adding another model
        let model_name2 = CString::new("another_model").unwrap();
        err = ptr::null_mut();
        simlin_project_add_model(
            proj,
            model_name2.as_ptr(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        // Verify model count
        let mut final_count: usize = 0;
        err = ptr::null_mut();
        simlin_project_get_model_count(
            proj,
            &mut final_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(final_count, 3);

        // Test adding duplicate model name (should fail)
        let duplicate_name = CString::new("new_model").unwrap();
        err = ptr::null_mut();
        simlin_project_add_model(
            proj,
            duplicate_name.as_ptr(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "Expected error when adding duplicate model name"
        );
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::DuplicateVariable);
        simlin_error_free(err);

        // Model count should not have changed
        let mut count_after_dup: usize = 0;
        err = ptr::null_mut();
        simlin_project_get_model_count(
            proj,
            &mut count_after_dup as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(count_after_dup, 3);

        // Clean up
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_add_model_null_safety() {
    unsafe {
        // Test with null project
        let model_name = CString::new("test").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_add_model(
            ptr::null_mut(),
            model_name.as_ptr(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "Expected error when adding model to null project"
        );
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::Generic);
        simlin_error_free(err);

        // Create a valid project for other null tests
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
            models: vec![],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        // Test with null model name
        err = ptr::null_mut();
        simlin_project_add_model(proj, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(
            !err.is_null(),
            "Expected error when adding model with null name"
        );
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::Generic);
        simlin_error_free(err);

        // Test with empty model name
        let empty_name = CString::new("").unwrap();
        err = ptr::null_mut();
        simlin_project_add_model(proj, empty_name.as_ptr(), &mut err as *mut *mut SimlinError);
        assert!(
            !err.is_null(),
            "Expected error when adding model with empty name"
        );
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::Generic);
        simlin_error_free(err);

        // Clean up
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_json_open() {
    let json_str = r#"{
        "name": "test_json_project",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 10.0,
            "dt": "1",
            "saveStep": 1.0,
            "method": "euler",
            "timeUnits": "days"
        },
        "models": [{
            "name": "main",
            "stocks": [{
                "uid": 1,
                "name": "population",
                "initialEquation": "100",
                "inflows": [],
                "outflows": [],
                "units": "people",
                "documentation": "",
                "can_be_module_input": false,
                "is_public": false,
                "dimensions": []
            }],
            "flows": [],
            "auxiliaries": [{
                "uid": 2,
                "name": "growth_rate",
                "equation": "0.1",
                "units": "",
                "documentation": "",
                "can_be_module_input": false,
                "is_public": false,
                "dimensions": []
            }],
            "modules": [],
            "simSpecs": {
                "startTime": 0.0,
                "endTime": 10.0,
                "dt": "1",
                "saveStep": 1.0,
                "method": "",
                "timeUnits": ""
            },
            "views": []
        }],
        "dimensions": [],
        "units": []
    }"#;

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let json_bytes = json_str.as_bytes();
        let proj = simlin_project_open_json(
            json_bytes.as_ptr(),
            json_bytes.len(),
            ffi::SimlinJsonFormat::Native as u32,
            &mut err,
        );

        assert!(!proj.is_null(), "project open failed");
        // Verify we can get the model
        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        if !err_get_model.is_null() {
            simlin_error_free(err_get_model);
            panic!("get_model failed");
        }
        assert!(!model.is_null());

        // Verify variable count
        let mut var_count: usize = 0;
        let mut err_get_var_count: *mut SimlinError = ptr::null_mut();
        simlin_model_get_var_count(
            model,
            &mut var_count as *mut usize,
            &mut err_get_var_count as *mut *mut SimlinError,
        );
        if !err_get_var_count.is_null() {
            simlin_error_free(err_get_var_count);
            panic!("get_var_count failed");
        }
        assert!(var_count > 0, "expected variables in model");

        // Clean up
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_json_open_invalid_json() {
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let invalid_json = b"not valid json {";
        let proj = simlin_project_open_json(
            invalid_json.as_ptr(),
            invalid_json.len(),
            ffi::SimlinJsonFormat::Native as u32,
            &mut err,
        );

        assert!(proj.is_null(), "expected null project for invalid JSON");
        // assert_ne!(err, engine::ErrorCode::NoError as c_int);  // Obsolete assertion from old API
    }
}

#[test]
fn test_project_json_open_null_input() {
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_json(
            ptr::null(),
            0,
            ffi::SimlinJsonFormat::Native as u32,
            &mut err,
        );

        assert!(proj.is_null());
        // assert_eq!(err, engine::ErrorCode::Generic as c_int);  // Obsolete assertion from old API
    }
}

#[test]
fn test_project_json_open_logistic_growth() {
    let json_bytes = include_bytes!("../../../test/logistic-growth.sd.json");

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_json(
            json_bytes.as_ptr(),
            json_bytes.len(),
            ffi::SimlinJsonFormat::Native as u32,
            &mut err,
        );

        assert!(!proj.is_null(), "project open failed");
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_json_open_sdai_format() {
    let json_str = r#"{
        "variables": [
            {
                "type": "stock",
                "name": "inventory",
                "equation": "50",
                "units": "widgets",
                "inflows": ["production"],
                "outflows": ["sales"]
            },
            {
                "type": "flow",
                "name": "production",
                "equation": "10",
                "units": "widgets/month"
            },
            {
                "type": "flow",
                "name": "sales",
                "equation": "8",
                "units": "widgets/month"
            },
            {
                "type": "variable",
                "name": "target_inventory",
                "equation": "100",
                "units": "widgets"
            }
        ],
        "specs": {
            "startTime": 0.0,
            "stopTime": 10.0,
            "dt": 1.0,
            "timeUnits": "months"
        }
    }"#;

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let json_bytes = json_str.as_bytes();
        let proj = simlin_project_open_json(
            json_bytes.as_ptr(),
            json_bytes.len(),
            ffi::SimlinJsonFormat::Sdai as u32,
            &mut err,
        );

        assert!(!proj.is_null(), "project open failed");
        // Verify we can get the model
        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        if !err_get_model.is_null() {
            simlin_error_free(err_get_model);
            panic!("get_model failed");
        }
        assert!(!model.is_null());

        // Verify variable count (at least 4 variables, may include built-ins)
        let mut var_count: usize = 0;
        let mut err_get_var_count: *mut SimlinError = ptr::null_mut();
        simlin_model_get_var_count(
            model,
            &mut var_count as *mut usize,
            &mut err_get_var_count as *mut *mut SimlinError,
        );
        if !err_get_var_count.is_null() {
            simlin_error_free(err_get_var_count);
            panic!("get_var_count failed");
        }
        assert!(
            var_count >= 4,
            "expected at least 4 variables, got {}",
            var_count
        );

        // Clean up
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_json_open_sdai_invalid() {
    let invalid_sdai = r#"{
        "variables": [
            {
                "type": "invalid_type",
                "name": "test"
            }
        ]
    }"#;

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let json_bytes = invalid_sdai.as_bytes();
        let proj = simlin_project_open_json(
            json_bytes.as_ptr(),
            json_bytes.len(),
            ffi::SimlinJsonFormat::Sdai as u32,
            &mut err,
        );

        assert!(
            proj.is_null(),
            "expected null project for invalid SDAI JSON"
        );
        // assert_ne!(err, engine::ErrorCode::NoError as c_int);  // Obsolete assertion from old API
    }
}

#[test]
fn test_project_json_open_invalid_format() {
    // Test that passing an invalid format discriminant returns an error
    let valid_json = r#"{"name": "test"}"#;

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let json_bytes = valid_json.as_bytes();
        let proj = simlin_project_open_json(
            json_bytes.as_ptr(),
            json_bytes.len(),
            9999, // Invalid format discriminant
            &mut err,
        );

        assert!(proj.is_null(), "expected null project for invalid format");
        assert!(!err.is_null(), "expected error for invalid format");

        // Verify error message mentions invalid format
        let msg_ptr = simlin_error_get_message(err);
        assert!(!msg_ptr.is_null());
        let msg = CStr::from_ptr(msg_ptr).to_str().unwrap();
        assert!(
            msg.contains("invalid JSON format discriminant"),
            "error message should mention invalid format: {}",
            msg
        );

        simlin_error_free(err);
    }
}

#[test]
fn test_project_serialize_json_invalid_format() {
    unsafe {
        let datamodel = TestProject::new("test_invalid_format").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);
        assert!(!proj.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();

        simlin_project_serialize_json(
            proj,
            9999, // Invalid format discriminant
            &mut out_buffer,
            &mut out_len,
            &mut err,
        );

        assert!(
            out_buffer.is_null(),
            "expected null buffer for invalid format"
        );
        assert_eq!(out_len, 0, "expected zero length for invalid format");
        assert!(!err.is_null(), "expected error for invalid format");

        // Verify error message mentions invalid format
        let msg_ptr = simlin_error_get_message(err);
        assert!(!msg_ptr.is_null());
        let msg = CStr::from_ptr(msg_ptr).to_str().unwrap();
        assert!(
            msg.contains("invalid JSON format discriminant"),
            "error message should mention invalid format: {}",
            msg
        );

        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_concurrent_project_ref_unref() {
    use std::thread;

    unsafe {
        // Create a test project
        let datamodel = TestProject::new("concurrent_test").build_datamodel();
        let pb_project = engine_serde::serialize(&datamodel);
        let encoded = pb_project.encode_to_vec();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            encoded.as_ptr(),
            encoded.len(),
            &mut err as *mut *mut SimlinError,
        );

        if !err.is_null() {
            simlin_error_free(err);
            panic!("failed to create project");
        }
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
        let pb_project = engine_serde::serialize(&datamodel);
        let encoded = pb_project.encode_to_vec();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            encoded.as_ptr(),
            encoded.len(),
            &mut err as *mut *mut SimlinError,
        );

        if !err.is_null() {
            simlin_error_free(err);
            panic!("failed to create project");
        }
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
        let pb_project = engine_serde::serialize(&datamodel);
        let encoded = pb_project.encode_to_vec();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            encoded.as_ptr(),
            encoded.len(),
            &mut err as *mut *mut SimlinError,
        );

        if !err.is_null() {
            simlin_error_free(err);
            panic!("failed to create project");
        }
        assert!(!proj.is_null());

        // Get model
        let mut err_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_model as *mut *mut SimlinError,
        );
        if !err_model.is_null() {
            simlin_error_free(err_model);
            panic!("failed to get model");
        }

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
        let pb_project = engine_serde::serialize(&datamodel);
        let encoded = pb_project.encode_to_vec();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            encoded.as_ptr(),
            encoded.len(),
            &mut err as *mut *mut SimlinError,
        );

        if !err.is_null() {
            simlin_error_free(err);
            panic!("failed to create project");
        }
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

#[test]
fn test_error_kind_equation_error() {
    let datamodel = TestProject::new("kind_test")
        .aux("bad", "1 + unknown", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0);

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let equation_error = error_slice
            .iter()
            .find(|e| e.code == SimlinErrorCode::UnknownDependency)
            .expect("should have unknown dependency error");

        assert_eq!(
            equation_error.kind,
            SimlinErrorKind::Variable,
            "equation errors should have Variable kind"
        );
        assert_eq!(
            equation_error.unit_error_kind,
            SimlinUnitErrorKind::NotApplicable,
            "non-unit errors should have NotApplicable unit_error_kind"
        );

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_kind_unit_consistency_error() {
    let datamodel = TestProject::new("unit_kind_test")
        .unit("Person", None)
        .unit("Dollar", None)
        .aux("x", "1", Some("Person"))
        .aux("y", "x", Some("Dollar"))
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0);

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let unit_error = error_slice
            .iter()
            .find(|e| e.kind == SimlinErrorKind::Units)
            .expect("should have unit error");

        assert_eq!(
            unit_error.kind,
            SimlinErrorKind::Units,
            "unit errors should have Units kind"
        );
        assert_eq!(
            unit_error.unit_error_kind,
            SimlinUnitErrorKind::Consistency,
            "unit mismatch should be Consistency variant"
        );

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_kind_all_error_kinds_mapped() {
    let datamodel = TestProject::new("all_kinds_test")
        .unit("A", None)
        .unit("B", None)
        .aux("eq_error", "1 + bogus", None)
        .aux("src", "1", Some("A"))
        .aux("unit_error", "src", Some("B"))
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count >= 2, "should have at least 2 errors");

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);

        let has_variable_kind = error_slice
            .iter()
            .any(|e| e.kind == SimlinErrorKind::Variable);
        let has_units_kind = error_slice.iter().any(|e| e.kind == SimlinErrorKind::Units);

        assert!(has_variable_kind, "should have Variable kind error");
        assert!(has_units_kind, "should have Units kind error");

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_is_simulatable_valid_project() {
    let datamodel = TestProject::new("valid_sim")
        .aux("x", "time", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let is_sim =
            simlin_project_is_simulatable(proj, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "should not have error");
        assert!(is_sim, "valid project should be simulatable");

        simlin_project_unref(proj);
    }
}

#[test]
fn test_is_simulatable_invalid_project() {
    let datamodel = TestProject::new("invalid_sim")
        .aux("x", "unknown_var", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let is_sim =
            simlin_project_is_simulatable(proj, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "should not have error in out_error");
        assert!(!is_sim, "invalid project should not be simulatable");

        simlin_project_unref(proj);
    }
}

#[test]
fn test_is_simulatable_null_project() {
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let is_sim = simlin_project_is_simulatable(
            ptr::null_mut(),
            ptr::null(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(!is_sim, "null project should not be simulatable");
    }
}

#[test]
fn test_is_simulatable_with_model_name() {
    let datamodel = TestProject::new("named_model")
        .aux("x", "1", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let model_name = CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let is_sim = simlin_project_is_simulatable(
            proj,
            model_name.as_ptr(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "should not have error");
        assert!(
            is_sim,
            "valid project with named model should be simulatable"
        );

        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_kind_unit_definition_error() {
    // Create a project with an invalid unit syntax to trigger a Definition error
    let datamodel = TestProject::new("def_error_test")
        .unit("BadUnit", Some("1///invalid"))
        .aux("x", "1", Some("BadUnit"))
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0, "should have at least one error");

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let unit_def_error = error_slice
            .iter()
            .find(|e| e.unit_error_kind == SimlinUnitErrorKind::Definition);

        assert!(
            unit_def_error.is_some(),
            "should have a Definition unit error kind, got: {:?}",
            error_slice
                .iter()
                .map(|e| (e.code, e.kind, e.unit_error_kind))
                .collect::<Vec<_>>()
        );

        let def_error = unit_def_error.unwrap();
        assert_eq!(
            def_error.kind,
            SimlinErrorKind::Units,
            "definition errors should have Units kind"
        );

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_kind_unit_inference_error() {
    // Create a project with conflicting inferred units to trigger an Inference error
    // Adding Widget + Month (time units) causes inference to fail
    let datamodel = TestProject::new("infer_error_test")
        .with_time_units("Month")
        .unit("Widget", None)
        .aux("input", "1", Some("Widget"))
        .aux("bad", "input + TIME", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0, "should have at least one error");

        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let unit_infer_error = error_slice
            .iter()
            .find(|e| e.unit_error_kind == SimlinUnitErrorKind::Inference);

        assert!(
            unit_infer_error.is_some(),
            "should have an Inference unit error kind, got: {:?}",
            error_slice
                .iter()
                .map(|e| (e.code, e.kind, e.unit_error_kind))
                .collect::<Vec<_>>()
        );

        let infer_error = unit_infer_error.unwrap();
        assert_eq!(
            infer_error.kind,
            SimlinErrorKind::Units,
            "inference errors should have Units kind"
        );

        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_latex_equation() {
    let datamodel = TestProject::new("latex_test")
        .aux("test_var", "10 + 5 * 2", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut get_model_error: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut get_model_error);
        assert!(get_model_error.is_null());
        assert!(!model.is_null());

        // Get LaTeX for the variable
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let ident = CString::new("test_var").unwrap();
        let latex_ptr = simlin_model_get_latex_equation(model, ident.as_ptr(), &mut out_error);

        assert!(out_error.is_null(), "expected no error getting latex");
        assert!(!latex_ptr.is_null(), "expected non-null latex string");

        let latex = CStr::from_ptr(latex_ptr).to_str().unwrap();
        assert!(!latex.is_empty(), "latex should not be empty");
        // The LaTeX should contain the equation components
        assert!(latex.contains("10"), "latex should contain 10");

        simlin_free_string(latex_ptr);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_latex_equation_nonexistent_var() {
    let datamodel = TestProject::new("latex_nonexistent").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut get_model_error: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut get_model_error);
        assert!(get_model_error.is_null());
        assert!(!model.is_null());

        let mut out_error: *mut SimlinError = ptr::null_mut();
        let ident = CString::new("nonexistent").unwrap();
        let latex_ptr = simlin_model_get_latex_equation(model, ident.as_ptr(), &mut out_error);

        // Should return null for nonexistent variable
        assert!(latex_ptr.is_null(), "expected null for nonexistent var");

        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_latex_equation_null_ident() {
    let datamodel = TestProject::new("latex_null_ident").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut get_model_error: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut get_model_error);
        assert!(get_model_error.is_null());
        assert!(!model.is_null());

        let mut out_error: *mut SimlinError = ptr::null_mut();
        let latex_ptr = simlin_model_get_latex_equation(model, ptr::null(), &mut out_error);

        // Should return error for null ident
        assert!(!out_error.is_null(), "expected error for null ident");
        assert!(latex_ptr.is_null());

        simlin_error_free(out_error);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_latex_equation_null_model() {
    unsafe {
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let ident = CString::new("test_var").unwrap();
        let latex_ptr =
            simlin_model_get_latex_equation(ptr::null_mut(), ident.as_ptr(), &mut out_error);

        // Should return error for null model
        assert!(!out_error.is_null(), "expected error for null model");
        assert!(latex_ptr.is_null());

        // Verify error details
        let code = simlin_error_get_code(out_error);
        assert_eq!(code, SimlinErrorCode::Generic);

        simlin_error_free(out_error);
    }
}

#[test]
fn test_model_get_latex_equation_invalid_utf8() {
    let datamodel = TestProject::new("latex_invalid_utf8").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut get_model_error: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut get_model_error);
        assert!(get_model_error.is_null());
        assert!(!model.is_null());

        // Create invalid UTF-8 sequence: 0xFF is never valid in UTF-8
        let invalid_utf8: [u8; 4] = [0xFF, 0xFE, 0x00, 0x00];
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let latex_ptr = simlin_model_get_latex_equation(
            model,
            invalid_utf8.as_ptr() as *const c_char,
            &mut out_error,
        );

        // Should return error for invalid UTF-8
        assert!(!out_error.is_null(), "expected error for invalid UTF-8");
        assert!(latex_ptr.is_null());

        // Verify error message mentions UTF-8
        let msg_ptr = simlin_error_get_message(out_error);
        assert!(!msg_ptr.is_null());
        let msg = CStr::from_ptr(msg_ptr).to_str().unwrap();
        assert!(
            msg.contains("UTF-8"),
            "error message should mention UTF-8: {}",
            msg
        );

        simlin_error_free(out_error);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_latex_equation_module_var_no_ast() {
    // Module variables exist in the model but have no AST (they reference other models)
    // This tests the path where var.ast() returns None
    use engine::datamodel::{self, Dt, Equation, Project, SimMethod, SimSpecs, Visibility};

    // Create a project with two models: a child model and main model with a module
    let project = Project {
        name: "module_test".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 1.0,
            dt: Dt::Dt(1.0),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: Some("Month".to_string()),
        },
        dimensions: vec![],
        units: vec![],
        models: vec![
            // Child model that will be referenced as a module
            datamodel::Model {
                name: "child_model".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                    ident: "child_var".to_string(),
                    equation: Equation::Scalar("42".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
            // Main model with a module variable
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![datamodel::Variable::Module(datamodel::Module {
                    ident: "my_module".to_string(),
                    model_name: "child_model".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: vec![],
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
        ],
        source: Default::default(),
        ai_information: None,
    };

    let proj = open_project_from_datamodel(&project);

    unsafe {
        let mut get_model_error: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut get_model_error);
        assert!(get_model_error.is_null());
        assert!(!model.is_null());

        // "my_module" is a Module variable that exists but has no equation/AST
        let mut out_error: *mut SimlinError = ptr::null_mut();
        let ident = CString::new("my_module").unwrap();
        let latex_ptr = simlin_model_get_latex_equation(model, ident.as_ptr(), &mut out_error);

        // Should return null since modules have no AST
        assert!(out_error.is_null(), "should not have an error");
        assert!(
            latex_ptr.is_null(),
            "should return null for module with no AST"
        );

        simlin_model_unref(model);
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

    // Verify the project has unit warnings
    {
        let project_locked = unsafe { (*proj).project.lock().unwrap() };
        let model = project_locked
            .models
            .get(&engine::canonicalize("main"))
            .unwrap();
        assert!(
            model.unit_warnings.is_some(),
            "expected unit warnings in the model"
        );
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
        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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

    // Verify no unit warnings initially
    {
        let project_locked = unsafe { (*proj).project.lock().unwrap() };
        let model = project_locked
            .models
            .get(&engine::canonicalize("main"))
            .unwrap();
        assert!(
            model.unit_warnings.is_none(),
            "should not have unit warnings initially"
        );
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
        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
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
        let project_locked = (*proj).project.lock().unwrap();
        let model = project_locked.datamodel.get_model("main").unwrap();
        assert!(
            model.get_variable("bad_sum").is_some(),
            "patch should be applied"
        );
        drop(project_locked);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_simlin_malloc_alignment() {
    unsafe {
        let ptr = simlin_malloc(1);
        assert!(!ptr.is_null());
        assert_eq!((ptr as usize) % align_of::<c_double>(), 0);
        simlin_free(ptr);
    }
}

#[test]
fn test_model_get_links_rejects_nul() {
    let datamodel = TestProject::new("nul_links")
        .stock("stock", "0", &["bad\0flow"], &[], None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(!model.is_null());
        assert!(err.is_null());

        let mut out_error: *mut SimlinError = ptr::null_mut();
        let links = simlin_model_get_links(model, &mut out_error);
        assert!(links.is_null());
        assert!(!out_error.is_null());
        assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);

        let msg_ptr = simlin_error_get_message(out_error);
        assert!(!msg_ptr.is_null());
        let msg = CStr::from_ptr(msg_ptr).to_str().unwrap();
        assert!(msg.contains("NUL"));

        simlin_error_free(out_error);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_svg() {
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    if !xmile_path.exists() {
        panic!("missing SIR.stmx fixture");
    }
    let data = std::fs::read(xmile_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_xmile(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "project_open_xmile failed");
        assert!(!proj.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let model_name = CString::new("main").unwrap();
        simlin_project_render_svg(
            proj,
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "render_svg failed");
        assert!(!out_buffer.is_null());
        assert!(out_len > 0);

        let svg = std::str::from_utf8(std::slice::from_raw_parts(out_buffer, out_len)).unwrap();
        assert!(svg.starts_with("<svg "));
        assert!(svg.contains("viewBox="));
        assert!(svg.contains("</svg>"));

        simlin_free(out_buffer);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_svg_null_project() {
    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        let model_name = CString::new("main").unwrap();
        simlin_project_render_svg(
            ptr::null_mut(),
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);
        simlin_error_free(err);
    }
}

#[test]
fn test_render_svg_null_model_name() {
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    if !xmile_path.exists() {
        panic!("missing SIR.stmx fixture");
    }
    let data = std::fs::read(xmile_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_xmile(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert!(!proj.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        simlin_project_render_svg(
            proj,
            ptr::null(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);

        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_render_svg_nonexistent_model() {
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    if !xmile_path.exists() {
        panic!("missing SIR.stmx fixture");
    }
    let data = std::fs::read(xmile_path).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_xmile(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert!(!proj.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let model_name = CString::new("nonexistent_model").unwrap();
        simlin_project_render_svg(
            proj,
            model_name.as_ptr(),
            &mut out_buffer as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        assert!(out_buffer.is_null());
        assert_eq!(out_len, 0);

        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

/// Helper: create a project + model + sim from a TestProject datamodel.
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
    if !err.is_null() {
        let msg_ptr = simlin_error_get_message(err);
        let msg = if !msg_ptr.is_null() {
            CStr::from_ptr(msg_ptr).to_str().unwrap_or("")
        } else {
            ""
        };
        simlin_error_free(err);
        panic!("get_value('{}') failed: {}", name, msg);
    }
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
    if !err.is_null() {
        let msg_ptr = simlin_error_get_message(err);
        let msg = if !msg_ptr.is_null() {
            CStr::from_ptr(msg_ptr).to_str().unwrap_or("")
        } else {
            ""
        };
        simlin_error_free(err);
        panic!("run_to_end failed: {}", msg);
    }
}

/// Helper: reset the sim and assert success.
unsafe fn reset_sim(sim: *mut SimlinSim) {
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_reset(sim, &mut err as *mut *mut SimlinError);
    if !err.is_null() {
        let msg_ptr = simlin_error_get_message(err);
        let msg = if !msg_ptr.is_null() {
            CStr::from_ptr(msg_ptr).to_str().unwrap_or("")
        } else {
            ""
        };
        simlin_error_free(err);
        panic!("reset failed: {}", msg);
    }
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
    if !err.is_null() {
        let msg_ptr = simlin_error_get_message(err);
        let msg = if !msg_ptr.is_null() {
            CStr::from_ptr(msg_ptr).to_str().unwrap_or("")
        } else {
            ""
        };
        simlin_error_free(err);
        panic!("get_series('{}') failed: {}", name, msg);
    }
    buf.truncate(written);
    buf
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
fn test_libsimlin_override_survives_run_to_end() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Set override
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_override(sim, c_name.as_ptr(), 0.2, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "set_override failed");

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
fn test_libsimlin_set_override_when_vm_is_none() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Consume the VM
        run_to_end(sim);

        // Set override while VM is None
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_override(sim, c_name.as_ptr(), 0.3, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "set_override with no VM should succeed");

        // Reset creates a new VM with the override applied
        reset_sim(sim);
        run_to_end(sim);

        // Verify the override took effect by comparing against default
        let series_overridden = get_series_vec(sim, "population", 200);

        // Reset with no override to get baseline
        err = ptr::null_mut();
        simlin_sim_clear_overrides(sim, &mut err as *mut *mut SimlinError);
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
fn test_libsimlin_override_flows_through_dependents() {
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
        simlin_sim_set_override(
            sim,
            c_name.as_ptr(),
            20.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "set_override failed");

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
fn test_libsimlin_clear_overrides_restores_defaults() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Get default series
        run_to_end(sim);
        let series_default = get_series_vec(sim, "population", 200);

        // Override, reset, run
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_override(sim, c_name.as_ptr(), 0.5, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        reset_sim(sim);
        run_to_end(sim);
        let series_overridden = get_series_vec(sim, "population", 200);

        // Clear overrides, reset, run — should match default
        err = ptr::null_mut();
        simlin_sim_clear_overrides(sim, &mut err as *mut *mut SimlinError);
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
fn test_libsimlin_set_override_by_offset_validates_without_vm() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Look up the offset of "births" (a flow, not an initial variable)
        // while the VM still exists.
        let c_births = CString::new("births").unwrap();
        let mut flow_offset: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_get_offset(
            sim,
            c_births.as_ptr(),
            &mut flow_offset,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "get_offset for births should succeed");

        // Consume the VM so we exercise the no-VM validation path
        run_to_end(sim);

        // Out-of-bounds offset should fail
        err = ptr::null_mut();
        simlin_sim_set_override_by_offset(sim, 99999, 42.0, &mut err as *mut *mut SimlinError);
        assert!(
            !err.is_null(),
            "out-of-bounds offset should fail even without a VM"
        );
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadOverride);
        simlin_error_free(err);

        // In-bounds offset for a non-initial variable (flow) should also fail
        err = ptr::null_mut();
        simlin_sim_set_override_by_offset(
            sim,
            flow_offset,
            42.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "non-initial offset should fail even without a VM"
        );
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadOverride);
        simlin_error_free(err);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_multiple_reset_override_cycles() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);
        let c_name = CString::new("birth_rate").unwrap();

        let mut prev_final = 0.0;
        for i in 1..=10 {
            let rate = i as f64 * 0.02;
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_sim_set_override(
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
