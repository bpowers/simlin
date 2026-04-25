// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for `GET /api/projects/{*path}`. Verifies AC3.1 (read),
//! AC3.3 (read), `.mdl` sidecar preference, path traversal rejection, 404.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use simlin_serve::build_router;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
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

async fn fetch(state: AppState, uri: &str) -> (StatusCode, Vec<u8>) {
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(uri)
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
