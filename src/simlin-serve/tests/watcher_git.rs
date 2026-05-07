// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Phase 4 Task 7: file-watcher reaction to `.git/index` and `.git/HEAD`
//! changes. Asserts AC2.4 — git status is recomputed when the watcher
//! fires for `.git/HEAD`/`.git/index` — by initializing a real repo,
//! committing a tracked .stmx, modifying it (so `git status` reports
//! dirty), and observing the registry's per-entry GitState flip from
//! Tracked{dirty:true} to Tracked{dirty:false} after the file is
//! committed.
//!
//! Skipped on hosts without `git` on PATH; CI has git so this is rare.

#![deny(unsafe_code)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use simlin_serve::events::{ChangeSource, EventBus, WsMessage};
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use simlin_serve::watcher::{ShutdownSignal, spawn_watcher};
use tempfile::TempDir;
use tokio::sync::Notify;
use tokio::sync::broadcast::error::RecvError;

/// True when `git --version` reports success on this host.
fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run `git` in `cwd`; assert success. Echoes stderr in the panic.
///
/// Strips every `GIT_*` env var inherited from the parent so the inner
/// git always operates on the temp repo at `cwd`. Without this, when
/// the workspace test suite runs from inside an outer `git commit` (the
/// project pre-commit hook invokes `cargo test --workspace`), `GIT_DIR`,
/// `GIT_WORK_TREE`, and `GIT_INDEX_FILE` propagate down to the inner
/// `git` and cause it to operate on the OUTER repository instead of
/// the freshly-`git init`'d temp dir, masking the watcher behavior the
/// test exercises.
fn must_git(cwd: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd).args(args);
    for (key, _) in std::env::vars() {
        if key.starts_with("GIT_") {
            cmd.env_remove(&key);
        }
    }
    let out = cmd
        .output()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    if !out.status.success() {
        panic!(
            "git {} exited {:?}: {}",
            args.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Wait for a `ProjectChanged{source: Disk}` event for `expected_path`.
/// We need to filter both by source and path because the watcher emits
/// other Disk-source events for the underlying file modification too.
async fn await_disk_event_for(
    rx: &mut tokio::sync::broadcast::Receiver<WsMessage>,
    expected_path: &str,
    timeout: Duration,
) -> Option<WsMessage> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(msg)) => match &msg {
                WsMessage::ProjectChanged {
                    source: ChangeSource::Disk,
                    path,
                    ..
                } if path == expected_path => return Some(msg),
                _ => continue,
            },
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => return None,
            Err(_) => return None,
        }
    }
}

/// Build an `AppState` rooted at `dir` with a real `GitProbe` (so we can
/// observe Tracked{dirty} transitions).
fn build_state(dir: &Path) -> AppState {
    let canonical = dir.canonicalize().expect("canonicalize");
    AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(GitProbe::detect()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        // Watcher-git tests stay below the HTTP surface, so the host
        // validator is never consulted; ports are placeholders.
        ui_port: 0,
        mcp_port: 0,
        strict_origin: true,
    }
}

/// AC2.4: a `git commit` rewrites `.git/index`; the watcher fires a
/// GitInternal event and the registry's git state for files inside the
/// repo flips from `Tracked{dirty:true}` (pre-commit working-tree edit)
/// to `Tracked{dirty:false}` (post-commit clean state).
#[tokio::test]
async fn git_commit_flips_registry_entry_from_dirty_to_clean() {
    if !git_available() {
        eprintln!("skipping: git binary not available");
        return;
    }
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path();
    must_git(repo, &["init", "-q", "-b", "main"]);
    must_git(repo, &["config", "user.email", "test@example.com"]);
    must_git(repo, &["config", "user.name", "test"]);

    let xmile = "<?xml version=\"1.0\"?>\n<xmile version=\"1.0\" xmlns=\"http://docs.oasis-open.org/xmile/ns/XMILE/v1.0\"><sim_specs method=\"euler\"><start>0</start><stop>10</stop><dt>1</dt></sim_specs><model><variables/></model></xmile>\n";
    let model_path = repo.join("model.stmx");
    std::fs::write(&model_path, xmile).expect("write initial");
    must_git(repo, &["add", "model.stmx"]);
    must_git(repo, &["commit", "-q", "-m", "initial"]);

    let canonical_root = repo.canonicalize().expect("canonicalize root");
    let model_canonical = model_path.canonicalize().expect("canonicalize model");

    // Make the file dirty BEFORE the watcher starts so the registry
    // entry's seeded git state already reflects "dirty". This avoids
    // racing the watcher's reaction to the model-file event with the
    // initial seed.
    let xmile_v2 = "<?xml version=\"1.0\"?>\n<xmile version=\"1.0\" xmlns=\"http://docs.oasis-open.org/xmile/ns/XMILE/v1.0\"><sim_specs method=\"euler\"><start>0</start><stop>20</stop><dt>1</dt></sim_specs><model><variables/></model></xmile>\n";
    std::fs::write(&model_path, xmile_v2).expect("write v2");

    let state = build_state(repo);
    let metadata = std::fs::metadata(&model_canonical).expect("metadata");
    state.registry.upsert(
        model_canonical.clone(),
        ProjectMeta {
            path: std::path::PathBuf::new(),
            format: ProjectFormat::Stmx,
            mtime: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            size: metadata.len(),
            git: state.git.status_for(&model_canonical),
            version: 0,
            doc: Default::default(),
            last_disk_hash: 0,
            last_diagnostic_keys: std::collections::BTreeSet::new(),
        },
    );
    // Sanity: working tree is dirty before the commit.
    assert_eq!(
        state.registry.get(&model_canonical).expect("entry").git,
        GitState::Tracked { dirty: true },
        "the working-tree edit must show as dirty before the commit"
    );

    let mut rx = state.events.subscribe();

    let shutdown: ShutdownSignal = Arc::new(Notify::new());
    let _watcher = spawn_watcher(state.clone(), shutdown.clone()).expect("spawn watcher");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Stage + commit. The commit rewrites .git/index, which the
    // watcher's GitInternal handler reacts to by invalidating the
    // GitProbe cache and re-evaluating each registry entry inside
    // this repo.
    must_git(repo, &["add", "model.stmx"]);
    must_git(repo, &["commit", "-q", "-m", "v2"]);

    // The watcher's debounce is 100ms; give the GitProbe cache + the
    // registry-update pass enough wall-clock time to land. Polling
    // because we don't want to assert wall-clock timing as part of
    // the success criterion -- only that the eventual state is
    // tracked+clean.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut final_state = state.registry.get(&model_canonical).expect("entry").git;
    while final_state != (GitState::Tracked { dirty: false })
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
        final_state = state.registry.get(&model_canonical).expect("entry").git;
    }
    assert_eq!(
        final_state,
        GitState::Tracked { dirty: false },
        "after the commit + .git/index event, the file must be tracked+clean"
    );

    // Sanity: enclosing_git_root resolves to the repo root we set up.
    let resolved = simlin_serve::git::enclosing_git_root(&model_canonical).expect("repo root");
    assert_eq!(resolved, canonical_root);

    // The git-status change must have been broadcast as a
    // ProjectChanged{source: Disk} event for the model. We poll the
    // receiver to find an event matching the model path with Disk
    // source. (Other Disk-source events from incidental file
    // modification may also fire; we just need to see at least one.)
    let event = await_disk_event_for(&mut rx, "model.stmx", Duration::from_secs(2)).await;
    assert!(
        event.is_some(),
        "git-status change must produce a ProjectChanged{{Disk}} broadcast for the file"
    );

    shutdown.notify_waiters();
}
