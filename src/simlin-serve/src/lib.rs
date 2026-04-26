// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![deny(unsafe_code)]

pub mod cli;
pub mod diagnostics;
pub mod discovery;
pub mod events;
pub mod git;
pub mod handlers;
pub mod hashing;
pub mod launcher;
pub mod loro_doc;
pub mod mcp;
pub mod middleware;
pub mod parse;
pub mod registry;
pub mod scan;
pub mod serving;
pub mod static_assets;
#[doc(hidden)]
pub mod test_support;
pub mod token;
pub mod validation;
pub mod watcher;
pub mod writer;

use axum::Router;
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::handlers::{
    AppState, create_new_project, get_project, list_projects, save_project, updates_ws_handler,
};
use crate::middleware::host_validator_middleware;
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
/// Tower's `Router::layer` calls stack in reverse application order: the last
/// `.layer()` call wraps all the others and becomes the outermost layer.
/// Applied below as host_validator → RequestBodyLimitLayer → TraceLayer, the
/// actual outer-to-inner execution order is:
///   TraceLayer (outermost) → RequestBodyLimitLayer → host_validator (innermost)
///
/// The host validator runs innermost — closest to the handlers — because the
/// body-limit and trace layers are inexpensive and applying them ahead of the
/// host check keeps the rejection path observable in tracing output.
/// (Cost-benefit goes the other way for expensive layers: the host check would
/// gate them out.)
pub fn build_router(state: AppState) -> Router {
    // The `/api/...` and `/healthz` routes are registered with `.route(...)`
    // so they take precedence over the SPA fallback, which catches everything
    // else (including unknown SPA routes — see `static_handler`).
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/projects", get(list_projects))
        // The `/api/projects/new` route must precede the wildcard
        // `/api/projects/{*rel_path}` route so a POST to `new` reaches
        // the create handler instead of being interpreted as a save
        // against a file literally named `new`. Axum's matcher walks
        // routes in the order they were registered.
        .route("/api/projects/new", post(create_new_project))
        .route(
            "/api/projects/{*rel_path}",
            get(get_project).post(save_project),
        )
        // WebSocket upgrade lives under /api/updates so it shares the
        // /api/* prefix that frontends use to distinguish data-plane
        // calls from SPA assets. The Query<WsParams> extractor enforces
        // a present `?token=...` query param ahead of the handler body.
        .route("/api/updates", get(updates_ws_handler))
        .fallback(static_handler)
        .layer(from_fn_with_state(state.clone(), host_validator_middleware))
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}
