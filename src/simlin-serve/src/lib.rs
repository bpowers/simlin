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
use tower_http::trace::TraceLayer;

use crate::handlers::{AppState, get_project, list_projects};

/// Build the HTTP router with the `AppState` shared across all handlers.
/// Exposed as a library function so integration tests and future callers can
/// exercise the router without spawning a process or binding a TCP port.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/projects", get(list_projects))
        .route("/api/projects/{*rel_path}", get(get_project))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}
