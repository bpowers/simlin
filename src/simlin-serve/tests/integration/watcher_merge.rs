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

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use simlin_serve::build_router;
use simlin_serve::events::{ChangeSource, EventBus, WsMessage};
use simlin_serve::handlers::AppState;
use simlin_serve::hashing::content_hash;
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use simlin_serve::test_support::{
    OS_EVENT_TIMEOUT, unavailable_git_probe, wait_for_watcher_ready, watcher_barrier,
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

/// Grace window allowed after a `watcher_barrier` before declaring an event
/// absent. The barrier already proves the watcher dispatched every event that
/// preceded it, so anything the trigger produced is queued on `rx` by now; this
/// only absorbs the scheduler hop between the actor's `publish` and a broadcast
/// receiver observing it. It is deliberately *not* the thing that makes the
/// absence assertion sound -- the barrier is, which is why this can be a
/// scheduler-hop's worth of time rather than a guess at how slow the OS is.
const POST_BARRIER_SETTLE: Duration = Duration::from_millis(50);

/// Helper: build an `AppState` rooted at `dir` with a fresh registry, no
/// git visibility, and an `EventBus`.
fn build_state(dir: &Path) -> AppState {
    let canonical = dir.canonicalize().expect("canonicalize");
    AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(unavailable_git_probe()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
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

const TEACUP_MDL_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/teacup.mdl");

/// The `room_temperature` equation in a canonical-JSON project value, as
/// the doc exports it. The `.mdl` tests observe edits through this aux
/// because Vensim has no project-name field to observe them through.
fn room_temperature_equation(project_json: &serde_json::Value) -> String {
    project_json["models"][0]["auxiliaries"]
        .as_array()
        .expect("auxiliaries")
        .iter()
        .find(|a| {
            simlin_engine::canonicalize(a["name"].as_str().unwrap_or("")).as_ref()
                == "room_temperature"
        })
        .and_then(|a| a["equation"].as_str())
        .expect("room temperature aux")
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    // Prove the OS-level watch is live, then write immediately: a write issued
    // before the watch registers is never reported.
    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    let updated = sd_json("renamed-on-disk");
    tokio::fs::write(&abs, &updated)
        .await
        .expect("write update");

    let event = await_disk_event(&mut rx, OS_EVENT_TIMEOUT)
        .await
        .expect("watcher emitted ProjectChanged{source: Disk}");
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    // Write the same bytes back. The watcher should see the event and
    // short-circuit because content_hash(bytes) == last_disk_hash.
    tokio::fs::write(&abs, &initial).await.expect("write echo");

    // The barrier is what makes this assertion meaningful: it blocks until the
    // watcher has dispatched the echo write's event, so "no Disk broadcast" is
    // a statement about echo-suppression rather than about how fast this host
    // happens to be.
    watcher_barrier(&state.root, &mut sightings)
        .await
        .expect("watcher processes the echo write");

    let no_event = await_disk_event(&mut rx, POST_BARRIER_SETTLE).await;
    if let Some(ev) = no_event {
        panic!("byte-identical disk write must not produce a Disk broadcast; got: {ev:?}");
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

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

    let event = await_disk_event(&mut rx, OS_EVENT_TIMEOUT)
        .await
        .expect("watcher fires Disk-source ProjectChanged");
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    // Write garbage that's not valid JSON.
    tokio::fs::write(&abs, b"this is not json {{{")
        .await
        .expect("write garbage");

    // Barrier first: without it, "no ProjectChanged{Disk}" would also hold on a
    // host too slow to have delivered the event yet.
    watcher_barrier(&state.root, &mut sightings)
        .await
        .expect("watcher processes the garbage write");

    let no_event = await_disk_event(&mut rx, POST_BARRIER_SETTLE).await;
    if let Some(ev) = no_event {
        panic!("invalid disk write must not produce a Disk broadcast; got: {ev:?}");
    }

    // Version unchanged; doc still reflects the baseline.
    let entry = state.registry.get(&abs_canonical).expect("entry");
    assert_eq!(entry.version, 0);
    let doc = state.registry.get_or_init_doc(&abs_canonical).expect("doc");
    let exported = doc.export_canonical_json().expect("export");
    assert_eq!(exported["name"].as_str(), Some("baseline"));

    shutdown.notify_waiters();
}

/// An external edit to a `.mdl` merges the `.mdl` and broadcasts for the
/// `.mdl`, exactly like any other format -- even when a same-stem
/// `.sd.json` sits next to it (the on-disk trace of an earlier release's
/// sidecar write). The two are independent projects: the `.sd.json`
/// entry is untouched by the `.mdl` event.
#[tokio::test]
async fn mdl_disk_edit_merges_the_mdl_regardless_of_a_same_stem_sd_json() {
    let dir = TempDir::new().expect("tempdir");
    let mdl = dir.path().join("teacup.mdl");
    std::fs::copy(TEACUP_MDL_FIXTURE, &mdl).expect("copy teacup.mdl");
    let sd_json_path = dir.path().join("teacup.sd.json");
    std::fs::write(&sd_json_path, sd_json("independent")).expect("write sd.json");
    let mdl_canonical = mdl.canonicalize().expect("canonicalize mdl");
    let sd_json_canonical = sd_json_path.canonicalize().expect("canonicalize sd.json");

    let state = build_state(dir.path());
    let initial_mdl_bytes = std::fs::read(&mdl_canonical).expect("read mdl");
    seed_registry(
        &state,
        &mdl_canonical,
        ProjectFormat::Mdl,
        content_hash(&initial_mdl_bytes),
    );
    seed_registry(
        &state,
        &sd_json_canonical,
        ProjectFormat::SdJson,
        content_hash(sd_json("independent").as_bytes()),
    );
    // Hydrate the .mdl doc so the merge has a baseline to compare against.
    let before = state
        .registry
        .get_or_init_doc(&mdl_canonical)
        .expect("hydrate")
        .export_canonical_json()
        .expect("export");
    assert_eq!(room_temperature_equation(&before), "70");
    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    // External edit in Vensim syntax: Room Temperature 70 -> 75.
    let original = String::from_utf8(initial_mdl_bytes).expect("utf8");
    let edited = original.replacen("Room Temperature=\n\t70", "Room Temperature=\n\t75", 1);
    assert_ne!(edited, original, "fixture must contain the expected line");
    tokio::fs::write(&mdl, edited).await.expect("write mdl");

    let event = await_disk_event(&mut rx, OS_EVENT_TIMEOUT)
        .await
        .expect("mdl edit must produce a Disk broadcast");
    match event {
        WsMessage::ProjectChanged { path, version, .. } => {
            assert_eq!(path, "teacup.mdl", "the broadcast names the .mdl");
            assert_eq!(version, 1);
        }
        other => panic!("expected ProjectChanged, got {other:?}"),
    }

    let after = state
        .registry
        .get_or_init_doc(&mdl_canonical)
        .expect("doc")
        .export_canonical_json()
        .expect("export");
    assert_eq!(room_temperature_equation(&after), "75", "disk edit merged");

    let sd_json_entry = state
        .registry
        .get(&sd_json_canonical)
        .expect("sd.json entry");
    assert_eq!(
        sd_json_entry.version, 0,
        "the same-stem .sd.json is a separate project and must be untouched"
    );

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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    // Create a brand-new .sd.json that's not yet in the registry.
    let new_path = state.root.join("brand_new.sd.json");
    tokio::fs::write(&new_path, sd_json("freshly-created"))
        .await
        .expect("create file");

    let event = await_disk_event(&mut rx, OS_EVENT_TIMEOUT)
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
///
/// Linux-only on the current macOS-latest runner: FSEvents on macOS 15
/// does not reliably deliver an actionable event for `unlink()` on a
/// file that existed *before* the watcher's `FSEventStreamCreate`. The
/// debouncer's file-id cache is populated only on Create events, so
/// pre-existing files are unknown to the cache when their unlink
/// flag fires; combined with the FSEvents tendency to coalesce flags
/// within a single dispatch, the actor never sees a `Remove(File)` /
/// `Modify(Name(Any))` event we can act on in this scenario. Sister
/// tests that mutate (`external_disk_edit_triggers_disk_source_broadcast`)
/// or that create-then-mutate inside the watch window all pass; the
/// failure is specific to the "unlink a pre-existing file" shape this
/// test exercises. Gating here while the design discussion plays out
/// in `docs/tech-debt.md#macos-rename-pairing-limitation`.
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS pre-existing-file unlink event missing; see tech-debt.md"
)]
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    tokio::fs::remove_file(&abs)
        .await
        .expect("remove the model file");

    let event = await_removed_event(&mut rx, OS_EVENT_TIMEOUT)
        .await
        .expect("watcher must broadcast ProjectRemoved");
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    tokio::fs::remove_file(&abs).await.expect("remove the file");

    watcher_barrier(&state.root, &mut sightings)
        .await
        .expect("watcher processes the removal");

    let no_event = await_removed_event(&mut rx, POST_BARRIER_SETTLE).await;
    if let Some(ev) = no_event {
        panic!(
            "removing an untracked path must not produce a ProjectRemoved broadcast; got: {ev:?}"
        );
    }

    shutdown.notify_waiters();
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    // Subscribe AFTER spawning the watcher so we don't pick up any startup
    // events; the broadcast channel has no replay.
    let mut rx = state.events.subscribe();

    // The probe file is inert (no model extension), so establishing readiness
    // cannot itself put anything on `rx` for us to mistake for the save's echo.
    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

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

    // Block until the watcher has dispatched the save's atomic_write events. A
    // Disk-source event surviving that barrier would mean the echo-suppression
    // hash was stored AFTER the OS write event fired, triggering a spurious
    // re-merge.
    watcher_barrier(&state.root, &mut sightings)
        .await
        .expect("watcher processes the save's write");

    let no_disk_event = await_disk_event(&mut rx, POST_BARRIER_SETTLE).await;
    if let Some(ev) = no_disk_event {
        panic!("save handler must not produce a Disk-source event; got: {ev:?}");
    }

    // The version is 1 from the save; the watcher must not have pushed it to 2.
    let entry = state.registry.get(&abs_canonical).expect("entry");
    assert_eq!(entry.version, 1, "version must be exactly 1 after the save");

    shutdown.notify_waiters();
}

/// An `.mdl` save rewrites the `.mdl` in place through the same primed
/// echo-suppression path every format uses: the watcher sees the save's
/// own atomic_write land on the `.mdl`, matches the primed hash, and stays
/// silent; the version is exactly 1 and no sibling file appears.
#[tokio::test]
async fn mdl_save_rewrites_in_place_and_echo_suppresses_its_own_watcher_event() {
    let dir = TempDir::new().expect("tempdir");
    let canonical_root = dir.path().canonicalize().expect("canon root");
    let mdl_path = canonical_root.join("teacup.mdl");
    std::fs::copy(TEACUP_MDL_FIXTURE, &mdl_path).expect("copy teacup.mdl");
    let mdl_bytes = std::fs::read(&mdl_path).expect("read mdl");

    let state = build_state(dir.path());
    seed_registry(
        &state,
        &mdl_path,
        ProjectFormat::Mdl,
        content_hash(&mdl_bytes),
    );

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();
    let mut rx = state.events.subscribe();

    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    // Drive a save via the HTTP layer at the .mdl path: GET the canonical
    // JSON, edit one equation, POST it back at version 0.
    let router = build_router(state.clone());
    let get = Request::builder()
        .method("GET")
        .uri("/api/projects/teacup.mdl")
        .header("host", format!("127.0.0.1:{TEST_UI_PORT}"))
        .body(Body::empty())
        .expect("build request");
    let response = router.clone().oneshot(get).await.expect("GET");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .expect("body");
    let got: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
    let mut project: serde_json::Value =
        serde_json::from_str(got["json"].as_str().expect("json string")).expect("project");
    let aux = project["models"][0]["auxiliaries"]
        .as_array_mut()
        .expect("auxiliaries")
        .iter_mut()
        .find(|a| {
            simlin_engine::canonicalize(a["name"].as_str().unwrap_or("")).as_ref()
                == "room_temperature"
        })
        .expect("room temperature aux");
    aux["equation"] = serde_json::Value::String("75".to_string());

    let body = serde_json::json!({
        "json": serde_json::to_string(&project).expect("reserialize"),
        "version": 0,
    });
    let post = Request::builder()
        .method("POST")
        .uri("/api/projects/teacup.mdl")
        .header("host", format!("127.0.0.1:{TEST_UI_PORT}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("build request");
    let response = router.oneshot(post).await.expect("POST save");
    assert_eq!(response.status(), StatusCode::OK, "save must return 200");

    // A Disk event surviving the barrier would mean the save's own write on
    // the .mdl was not echo-suppressed.
    watcher_barrier(&state.root, &mut sightings)
        .await
        .expect("watcher processes the save's write");

    let no_disk_event = await_disk_event(&mut rx, POST_BARRIER_SETTLE).await;
    if let Some(ev) = no_disk_event {
        panic!("the .mdl save's own write must echo-suppress; got: {ev:?}");
    }

    let entry = state.registry.get(&mdl_path).expect("mdl entry");
    assert_eq!(entry.version, 1, "version must be exactly 1 after the save");
    assert_eq!(entry.format, ProjectFormat::Mdl);
    assert!(
        !canonical_root.join("teacup.sd.json").exists(),
        "no sidecar may be created"
    );
    let text = std::fs::read_to_string(&mdl_path).expect("read mdl");
    assert!(
        text.starts_with("{UTF-8}"),
        "the .mdl must still be Vensim text"
    );
    assert!(text.contains("75"), "the edit must be on disk");

    shutdown.notify_waiters();
}

/// Phase 8 Task 2: an external rename of a tracked model file re-keys the
/// registry entry and broadcasts `ProjectRenamed`. The pre-rename version,
/// echo-suppression hash, and `LoroDoc` are all preserved across the
/// re-key, so the SPA's editor can stay mounted on the new path without
/// re-fetching.
///
/// Linux-only: the test depends on `notify-debouncer-full` pairing a
/// rename's two underlying notify events (source + destination) into a
/// single `Modify(Name(Both))`. On Linux that pairing comes from
/// inotify's `IN_MOVED_FROM` / `IN_MOVED_TO` cookies, which always
/// arrive together regardless of when the file appeared. On macOS
/// FSEvents reports rename sides as separate `ITEM_RENAMED` events that
/// the debouncer can only pair via its file-id cache. The cache is
/// only populated on Create events, so files that existed *before* the
/// watcher started (which describes every test fixture and the typical
/// open-a-folder UX) skip the pairing entirely; the watcher sees two
/// unpaired single-path events instead of a `Modify(Name(Both))`. The
/// underlying fix needs a heuristic outside `notify-debouncer-full`'s
/// model and is being tracked in a follow-up issue rather than
/// land-and-papered-over here. See
/// `docs/tech-debt.md#macos-rename-pairing-limitation` for the design
/// discussion.
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS rename pairing not supported; see tech-debt.md"
)]
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    // A rename cannot be replayed, so readiness must be established before it.
    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    tokio::fs::rename(&from_abs, &to_abs)
        .await
        .expect("external rename");

    let event = await_renamed_event(&mut rx, OS_EVENT_TIMEOUT)
        .await
        .expect("watcher emits ProjectRenamed");
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
///
/// Linux-only for the same reason as
/// [`external_rename_re_keys_registry_and_emits_project_renamed`]: the
/// rename-pair classification fires only when the debouncer was able
/// to coalesce both sides via inotify's MOVED_FROM/TO cookies. On
/// macOS the destination side surfaces as a content-modify against
/// the existing registry entry (which the watcher merges via the
/// usual disk-edit path) and the source side as a stand-alone removal
/// — the dual-`ProjectRemoved` invariant is a Linux-side artefact of
/// the paired rename event. See
/// `docs/tech-debt.md#macos-rename-pairing-limitation`.
#[cfg_attr(
    target_os = "macos",
    ignore = "macOS rename pairing not supported; see tech-debt.md"
)]
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    // A rename cannot be replayed, so readiness must be established before it.
    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    // Rename a.sd.json -> b.sd.json (overwrites b on disk).
    tokio::fs::rename(&a_abs, &b_abs)
        .await
        .expect("rename a -> b");

    // Collect events until we see both ProjectRemoved paths and a
    // ProjectChanged for the destination.
    let deadline = tokio::time::Instant::now() + OS_EVENT_TIMEOUT;
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
    let watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");
    let mut sightings = watcher.probe_sightings();

    // A rename cannot be replayed, so readiness must be established before it.
    wait_for_watcher_ready(&state.root, &mut sightings)
        .await
        .expect("watcher becomes ready");

    tokio::fs::rename(&from_abs, &to_abs)
        .await
        .expect("external rename");

    // Allow either a ProjectChanged{Disk} (Created path) or a
    // ProjectRenamed depending on which arm picks up. The contract is:
    // the destination must end up registered. Wait for either.
    let deadline = tokio::time::Instant::now() + OS_EVENT_TIMEOUT;
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
