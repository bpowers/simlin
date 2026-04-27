// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for the layers mounted on `build_router`. We verify the
//! body-limit layer rejects oversized requests via the `Content-Length`
//! header check (which `tower-http`'s `RequestBodyLimitLayer` performs before
//! invoking any inner service, so we can observe the rejection without
//! actually streaming a giant body through axum).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::test_support::unavailable_git_probe;
use tempfile::TempDir;
use tower::ServiceExt;

// Phase 2 bumped the body limit to 16 MiB; pick a value comfortably
// above that so this test continues to exercise the rejection path.
const OVERSIZED_BYTES: usize = 17 * 1024 * 1024;

// Synthetic ports that match the `Host:` header on every request below.
// The host validator middleware (Phase 8 Task 8) rejects mismatched values
// with 421 Misdirected Request before the body-limit layer even fires.
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

fn build_state() -> AppState {
    let dir = TempDir::new().expect("tempdir");
    let canonical = dir.path().canonicalize().expect("canonicalize");
    // Leak the tempdir so its lifetime spans the request. For tests this is
    // acceptable because each test process is short-lived.
    std::mem::forget(dir);
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

#[tokio::test]
async fn oversized_content_length_is_rejected_by_body_limit() {
    // The layer checks `Content-Length` ahead of invoking the inner service
    // (per tower-http's documentation). Setting the header on a request to
    // an existing route lets us observe the rejection without streaming a
    // 5 MiB body through axum's per-route routing first.
    let app = build_router(build_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .header(header::CONTENT_LENGTH, OVERSIZED_BYTES.to_string())
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");

    assert_eq!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "RequestBodyLimitLayer should reject Content-Length above the cap"
    );
}

#[tokio::test]
async fn within_limit_request_passes_through() {
    // Sanity check: a request announcing a small body passes the limit and
    // reaches the handler.
    let app = build_router(build_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/healthz")
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .header(header::CONTENT_LENGTH, "0")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::OK);
}
