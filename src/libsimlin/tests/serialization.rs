// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod common;

use std::ffi::CStr;
use std::ptr;

use prost::Message;
use serde_json::Value;
use simlin::*;
use simlin_engine::serde as engine_serde;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use common::open_project_from_datamodel;

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
            SimlinJsonFormat::Sdai as u32,
            false,
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
            SimlinJsonFormat::Sdai as u32,
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
        let project2_locked = (*proj2).datamodel.lock().unwrap();
        let roundtrip_datamodel = &project2_locked;
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
            SimlinJsonFormat::Native as u32,
            false,
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
            SimlinJsonFormat::Native as u32,
            false,
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
            SimlinJsonFormat::Native as u32,
            false,
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
            SimlinJsonFormat::Native as u32,
            false,
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
            SimlinJsonFormat::Sdai as u32,
            false,
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
            false,
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
fn test_project_serialize_json_native() {
    let datamodel = TestProject::new("json_native").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_json(
            proj,
            SimlinJsonFormat::Native as u32,
            false,
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error serializing json");
        assert!(!out_buffer.is_null(), "expected JSON buffer");

        let slice = std::slice::from_raw_parts(out_buffer, out_len);
        let json_str = std::str::from_utf8(slice).expect("valid utf-8 JSON");

        let actual: Value = serde_json::from_str(json_str).expect("parsed json");
        let expected_project: engine::json::Project = datamodel.clone().into();
        let expected = serde_json::to_value(expected_project).unwrap();

        assert_eq!(actual, expected);

        simlin_free(out_buffer);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_serialize_json_sdai() {
    let datamodel = TestProject::new("json_sdai").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_json(
            proj,
            SimlinJsonFormat::Sdai as u32,
            false,
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error serializing sdai");
        assert!(!out_buffer.is_null(), "expected SDAI JSON buffer");

        let slice = std::slice::from_raw_parts(out_buffer, out_len);
        let json_str = std::str::from_utf8(slice).expect("valid utf-8 SDAI JSON");

        let actual: Value = serde_json::from_str(json_str).expect("parsed json");
        let expected_model: engine::json_sdai::SdaiModel = datamodel.clone().into();
        let expected = serde_json::to_value(expected_model).unwrap();

        assert_eq!(actual, expected);

        simlin_free(out_buffer);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_export_xmile() {
    // Load a project from protobuf first
    let pb_path = std::path::Path::new("testdata/SIR_project.pb");
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

        // Export to XMILE
        let mut output: *mut u8 = std::ptr::null_mut();
        let mut output_len: usize = 0;
        err = ptr::null_mut();
        simlin_project_serialize_xmile(
            proj,
            &mut output as *mut *mut u8,
            &mut output_len as *mut usize,
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
                "project_serialize_xmile failed with error {:?}: {}",
                code, msg
            );
        }
        assert!(!output.is_null());
        assert!(output_len > 0);

        // Verify the output is valid XMILE by trying to parse it
        let xmile_data = std::slice::from_raw_parts(output, output_len);
        let xmile_str = std::str::from_utf8(xmile_data).unwrap();
        assert!(xmile_str.contains("<?xml"));
        assert!(xmile_str.contains("<xmile"));

        // Clean up
        simlin_free(output);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_export_null_project() {
    unsafe {
        let mut output: *mut u8 = std::ptr::null_mut();
        let mut output_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_xmile(
            std::ptr::null_mut(),
            &mut output as *mut *mut u8,
            &mut output_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "Expected an error but got success");
        simlin_error_free(err);
        assert!(output.is_null());
    }
}

#[test]
fn test_project_serialize() {
    // Create a project with some content
    let test_project = TestProject::new("test_serialize")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * death_rate", None)
        .aux("birth_rate", "0.02", None)
        .aux("death_rate", "0.01", None);

    let datamodel_project = test_project.build_datamodel();
    let original_pb = engine_serde::serialize(&datamodel_project).unwrap();

    let mut buf = Vec::new();
    original_pb.encode(&mut buf).unwrap();

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

        // Serialize it back out
        let mut output: *mut u8 = std::ptr::null_mut();
        let mut output_len: usize = 0;
        err = ptr::null_mut();
        simlin_project_serialize_protobuf(
            proj,
            &mut output as *mut *mut u8,
            &mut output_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert!(!output.is_null());
        assert!(output_len > 0);

        // Verify we can open the serialized project
        let proj2 = simlin_project_open_protobuf(output, output_len, &mut err);
        assert!(!proj2.is_null());
        // Get models and create simulations from both projects and verify they work identically
        let mut err_get_model1: *mut SimlinError = ptr::null_mut();
        let model1 = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model1 as *mut *mut SimlinError,
        );
        if !err_get_model1.is_null() {
            simlin_error_free(err_get_model1);
            panic!("get_model failed");
        }
        err = ptr::null_mut();
        let model2 =
            simlin_project_get_model(proj2, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(!model1.is_null());
        assert!(err.is_null());
        assert!(!model2.is_null());

        err = ptr::null_mut();
        let sim1 = simlin_sim_new(model1, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        err = ptr::null_mut();
        let sim2 = simlin_sim_new(model2, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim1.is_null());
        assert!(!sim2.is_null());

        // Run both simulations
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim1, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim2, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // Check they have same number of variables and steps
        let mut var_count1: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model1,
            0,
            ptr::null(),
            &mut var_count1 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        let mut var_count2: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model2,
            0,
            ptr::null(),
            &mut var_count2 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(var_count1, var_count2);

        let mut step_count1: usize = 0;
        err = ptr::null_mut();
        simlin_sim_get_stepcount(
            sim1,
            &mut step_count1 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        let mut step_count2: usize = 0;
        err = ptr::null_mut();
        simlin_sim_get_stepcount(
            sim2,
            &mut step_count2 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(step_count1, step_count2);

        // Clean up
        simlin_free(output);
        simlin_sim_unref(sim1);
        simlin_sim_unref(sim2);
        simlin_model_unref(model1);
        simlin_model_unref(model2);
        simlin_project_unref(proj);
        simlin_project_unref(proj2);
    }
}

#[test]
fn test_project_serialize_with_ltm() {
    // Create a project with a loop
    let test_project = TestProject::new("test_serialize_ltm")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("stock", "100", &["inflow"], &[], None)
        .flow("inflow", "stock * 0.1", None);

    let datamodel_project = test_project.build_datamodel();
    let original_pb = engine_serde::serialize(&datamodel_project).unwrap();

    let mut buf = Vec::new();
    original_pb.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        // LTM will be enabled when creating simulation

        // Serialize the project (should NOT include LTM variables)
        let mut output: *mut u8 = std::ptr::null_mut();
        let mut output_len: usize = 0;
        err = ptr::null_mut();
        simlin_project_serialize_protobuf(
            proj,
            &mut output as *mut *mut u8,
            &mut output_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        // Open the serialized project
        let proj2 = simlin_project_open_protobuf(output, output_len, &mut err);
        assert!(!proj2.is_null());

        // Create sims from both
        let mut err_get_model1: *mut SimlinError = ptr::null_mut();
        let model1 = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model1 as *mut *mut SimlinError,
        );
        if !err_get_model1.is_null() {
            simlin_error_free(err_get_model1);
            panic!("get_model failed");
        }
        err = ptr::null_mut();
        let model2 =
            simlin_project_get_model(proj2, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(!model1.is_null());
        assert!(err.is_null());
        assert!(!model2.is_null());

        err = ptr::null_mut();
        let sim1 = simlin_sim_new(model1, true, &mut err as *mut *mut SimlinError); // Has LTM
        assert!(err.is_null());
        err = ptr::null_mut();
        let sim2 = simlin_sim_new(model2, false, &mut err as *mut *mut SimlinError); // No LTM
        assert!(err.is_null());

        // Run both
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim1, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim2, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());

        // Both original models should have the same number of variables
        // (they're from the same serialized project without LTM augmentation)
        let mut var_count1: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model1,
            0,
            ptr::null(),
            &mut var_count1 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        let mut var_count2: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model2,
            0,
            ptr::null(),
            &mut var_count2 as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(
            var_count1, var_count2,
            "Models from serialized projects should have same variable count"
        );

        // Clean up
        simlin_free(output);
        simlin_sim_unref(sim1);
        simlin_sim_unref(sim2);
        simlin_model_unref(model1);
        simlin_model_unref(model2);
        simlin_project_unref(proj);
        simlin_project_unref(proj2);
    }
}

#[test]
fn test_project_serialize_null_safety() {
    unsafe {
        // Test with null project
        let mut output: *mut u8 = std::ptr::null_mut();
        let mut output_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_protobuf(
            ptr::null_mut(),
            &mut output as *mut *mut u8,
            &mut output_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        simlin_error_free(err);
        assert!(output.is_null());

        // Test with null output pointer
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

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        err = ptr::null_mut();
        simlin_project_serialize_protobuf(
            proj,
            ptr::null_mut(),
            &mut output_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        simlin_error_free(err);
        // Test with null output_len pointer
        err = ptr::null_mut();
        simlin_project_serialize_protobuf(
            proj,
            &mut output as *mut *mut u8,
            ptr::null_mut(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null());
        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}
