// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Integration tests for `POST /api/projects/{*path}`. Exercises the
//! version-check + validation portion of Subcomponent A. The actual
//! disk-write path is covered by Subcomponent B; these tests assert
//! that the file on disk is *not* modified at this stage.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use simlin_serve::build_router;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use tempfile::TempDir;
use tower::ServiceExt;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn copy_fixture(name: &str, dest_dir: &std::path::Path) -> PathBuf {
    let src = PathBuf::from(FIXTURES_DIR).join(name);
    let dest = dest_dir.join(name);
    fs::copy(&src, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
    dest
}

fn build_state(root: PathBuf) -> AppState {
    AppState {
        registry: Arc::new(ProjectRegistry::new(root.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(root),
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
