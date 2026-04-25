// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use simlin_serve::build_router;
use tower::ServiceExt;

#[tokio::test]
async fn healthz_returns_ok() {
    let app = build_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/healthz")
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
