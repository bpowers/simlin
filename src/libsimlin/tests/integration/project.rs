// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::{CStr, CString};
use std::ptr;
use std::sync::atomic::Ordering;

use prost::Message;
use simlin::*;
use simlin_engine::serde as engine_serde;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use crate::common::{expect_error_code, expect_no_error, open_project_from_datamodel};

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
    // Load the SIR XMILE model. Hard failure, not a skip: a silently
    // skipped fixture turns this test into a no-op pass (GH #897).
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    let data = std::fs::read(xmile_path).expect("SIR.stmx fixture must exist");

    unsafe {
        // Import XMILE
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
        expect_no_error(err, "project_open_xmile");
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
        expect_no_error(err, "sim_run_to_end");

        // Check we have expected variables
        err = ptr::null_mut();
        let mut var_count: usize = 0;
        simlin_model_get_var_count(
            model,
            0,
            ptr::null(),
            &mut var_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "get_var_count");
        assert!(var_count > 0);

        // Clean up
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_import_mdl() {
    // Load the SIR MDL model (hard failure, not a skip -- GH #897).
    let mdl_path = std::path::Path::new("testdata/SIR.mdl");
    let data = std::fs::read(mdl_path).expect("SIR.mdl fixture must exist");

    unsafe {
        // Import MDL
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_vensim(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "project_open_vensim");
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
        expect_no_error(err, "sim_run_to_end");

        // Check we have expected variables
        err = ptr::null_mut();
        let mut var_count: usize = 0;
        simlin_model_get_var_count(
            model,
            0,
            ptr::null(),
            &mut var_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "get_var_count");
        assert!(var_count > 0);

        // Clean up
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_add_model() {
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
            macro_spec: None,
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
        expect_no_error(err, "project open");
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
        let new_model =
            simlin_project_get_model(proj, model_name.as_ptr(), &mut err as *mut *mut SimlinError);
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
                "dimensions": []
            }],
            "flows": [],
            "auxiliaries": [{
                "uid": 2,
                "name": "growth_rate",
                "equation": "0.1",
                "units": "",
                "documentation": "",
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
            SimlinJsonFormat::Native as u32,
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
        expect_no_error(err_get_model, "get_model");
        assert!(!model.is_null());

        // Verify variable count
        let mut var_count: usize = 0;
        let mut err_get_var_count: *mut SimlinError = ptr::null_mut();
        simlin_model_get_var_count(
            model,
            0,
            ptr::null(),
            &mut var_count as *mut usize,
            &mut err_get_var_count as *mut *mut SimlinError,
        );
        expect_no_error(err_get_var_count, "get_var_count");
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
            SimlinJsonFormat::Native as u32,
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
        let proj =
            simlin_project_open_json(ptr::null(), 0, SimlinJsonFormat::Native as u32, &mut err);

        assert!(proj.is_null());
        // assert_eq!(err, engine::ErrorCode::Generic as c_int);  // Obsolete assertion from old API
    }
}

#[test]
fn test_project_json_open_logistic_growth() {
    let json_bytes = include_bytes!("../../../../test/logistic-growth.sd.json");

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_json(
            json_bytes.as_ptr(),
            json_bytes.len(),
            SimlinJsonFormat::Native as u32,
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
            SimlinJsonFormat::Sdai as u32,
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
        expect_no_error(err_get_model, "get_model");
        assert!(!model.is_null());

        // Verify variable count (at least 4 variables, may include built-ins)
        let mut var_count: usize = 0;
        let mut err_get_var_count: *mut SimlinError = ptr::null_mut();
        simlin_model_get_var_count(
            model,
            0,
            ptr::null(),
            &mut var_count as *mut usize,
            &mut err_get_var_count as *mut *mut SimlinError,
        );
        expect_no_error(err_get_var_count, "get_var_count");
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
            SimlinJsonFormat::Sdai as u32,
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

/// F2 regression: `simlin_project_is_simulatable` must return true for a model
/// that contains a queue stock. Before the `build_sim` dispatch, the ordinary
/// `compile_project_incremental` path hit the `QueueNotExpanded` guard, so this
/// returned false for a model that `simlin_sim_new` simulates correctly.
#[test]
fn test_is_simulatable_queue_model() {
    let xml = include_str!("../../../../test/queues/queue_drain.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse queue_drain.xmile");
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let is_sim =
            simlin_project_is_simulatable(proj, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "should not have error");
        assert!(is_sim, "queue model must be simulatable");

        simlin_project_unref(proj);
    }
}

/// F2 regression twin for a conveyor stock.
#[test]
fn test_is_simulatable_conveyor_model() {
    let xml = include_str!("../../../../test/conveyors/minimal_conveyor.xmile");
    let datamodel = engine::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("parse minimal_conveyor.xmile");
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let is_sim =
            simlin_project_is_simulatable(proj, ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "should not have error");
        assert!(is_sim, "conveyor model must be simulatable");

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
fn test_get_model_null_name_returns_default() {
    let datamodel = TestProject::new("default_model")
        .stock("population", "100", &[], &[], None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null(), "null name should return default model");
        assert!(!model.is_null());

        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_get_model_valid_name() {
    let datamodel = TestProject::new("named_model")
        .stock("population", "100", &[], &[], None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let name = CString::new("main").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, name.as_ptr(), &mut err);
        assert!(err.is_null(), "exact model name should succeed");
        assert!(!model.is_null());

        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_get_model_canonical_name_match() {
    let mut datamodel = TestProject::new("canonical_test")
        .stock("population", "100", &[], &[], None)
        .build_datamodel();
    // Rename the model to have mixed case so we can test canonical matching
    datamodel.models[0].name = "My Model".to_string();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        // "my_model" is the canonical form of "My Model"
        let name = CString::new("my_model").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, name.as_ptr(), &mut err);
        assert!(err.is_null(), "canonical name variant should succeed");
        assert!(!model.is_null());

        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_get_model_nonexistent_name_returns_error() {
    let datamodel = TestProject::new("model_lookup")
        .stock("population", "100", &[], &[], None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let name = CString::new("nonexistent_model").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, name.as_ptr(), &mut err);
        assert!(model.is_null(), "nonexistent model name should return null");
        assert!(!err.is_null(), "should set error for missing model");
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadModelName);

        simlin_error_free(err);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_project_open_roundtrip() {
    // Create a project using TestProject, serialize to protobuf, open it,
    // and verify it loads correctly.
    let test_project = TestProject::new("roundtrip_test")
        .with_sim_time(0.0, 100.0, 0.25)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * 0.01", None)
        .aux("birth_rate", "0.02", None);

    // Build the datamodel and serialize to protobuf
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

        // Verify reference counting starts at 1
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

        // Verify we can access the project data through the mutex
        {
            let datamodel_locked = (*proj).datamodel.lock().unwrap();
            let dm = &*datamodel_locked;
            assert_eq!(dm.models.len(), 1);
            let model = &dm.models[0];
            assert_eq!(model.variables.len(), 4); // population, births, deaths, birth_rate
        }

        // Get the default model
        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        expect_no_error(err_get_model, "get_model");
        assert!(!model.is_null());
        // Model creation should increment project ref count
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);

        // Create a simulation
        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Run to completion
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "sim_run_to_end");

        // Verify time series
        let c_name = CString::new("population").unwrap();
        let mut step_count: usize = 0;
        err = ptr::null_mut();
        simlin_sim_get_stepcount(
            sim,
            &mut step_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert!(step_count > 0);
        let mut series = vec![0.0f64; step_count];
        let mut written: usize = 0;
        err = ptr::null_mut();
        simlin_sim_get_series(
            sim,
            c_name.as_ptr(),
            series.as_mut_ptr(),
            step_count,
            &mut written,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(written, step_count);
        // First value should be 100 (initial population)
        assert!((series[0] - 100.0).abs() < 1e-9);
        // Population should be growing (net birth rate > death rate: 0.02 > 0.01)
        assert!(*series.last().unwrap() > 100.0);

        // Clean up (reverse order of creation)
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_import_export_roundtrip() {
    // Load XMILE model (hard failure, not a skip -- GH #897).
    let xmile_path = std::path::Path::new("testdata/SIR.stmx");
    let data = std::fs::read(xmile_path).expect("SIR.stmx fixture must exist");

    unsafe {
        // Import XMILE
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj1 =
            simlin_project_open_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
        expect_no_error(err, "project_open_xmile");
        assert!(!proj1.is_null());

        // Export to XMILE
        let mut output: *mut u8 = std::ptr::null_mut();
        let mut output_len: usize = 0;
        err = ptr::null_mut();
        simlin_project_serialize_xmile(
            proj1,
            &mut output as *mut *mut u8,
            &mut output_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "project_serialize_xmile");

        // Import the exported XMILE
        err = ptr::null_mut();
        let proj2 =
            simlin_project_open_xmile(output, output_len, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "project_open_xmile (2nd)");
        assert!(!proj2.is_null());

        // Get models and verify both projects can simulate
        err = ptr::null_mut();
        let model1 =
            simlin_project_get_model(proj1, std::ptr::null(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        err = ptr::null_mut();
        let model2 =
            simlin_project_get_model(proj2, std::ptr::null(), &mut err as *mut *mut SimlinError);
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

        err = ptr::null_mut();
        simlin_sim_run_to_end(sim1, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "sim_run_to_end (1st)");

        err = ptr::null_mut();
        simlin_sim_run_to_end(sim2, &mut err as *mut *mut SimlinError);
        expect_no_error(err, "sim_run_to_end (2nd)");

        // Clean up
        simlin_sim_unref(sim1);
        simlin_sim_unref(sim2);
        simlin_model_unref(model1);
        simlin_model_unref(model2);
        simlin_free(output);
        simlin_project_unref(proj1);
        simlin_project_unref(proj2);
    }
}

#[test]
fn test_import_invalid_data() {
    unsafe {
        // Test with null data
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj =
            simlin_project_open_xmile(std::ptr::null(), 0, &mut err as *mut *mut SimlinError);
        assert!(proj.is_null());
        assert!(!err.is_null(), "Expected an error but got success");
        simlin_error_free(err);

        // Test with invalid XML
        let bad_data = b"not xml at all";
        err = ptr::null_mut();
        let proj = simlin_project_open_xmile(
            bad_data.as_ptr(),
            bad_data.len(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(proj.is_null());
        assert!(!err.is_null(), "Expected an error but got success");
        simlin_error_free(err);

        // Test with invalid MDL
        err = ptr::null_mut();
        let proj = simlin_project_open_vensim(
            bad_data.as_ptr(),
            bad_data.len(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(proj.is_null());
        assert!(!err.is_null(), "Expected an error but got success");
        simlin_error_free(err);
    }
}

#[test]
fn test_error_api_with_valid_project() {
    // Create a project with intentional errors
    let project = engine::project_io::Project {
        name: "test_errors".to_string(),
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
            variables: vec![
                // Variable with an equation error (unknown dependency)
                engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "error_var".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "unknown_var + 1".to_string(),
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
                },
                // Variable with bad units
                engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "bad_units_var".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "1".to_string(),
                                            initial_equation: None,
                                        },
                                    ),
                                ),
                            }),
                            documentation: String::new(),
                            units: "bad units here!!!".to_string(),
                            gf: None,
                            can_be_module_input: false,
                            visibility: engine::project_io::variable::Visibility::Private as i32,
                            uid: 0,
                            compat: None,
                        },
                    )),
                },
            ],
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

        // Test getting all errors
        let mut err_get_errors: *mut SimlinError = ptr::null_mut();
        let all_errors =
            simlin_project_get_errors(proj, &mut err_get_errors as *mut *mut SimlinError);
        assert!(err_get_errors.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0);

        // Verify we can access error details
        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let mut found_unknown_dep = false;
        let mut found_bad_units = false;

        for error in error_slice {
            if error.code == SimlinErrorCode::UnknownDependency {
                found_unknown_dep = true;
                assert!(!error.variable_name.is_null());
                let var_name = CStr::from_ptr(error.variable_name).to_str().unwrap();
                assert_eq!(var_name, "error_var");
            }
            // Bad units will show up as an error during parsing
            if !error.variable_name.is_null() {
                let var_name = CStr::from_ptr(error.variable_name).to_str().unwrap();
                if var_name == "bad_units_var" {
                    found_bad_units = true;
                }
            }
        }

        assert!(
            found_unknown_dep,
            "Should have found unknown dependency error"
        );
        assert!(found_bad_units, "Should have found bad units error");

        // Clean up
        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_api_with_compilation_errors() {
    // Create a project with compilation errors
    let project = engine::project_io::Project {
        name: "test_compilation_errors".to_string(),
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
            variables: vec![
                // This will cause a compilation error - circular reference
                engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "a".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "b + 1".to_string(),
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
                },
                engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "b".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "a + 1".to_string(),
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
                },
            ],
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
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        // The project should have compilation errors due to circular reference
        let mut err_get_errors: *mut SimlinError = ptr::null_mut();
        let all_errors =
            simlin_project_get_errors(proj, &mut err_get_errors as *mut *mut SimlinError);
        assert!(err_get_errors.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0);

        // Verify we found the compilation error
        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        let mut found_compilation_error = false;
        for error in error_slice {
            // Circular references or other compilation errors should be present
            if error.code == SimlinErrorCode::CircularDependency
                || error.code == SimlinErrorCode::BadModelName
                || error.code == SimlinErrorCode::Generic
            {
                found_compilation_error = true;
                break;
            }
        }
        assert!(
            found_compilation_error,
            "Should have found a compilation error"
        );

        // Clean up
        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_api_no_errors() {
    // Create a valid project with no errors
    let project = engine::project_io::Project {
        name: "test_no_errors".to_string(),
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
                        ident: "time_var".to_string(),
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
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        // Test that there are no errors (including compilation errors)
        err = ptr::null_mut();
        let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(all_errors.is_null());

        // Clean up
        simlin_project_unref(proj);
    }
}

#[test]
fn test_get_errors_repeated_calls_reuse_sync_state() {
    // Verifies that calling simlin_project_get_errors multiple times
    // reuses the persistent sync state rather than creating fresh
    // salsa inputs on every call (which would cause unbounded DB growth).
    let project = engine::project_io::Project {
        name: "repeated_errors".to_string(),
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
                        ident: "x".to_string(),
                        equation: Some(engine::project_io::variable::Equation {
                            equation: Some(
                                engine::project_io::variable::equation::Equation::Scalar(
                                    engine::project_io::variable::ScalarEquation {
                                        equation: "unknown_ref".to_string(),
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
    use prost::Message;
    project.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        // Call get_errors multiple times -- each should return the
        // same result and not grow the DB.
        for _ in 0..5 {
            err = ptr::null_mut();
            let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!all_errors.is_null(), "should have errors for unknown_ref");
            simlin_error_free(all_errors);
        }

        simlin_project_unref(proj);
    }
}

#[test]
fn test_error_api_null_safety() {
    unsafe {
        // Test with null project
        let mut err: *mut SimlinError = ptr::null_mut();
        let errors = simlin_project_get_errors(ptr::null_mut(), &mut err as *mut *mut SimlinError);
        assert!(errors.is_null());

        // Test free functions with null (should not crash)
        simlin_error_free(ptr::null_mut());
        simlin_error_free(ptr::null_mut());
    }
}

#[test]
fn test_error_offsets() {
    // Create a project with an error at a specific location
    let project = engine::project_io::Project {
        name: "test_offsets".to_string(),
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
                        ident: "var_with_offset_error".to_string(),
                        equation: Some(engine::project_io::variable::Equation {
                            equation: Some(
                                engine::project_io::variable::equation::Equation::Scalar(
                                    engine::project_io::variable::ScalarEquation {
                                        equation: "1 + unknown_var_here".to_string(),
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
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        let mut err_get_errors: *mut SimlinError = ptr::null_mut();
        let all_errors =
            simlin_project_get_errors(proj, &mut err_get_errors as *mut *mut SimlinError);
        assert!(err_get_errors.is_null());
        assert!(!all_errors.is_null());
        let count = simlin_error_get_detail_count(all_errors);
        assert!(count > 0);

        // Check that offsets are set (they should point to "unknown_var_here")
        let errors = simlin_error_get_details(all_errors);
        let error_slice = std::slice::from_raw_parts(errors, count);
        for error in error_slice {
            if error.code == SimlinErrorCode::UnknownDependency {
                // The offset should point to the unknown variable reference
                assert!(
                    error.start_offset > 0 || error.end_offset > 0,
                    "Error offsets should be set for unknown dependency"
                );
            }
        }

        // Clean up
        simlin_error_free(all_errors);
        simlin_project_unref(proj);
    }
}

/// Verify that loading a project with stdlib module references (hiring model)
/// includes the stdlib model definitions in the serialized JSON output.
/// This is the regression test for the bug where the TypeScript diagram
/// editor could not display or navigate into stdlib modules.
#[test]
fn test_stdlib_models_present_after_json_open() {
    let hiring_json = std::fs::read("../../test/hiring.sd.json").unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_json(
            hiring_json.as_ptr(),
            hiring_json.len(),
            0, // SimlinJsonFormat::Native
            &mut err as *mut *mut SimlinError,
        );
        assert!(!proj.is_null(), "failed to open hiring model");
        assert!(err.is_null(), "unexpected error opening hiring model");

        // Serialize back to JSON and check that stdlib models are included
        let mut out_buf: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        err = ptr::null_mut();
        simlin_project_serialize_json(
            proj,
            0,    // Native format
            true, // include stdlib models
            &mut out_buf as *mut *mut u8,
            &mut out_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "serialize failed");
        assert!(!out_buf.is_null());
        assert!(out_len > 0);

        let json_bytes = std::slice::from_raw_parts(out_buf, out_len);
        let json_str = std::str::from_utf8(json_bytes).unwrap();
        let json: serde_json::Value = serde_json::from_str(json_str).unwrap();

        let models = json["models"].as_array().unwrap();
        let model_names: Vec<&str> = models.iter().map(|m| m["name"].as_str().unwrap()).collect();

        // The hiring model references systems_rate, systems_conversion,
        // and systems_leak stdlib modules. All should be present.
        assert!(
            model_names.contains(&"stdlib\u{205A}systems_rate"),
            "stdlib systems_rate missing from serialized models: {:?}",
            model_names
        );
        assert!(
            model_names.contains(&"stdlib\u{205A}systems_conversion"),
            "stdlib systems_conversion missing: {:?}",
            model_names
        );
        assert!(
            model_names.contains(&"stdlib\u{205A}systems_leak"),
            "stdlib systems_leak missing: {:?}",
            model_names
        );

        // The in-memory datamodel should NOT include stdlib models (they
        // are only injected into the JSON serialization output).
        let mut model_count: usize = 0;
        err = ptr::null_mut();
        simlin_project_get_model_count(
            proj,
            &mut model_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(
            model_count, 1,
            "in-memory datamodel should only have 'main', not stdlib models"
        );

        // But the JSON output should have main + 3 stdlib models
        assert_eq!(
            model_names.len(),
            4,
            "JSON output should have main + 3 stdlib models: {:?}",
            model_names
        );

        simlin_free(out_buf);

        // Verify protobuf serialization does NOT include stdlib models
        // (protobuf is used for Firestore persistence).
        let mut pb_buf: *mut u8 = ptr::null_mut();
        let mut pb_len: usize = 0;
        err = ptr::null_mut();
        simlin_project_serialize_protobuf(
            proj,
            &mut pb_buf as *mut *mut u8,
            &mut pb_len as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "protobuf serialize failed");
        assert!(!pb_buf.is_null());

        let pb_bytes = std::slice::from_raw_parts(pb_buf, pb_len);
        let pb_project =
            engine::project_io::Project::decode(pb_bytes).expect("protobuf decode failed");
        let pb_model_names: Vec<&str> = pb_project.models.iter().map(|m| m.name.as_str()).collect();
        assert!(
            !pb_model_names.iter().any(|n| n.starts_with("stdlib")),
            "protobuf output should NOT contain stdlib models: {:?}",
            pb_model_names
        );

        simlin_free(pb_buf);
        simlin_project_unref(proj);
    }
}

// ── simlin_project_replace_contents ────────────────────────────────────

/// Sorted variable names of `model` via the public FFI (what a live handle
/// observes), asserting the query succeeded.
unsafe fn ffi_var_names(model: *mut SimlinModel) -> Vec<String> {
    let mut count: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_model_get_var_count(model, 0, ptr::null(), &mut count, &mut err);
    expect_no_error(err, "get_var_count");
    let mut names: Vec<*mut std::os::raw::c_char> = vec![ptr::null_mut(); count];
    let mut written: usize = 0;
    err = ptr::null_mut();
    simlin_model_get_var_names(
        model,
        0,
        ptr::null(),
        names.as_mut_ptr(),
        count,
        &mut written,
        &mut err,
    );
    expect_no_error(err, "get_var_names");
    let mut out: Vec<String> = names
        .into_iter()
        .take(written)
        .map(|p| {
            let s = CStr::from_ptr(p).to_str().unwrap().to_string();
            simlin_free_string(p);
            s
        })
        .collect();
    out.sort();
    out
}

unsafe fn replace_contents_ok(dst: *mut SimlinProject, src: *mut SimlinProject) {
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_project_replace_contents(dst, src, &mut err);
    expect_no_error(err, "replace_contents");
}

/// Sim step count via the FFI, asserting success.
unsafe fn ffi_stepcount(sim: *mut SimlinSim) -> usize {
    let mut steps: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_get_stepcount(sim, &mut steps, &mut err);
    expect_no_error(err, "get_stepcount");
    steps
}

fn population_project(stop: f64) -> engine::datamodel::Project {
    TestProject::new("population")
        .with_sim_time(0.0, stop, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * birth_rate", None)
        .aux("birth_rate", "0.02", None)
        .build_datamodel()
}

/// The core contract: a `SimlinModel` handle obtained BEFORE the replace
/// keeps working and observes the new contents (variables, sim specs), so a
/// host reloading a model file in place does not have to invalidate its
/// model objects. The source project is untouched and, once its own
/// refcount is dropped, the destination still holds an independent copy.
#[test]
fn test_replace_contents_live_model_handle_observes_new_contents() {
    let dst = open_project_from_datamodel(&population_project(10.0));
    let src_datamodel = TestProject::new("population_v2")
        .with_sim_time(0.0, 20.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * birth_rate", None)
        .flow("deaths", "population * death_rate", None)
        .aux("birth_rate", "0.02", None)
        .aux("death_rate", "0.01", None)
        .build_datamodel();
    let src = open_project_from_datamodel(&src_datamodel);

    unsafe {
        let src_snapshot = (*src).datamodel.lock().unwrap().clone();
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(dst, ptr::null(), &mut err);
        expect_no_error(err, "get_model");
        assert_eq!(
            ffi_var_names(model),
            vec!["birth_rate", "births", "population"]
        );

        let dst_refs_before = (*dst).ref_count.load(Ordering::SeqCst);
        let src_refs_before = (*src).ref_count.load(Ordering::SeqCst);

        replace_contents_ok(dst, src);

        // The same handle now sees the replacement's variables ...
        assert_eq!(
            ffi_var_names(model),
            vec!["birth_rate", "births", "death_rate", "deaths", "population"]
        );
        // ... and a simulation created through it compiles the replacement
        // (20 time units => 21 saved steps, vs 11 for the original).
        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err);
        expect_no_error(err, "sim_new after replace");
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err);
        expect_no_error(err, "run_to_end after replace");
        assert_eq!(ffi_stepcount(sim), 21);
        simlin_sim_unref(sim);

        // Replacing copies contents only: neither refcount moves, and the
        // source project is left exactly as it was.
        assert_eq!((*dst).ref_count.load(Ordering::SeqCst), dst_refs_before);
        assert_eq!((*src).ref_count.load(Ordering::SeqCst), src_refs_before);
        assert!(
            *(*src).datamodel.lock().unwrap() == src_snapshot,
            "source project must be left untouched"
        );

        // The copy is deep: freeing the source leaves the destination usable.
        simlin_project_unref(src);
        assert_eq!(
            ffi_var_names(model),
            vec!["birth_rate", "births", "death_rate", "deaths", "population"]
        );

        simlin_model_unref(model);
        simlin_project_unref(dst);
    }
}

/// `simlin_project_get_errors` on the destination reflects the replacement's
/// diagnostics: a clean project replaced by a broken one reports the error,
/// and replacing back clears it (the salsa db is re-synced, not just the
/// datamodel swapped).
#[test]
fn test_replace_contents_get_errors_reflects_new_contents() {
    let clean = population_project(10.0);
    let broken = TestProject::new("broken")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("a", "b + 1", None)
        .aux("b", "a + 1", None)
        .build_datamodel();
    let dst = open_project_from_datamodel(&clean);
    let broken_src = open_project_from_datamodel(&broken);
    let clean_src = open_project_from_datamodel(&clean);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let errors = simlin_project_get_errors(dst, &mut err);
        expect_no_error(err, "get_errors (clean)");
        assert!(errors.is_null(), "the population model has no errors");

        replace_contents_ok(dst, broken_src);
        err = ptr::null_mut();
        let errors = simlin_project_get_errors(dst, &mut err);
        expect_no_error(err, "get_errors (broken)");
        assert!(
            !errors.is_null(),
            "after replacing with a circular model, get_errors must report it"
        );
        let count = simlin_error_get_detail_count(errors);
        let details = std::slice::from_raw_parts(simlin_error_get_details(errors), count);
        assert!(
            details
                .iter()
                .any(|d| d.code == SimlinErrorCode::CircularDependency),
            "expected a CircularDependency detail"
        );
        simlin_error_free(errors);

        replace_contents_ok(dst, clean_src);
        err = ptr::null_mut();
        let errors = simlin_project_get_errors(dst, &mut err);
        expect_no_error(err, "get_errors (clean again)");
        assert!(
            errors.is_null(),
            "replacing back must clear the diagnostics"
        );

        simlin_project_unref(clean_src);
        simlin_project_unref(broken_src);
        simlin_project_unref(dst);
    }
}

/// A `SimlinModel` handle whose model name is absent from the replacement is
/// left dangling BY NAME only: every call through it returns a clear
/// `BadModelName` error (no crash, no stale data), and once a model of that
/// name reappears the same handle works again. Meanwhile the models the
/// replacement DOES contain are reachable through fresh handles.
#[test]
fn test_replace_contents_missing_model_handle_errors_cleanly() {
    let dst = open_project_from_datamodel(&population_project(10.0));
    let mut renamed = population_project(10.0);
    renamed.models[0].name = "other".to_string();
    let renamed_src = open_project_from_datamodel(&renamed);
    let original_src = open_project_from_datamodel(&population_project(10.0));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let main_handle = simlin_project_get_model(dst, ptr::null(), &mut err);
        expect_no_error(err, "get_model(main)");

        replace_contents_ok(dst, renamed_src);

        // Every query through the stale-by-name handle reports BadModelName.
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(main_handle, 0, ptr::null(), &mut count, &mut err);
        expect_error_code(err, SimlinErrorCode::BadModelName, "get_var_count");

        // `simlin_sim_new` defers a compile failure to the first run (its own
        // contract), so the missing model surfaces there, naming the model.
        err = ptr::null_mut();
        let sim = simlin_sim_new(main_handle, false, &mut err);
        expect_no_error(err, "sim_new (deferred compile failure)");
        assert!(!sim.is_null());
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err);
        assert!(
            !err.is_null(),
            "running a sim for a missing model must fail"
        );
        let code = simlin_error_get_code(err);
        let msg = CStr::from_ptr(simlin_error_get_message(err))
            .to_str()
            .unwrap()
            .to_string();
        simlin_error_free(err);
        assert_eq!(code, SimlinErrorCode::NotSimulatable);
        assert!(
            msg.contains("main"),
            "the error must name the missing model: {msg}"
        );
        simlin_sim_unref(sim);

        // The replacement's own model is reachable through a fresh handle.
        let other_name = CString::new("other").unwrap();
        err = ptr::null_mut();
        let other_handle = simlin_project_get_model(dst, other_name.as_ptr(), &mut err);
        expect_no_error(err, "get_model(other)");
        assert_eq!(
            ffi_var_names(other_handle),
            vec!["birth_rate", "births", "population"]
        );
        simlin_model_unref(other_handle);

        // Restoring a project that has "main" again revives the old handle.
        replace_contents_ok(dst, original_src);
        assert_eq!(
            ffi_var_names(main_handle),
            vec!["birth_rate", "births", "population"]
        );

        simlin_model_unref(main_handle);
        simlin_project_unref(original_src);
        simlin_project_unref(renamed_src);
        simlin_project_unref(dst);
    }
}

/// A `SimlinSim` created before the replace is a stale snapshot: it was
/// compiled from the old contents and keeps its results and its ability to
/// reset/re-run against that compiled program. Only a sim created AFTER the
/// replace reflects the new contents.
#[test]
fn test_replace_contents_existing_sim_is_stale_snapshot() {
    let dst = open_project_from_datamodel(&population_project(10.0));
    let src = open_project_from_datamodel(&population_project(20.0));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(dst, ptr::null(), &mut err);
        expect_no_error(err, "get_model");

        err = ptr::null_mut();
        let old_sim = simlin_sim_new(model, false, &mut err);
        expect_no_error(err, "sim_new (before)");
        err = ptr::null_mut();
        simlin_sim_run_to_end(old_sim, &mut err);
        expect_no_error(err, "run_to_end (before)");
        assert_eq!(ffi_stepcount(old_sim), 11);

        replace_contents_ok(dst, src);

        // Results already computed are untouched ...
        assert_eq!(ffi_stepcount(old_sim), 11);
        // ... and reset + re-run still executes the OLD compiled program.
        err = ptr::null_mut();
        simlin_sim_reset(old_sim, &mut err);
        expect_no_error(err, "reset (stale sim)");
        err = ptr::null_mut();
        simlin_sim_run_to_end(old_sim, &mut err);
        expect_no_error(err, "run_to_end (stale sim)");
        assert_eq!(ffi_stepcount(old_sim), 11);

        // A fresh sim from the same handle picks up the replacement.
        err = ptr::null_mut();
        let new_sim = simlin_sim_new(model, false, &mut err);
        expect_no_error(err, "sim_new (after)");
        err = ptr::null_mut();
        simlin_sim_run_to_end(new_sim, &mut err);
        expect_no_error(err, "run_to_end (after)");
        assert_eq!(ffi_stepcount(new_sim), 21);

        simlin_sim_unref(new_sim);
        simlin_sim_unref(old_sim);
        simlin_model_unref(model);
        simlin_project_unref(src);
        simlin_project_unref(dst);
    }
}

/// Replacing a project with itself is a permitted no-op re-sync (it must not
/// deadlock on the project's own locks), and both NULL positions reject with
/// `Generic` without touching anything.
#[test]
fn test_replace_contents_self_and_null_safety() {
    let proj = open_project_from_datamodel(&population_project(10.0));

    unsafe {
        let snapshot = (*proj).datamodel.lock().unwrap().clone();
        replace_contents_ok(proj, proj);
        assert!(*(*proj).datamodel.lock().unwrap() == snapshot);

        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_replace_contents(ptr::null_mut(), proj, &mut err);
        expect_error_code(err, SimlinErrorCode::Generic, "NULL dst");

        err = ptr::null_mut();
        simlin_project_replace_contents(proj, ptr::null(), &mut err);
        expect_error_code(err, SimlinErrorCode::Generic, "NULL src");
        assert!(*(*proj).datamodel.lock().unwrap() == snapshot);

        simlin_project_unref(proj);
    }
}
