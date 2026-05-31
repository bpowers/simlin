// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;

use prost::Message;
use simlin::*;
use simlin_engine::serde as engine_serde;
use simlin_engine::test_common::TestProject;

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
fn test_analyze_get_links() {
    // Create a project with a reinforcing loop using TestProject
    let test_project = TestProject::new("test_links")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * birth_rate", None)
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

        // Test without LTM enabled - should get structural links only
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

        err = ptr::null_mut();
        let links = simlin_analyze_get_links(sim, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!links.is_null());
        assert!((*links).count > 0, "Should have detected causal links");

        // Verify link structure
        let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);

        // Should have links like:
        // - birth_rate -> births
        // - population -> births
        // - births -> population
        let mut found_rate_to_births = false;
        let mut found_pop_to_births = false;
        let mut found_births_to_pop = false;

        for link in links_slice {
            assert!(!link.from.is_null());
            assert!(!link.to.is_null());

            let from = CStr::from_ptr(link.from).to_str().unwrap();
            let to = CStr::from_ptr(link.to).to_str().unwrap();

            if from == "birth_rate" && to == "births" {
                found_rate_to_births = true;
            }
            if from == "population" && to == "births" {
                found_pop_to_births = true;
            }
            if from == "births" && to == "population" {
                found_births_to_pop = true;
            }

            // Without LTM, scores should be null
            assert!(link.score.is_null(), "Score should be null without LTM");
            assert_eq!(link.score_len, 0, "Score length should be 0 without LTM");
        }

        assert!(
            found_rate_to_births,
            "Should find birth_rate -> births link"
        );
        assert!(found_pop_to_births, "Should find population -> births link");
        assert!(found_births_to_pop, "Should find births -> population link");

        simlin_free_links(links);

        // Now test with LTM enabled
        // Create new sim with LTM enabled
        let mut err_get_model_ltm: *mut SimlinError = ptr::null_mut();
        let model_ltm = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model_ltm as *mut *mut SimlinError,
        );
        if !err_get_model_ltm.is_null() {
            simlin_error_free(err_get_model_ltm);
            panic!("get_model failed");
        }
        assert!(!model_ltm.is_null());
        err = ptr::null_mut();
        let sim_ltm = simlin_sim_new(model_ltm, true, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim_ltm.is_null());

        // Run simulation to generate score data
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim_ltm, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // Get links with scores
        err = ptr::null_mut();
        let links_with_scores =
            simlin_analyze_get_links(sim_ltm, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!links_with_scores.is_null());
        assert!((*links_with_scores).count > 0);

        let links_slice =
            std::slice::from_raw_parts((*links_with_scores).links, (*links_with_scores).count);

        // Verify that scores are now populated
        for link in links_slice {
            let from = CStr::from_ptr(link.from).to_str().unwrap();
            let to = CStr::from_ptr(link.to).to_str().unwrap();

            // Links in the feedback loop should have scores
            if (from == "births" && to == "population") || (from == "population" && to == "births")
            {
                assert!(
                    !link.score.is_null(),
                    "Feedback loop links should have scores"
                );
                assert!(
                    link.score_len > 0,
                    "Score length should be > 0 for feedback links"
                );

                // All scores should be finite (initial timesteps are 0, not NaN)
                let scores = std::slice::from_raw_parts(link.score, link.score_len);
                for &score in scores {
                    assert!(score.is_finite(), "All scores should be finite");
                }
            }
        }

        simlin_free_links(links_with_scores);

        // Clean up
        simlin_sim_unref(sim);
        simlin_sim_unref(sim_ltm);
        simlin_model_unref(model);
        simlin_model_unref(model_ltm);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_analyze_get_links_no_loops() {
    // Create a project with no feedback loops
    let test_project = TestProject::new("test_no_loops")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("input", "10", None)
        .aux("output", "input * 2", None);

    // Build the datamodel and serialize to protobuf
    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project).unwrap();

    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
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

        err = ptr::null_mut();
        let links = simlin_analyze_get_links(sim, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!links.is_null());

        // Should still find the causal link from input to output
        assert!((*links).count > 0, "Should find input -> output link");

        let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);
        let mut found_link = false;
        for link in links_slice {
            let from = CStr::from_ptr(link.from).to_str().unwrap();
            let to = CStr::from_ptr(link.to).to_str().unwrap();

            if from == "input" && to == "output" {
                found_link = true;
                // input appears positively in "input * 2", so polarity is Positive
                assert_eq!(link.polarity, SimlinLinkPolarity::Positive);
            }
        }
        assert!(found_link, "Should find input -> output link");

        simlin_free_links(links);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_analyze_get_links_null_safety() {
    unsafe {
        // Test with null sim
        let mut err: *mut SimlinError = ptr::null_mut();
        let links = simlin_analyze_get_links(ptr::null_mut(), &mut err as *mut *mut SimlinError);
        assert!(links.is_null());

        // Test free with null (should not crash)
        simlin_free_links(ptr::null_mut());
    }
}

#[test]
fn test_analyze_get_relative_loop_score_renamed() {
    // Create a project with a reinforcing loop
    let test_project = TestProject::new("test_renamed")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * 0.02", None);

    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project).unwrap();

    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        // Create simulation with LTM enabled

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
        let sim = simlin_sim_new(model, true, &mut err as *mut *mut SimlinError); // Enable LTM for relative loop scores
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Run simulation
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // Get loops to find loop ID
        err = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!loops.is_null());
        assert!((*loops).count > 0);

        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let loop_id = CStr::from_ptr(loop_slice[0].id).to_str().unwrap();

        // Test renamed function
        let mut step_count: usize = 0;
        err = ptr::null_mut();
        simlin_sim_get_stepcount(
            sim,
            &mut step_count as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null());
        let mut scores = vec![0.0; step_count];

        let loop_id_c = CString::new(loop_id).unwrap();
        let mut written: usize = 0;
        err = ptr::null_mut();
        simlin_analyze_get_relative_loop_score(
            sim,
            loop_id_c.as_ptr(),
            scores.as_mut_ptr(),
            scores.len(),
            &mut written as *mut usize,
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            err.is_null(),
            "Should successfully get relative loop scores"
        );
        assert_eq!(written, scores.len());

        // No NaN values should be returned from the API
        for score in &scores {
            assert!(score.is_finite(), "Scores should never be NaN");
        }
        // Initial timesteps are 0 (no dynamics yet); subsequent ones are 1.0
        let nonzero: Vec<f64> = scores.iter().copied().filter(|s| *s != 0.0).collect();
        assert!(!nonzero.is_empty(), "Should have non-zero scores");
        for score in &nonzero {
            assert_eq!(*score, 1.0, "Single loop should have relative score of 1.0");
        }

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Helper: build a two-loops-in-one-partition project
/// (reinforcing births + balancing deaths on a shared population
/// stock) and run it with LTM enabled.  Returns an open sim, ready
/// for `simlin_analyze_get_relative_loop_score` calls.
unsafe fn setup_two_loop_sim() -> (*mut SimlinProject, *mut SimlinModel, *mut SimlinSim) {
    let test_project = TestProject::new("two_loop_partition")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None);

    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project).unwrap();
    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    let mut err: *mut SimlinError = ptr::null_mut();
    let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
    assert!(err.is_null());
    assert!(!proj.is_null());

    err = ptr::null_mut();
    let model = simlin_project_get_model(proj, ptr::null(), &mut err);
    assert!(err.is_null());
    assert!(!model.is_null());

    err = ptr::null_mut();
    let sim = simlin_sim_new(model, true, &mut err);
    assert!(err.is_null());
    assert!(!sim.is_null());

    err = ptr::null_mut();
    simlin_sim_run_to_end(sim, &mut err);
    assert!(err.is_null());

    (proj, model, sim)
}

/// Helper: fetch one loop's relative-loop-score series.
unsafe fn get_rel_score(sim: *mut SimlinSim, loop_id: &str) -> Vec<f64> {
    let mut err: *mut SimlinError = ptr::null_mut();
    let mut step_count: usize = 0;
    simlin_sim_get_stepcount(sim, &mut step_count as *mut usize, &mut err);
    assert!(err.is_null());

    let id_c = CString::new(loop_id).unwrap();
    let mut scores = vec![0.0_f64; step_count];
    let mut written: usize = 0;
    err = ptr::null_mut();
    simlin_analyze_get_relative_loop_score(
        sim,
        id_c.as_ptr(),
        scores.as_mut_ptr(),
        scores.len(),
        &mut written as *mut usize,
        &mut err,
    );
    assert!(err.is_null(), "expected rel score lookup to succeed");
    assert_eq!(written, scores.len());
    scores
}

/// Repeated FFI queries for the same loop must return the same
/// series bit-for-bit -- the cache must be stable across reads.
#[test]
fn test_rel_loop_score_cache_is_stable_across_calls() {
    unsafe {
        let (proj, model, sim) = setup_two_loop_sim();

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        assert!(!loops.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        assert!(
            (*loops).count >= 2,
            "two-flow stock must expose at least two loops"
        );
        let id_first = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();

        let first_call = get_rel_score(sim, &id_first);
        let second_call = get_rel_score(sim, &id_first);
        assert_eq!(
            first_call, second_call,
            "cached denominator must yield bit-identical output on repeat query"
        );

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Two loops in the same cycle partition must share a denominator
/// and produce |rel_r| + |rel_b| == 1.0 at every timestep (the
/// denominator is Σ|loop_score_j| across the partition by
/// construction).  If the cache were incorrectly keyed per-loop,
/// the two series would self-normalize independently and sum
/// closer to 2.0; the partition-keyed cache preserves the math.
#[test]
fn test_rel_loop_score_partition_sums_to_one_for_in_partition_loops() {
    unsafe {
        let (proj, model, sim) = setup_two_loop_sim();

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        assert_eq!(
            (*loops).count,
            2,
            "population + births + deaths should produce exactly two loops"
        );
        let id_a = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();
        let id_b = CStr::from_ptr(loop_slice[1].id)
            .to_str()
            .unwrap()
            .to_string();

        let score_a = get_rel_score(sim, &id_a);
        let score_b = get_rel_score(sim, &id_b);
        assert_eq!(score_a.len(), score_b.len());
        for t in 0..score_a.len() {
            // Once dynamics are nonzero (after the first step), the
            // absolute values must sum to 1.0.  SAFEDIV-0 means the
            // initial step can be 0 + 0.
            let magnitude_sum = score_a[t].abs() + score_b[t].abs();
            assert!(
                magnitude_sum < 1e-9 || (magnitude_sum - 1.0).abs() < 1e-9,
                "|rel_a| + |rel_b| at t={} should be 0 or 1, got {}",
                t,
                magnitude_sum
            );
        }

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// After `simlin_sim_reset`, the cache must not serve stale
/// denominators.  Without re-running, `results` is `None` and the
/// FFI should return a "no results" error rather than using a
/// cached vector tied to the previous run.
#[test]
fn test_rel_loop_score_cache_invalidated_on_reset() {
    unsafe {
        let (proj, model, sim) = setup_two_loop_sim();

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();

        // Prime the cache.
        let _ = get_rel_score(sim, &id);

        // Reset wipes results AND the cache.  The next query must
        // fail with "no results", proving the cached vector is not
        // being served against stale (now-absent) results.
        err = ptr::null_mut();
        simlin_sim_reset(sim, &mut err);
        assert!(err.is_null());

        let id_c = CString::new(id.as_str()).unwrap();
        let mut scores = vec![0.0_f64; 16];
        let mut written: usize = 0;
        err = ptr::null_mut();
        simlin_analyze_get_relative_loop_score(
            sim,
            id_c.as_ptr(),
            scores.as_mut_ptr(),
            scores.len(),
            &mut written as *mut usize,
            &mut err,
        );
        assert!(
            !err.is_null(),
            "after reset, relative loop score must report missing results"
        );
        simlin_error_free(err);

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// `simlin_analyze_get_relative_loop_score` must keep working against
/// results produced before a patch, even when the patch restructures
/// the project's loops.  The relative-score query reads the
/// `loop_partitions` snapshot captured on SimState at `sim_new` time,
/// so it is bound to the loop grouping the VM actually ran under.
/// Without the snapshot, the FFI would re-query
/// `model_ltm_variables` against the current DB -- which may have
/// different loop IDs (or none at all) after a rename / delete /
/// structural change -- and return `DoesNotExist` for IDs whose
/// series are still present in `state.results`.
#[test]
fn test_rel_loop_score_survives_post_sim_rename() {
    unsafe {
        let (proj, model, sim) = setup_two_loop_sim();

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();

        let before = get_rel_score(sim, &id);
        simlin_free_loops(loops);

        // Rename a variable that participates in both loops.  This
        // causes model_ltm_variables to re-run on the renamed
        // project; the snapshot on SimState is what keeps the
        // post-sim query answerable against the pre-patch results.
        let patch_json = br#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "renameVariable",
                            "payload": {"from": "population", "to": "pop_v2"}
                        }
                    ]
                }
            ]
        }"#;
        err = ptr::null_mut();
        let mut collected: *mut SimlinError = ptr::null_mut();
        simlin_project_apply_patch(
            proj,
            patch_json.as_ptr(),
            patch_json.len(),
            false,
            true,
            &mut collected,
            &mut err,
        );
        assert!(err.is_null(), "renameVariable patch must apply cleanly");
        assert!(collected.is_null());

        // The snapshot on SimState still carries the compilation-era
        // partition for this loop, so the query must succeed and
        // return the same series (the VM's results have not changed).
        let after = get_rel_score(sim, &id);
        assert_eq!(
            before, after,
            "rel_loop_score must read from the sim-time partition snapshot, \
             not the patched project's current loop mapping"
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// After a second `run_to_end` cycle (reset + run), the cache must
/// be repopulated against the new results, not reused from the
/// pre-reset state.  Re-running the same sim with the same inputs
/// produces identical results, so the post-reset series must match
/// the pre-reset series bit-for-bit.  A bug that served stale cache
/// after reset would either fail (empty cache + wrong denominator)
/// or silently return stale values.
#[test]
fn test_rel_loop_score_cache_repopulated_after_rerun() {
    unsafe {
        let (proj, model, sim) = setup_two_loop_sim();

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();

        let before = get_rel_score(sim, &id);

        // Reset and re-run: new results, cache cleared, same inputs.
        err = ptr::null_mut();
        simlin_sim_reset(sim, &mut err);
        assert!(err.is_null());
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err);
        assert!(err.is_null());

        let after = get_rel_score(sim, &id);
        assert_eq!(
            before, after,
            "re-running with identical inputs must reproduce the same series"
        );

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

// === Arrayed-loop FFI tests (issue #463) ===
//
// The fixture is a 2-region A2A model with heterogeneous per-region
// birth_rate (NYC=0.05, Boston=0.20).  This produces per-element
// distinct loop_scores so a bare arrayed ID returns argmax-abs (not
// slot 0) and subscripted access exposes specific elements.

fn build_arrayed_test_sim_protobuf() -> Vec<u8> {
    let test_project = TestProject::new("arrayed_ffi")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["NYC", "Boston"])
        .array_with_ranges(
            "birth_rate[Region]",
            vec![("NYC", "0.05"), ("Boston", "0.20")],
        )
        .array_stock("population[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "population * birth_rate", None);
    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project).unwrap();
    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();
    buf
}

unsafe fn open_arrayed_sim_with_ltm(
    buf: &[u8],
) -> (*mut SimlinProject, *mut SimlinModel, *mut SimlinSim) {
    let mut err: *mut SimlinError = ptr::null_mut();
    let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
    assert!(err.is_null());
    assert!(!proj.is_null());

    let mut err_get_model: *mut SimlinError = ptr::null_mut();
    let model = simlin_project_get_model(proj, ptr::null(), &mut err_get_model);
    assert!(err_get_model.is_null());
    assert!(!model.is_null());

    err = ptr::null_mut();
    let sim = simlin_sim_new(model, true, &mut err);
    assert!(err.is_null());
    assert!(!sim.is_null());

    err = ptr::null_mut();
    simlin_sim_run_to_end(sim, &mut err);
    assert!(err.is_null());

    (proj, model, sim)
}

unsafe fn read_relative_loop_series(
    sim: *mut SimlinSim,
    loop_id: &str,
) -> Result<Vec<f64>, (SimlinErrorCode, String)> {
    let mut step_count: usize = 0;
    let mut err: *mut SimlinError = ptr::null_mut();
    simlin_sim_get_stepcount(sim, &mut step_count, &mut err);
    assert!(err.is_null());

    let mut scores = vec![0.0_f64; step_count];
    let loop_id_c = CString::new(loop_id).unwrap();
    let mut written: usize = 0;
    err = ptr::null_mut();
    simlin_analyze_get_relative_loop_score(
        sim,
        loop_id_c.as_ptr(),
        scores.as_mut_ptr(),
        scores.len(),
        &mut written,
        &mut err,
    );
    if !err.is_null() {
        let code = simlin_error_get_code(err);
        let msg = simlin_error_get_message(err);
        let msg_str = if msg.is_null() {
            String::new()
        } else {
            CStr::from_ptr(msg).to_str().unwrap().to_string()
        };
        simlin_error_free(err);
        return Err((code, msg_str));
    }
    scores.truncate(written);
    Ok(scores)
}

#[test]
fn test_arrayed_bare_id_returns_argmax_abs_not_slot_zero() {
    let buf = build_arrayed_test_sim_protobuf();
    unsafe {
        let (proj, model, sim) = open_arrayed_sim_with_ltm(&buf);

        // Discover the loop id.
        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        assert!(!loops.is_null());
        assert!((*loops).count > 0);
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let loop_id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();

        // Bare ID on an arrayed loop should return argmax-abs across slots,
        // which for a single-loop partition is the slot whose
        // |loop_score| is largest.  With Boston's birth_rate = 0.20 vs
        // NYC's 0.05, Boston's loop_score has 4x the magnitude, so the
        // aggregator should pick Boston's signed value.
        let bare_series = read_relative_loop_series(sim, &loop_id).expect("bare access works");
        // For a single-loop partition, rel_loop_score = sign(loop_score) = ±1.
        // We expect mostly positive (reinforcing) values once dynamics start.
        let nonzero: Vec<f64> = bare_series.iter().copied().filter(|s| *s != 0.0).collect();
        assert!(!nonzero.is_empty(), "should have non-zero scores");
        for s in &nonzero {
            assert_eq!(
                (*s).abs(),
                1.0,
                "single-loop partition rel score should be ±1, got {s}"
            );
        }

        // Compare against subscripted access for NYC and Boston: both
        // should also return ±1 since they're each in their own
        // single-loop partition (no cross-element).
        let nyc_series = read_relative_loop_series(sim, &format!("{loop_id}[NYC]"))
            .expect("subscripted NYC access works");
        let boston_series = read_relative_loop_series(sim, &format!("{loop_id}[Boston]"))
            .expect("subscripted Boston access works");
        assert_eq!(nyc_series.len(), bare_series.len());
        assert_eq!(boston_series.len(), bare_series.len());

        // The bare argmax-abs path must equal ONE of the per-element
        // series at every step (whichever has larger |rel|).  Since
        // rel = ±1 for both elements here, ties go to slot 0 (NYC).
        for t in 0..bare_series.len() {
            let bare = bare_series[t];
            assert!(
                bare == nyc_series[t] || bare == boston_series[t],
                "bare aggregated series at step {t} must match one of the per-element series; \
                 got bare={bare}, nyc={}, boston={}",
                nyc_series[t],
                boston_series[t]
            );
        }

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_arrayed_case_insensitive_subscript() {
    let buf = build_arrayed_test_sim_protobuf();
    unsafe {
        let (proj, model, sim) = open_arrayed_sim_with_ltm(&buf);

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let loop_id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();

        let canonical = read_relative_loop_series(sim, &format!("{loop_id}[Boston]"))
            .expect("canonical case works");
        let lower =
            read_relative_loop_series(sim, &format!("{loop_id}[boston]")).expect("lowercase works");
        let upper =
            read_relative_loop_series(sim, &format!("{loop_id}[BOSTON]")).expect("uppercase works");
        let mixed = read_relative_loop_series(sim, &format!("{loop_id}[BoStOn]"))
            .expect("mixed case works");

        assert_eq!(canonical, lower);
        assert_eq!(canonical, upper);
        assert_eq!(canonical, mixed);

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_arrayed_errors_for_bad_subscripts() {
    let buf = build_arrayed_test_sim_protobuf();
    unsafe {
        let (proj, model, sim) = open_arrayed_sim_with_ltm(&buf);

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let loop_id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();

        // Unknown element name.
        let res = read_relative_loop_series(sim, &format!("{loop_id}[Tokyo]"));
        let (_, msg) = res.expect_err("unknown element should error");
        assert!(
            msg.contains("Tokyo") || msg.contains("tokyo"),
            "error should mention the bad element name: {msg}"
        );

        // Wrong dim count (too many subscripts).
        let res = read_relative_loop_series(sim, &format!("{loop_id}[NYC, 2]"));
        let (_, msg) = res.expect_err("dim count mismatch should error");
        assert!(
            msg.contains("dimension") || msg.contains("subscript"),
            "error should mention dimension/subscript: {msg}"
        );

        // Empty brackets.
        let res = read_relative_loop_series(sim, &format!("{loop_id}[]"));
        assert!(res.is_err(), "empty brackets should error");

        // Malformed.
        let res = read_relative_loop_series(sim, &format!("{loop_id}[NYC"));
        assert!(res.is_err(), "unclosed bracket should error");

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_get_loop_element_count_arrayed_vs_scalar() {
    // Arrayed: element_count == n_elements.
    let buf = build_arrayed_test_sim_protobuf();
    unsafe {
        let (proj, model, sim) = open_arrayed_sim_with_ltm(&buf);
        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let loop_id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();
        let loop_id_c = CString::new(loop_id.clone()).unwrap();

        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_analyze_get_loop_element_count(sim, loop_id_c.as_ptr(), &mut count, &mut err);
        assert!(err.is_null());
        assert_eq!(count, 2, "2-region arrayed loop has 2 elements");

        // Unknown loop -> 0 + error.
        let unknown = CString::new("nonexistent_loop").unwrap();
        let mut count2: usize = 999;
        err = ptr::null_mut();
        simlin_analyze_get_loop_element_count(sim, unknown.as_ptr(), &mut count2, &mut err);
        assert!(!err.is_null(), "unknown loop must error");
        let code = simlin_error_get_code(err);
        assert_eq!(code, SimlinErrorCode::DoesNotExist);
        simlin_error_free(err);

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }

    // Scalar: element_count == 1.
    let test_project = TestProject::new("scalar_for_count")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * 0.05", None);
    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project).unwrap();
    let mut buf2 = Vec::new();
    project.encode(&mut buf2).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf2.as_ptr(), buf2.len(), &mut err);
        assert!(err.is_null());
        let mut errm: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut errm);
        assert!(errm.is_null());
        err = ptr::null_mut();
        let sim = simlin_sim_new(model, true, &mut err);
        assert!(err.is_null());
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim, &mut err);
        assert!(err.is_null());

        err = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let loop_id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();
        let loop_id_c = CString::new(loop_id).unwrap();

        let mut count: usize = 0;
        err = ptr::null_mut();
        simlin_analyze_get_loop_element_count(sim, loop_id_c.as_ptr(), &mut count, &mut err);
        assert!(err.is_null());
        assert_eq!(count, 1, "scalar loop has element count 1");

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_subscripted_loop_id_uses_per_element_cache() {
    // Regression-style: repeated subscripted calls on the same
    // (partition, element) must return identical numbers (cache hit
    // doesn't drift).  Indirectly validates the cache key change.
    let buf = build_arrayed_test_sim_protobuf();
    unsafe {
        let (proj, model, sim) = open_arrayed_sim_with_ltm(&buf);

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        let loop_id = CStr::from_ptr(loop_slice[0].id)
            .to_str()
            .unwrap()
            .to_string();

        let first = read_relative_loop_series(sim, &format!("{loop_id}[NYC]")).unwrap();
        let second = read_relative_loop_series(sim, &format!("{loop_id}[NYC]")).unwrap();
        assert_eq!(first, second);

        // Different element should produce a possibly-different series
        // (or the same -- both are ±1 in this single-loop partition --
        // but the dispatch path must produce a value either way).
        let boston = read_relative_loop_series(sim, &format!("{loop_id}[Boston]")).unwrap();
        assert_eq!(boston.len(), first.len());

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

// === Two-A2A-subsystem round-trip (GH #487, AC2.5) ===
//
// Two disconnected reinforcing apply-to-all feedback loops over *different*
// dimensions: `pop[Region] -> births[Region] = pop[Region]*0.1 -> pop[Region]`
// over `Region = {a, b, c}`, and `widgets[Product] -> production[Product] =
// widgets[Product]*0.05 -> widgets[Product]` over `Product = {x, y}`.  Each
// element of each loop is its own cycle partition (element-wise uncoupled), so
// every per-element relative loop score is +1 (a single-loop partition).  Before
// the partition-correctness fix the two A2A loops both fell into the catch-all
// `None` cohort and cross-normalized to a pooled value (each ~0.5); the per-slot
// `loop_partitions` keep them separate.  This test confirms the FFI exposes the
// per-slot data correctly (the subscripted-loop-id accessor) and that what it
// returns matches the engine's `compute_rel_loop_scores_per_element` exactly --
// i.e. a round trip through the C API preserves the per-slot partitions.

/// Build the two-A2A-subsystem datamodel project.
fn two_a2a_subsystems_project() -> simlin_engine::datamodel::Project {
    TestProject::new("two_a2a")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Product", &["x", "y"])
        .array_stock("pop[Region]", "100", &["births"], &[], None)
        .array_flow("births[Region]", "pop * 0.1", None)
        .array_stock("widgets[Product]", "50", &["production"], &[], None)
        .array_flow("production[Product]", "widgets * 0.05", None)
        .build_datamodel()
}

/// Compute the engine's reference per-element relative loop scores for `project`
/// (the same path `simlin_analyze_get_relative_loop_score` re-derives from its
/// snapshots): compile with LTM, run the VM, then call the production
/// post-simulation normalizer.  Returns `(rel_per_element, n_slots_by_loop)`.
fn engine_reference_rel_per_element(
    project: &simlin_engine::datamodel::Project,
) -> (
    std::collections::HashMap<String, Vec<f64>>,
    std::collections::HashMap<String, usize>,
) {
    use simlin_engine::db::{
        compile_project_incremental, model_ltm_variables, set_project_ltm_enabled,
        sync_from_datamodel_incremental, SimlinDb,
    };
    use simlin_engine::Vm;

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main").unwrap();
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    let loop_partitions = ltm_vars.loop_partitions.clone();
    let n_slots_by_loop: std::collections::HashMap<String, usize> = loop_partitions
        .iter()
        .map(|(id, pv)| (id.clone(), pv.len().max(1)))
        .collect();

    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    let results = vm.into_results();

    let rel =
        simlin_engine::ltm_post::compute_rel_loop_scores_per_element(&results, &loop_partitions);
    (rel, n_slots_by_loop)
}

#[test]
fn test_two_a2a_subsystems_per_slot_rel_score_round_trips() {
    let project = two_a2a_subsystems_project();
    let (engine_rel, engine_n_slots) = engine_reference_rel_per_element(&project);
    assert_eq!(
        engine_rel.len(),
        2,
        "two disconnected A2A subsystems should produce exactly two loop_score series"
    );

    let pb = engine_serde::serialize(&project).unwrap();
    let mut buf = Vec::new();
    pb.encode(&mut buf).unwrap();

    unsafe {
        let (proj, model, sim) = open_arrayed_sim_with_ltm(&buf);

        let mut err: *mut SimlinError = ptr::null_mut();
        let loops = simlin_analyze_get_loops(model, &mut err);
        assert!(err.is_null());
        assert!(!loops.is_null());
        let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
        assert_eq!(
            (*loops).count,
            2,
            "two disconnected A2A subsystems => two detected loops"
        );

        let mut step_count: usize = 0;
        simlin_sim_get_stepcount(sim, &mut step_count, &mut err);
        assert!(err.is_null());

        // The two loops live over different dimensions, so their element
        // counts differ; collect (id, n_elements) for each.
        let mut loop_ids: Vec<(String, usize)> = Vec::new();
        for l in loop_slice {
            let id = CStr::from_ptr(l.id).to_str().unwrap().to_string();
            let id_c = CString::new(id.clone()).unwrap();
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_analyze_get_loop_element_count(sim, id_c.as_ptr(), &mut count, &mut err);
            assert!(err.is_null(), "loop element count must succeed for {id}");
            assert!(count >= 1);
            loop_ids.push((id, count));
        }
        // One loop over Region (3 elements), one over Product (2).
        let mut counts: Vec<usize> = loop_ids.iter().map(|(_, n)| *n).collect();
        counts.sort_unstable();
        assert_eq!(counts, vec![2, 3], "expected element counts 2 and 3");

        let region_names = ["a", "b", "c"];
        let product_names = ["x", "y"];
        for (loop_id, n_elements) in &loop_ids {
            let engine_series = engine_rel
                .get(loop_id)
                .unwrap_or_else(|| panic!("engine reference missing loop {loop_id}"));
            let engine_stride = engine_series.len() / step_count;
            // For a pure-A2A loop in its own partition the engine stride is
            // exactly the loop's slot count.
            assert_eq!(*engine_n_slots.get(loop_id).unwrap(), *n_elements);
            assert_eq!(engine_stride, *n_elements);

            // The FFI bare-arrayed-id form is the argmax-abs aggregator over the
            // loop's slots; since every per-element score is +1 here, it must be
            // +1 at every nonzero step too.
            let bare = read_relative_loop_series(sim, loop_id).expect("bare access works");
            assert_eq!(bare.len(), step_count);
            for v in &bare {
                assert!(
                    *v == 0.0 || *v == 1.0,
                    "bare aggregator should be 0 or 1, got {v}"
                );
            }

            // The FFI subscripted form must reproduce the engine's per-element
            // series bit-for-bit.
            let elem_names: &[&str] = if *n_elements == 3 {
                &region_names
            } else {
                &product_names
            };
            for (elem_idx, elem_name) in elem_names.iter().enumerate() {
                let ffi_series = read_relative_loop_series(sim, &format!("{loop_id}[{elem_name}]"))
                    .unwrap_or_else(|(c, m)| {
                        panic!("subscripted access {loop_id}[{elem_name}] failed: {c:?} {m}")
                    });
                assert_eq!(ffi_series.len(), step_count);
                for (s, &ffi_v) in ffi_series.iter().enumerate() {
                    let engine_v = engine_series[s * engine_stride + elem_idx];
                    assert_eq!(
                        ffi_v, engine_v,
                        "FFI rel-loop-score for {loop_id}[{elem_name}] at step {s} must match \
                         compute_rel_loop_scores_per_element ({ffi_v} vs {engine_v})"
                    );
                    // AC2.1: each loop is alone in its (per-element) partition,
                    // so the score is +1 once dynamics are nonzero -- NOT the
                    // pre-fix pooled ~0.5 the two loops would share if they
                    // cross-normalized.
                    if ffi_v != 0.0 {
                        assert_eq!(
                            ffi_v, 1.0,
                            "single-loop-per-partition rel score for {loop_id}[{elem_name}] at \
                             step {s} should be 1.0, not pooled"
                        );
                    }
                }
            }
        }

        simlin_free_loops(loops);
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Build a tiny reinforcing-loop project (population grows proportionally to
/// itself) and open it via the protobuf FFI, returning `(project, model)`.
///
/// population[t] is a stock fed by `births = population * growth_rate`, so the
/// strongest-path discovery finds a single reinforcing loop
/// `population -> births -> population` with a non-trivial importance series.
unsafe fn open_reinforcing_loop_model() -> (*mut SimlinProject, *mut SimlinModel) {
    let test_project = TestProject::new("main")
        .with_sim_time(0.0, 20.0, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * growth_rate", None)
        .aux("growth_rate", "0.1", None);

    let datamodel_project = test_project.build_datamodel();
    let project = engine_serde::serialize(&datamodel_project).unwrap();
    let mut buf = Vec::new();
    project.encode(&mut buf).unwrap();

    let mut err: *mut SimlinError = ptr::null_mut();
    let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
    assert!(err.is_null(), "project open should not error");
    assert!(!proj.is_null());

    let mut err_model: *mut SimlinError = ptr::null_mut();
    let model = simlin_project_get_model(proj, ptr::null(), &mut err_model);
    assert!(err_model.is_null(), "get_model should not error");
    assert!(!model.is_null());

    (proj, model)
}

/// Collect the C string array at `(ptr, count)` into owned Rust strings.
unsafe fn c_string_array(ptr: *mut *mut c_char, count: usize) -> Vec<String> {
    if ptr.is_null() || count == 0 {
        return Vec::new();
    }
    let slice = std::slice::from_raw_parts(ptr, count);
    slice
        .iter()
        .map(|&p| {
            assert!(!p.is_null(), "string entry must not be NULL");
            CStr::from_ptr(p).to_string_lossy().into_owned()
        })
        .collect()
}

#[test]
fn discover_loops_returns_loops_periods_and_importance() {
    unsafe {
        let (proj, model) = open_reinforcing_loop_model();

        let mut err: *mut SimlinError = ptr::null_mut();
        // budget_ms = 0 => unlimited; the model is tiny so this completes.
        let result = simlin_analyze_discover_loops(model, 0, &mut err);
        assert!(err.is_null(), "discovery should not error");
        assert!(!result.is_null(), "discovery result must not be NULL");

        let res = &*result;
        assert!(
            !res.truncated,
            "an unbudgeted run on a tiny model must not be truncated"
        );
        assert!(
            res.loop_count > 0,
            "discovery should find at least one loop in a reinforcing model"
        );

        // Each discovered loop carries an id, a closed variable chain, and a
        // per-step importance series.
        let loops = std::slice::from_raw_parts(res.loops, res.loop_count);
        for lp in loops {
            let id = CStr::from_ptr(lp.id).to_string_lossy().into_owned();
            assert!(!id.is_empty(), "loop id must not be empty");
            assert!(
                lp.var_count >= 2,
                "loop {id} must have at least two variables in its chain"
            );
            let vars = c_string_array(lp.variables, lp.var_count);
            assert!(
                vars.iter().any(|v| v == "population"),
                "loop {id} should include population, got {vars:?}"
            );
            assert!(
                lp.importance_len > 0,
                "loop {id} must have a non-empty importance series"
            );
            let importance = std::slice::from_raw_parts(lp.importance, lp.importance_len);
            assert!(
                importance.iter().all(|v| v.is_finite()),
                "loop {id} importance series must be finite"
            );
        }

        // Dominant periods cover the simulation with valid bounds.
        assert!(
            res.period_count > 0,
            "a model with loops should produce at least one dominant period"
        );
        let periods = std::slice::from_raw_parts(res.periods, res.period_count);
        for p in periods {
            assert!(p.start <= p.end, "period start must not exceed its end");
            let names = c_string_array(p.dominant_loops, p.dominant_loop_count);
            assert!(
                !names.is_empty(),
                "a dominant period must name at least one loop"
            );
        }

        simlin_free_discovery_result(result);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn discover_loops_tiny_budget_truncates() {
    unsafe {
        // A goal-seeking (balancing) model over many saved timesteps. Values
        // stay bounded (population converges toward the goal), and the large
        // step count makes the per-timestep discovery sweep reliably take well
        // over a millisecond -- so a 1ms budget trips the per-step elapsed
        // check and reports truncation. The budget is checked at the top of
        // each step, so the sweep stops promptly rather than hanging.
        let test_project = TestProject::new("main")
            .with_sim_time(0.0, 200_000.0, 1.0)
            .stock("population", "10", &["adjustment"], &[], None)
            .flow("adjustment", "(goal - population) * 0.1", None)
            .aux("goal", "1000", None);
        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project).unwrap();
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(err.is_null());
        assert!(!proj.is_null());
        let mut err_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err_model);
        assert!(err_model.is_null());
        assert!(!model.is_null());

        err = ptr::null_mut();
        let result = simlin_analyze_discover_loops(model, 1, &mut err);
        assert!(
            err.is_null(),
            "discovery should not error even when truncated"
        );
        assert!(!result.is_null());

        let res = &*result;
        assert!(
            res.truncated,
            "a 1ms budget on a 200k-step sweep must report truncated discovery"
        );

        simlin_free_discovery_result(result);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn discover_loops_null_model_errors_without_panic() {
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let result = simlin_analyze_discover_loops(ptr::null_mut(), 0, &mut err);
        assert!(result.is_null(), "null model must yield a null result");
        assert!(!err.is_null(), "null model must surface an error");
        simlin_error_free(err);

        // Freeing a null result is a no-op.
        simlin_free_discovery_result(ptr::null_mut());
    }
}
