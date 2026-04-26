// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for `GET /api/projects/{*path}`. Verifies AC3.1 (read),
//! AC3.3 (read), `.mdl` sidecar preference, path traversal rejection, 404,
//! symlink escape, and malformed sidecar handling.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
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
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(root),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(String::new()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    }
}

async fn fetch(state: AppState, uri: &str) -> (StatusCode, Vec<u8>) {
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body bytes");
    (status, body.to_vec())
}

fn parse_body(body: &[u8]) -> Value {
    serde_json::from_slice(body).expect("response json")
}

#[tokio::test]
async fn ac3_1_reads_stmx_returns_canonical_json() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.stmx", dir.path());

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, body) = fetch(state, "/api/projects/teacup.stmx").await;
    assert_eq!(status, StatusCode::OK);
    let value = parse_body(&body);

    assert_eq!(value["source_format"].as_str(), Some("stmx"));
    let json_str = value["json"].as_str().expect("json field is a string");
    assert!(!json_str.is_empty(), "json field should be non-empty");
    let inner: Value = serde_json::from_str(json_str).expect("inner json parses");
    assert!(
        inner["models"].is_array() || inner["models"].is_object(),
        "inner json should have a models field"
    );
}

#[tokio::test]
async fn ac3_1_reads_xmile_returns_canonical_json() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.xmile", dir.path());

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, body) = fetch(state, "/api/projects/teacup.xmile").await;
    assert_eq!(status, StatusCode::OK);
    let value = parse_body(&body);

    assert_eq!(value["source_format"].as_str(), Some("xmile"));
    let json_str = value["json"].as_str().expect("json field is a string");
    let inner: Value = serde_json::from_str(json_str).expect("inner json parses");
    assert!(inner["models"].is_array());
}

#[tokio::test]
async fn ac3_3_reads_mdl_via_native_parser() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.mdl", dir.path());

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, body) = fetch(state, "/api/projects/teacup.mdl").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    let value = parse_body(&body);

    assert_eq!(value["source_format"].as_str(), Some("mdl"));
    let json_str = value["json"].as_str().expect("json field is a string");
    assert!(!json_str.is_empty(), "json field should be non-empty");
}

#[tokio::test]
async fn mdl_sidecar_preferred_when_present() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.mdl", dir.path());

    // Sibling `.sd.json` becomes source-of-truth once it exists.
    let sidecar = r#"{"name":"sidecar-marker","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
    fs::write(dir.path().join("teacup.sd.json"), sidecar).unwrap();

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, body) = fetch(state, "/api/projects/teacup.mdl").await;
    assert_eq!(status, StatusCode::OK);
    let value = parse_body(&body);

    assert_eq!(
        value["source_format"].as_str(),
        Some("sd_json"),
        "the sidecar overrides the .mdl source format"
    );
    let json_str = value["json"].as_str().expect("json field is a string");
    let inner: Value = serde_json::from_str(json_str).expect("inner json parses");
    assert_eq!(
        inner["name"].as_str(),
        Some("sidecar-marker"),
        "the response body should reflect the sidecar contents, not the parsed mdl"
    );
}

#[tokio::test]
async fn path_traversal_via_dotdot_is_rejected() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.stmx", dir.path());

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, _) = fetch(state, "/api/projects/../../etc/passwd").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::FORBIDDEN,
        "expected 400 or 403 for traversal, got {status}"
    );
}

#[tokio::test]
async fn missing_file_returns_404() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.stmx", dir.path());

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, _) = fetch(state, "/api/projects/missing.stmx").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn null_byte_in_path_is_rejected() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.stmx", dir.path());

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    // The percent-encoded null byte (%00) must be rejected before any
    // filesystem lookup.
    let (status, _) = fetch(state, "/api/projects/teacup.stmx%00.bak").await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::FORBIDDEN,
        "expected 400 or 403 for null byte, got {status}"
    );
}

#[tokio::test]
async fn sd_json_format_round_trips() {
    let dir = TempDir::new().unwrap();
    let json = r#"{"name":"empty","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
    fs::write(dir.path().join("model.sd.json"), json).unwrap();

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, body) = fetch(state, "/api/projects/model.sd.json").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "body: {}",
        String::from_utf8_lossy(&body)
    );
    let value = parse_body(&body);

    assert_eq!(value["source_format"].as_str(), Some("sd_json"));
    let json_str = value["json"].as_str().expect("json field is a string");
    let inner: Value = serde_json::from_str(json_str).expect("inner json parses");
    assert_eq!(inner["name"].as_str(), Some("empty"));
}

// On Unix, a symlink inside the scan root that points outside must be
// rejected with 403. The dot-dot traversal check catches most cases at
// the sanitization layer, but a symlink is only caught after
// `canonicalize` because it looks like a normal path component.
#[cfg(unix)]
#[tokio::test]
async fn symlink_escape_returns_403() {
    use std::os::unix::fs::symlink;

    // `outside` holds a real file that lives outside the scan root.
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("secret.stmx"), "<secret/>").unwrap();

    // `root` is the scan root. `innocent.stmx` is a symlink that
    // passes the sanitize_rel_path check (no `..`) but resolves outside.
    let root = TempDir::new().unwrap();
    symlink(
        outside.path().join("secret.stmx"),
        root.path().join("innocent.stmx"),
    )
    .unwrap();

    // canonicalize the root so AppState matches production behavior.
    let canonical_root = root.path().canonicalize().unwrap();
    let state = build_state(canonical_root);

    let (status, _body) = fetch(state, "/api/projects/innocent.stmx").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "a symlink pointing outside the scan root must return 403"
    );
}

// When a `.mdl` file has a sibling `.sd.json` sidecar that contains
// invalid JSON, the handler must return 400 Bad Request (not 500).
#[tokio::test]
async fn malformed_sidecar_returns_400() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.mdl", dir.path());

    // Sidecar exists but is not valid JSON.
    fs::write(dir.path().join("teacup.sd.json"), "not actually json").unwrap();

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, body) = fetch(state, "/api/projects/teacup.mdl").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "malformed sidecar JSON must yield 400; body: {}",
        String::from_utf8_lossy(&body)
    );
    let value = parse_body(&body);
    let error_msg = value["error"].as_str().expect("error field present");
    assert!(
        !error_msg.is_empty(),
        "error body should carry a non-empty message"
    );
}

// When a `.mdl` sidecar is syntactically valid JSON but not a Project
// shape, the deserializer will fail and the handler must return 400.
#[tokio::test]
async fn sidecar_with_wrong_shape_returns_400() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.mdl", dir.path());

    // Valid JSON but missing the required Project fields.
    fs::write(dir.path().join("teacup.sd.json"), r#"{"foo":"bar"}"#).unwrap();

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, body) = fetch(state, "/api/projects/teacup.mdl").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "wrong-shape sidecar JSON must yield 400; body: {}",
        String::from_utf8_lossy(&body)
    );
}

// Phase 3 Task 5: after the first GET, the registry's ProjectMeta.doc
// is populated and serves as the source of truth on subsequent reads.
// Two consecutive GETs must yield byte-identical canonical JSON.
//
// The "delete file between GETs" technique the plan suggests for proving
// the cache hit doesn't survive the path-resolution canonicalize() call
// (which requires the file to exist for the security check). So we
// verify the property by mutating the on-disk file between calls and
// asserting the second call returns the *cached* (unchanged) content
// rather than the disk-edited content.
#[tokio::test]
async fn second_get_returns_cached_content_after_external_disk_edit() {
    let dir = TempDir::new().unwrap();
    let file_path = copy_fixture("teacup.stmx", dir.path());
    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    // First GET: hydrates the doc from disk.
    let (status, body1) = fetch(state.clone(), "/api/projects/teacup.stmx").await;
    assert_eq!(status, StatusCode::OK);
    let value1 = parse_body(&body1);
    let json1 = value1["json"].as_str().expect("json string").to_owned();

    // External edit: replace the on-disk file with completely different
    // (still-parseable) XMILE. If the GET re-read disk on every call,
    // the second response would reflect this change.
    let unrelated = r#"<?xml version="1.0" encoding="UTF-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0"><header><name>changed-after-first-get</name></header><sim_specs><start>0</start><stop>10</stop><dt>1</dt></sim_specs><model><variables/></model></xmile>"#;
    fs::write(&file_path, unrelated).expect("rewrite file");

    let (status, body2) = fetch(state.clone(), "/api/projects/teacup.stmx").await;
    assert_eq!(status, StatusCode::OK);
    let value2 = parse_body(&body2);
    assert_eq!(
        value2["json"].as_str(),
        Some(json1.as_str()),
        "second GET should return the cached doc state, not the disk-edited content"
    );
}

// Sanity check Task 5's response shape: the wire shape must remain
// `{ json, version, source_format }` — doc-sourcing changes the source
// of truth but not the response schema.
#[tokio::test]
async fn doc_sourced_response_keeps_phase1_shape() {
    let dir = TempDir::new().unwrap();
    copy_fixture("teacup.stmx", dir.path());
    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical);

    let (status, body) = fetch(state, "/api/projects/teacup.stmx").await;
    assert_eq!(status, StatusCode::OK);
    let value = parse_body(&body);

    assert!(value.get("json").is_some(), "json field present");
    assert!(value.get("version").is_some(), "version field present");
    assert!(
        value.get("source_format").is_some(),
        "source_format field present"
    );
    assert!(value["json"].is_string(), "json is a string");
    assert!(value["version"].is_number(), "version is a number");
    assert_eq!(value["source_format"].as_str(), Some("stmx"));
}
