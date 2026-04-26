// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for the `host_validator_middleware` (Phase 8 Task 8).
//!
//! Defense-in-depth against DNS rebinding (cf. CVE-2025-66414). Every
//! HTTP request, regardless of route, must carry a `Host:` header
//! matching `127.0.0.1:<ui_port>`, `localhost:<ui_port>`,
//! `127.0.0.1:<mcp_port>`, or `localhost:<mcp_port>`. Anything else is
//! rejected with `421 Misdirected Request` before the inner handler
//! runs. A missing `Host:` header is also rejected — HTTP/1.1 mandates
//! one, and HTTP/1.0 clients (which may omit it) are not the target
//! audience for a local-only browser server.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::mcp::build_mcp_router;
use simlin_serve::registry::ProjectRegistry;
use tempfile::TempDir;
use tower::ServiceExt;

const UI_PORT: u16 = 54321;
const MCP_PORT: u16 = 7878;

fn build_state() -> (AppState, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let canonical = dir.path().canonicalize().expect("canonicalize");
    let state = AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(String::new()),
        ui_port: UI_PORT,
        mcp_port: MCP_PORT,
        strict_origin: true,
    };
    (state, dir)
}

#[tokio::test]
async fn allowed_host_127_0_0_1_passes() {
    let (state, _dir) = build_state();
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .header(header::HOST, format!("127.0.0.1:{UI_PORT}"))
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn allowed_host_localhost_passes() {
    let (state, _dir) = build_state();
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .header(header::HOST, format!("localhost:{UI_PORT}"))
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn host_with_attacker_dns_name_is_rejected_with_421() {
    let (state, _dir) = build_state();
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .header(header::HOST, format!("evil.example.com:{UI_PORT}"))
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn host_with_wrong_port_is_rejected_with_421() {
    // 127.0.0.1 is allowlisted, but only at the actual ui_port and
    // mcp_port. A request claiming 127.0.0.1 on a random port comes
    // from a client misbehaving (or rebinding) rather than the SPA.
    let (state, _dir) = build_state();
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .header(header::HOST, "127.0.0.1:1234".to_string())
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn missing_host_header_is_rejected_with_421() {
    let (state, _dir) = build_state();
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn ui_router_accepts_mcp_port_host_too() {
    // The middleware accepts the Host header at any of the four
    // allowlisted entries; the per-router port mapping is enforced by
    // the OS bind, not by the Host check. This test guards the
    // middleware against an over-narrow allowlist that would reject
    // legitimate same-origin proxy hops.
    let (state, _dir) = build_state();
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/projects")
                .header(header::HOST, format!("127.0.0.1:{MCP_PORT}"))
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn mcp_router_rejects_attacker_host() {
    // Same defense applies to the MCP router. We don't attempt the
    // full Streamable HTTP handshake here — a 421 short-circuits ahead
    // of the rmcp service.
    let (state, _dir) = build_state();
    let app = build_mcp_router(Arc::new(state));
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/mcp")
                .header(header::HOST, "evil.example.com".to_string())
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn mcp_router_accepts_loopback_host() {
    let (state, _dir) = build_state();
    let app = build_mcp_router(Arc::new(state));
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/mcp")
                .header(header::HOST, format!("127.0.0.1:{MCP_PORT}"))
                .header(header::ACCEPT, "application/json")
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    // We don't speak the full MCP handshake here; what matters is that
    // the request was NOT rejected by the host validator (anything
    // other than 421 means the validator passed and the inner service
    // handled the request).
    assert_ne!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn healthz_is_also_gated_by_the_host_check() {
    // healthz is a public probe endpoint, but it still lives behind the
    // host check — a malicious page using rebinding to ping the local
    // server would reveal "this user runs simlin-serve" as a
    // fingerprinting signal otherwise.
    let (state, _dir) = build_state();
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/healthz")
                .header(header::HOST, "evil.example.com".to_string())
                .body(Body::empty())
                .expect("request build"),
        )
        .await
        .expect("router response");
    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
}
