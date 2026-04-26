// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end smoke test exercising every public route the Phase 1 binary
//! exposes against a single tempdir-backed fixture: `/healthz`,
//! `/api/projects`, `/api/projects/teacup.stmx`, and `/`.
//!
//! `/` is skipped (with a documented eprintln) when `web/dist/index.html`
//! is missing — that lets `cargo test --workspace` keep passing on
//! configurations that haven't run the frontend build.

#![deny(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::test_support::unavailable_git_probe;
use tempfile::TempDir;
use tower::ServiceExt;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
// Synthetic ports for the host validator middleware (Phase 8 Task 8).
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

fn web_dist_present() -> bool {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("web/dist/index.html")
        .is_file()
}

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
        launch_token: Arc::new(String::new()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    }
}

async fn one_shot(state: AppState, uri: &str) -> (StatusCode, Vec<u8>, Option<String>) {
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
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("body bytes");
    (status, body.to_vec(), content_type)
}

#[tokio::test]
async fn end_to_end_phase_one_routes_smoke() {
    let dir = TempDir::new().expect("tempdir");
    copy_fixture("teacup.stmx", dir.path());
    let canonical = dir.path().canonicalize().expect("canonicalize tempdir");

    // /healthz — always works, regardless of frontend build state.
    let (status, body, _) = one_shot(build_state(canonical.clone()), "/healthz").await;
    assert_eq!(status, StatusCode::OK, "healthz status");
    assert_eq!(
        body, b"ok",
        "healthz body should be the literal string 'ok'"
    );

    // /api/projects — registry returns the single fixture entry.
    let (status, body, content_type) =
        one_shot(build_state(canonical.clone()), "/api/projects").await;
    assert_eq!(status, StatusCode::OK, "list projects status");
    assert!(
        content_type
            .as_deref()
            .unwrap_or("")
            .starts_with("application/json")
    );
    let listing: Value = serde_json::from_slice(&body).expect("list projects body");
    let projects = listing["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 1, "expected one model in tempdir");
    assert_eq!(projects[0]["path"].as_str(), Some("teacup.stmx"));
    assert_eq!(projects[0]["format"].as_str(), Some("stmx"));

    // /api/projects/teacup.stmx — full read path returns canonical JSON.
    let (status, body, _) =
        one_shot(build_state(canonical.clone()), "/api/projects/teacup.stmx").await;
    assert_eq!(status, StatusCode::OK, "get project status");
    let payload: Value = serde_json::from_slice(&body).expect("get project body");
    let json_str = payload["json"].as_str().expect("json field");
    assert!(!json_str.is_empty(), "canonical json should be non-empty");
    let project: Value = serde_json::from_str(json_str).expect("nested json");
    assert!(
        project["models"].is_array(),
        "canonical project JSON should expose a `models` array"
    );
    assert_eq!(payload["source_format"].as_str(), Some("stmx"));

    // / — SPA index. Skipped when web/dist/ isn't built.
    if web_dist_present() {
        let (status, body, content_type) = one_shot(build_state(canonical.clone()), "/").await;
        assert_eq!(status, StatusCode::OK, "spa index status");
        assert!(
            content_type
                .as_deref()
                .unwrap_or("")
                .starts_with("text/html")
        );
        let html = String::from_utf8_lossy(&body);
        assert!(
            html.contains("<div id=\"root\"></div>"),
            "expected SPA root marker"
        );
    } else {
        eprintln!("web/dist not built; skipping `/` portion of e2e_smoke");
    }
}
