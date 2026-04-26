// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Phase 8 Task 6 parity test.
//!
//! Verifies that the HTTP `POST /api/projects/new` endpoint and the MCP
//! `create_model` tool produce byte-identical files when invoked with
//! the same logical project name and default sim-specs.  Locks down
//! the contract that both surfaces share
//! `simlin_mcp_core::types::build_empty_project` so a future refactor
//! cannot let them drift in shape, name, or sim-spec defaults.
//!
//! `.sd.json` is the format used for byte-comparison because the MCP
//! `create_model` tool always writes NativeJson regardless of input
//! extension (it is the AI-canonical format); the HTTP endpoint
//! exposes `stmx` and `sd_json` and the JSON arm is the matching one.
//! For the XMILE arm we additionally compare the in-memory project
//! shape that both paths feed into their respective serialisers — same
//! shape, same defaults, just different writers.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use simlin_mcp_core::test_support::TestFileSystemAccess;
use simlin_mcp_core::tools::create_model::{CreateModelInput, create_model};
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use tempfile::TempDir;
use tower::ServiceExt;

fn build_state(root: PathBuf) -> AppState {
    AppState {
        registry: Arc::new(ProjectRegistry::new(root.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(root),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(String::new()),
    }
}

async fn http_post_new(state: AppState, body: serde_json::Value) -> (StatusCode, Vec<u8>) {
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/projects/new")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request build"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    (status, bytes.to_vec())
}

#[tokio::test]
async fn http_create_and_mcp_create_model_produce_byte_identical_sd_json_files() {
    let http_dir = TempDir::new().unwrap();
    let mcp_dir = TempDir::new().unwrap();
    let http_root = http_dir.path().canonicalize().unwrap();
    let mcp_root = mcp_dir.path().canonicalize().unwrap();
    let state = build_state(http_root.clone());

    let (status, body) = http_post_new(
        state,
        serde_json::json!({"name": "shared", "format": "sd_json"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create failed: {}",
        String::from_utf8_lossy(&body)
    );

    let http_path = http_root.join("shared.sd.json");
    assert!(http_path.is_file());
    let http_bytes = fs::read(&http_path).unwrap();

    let mcp_path = mcp_root.join("shared.sd.json");
    let _ = create_model(
        &TestFileSystemAccess,
        CreateModelInput {
            project_path: mcp_path.to_string_lossy().to_string(),
            sim_specs: None,
        },
    )
    .await
    .expect("mcp create_model");

    assert!(mcp_path.is_file(), "MCP create must write the file");
    let mcp_bytes = fs::read(&mcp_path).unwrap();

    assert_eq!(
        http_bytes, mcp_bytes,
        "HTTP create_new_project and MCP create_model must produce byte-identical \
         .sd.json files when they share name + default sim-specs (proves both go \
         through simlin_mcp_core::types::build_empty_project)"
    );

    // Sanity: the shared file actually parses as a native json::Project
    // with the canonical empty shape.  Note: `method` is empty on the
    // wire because the datamodel::SimMethod::Euler -> json::SimSpecs
    // round-trip elides the default ("" reads back as Euler in the
    // reverse conversion).
    let project: simlin_engine::json::Project =
        serde_json::from_slice(&http_bytes).expect("parses");
    assert_eq!(project.models.len(), 1);
    assert_eq!(project.models[0].name, "main");
    assert_eq!(project.sim_specs.start_time, 0.0);
    assert_eq!(project.sim_specs.end_time, 100.0);
    assert_eq!(project.sim_specs.dt, "0.25");
    assert_eq!(project.sim_specs.save_step, 1.0);
}

#[tokio::test]
async fn http_create_stmx_produces_xmile_with_canonical_empty_project_shape() {
    // The MCP `create_model` tool always writes NativeJson, so a
    // direct byte comparison against an XMILE write is impossible.
    // Instead, verify that the HTTP `format: stmx` path's bytes parse
    // back to a project with the same shape that
    // `build_empty_project` produces — the assertion that locks down
    // the shared helper for the XMILE arm.
    let http_dir = TempDir::new().unwrap();
    let http_root = http_dir.path().canonicalize().unwrap();
    let state = build_state(http_root.clone());

    let (status, body) = http_post_new(
        state,
        serde_json::json!({"name": "shared", "format": "stmx"}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "create failed: {}",
        String::from_utf8_lossy(&body)
    );

    let http_path = http_root.join("shared.stmx");
    let xmile_bytes = fs::read(&http_path).unwrap();

    let mut reader = std::io::Cursor::new(&xmile_bytes[..]);
    let parsed = simlin_engine::open_xmile(&mut reader).expect("xmile parses");

    // The canonical empty project: one `main` model with no variables
    // and the canonical default sim-specs (build_empty_project's
    // single source of truth).
    let canonical = simlin_mcp_core::types::build_empty_project();
    assert_eq!(parsed.models.len(), canonical.models.len());
    assert_eq!(parsed.models[0].name, canonical.models[0].name);
    assert!(parsed.models[0].variables.is_empty());
    assert_eq!(parsed.sim_specs.start, canonical.sim_specs.start);
    assert_eq!(parsed.sim_specs.stop, canonical.sim_specs.stop);
    assert_eq!(parsed.sim_specs.dt, canonical.sim_specs.dt);
    assert_eq!(parsed.sim_specs.sim_method, canonical.sim_specs.sim_method);
}
