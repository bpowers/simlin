// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![deny(unsafe_code)]

pub mod cli;
pub mod discovery;
pub mod git;
pub mod handlers;
pub mod launcher;
pub mod loro_doc;
pub mod parse;
pub mod registry;
pub mod scan;
pub mod static_assets;
pub mod token;
pub mod validation;
pub mod writer;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::handlers::{AppState, get_project, list_projects, save_project};
use crate::static_assets::static_handler;

/// Maximum accepted request body size. POST bodies carry the full
/// canonical JSON of an edited model; 16 MiB comfortably accommodates
/// even large real-world projects (the largest fixtures in
/// `test/test-models` are well under 1 MiB serialized). Bumped from the
/// 4 MiB read-only Phase 1 limit when Phase 2's save path landed.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

/// Build the HTTP router with the `AppState` shared across all handlers.
/// Exposed as a library function so integration tests and future callers can
/// exercise the router without spawning a process or binding a TCP port.
///
/// Layers (outer-to-inner): body-size limit, request tracing. The limit is
/// applied first so an oversized body is rejected before any tracing event
/// records its size.
pub fn build_router(state: AppState) -> Router {
    // The `/api/...` and `/healthz` routes are registered with `.route(...)`
    // so they take precedence over the SPA fallback, which catches everything
    // else (including unknown SPA routes — see `static_handler`).
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/projects", get(list_projects))
        .route(
            "/api/projects/{*rel_path}",
            get(get_project).post(save_project),
        )
        .fallback(static_handler)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}
