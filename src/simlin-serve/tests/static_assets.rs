// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for the embedded static-asset handler. The tests skip
//! cleanly when `web/dist/` does not exist (e.g. on CI without
//! `SIMLIN_SERVE_BUILD_WEB=1`); skipping is documented behavior so the
//! whole-workspace test command still passes in lean configurations.

#![deny(unsafe_code)]

use std::path::Path;
use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::test_support::unavailable_git_probe;
use tempfile::TempDir;
use tower::ServiceExt;

// Synthetic ports for the host validator middleware (Phase 8 Task 8).
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

fn web_dist_index_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("web/dist/index.html")
}

fn web_dist_is_built() -> bool {
    web_dist_index_path().is_file()
}

fn build_state() -> AppState {
    let dir = TempDir::new().unwrap();
    let canonical = dir.path().canonicalize().unwrap();
    // Leak the tempdir so the AppState's root remains valid for the duration
    // of a oneshot. A oneshot test never reads files so there's nothing to
    // clean up; leaking avoids an awkward lifetime on the helper.
    std::mem::forget(dir);
    AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(unavailable_git_probe()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(String::new()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    }
}

#[tokio::test]
async fn root_path_serves_index_html() {
    if !web_dist_is_built() {
        eprintln!("web/dist not built; skipping static_assets::root_path_serves_index_html");
        return;
    }

    let app = build_router(build_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/")
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(content_type.starts_with("text/html"), "got {content_type}");

    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(
        body.contains("<div id=\"root\"></div>"),
        "expected SPA root marker in index.html"
    );
}

#[tokio::test]
async fn unknown_extensionless_path_falls_through_to_index() {
    if !web_dist_is_built() {
        eprintln!(
            "web/dist not built; skipping static_assets::unknown_extensionless_path_falls_through_to_index"
        );
        return;
    }

    let app = build_router(build_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/some/spa/route")
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "extensionless paths should fall through to index.html for SPA routing"
    );
}

#[tokio::test]
async fn missing_asset_with_extension_returns_404() {
    if !web_dist_is_built() {
        eprintln!(
            "web/dist not built; skipping static_assets::missing_asset_with_extension_returns_404"
        );
        return;
    }

    let app = build_router(build_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/assets/missing-asset.js")
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "paths containing '.' that miss the embed should be 404, not SPA fallback"
    );
}

#[tokio::test]
async fn api_routes_take_precedence_over_static_fallback() {
    // The /api/projects route is registered with .route(...), so it must beat
    // the SPA fallback even when web/dist isn't built.
    let app = build_router(build_state());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        content_type.starts_with("application/json"),
        "expected JSON, got {content_type}"
    );
}
