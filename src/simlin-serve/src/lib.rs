// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![deny(unsafe_code)]

pub mod cli;
pub mod discovery;
pub mod git;
pub mod handlers;
pub mod parse;
pub mod registry;
pub mod scan;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::handlers::{AppState, get_project, list_projects};

/// Maximum accepted request body size. Phase 1 is read-only so this is
/// conservative; Phase 2 may bump this when the save path lands and large
/// XMILE/MDL projects need to round-trip.
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Build the HTTP router with the `AppState` shared across all handlers.
/// Exposed as a library function so integration tests and future callers can
/// exercise the router without spawning a process or binding a TCP port.
///
/// Layers (outer-to-inner): body-size limit, request tracing. The limit is
/// applied first so an oversized body is rejected before any tracing event
/// records its size.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/projects", get(list_projects))
        .route("/api/projects/{*rel_path}", get(get_project))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}
