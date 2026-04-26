// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for the watcher's disk -> Loro merge path (Phase 4 Task 5).
//!
//! These exercise the full read -> hash-compare -> parse -> validate -> merge
//! pipeline by wiring up an `EventBus` subscriber, externally mutating a file
//! under the watched root, and waiting for `ProjectChanged { source: Disk }`
//! to land. AC4.2 (disk-driven update) and AC4.4 (byte-identical
//! echo-suppression) are both covered here. AC4.3 (browser+disk concurrent
//! edits both preserved) is also covered: the test seeds an in-memory edit
//! through the registry's `check_increment_and_merge` primitive, then triggers
//! a disk edit, and asserts both edits are visible in the merged doc.

#![deny(unsafe_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use simlin_serve::build_router;
use simlin_serve::events::{ChangeSource, EventBus, WsMessage};
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::hashing::content_hash;
use simlin_serve::registry::{
    GitState, ProjectFormat, ProjectMeta, ProjectRegistry, RegistryError,
};
use simlin_serve::watcher::{ShutdownSignal, spawn_watcher};
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;
use tower::ServiceExt;

// Synthetic ports for the host validator middleware (Phase 8 Task 8).
// The save below uses these in its `Host:` header.
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

/// Helper: build an `AppState` rooted at `dir` with a fresh registry, no
/// git visibility, and an `EventBus`.
fn build_state(dir: &Path) -> AppState {
    let canonical = dir.canonicalize().expect("canonicalize");
    AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new("watcher-merge-token".to_string()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    }
}

/// Helper: seed a registry entry for `abs_path`. Mirrors the saved-from-disk
/// state without going through discovery (the watcher tests want a controlled
/// pre-state).
fn seed_registry(state: &AppState, abs_path: &Path, format: ProjectFormat, hash: u64) {
    let metadata = std::fs::metadata(abs_path).expect("file exists");
    let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    state.registry.upsert(
        abs_path.to_path_buf(),
        ProjectMeta {
            path: std::path::PathBuf::new(),
            format,
            mtime,
            size: metadata.len(),
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: hash,
            last_diagnostic_keys: std::collections::BTreeSet::new(),
        },
    );
}

/// Wait for the next `ProjectChanged { source: Disk }` event. Bounds the
/// wait to `timeout` so a misbehaving watcher fails the test rather than
/// hanging indefinitely.
async fn await_disk_event(
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

/// Wait for the next `ProjectRemoved` event under the same bounded-wait
/// rules as `await_disk_event`.
async fn await_removed_event(
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
            Ok(Ok(msg @ WsMessage::ProjectRemoved { .. })) => return Some(msg),
            Ok(Ok(_other)) => continue,
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => return None,
            Err(_) => return None,
        }
    }
}

/// Wait for the next `ProjectRenamed` event under the same bounded-wait
/// rules as `await_disk_event`.
async fn await_renamed_event(
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
            Ok(Ok(msg @ WsMessage::ProjectRenamed { .. })) => return Some(msg),
            Ok(Ok(_other)) => continue,
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => return None,
            Err(_) => return None,
        }
    }
}

/// Minimal `.sd.json` content with a single named project. The disk-merge tests
/// mutate `name` to force a merge and observe via the doc state.
fn sd_json(name: &str) -> String {
    serde_json::json!({
        "name": name,
        "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
        "models": [{"name": "main"}]
    })
    .to_string()
}

/// `.sd.json` with two stocks; AC4.3 mutates each stock independently
/// (one via the registry merge primitive, the other via disk).
fn sd_json_with_two_stocks(s1_eq: &str, s2_eq: &str) -> String {
    serde_json::json!({
        "name": "demo",
        "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
        "models": [{
            "name": "main",
            "stocks": [
                {"name": "S1", "initialEquation": s1_eq, "inflows": [], "outflows": []},
                {"name": "S2", "initialEquation": s2_eq, "inflows": [], "outflows": []}
            ]
        }]
    })
    .to_string()
}

/// AC4.2: external `.sd.json` mutation triggers a `ProjectChanged` event
/// with `source: Disk`. The merged in-memory doc reflects the disk state.
#[tokio::test]
async fn external_disk_edit_triggers_disk_source_broadcast() {
    let dir = TempDir::new().expect("tempdir");
    let abs = dir.path().join("model.sd.json");
    let initial = sd_json("baseline");
    std::fs::write(&abs, &initial).expect("write initial");
    // Canonicalize abs after the write so the registry key matches what
    // the watcher's classify will produce (canonicalization needs the
    // file to exist).
    let abs_canonical = abs.canonicalize().expect("canonicalize abs");

    let state = build_state(dir.path());
    seed_registry(
        &state,
        &abs_canonical,
        ProjectFormat::SdJson,
        content_hash(initial.as_bytes()),
    );
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    // Give the OS-level watch a moment to register; otherwise the file
    // write below races the watch setup and the event never arrives.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let updated = sd_json("renamed-on-disk");
    tokio::fs::write(&abs, &updated)
        .await
        .expect("write update");

    let event = await_disk_event(&mut rx, Duration::from_secs(2))
        .await
        .expect("watcher emitted ProjectChanged{source: Disk} within 2s");
    match event {
        WsMessage::ProjectChanged { source, .. } => {
            assert_eq!(source, ChangeSource::Disk);
        }
        other => panic!("expected ProjectChanged, got {other:?}"),
    }

    // The doc reflects the disk state.
    let doc = state.registry.get_or_init_doc(&abs_canonical).expect("doc");
    let exported = doc.export_canonical_json().expect("export");
    assert_eq!(exported["name"].as_str(), Some("renamed-on-disk"));

    shutdown.notify_waiters();
}

/// AC4.4: an atomic-write whose bytes are byte-identical to the cached
/// `last_disk_hash` does NOT trigger a re-merge. The version stays put,
/// no `ProjectChanged{source: Disk}` is broadcast.
#[tokio::test]
async fn echo_suppression_skips_byte_identical_disk_writes() {
    let dir = TempDir::new().expect("tempdir");
    let abs = dir.path().join("model.sd.json");
    let initial = sd_json("baseline");
    std::fs::write(&abs, &initial).expect("write initial");
    let abs_canonical = abs.canonicalize().expect("canonicalize abs");

    let state = build_state(dir.path());
    let baseline_hash = content_hash(initial.as_bytes());
    seed_registry(&state, &abs_canonical, ProjectFormat::SdJson, baseline_hash);
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Write the same bytes back. The watcher should see the event and
    // short-circuit because content_hash(bytes) == last_disk_hash.
    tokio::fs::write(&abs, &initial).await.expect("write echo");

    // Wait long enough for the debouncer to flush + process; then assert
    // that no Disk-source event arrived.
    let no_event = tokio::time::timeout(
        Duration::from_millis(800),
        await_disk_event(&mut rx, Duration::from_millis(800)),
    )
    .await;
    if let Ok(Some(_)) = no_event {
        panic!("byte-identical disk write must not produce a Disk broadcast");
    }

    // Version still 0 (unchanged), confirming no merge ran.
    let entry = state.registry.get(&abs_canonical).expect("entry");
    assert_eq!(
        entry.version, 0,
        "echo-suppressed write must not bump version"
    );

    shutdown.notify_waiters();
}

/// AC4.3: A registry-driven edit (simulating a browser save through the
/// merge primitive) plus an external disk edit on a different stock both
/// land in the merged doc. Per-variable LWW from the Loro doc keeps both
/// stocks' new equations intact.
#[tokio::test]
async fn browser_and_disk_edits_both_preserved_via_merge() {
    let dir = TempDir::new().expect("tempdir");
    let abs = dir.path().join("two_stocks.sd.json");
    let initial = sd_json_with_two_stocks("0", "0");
    std::fs::write(&abs, &initial).expect("write initial");
    let abs_canonical = abs.canonicalize().expect("canonicalize abs");

    let state = build_state(dir.path());
    seed_registry(
        &state,
        &abs_canonical,
        ProjectFormat::SdJson,
        content_hash(initial.as_bytes()),
    );
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Simulate a browser save through the merge primitive: S1 gets
    // initialEquation = "100", S2 stays at "0". Then "echo" the result
    // to disk under the matching last_disk_hash so the watcher won't
    // re-merge our own bytes.
    let browser_edit: serde_json::Value =
        serde_json::from_str(&sd_json_with_two_stocks("100", "0")).expect("parse browser edit");
    state
        .registry
        .check_increment_and_merge(&abs_canonical, 0, &browser_edit)
        .expect("browser merge succeeds");
    let on_disk_after_browser_save = sd_json_with_two_stocks("100", "0");
    let echo_hash = content_hash(on_disk_after_browser_save.as_bytes());
    std::fs::write(&abs, &on_disk_after_browser_save).expect("echo browser save to disk");
    // Refresh meta so the next watcher event sees a matching hash for
    // the browser-save echo. This mirrors what the save handler does
    // in production via refresh_after_write.
    let metadata = std::fs::metadata(&abs_canonical).expect("metadata");
    state.registry.refresh_after_write(
        &abs_canonical,
        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        metadata.len(),
        echo_hash,
    );

    // Now an external editor reads the post-browser-save file (S1="100",
    // S2="0"), bumps S2 to "200", and writes back. Crucially the disk
    // bytes still carry S1="100" because the editor reads from disk.
    // The merge must preserve S1 (no churn) and apply S2's new value.
    let disk_edit = sd_json_with_two_stocks("100", "200");
    tokio::fs::write(&abs, &disk_edit)
        .await
        .expect("write disk edit");

    let event = await_disk_event(&mut rx, Duration::from_secs(2))
        .await
        .expect("watcher fires Disk-source ProjectChanged within 2s");
    match event {
        WsMessage::ProjectChanged { source, .. } => assert_eq!(source, ChangeSource::Disk),
        other => panic!("expected ProjectChanged, got {other:?}"),
    }

    // After both edits the merged doc must show S1="100" (browser edit
    // preserved across the disk merge) AND S2="200" (disk edit applied).
    // This is the property AC4.3 names "browser-side in-flight edits
    // are preserved across an external disk edit".
    let doc = state.registry.get_or_init_doc(&abs_canonical).expect("doc");
    let exported = doc.export_canonical_json().expect("export");
    let stocks = exported["models"][0]["stocks"]
        .as_array()
        .expect("stocks array");
    let s1 = stocks
        .iter()
        .find(|v| v["name"] == "S1")
        .expect("S1 present");
    let s2 = stocks
        .iter()
        .find(|v| v["name"] == "S2")
        .expect("S2 present");
    assert_eq!(s1["initialEquation"], "100", "browser edit on S1 preserved");
    assert_eq!(s2["initialEquation"], "200", "disk edit on S2 applied");

    shutdown.notify_waiters();
}

/// Negative test: an external write that produces invalid JSON does NOT
/// merge. The in-memory doc stays at its last-known-good state, and no
/// `ProjectChanged{source: Disk}` is broadcast.
#[tokio::test]
async fn invalid_json_disk_write_does_not_merge() {
    let dir = TempDir::new().expect("tempdir");
    let abs = dir.path().join("model.sd.json");
    let initial = sd_json("baseline");
    std::fs::write(&abs, &initial).expect("write initial");
    let abs_canonical = abs.canonicalize().expect("canonicalize abs");

    let state = build_state(dir.path());
    seed_registry(
        &state,
        &abs_canonical,
        ProjectFormat::SdJson,
        content_hash(initial.as_bytes()),
    );
    // Hydrate the doc so we can compare pre/post state.
    state
        .registry
        .get_or_init_doc(&abs_canonical)
        .expect("hydrate doc");
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Write garbage that's not valid JSON.
    tokio::fs::write(&abs, b"this is not json {{{")
        .await
        .expect("write garbage");

    // No ProjectChanged{Disk} should arrive.
    let no_event = tokio::time::timeout(
        Duration::from_millis(800),
        await_disk_event(&mut rx, Duration::from_millis(800)),
    )
    .await;
    if let Ok(Some(_)) = no_event {
        panic!("invalid disk write must not produce a Disk broadcast");
    }

    // Version unchanged; doc still reflects the baseline.
    let entry = state.registry.get(&abs_canonical).expect("entry");
    assert_eq!(entry.version, 0);
    let doc = state.registry.get_or_init_doc(&abs_canonical).expect("doc");
    let exported = doc.export_canonical_json().expect("export");
    assert_eq!(exported["name"].as_str(), Some("baseline"));

    shutdown.notify_waiters();
}

/// Sidecar-override case: a `.mdl` event is ignored when a sibling
/// `.sd.json` exists (sidecar is canonical). Watcher must not parse the
/// `.mdl`, must not broadcast.
#[tokio::test]
async fn mdl_event_ignored_when_sidecar_exists() {
    let dir = TempDir::new().expect("tempdir");
    let mdl = dir.path().join("model.mdl");
    let sidecar = dir.path().join("model.sd.json");
    std::fs::write(&mdl, b"{UTF-8}\n\nplaceholder=1\n  ~\n  ~|\n").expect("write mdl");
    std::fs::write(&sidecar, sd_json("from-sidecar")).expect("write sidecar");
    let sidecar_canonical = sidecar.canonicalize().expect("canonicalize sidecar");

    let state = build_state(dir.path());
    let initial_sidecar_bytes = std::fs::read(&sidecar_canonical).expect("read sidecar");
    seed_registry(
        &state,
        &sidecar_canonical,
        ProjectFormat::SdJson,
        content_hash(&initial_sidecar_bytes),
    );
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Touch the .mdl file. Sidecar exists -> the event must be ignored.
    tokio::fs::write(&mdl, b"{UTF-8}\n\nupdated_value=2\n  ~\n  ~|\n")
        .await
        .expect("touch mdl");

    let no_event = tokio::time::timeout(
        Duration::from_millis(800),
        await_disk_event(&mut rx, Duration::from_millis(800)),
    )
    .await;
    if let Ok(Some(_)) = no_event {
        panic!("mdl event with sidecar present must not produce a Disk broadcast");
    }

    shutdown.notify_waiters();
}

/// Created-on-a-fresh-path: a new `.stmx` appearing in the watched root
/// gets a registry entry and a `ProjectChanged{source: Disk}` event.
#[tokio::test]
async fn create_event_for_new_path_adds_registry_entry_and_broadcasts() {
    let dir = TempDir::new().expect("tempdir");
    let state = build_state(dir.path());
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Create a brand-new .sd.json that's not yet in the registry.
    let new_path = state.root.join("brand_new.sd.json");
    tokio::fs::write(&new_path, sd_json("freshly-created"))
        .await
        .expect("create file");

    let event = await_disk_event(&mut rx, Duration::from_secs(2))
        .await
        .expect("watcher must broadcast for new file");
    match event {
        WsMessage::ProjectChanged { source, .. } => assert_eq!(source, ChangeSource::Disk),
        other => panic!("expected ProjectChanged, got {other:?}"),
    }

    // Registry now has the entry.
    let new_canonical = new_path.canonicalize().expect("canonicalize new");
    let entry = state
        .registry
        .get(&new_canonical)
        .expect("registry has the new entry");
    assert_eq!(entry.format, ProjectFormat::SdJson);

    shutdown.notify_waiters();
}

/// AC4 closeout: deleting a model file from disk drops the registry
/// entry and broadcasts `ProjectRemoved` so the SPA can drop the entry
/// from its sidebar.
#[tokio::test]
async fn external_remove_drops_registry_entry_and_broadcasts_removed() {
    let dir = TempDir::new().expect("tempdir");
    let abs = dir.path().join("doomed.sd.json");
    let initial = sd_json("baseline");
    std::fs::write(&abs, &initial).expect("write initial");
    let abs_canonical = abs.canonicalize().expect("canonicalize abs");

    let state = build_state(dir.path());
    seed_registry(
        &state,
        &abs_canonical,
        ProjectFormat::SdJson,
        content_hash(initial.as_bytes()),
    );
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::fs::remove_file(&abs)
        .await
        .expect("remove the model file");

    let event = await_removed_event(&mut rx, Duration::from_secs(2))
        .await
        .expect("watcher must broadcast ProjectRemoved within 2s");
    match event {
        WsMessage::ProjectRemoved { path } => {
            assert_eq!(path, "doomed.sd.json");
        }
        other => panic!("expected ProjectRemoved, got {other:?}"),
    }

    // Registry no longer has the entry.
    assert!(
        state.registry.get(&abs_canonical).is_none(),
        "registry must drop the entry after the file is removed"
    );

    shutdown.notify_waiters();
}

/// Removing a path the registry never knew about is a no-op and produces
/// no `ProjectRemoved` event. The watcher's `Removed` arm goes through
/// `registry.remove` (which is a no-op for missing keys) and the
/// broadcast surface stays clean for unrelated files.
#[tokio::test]
async fn remove_of_untracked_path_is_silent() {
    let dir = TempDir::new().expect("tempdir");
    let abs = dir.path().join("never_tracked.sd.json");
    let initial = sd_json("baseline");
    std::fs::write(&abs, &initial).expect("write initial");

    let state = build_state(dir.path());
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::fs::remove_file(&abs).await.expect("remove the file");

    let no_event = tokio::time::timeout(
        Duration::from_millis(800),
        await_removed_event(&mut rx, Duration::from_millis(800)),
    )
    .await;
    if let Ok(Some(_)) = no_event {
        panic!("removing an untracked path must not produce a ProjectRemoved broadcast");
    }

    shutdown.notify_waiters();
}

/// Invariant: `merge_disk_change` is the registry primitive the watcher
/// uses; it must reject paths the registry doesn't yet know about with
/// NotFound.
#[test]
fn merge_disk_change_returns_not_found_when_registry_has_no_entry() {
    let dir = TempDir::new().expect("tempdir");
    let canonical = dir.path().canonicalize().expect("canonicalize");
    let registry = ProjectRegistry::new(canonical.clone());
    let err = registry
        .merge_disk_change(&canonical.join("not-tracked.stmx"), &serde_json::json!({}))
        .unwrap_err();
    assert_eq!(err, RegistryError::NotFound);
}

/// AC4.4 (save path): a POST /api/projects save must NOT produce a
/// `ProjectChanged { source: Disk }` event — the watcher echo-suppression
/// must catch the atomic_write and suppress the re-merge. This tests the
/// pre-write hash ordering fix: `prime_echo_hash` runs before
/// `commit_write` so the hash is already in the registry when the OS
/// event fires.
#[tokio::test]
async fn save_handler_atomic_write_does_not_produce_disk_source_event() {
    let dir = TempDir::new().expect("tempdir");
    let abs = dir.path().join("model.sd.json");
    let initial = sd_json("save-echo-test");
    std::fs::write(&abs, &initial).expect("write initial");
    let abs_canonical = abs.canonicalize().expect("canonicalize abs");

    let state = build_state(dir.path());
    seed_registry(
        &state,
        &abs_canonical,
        ProjectFormat::SdJson,
        content_hash(initial.as_bytes()),
    );

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    // Subscribe AFTER spawning the watcher so we don't pick up any startup
    // events; the broadcast channel has no replay.
    let mut rx = state.events.subscribe();

    // Wait longer than the 100ms debounce window so any spurious events
    // from watcher startup are flushed before the HTTP save drives the
    // write we're actually testing against.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Drive a save via the HTTP layer; version 0 -> 1.
    let router = build_router(state.clone());
    let updated = sd_json("save-echo-test-renamed");
    let body = serde_json::json!({
        "json": updated,
        "version": 0,
    });
    let request = Request::builder()
        .method("POST")
        .uri("/api/projects/model.sd.json")
        .header("host", format!("127.0.0.1:{TEST_UI_PORT}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request");
    let response = router.oneshot(request).await.expect("POST save");
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "save handler must return 200"
    );

    // Wait long enough for the watcher debounce window to flush (100ms)
    // plus a generous processing budget. A Disk-source event here would
    // indicate the echo-suppression hash was stored AFTER the OS write
    // event fired, triggering a spurious re-merge.
    let no_disk_event = tokio::time::timeout(
        Duration::from_millis(800),
        await_disk_event(&mut rx, Duration::from_millis(800)),
    )
    .await;
    if let Ok(Some(ev)) = no_disk_event {
        panic!("save handler must not produce a Disk-source event; got: {ev:?}");
    }

    // The version is 1 from the save; the watcher must not have pushed it to 2.
    let entry = state.registry.get(&abs_canonical).expect("entry");
    assert_eq!(entry.version, 1, "version must be exactly 1 after the save");

    shutdown.notify_waiters();
}

/// Phase 8 Task 2: an external rename of a tracked model file re-keys the
/// registry entry and broadcasts `ProjectRenamed`. The pre-rename version,
/// echo-suppression hash, and `LoroDoc` are all preserved across the
/// re-key, so the SPA's editor can stay mounted on the new path without
/// re-fetching.
#[tokio::test]
async fn external_rename_re_keys_registry_and_emits_project_renamed() {
    let dir = TempDir::new().expect("tempdir");
    let from_abs = dir.path().join("a.sd.json");
    let to_abs = dir.path().join("b.sd.json");
    let initial = sd_json("baseline");
    std::fs::write(&from_abs, &initial).expect("write initial");
    let from_canonical = from_abs.canonicalize().expect("canonicalize from");

    let state = build_state(dir.path());
    let baseline_hash = content_hash(initial.as_bytes());
    seed_registry(
        &state,
        &from_canonical,
        ProjectFormat::SdJson,
        baseline_hash,
    );
    // Apply a browser-style edit so the registry's version advances and
    // we can confirm rename preserves it across the re-key.
    let browser_edit: serde_json::Value =
        serde_json::from_str(&sd_json("after-browser-edit")).expect("parse browser edit");
    state
        .registry
        .check_increment_and_merge(&from_canonical, 0, &browser_edit)
        .expect("browser merge succeeds");
    let pre_rename_version = state
        .registry
        .get(&from_canonical)
        .expect("from entry")
        .version;
    assert_eq!(pre_rename_version, 1, "browser edit bumped to 1");
    let pre_doc_arc = state.registry.get(&from_canonical).expect("from entry").doc;

    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    // Give the OS-level watch a moment to register before the rename.
    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::fs::rename(&from_abs, &to_abs)
        .await
        .expect("external rename");

    let event = await_renamed_event(&mut rx, Duration::from_secs(2))
        .await
        .expect("watcher emits ProjectRenamed within 2s");
    match event {
        WsMessage::ProjectRenamed { from, to } => {
            assert_eq!(from, "a.sd.json");
            assert_eq!(to, "b.sd.json");
        }
        other => panic!("expected ProjectRenamed, got {other:?}"),
    }

    let to_canonical = to_abs.canonicalize().expect("canonicalize to");
    assert!(
        state.registry.get(&from_canonical).is_none(),
        "old key dropped"
    );
    let entry = state
        .registry
        .get(&to_canonical)
        .expect("registry has new key");
    assert_eq!(
        entry.version, pre_rename_version,
        "version preserved across rename"
    );
    assert_eq!(
        entry.last_disk_hash, baseline_hash,
        "echo-suppression hash preserved"
    );
    assert!(
        Arc::ptr_eq(&pre_doc_arc, &entry.doc),
        "LoroDoc carried over verbatim"
    );

    shutdown.notify_waiters();
}

/// Rename-collision: when `mv a.sd.json b.sd.json` occurs and both files
/// are already tracked, the watcher must drop the source entry, broadcast
/// `ProjectRemoved` for both paths, then re-hydrate the destination from
/// the freshly renamed file.
///
/// Before the fix for I1.b, the `AlreadyExists` arm only removed and
/// broadcast for the destination, leaving the source (`a.sd.json`) as a
/// phantom entry in the registry — clicks on it from the SPA would return
/// 404 because the file no longer exists on disk.
#[tokio::test]
async fn rename_over_tracked_destination_removes_both_and_rehydrates() {
    let dir = TempDir::new().expect("tempdir");
    let a_abs = dir.path().join("a.sd.json");
    let b_abs = dir.path().join("b.sd.json");
    let a_content = sd_json("project-a");
    let b_content = sd_json("project-b");
    std::fs::write(&a_abs, &a_content).expect("write a");
    std::fs::write(&b_abs, &b_content).expect("write b");
    let a_canonical = a_abs.canonicalize().expect("canonicalize a");
    let b_canonical = b_abs.canonicalize().expect("canonicalize b");

    let state = build_state(dir.path());
    seed_registry(
        &state,
        &a_canonical,
        ProjectFormat::SdJson,
        content_hash(a_content.as_bytes()),
    );
    seed_registry(
        &state,
        &b_canonical,
        ProjectFormat::SdJson,
        content_hash(b_content.as_bytes()),
    );

    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Rename a.sd.json -> b.sd.json (overwrites b on disk).
    tokio::fs::rename(&a_abs, &b_abs)
        .await
        .expect("rename a -> b");

    // Collect events until we see both ProjectRemoved paths and a
    // ProjectChanged for the destination. Use a generous timeout so the
    // test is reliable on slow CI machines.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut removed_paths: Vec<String> = Vec::new();
    let mut saw_b_changed = false;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(WsMessage::ProjectRemoved { path })) => {
                removed_paths.push(path);
                if removed_paths.len() == 2 && saw_b_changed {
                    break;
                }
            }
            Ok(Ok(WsMessage::ProjectChanged {
                source: ChangeSource::Disk,
                path,
                ..
            })) if path == "b.sd.json" => {
                saw_b_changed = true;
                if removed_paths.len() == 2 {
                    break;
                }
            }
            Ok(Ok(_other)) => continue,
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => break,
            Err(_) => break,
        }
    }

    // Both ProjectRemoved events must have fired.
    let mut sorted = removed_paths.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        vec!["a.sd.json", "b.sd.json"],
        "ProjectRemoved must fire for both source and destination; got: {removed_paths:?}"
    );

    // The destination must have been re-hydrated with the renamed file's content.
    assert!(
        saw_b_changed,
        "watcher must emit ProjectChanged{{source: Disk}} for b.sd.json after re-hydration"
    );

    // Source is gone from the registry.
    assert!(
        state.registry.get(&a_canonical).is_none(),
        "source a.sd.json must be removed from registry after rename-collision"
    );

    // Destination is present with fresh content (project-a, since the file
    // that was renamed onto b.sd.json came from a.sd.json).
    let b_canonical_new = b_abs.canonicalize().expect("canonicalize b post-rename");
    let doc = state
        .registry
        .get_or_init_doc(&b_canonical_new)
        .expect("destination must be in registry");
    let exported = doc.export_canonical_json().expect("export");
    assert_eq!(
        exported["name"].as_str(),
        Some("project-a"),
        "destination doc must reflect the renamed file's content"
    );

    shutdown.notify_waiters();
}

/// Edge case: a paired-rename event lands for a path the registry never
/// knew about (e.g. the source extension was outside our denylist when we
/// scanned, or the file was created and renamed before discovery caught
/// up). In that case we treat the destination side as a fresh Created
/// event so the registry hydrates the new entry.
#[tokio::test]
async fn rename_of_untracked_path_falls_through_to_created() {
    let dir = TempDir::new().expect("tempdir");
    let from_abs = dir.path().join("a.sd.json");
    let to_abs = dir.path().join("b.sd.json");
    std::fs::write(&from_abs, sd_json("initial")).expect("write initial");
    // Do NOT seed the registry: the watcher should treat this as a
    // fresh Created event for the destination after the rename.

    let state = build_state(dir.path());
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(50)).await;

    tokio::fs::rename(&from_abs, &to_abs)
        .await
        .expect("external rename");

    // Allow either a ProjectChanged{Disk} (Created path) or a
    // ProjectRenamed depending on which arm picks up. The contract is:
    // the destination must end up registered. Wait for either.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw_event = false;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(WsMessage::ProjectChanged { source, path, .. })) => {
                if source == ChangeSource::Disk && path == "b.sd.json" {
                    saw_event = true;
                    break;
                }
            }
            Ok(Ok(WsMessage::ProjectRenamed { to, .. })) if to == "b.sd.json" => {
                saw_event = true;
                break;
            }
            Ok(Ok(_other)) => continue,
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => break,
            Err(_) => break,
        }
    }
    assert!(saw_event, "watcher must broadcast for the renamed path");

    let to_canonical = to_abs.canonicalize().expect("canonicalize to");
    assert!(
        state.registry.get(&to_canonical).is_some(),
        "registry hydrates the destination as a fresh entry"
    );

    shutdown.notify_waiters();
}
