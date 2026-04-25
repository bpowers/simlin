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
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use tempfile::TempDir;
use tower::ServiceExt;

const OVERSIZED_BYTES: usize = 5 * 1024 * 1024;

fn build_state() -> AppState {
    let dir = TempDir::new().expect("tempdir");
    let canonical = dir.path().canonicalize().expect("canonicalize");
    // Leak the tempdir so its lifetime spans the request. For tests this is
    // acceptable because each test process is short-lived.
    std::mem::forget(dir);
    AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(canonical),
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
                .header(header::CONTENT_LENGTH, "0")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::OK);
}
