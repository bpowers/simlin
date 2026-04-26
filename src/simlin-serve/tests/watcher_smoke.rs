// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Smoke tests for the file-watcher actor (Phase 4 Task 2).
//!
//! These tests verify the watcher's plumbing only — that creating a file
//! under the watched root produces at least one debounced event arriving
//! through the actor's channel, and that the shutdown signal terminates
//! the loop. The merge logic that consumes those events is tested
//! separately in `watcher_merge.rs` (Subcomponent B).

#![deny(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::watcher::{ShutdownSignal, spawn_watcher};
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::mpsc;

/// Helper: build an `AppState` rooted at `dir`.
fn build_app_state(dir: &std::path::Path) -> AppState {
    let canonical = dir.canonicalize().expect("canonicalize");
    AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new("smoke-token".to_string()),
    }
}

#[tokio::test]
async fn watcher_emits_debounced_event_for_new_file() {
    let dir = TempDir::new().expect("tempdir");
    let state = build_app_state(dir.path());
    let shutdown: ShutdownSignal = Arc::new(Notify::new());

    // The smoke test installs a side-channel hook so it can observe
    // raw debounced batches without depending on the merge path that
    // Subcomponent B will fill in.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let probe = simlin_serve::watcher::TestHook::new(tx);
    let _handle =
        spawn_watcher(state.clone(), shutdown.clone(), Some(probe)).expect("spawn watcher");

    // The watcher needs the directory to exist before `watch()` is called,
    // which it does (TempDir::new creates it). Give the OS-level watch a
    // moment to register; otherwise the file write below races the watch
    // setup and the event never arrives.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let target = state.root.join("foo.stmx");
    tokio::fs::write(&target, b"<xmile/>")
        .await
        .expect("write file");

    // The debouncer's tick rate is 25ms (timeout/4 = 100ms/4); we wait up
    // to 500ms which is generous enough for any platform-specific delay.
    let received = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .expect("watcher emitted nothing within 500ms")
        .expect("hook channel closed");

    // The hook delivers the raw DebounceEventResult. We just need a non-empty
    // success batch — classification is exercised in Task 3's unit tests.
    let events = received.expect("debouncer surfaced an error result");
    assert!(
        !events.is_empty(),
        "debouncer must produce at least one event for a new file"
    );

    shutdown.notify_waiters();
}

#[tokio::test]
async fn watcher_shutdown_signal_terminates_actor() {
    let dir = TempDir::new().expect("tempdir");
    let state = build_app_state(dir.path());
    let shutdown: ShutdownSignal = Arc::new(Notify::new());

    let handle = spawn_watcher(state.clone(), shutdown.clone(), None).expect("spawn watcher");

    // Trigger shutdown immediately.
    shutdown.notify_waiters();

    // The actor should exit within a tick cycle (timeout/4 = 25ms);
    // 500ms is the same generous bound the previous test uses.
    tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("watcher actor did not exit within 500ms")
        .expect("watcher actor task panicked");
}
