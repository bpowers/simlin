// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use tempfile::TempDir;
use tower::ServiceExt;

// Synthetic test ports used by oneshot tests that don't bind a real
// listener — the host validator middleware (Phase 8 Task 8) checks
// the request `Host:` header against `127.0.0.1:<ui_port>`, so the
// header below must match these values.
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

#[tokio::test]
async fn healthz_returns_ok() {
    let dir = TempDir::new().expect("tempdir");
    let canonical = dir.path().canonicalize().expect("canonicalize tempdir");
    let state = AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(String::new()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    };
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/healthz")
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = to_bytes(response.into_body(), 1024)
        .await
        .expect("body bytes");
    assert_eq!(body_bytes.as_ref(), b"ok");
}
