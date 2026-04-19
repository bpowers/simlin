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

/// Regression test for iteration-17 codex P2: `simlin_sim_new` resets
/// `ltm_enabled` back to false once compilation completes, which
/// invalidates `model_all_diagnostics`' LTM accumulator branch.  Before
/// the fix, a later `simlin_project_get_errors` call against a model
/// that had auto-switched to discovery mode would report nothing, even
/// though the simulation had already lost its per-loop scoring.
///
/// The fix captures LTM diagnostics on `SimlinProject` at sim_new time
/// and replays them through `simlin_project_get_errors`.
#[test]
fn test_auto_flip_warning_surfaces_via_get_errors_after_sim_new() {
    // Build N disjoint 3-cycles to exceed MAX_LTM_TOTAL_CIRCUITS = 10_000.
    // Using scalar auxes so each cycle is variable-level distinct; the
    // emit-count backstop will trip on the post-collapse estimate.
    let n: usize = 10_001;
    let mut builder = TestProject::new("auto_flip_ffi").with_sim_time(0.0, 1.0, 1.0);
    for k in 0..n {
        let aux_name = format!("aux_{k}");
        let flow_name = format!("flow_{k}");
        let stock_name = format!("stock_{k}");
        builder = builder.aux(&aux_name, &stock_name, None);
        builder = builder.flow(&flow_name, &aux_name, None);
        builder = builder.stock(&stock_name, "0", &[flow_name.as_str()], &[], None);
    }
    let datamodel_project = builder.build_datamodel();
    let pb = engine_serde::serialize(&datamodel_project).unwrap();
    let mut buf = Vec::new();
    pb.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null(), "project open failed");
        assert!(err.is_null(), "project open reported error");

        // Baseline: before sim_new, get_errors must NOT report an LTM
        // auto-flip warning -- the project was not compiled under LTM.
        err = ptr::null_mut();
        let baseline = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "baseline get_errors set out_error");
        if !baseline.is_null() {
            let detail_count = simlin_error_get_detail_count(baseline);
            for i in 0..detail_count {
                let detail = simlin_error_get_detail(baseline, i);
                assert!(!detail.is_null());
                if !(*detail).message.is_null() {
                    let msg = CStr::from_ptr((*detail).message).to_str().unwrap_or("");
                    assert!(
                        !msg.contains("discovery mode"),
                        "baseline (pre-LTM-sim) get_errors leaked an LTM diagnostic: {msg}"
                    );
                }
            }
            simlin_error_free(baseline);
        }

        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        assert!(err_get_model.is_null(), "get_model failed");
        assert!(!model.is_null());

        // Create a sim with LTM enabled.  The large-disjoint-cycles
        // structure auto-flips to discovery, which emits the diagnostic
        // we expect to capture.
        err = ptr::null_mut();
        let sim = simlin_sim_new(model, true, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "sim_new with enable_ltm=true should succeed");
        assert!(!sim.is_null());

        // After sim_new (flag has been reset by simlin_sim_new),
        // get_errors must still surface the auto-flip warning via the
        // captured-diagnostic path.
        err = ptr::null_mut();
        let post_sim = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "post-sim get_errors set out_error");
        assert!(
            !post_sim.is_null(),
            "post-sim get_errors should return the captured LTM warning, got null"
        );

        let mut found_auto_flip = false;
        let detail_count = simlin_error_get_detail_count(post_sim);
        for i in 0..detail_count {
            let detail = simlin_error_get_detail(post_sim, i);
            assert!(!detail.is_null());
            if !(*detail).message.is_null() {
                let msg = CStr::from_ptr((*detail).message).to_str().unwrap_or("");
                if msg.contains("discovery mode") {
                    found_auto_flip = true;
                    break;
                }
            }
        }
        assert!(
            found_auto_flip,
            "post-sim get_errors must surface the LTM auto-flip warning"
        );
        simlin_error_free(post_sim);

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Regression test for iter-18 codex P2 / claude #1: repeated
/// `simlin_sim_new(enable_ltm = true)` on a project with an
/// auto-flipping model must produce exactly one LTM warning per
/// model-message pair in `simlin_project_get_errors` output, not
/// accumulate duplicates across calls.  Exercises the unconditional-
/// overwrite semantics in `simlin_sim_new` combined with the
/// `(model, message)` dedup key in `append_captured_diagnostics`.
#[test]
fn test_pending_ltm_diagnostics_overwrite_deduplicates() {
    let n: usize = 10_001;
    let mut builder = TestProject::new("overwrite_regression").with_sim_time(0.0, 1.0, 1.0);
    for k in 0..n {
        let aux_name = format!("aux_{k}");
        let flow_name = format!("flow_{k}");
        let stock_name = format!("stock_{k}");
        builder = builder.aux(&aux_name, &stock_name, None);
        builder = builder.flow(&flow_name, &aux_name, None);
        builder = builder.stock(&stock_name, "0", &[flow_name.as_str()], &[], None);
    }
    let datamodel_project = builder.build_datamodel();
    let pb = engine_serde::serialize(&datamodel_project).unwrap();
    let mut buf = Vec::new();
    pb.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());
        assert!(err.is_null());

        let mut err_get_model: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(
            proj,
            ptr::null(),
            &mut err_get_model as *mut *mut SimlinError,
        );
        assert!(err_get_model.is_null());
        assert!(!model.is_null());

        let count_discovery_mode = |errors: *mut SimlinError| -> usize {
            if errors.is_null() {
                return 0;
            }
            let mut hits = 0usize;
            let n_details = simlin_error_get_detail_count(errors);
            for i in 0..n_details {
                let detail = simlin_error_get_detail(errors, i);
                if !detail.is_null() && !(*detail).message.is_null() {
                    let msg = CStr::from_ptr((*detail).message).to_str().unwrap_or("");
                    if msg.contains("discovery mode") {
                        hits += 1;
                    }
                }
            }
            hits
        };

        // Run sim_new with LTM three times in a row on the same
        // project + model.  After each, count "discovery mode"
        // mentions in the get_errors output.  The first call
        // populates `pending_ltm_diagnostics`; the second and third
        // must *overwrite* (not accumulate), and the dedup path must
        // keep the salsa-surfaced warning from stacking with the
        // captured slot.
        for call_idx in 0..3 {
            err = ptr::null_mut();
            let sim = simlin_sim_new(model, true, &mut err as *mut *mut SimlinError);
            assert!(err.is_null(), "sim_new #{call_idx} failed");
            assert!(!sim.is_null());

            err = ptr::null_mut();
            let errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            let hits = count_discovery_mode(errors);
            assert_eq!(
                hits, 1,
                "sim_new call #{call_idx}: expected exactly one 'discovery mode' warning, got {hits}"
            );
            if !errors.is_null() {
                simlin_error_free(errors);
            }
            simlin_sim_unref(sim);
        }

        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_pending_ltm_diagnostics_multi_model_project() {
    use simlin_engine::datamodel;

    // Build a multi-model project: "dense" trips auto-flip,
    // "sparse" does not.  Construct manually since `TestProject`
    // produces a single-model project.
    let dense_name = "dense";
    let sparse_name = "sparse";
    let n: usize = 10_001;
    let mut dense_vars: Vec<datamodel::Variable> = Vec::with_capacity(3 * n);
    for k in 0..n {
        let aux = format!("aux_{k}");
        let flow = format!("flow_{k}");
        let stock = format!("stock_{k}");
        dense_vars.push(datamodel::Variable::Aux(datamodel::Aux {
            ident: aux.clone(),
            equation: datamodel::Equation::Scalar(stock.clone()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        dense_vars.push(datamodel::Variable::Flow(datamodel::Flow {
            ident: flow.clone(),
            equation: datamodel::Equation::Scalar(aux.clone()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
        dense_vars.push(datamodel::Variable::Stock(datamodel::Stock {
            ident: stock,
            equation: datamodel::Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![flow],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    }
    let sparse_vars = vec![
        datamodel::Variable::Stock(datamodel::Stock {
            ident: "population".to_string(),
            equation: datamodel::Equation::Scalar("100".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec!["births".to_string()],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
        datamodel::Variable::Flow(datamodel::Flow {
            ident: "births".to_string(),
            equation: datamodel::Equation::Scalar("population * 0.02".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
    ];
    let sim_specs = datamodel::SimSpecs {
        start: 0.0,
        stop: 1.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    };
    let datamodel_project = datamodel::Project {
        name: "multi".to_string(),
        sim_specs: sim_specs.clone(),
        dimensions: vec![],
        units: vec![],
        models: vec![
            datamodel::Model {
                name: dense_name.to_string(),
                sim_specs: None,
                variables: dense_vars,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
            datamodel::Model {
                name: sparse_name.to_string(),
                sim_specs: None,
                variables: sparse_vars,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            },
        ],
        source: Default::default(),
        ai_information: None,
    };
    let pb = engine_serde::serialize(&datamodel_project).unwrap();
    let mut buf = Vec::new();
    pb.encode(&mut buf).unwrap();

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
        assert!(!proj.is_null());

        // sim_new on the sparse model: `collect_all_diagnostics`
        // still iterates every model, so the dense model's auto-flip
        // warning is captured into `pending_ltm_diagnostics`.  That
        // is the correct project-wide scoping: the warning tells the
        // caller that at least one model in their project would drop
        // to discovery mode under LTM.  The check below pins that
        // scoping so a future change that narrows capture to
        // "current sim's model" is visible as a test failure.
        let sparse_c = CString::new(sparse_name).unwrap();
        err = ptr::null_mut();
        let sparse_model =
            simlin_project_get_model(proj, sparse_c.as_ptr(), &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sparse_model.is_null());
        err = ptr::null_mut();
        let sparse_sim = simlin_sim_new(sparse_model, true, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sparse_sim.is_null());

        err = ptr::null_mut();
        let errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(
            !errors.is_null(),
            "get_errors should report the dense model's LTM warning even when the sim is for the sparse model"
        );
        let mut dense_hits = 0usize;
        let mut sparse_hits = 0usize;
        let n_details = simlin_error_get_detail_count(errors);
        for i in 0..n_details {
            let detail = simlin_error_get_detail(errors, i);
            if !detail.is_null() && !(*detail).message.is_null() {
                let msg = CStr::from_ptr((*detail).message).to_str().unwrap_or("");
                if msg.contains("discovery mode") {
                    if msg.contains("'dense'") {
                        dense_hits += 1;
                    }
                    if msg.contains("'sparse'") {
                        sparse_hits += 1;
                    }
                }
            }
        }
        assert_eq!(
            dense_hits, 1,
            "exactly one 'discovery mode' warning must name the dense model"
        );
        assert_eq!(
            sparse_hits, 0,
            "sparse model must not emit a 'discovery mode' warning"
        );
        simlin_error_free(errors);

        simlin_sim_unref(sparse_sim);
        simlin_model_unref(sparse_model);
        simlin_project_unref(proj);
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
