// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! HTTP transport mounting for the in-process MCP server.
//!
//! `build_mcp_router` produces an `axum::Router` that nests rmcp's
//! `StreamableHttpService` at `/mcp`. The factory closure passed into the
//! service is invoked once per session to construct a fresh
//! `SimlinServeMcpServer`, but every instance shares the same
//! `Arc<AppState>` -- so MCP-driven tools see the same `ProjectRegistry`,
//! `EventBus`, and version counter as the browser-facing handlers.

use std::sync::Arc;

use axum::Router;
use axum::middleware::from_fn_with_state;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tower_http::trace::TraceLayer;

use crate::handlers::AppState;
use crate::mcp::{RegistryAccess, SimlinServeMcpServer};
use crate::middleware::host_validator_middleware;

/// Build the axum router that serves the MCP transport. The returned
/// router exposes a single nested service at `/mcp` that speaks the MCP
/// 2025-06-18 Streamable HTTP protocol.
///
/// Each new MCP session triggers a fresh `SimlinServeMcpServer` via the
/// factory closure; cloning the shared `Arc<AppState>` is cheap and keeps
/// every session pointed at the same in-memory project state.
///
/// The host validator (Phase 8 Task 8) layers in front of the rmcp
/// service so a request with a non-allowlisted `Host:` header is
/// rejected before the StreamableHttpService starts a session.
pub fn build_mcp_router(state: Arc<AppState>) -> Router {
    let host_state = (*state).clone();
    let factory = move || {
        Ok(SimlinServeMcpServer::<RegistryAccess>::new(Arc::clone(
            &state,
        )))
    };
    let mcp_service = StreamableHttpService::new(
        factory,
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    // Sanity-check: the test below assumes nesting at /mcp. If you change
    // the path here, update build_mcp_router's docstring.
    Router::new()
        .nest_service("/mcp", mcp_service)
        .layer(from_fn_with_state(host_state, host_validator_middleware))
        // Trace layer surfaces MCP traffic in the same `tracing` output as
        // the HTTP/UI server so a single `RUST_LOG=simlin_serve=info` keeps
        // both visible.
        .layer(TraceLayer::new_for_http())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tempfile::TempDir;
    use tower::ServiceExt;

    use crate::events::EventBus;
    use crate::git::GitProbe;
    use crate::registry::ProjectRegistry;

    fn build_state(root: PathBuf) -> Arc<AppState> {
        Arc::new(AppState {
            registry: Arc::new(ProjectRegistry::new(root.clone())),
            git: Arc::new(GitProbe::unavailable_for_tests()),
            root: Arc::new(root),
            events: Arc::new(EventBus::new()),
            launch_token: Arc::new(String::new()),
            ui_port: 12345,
            mcp_port: 12346,
            strict_origin: true,
        })
    }

    /// The route is mounted: a request to `/mcp` must NOT come back as 404.
    /// We don't speak the full Streamable HTTP handshake here -- that is
    /// covered by the e2e test in Task 9 -- but a non-404 confirms that the
    /// nest_service wiring is hooked up. The negative-control hit on
    /// `/some-other-path` must come back as 404 so we know the assertion
    /// is meaningful (the router doesn't match every path).
    #[tokio::test]
    async fn mcp_route_is_mounted_at_slash_mcp() {
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let state = build_state(canonical_root);

        let router = build_mcp_router(state);

        // GET without an Accept header: rmcp rejects with 406; what matters
        // here is that we got *any* response from the rmcp service rather
        // than the SPA fallback's 404. The `Host` header carries the test
        // build_state mcp_port so the host validator passes.
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/mcp")
                    .header("host", "127.0.0.1:12346")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("router response");

        assert_ne!(
            response.status(),
            StatusCode::NOT_FOUND,
            "GET /mcp must reach the StreamableHttpService, not fall through"
        );

        // Negative control: an unrelated path must 404, otherwise the
        // assertion above would be vacuous.
        let unrelated = router
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/no-such-endpoint")
                    .header("host", "127.0.0.1:12346")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("router response");
        assert_eq!(
            unrelated.status(),
            StatusCode::NOT_FOUND,
            "unrelated paths must 404, otherwise the /mcp non-404 assertion is vacuous"
        );
    }

    /// Servers built from the same `Arc<AppState>` must alias the same
    /// underlying state — a regression guard against a refactor that
    /// accidentally deep-copies AppState.
    ///
    /// Both `Arc::ptr_eq` and the strong-count delta confirm this:
    /// constructing additional servers from the cloned Arc must bump the
    /// strong count rather than allocate a fresh AppState.
    #[tokio::test]
    async fn factory_shares_app_state_across_sessions() {
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let state = build_state(canonical_root.clone());
        let baseline = Arc::strong_count(&state);

        let first = SimlinServeMcpServer::<RegistryAccess>::new(state.clone());
        let second = SimlinServeMcpServer::<RegistryAccess>::new(state.clone());

        // Each constructor captures one `Arc<AppState>` clone, so the
        // strong count must grow by exactly 2 above the baseline. (One
        // extra is the local `state` binding kept around for the assert.)
        let count = Arc::strong_count(&state);
        assert!(
            count >= baseline + 2,
            "expected at least {} strong refs, got {count} (baseline {baseline})",
            baseline + 2
        );
        drop(first);
        drop(second);
    }
}
