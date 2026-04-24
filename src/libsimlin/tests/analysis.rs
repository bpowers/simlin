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
