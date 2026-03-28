// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod common;

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double};
use std::ptr;
use std::sync::atomic::Ordering;

use prost::Message;
use simlin::*;
use simlin_engine::serde as engine_serde;
use simlin_engine::test_common::TestProject;
use simlin_engine::{self as engine};

use common::open_project_from_datamodel;

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

        // Fetch var names from sim
        err = ptr::null_mut();
        let mut count: usize = 0;
        simlin_sim_get_var_count(
            sim,
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
        simlin_sim_get_var_names(
            sim,
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
        simlin_sim_get_var_count(
            sim,
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
        simlin_sim_get_var_names(
            sim,
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

#[test]
fn test_concurrent_project_ref_unref() {
    use std::thread;

    unsafe {
        // Create a test project
        let datamodel = TestProject::new("concurrent_test").build_datamodel();
        let pb_project = engine_serde::serialize(&datamodel).unwrap();
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
        let pb_project = engine_serde::serialize(&datamodel).unwrap();
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
                        0,
                        ptr::null(),
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
        let pb_project = engine_serde::serialize(&datamodel).unwrap();
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
        let model =
            simlin_project_get_model(proj, ptr::null(), &mut err_model as *mut *mut SimlinError);
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
        let pb_project = engine_serde::serialize(&datamodel).unwrap();
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
fn test_libsimlin_set_value_survives_run_to_end() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Set value for birth_rate
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(sim, c_name.as_ptr(), 0.2, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "set_value failed");

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
fn test_libsimlin_set_value_when_vm_is_none() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Consume the VM
        run_to_end(sim);

        // Set value while VM is None
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(sim, c_name.as_ptr(), 0.3, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "set_value with no VM should succeed");

        // Reset creates a new VM with the value applied
        reset_sim(sim);
        run_to_end(sim);

        // Verify the set value took effect by comparing against default
        let series_overridden = get_series_vec(sim, "population", 200);

        // Reset with no value set to get baseline
        err = ptr::null_mut();
        simlin_sim_clear_values(sim, &mut err as *mut *mut SimlinError);
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
fn test_libsimlin_set_value_flows_through_dependents() {
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
        simlin_sim_set_value(
            sim,
            c_name.as_ptr(),
            20.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(err.is_null(), "set_value failed");

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
fn test_libsimlin_clear_values_restores_defaults() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Get default series
        run_to_end(sim);
        let series_default = get_series_vec(sim, "population", 200);

        // Override, reset, run
        let c_name = CString::new("birth_rate").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(sim, c_name.as_ptr(), 0.5, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        reset_sim(sim);
        run_to_end(sim);
        let series_overridden = get_series_vec(sim, "population", 200);

        // Clear overrides, reset, run — should match default
        err = ptr::null_mut();
        simlin_sim_clear_values(sim, &mut err as *mut *mut SimlinError);
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
fn test_libsimlin_set_value_validates_without_vm() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);

        // Consume the VM so we exercise the no-VM validation path
        run_to_end(sim);

        // Setting a non-constant variable (flow) by name should fail
        let c_births = CString::new("births").unwrap();
        let mut err: *mut SimlinError = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_births.as_ptr(),
            42.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(
            !err.is_null(),
            "non-constant variable should fail even without a VM"
        );
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadOverride);
        simlin_error_free(err);

        // Setting a nonexistent variable should fail
        let c_nonexistent = CString::new("nonexistent_var").unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value(
            sim,
            c_nonexistent.as_ptr(),
            42.0,
            &mut err as *mut *mut SimlinError,
        );
        assert!(!err.is_null(), "nonexistent variable should fail");
        assert_eq!(simlin_error_get_code(err), SimlinErrorCode::DoesNotExist);
        simlin_error_free(err);

        // Setting a constant variable (birth_rate) should succeed
        let c_rate = CString::new("birth_rate").unwrap();
        err = ptr::null_mut();
        simlin_sim_set_value(sim, c_rate.as_ptr(), 0.5, &mut err as *mut *mut SimlinError);
        assert!(err.is_null(), "constant variable should succeed without VM");

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

#[test]
fn test_libsimlin_multiple_reset_set_value_cycles() {
    let dm = build_population_datamodel();
    unsafe {
        let (proj, model, sim) = create_test_sim(&dm);
        let c_name = CString::new("birth_rate").unwrap();

        let mut prev_final = 0.0;
        for i in 1..=10 {
            let rate = i as f64 * 0.02;
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_sim_set_value(
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

#[test]
fn test_sim_get_var_count_and_names() {
    let datamodel = TestProject::new("sim_vars")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .aux("growth_rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        assert!(!model.is_null());

        let sim = simlin_sim_new(model, false, &mut err);
        assert!(err.is_null());
        assert!(!sim.is_null());

        // Get count
        let mut count: usize = 0;
        simlin_sim_get_var_count(sim, &mut count, &mut err);
        assert!(err.is_null(), "get_var_count should succeed");
        assert!(count > 0, "expected at least one sim var");

        // Verify no internal ($-prefixed) vars are counted: the count
        // should match the number of names returned.
        let mut name_ptrs: Vec<*mut c_char> = vec![ptr::null_mut(); count];
        let mut written: usize = 0;
        simlin_sim_get_var_names(sim, name_ptrs.as_mut_ptr(), count, &mut written, &mut err);
        assert!(err.is_null(), "get_var_names should succeed");
        assert_eq!(written, count, "written count must match var count");

        let mut names: Vec<String> = Vec::with_capacity(written);
        for &p in &name_ptrs[..written] {
            assert!(!p.is_null());
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            assert!(
                !s.starts_with('$'),
                "internal var '{}' should be filtered out",
                s,
            );
            names.push(s);
            simlin_free_string(p);
        }

        // Names should be sorted
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "sim var names should be sorted");

        // All model-level variables should appear (possibly flattened)
        for expected in &["population", "births", "deaths", "growth_rate"] {
            assert!(
                names.iter().any(|n| n.contains(expected)),
                "expected '{}' in sim var names {:?}",
                expected,
                names,
            );
        }

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

// ── Phase 7: Incremental compilation integration tests ─────────────────

#[test]
fn test_sim_lifecycle() {
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
            variables: vec![engine::project_io::Variable {
                v: Some(engine::project_io::variable::V::Aux(
                    engine::project_io::variable::Aux {
                        ident: "time".to_string(),
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
        // Project ref count should have increased when model was created
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);

        // Test model reference counting
        simlin_model_ref(model);
        assert_eq!((*model).ref_count.load(Ordering::SeqCst), 2);
        simlin_model_unref(model);
        assert_eq!((*model).ref_count.load(Ordering::SeqCst), 1);

        err = ptr::null_mut();
        let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim.is_null());
        // Model ref count should have increased when sim was created
        assert_eq!((*model).ref_count.load(Ordering::SeqCst), 2);

        // Test sim reference counting
        simlin_sim_ref(sim);
        assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 2);
        simlin_sim_unref(sim);
        assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 1);
        simlin_sim_unref(sim);
        // Sim should be freed now, model ref count should decrease
        assert_eq!((*model).ref_count.load(Ordering::SeqCst), 1);

        simlin_model_unref(model);
        // Model should be freed now, project ref count should decrease
        assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

        simlin_project_unref(proj);
    }
}

#[test]
fn test_ltm_enabled_sim() {
    // Create a project with a feedback loop
    let test_project = TestProject::new("test_ltm")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * 0.02", None);

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

        // Create simulation with LTM enabled
        err = ptr::null_mut();
        let sim_ltm = simlin_sim_new(model, true, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim_ltm.is_null());

        // Run simulation
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim_ltm, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // Create another sim without LTM
        err = ptr::null_mut();
        let sim_no_ltm = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        assert!(!sim_no_ltm.is_null());

        // Run this one too
        err = ptr::null_mut();
        simlin_sim_run_to_end(sim_no_ltm, &mut err as *mut *mut SimlinError);
        assert!(err.is_null());
        // Clean up
        simlin_sim_unref(sim_ltm);
        simlin_sim_unref(sim_no_ltm);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
