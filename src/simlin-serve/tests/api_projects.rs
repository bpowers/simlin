// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for `GET /api/projects`. Verifies AC1.1, AC1.2, AC2.5.

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
use simlin_serve::test_support::unavailable_git_probe;
use tempfile::TempDir;
use tower::ServiceExt;

// Synthetic ports for the host validator middleware (Phase 8 Task 8).
// Matches `Host:` headers below.
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

fn touch(dir: &std::path::Path, rel: &str, contents: &[u8]) -> PathBuf {
    let p = dir.join(rel);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(&p, contents).expect("write file");
    p
}

fn build_state(root: PathBuf, git: GitProbe) -> AppState {
    AppState {
        registry: Arc::new(ProjectRegistry::new(root.clone())),
        git: Arc::new(git),
        root: Arc::new(root),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(String::new()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    }
}

async fn fetch_projects(state: AppState) -> Value {
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::OK);
    let body_bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body bytes");
    serde_json::from_slice(&body_bytes).expect("response json")
}

#[tokio::test]
async fn ac1_1_lists_all_three_format_types() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "model_a.stmx", b"<root/>\n");
    touch(dir.path(), "model_b.xmile", b"<root/>\n");
    touch(dir.path(), "model_c.mdl", b"contents");

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical, unavailable_git_probe());

    let body = fetch_projects(state).await;
    let projects = body["projects"].as_array().expect("projects array");
    assert_eq!(projects.len(), 3, "expected three discovered models");

    let formats: Vec<&str> = projects
        .iter()
        .map(|p| p["format"].as_str().expect("format string"))
        .collect();
    assert!(formats.contains(&"stmx"));
    assert!(formats.contains(&"xmile"));
    assert!(formats.contains(&"mdl"));
}

#[tokio::test]
async fn ac1_2_relative_paths_use_forward_slashes() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "sub/d.stmx", b"<root/>\n");
    touch(dir.path(), "deep/nested/e.xmile", b"<root/>\n");

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical, unavailable_git_probe());

    let body = fetch_projects(state).await;
    let projects = body["projects"].as_array().expect("projects array");
    let paths: Vec<&str> = projects
        .iter()
        .map(|p| p["path"].as_str().expect("path string"))
        .collect();
    assert!(
        paths.contains(&"sub/d.stmx"),
        "expected forward-slashed relative path, got {paths:?}"
    );
    assert!(
        paths.contains(&"deep/nested/e.xmile"),
        "expected forward-slashed nested relative path, got {paths:?}"
    );
    for path in &paths {
        assert!(
            !path.contains('\\'),
            "path should never contain backslashes on the wire: {path}"
        );
    }
}

#[tokio::test]
async fn ac2_5_unavailable_git_propagates_to_response() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "model.stmx", b"<root/>\n");

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical, unavailable_git_probe());

    let body = fetch_projects(state).await;
    assert_eq!(
        body["git_available"].as_bool(),
        Some(false),
        "git_available should reflect the GitProbe state"
    );

    let projects = body["projects"].as_array().expect("projects array");
    for project in projects {
        let kind = project["git"]["kind"].as_str().expect("git.kind");
        assert_eq!(
            kind, "unavailable",
            "every entry should report git.kind=unavailable when the probe is unavailable"
        );
    }
}

#[tokio::test]
async fn snapshot_is_sorted_alphabetically_by_path() {
    let dir = TempDir::new().unwrap();
    touch(dir.path(), "zebra.stmx", b"<root/>\n");
    touch(dir.path(), "apple.stmx", b"<root/>\n");
    touch(dir.path(), "middle/banana.xmile", b"<root/>\n");

    let canonical = dir.path().canonicalize().unwrap();
    let state = build_state(canonical, unavailable_git_probe());

    let body = fetch_projects(state).await;
    let projects = body["projects"].as_array().expect("projects array");
    let paths: Vec<&str> = projects
        .iter()
        .map(|p| p["path"].as_str().expect("path string"))
        .collect();
    assert_eq!(
        paths,
        vec!["apple.stmx", "middle/banana.xmile", "zebra.stmx"]
    );
}
