// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Smoke tests for the file-watcher actor (Phase 4 Task 2).
//!
//! These tests verify the watcher's plumbing by observing the EventBus:
//! creating a file under the watched root must produce a `ProjectChanged`
//! event, proving the debouncer → actor → handler pipeline is wired up end
//! to end. The merge logic is tested separately in `watcher_merge.rs`.

#![deny(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use simlin_serve::events::{ChangeSource, EventBus, WsMessage};
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::watcher::{ShutdownSignal, spawn_watcher};
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;

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

/// Wait for any `ProjectChanged { source: Disk }` event on `rx`.
async fn await_disk_changed(
    rx: &mut tokio::sync::broadcast::Receiver<WsMessage>,
    timeout: Duration,
) -> Option<WsMessage> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(
                msg @ WsMessage::ProjectChanged {
                    source: ChangeSource::Disk,
                    ..
                },
            )) => return Some(msg),
            Ok(Ok(_other)) => continue,
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => return None,
            Err(_) => return None,
        }
    }
}

#[tokio::test]
async fn watcher_emits_project_changed_for_new_file() {
    let dir = TempDir::new().expect("tempdir");
    let state = build_app_state(dir.path());
    let shutdown: ShutdownSignal = Arc::new(Notify::new());

    let mut rx = state.events.subscribe();

    let _handle = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    // Give the OS-level watch a moment to register before writing.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // The minimal sd.json the watcher's parse path accepts.
    let content = r#"{"name":"smoke","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
    let target = state.root.join("smoke.sd.json");
    tokio::fs::write(&target, content)
        .await
        .expect("write file");

    // The debouncer window is 100ms; we wait up to 2s to be robust on
    // slow CI machines.
    let event = await_disk_changed(&mut rx, Duration::from_secs(2))
        .await
        .expect("watcher must emit ProjectChanged{source: Disk} within 2s");
    match event {
        WsMessage::ProjectChanged { source, .. } => {
            assert_eq!(source, ChangeSource::Disk);
        }
        WsMessage::ProjectRemoved { .. } => panic!("expected ProjectChanged, got ProjectRemoved"),
    }

    shutdown.notify_waiters();
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
    tokio::time::timeout(Duration::from_millis(500), handle)
        .await
        .expect("watcher actor did not exit within 500ms")
        .expect("watcher actor task panicked");
}
