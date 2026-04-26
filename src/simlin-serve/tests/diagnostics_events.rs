// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Integration tests for the `WsMessage::DiagnosticsChanged` notification.
//!
//! These exercise every merge surface that should emit the notification:
//! the HTTP save handler, the MCP `RegistryAccess::save` and `::create`
//! paths, and the file watcher's `handle_model_change`. Each test
//! subscribes to the EventBus before triggering work, then asserts the
//! exact ordering and payload of the resulting messages.
//!
//! Ordering invariant under test: when both `ProjectChanged` and
//! `DiagnosticsChanged` are emitted for one operation, `ProjectChanged`
//! arrives first. Subscribers (notably the MCP per-session forwarder)
//! depend on this so they never see "diagnostics for state X" before
//! "state has advanced to X".

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use simlin_mcp_core::access::ProjectAccess;
use simlin_mcp_core::types::SourceFormat;
use simlin_serve::build_router;
use simlin_serve::events::{ChangeSource, EventBus, WsMessage};
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::mcp::RegistryAccess;
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use tempfile::TempDir;
use tokio::sync::broadcast::Receiver;
use tower::ServiceExt;

/// Minimal valid sd.json: one model, no variables. Should produce no
/// error diagnostics.
const CLEAN_SD_JSON: &str = r#"{
    "name": "demo",
    "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
    "models": [{"name": "main"}]
}"#;

/// A project with a single auxiliary referencing an undefined identifier.
/// Will produce an `unknown_dependency` diagnostic on `bad`.
const BROKEN_SD_JSON: &str = r#"{
    "name": "demo",
    "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
    "models": [{
        "name": "main",
        "auxiliaries": [
            {"name": "bad", "equation": "1 + bogus"}
        ]
    }]
}"#;

/// `BROKEN_SD_JSON` with the equation fixed.
const FIXED_SD_JSON: &str = r#"{
    "name": "demo",
    "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
    "models": [{
        "name": "main",
        "auxiliaries": [
            {"name": "bad", "equation": "1 + 1"}
        ]
    }]
}"#;

/// Same as `FIXED_SD_JSON` but with an extra clean variable. Diagnostically
/// identical (both clean), so a save from one to the other should NOT
/// emit `DiagnosticsChanged`.
const FIXED_PLUS_VAR_SD_JSON: &str = r#"{
    "name": "demo",
    "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
    "models": [{
        "name": "main",
        "auxiliaries": [
            {"name": "bad", "equation": "1 + 1"},
            {"name": "extra", "equation": "5"}
        ]
    }]
}"#;

// Synthetic ports for the host validator middleware (Phase 8 Task 8).
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

fn build_state(root: PathBuf) -> Arc<AppState> {
    Arc::new(AppState {
        registry: Arc::new(ProjectRegistry::new(root.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(root),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(String::new()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    })
}

fn seed_registry(state: &AppState, abs_path: &Path, format: ProjectFormat) {
    let metadata = fs::metadata(abs_path).expect("file exists");
    let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    state.registry.upsert(
        abs_path.to_path_buf(),
        ProjectMeta {
            path: PathBuf::new(),
            format,
            mtime,
            size: metadata.len(),
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: 0,
            last_diagnostic_keys: BTreeSet::new(),
        },
    );
}

async fn http_post_save(state: AppState, uri: &str, body: Value) -> (StatusCode, Vec<u8>) {
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .expect("request"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .expect("body bytes");
    (status, body.to_vec())
}

/// Receive the next event matching `predicate` within `timeout`.
/// Used so a test can wait for a specific kind of event (e.g.
/// `DiagnosticsChanged` only) without being thrown by intervening events.
async fn await_event<F>(rx: &mut Receiver<WsMessage>, predicate: F) -> WsMessage
where
    F: Fn(&WsMessage) -> bool,
{
    let timeout = std::time::Duration::from_secs(2);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(std::time::Duration::from_millis(1));
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(msg)) if predicate(&msg) => return msg,
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => panic!("event bus error: {e}"),
            Err(_) => panic!("timed out waiting for matching event"),
        }
    }
}

/// Drain pending events for a brief window; assert nothing matching
/// `predicate` arrives. Used to enforce the "no `DiagnosticsChanged`"
/// branch of scenario 3.
async fn assert_no_event_for<F>(rx: &mut Receiver<WsMessage>, predicate: F, window_ms: u64)
where
    F: Fn(&WsMessage) -> bool,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(window_ms);
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(std::time::Duration::from_millis(1));
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(msg)) if predicate(&msg) => panic!("expected no matching event, got {msg:?}"),
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
}

// ---- HTTP save path ----

#[tokio::test]
async fn http_save_emits_diagnostics_changed_after_project_changed_when_set_differs() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("model.sd.json");
    fs::write(&abs, BROKEN_SD_JSON).expect("write broken sd.json");
    let state = build_state(canonical_root);
    seed_registry(&state, &abs, ProjectFormat::SdJson);

    let mut rx = state.events.subscribe();

    // Save the same broken content back. validate_save accepts it (the
    // baseline already includes the unknown_dependency error so it
    // is not a "new" error introduced by this save). The merge succeeds
    // and DiagnosticsChanged should fire because the cached
    // last_diagnostic_keys was empty at insert time and the freshly
    // computed set has the unknown_dependency entry.
    let body = json!({
        "json": BROKEN_SD_JSON,
        "version": 0,
    });
    let (status, response_bytes) =
        http_post_save((*state).clone(), "/api/projects/model.sd.json", body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "save failed: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    // First event: ProjectChanged with version 1 from a User-source save.
    let first = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::ProjectChanged { .. })
    })
    .await;
    match first {
        WsMessage::ProjectChanged {
            path,
            version,
            source,
        } => {
            assert_eq!(path, "model.sd.json");
            assert_eq!(version, 1);
            assert_eq!(source, ChangeSource::User);
        }
        _ => unreachable!(),
    }

    // Second event: DiagnosticsChanged carrying the unknown_dependency error.
    let second = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::DiagnosticsChanged { .. })
    })
    .await;
    match second {
        WsMessage::DiagnosticsChanged { path, errors } => {
            assert_eq!(path, "model.sd.json");
            assert_eq!(errors.len(), 1, "expected one error, got {errors:?}");
            assert_eq!(errors[0].code, "unknown_dependency");
            assert_eq!(errors[0].variable_name.as_deref(), Some("bad"));
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn http_save_emits_diagnostics_changed_with_empty_errors_when_fix_lands() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("model.sd.json");
    fs::write(&abs, BROKEN_SD_JSON).expect("write broken sd.json");
    let state = build_state(canonical_root);
    seed_registry(&state, &abs, ProjectFormat::SdJson);

    // First save: persists the broken content and seeds last_diagnostic_keys
    // with the unknown_dependency entry. Drain its events.
    let body = json!({"json": BROKEN_SD_JSON, "version": 0});
    let (status, body_bytes) =
        http_post_save((*state).clone(), "/api/projects/model.sd.json", body).await;
    assert_eq!(status, StatusCode::OK, "first save: {body_bytes:?}");

    let mut rx = state.events.subscribe();

    // Second save: equation fixed → no errors. The cached set has one
    // entry; the new set is empty; DiagnosticsChanged should fire with
    // an empty error list.
    let body = json!({"json": FIXED_SD_JSON, "version": 1});
    let (status, response_bytes) =
        http_post_save((*state).clone(), "/api/projects/model.sd.json", body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "fix save failed: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    let pc = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::ProjectChanged { .. })
    })
    .await;
    match pc {
        WsMessage::ProjectChanged { version, .. } => assert_eq!(version, 2),
        _ => unreachable!(),
    }

    let dc = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::DiagnosticsChanged { .. })
    })
    .await;
    match dc {
        WsMessage::DiagnosticsChanged { path, errors } => {
            assert_eq!(path, "model.sd.json");
            assert!(
                errors.is_empty(),
                "expected empty error list after fix, got {errors:?}"
            );
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn http_save_does_not_emit_diagnostics_changed_when_set_unchanged() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("model.sd.json");
    fs::write(&abs, FIXED_SD_JSON).expect("write fixed sd.json");
    let state = build_state(canonical_root);
    seed_registry(&state, &abs, ProjectFormat::SdJson);

    // First save: keeps the project clean. Cached is empty, new is empty
    // → no DiagnosticsChanged. Drain ProjectChanged (and confirm absence).
    let body = json!({"json": FIXED_SD_JSON, "version": 0});
    let (status, body_bytes) =
        http_post_save((*state).clone(), "/api/projects/model.sd.json", body).await;
    assert_eq!(status, StatusCode::OK, "first save: {body_bytes:?}");

    let mut rx = state.events.subscribe();

    // Second save: adds a new (clean) variable. Diagnostically identical
    // to the prior state (both empty sets). DiagnosticsChanged must NOT
    // fire even though ProjectChanged does.
    let body = json!({"json": FIXED_PLUS_VAR_SD_JSON, "version": 1});
    let (status, response_bytes) =
        http_post_save((*state).clone(), "/api/projects/model.sd.json", body).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "second save failed: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    let pc = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::ProjectChanged { .. })
    })
    .await;
    match pc {
        WsMessage::ProjectChanged { version, .. } => assert_eq!(version, 2),
        _ => unreachable!(),
    }

    // Within a 200ms window after ProjectChanged, no DiagnosticsChanged
    // event should arrive. The helper is synchronous so any genuine
    // emit would already be in the channel by the time we observe
    // ProjectChanged.
    assert_no_event_for(
        &mut rx,
        |msg| matches!(msg, WsMessage::DiagnosticsChanged { .. }),
        200,
    )
    .await;
}

// ---- MCP save path ----

#[tokio::test]
async fn mcp_save_emits_diagnostics_changed_when_set_differs() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("model.sd.json");
    fs::write(&abs, BROKEN_SD_JSON).expect("write broken sd.json");
    let state = build_state(canonical_root);
    seed_registry(&state, &abs, ProjectFormat::SdJson);

    let access = RegistryAccess::new(state.clone());
    let mut rx = state.events.subscribe();

    // Open + save the same content. The MCP save's validation accepts it
    // (baseline matches), the merge increments the version, and the
    // DiagnosticsChanged emit fires because the cached set was empty and
    // the new set has the error.
    let opened = access.open(&abs).await.expect("open");
    let new_version = access
        .save(&abs, &opened.project, opened.source_format, Some(0))
        .await
        .expect("mcp save");
    assert_eq!(new_version, 1);

    let pc = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::ProjectChanged { .. })
    })
    .await;
    match pc {
        WsMessage::ProjectChanged {
            version, source, ..
        } => {
            assert_eq!(version, 1);
            assert_eq!(source, ChangeSource::Agent);
        }
        _ => unreachable!(),
    }

    let dc = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::DiagnosticsChanged { .. })
    })
    .await;
    match dc {
        WsMessage::DiagnosticsChanged { path, errors } => {
            assert_eq!(path, "model.sd.json");
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].code, "unknown_dependency");
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn mcp_create_emits_diagnostics_changed_when_new_project_has_errors() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root.clone());

    let access = RegistryAccess::new(state.clone());
    let mut rx = state.events.subscribe();

    // Build a broken project in-memory and create it on disk via MCP.
    // create() bypasses the baseline-validation gate (a brand-new file
    // has no baseline), so a project with errors lands successfully.
    // The fresh entry's last_diagnostic_keys defaults to empty; the
    // recompute against the just-written project produces the
    // unknown_dependency entry → DiagnosticsChanged fires.
    let json_project: simlin_engine::json::Project =
        serde_json::from_str(BROKEN_SD_JSON).expect("parse broken");
    let project: simlin_engine::datamodel::Project = json_project.into();
    let abs = canonical_root.join("new.sd.json");
    access
        .create(&abs, &project, SourceFormat::NativeJson)
        .await
        .expect("mcp create");

    let pc = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::ProjectChanged { .. })
    })
    .await;
    match pc {
        WsMessage::ProjectChanged {
            path,
            version,
            source,
        } => {
            assert_eq!(path, "new.sd.json");
            assert_eq!(version, 0);
            assert_eq!(source, ChangeSource::Agent);
        }
        _ => unreachable!(),
    }

    let dc = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::DiagnosticsChanged { .. })
    })
    .await;
    match dc {
        WsMessage::DiagnosticsChanged { path, errors } => {
            assert_eq!(path, "new.sd.json");
            assert_eq!(errors.len(), 1);
            assert_eq!(errors[0].code, "unknown_dependency");
        }
        _ => unreachable!(),
    }
}

#[tokio::test]
async fn mcp_create_does_not_emit_diagnostics_changed_for_clean_project() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root.clone());

    let access = RegistryAccess::new(state.clone());
    let mut rx = state.events.subscribe();

    // Clean project: cached set was empty (fresh entry) and computed set
    // is also empty → DiagnosticsChanged must not fire.
    let json_project: simlin_engine::json::Project =
        serde_json::from_str(CLEAN_SD_JSON).expect("parse clean");
    let project: simlin_engine::datamodel::Project = json_project.into();
    let abs = canonical_root.join("clean.sd.json");
    access
        .create(&abs, &project, SourceFormat::NativeJson)
        .await
        .expect("mcp create clean");

    let pc = await_event(&mut rx, |msg| {
        matches!(msg, WsMessage::ProjectChanged { .. })
    })
    .await;
    match pc {
        WsMessage::ProjectChanged { path, .. } => {
            assert_eq!(path, "clean.sd.json");
        }
        _ => unreachable!(),
    }

    assert_no_event_for(
        &mut rx,
        |msg| matches!(msg, WsMessage::DiagnosticsChanged { .. }),
        200,
    )
    .await;
}

// ---- Watcher path ----

#[tokio::test]
async fn watcher_merge_does_not_emit_diagnostics_changed_when_both_states_clean() {
    use simlin_serve::hashing::content_hash;
    use simlin_serve::watcher::{ShutdownSignal, spawn_watcher};
    use tokio::sync::Notify;
    use tokio::time::Duration;

    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("model.sd.json");
    // Start with a clean file. Register the initial bytes' hash as the
    // last_disk_hash so an exact rewrite would echo-suppress; we need
    // the watcher to actually merge so we'll write *different* (but
    // still clean) bytes below.
    fs::write(&abs, FIXED_SD_JSON).expect("write clean sd.json");
    let state = build_state(canonical_root);
    let initial_hash = content_hash(FIXED_SD_JSON.as_bytes());
    state.registry.upsert(
        abs.clone(),
        ProjectMeta {
            path: PathBuf::new(),
            format: ProjectFormat::SdJson,
            mtime: SystemTime::UNIX_EPOCH,
            size: 0,
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: initial_hash,
            last_diagnostic_keys: BTreeSet::new(),
        },
    );

    // Hydrate the doc so the watcher's baseline check sees a clean
    // pre-state. Without this, the freshly-discovered file's empty
    // baseline would reject any new-error introduction; we need the
    // baseline to match the on-disk clean state.
    state
        .registry
        .get_or_init_doc(&abs)
        .expect("hydrate clean baseline");

    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher((*state).clone(), shutdown.clone()).expect("spawn watcher");

    // Let the OS watch register before we trigger the disk edit.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Write structurally-different but still clean content. The
    // watcher merges, ProjectChanged{Disk} fires, and the diagnostic
    // set goes from empty to empty → no DiagnosticsChanged.
    fs::write(&abs, FIXED_PLUS_VAR_SD_JSON).expect("write disk-edited content");

    // Wait up to 5s for the disk-source ProjectChanged.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(WsMessage::ProjectChanged {
                    source: ChangeSource::Disk,
                    ..
                }) => return,
                Ok(_) => continue,
                Err(e) => panic!("event bus error: {e}"),
            }
        }
    })
    .await
    .expect("project changed (disk) within timeout");

    // No DiagnosticsChanged should follow within a 200ms window.
    assert_no_event_for(
        &mut rx,
        |msg| matches!(msg, WsMessage::DiagnosticsChanged { .. }),
        200,
    )
    .await;

    shutdown.notify_waiters();
}

#[tokio::test]
async fn watcher_merge_emits_diagnostics_changed_when_disk_fixes_existing_errors() {
    use simlin_serve::hashing::content_hash;
    use simlin_serve::watcher::{ShutdownSignal, spawn_watcher};
    use tokio::sync::Notify;
    use tokio::time::Duration;

    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("model.sd.json");
    // Start with broken content on disk; the in-memory doc hydrates from
    // it, picking up the unknown_dependency diagnostic. After the first
    // hydrate + maybe-emit, the cached set has one entry.
    fs::write(&abs, BROKEN_SD_JSON).expect("write broken sd.json");
    let state = build_state(canonical_root);
    let initial_hash = content_hash(BROKEN_SD_JSON.as_bytes());
    state.registry.upsert(
        abs.clone(),
        ProjectMeta {
            path: PathBuf::new(),
            format: ProjectFormat::SdJson,
            mtime: SystemTime::UNIX_EPOCH,
            size: 0,
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: initial_hash,
            last_diagnostic_keys: BTreeSet::new(),
        },
    );

    // Hydrate the doc and prime the cached diagnostic set so the
    // watcher's post-merge maybe_emit observes the transition from
    // non-empty to empty (rather than empty to non-empty, which the
    // validate_save_project gate blocks for net-new errors anyway).
    state
        .registry
        .get_or_init_doc(&abs)
        .expect("hydrate broken state");
    let mut keys = BTreeSet::new();
    keys.insert(("unknown_dependency".to_string(), Some("bad".to_string())));
    state
        .registry
        .update_diagnostic_keys_if_changed(&abs, &keys);

    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher((*state).clone(), shutdown.clone()).expect("spawn watcher");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Disk-edit to the fixed content. Watcher merges, ProjectChanged
    // fires, then DiagnosticsChanged with errors:[] since the cached
    // set went from one entry to empty.
    fs::write(&abs, FIXED_SD_JSON).expect("write fix");

    let pc = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rx.recv().await {
                Ok(
                    msg @ WsMessage::ProjectChanged {
                        source: ChangeSource::Disk,
                        ..
                    },
                ) => return msg,
                Ok(_) => continue,
                Err(e) => panic!("event bus error: {e}"),
            }
        }
    })
    .await
    .expect("project changed (disk) within timeout");
    let _ = pc;

    let dc = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match rx.recv().await {
                Ok(msg @ WsMessage::DiagnosticsChanged { .. }) => return msg,
                Ok(_) => continue,
                Err(e) => panic!("event bus error: {e}"),
            }
        }
    })
    .await
    .expect("diagnostics changed within timeout");
    match dc {
        WsMessage::DiagnosticsChanged { path, errors } => {
            assert_eq!(path, "model.sd.json");
            assert!(
                errors.is_empty(),
                "expected empty errors after disk fix, got {errors:?}"
            );
        }
        _ => unreachable!(),
    }

    shutdown.notify_waiters();
}
