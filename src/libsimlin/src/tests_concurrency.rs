// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// ── concurrency regression tests ────────────────────────────────────────
//
// Timeout policy: positive waits ("the other thread SHOULD get here") use
// POSITIVE_WAIT, generous because `recv_timeout` returns as soon as the
// message arrives -- the budget is only consumed on genuine failure, and a
// tight budget false-fails under coverage instrumentation or CI load
// (GH #726). Negative waits ("the other thread should NOT have completed
// yet") stay short: slowness can only make them pass, never fail, and they
// are paid in full on every run.

const POSITIVE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

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

/// A test that panics while holding the patch-test-hook guard (e.g. a timing
/// assertion blown by coverage instrumentation slowdown, GH #726) poisons the
/// hook mutexes. Subsequent hook installs must recover instead of cascading
/// that one failure into spurious panics in every other hook-based test.
#[test]
fn test_patch_hook_survives_poisoning_panic() {
    use crate::patch::install_patch_test_hook;
    use std::sync::Arc;
    use std::thread;

    let result = thread::spawn(|| {
        let _guard = install_patch_test_hook(Arc::new(|_, _| {}));
        panic!("intentional panic while holding the patch test hook guard");
    })
    .join();
    assert!(result.is_err(), "spawned thread should have panicked");

    // Without poison recovery this second install panics on the poisoned
    // lock; with it, hook-based tests keep working after an earlier failure.
    let _guard = install_patch_test_hook(Arc::new(|_, _| {}));
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
        .recv_timeout(POSITIVE_WAIT)
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
        .recv_timeout(POSITIVE_WAIT)
        .expect("writer B should complete after writer A releases the snapshot lock");
    writer_a.join().expect("writer A should not panic");
    writer_b.join().expect("writer B should not panic");

    unsafe {
        simlin_project_unref(proj);
    }
}

/// Regression test for issue #297: a concurrent reader must never observe the
/// db's sync state as missing (`None`) during patch validation.
///
/// The db now owns its own sync state (rather than a separate `sync_state`
/// mutex), so the invariant is structural: the state is only transiently absent
/// inside `db.sync_staged`/`db.restore`, which run under `&mut self` and thus
/// require the db lock. Any concurrent reader takes the SAME db lock, so it can
/// only ever observe a fully-synced db -- a reader that races a staging patch
/// blocks until the patch decision releases the lock, then sees a consistent
/// `Some(SourceProject)`, never a half-staged or `None` state.
///
/// This is a genuine CROSS-THREAD test: the patch hook holds the db lock during
/// staging while a reader thread tries to `db.lock()` + `current_source_project`.
/// (The reader probes `current_source_project` directly, the lowest-level form
/// of the invariant; #298 covers the higher-level reader path through
/// `simlin_sim_new` observing the committed value.) It would FAIL -- not merely
/// pass vacuously -- if a reader could ever acquire the lock mid-staging and see
/// `None`: the reader asserts `Some` while the staging thread is alive.
#[test]
fn test_issue_297_patch_staging_keeps_sync_state_present() {
    use crate::patch::{PatchHookPoint, install_patch_test_hook};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let datamodel = TestProject::new("issue_297")
        .aux("alpha", "100", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    // The hook blocks the patching thread (holding the db lock) at the staging
    // point until the main thread releases it, opening a deterministic window
    // in which the reader thread is guaranteed to be contending for the lock.
    let (hook_enter_tx, hook_enter_rx) = mpsc::channel::<()>();
    let release = Arc::new(AtomicBool::new(false));
    let release_for_hook = Arc::clone(&release);
    let hook = Arc::new(move |point: PatchHookPoint, project_ref: &SimlinProject| {
        if point == PatchHookPoint::StagedSyncWhileDbLocked
            && (project_ref as *const SimlinProject as usize) == proj_addr
        {
            hook_enter_tx
                .send(())
                .expect("issue #297 hook enter send should succeed");
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
                    "payload": { "aux": { "name": "alpha", "equation": "123" } }
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
        assert!(out_error.is_null(), "patch should succeed");
        simlin_project_unref(proj);
    });

    hook_enter_rx
        .recv_timeout(POSITIVE_WAIT)
        .expect("issue #297 hook should have been reached during staging");

    // Reader races the staging patch: it must block on the db lock until the
    // patch releases it, then observe a consistent `Some` sync state.
    let (reader_done_tx, reader_done_rx) = mpsc::channel::<bool>();
    let reader = thread::spawn(move || {
        let proj_ref = unsafe { &*(proj_addr as *const SimlinProject) };
        let db = proj_ref.lock_db();
        let observed_some = db.current_source_project().is_some();
        drop(db);
        reader_done_tx
            .send(observed_some)
            .expect("reader result send should succeed");
    });

    // While the staging thread holds the lock, the reader cannot complete.
    assert!(
        reader_done_rx
            .recv_timeout(Duration::from_millis(200))
            .is_err(),
        "reader should block while patch staging holds the db lock"
    );

    release.store(true, Ordering::Release);

    let observed_some = reader_done_rx
        .recv_timeout(POSITIVE_WAIT)
        .expect("reader should complete after the patch decision releases the lock");
    assert!(
        observed_some,
        "concurrent reader must observe Some(SourceProject), never a None sync state"
    );

    writer.join().expect("writer should not panic");
    reader.join().expect("reader should not panic");

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
        .recv_timeout(POSITIVE_WAIT)
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
        .recv_timeout(POSITIVE_WAIT)
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

/// Regression test: two concurrent patches to disjoint variables must
/// both be visible in the final state. Without proper serialization,
/// the second writer can snapshot a stale datamodel and overwrite the
/// first writer's changes (lost update).
#[test]
fn test_concurrent_patches_no_lost_update() {
    use crate::patch::{install_patch_test_hook, PatchHookPoint};
    use std::ffi::CString;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread;
    use std::time::Duration;

    let datamodel = TestProject::new("lost_update")
        .with_sim_time(0.0, 1.0, 1.0)
        .aux("a", "1", None)
        .aux("b", "10", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    // Hook: pause only the FIRST patch call at StagedSyncWhileDbLocked.
    // This gives Thread B a window to start its own patch.
    let (hook_enter_tx, hook_enter_rx) = mpsc::channel::<()>();
    let release = Arc::new(AtomicBool::new(false));
    let release_for_hook = Arc::clone(&release);
    let hook_count = Arc::new(AtomicU32::new(0));
    let hook = Arc::new(move |point: PatchHookPoint, project_ref: &SimlinProject| {
        if point == PatchHookPoint::StagedSyncWhileDbLocked
            && (project_ref as *const SimlinProject as usize) == proj_addr
            && hook_count.fetch_add(1, Ordering::SeqCst) == 0
        {
            let _ = hook_enter_tx.send(());
            while !release_for_hook.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        }
    });
    let _hook_guard = install_patch_test_hook(hook);

    // Extra refs for the two threads.
    unsafe {
        simlin_project_ref(proj);
        simlin_project_ref(proj);
    }

    // Thread A: patch a → 2
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
    let thread_a = thread::spawn(move || unsafe {
        let proj = proj_addr as *mut SimlinProject;
        let mut out_error: *mut SimlinError = std::ptr::null_mut();
        let mut collected: *mut SimlinError = std::ptr::null_mut();
        let bytes = patch_a.as_bytes();
        simlin_project_apply_patch(
            proj,
            bytes.as_ptr(),
            bytes.len(),
            false,
            true,
            &mut collected,
            &mut out_error,
        );
        if !collected.is_null() {
            simlin_error_free(collected);
        }
        assert!(out_error.is_null(), "Thread A patch should succeed");
        simlin_project_unref(proj);
    });

    // Wait for Thread A to enter validation (holding db lock).
    hook_enter_rx
        .recv_timeout(POSITIVE_WAIT)
        .expect("Thread A should reach the hook");

    // Thread B: patch b → 20. With the fix, this blocks on the
    // datamodel lock until Thread A commits. Without the fix, Thread B
    // would snapshot the stale datamodel (before Thread A's change).
    let patch_b = String::from(
        r#"{
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": { "aux": { "name": "b", "equation": "20" } }
                }]
            }]
        }"#,
    );
    let (b_done_tx, b_done_rx) = mpsc::channel::<()>();
    let thread_b = thread::spawn(move || unsafe {
        let proj = proj_addr as *mut SimlinProject;
        let mut out_error: *mut SimlinError = std::ptr::null_mut();
        let mut collected: *mut SimlinError = std::ptr::null_mut();
        let bytes = patch_b.as_bytes();
        simlin_project_apply_patch(
            proj,
            bytes.as_ptr(),
            bytes.len(),
            false,
            true,
            &mut collected,
            &mut out_error,
        );
        if !collected.is_null() {
            simlin_error_free(collected);
        }
        assert!(out_error.is_null(), "Thread B patch should succeed");
        let _ = b_done_tx.send(());
        simlin_project_unref(proj);
    });

    // Thread B should be blocked on the datamodel lock while Thread A
    // is paused in validation.
    assert!(
        b_done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
        "Thread B should block while Thread A holds the datamodel lock"
    );

    // Release Thread A so it commits and drops the datamodel lock.
    release.store(true, Ordering::Release);

    // Both threads should complete.
    thread_a.join().expect("Thread A should not panic");
    b_done_rx
        .recv_timeout(POSITIVE_WAIT)
        .expect("Thread B should complete after Thread A releases");
    thread_b.join().expect("Thread B should not panic");

    // Verify BOTH changes are present by simulating and reading values.
    unsafe {
        let mut out_error: *mut SimlinError = std::ptr::null_mut();
        let model = simlin_project_get_model(proj, std::ptr::null(), &mut out_error);
        assert!(!model.is_null(), "model lookup should succeed");
        let sim = simlin_sim_new(model, false, &mut out_error);
        assert!(!sim.is_null(), "sim creation should succeed");
        assert!(out_error.is_null());

        simlin_sim_run_to_end(sim, &mut out_error);
        assert!(out_error.is_null(), "simulation should run");

        let a_name = CString::new("a").unwrap();
        let b_name = CString::new("b").unwrap();
        let mut a_val = 0.0_f64;
        let mut b_val = 0.0_f64;
        simlin_sim_get_value(sim, a_name.as_ptr(), &mut a_val, &mut out_error);
        assert!(out_error.is_null());
        simlin_sim_get_value(sim, b_name.as_ptr(), &mut b_val, &mut out_error);
        assert!(out_error.is_null());

        assert!(
            (a_val - 2.0).abs() < 1e-10,
            "variable 'a' should be 2 (Thread A's patch), got {}",
            a_val
        );
        assert!(
            (b_val - 20.0).abs() < 1e-10,
            "variable 'b' should be 20 (Thread B's patch), got {}",
            b_val
        );

        simlin_sim_unref(sim);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A tool call holds the datamodel only while it takes the contents it
/// answers from: a host's hit test and revision read, which lock only the
/// datamodel, answer while a call is under way, as hover must while an agent
/// analyzes a large model.
#[cfg(feature = "agent_tools")]
#[test]
fn a_tool_call_holds_the_datamodel_only_while_it_takes_the_contents() {
    use crate::install_db_section_test_hook;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    let datamodel = TestProject::new("tool_lock")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let release = Arc::new(AtomicBool::new(false));
    let release_for_hook = Arc::clone(&release);
    let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
        if project as *const SimlinProject as usize == proj_addr {
            let _ = entered_tx.send(());
            while !release_for_hook.load(Ordering::Acquire) {
                thread::yield_now();
            }
        }
    }));
    // Released however the test ends, so no thread is left spinning.
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let release_guard = Release(Arc::clone(&release));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;
        let caller = thread::spawn(move || {
            let name = CString::new("read_model").unwrap();
            let (mut buf, mut len, mut is_error) = (ptr::null_mut(), 0usize, false);
            let mut err: *mut SimlinError = ptr::null_mut();
            crate::tools::simlin_tool_session_call(
                session_addr as *mut crate::tools::SimlinToolSession,
                name.as_ptr(),
                ptr::null(),
                0,
                &mut buf,
                &mut len,
                &mut is_error,
                &mut err,
            );
            assert!(err.is_null() && !is_error);
            simlin_free(buf);
        });
        entered_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the tool call reached its database-only section");

        let (done_tx, done_rx) = mpsc::channel::<u64>();
        let model_addr = model as usize;
        let reader = thread::spawn(move || {
            let model = model_addr as *mut SimlinModel;
            let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
            let mut err: *mut SimlinError = ptr::null_mut();
            // The model has no diagram, so the hit test reports that; what
            // matters is that it returns.
            simlin_model_hit_test(model, 0.0, 0.0, 1.0, &mut hit, &mut uid, &mut part, &mut err);
            if !err.is_null() {
                simlin_error_free(err);
            }
            let mut revision = u64::MAX;
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_project_get_revision(proj_addr as *mut SimlinProject, &mut revision, &mut err);
            assert!(err.is_null());
            let _ = done_tx.send(revision);
        });
        let revision = done_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("a hit test and a revision read answer while a tool call is under way");
        assert_eq!(revision, 0);

        drop(release_guard);
        caller.join().expect("the tool call finished");
        reader.join().expect("the reader finished");
        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A tool call that finds the database held -- by another session's call on
/// the project, or a host's query that holds only the database -- waits for
/// it with the datamodel released: a host's hit test and revision read answer
/// meanwhile, as they do behind the first call alone. A call's wait is not
/// counted, so one that waited with the datamodel held would keep them, and a
/// person's edit, waiting for the whole of the other work.
#[cfg(feature = "agent_tools")]
#[test]
fn a_tool_call_waiting_for_the_database_leaves_the_datamodel_to_others() {
    use crate::install_db_section_test_hook;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    let datamodel = TestProject::new("tool_lock_two_sessions")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    // The first call to take the database holds it until released.
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let hold = Arc::new(AtomicBool::new(true));
    let release = Arc::new(AtomicBool::new(false));
    let (hold_in_hook, release_in_hook) = (Arc::clone(&hold), Arc::clone(&release));
    let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
        if project as *const SimlinProject as usize == proj_addr
            && hold_in_hook.swap(false, Ordering::SeqCst)
        {
            let _ = entered_tx.send(());
            while !release_in_hook.load(Ordering::Acquire) {
                thread::yield_now();
            }
        }
    }));
    // Released however the test ends, so no thread is left spinning.
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let release_guard = Release(Arc::clone(&release));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        // Two sessions on one model, as two agents on one project have.
        let first = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let second = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let call = |session_addr: usize| {
            thread::spawn(move || {
                let name = CString::new("read_model").unwrap();
                let (mut buf, mut len, mut is_error) = (ptr::null_mut(), 0usize, false);
                let mut err: *mut SimlinError = ptr::null_mut();
                crate::tools::simlin_tool_session_call(
                    session_addr as *mut crate::tools::SimlinToolSession,
                    name.as_ptr(),
                    ptr::null(),
                    0,
                    &mut buf,
                    &mut len,
                    &mut is_error,
                    &mut err,
                );
                assert!(err.is_null() && !is_error);
                simlin_free(buf);
            })
        };
        let answering = call(first as usize);
        entered_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the first session's call holds the database");
        let waiting = call(second as usize);
        // Time for the second call to reach the database and wait for it. Were
        // it slower, the test would pass without having tested the wait; it
        // could not fail.
        thread::sleep(std::time::Duration::from_millis(200));

        let (done_tx, done_rx) = mpsc::channel::<u64>();
        let model_addr = model as usize;
        let reader = thread::spawn(move || {
            let model = model_addr as *mut SimlinModel;
            let (mut hit, mut uid, mut part) = (false, 0, SimlinHitPart::Body);
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_model_hit_test(model, 0.0, 0.0, 1.0, &mut hit, &mut uid, &mut part, &mut err);
            if !err.is_null() {
                simlin_error_free(err);
            }
            let mut revision = u64::MAX;
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_project_get_revision(proj_addr as *mut SimlinProject, &mut revision, &mut err);
            assert!(err.is_null());
            let _ = done_tx.send(revision);
        });
        let revision = done_rx.recv_timeout(POSITIVE_WAIT).expect(
            "a hit test and a revision read answer while a second session's call waits for the database",
        );
        assert_eq!(revision, 0);

        drop(release_guard);
        answering.join().expect("the first call answered");
        waiting.join().expect("the second call answered");
        reader.join().expect("the reader finished");
        crate::tools::simlin_tool_session_unref(first);
        crate::tools::simlin_tool_session_unref(second);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A host's read of a run that has to simulate stops, as a call does, for
/// work that waits for the project, and says so with a code of its own: the
/// host tells a read that stopped from a run the session lacks, and reads it
/// again once that work is done.
#[cfg(feature = "agent_tools")]
#[test]
fn an_interrupted_read_of_a_run_is_told_from_a_missing_one() {
    use std::sync::atomic::Ordering;

    let datamodel = TestProject::new("tool_read_stops")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let read = |name: &str| -> Result<(), SimlinErrorCode> {
            let name = CString::new(name).unwrap();
            let mut err: *mut SimlinError = ptr::null_mut();
            let results = crate::tools::simlin_tool_session_get_run(
                session,
                name.as_ptr(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut err,
            );
            if err.is_null() {
                simlin_results_unref(results);
                Ok(())
            } else {
                let code = simlin_error_get_code(err);
                simlin_error_free(err);
                Err(code)
            }
        };
        assert_eq!(read("missing"), Err(SimlinErrorCode::DoesNotExist));
        // Work waiting for the project, counted as an edit that holds the
        // datamodel while it waits for the database counts itself.
        (*proj).waiting_for_db.fetch_add(1, Ordering::SeqCst);
        assert_eq!(read("current"), Err(SimlinErrorCode::Interrupted));
        (*proj).waiting_for_db.fetch_sub(1, Ordering::SeqCst);
        assert_eq!(read("current"), Ok(()), "the read again, once the work is done");

        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A call of a tool that edits, made while a reading call on the same session
/// runs, counts itself as waiting from before it waits for the session, which
/// the reading call holds for its whole length: the reading call stops at its
/// next checkpoint and keeps nothing, and the edit is made, rather than
/// waiting the reading call out. With one session per document, and an agent
/// that makes its calls in parallel, this is a host's own setup.
#[cfg(feature = "agent_tools")]
#[test]
fn an_edit_stops_a_reading_call_on_its_own_session() {
    use crate::install_db_section_test_hook;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    let datamodel = TestProject::new("tool_edit_own_session")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;
    // Call `tool` on the session at `session_addr`: whether it refused, and
    // its output.
    let call = |session_addr: usize, tool: &str, input: &str| unsafe {
        let name = CString::new(tool).unwrap();
        let (mut buf, mut len, mut is_error) = (ptr::null_mut(), 0usize, false);
        let mut err: *mut SimlinError = ptr::null_mut();
        crate::tools::simlin_tool_session_call(
            session_addr as *mut crate::tools::SimlinToolSession,
            name.as_ptr(),
            input.as_ptr(),
            input.len(),
            &mut buf,
            &mut len,
            &mut is_error,
            &mut err,
        );
        assert!(err.is_null());
        let output: serde_json::Value =
            serde_json::from_slice(std::slice::from_raw_parts(buf, len)).unwrap();
        simlin_free(buf);
        (is_error, output)
    };

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;
        // An edit is of a model the session has read.
        call(session_addr, "read_model", "");

        // The next call waits inside its database section until work waits
        // for the project, or the positive wait runs out.
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let hold = Arc::new(AtomicBool::new(true));
        let hold_in_hook = Arc::clone(&hold);
        let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
            if project as *const SimlinProject as usize == proj_addr
                && hold_in_hook.swap(false, Ordering::SeqCst)
            {
                let _ = entered_tx.send(());
                let deadline = std::time::Instant::now() + POSITIVE_WAIT;
                while !project.is_waited_on() && std::time::Instant::now() < deadline {
                    thread::yield_now();
                }
            }
        }));
        let reader = thread::spawn(move || call(session_addr, "read_variables", r#"{"names": ["rate"]}"#));
        entered_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the reading call holds the session in its database section");
        let editor = thread::spawn(move || {
            let started = std::time::Instant::now();
            let (is_error, output) = call(
                session_addr,
                "edit_model",
                r#"{"summary": "a faster rate", "operations": [
                    {"op": "set_equation", "variable": "rate", "equation": "0.03"}]}"#,
            );
            (is_error, output, started.elapsed())
        });
        let (is_error, output) = reader.join().expect("the reading call finished");
        assert!(is_error, "{output}");
        assert_eq!(output["interrupted"], true, "the reading call stopped: {output}");
        let (is_error, output, waited) = editor.join().expect("the edit finished");
        assert!(!is_error, "the edit is made: {output}");
        assert!(
            waited < POSITIVE_WAIT / 2,
            "the edit waited {waited:?}, not the reading call's whole length"
        );
        let mut revision = 0;
        simlin_project_get_revision(proj, &mut revision, &mut err);
        assert!(err.is_null());
        assert_eq!(revision, 1, "the edit was made");
        assert!(!(*proj).is_waited_on(), "nothing waits once the edit is in");

        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A tool call stops for an edit that waits for the database: the edit holds
/// the datamodel while it waits, so every hit test waits with it, and a call
/// that ran on would keep them waiting for the rest of its work. The call
/// answers that it stopped and kept nothing, and the edit lands.
#[cfg(feature = "agent_tools")]
#[test]
fn a_tool_call_stops_for_an_edit_that_waits_for_it() {
    use crate::install_db_section_test_hook;
    use std::sync::mpsc;
    use std::thread;

    let datamodel = TestProject::new("tool_yield")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    // Inside the call, with the database held, wait until the edit is
    // waiting for it.
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
        if project as *const SimlinProject as usize == proj_addr {
            let _ = entered_tx.send(());
            let deadline = std::time::Instant::now() + POSITIVE_WAIT;
            while !project.is_waited_on() && std::time::Instant::now() < deadline {
                thread::yield_now();
            }
        }
    }));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;
        let caller = thread::spawn(move || {
            let name = CString::new("read_model").unwrap();
            let (mut buf, mut len, mut is_error) = (ptr::null_mut(), 0usize, false);
            let mut err: *mut SimlinError = ptr::null_mut();
            crate::tools::simlin_tool_session_call(
                session_addr as *mut crate::tools::SimlinToolSession,
                name.as_ptr(),
                ptr::null(),
                0,
                &mut buf,
                &mut len,
                &mut is_error,
                &mut err,
            );
            assert!(err.is_null());
            let output: serde_json::Value =
                serde_json::from_slice(std::slice::from_raw_parts(buf, len)).unwrap();
            simlin_free(buf);
            (is_error, output)
        });
        entered_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the tool call reached its database-only section");

        let editor = thread::spawn(move || {
            let patch = br#"{"models": [{"name": "main", "ops": [
                {"type": "upsertAux", "payload": {"aux": {"name": "rate", "equation": "0.03"}}}
            ]}]}"#;
            let (mut collected, mut err) = (ptr::null_mut(), ptr::null_mut());
            simlin_project_apply_patch(
                proj_addr as *mut SimlinProject,
                patch.as_ptr(),
                patch.len(),
                false,
                false,
                &mut collected,
                &mut err,
            );
            if !collected.is_null() {
                simlin_error_free(collected);
            }
            assert!(err.is_null(), "the edit lands");
        });

        let (is_error, output) = caller.join().expect("the tool call finished");
        assert!(is_error, "{output}");
        assert_eq!(output["interrupted"], true, "{output}");
        editor.join().expect("the edit finished");
        let mut revision = 0;
        simlin_project_get_revision(proj, &mut revision, &mut err);
        assert!(err.is_null());
        assert_eq!(revision, 1, "the edit landed");
        assert!(!(*proj).is_waited_on(), "nothing waits once the edit is in");

        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// Call `read_model` on the session at `session_addr`, as a host's thread
/// does: whether it refused, and its output.
#[cfg(feature = "agent_tools")]
unsafe fn read_model_on(session_addr: usize) -> (bool, serde_json::Value) {
    let name = CString::new("read_model").unwrap();
    let (mut buf, mut len, mut is_error) = (ptr::null_mut(), 0usize, false);
    let mut err: *mut SimlinError = ptr::null_mut();
    crate::tools::simlin_tool_session_call(
        session_addr as *mut crate::tools::SimlinToolSession,
        name.as_ptr(),
        ptr::null(),
        0,
        &mut buf,
        &mut len,
        &mut is_error,
        &mut err,
    );
    assert!(err.is_null());
    let output: serde_json::Value =
        serde_json::from_slice(std::slice::from_raw_parts(buf, len)).unwrap();
    simlin_free(buf);
    (is_error, output)
}

/// A tool call stops for a query that takes the database alone -- an
/// equation's rendering, the model's links -- as it does for an edit: the
/// query is the person's, and holds no datamodel lock only because it reads
/// none. Uncounted, it would wait the call's whole length.
#[cfg(feature = "agent_tools")]
#[test]
fn a_tool_call_stops_for_a_query_that_waits_for_the_database_alone() {
    use crate::install_db_section_test_hook;
    use std::sync::mpsc;
    use std::thread;

    let datamodel = TestProject::new("tool_yield_to_query")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;
    unsafe {
        // Built, so the query takes the database alone.
        drop((*proj).lock_db());
    }

    // Inside the call, with the database held, wait until the query is
    // waiting for it.
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
        if project as *const SimlinProject as usize == proj_addr {
            let _ = entered_tx.send(());
            let deadline = std::time::Instant::now() + POSITIVE_WAIT;
            while !project.is_waited_on() && std::time::Instant::now() < deadline {
                thread::yield_now();
            }
        }
    }));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;
        let caller = thread::spawn(move || read_model_on(session_addr));
        entered_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the tool call reached its database-only section");

        let model_addr = model as usize;
        let query = thread::spawn(move || {
            let births = CString::new("births").unwrap();
            let mut err: *mut SimlinError = ptr::null_mut();
            let latex = simlin_model_get_latex_equation(
                model_addr as *mut SimlinModel,
                births.as_ptr(),
                &mut err,
            );
            assert!(err.is_null() && !latex.is_null(), "the query answers");
            simlin_free_string(latex);
        });

        let (is_error, output) = caller.join().expect("the tool call finished");
        assert!(is_error, "{output}");
        assert_eq!(output["interrupted"], true, "{output}");
        query.join().expect("the query finished");
        assert!(!(*proj).is_waited_on(), "nothing waits once the query has answered");

        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// An edit of the diagram alone (a drag's commit) takes the datamodel and no
/// database: it lands while a tool call holds the database, and the call
/// runs on, answering at the revision it began at. A diagram edit that also
/// took the database would wait every call out, or stop it.
#[cfg(feature = "agent_tools")]
#[test]
fn a_diagram_edit_lands_during_a_tool_call_without_stopping_it() {
    use crate::install_db_section_test_hook;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    let bytes = crate::entry_point_tests::project_json(true, 100.0, 100.0, "0.1");
    let proj = unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_json(bytes.as_ptr(), bytes.len(), 0, &mut err);
        assert!(err.is_null());
        proj
    };
    let proj_addr = proj as usize;

    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let release = Arc::new(AtomicBool::new(false));
    let release_for_hook = Arc::clone(&release);
    let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
        if project as *const SimlinProject as usize == proj_addr {
            let _ = entered_tx.send(());
            while !release_for_hook.load(Ordering::Acquire) {
                thread::yield_now();
            }
        }
    }));
    // Released however the test ends, so no thread is left spinning.
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let release_guard = Release(Arc::clone(&release));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;
        let caller = thread::spawn(move || read_model_on(session_addr));
        entered_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the tool call reached its database-only section");

        let (landed_tx, landed_rx) = mpsc::channel::<u64>();
        let editor = thread::spawn(move || {
            let proj = proj_addr as *mut SimlinProject;
            let err = crate::entry_point_tests::apply_patch(
                proj,
                &crate::entry_point_tests::move_stock(),
                false,
                false,
            );
            assert!(err.is_null(), "the diagram edit lands");
            let _ = landed_tx.send(crate::entry_point_tests::revision(proj));
        });
        let landed_at = landed_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("a diagram edit lands while a tool call holds the database");
        assert_eq!(landed_at, 1);
        assert!(
            !(*proj).is_waited_on(),
            "the diagram edit did not wait for the database"
        );

        drop(release_guard);
        let (is_error, output) = caller.join().expect("the tool call finished");
        assert!(!is_error, "the call ran on: {output}");
        assert_eq!(output["revision"], 0, "at the revision it began at");
        editor.join().expect("the edit finished");

        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A call its host cancels while it runs -- here from inside the call, as any
/// thread may -- stops and answers that it was cancelled, not interrupted,
/// and a call made after the cancel answers as usual.
#[cfg(feature = "agent_tools")]
#[test]
fn a_cancelled_tool_call_stops_and_a_later_call_answers() {
    use crate::install_db_section_test_hook;
    use std::sync::atomic::{AtomicBool, Ordering};

    let datamodel = TestProject::new("tool_cancel")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;
        let proj_addr = proj as usize;
        let cancel_once = Arc::new(AtomicBool::new(true));
        let cancel_in_hook = Arc::clone(&cancel_once);
        let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
            if project as *const SimlinProject as usize == proj_addr
                && cancel_in_hook.swap(false, Ordering::SeqCst)
            {
                crate::tools::simlin_tool_session_cancel(
                    session_addr as *mut crate::tools::SimlinToolSession,
                );
            }
        }));

        let (is_error, output) = read_model_on(session_addr);
        assert!(is_error, "{output}");
        assert_eq!(output["cancelled"], true, "{output}");
        assert!(output.get("interrupted").is_none(), "{output}");
        assert!(!cancel_once.load(Ordering::SeqCst), "the hook cancelled");

        let (is_error, output) = read_model_on(session_addr);
        assert!(!is_error, "a call after the cancel answers: {output}");

        // NULL is no session to cancel.
        crate::tools::simlin_tool_session_cancel(ptr::null_mut());
        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A cancel covers a call waiting for the session as well as the one
/// answering: a host that closes a window stops all the work it began. The
/// waiting call answers as soon as it has the session, without waiting for
/// the database, which another session's call may hold.
#[cfg(feature = "agent_tools")]
#[test]
fn a_cancel_covers_a_call_waiting_for_the_session() {
    use crate::install_db_section_test_hook;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    let datamodel = TestProject::new("tool_cancel_waiting")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    // The first call holds the session inside the hook until released. The
    // hook runs once a call holds the database: how many do is counted.
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let hold = Arc::new(AtomicBool::new(true));
    let release = Arc::new(AtomicBool::new(false));
    let entered = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (hold_in_hook, release_in_hook) = (Arc::clone(&hold), Arc::clone(&release));
    let entered_in_hook = Arc::clone(&entered);
    let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
        if project as *const SimlinProject as usize != proj_addr {
            return;
        }
        entered_in_hook.fetch_add(1, Ordering::SeqCst);
        if hold_in_hook.swap(false, Ordering::SeqCst) {
            let _ = entered_tx.send(());
            while !release_in_hook.load(Ordering::Acquire) {
                thread::yield_now();
            }
        }
    }));
    struct Release(Arc<AtomicBool>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let release_guard = Release(Arc::clone(&release));

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;

        let answering = thread::spawn(move || read_model_on(session_addr));
        entered_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the first call holds the session");
        let waiting = thread::spawn(move || read_model_on(session_addr));
        let deadline = std::time::Instant::now() + POSITIVE_WAIT;
        while (*session).calls_begun() < 2 && std::time::Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!((*session).calls_begun(), 2, "the second call has begun");

        crate::tools::simlin_tool_session_cancel(session);
        drop(release_guard);
        for (which, call) in [("answering", answering), ("waiting", waiting)] {
            let (is_error, output) = call.join().expect("the call finished");
            assert!(is_error, "{which}: {output}");
            assert_eq!(output["cancelled"], true, "{which}: {output}");
        }
        assert_eq!(
            entered.load(Ordering::SeqCst),
            1,
            "the waiting call answered before it took the database"
        );

        let (is_error, output) = read_model_on(session_addr);
        assert!(!is_error, "a call after the cancel answers: {output}");

        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// A call of a tool that edits, cancelled while it waits for the session, is
/// not made: it answers as soon as it has the session, having taken no lock
/// of the project's, whose database another session's call may hold, and
/// leaves no one counted as waiting.
#[cfg(feature = "agent_tools")]
#[test]
fn an_edit_cancelled_while_it_waits_for_the_session_is_not_made() {
    use crate::install_db_section_test_hook;
    use crate::lock_order::{trace, Event, Rank};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    let datamodel = TestProject::new("tool_cancel_edit")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    let proj_addr = proj as usize;

    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;
        // An edit is of a model the session has read.
        let (is_error, output) = read_model_on(session_addr);
        assert!(!is_error, "{output}");

        // The next call holds the session inside the hook until released.
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let hold = Arc::new(AtomicBool::new(true));
        let release = Arc::new(AtomicBool::new(false));
        let (hold_in_hook, release_in_hook) = (Arc::clone(&hold), Arc::clone(&release));
        let _hook = install_db_section_test_hook(Arc::new(move |project: &SimlinProject| {
            if project as *const SimlinProject as usize == proj_addr
                && hold_in_hook.swap(false, Ordering::SeqCst)
            {
                let _ = entered_tx.send(());
                while !release_in_hook.load(Ordering::Acquire) {
                    thread::yield_now();
                }
            }
        }));
        struct Release(Arc<AtomicBool>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let release_guard = Release(Arc::clone(&release));

        let answering = thread::spawn(move || read_model_on(session_addr));
        entered_rx
            .recv_timeout(POSITIVE_WAIT)
            .expect("the reading call holds the session");
        let editing = thread::spawn(move || {
            trace::of(|| {
                let name = CString::new("edit_model").unwrap();
                let input = r#"{"summary": "a faster rate", "operations": [
                    {"op": "set_equation", "variable": "rate", "equation": "0.03"}]}"#;
                let (mut buf, mut len, mut is_error) = (ptr::null_mut(), 0usize, false);
                let mut err: *mut SimlinError = ptr::null_mut();
                crate::tools::simlin_tool_session_call(
                    session_addr as *mut crate::tools::SimlinToolSession,
                    name.as_ptr(),
                    input.as_ptr(),
                    input.len(),
                    &mut buf,
                    &mut len,
                    &mut is_error,
                    &mut err,
                );
                assert!(err.is_null());
                let output: serde_json::Value =
                    serde_json::from_slice(std::slice::from_raw_parts(buf, len)).unwrap();
                simlin_free(buf);
                (is_error, output)
            })
        });
        let deadline = std::time::Instant::now() + POSITIVE_WAIT;
        while (*session).calls_begun() < 3 && std::time::Instant::now() < deadline {
            thread::yield_now();
        }
        assert_eq!((*session).calls_begun(), 3, "the edit has begun");

        crate::tools::simlin_tool_session_cancel(session);
        drop(release_guard);
        let _ = answering.join().expect("the reading call finished");
        let ((is_error, output), events) = editing.join().expect("the edit's call finished");
        assert!(is_error, "{output}");
        assert_eq!(output["cancelled"], true, "{output}");
        assert_eq!(
            events,
            [Event::Acquire(Rank::Session), Event::Release(Rank::Session)],
            "it took the session and nothing of the project's"
        );
        let mut revision = u64::MAX;
        simlin_project_get_revision(proj, &mut revision, &mut err);
        assert!(err.is_null());
        assert_eq!(revision, 0, "the edit was not made");
        assert!(!(*proj).is_waited_on(), "no one is left counted as waiting");

        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}

/// An edit is the work other calls stop for, so it stops for none: one whose
/// gate runs while a person's query waits for the database is made, not
/// refused as interrupted. The waiter is the production count a waiting
/// accessor holds (`SimlinProject::count_waiting`, what `lock_counted` takes
/// while it waits), held for the length of the call; that a reading call
/// stops for it is checked first.
#[cfg(feature = "agent_tools")]
#[test]
fn an_edit_is_made_while_a_persons_query_waits_for_the_database() {
    let datamodel = TestProject::new("edit_with_waiter")
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.02", None)
        .build_datamodel();
    let proj = open_project_from_datamodel(&datamodel);
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let model = simlin_project_get_model(proj, ptr::null(), &mut err);
        assert!(err.is_null());
        let session = crate::tools::simlin_tool_session_new(model, &mut err);
        assert!(err.is_null());
        let session_addr = session as usize;
        let (refused, outline) = read_model_on(session_addr);
        assert!(!refused, "{outline}");

        let waiter = (*proj).count_waiting();
        let (refused, outline) = read_model_on(session_addr);
        assert!(refused, "a reading call stops for the waiter: {outline}");
        assert_eq!(outline["interrupted"], true, "{outline}");

        let name = CString::new("edit_model").unwrap();
        let input = serde_json::json!({"summary": "a faster rate", "operations": [
            {"op": "set_equation", "variable": "rate", "equation": "0.03"}
        ]})
        .to_string();
        let (mut buf, mut len, mut is_error) = (ptr::null_mut(), 0usize, false);
        crate::tools::simlin_tool_session_call(
            session,
            name.as_ptr(),
            input.as_ptr(),
            input.len(),
            &mut buf,
            &mut len,
            &mut is_error,
            &mut err,
        );
        assert!(err.is_null());
        let output: serde_json::Value =
            serde_json::from_slice(std::slice::from_raw_parts(buf, len)).unwrap();
        simlin_free(buf);
        assert!(!is_error, "the edit is made: {output}");
        assert_eq!((*proj).datamodel.lock().unwrap().revision(), 1);
        drop(waiter);

        crate::tools::simlin_tool_session_unref(session);
        simlin_model_unref(model);
        simlin_project_unref(proj);
    }
}
