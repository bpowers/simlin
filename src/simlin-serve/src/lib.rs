// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![deny(unsafe_code)]

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;
use tower_http::trace::TraceLayer;

/// Build the HTTP router. Exposed as a library function so integration tests
/// and future callers can exercise the router without spawning a process or
/// binding a TCP port.
pub fn build_router() -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .layer(TraceLayer::new_for_http())
}

async fn healthz() -> (StatusCode, &'static str) {
    (StatusCode::OK, "ok")
}
