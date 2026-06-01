// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod common;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use prost::Message;
use serde_json::Value;
use simlin::*;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use common::open_project_from_datamodel;

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
    use engine::datamodel::{self, Compat, Dt, Equation, Project, SimMethod, SimSpecs};

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
                    equation: Equation::Scalar("42".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
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
                    ai_state: None,
                    uid: None,
                    compat: Compat::default(),
                })],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
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
fn test_model_get_var_json_stock() {
    let datamodel = TestProject::new("get_var_json")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .aux("birth_rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        assert!(!model.is_null());

        let var_name = CString::new("population").unwrap();
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_model_get_var_json(
            model,
            var_name.as_ptr(),
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(out_error.is_null(), "expected no error");
        assert!(!out_buffer.is_null());
        assert!(out_len > 0);

        let slice = std::slice::from_raw_parts(out_buffer, out_len);
        let json: Value = serde_json::from_slice(slice).expect("valid JSON");

        assert_eq!(json["type"], "stock");
        assert_eq!(json["name"], "population");
        assert_eq!(json["initialEquation"], "100");
        assert!(json["inflows"]
            .as_array()
            .unwrap()
            .contains(&Value::String("births".to_string())));
        assert!(json["outflows"]
            .as_array()
            .unwrap()
            .contains(&Value::String("deaths".to_string())));

        simlin_free(out_buffer);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_var_json_flow() {
    let datamodel = TestProject::new("get_var_json_flow")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * 0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());

        let var_name = CString::new("births").unwrap();
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_model_get_var_json(
            model,
            var_name.as_ptr(),
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(out_error.is_null());
        let slice = std::slice::from_raw_parts(out_buffer, out_len);
        let json: Value = serde_json::from_slice(slice).expect("valid JSON");

        assert_eq!(json["type"], "flow");
        assert_eq!(json["name"], "births");

        simlin_free(out_buffer);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_var_json_aux() {
    let datamodel = TestProject::new("get_var_json_aux")
        .aux("rate", "0.05", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());

        let var_name = CString::new("rate").unwrap();
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_model_get_var_json(
            model,
            var_name.as_ptr(),
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(out_error.is_null());
        let slice = std::slice::from_raw_parts(out_buffer, out_len);
        let json: Value = serde_json::from_slice(slice).expect("valid JSON");

        assert_eq!(json["type"], "aux");
        assert_eq!(json["name"], "rate");

        simlin_free(out_buffer);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_var_json_not_found() {
    let datamodel = TestProject::new("get_var_not_found")
        .aux("rate", "0.05", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());

        let var_name = CString::new("nonexistent").unwrap();
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_model_get_var_json(
            model,
            var_name.as_ptr(),
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(
            !out_error.is_null(),
            "expected error for nonexistent variable"
        );
        assert_eq!(
            simlin_error_get_code(out_error),
            SimlinErrorCode::DoesNotExist
        );
        assert!(out_buffer.is_null());

        simlin_error_free(out_error);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_var_json_null_var_name() {
    let datamodel = TestProject::new("get_var_null_name")
        .aux("rate", "0.05", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_model_get_var_json(
            model,
            ptr::null(),
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(!out_error.is_null(), "expected error for NULL var_name");
        simlin_error_free(out_error);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_var_names_with_type_mask() {
    let datamodel = TestProject::new("var_names_filtered")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .aux("birth_rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());

        // All variables (type_mask=0)
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(model, 0, ptr::null(), &mut count, &mut err);
        assert!(err.is_null());
        assert_eq!(count, 4, "expected 4 variables (1 stock + 2 flows + 1 aux)");

        let mut name_ptrs: Vec<*mut c_char> = vec![ptr::null_mut(); count];
        let mut written: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_names(
            model,
            0,
            ptr::null(),
            name_ptrs.as_mut_ptr(),
            count,
            &mut written,
            &mut err,
        );
        assert!(err.is_null());
        assert_eq!(written, 4);
        let all_names: Vec<String> = name_ptrs
            .iter()
            .map(|&p| {
                let s = CStr::from_ptr(p).to_string_lossy().into_owned();
                simlin_free_string(p);
                s
            })
            .collect();
        assert!(all_names.contains(&"population".to_string()));
        assert!(all_names.contains(&"births".to_string()));
        assert!(all_names.contains(&"deaths".to_string()));
        assert!(all_names.contains(&"birth_rate".to_string()));

        // Stocks only (type_mask=SIMLIN_VARTYPE_STOCK)
        count = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model,
            SIMLIN_VARTYPE_STOCK,
            ptr::null(),
            &mut count,
            &mut err,
        );
        assert!(err.is_null());
        assert_eq!(count, 1, "expected 1 stock");

        let mut stock_ptrs: Vec<*mut c_char> = vec![ptr::null_mut(); count];
        written = 0;
        err = ptr::null_mut();
        simlin_model_get_var_names(
            model,
            SIMLIN_VARTYPE_STOCK,
            ptr::null(),
            stock_ptrs.as_mut_ptr(),
            count,
            &mut written,
            &mut err,
        );
        assert!(err.is_null());
        assert_eq!(written, 1);
        let stock_name = CStr::from_ptr(stock_ptrs[0]).to_string_lossy().into_owned();
        simlin_free_string(stock_ptrs[0]);
        assert_eq!(stock_name, "population");

        // Flows only (type_mask=SIMLIN_VARTYPE_FLOW)
        count = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model,
            SIMLIN_VARTYPE_FLOW,
            ptr::null(),
            &mut count,
            &mut err,
        );
        assert!(err.is_null());
        assert_eq!(count, 2, "expected 2 flows");

        // Auxs only (type_mask=SIMLIN_VARTYPE_AUX)
        count = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(model, SIMLIN_VARTYPE_AUX, ptr::null(), &mut count, &mut err);
        assert!(err.is_null());
        assert_eq!(count, 1, "expected 1 aux");

        // Combined: stocks + flows
        count = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model,
            SIMLIN_VARTYPE_STOCK | SIMLIN_VARTYPE_FLOW,
            ptr::null(),
            &mut count,
            &mut err,
        );
        assert!(err.is_null());
        assert_eq!(count, 3, "expected 1 stock + 2 flows");

        // Substring filter
        let filter = CString::new("birth").unwrap();
        count = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(model, 0, filter.as_ptr(), &mut count, &mut err);
        assert!(err.is_null());
        assert_eq!(count, 2, "expected births + birth_rate");

        // Combined: type_mask + filter
        count = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model,
            SIMLIN_VARTYPE_FLOW,
            filter.as_ptr(),
            &mut count,
            &mut err,
        );
        assert!(err.is_null());
        assert_eq!(
            count, 1,
            "expected only flow 'births' matching filter 'birth'"
        );

        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_sim_specs_json() {
    let datamodel = TestProject::new("get_sim_specs_json")
        .aux("rate", "0.05", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());

        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_model_get_sim_specs_json(model, &mut out_buffer, &mut out_len, &mut out_error);

        assert!(out_error.is_null(), "expected no error");
        assert!(!out_buffer.is_null());

        let slice = std::slice::from_raw_parts(out_buffer, out_len);
        let sim_specs: Value = serde_json::from_slice(slice).expect("valid JSON");

        assert!(
            sim_specs["startTime"].is_number(),
            "startTime should be a number"
        );
        assert!(
            sim_specs["endTime"].is_number(),
            "endTime should be a number"
        );
        assert!(
            sim_specs["startTime"].as_f64().unwrap() < sim_specs["endTime"].as_f64().unwrap(),
            "startTime should be less than endTime"
        );

        simlin_free(out_buffer);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_var_json_case_insensitive() {
    let datamodel = TestProject::new("get_var_case")
        .stock("Population", "100", &[], &[], None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());

        let var_name = CString::new("population").unwrap();
        let mut out_buffer: *mut u8 = ptr::null_mut();
        let mut out_len: usize = 0;
        let mut out_error: *mut SimlinError = ptr::null_mut();

        simlin_model_get_var_json(
            model,
            var_name.as_ptr(),
            &mut out_buffer,
            &mut out_len,
            &mut out_error,
        );

        assert!(
            out_error.is_null(),
            "should find variable with different casing"
        );
        assert!(!out_buffer.is_null());

        let slice = std::slice::from_raw_parts(out_buffer, out_len);
        let json: Value = serde_json::from_slice(slice).expect("valid JSON");
        assert_eq!(json["type"], "stock");

        simlin_free(out_buffer);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_functions() {
    // Create a project with multiple models
    let project = engine::project_io::Project {
        name: "test_multi_model".to_string(),
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
        models: vec![
            engine::project_io::Model {
                name: "model1".to_string(),
                variables: vec![
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "var1".to_string(),
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
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                                compat: None,
                            },
                        )),
                    },
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "var2".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "var1 * 2".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
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
            },
            engine::project_io::Model {
                name: "model2".to_string(),
                variables: vec![
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Stock(
                            engine::project_io::variable::Stock {
                                ident: "stock".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "100".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                inflows: vec!["inflow".to_string()],
                                outflows: vec![],
                                non_negative: false,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                                compat: None,
                            },
                        )),
                    },
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Flow(
                            engine::project_io::variable::Flow {
                                ident: "inflow".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "rate * stock".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                non_negative: false,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                                compat: None,
                            },
                        )),
                    },
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "rate".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "0.1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
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
            },
        ],
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

        // Test simlin_project_get_model_count
        let mut model_count: usize = 0;
        err = ptr::null_mut();
        simlin_project_get_model_count(
            proj,
            &mut model_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(model_count, 2, "Should have 2 models");

        // Test simlin_project_get_model_names
        let mut model_names: Vec<*mut c_char> = vec![ptr::null_mut(); 2];
        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_project_get_model_names(
            proj,
            model_names.as_mut_ptr(),
            2,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(count, 2);

        let mut names = Vec::new();
        for name_ptr in &model_names {
            assert!(!name_ptr.is_null());
            let name = CStr::from_ptr(*name_ptr).to_string_lossy().into_owned();
            names.push(name.clone());
            simlin_free_string(*name_ptr);
        }
        assert!(names.contains(&"model1".to_string()));
        assert!(names.contains(&"model2".to_string()));

        // Test simlin_project_get_model with specific name
        let model1_name = CString::new("model1").unwrap();
        err = ptr::null_mut();
        let model1 = simlin_project_get_model(
            proj,
            model1_name.as_ptr(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(!model1.is_null());
        assert!(err.is_null());
        assert_eq!((*model1).model_name.as_str(), "model1");

        // Test simlin_project_get_model with null (should get first model)
        let mut err_get_model_default: *mut SimlinError = ptr::null_mut();
        let model_default = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model_default as *mut *mut SimlinError,
        );
        if !err_get_model_default.is_null() {
            simlin_error_free(err_get_model_default);
            panic!("get_model failed");
        }
        assert!(!model_default.is_null());
        assert_eq!((*model_default).model_name.as_str(), "model1");

        // Test simlin_project_get_model with non-existent name (should return error)
        let bad_name = CString::new("nonexistent").unwrap();
        err = ptr::null_mut();
        let model_fallback =
            simlin_project_get_model(proj, bad_name.as_ptr(), &mut err as *mut *mut SimlinError);
        assert!(model_fallback.is_null());
        assert!(!err.is_null());
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadModelName);
        simlin_error_free(err);

        // Test simlin_model_get_var_count
        let model2_name = CString::new("model2").unwrap();
        err = ptr::null_mut();
        let model2 = simlin_project_get_model(
            proj,
            model2_name.as_ptr(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(!model2.is_null());
        assert!(err.is_null());

        let mut var_count: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            model2,
            0,
            ptr::null(),
            &mut var_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert!(
            var_count >= 3,
            "model2 should have at least 3 variables (stock, inflow, rate)"
        );

        // Test simlin_model_get_var_names
        let mut var_names: Vec<*mut c_char> = vec![ptr::null_mut(); var_count];
        let mut written: usize = 0;
        err = ptr::null_mut();
        simlin_model_get_var_names(
            model2,
            0,
            ptr::null(),
            var_names.as_mut_ptr(),
            var_count,
            &mut written as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        assert_eq!(written, var_count);

        let mut var_name_strings = Vec::new();
        for name_ptr in &var_names {
            assert!(!name_ptr.is_null());
            let name = CStr::from_ptr(*name_ptr).to_string_lossy().into_owned();
            var_name_strings.push(name.clone());
            simlin_free_string(*name_ptr);
        }
        assert!(var_name_strings.contains(&"stock".to_string()));
        assert!(var_name_strings.contains(&"inflow".to_string()));
        assert!(var_name_strings.contains(&"rate".to_string()));
        // time may or may not be included depending on compilation

        // Test simlin_model_get_links
        err = ptr::null_mut();
        let links = simlin_model_get_links(model2, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!links.is_null());
        assert!((*links).count > 0, "Should have causal links");

        // Verify link structure
        let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);
        let mut found_rate_to_inflow = false;
        let mut found_stock_to_inflow = false;
        let mut found_inflow_to_stock = false;

        for link in links_slice {
            assert!(!link.from.is_null());
            assert!(!link.to.is_null());

            let from = CStr::from_ptr(link.from).to_str().unwrap();
            let to = CStr::from_ptr(link.to).to_str().unwrap();

            if from == "rate" && to == "inflow" {
                found_rate_to_inflow = true;
            }
            if from == "stock" && to == "inflow" {
                found_stock_to_inflow = true;
            }
            if from == "inflow" && to == "stock" {
                found_inflow_to_stock = true;
            }

            // Model-level links should not have scores
            assert!(link.score.is_null());
            assert_eq!(link.score_len, 0);
        }

        assert!(found_rate_to_inflow, "Should find rate -> inflow link");
        assert!(found_stock_to_inflow, "Should find stock -> inflow link");
        assert!(found_inflow_to_stock, "Should find inflow -> stock link");

        simlin_free_links(links);

        // Clean up
        simlin_model_unref(model1);
        simlin_model_unref(model2);
        simlin_model_unref(model_default);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_null_safety() {
    unsafe {
        // Test null project
        let mut count: usize = 0;
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_project_get_model_count(
            ptr::null_mut(),
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle null gracefully

        let mut names: [*mut c_char; 2] = [ptr::null_mut(); 2];
        let _written: usize = 0;
        err = ptr::null_mut();
        simlin_project_get_model_names(
            ptr::null_mut(),
            names.as_mut_ptr(),
            2,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle null gracefully

        err = ptr::null_mut();
        let model = simlin_project_get_model(
            ptr::null_mut(),
            ptr::null(),
            &mut err as *mut *mut SimlinError,
        );
        assert!(model.is_null());
        // err might be set for null input

        // Test null model
        simlin_model_ref(ptr::null_mut());
        simlin_model_unref(ptr::null_mut());

        count = 0;
        err = ptr::null_mut();
        simlin_model_get_var_count(
            ptr::null_mut(),
            0,
            ptr::null(),
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle null gracefully

        let mut var_names: [*mut c_char; 2] = [ptr::null_mut(); 2];
        err = ptr::null_mut();
        simlin_model_get_var_names(
            ptr::null_mut(),
            0,
            ptr::null(),
            var_names.as_mut_ptr(),
            2,
            &mut count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        // Should handle null gracefully

        err = ptr::null_mut();
        let links = simlin_model_get_links(ptr::null_mut(), &mut err as *mut *mut SimlinError);
        assert!(links.is_null());

        // Test null sim creation - should return error for NULL model
        err = ptr::null_mut();
        let sim = simlin_sim_new(ptr::null_mut(), false, &mut err as *mut *mut SimlinError);
        assert!(!err.is_null(), "Expected error for NULL model");
        assert!(sim.is_null());
        simlin_error_free(err);
    }
}

#[test]
fn test_model_get_links_reports_polarity() {
    // simlin_model_get_links must populate real link polarities from the
    // engine's static analysis (compute_link_polarities), not hard-code
    // Unknown: a positive constant multiplier gives a Positive link, and a
    // subtraction's right operand gives a Negative link.
    let datamodel = TestProject::new("polarity_test")
        .aux("input", "10", None)
        .aux("doubled", "input * 2", None)
        .aux("negated", "100 - input", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        assert!(!model.is_null());

        let links = simlin_model_get_links(model, &mut err);
        assert!(err.is_null());
        assert!(!links.is_null());

        let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);
        let mut doubled_polarity = None;
        let mut negated_polarity = None;
        for link in links_slice {
            let from = CStr::from_ptr(link.from).to_str().unwrap();
            let to = CStr::from_ptr(link.to).to_str().unwrap();
            if from == "input" && to == "doubled" {
                doubled_polarity = Some(link.polarity);
            }
            if from == "input" && to == "negated" {
                negated_polarity = Some(link.polarity);
            }
        }

        assert_eq!(
            doubled_polarity,
            Some(SimlinLinkPolarity::Positive),
            "input -> doubled (input * 2) must be a Positive link"
        );
        assert_eq!(
            negated_polarity,
            Some(SimlinLinkPolarity::Negative),
            "input -> negated (100 - input) must be a Negative link"
        );

        simlin_free_links(links);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_model_get_links_collapses_macro_internals() {
    // The model-level link view must match the sim-level default
    // (include_internal=false): macro/module-internal synthetic nodes
    // ($-prefixed) are collapsed into composite real-variable edges, so the
    // through-contribution of a SMOOTH is one `level -> smoothed` edge rather
    // than a chain through `$⁚smoothed⁚0⁚smth1`.
    let datamodel = TestProject::new("collapse_test")
        .aux("level", "TIME * 2", None)
        .aux("smoothed", "SMTH1(level, 5)", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        assert!(!model.is_null());

        let links = simlin_model_get_links(model, &mut err);
        assert!(err.is_null());
        assert!(!links.is_null());

        let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);
        let mut found_composite = false;
        for link in links_slice {
            let from = CStr::from_ptr(link.from).to_str().unwrap();
            let to = CStr::from_ptr(link.to).to_str().unwrap();
            assert!(
                !from.starts_with('$') && !to.starts_with('$'),
                "no synthetic node may appear in the collapsed link view: {from} -> {to}"
            );
            if from == "level" && to == "smoothed" {
                found_composite = true;
            }
        }
        assert!(
            found_composite,
            "the SMOOTH chain must collapse to a level -> smoothed composite edge"
        );

        simlin_free_links(links);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
