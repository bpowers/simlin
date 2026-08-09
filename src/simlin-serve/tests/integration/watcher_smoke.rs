// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Lifecycle test for the file-watcher actor (Phase 4 Task 2).
//!
//! The debouncer → actor → handler pipeline is covered end to end in
//! `watcher_merge.rs`, which asserts the registry side effects as well as
//! the broadcast. What is left here is the half that suite never
//! exercises: that the actor actually stops when told to.

use std::sync::Arc;
use std::time::Duration;

use simlin_serve::events::EventBus;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::test_support::unavailable_git_probe;
use simlin_serve::watcher::{ShutdownSignal, spawn_watcher};
use tempfile::TempDir;
use tokio::sync::Notify;

/// Helper: build an `AppState` rooted at `dir`.
fn build_app_state(dir: &std::path::Path) -> AppState {
    let canonical = dir.canonicalize().expect("canonicalize");
    AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(unavailable_git_probe()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        // Watcher tests don't go through the HTTP layer, so the host
        // validator is never consulted; ports are placeholders.
        ui_port: 0,
        mcp_port: 0,
        strict_origin: true,
    }
}

#[tokio::test]
async fn watcher_shutdown_signal_terminates_actor() {
    let dir = TempDir::new().expect("tempdir");
    let state = build_app_state(dir.path());
    let shutdown: ShutdownSignal = Arc::new(Notify::new());

    let handle = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    // Trigger shutdown immediately.
    shutdown.notify_waiters();

    // The actor should exit within a tick cycle (timeout/4 = 25ms);
    // 500ms is a generous bound for slow CI machines.
    tokio::time::timeout(Duration::from_millis(500), handle.into_join_handle())
        .await
        .expect("watcher actor did not exit within 500ms")
        .expect("watcher actor task panicked");
}
