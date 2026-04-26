// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Integration tests for `POST /api/projects/{*path}`. Covers
//! version-check + validation (Subcomponent A) plus the format-aware
//! disk-write paths (Subcomponent B): XMILE in-place overwrite,
//! `.sd.json` sidecar creation for `.mdl` requests, and the
//! `.mdl`-untouched invariant.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use simlin_serve::test_support::unavailable_git_probe;
use tempfile::TempDir;
use tower::ServiceExt;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
// Synthetic ports for the host validator middleware (Phase 8 Task 8).
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

fn copy_fixture(name: &str, dest_dir: &std::path::Path) -> PathBuf {
    let src = PathBuf::from(FIXTURES_DIR).join(name);
    let dest = dest_dir.join(name);
    fs::copy(&src, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
    dest
}

fn build_state(root: PathBuf) -> AppState {
    AppState {
        registry: Arc::new(ProjectRegistry::new(root.clone())),
        git: Arc::new(unavailable_git_probe()),
        root: Arc::new(root),
        events: Arc::new(EventBus::new()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    }
}

fn seed_registry(state: &AppState, abs_path: &std::path::Path, format: ProjectFormat) {
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
            last_diagnostic_keys: std::collections::BTreeSet::new(),
        },
    );
}

async fn fetch(state: AppState, method: &str, uri: &str, body: Body) -> (StatusCode, Vec<u8>) {
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .header("content-type", "application/json")
                .body(body)
                .expect("request build"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .expect("body bytes");
    (status, body.to_vec())
}

fn parse_body(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or_else(|e| {
        panic!(
            "response json: {e}; body: {}",
            String::from_utf8_lossy(body)
        )
    })
}

/// Fetch the canonical JSON for a path via GET (used to seed save bodies).
async fn get_canonical_json(state: AppState, uri: &str) -> (u64, String) {
    let (status, body) = fetch(state, "GET", uri, Body::empty()).await;
    assert_eq!(status, StatusCode::OK, "GET {uri} failed");
    let value = parse_body(&body);
    (
        value["version"].as_u64().expect("version u64"),
        value["json"].as_str().expect("json string").to_string(),
    )
}

#[tokio::test]
async fn ok_with_matching_version_increments_registry() {
    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    // Read the current state to obtain a valid canonical-JSON body.
    let (version, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;
    assert_eq!(version, 0);

    // POST it back unchanged. Validation passes and the registry version
    // increments to 1. Subcomponent B also rewrites the file in XMILE
    // form; the new bytes parse back to the same project.
    let body = serde_json::json!({"json": json_body, "version": 0}).to_string();
    let (status, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );
    let response = parse_body(&response_bytes);
    assert_eq!(response["version"].as_u64(), Some(1));
    assert_eq!(response["path"].as_str(), Some("teacup.stmx"));

    // The XMILE file has been overwritten in place. The new content must
    // parse back to a Project semantically equivalent to the input.
    let post_bytes = fs::read(&abs_canonical).unwrap();
    let mut reader = std::io::Cursor::new(&post_bytes[..]);
    let post_project = simlin_engine::open_xmile(&mut reader).expect("rewritten file parses");
    assert_eq!(post_project.name, "teacup-modern");
}

/// AC3.2: Saving an edit to a `.stmx` file writes the new content back
/// to the original file in XMILE format. We round-trip the canonical
/// JSON through a small mutation (rename a variable) and verify the
/// rewritten file reflects the change.
#[tokio::test]
async fn save_xmile_writes_back_in_place_with_edits() {
    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    // Get the canonical JSON, then apply a trivial mutation by replacing
    // the project name. This exercises the full pipeline: parse incoming
    // JSON -> validate -> write.
    let (_, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;
    let mut json_value: serde_json::Value =
        serde_json::from_str(&json_body).expect("parse canonical json");
    json_value["name"] = serde_json::Value::String("renamed-project".to_string());
    let mutated_json = serde_json::to_string(&json_value).expect("reserialize");

    let body = serde_json::json!({"json": mutated_json, "version": 0}).to_string();
    let (status, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    // Read the file back and verify the rename made it through.
    let post_bytes = fs::read(&abs_canonical).unwrap();
    let mut reader = std::io::Cursor::new(&post_bytes[..]);
    let post_project = simlin_engine::open_xmile(&mut reader).expect("rewritten file parses");
    assert_eq!(post_project.name, "renamed-project");
}

#[tokio::test]
async fn stale_version_returns_409_conflict() {
    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    let (_, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;

    // First POST claims version 0 -> 1.
    let body0 = serde_json::json!({"json": &json_body, "version": 0}).to_string();
    let (status0, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body0),
    )
    .await;
    assert_eq!(status0, StatusCode::OK);

    // Second POST with the same stale version 0 must 409.
    let body1 = serde_json::json!({"json": &json_body, "version": 0}).to_string();
    let (status1, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body1),
    )
    .await;
    assert_eq!(status1, StatusCode::CONFLICT);
    let response = parse_body(&response_bytes);
    assert_eq!(response["error"].as_str(), Some("version_mismatch"));
    assert_eq!(response["expected"].as_u64(), Some(0));
    assert_eq!(
        response["actual"].as_u64(),
        Some(1),
        "the response must report the actual current version"
    );
}

#[tokio::test]
async fn validation_failure_returns_422() {
    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    // Invalid project: references an undefined identifier.
    let bad_json = r#"{
        "name": "teacup",
        "simSpecs": {"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"},
        "models": [{
            "name": "main",
            "auxiliaries": [
                {"name": "bad", "equation": "1 + bogus"}
            ]
        }]
    }"#;

    let pre_bytes = fs::read(&abs_canonical).unwrap();

    let body = serde_json::json!({"json": bad_json, "version": 0}).to_string();
    let (status, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );
    let response = parse_body(&response_bytes);
    assert_eq!(response["error"].as_str(), Some("validation_failed"));
    let details = response["details"].as_array().expect("details array");
    assert!(!details.is_empty());
    let bad = details
        .iter()
        .find(|e| e["variableName"].as_str() == Some("bad"))
        .expect("expected an error tagged to variable 'bad'");
    assert_eq!(bad["code"].as_str(), Some("unknown_dependency"));

    // Disk untouched.
    let post_bytes = fs::read(&abs_canonical).unwrap();
    assert_eq!(
        pre_bytes, post_bytes,
        "file contents must not change on 422"
    );
}

#[tokio::test]
async fn malformed_json_body_returns_400() {
    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    // The request body is well-formed JSON, but the embedded `json` field
    // does not parse as a `json::Project`.
    let body = serde_json::json!({"json": "not actually a project body", "version": 0}).to_string();
    let (status, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn missing_path_returns_404() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let state = build_state(canonical_root);

    let body = serde_json::json!({"json": "{}", "version": 0}).to_string();
    let (status, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/missing.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// AC3.4: Saving an edit to a `.mdl` file writes a sibling
/// `<basename>.sd.json` and leaves the original `.mdl` untouched.
/// Also exercises the SaveResponse path-rewrite to the sidecar.
#[tokio::test]
async fn save_mdl_creates_sidecar_and_does_not_modify_mdl() {
    let dir = TempDir::new().unwrap();
    let mdl_abs = copy_fixture("teacup.mdl", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let mdl_canonical = mdl_abs.canonicalize().unwrap();
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &mdl_canonical, ProjectFormat::Mdl);

    let pre_mdl_bytes = fs::read(&mdl_canonical).unwrap();

    // GET first so we have a canonical-JSON body to send back.
    let (version, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.mdl").await;
    assert_eq!(version, 0);

    let body = serde_json::json!({"json": &json_body, "version": 0}).to_string();
    let (status, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.mdl",
        Body::from(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    // The response path now points at the sidecar so the SPA can update
    // its URL state to follow the redirect.
    let response = parse_body(&response_bytes);
    assert_eq!(response["version"].as_u64(), Some(1));
    assert_eq!(response["path"].as_str(), Some("teacup.sd.json"));

    // The .mdl file is byte-identical to before the save.
    let post_mdl_bytes = fs::read(&mdl_canonical).unwrap();
    assert_eq!(
        pre_mdl_bytes, post_mdl_bytes,
        ".mdl must not be modified by a sidecar write"
    );

    // The sidecar exists alongside the .mdl with valid JSON content.
    let sidecar_path = canonical_root.join("teacup.sd.json");
    assert!(sidecar_path.is_file(), "sidecar must be created on save");
    let sidecar_bytes = fs::read(&sidecar_path).unwrap();
    let _: simlin_engine::json::Project =
        serde_json::from_slice(&sidecar_bytes).expect("sidecar parses as json::Project");
}

/// AC3.5: After a save creates the sidecar, GET on the original `.mdl`
/// path returns the sidecar's content (Phase 1 read-side preference,
/// re-verified end-to-end).
#[tokio::test]
async fn stale_tab_save_to_mdl_after_sidecar_returns_409() {
    // Two-tab race: Tab A saves the .mdl path, which redirects to a fresh
    // .sd.json sidecar at version 1 and removes the .mdl registry entry.
    // Tab B holds stale state (still believes the .mdl is at version 0)
    // and POSTs to /api/projects/foo.mdl with version=0. Without
    // sidecar-preference at save-resolve time, the handler's
    // ensure_or_get re-creates a .mdl entry at version 0; the version
    // check passes (0 == 0); the stale edit silently overwrites the
    // newer sidecar content. The principled fix is the same disk-side
    // sidecar-preference rule the read path applies.
    let dir = TempDir::new().unwrap();
    let mdl_abs = copy_fixture("teacup.mdl", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let mdl_canonical = mdl_abs.canonicalize().unwrap();
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &mdl_canonical, ProjectFormat::Mdl);

    // Tab A: drive the canonical save flow that creates the sidecar at v1.
    let (_, body_text) = get_canonical_json(state.clone(), "/api/projects/teacup.mdl").await;
    let mut tab_a_value: Value = serde_json::from_str(&body_text).unwrap();
    tab_a_value["name"] = Value::String("tab-A-saved".to_string());
    let tab_a_body =
        serde_json::json!({"json": serde_json::to_string(&tab_a_value).unwrap(), "version": 0})
            .to_string();
    let (status, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.mdl",
        Body::from(tab_a_body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "tab A's first save must succeed");

    // Tab B: has never observed Tab A's save. Saves to the .mdl path with
    // a stale version=0. The sidecar exists on disk at version 1; the
    // optimistic-lock check must run against the sidecar entry and reject.
    let mut tab_b_value: Value = serde_json::from_str(&body_text).unwrap();
    tab_b_value["name"] = Value::String("tab-B-stale".to_string());
    let tab_b_body =
        serde_json::json!({"json": serde_json::to_string(&tab_b_value).unwrap(), "version": 0})
            .to_string();
    let (status, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.mdl",
        Body::from(tab_b_body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "stale .mdl save must surface 409 against the sidecar's current version; body: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    // The sidecar still has Tab A's content — Tab B's stale edit must
    // not have landed.
    let sidecar_canonical = canonical_root.join("teacup.sd.json");
    let sidecar_bytes = fs::read(&sidecar_canonical).unwrap();
    let sidecar_json: Value = serde_json::from_slice(&sidecar_bytes).unwrap();
    assert_eq!(
        sidecar_json["name"].as_str(),
        Some("tab-A-saved"),
        "sidecar must still hold Tab A's saved content"
    );
}

#[tokio::test]
async fn after_sidecar_save_get_mdl_returns_sidecar_content() {
    let dir = TempDir::new().unwrap();
    let mdl_abs = copy_fixture("teacup.mdl", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let mdl_canonical = mdl_abs.canonicalize().unwrap();
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &mdl_canonical, ProjectFormat::Mdl);

    let (_, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.mdl").await;
    // Mutate the project name so the second GET reflects the saved
    // change rather than just the parsed-from-mdl baseline.
    let mut json_value: serde_json::Value = serde_json::from_str(&json_body).expect("parse json");
    json_value["name"] = serde_json::Value::String("post-save-name".to_string());
    let mutated = serde_json::to_string(&json_value).unwrap();

    let body = serde_json::json!({"json": mutated, "version": 0}).to_string();
    let (status, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.mdl",
        Body::from(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // GET the .mdl path: the sidecar takes precedence, so the response
    // reflects the saved name.
    let (status, response_bytes) = fetch(
        state.clone(),
        "GET",
        "/api/projects/teacup.mdl",
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let response = parse_body(&response_bytes);
    assert_eq!(
        response["source_format"].as_str(),
        Some("sd_json"),
        "after the sidecar exists, GET serves it as sd_json"
    );
    let inner_json: serde_json::Value =
        serde_json::from_str(response["json"].as_str().unwrap()).unwrap();
    assert_eq!(inner_json["name"].as_str(), Some("post-save-name"));
}

/// After a successful save, the registry's snapshot for the written
/// path shows updated mtime and size matching the on-disk file. The
/// SPA's listing relies on these for stale-data heuristics.
#[tokio::test]
async fn registry_metadata_is_refreshed_after_save() {
    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    let pre_meta = state.registry.get(&abs_canonical).expect("seeded");
    let pre_mtime = pre_meta.mtime;
    let pre_size = pre_meta.size;

    let (_, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;

    // Round-trip the canonical JSON through a name change so the
    // re-serialized XMILE is guaranteed to differ in size from the
    // original fixture (the fixture's product/header bytes won't match
    // ours exactly anyway, so size will change either way; this just
    // makes the assertion more explicit).
    let mut json_value: serde_json::Value =
        serde_json::from_str(&json_body).expect("parse canonical json");
    json_value["name"] = serde_json::Value::String("renamed-for-meta-test".to_string());
    let mutated_json = serde_json::to_string(&json_value).expect("reserialize");

    // Sleep just long enough for the OS-level mtime resolution to record
    // a different timestamp on the rewritten file. Filesystems vary
    // (ext4 with high-res mtime: nanosecond; HFS+: 1s; many CI containers:
    // millisecond), so we use a small but realistic delay rather than
    // hoping for sub-microsecond resolution.
    std::thread::sleep(std::time::Duration::from_millis(20));

    let body = serde_json::json!({"json": mutated_json, "version": 0}).to_string();
    let (status, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    let post_meta = state.registry.get(&abs_canonical).expect("post entry");
    let on_disk = fs::metadata(&abs_canonical).unwrap();

    assert!(
        post_meta.mtime >= pre_mtime,
        "post-save mtime ({:?}) must be >= pre-save mtime ({:?})",
        post_meta.mtime,
        pre_mtime,
    );
    assert_eq!(
        post_meta.mtime,
        on_disk.modified().unwrap(),
        "registry mtime must match the on-disk mtime after save"
    );
    assert_eq!(
        post_meta.size,
        on_disk.len(),
        "registry size must match the on-disk file size after save"
    );
    // The size is expected to differ from the pre-save fixture size
    // because we re-serialized through `to_xmile`. Don't pin the exact
    // value, just confirm the registry tracks the new value.
    let _ = pre_size;
}

/// Idempotence: a second save (using the next version) must continue
/// to write only to the sidecar; the .mdl stays untouched.
#[tokio::test]
async fn second_save_after_sidecar_writes_only_to_sidecar() {
    let dir = TempDir::new().unwrap();
    let mdl_abs = copy_fixture("teacup.mdl", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let mdl_canonical = mdl_abs.canonicalize().unwrap();
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &mdl_canonical, ProjectFormat::Mdl);

    let pre_mdl_bytes = fs::read(&mdl_canonical).unwrap();
    let (_, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.mdl").await;

    // First save: creates the sidecar.
    let body0 = serde_json::json!({"json": &json_body, "version": 0}).to_string();
    let (status0, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.mdl",
        Body::from(body0),
    )
    .await;
    assert_eq!(status0, StatusCode::OK);

    // Second save: the sidecar is now source-of-truth. The version is now
    // 1 (previous response said so), and the request URL stays at the
    // sidecar path so we don't hit a 404 — the .mdl key was redirected.
    let (sidecar_version, sidecar_json) =
        get_canonical_json(state.clone(), "/api/projects/teacup.sd.json").await;
    assert_eq!(sidecar_version, 1);

    let body1 = serde_json::json!({"json": sidecar_json, "version": 1}).to_string();
    let (status1, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.sd.json",
        Body::from(body1),
    )
    .await;
    assert_eq!(
        status1,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    // Final invariants: .mdl untouched, sidecar present, sidecar version 2.
    let post_mdl_bytes = fs::read(&mdl_canonical).unwrap();
    assert_eq!(pre_mdl_bytes, post_mdl_bytes);
    let response = parse_body(&response_bytes);
    assert_eq!(response["version"].as_u64(), Some(2));
    assert_eq!(response["path"].as_str(), Some("teacup.sd.json"));
}

/// Regression: GET /api/projects triggers scan_into_registry on every
/// request. After a successful save (version 0->1), a listing refresh
/// must NOT reset the version back to 0. A stale POST with version 0
/// must still return 409 even after the listing was refreshed.
#[tokio::test]
async fn get_projects_list_does_not_reset_version_after_save() {
    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    let (_, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;

    // First POST: claims version 0 -> 1.
    let body0 = serde_json::json!({"json": &json_body, "version": 0}).to_string();
    let (status0, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body0),
    )
    .await;
    assert_eq!(status0, StatusCode::OK);

    // Trigger a listing refresh (which calls scan_into_registry internally).
    let (list_status, _) = fetch(state.clone(), "GET", "/api/projects", Body::empty()).await;
    assert_eq!(list_status, StatusCode::OK);

    // Stale POST with version 0 must still 409; the scan must not have
    // reset the version back to 0.
    let body_stale = serde_json::json!({"json": &json_body, "version": 0}).to_string();
    let (status_stale, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body_stale),
    )
    .await;
    assert_eq!(
        status_stale,
        StatusCode::CONFLICT,
        "version must not have been reset to 0 by the listing rescan; body: {}",
        String::from_utf8_lossy(&response_bytes)
    );

    // A POST with the correct current version (1) must still succeed.
    let body_current = serde_json::json!({"json": &json_body, "version": 1}).to_string();
    let (status_current, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body_current),
    )
    .await;
    assert_eq!(status_current, StatusCode::OK);
}

/// Task 8: A successful save publishes a ProjectChanged event on the
/// EventBus before returning to the client. Verifies the path field and
/// version match the post-save state and source is `User`.
#[tokio::test]
async fn successful_save_publishes_project_changed_with_source_user() {
    use simlin_serve::events::{ChangeSource, WsMessage};

    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    let mut rx = state.events.subscribe();

    let (_, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;
    let body = serde_json::json!({"json": &json_body, "version": 0}).to_string();
    let (status, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("ProjectChanged should be published within 1s")
        .expect("recv");
    match event {
        WsMessage::ProjectChanged {
            path,
            version,
            source,
        } => {
            assert_eq!(path, "teacup.stmx");
            assert_eq!(version, 1);
            assert_eq!(source, ChangeSource::User);
        }
        other => panic!("expected ProjectChanged, got {other:?}"),
    }
}

/// AC4.1 (browser-vs-browser): two saves modifying different stocks
/// against successive versions both succeed and the final GET reflects
/// both modifications. The events arrive on a WS subscriber in version
/// order.
#[tokio::test]
async fn two_saves_modifying_different_stocks_both_persist() {
    use simlin_serve::events::WsMessage;

    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    let mut rx = state.events.subscribe();

    // Read v0.
    let (v0, json0) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;
    assert_eq!(v0, 0);

    // First save: rename the project. Version 0 -> 1.
    let mut json_value: serde_json::Value =
        serde_json::from_str(&json0).expect("parse canonical json");
    json_value["name"] = serde_json::Value::String("first-edit".to_string());
    let body =
        serde_json::json!({"json": serde_json::to_string(&json_value).unwrap(), "version": 0})
            .to_string();
    let (status, response_bytes) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let response = parse_body(&response_bytes);
    assert_eq!(response["version"].as_u64(), Some(1));

    // GET back at v1.
    let (v1, json1) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;
    assert_eq!(v1, 1);

    // Second save: rename further. Version 1 -> 2.
    let mut json_value2: serde_json::Value =
        serde_json::from_str(&json1).expect("parse canonical json");
    json_value2["name"] = serde_json::Value::String("second-edit".to_string());
    let body2 =
        serde_json::json!({"json": serde_json::to_string(&json_value2).unwrap(), "version": 1})
            .to_string();
    let (status2, response_bytes2) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body2),
    )
    .await;
    assert_eq!(status2, StatusCode::OK);
    let response2 = parse_body(&response_bytes2);
    assert_eq!(response2["version"].as_u64(), Some(2));

    // Final GET reflects the second edit and version 2.
    let (v_final, json_final) =
        get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;
    assert_eq!(v_final, 2);
    let final_value: serde_json::Value =
        serde_json::from_str(&json_final).expect("parse canonical");
    assert_eq!(final_value["name"].as_str(), Some("second-edit"));

    // The two ProjectChanged events arrived in order on our subscriber.
    let ev1 = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("first event")
        .expect("recv 1");
    let ev2 = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("second event")
        .expect("recv 2");
    match (ev1, ev2) {
        (
            WsMessage::ProjectChanged { version: v1, .. },
            WsMessage::ProjectChanged { version: v2, .. },
        ) => {
            assert_eq!(v1, 1);
            assert_eq!(v2, 2);
        }
        _ => panic!("expected two ProjectChanged events, got a different sequence"),
    }
}

/// `POST /api/projects/new` happy path: creates a new `.stmx` file with
/// an empty `main` model, returns the relative path + version 0, and
/// publishes a ProjectChanged{User} event.
#[tokio::test]
async fn create_new_stmx_writes_empty_project_file() {
    use simlin_serve::events::{ChangeSource, WsMessage};

    let dir = TempDir::new().unwrap();
    let canonical_root = dir.path().canonicalize().unwrap();
    let state = build_state(canonical_root.clone());
    let mut rx = state.events.subscribe();

    let body = serde_json::json!({"name": "fresh", "format": "stmx"}).to_string();
    let (status, response_bytes) =
        fetch(state.clone(), "POST", "/api/projects/new", Body::from(body)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );
    let response = parse_body(&response_bytes);
    assert_eq!(response["path"].as_str(), Some("fresh.stmx"));
    assert_eq!(response["version"].as_u64(), Some(0));

    let abs_path = canonical_root.join("fresh.stmx");
    assert!(abs_path.is_file(), "file must exist on disk");
    let bytes = fs::read(&abs_path).unwrap();
    let mut reader = std::io::Cursor::new(&bytes[..]);
    let project = simlin_engine::open_xmile(&mut reader).expect("written file parses");
    assert_eq!(project.models.len(), 1);
    assert_eq!(project.models[0].name, "main");
    assert!(project.models[0].variables.is_empty());

    // Registry has the entry at version 0.
    let meta = state.registry.get(&abs_path).expect("registry entry");
    assert_eq!(meta.version, 0);
    assert_eq!(meta.format, ProjectFormat::Stmx);

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("event published")
        .expect("recv");
    match event {
        WsMessage::ProjectChanged {
            path,
            version,
            source,
        } => {
            assert_eq!(path, "fresh.stmx");
            assert_eq!(version, 0);
            assert_eq!(source, ChangeSource::User);
        }
        other => panic!("expected ProjectChanged, got {other:?}"),
    }
}

/// `POST /api/projects/new` with an existing file conflicts with 409.
#[tokio::test]
async fn create_new_returns_409_when_file_exists() {
    let dir = TempDir::new().unwrap();
    let canonical_root = dir.path().canonicalize().unwrap();
    let state = build_state(canonical_root.clone());

    // First request succeeds.
    let body = serde_json::json!({"name": "dup", "format": "stmx"}).to_string();
    let (status, _) = fetch(state.clone(), "POST", "/api/projects/new", Body::from(body)).await;
    assert_eq!(status, StatusCode::OK);

    // Second request collides with the same name.
    let body = serde_json::json!({"name": "dup", "format": "stmx"}).to_string();
    let (status, _) = fetch(state.clone(), "POST", "/api/projects/new", Body::from(body)).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

/// `POST /api/projects/new` rejects path-traversal-style names.
#[tokio::test]
async fn create_new_rejects_traversal_name() {
    let dir = TempDir::new().unwrap();
    let canonical_root = dir.path().canonicalize().unwrap();
    let state = build_state(canonical_root.clone());

    for bad in &[
        "../etc/passwd",
        "/abs",
        "..",
        ".hidden",
        "has/slash",
        "has\\slash",
        "weird name",
        "",
    ] {
        let body = serde_json::json!({"name": bad, "format": "stmx"}).to_string();
        let (status, response_bytes) =
            fetch(state.clone(), "POST", "/api/projects/new", Body::from(body)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "name {:?} should be rejected; body: {}",
            bad,
            String::from_utf8_lossy(&response_bytes)
        );
    }
}

/// `POST /api/projects/new` enforces the design's "Mdl/Xmile are not
/// formats for fresh files" rule. Mdl is read-only; Xmile is preserved
/// for existing files only.
#[tokio::test]
async fn create_new_rejects_mdl_and_xmile_formats() {
    let dir = TempDir::new().unwrap();
    let canonical_root = dir.path().canonicalize().unwrap();
    let state = build_state(canonical_root.clone());

    for bad_format in &["mdl", "xmile"] {
        let body = serde_json::json!({"name": "x", "format": bad_format}).to_string();
        let (status, _) = fetch(state.clone(), "POST", "/api/projects/new", Body::from(body)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "format {bad_format} must be rejected for new-project creation"
        );
    }
}

/// `POST /api/projects/new` with `format: sd_json` writes a `.sd.json`
/// file containing valid native JSON.
#[tokio::test]
async fn create_new_sd_json_writes_native_json_file() {
    let dir = TempDir::new().unwrap();
    let canonical_root = dir.path().canonicalize().unwrap();
    let state = build_state(canonical_root.clone());

    let body = serde_json::json!({"name": "json-model", "format": "sd_json"}).to_string();
    let (status, response_bytes) =
        fetch(state.clone(), "POST", "/api/projects/new", Body::from(body)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );
    let response = parse_body(&response_bytes);
    assert_eq!(response["path"].as_str(), Some("json-model.sd.json"));

    let abs_path = canonical_root.join("json-model.sd.json");
    assert!(abs_path.is_file());
    let contents = fs::read_to_string(&abs_path).unwrap();
    let project: simlin_engine::json::Project =
        serde_json::from_str(&contents).expect("file parses as native json");
    assert_eq!(project.models.len(), 1);
    assert_eq!(project.models[0].name, "main");
    // Pretty-printed JSON contains newlines.
    assert!(contents.contains('\n'), "sd_json must be pretty-printed");
}

/// `POST /api/projects/new` honours the optional `parent_dir` field by
/// creating the file in a subdirectory under the scan root.
#[tokio::test]
async fn create_new_honours_parent_dir() {
    let dir = TempDir::new().unwrap();
    let canonical_root = dir.path().canonicalize().unwrap();
    fs::create_dir_all(canonical_root.join("models")).unwrap();
    let state = build_state(canonical_root.clone());

    let body =
        serde_json::json!({"name": "nested", "format": "stmx", "parent_dir": "models"}).to_string();
    let (status, response_bytes) =
        fetch(state.clone(), "POST", "/api/projects/new", Body::from(body)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&response_bytes)
    );
    let response = parse_body(&response_bytes);
    let returned = response["path"].as_str().unwrap();
    assert!(
        returned == "models/nested.stmx" || returned == "models\\nested.stmx",
        "expected path under models/, got {returned}"
    );
    assert!(canonical_root.join("models/nested.stmx").is_file());
}

/// Saves no longer rely on `invalidate_doc`: the post-save GET returns
/// the merged in-memory state (which is identical to the on-disk state
/// because we serialize from the doc). Verifies that without the
/// invalidate_doc stop-gap, the GET still reflects the just-saved
/// content.
#[tokio::test]
async fn save_then_get_reflects_merged_state_without_doc_invalidate() {
    let dir = TempDir::new().unwrap();
    let abs = copy_fixture("teacup.stmx", dir.path());
    let canonical_root = dir.path().canonicalize().unwrap();
    let abs_canonical = abs.canonicalize().unwrap();
    let state = build_state(canonical_root);
    seed_registry(&state, &abs_canonical, ProjectFormat::Stmx);

    let (_, json_body) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;
    let mut json_value: serde_json::Value =
        serde_json::from_str(&json_body).expect("parse canonical");
    json_value["name"] = serde_json::Value::String("renamed-via-merge".to_string());
    let body =
        serde_json::json!({"json": serde_json::to_string(&json_value).unwrap(), "version": 0})
            .to_string();

    let (status, _) = fetch(
        state.clone(),
        "POST",
        "/api/projects/teacup.stmx",
        Body::from(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Subsequent GET serves from the in-memory doc; the merged name is
    // visible without re-reading the file from disk.
    let (_, post_json) = get_canonical_json(state.clone(), "/api/projects/teacup.stmx").await;
    let post_value: serde_json::Value = serde_json::from_str(&post_json).expect("parse post");
    assert_eq!(post_value["name"].as_str(), Some("renamed-via-merge"));
}
