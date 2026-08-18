// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::CStr;
use std::ptr;

use prost::Message;
use serde_json::Value;
use simlin::*;
use simlin_engine::serde as engine_serde;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use crate::common::{expect_error_code, expect_no_error, open_project_from_datamodel};

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

/// Every NULL-able pointer position on `simlin_project_serialize_json`, in
/// one place: the project handle and the two out-parameters each reject
/// with `Generic` rather than dereferencing. Mirrors the shape of
/// `test_project_serialize_null_safety` on the protobuf twin.
#[test]
fn test_serialize_json_null_safety() {
    let datamodel = TestProject::new("error_test").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;

        // NULL project.
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

        // NULL out_buffer.
        out_error = ptr::null_mut();
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

        // NULL out_len.
        out_error = ptr::null_mut();
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
    // Load a project from protobuf first (hard failure, not a skip -- GH #897).
    let pb_path = std::path::Path::new("testdata/SIR_project.pb");
    let data = std::fs::read(pb_path).expect("SIR_project.pb fixture must exist");

    unsafe {
        // Open project
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            data.as_ptr(),
            data.len(),
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "project open");
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
        expect_no_error(err, "project_serialize_xmile");
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
        expect_no_error(err, "project open");
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
        expect_no_error(err_get_model1, "get_model");
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
        expect_no_error(err_get_model1, "get_model");
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
                macro_spec: None,
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

// ── simlin_project_serialize_mdl ───────────────────────────────────────

/// Sorted variable idents of the named model, read straight off the
/// project's datamodel.
unsafe fn model_var_idents(proj: *mut SimlinProject, model_name: &str) -> Vec<String> {
    let dm = (*proj).datamodel.lock().unwrap();
    let model = dm
        .get_model(model_name)
        .unwrap_or_else(|| panic!("model '{model_name}' must exist"));
    let mut idents: Vec<String> = model
        .variables
        .iter()
        .map(|v| v.get_ident().to_string())
        .collect();
    idents.sort();
    idents
}

/// Number of elements in the named model's first stock-and-flow view (0 when
/// it has no view).
unsafe fn view_element_count(proj: *mut SimlinProject, model_name: &str) -> usize {
    let dm = (*proj).datamodel.lock().unwrap();
    let model = dm
        .get_model(model_name)
        .unwrap_or_else(|| panic!("model '{model_name}' must exist"));
    model
        .views
        .first()
        .map(|engine::datamodel::View::StockFlow(sf)| sf.elements.len())
        .unwrap_or(0)
}

/// Serialize `proj` to MDL, asserting no hard error, and return the text
/// plus the (possibly NULL) collected-warnings handle for the caller to
/// inspect and free.
unsafe fn serialize_mdl_ok(proj: *mut SimlinProject) -> (String, *mut SimlinError) {
    let mut out_buffer: *mut u8 = ptr::null_mut();
    let mut out_len: usize = 0;
    let mut collected: *mut SimlinError = ptr::null_mut();
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_project_serialize_mdl(
        proj,
        &mut out_buffer,
        &mut out_len,
        &mut collected,
        &mut err,
    );
    expect_no_error(err, "serialize_mdl");
    assert!(!out_buffer.is_null(), "expected an MDL buffer");
    assert!(out_len > 0, "expected non-empty MDL text");
    let text = std::str::from_utf8(std::slice::from_raw_parts(out_buffer, out_len))
        .expect("MDL output must be UTF-8")
        .to_string();
    simlin_free(out_buffer);
    (text, collected)
}

/// A Vensim model with a sketch survives open -> serialize_mdl -> open: the
/// same variables are present and the view elements are carried through the
/// sketch section, so a file-backed `.mdl` model keeps its diagram when
/// pysimlin writes it back in place.
#[test]
fn test_serialize_mdl_roundtrip_preserves_variables_and_sketch() {
    let data = std::fs::read("testdata/SIR.mdl").expect("SIR.mdl fixture must exist");

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_vensim(data.as_ptr(), data.len(), &mut err);
        expect_no_error(err, "open_vensim(SIR.mdl)");
        assert!(!proj.is_null());

        let original_vars = model_var_idents(proj, "main");
        let original_elements = view_element_count(proj, "main");
        assert!(
            original_elements > 0,
            "the SIR.mdl fixture must carry a sketch for this test to be meaningful"
        );

        let (mdl_text, collected) = serialize_mdl_ok(proj);
        assert!(
            collected.is_null(),
            "the SIR model uses no lossy constructs, so no export warnings are expected"
        );
        assert!(
            mdl_text.contains("Sketch information"),
            "MDL output must include a sketch section"
        );

        err = ptr::null_mut();
        let proj2 = simlin_project_open_vensim(mdl_text.as_ptr(), mdl_text.len(), &mut err);
        expect_no_error(err, "re-open serialized MDL");
        assert!(!proj2.is_null());

        assert_eq!(
            model_var_idents(proj2, "main"),
            original_vars,
            "round-tripped model must contain the same variables"
        );
        assert_eq!(
            view_element_count(proj2, "main"),
            original_elements,
            "round-tripped model must contain the same view elements"
        );

        simlin_project_unref(proj2);
        simlin_project_unref(proj);
    }
}

/// A construct MDL cannot represent losslessly (a non-negative stock) is
/// exported anyway, and the degradation is reported as a Warning-severity
/// detail on the collected-errors channel -- not as a hard error, so the
/// output buffer is still produced.
#[test]
fn test_serialize_mdl_reports_lossiness_warnings_non_fatally() {
    let datamodel = TestProject::new("mdl_warn")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock_with_options(
            "reservoir",
            "100",
            &["inflow"],
            &[],
            None,
            "",
            true,
            false,
            engine::datamodel::Visibility::Private,
            None,
        )
        .flow("inflow", "1", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let (mdl_text, collected) = serialize_mdl_ok(proj);
        assert!(
            mdl_text.contains("reservoir"),
            "export must still emit the degraded variable"
        );
        assert!(
            !collected.is_null(),
            "expected the dropped non-negative flag to be reported as a warning"
        );

        let count = simlin_error_get_detail_count(collected);
        assert_eq!(count, 1, "exactly one lossy construct in this model");
        let detail = &*simlin_error_get_detail(collected, 0);
        assert_eq!(detail.severity, SimlinErrorSeverity::Warning);
        assert_eq!(detail.kind, SimlinErrorKind::Model);
        assert_eq!(detail.code, SimlinErrorCode::Generic);
        let message = CStr::from_ptr(detail.message).to_str().unwrap();
        assert!(
            message.contains("reservoir") && message.contains("non-negative"),
            "warning must name the variable and the dropped construct: {message}"
        );
        let details = CStr::from_ptr(detail.details).to_str().unwrap();
        assert!(
            details.contains("reservoir"),
            "bare reason must name the variable: {details}"
        );
        // The aggregate's top-level code mirrors its first detail.
        assert_eq!(simlin_error_get_code(collected), SimlinErrorCode::Generic);
        simlin_error_free(collected);

        // A caller that passes NULL for the warnings channel still gets the text.
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_mdl(
            proj,
            &mut out_buffer,
            &mut out_len,
            ptr::null_mut(),
            &mut err,
        );
        expect_no_error(err, "serialize_mdl with NULL warnings channel");
        assert!(!out_buffer.is_null() && out_len > 0);
        simlin_free(out_buffer);

        simlin_project_unref(proj);
    }
}

/// A project MDL cannot express at all (two ordinary models) is a hard
/// failure: `out_error` is set, and no buffer or warnings handle is produced.
#[test]
fn test_serialize_mdl_hard_error_for_multi_model_project() {
    let mut datamodel = TestProject::new("mdl_multi")
        .aux("a", "1", None)
        .build_datamodel();
    datamodel.models.push(engine::datamodel::Model {
        name: "second".to_string(),
        sim_specs: None,
        variables: vec![],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut collected: *mut SimlinError = ptr::null_mut();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_mdl(
            proj,
            &mut out_buffer,
            &mut out_len,
            &mut collected,
            &mut err,
        );
        assert!(
            !err.is_null(),
            "expected a hard error for a two-model project"
        );
        let msg = CStr::from_ptr(simlin_error_get_message(err))
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            msg.contains("single model"),
            "error must explain the MDL single-model limit: {msg}"
        );
        simlin_error_free(err);
        assert!(out_buffer.is_null(), "no buffer on hard failure");
        assert_eq!(out_len, 0);
        assert!(collected.is_null(), "no warnings handle on hard failure");

        simlin_project_unref(proj);
    }
}

/// Every NULL-able pointer position on `simlin_project_serialize_mdl`: the
/// project handle and the two required out-parameters each reject with
/// `Generic` rather than dereferencing (`out_collected_errors` is optional
/// and covered by the warnings test above).
#[test]
fn test_serialize_mdl_null_safety() {
    let datamodel = TestProject::new("mdl_null").build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut collected: *mut SimlinError = ptr::null_mut();

        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_serialize_mdl(
            ptr::null_mut(),
            &mut out_buffer,
            &mut out_len,
            &mut collected,
            &mut err,
        );
        expect_error_code(err, SimlinErrorCode::Generic, "NULL project");
        assert!(collected.is_null());

        err = ptr::null_mut();
        simlin_project_serialize_mdl(
            proj,
            ptr::null_mut(),
            &mut out_len,
            &mut collected,
            &mut err,
        );
        expect_error_code(err, SimlinErrorCode::Generic, "NULL out_buffer");

        err = ptr::null_mut();
        simlin_project_serialize_mdl(
            proj,
            &mut out_buffer,
            ptr::null_mut(),
            &mut collected,
            &mut err,
        );
        expect_error_code(err, SimlinErrorCode::Generic, "NULL out_len");
        assert!(out_buffer.is_null());

        simlin_project_unref(proj);
    }
}
