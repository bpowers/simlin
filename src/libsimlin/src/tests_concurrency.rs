// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// ── concurrency regression tests ────────────────────────────────────────

/// Regression test for issue #303: concurrent add_model + sim_new should never
/// produce a spurious NotSimulatable error caused by sync_state being temporarily
/// None between .take() and the restore.
#[test]
fn test_concurrent_add_model_and_sim_new_no_spurious_not_simulatable() {
    use std::ffi::CString;
    use std::sync::Arc;
    use std::thread;

    let datamodel = TestProject::new("concurrent_add_model")
        .stock("population", "100", &["births"], &["deaths"], None)
        .flow("births", "population * 0.02", None)
        .flow("deaths", "population * 0.01", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);

    let thread_count = 20;
    unsafe {
        for _ in 0..thread_count {
            simlin_project_ref(proj);
        }
    }

    let proj_addr = proj as usize;
    let barrier = Arc::new(std::sync::Barrier::new(thread_count));

    let mut handles = Vec::new();

    // 10 threads doing add_model
    for i in 0..10 {
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            unsafe {
                let proj = proj_addr as *mut SimlinProject;
                let model_name = CString::new(format!("extra_model_{i}")).unwrap();
                let mut out_error: *mut SimlinError = std::ptr::null_mut();
                simlin_project_add_model(proj, model_name.as_ptr(), &mut out_error);
                if !out_error.is_null() {
                    simlin_error_free(out_error);
                }
                simlin_project_unref(proj);
            }
        }));
    }

    // 10 threads doing sim_new on the "main" model
    for _ in 0..10 {
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            unsafe {
                let proj = proj_addr as *mut SimlinProject;
                let mut out_error: *mut SimlinError = std::ptr::null_mut();
                let model = simlin_project_get_model(proj, std::ptr::null(), &mut out_error);
                if model.is_null() {
                    if !out_error.is_null() {
                        simlin_error_free(out_error);
                    }
                    simlin_project_unref(proj);
                    return;
                }

                let mut sim_error: *mut SimlinError = std::ptr::null_mut();
                let sim = simlin_sim_new(model, false, &mut sim_error);

                if !sim_error.is_null() {
                    let code = simlin_error_get_code(sim_error);
                    assert_ne!(
                        code,
                        SimlinErrorCode::NotSimulatable,
                        "sim_new should never fail with NotSimulatable due to missing sync_state"
                    );
                    simlin_error_free(sim_error);
                }

                if !sim.is_null() {
                    simlin_sim_unref(sim);
                }
                simlin_model_unref(model);
                simlin_project_unref(proj);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("thread panicked");
    }

    unsafe {
        simlin_project_unref(proj);
    }
}

/// Regression test for issue #296: warning baseline and datamodel snapshot
/// must be captured under one project lock scope so competing patches cannot
/// interleave between those reads.
#[test]
fn test_issue_296_snapshot_lock_blocks_competing_patch() {
    use crate::patch::{PatchHookPoint, install_patch_test_hook};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let datamodel = TestProject::new("issue_296")
        .aux("a", "1", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    let (hook_enter_tx, hook_enter_rx) = mpsc::channel::<()>();
    let release = Arc::new(AtomicBool::new(false));
    let release_for_hook = Arc::clone(&release);
    let hook = Arc::new(move |point: PatchHookPoint, project_ref: &SimlinProject| {
        if point == PatchHookPoint::SnapshotWhileProjectLocked
            && (project_ref as *const SimlinProject as usize) == proj_addr
        {
            hook_enter_tx
                .send(())
                .expect("issue #296 hook enter send should succeed");
            while !release_for_hook.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        }
    });
    let _hook_guard = install_patch_test_hook(hook);

    let patch_a = String::from(
        r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "a", "equation": "2" } }
                }]
            }]
        }"#,
    );
    let patch_b = String::from(
        r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "a", "equation": "3" } }
                }]
            }]
        }"#,
    );

    unsafe {
        simlin_project_ref(proj);
        simlin_project_ref(proj);
    }

    let writer_a = thread::spawn(move || unsafe {
        let proj = proj_addr as *mut SimlinProject;
        let mut out_error: *mut SimlinError = std::ptr::null_mut();
        let mut collected: *mut SimlinError = std::ptr::null_mut();
        let bytes = patch_a.as_bytes();
        simlin_project_apply_patch(
            proj,
            bytes.as_ptr(),
            bytes.len(),
            true,
            true,
            &mut collected,
            &mut out_error,
        );
        if !collected.is_null() {
            simlin_error_free(collected);
        }
        assert!(
            out_error.is_null(),
            "writer A patch should succeed while issue #296 hook is active"
        );
        simlin_project_unref(proj);
    });

    hook_enter_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("issue #296 hook should have been reached");

    let (writer_b_done_tx, writer_b_done_rx) = mpsc::channel::<()>();
    let writer_b = thread::spawn(move || unsafe {
        let proj = proj_addr as *mut SimlinProject;
        let mut out_error: *mut SimlinError = std::ptr::null_mut();
        let mut collected: *mut SimlinError = std::ptr::null_mut();
        let bytes = patch_b.as_bytes();
        simlin_project_apply_patch(
            proj,
            bytes.as_ptr(),
            bytes.len(),
            true,
            true,
            &mut collected,
            &mut out_error,
        );
        if !collected.is_null() {
            simlin_error_free(collected);
        }
        assert!(out_error.is_null(), "writer B patch should succeed");
        writer_b_done_tx
            .send(())
            .expect("writer B done send should succeed");
        simlin_project_unref(proj);
    });

    assert!(
        writer_b_done_rx
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "writer B should still be blocked while writer A holds the snapshot lock"
    );

    release.store(true, Ordering::Release);

    writer_b_done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("writer B should complete after writer A releases the snapshot lock");
    writer_a.join().expect("writer A should not panic");
    writer_b.join().expect("writer B should not panic");

    unsafe {
        simlin_project_unref(proj);
    }
}

/// Regression test for issue #297: sync_state must never be temporarily set to
/// None during patch validation.
#[test]
fn test_issue_297_patch_staging_keeps_sync_state_present() {
    use crate::patch::{PatchHookPoint, install_patch_test_hook};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let datamodel = TestProject::new("issue_297")
        .aux("alpha", "100", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    let hook_fired = Arc::new(AtomicBool::new(false));
    let observed_none = Arc::new(AtomicBool::new(false));
    let hook_fired_for_hook = Arc::clone(&hook_fired);
    let observed_none_for_hook = Arc::clone(&observed_none);
    let hook = Arc::new(move |point: PatchHookPoint, project_ref: &SimlinProject| {
        if point == PatchHookPoint::StagedSyncWhileDbLocked
            && (project_ref as *const SimlinProject as usize) == proj_addr
        {
            hook_fired_for_hook.store(true, Ordering::SeqCst);
            if project_ref.sync_state.lock().unwrap().is_none() {
                observed_none_for_hook.store(true, Ordering::SeqCst);
            }
        }
    });
    let _hook_guard = install_patch_test_hook(hook);

    let patch = r#"{
        "models": [{
            "name": "main",
            "ops": [{
                "type": "upsertAux",
                "payload": { "aux": { "name": "alpha", "equation": "123" } }
            }]
        }]
    }"#;

    unsafe {
        let mut out_error: *mut SimlinError = std::ptr::null_mut();
        let mut collected: *mut SimlinError = std::ptr::null_mut();
        let bytes = patch.as_bytes();
        simlin_project_apply_patch(
            proj,
            bytes.as_ptr(),
            bytes.len(),
            true,
            true,
            &mut collected,
            &mut out_error,
        );
        if !collected.is_null() {
            simlin_error_free(collected);
        }
        assert!(out_error.is_null(), "patch should succeed");
    }

    assert!(
        hook_fired.load(Ordering::SeqCst),
        "issue #297 hook should be reached during patch staging"
    );
    assert!(
        !observed_none.load(Ordering::SeqCst),
        "sync_state should never be None during patch staging"
    );

    unsafe {
        simlin_project_unref(proj);
    }
}

/// Regression test for issue #298: readers must not observe staged DB state
/// while patch validation is still in-flight.
#[test]
fn test_issue_298_sim_new_blocks_until_patch_decision() {
    use crate::patch::{PatchHookPoint, install_patch_test_hook};
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let datamodel = TestProject::new("issue_298")
        .aux("alpha", "100", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    let (hook_enter_tx, hook_enter_rx) = mpsc::channel::<()>();
    let release = Arc::new(AtomicBool::new(false));
    let release_for_hook = Arc::clone(&release);
    let hook = Arc::new(move |point: PatchHookPoint, project_ref: &SimlinProject| {
        if point == PatchHookPoint::StagedSyncWhileDbLocked
            && (project_ref as *const SimlinProject as usize) == proj_addr
        {
            hook_enter_tx
                .send(())
                .expect("issue #298 hook enter send should succeed");
            while !release_for_hook.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        }
    });
    let _hook_guard = install_patch_test_hook(hook);

    let patch = String::from(
        r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "alpha", "equation": "999" } }
                }]
            }]
        }"#,
    );

    unsafe {
        simlin_project_ref(proj);
        simlin_project_ref(proj);
    }

    let writer = thread::spawn(move || unsafe {
        let proj = proj_addr as *mut SimlinProject;
        let mut out_error: *mut SimlinError = std::ptr::null_mut();
        let mut collected: *mut SimlinError = std::ptr::null_mut();
        let bytes = patch.as_bytes();
        simlin_project_apply_patch(
            proj,
            bytes.as_ptr(),
            bytes.len(),
            true,
            true,
            &mut collected,
            &mut out_error,
        );
        if !collected.is_null() {
            simlin_error_free(collected);
        }
        assert!(out_error.is_null(), "dry-run patch should succeed");
        simlin_project_unref(proj);
    });

    hook_enter_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("issue #298 hook should have been reached");

    let (reader_result_tx, reader_result_rx) = mpsc::channel::<f64>();
    let reader = thread::spawn(move || unsafe {
        let proj = proj_addr as *mut SimlinProject;
        let mut out_error: *mut SimlinError = std::ptr::null_mut();
        let model = simlin_project_get_model(proj, std::ptr::null(), &mut out_error);
        assert!(!model.is_null(), "model lookup should succeed");
        assert!(out_error.is_null(), "model lookup should not error");

        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null(), "sim_new should succeed");
        assert!(out_error.is_null(), "sim_new should not error");

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null(), "simulation should run");

        let mut value = 0.0_f64;
        let alpha_name = CString::new("alpha").unwrap();
        simlin_sim_get_value(sim, alpha_name.as_ptr(), &mut value, &mut out_error);
        assert!(out_error.is_null(), "get_value should succeed");

        reader_result_tx
            .send(value)
            .expect("reader result send should succeed");
        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    });

    assert!(
        reader_result_rx
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "sim_new reader should block while patch validation holds the db lock"
    );

    release.store(true, Ordering::Release);

    let alpha_value = reader_result_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reader should complete after patch decision");
    assert!(
        (alpha_value - 100.0).abs() < 1e-10,
        "reader should observe committed value 100 after dry-run, got {}",
        alpha_value
    );

    writer.join().expect("writer should not panic");
    reader.join().expect("reader should not panic");

    unsafe {
        simlin_project_unref(proj);
    }
}
