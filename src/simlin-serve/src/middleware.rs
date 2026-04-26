// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! HTTP middleware for `simlin-serve`.
//!
//! [`host_validator_middleware`] is defense-in-depth against DNS
//! rebinding (cf. CVE-2025-66414, the December 2025 incident in the
//! official MCP TypeScript SDK). The browser refuses to honor
//! `Set-Cookie` and same-origin checks against an attacker-controlled
//! DNS name only because of the `Host:` header on each request — a
//! malicious site that resolves a controlled name to `127.0.0.1` and
//! lets the browser load its own page can otherwise reach our server
//! over the local listener. By rejecting any `Host:` value that isn't
//! one of the four loopback entries we expect, we close that gap
//! before any handler runs.
//!
//! The validator does NOT replace the bearer-token gate on `/api/*`
//! or the WebSocket upgrade — the layers compose: the token gate
//! defeats CSRF-style attacks, the host check defeats rebinding-style
//! attacks. Either layer alone is incomplete.

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::handlers::AppState;

/// Reject requests whose `Host:` header is not one of the per-launch
/// allowlist values. The allowlist is computed at request time from
/// the `AppState` ports rather than precomputed at router-build time
/// so a future change that lets ports rotate at runtime continues to
/// work without coordinating router rebuilds. (Today's `main()`
/// populates the ports once, after the listeners bind.)
///
/// Returns `421 Misdirected Request` (RFC 7540 §9.1.2) on rejection
/// because that status precisely communicates "the server cannot
/// produce a response for this combination of authority and request"
/// — which is exactly what's happening when an attacker-controlled
/// DNS name lands at our loopback bind.
pub async fn host_validator_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());

    let host = match host {
        Some(h) => h,
        None => {
            // HTTP/1.1 mandates a Host header; HTTP/1.0 may omit it.
            // Either way, the browser-driven SPA always sends one, so a
            // request without one is hostile or malformed.
            tracing::warn!("rejecting request without Host header");
            return host_rejected();
        }
    };

    if !is_allowed_host(host, state.ui_port, state.mcp_port) {
        tracing::warn!(host = %host, "rejecting request with unallowed Host header");
        return host_rejected();
    }

    next.run(request).await
}

fn host_rejected() -> Response {
    (StatusCode::MISDIRECTED_REQUEST, "Host header not allowed").into_response()
}

/// Match `host` against `127.0.0.1:<port>` or `localhost:<port>` for
/// either of the two bound ports. The check is byte-literal: a
/// well-known IPv6 loopback (`[::1]:<port>`) is intentionally not on
/// the list because both server bind addresses are IPv4 — accepting
/// IPv6 would suggest a code path that doesn't exist.
fn is_allowed_host(host: &str, ui_port: u16, mcp_port: u16) -> bool {
    [
        format!("127.0.0.1:{ui_port}"),
        format!("localhost:{ui_port}"),
        format!("127.0.0.1:{mcp_port}"),
        format!("localhost:{mcp_port}"),
    ]
    .iter()
    .any(|allowed| allowed == host)
}

/// Match `origin` against `http://127.0.0.1:<ui_port>` or
/// `http://localhost:<ui_port>` (only the UI port — the MCP server
/// doesn't host the SPA so a WS Origin claiming the MCP port is
/// suspicious). HTTPS variants are out of scope: `simlin-serve` binds
/// loopback only and never speaks TLS, so an `https://` origin would
/// be a forgery rather than a legitimate proxy.
pub fn is_allowed_origin(origin: &str, ui_port: u16) -> bool {
    [
        format!("http://127.0.0.1:{ui_port}"),
        format!("http://localhost:{ui_port}"),
    ]
    .iter()
    .any(|allowed| allowed == origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_v4_is_allowed_at_ui_port() {
        assert!(is_allowed_host("127.0.0.1:54321", 54321, 7878));
    }

    #[test]
    fn loopback_v4_is_allowed_at_mcp_port() {
        assert!(is_allowed_host("127.0.0.1:7878", 54321, 7878));
    }

    #[test]
    fn localhost_at_ui_port_is_allowed() {
        assert!(is_allowed_host("localhost:54321", 54321, 7878));
    }

    #[test]
    fn localhost_at_mcp_port_is_allowed() {
        assert!(is_allowed_host("localhost:7878", 54321, 7878));
    }

    #[test]
    fn attacker_dns_name_at_loopback_port_is_rejected() {
        assert!(!is_allowed_host("evil.example.com:54321", 54321, 7878));
    }

    #[test]
    fn loopback_v4_at_unrelated_port_is_rejected() {
        // The attacker rebinds DNS to the loopback IP but claims a
        // different port — still rejected because the port is part of
        // the allowlist key.
        assert!(!is_allowed_host("127.0.0.1:1234", 54321, 7878));
    }

    #[test]
    fn ipv6_loopback_is_rejected_by_design() {
        // The server only binds 127.0.0.1; an IPv6-claiming Host can
        // only come from a misconfigured proxy or a forged request.
        assert!(!is_allowed_host("[::1]:54321", 54321, 7878));
    }

    #[test]
    fn host_with_no_port_is_rejected() {
        // The bound listener always carries a port, so a Host without
        // one cannot match. Defensive: this also catches HTTP/1.0
        // clients that emit just the host name.
        assert!(!is_allowed_host("127.0.0.1", 54321, 7878));
        assert!(!is_allowed_host("localhost", 54321, 7878));
    }

    #[test]
    fn allowed_ui_origin_passes() {
        assert!(is_allowed_origin("http://127.0.0.1:54321", 54321));
        assert!(is_allowed_origin("http://localhost:54321", 54321));
    }

    #[test]
    fn https_origin_at_loopback_is_rejected() {
        // No TLS code path exists; treat https://127.0.0.1:<port> as a
        // forgery rather than a legitimate proxy.
        assert!(!is_allowed_origin("https://127.0.0.1:54321", 54321));
    }

    #[test]
    fn cross_origin_attacker_is_rejected() {
        assert!(!is_allowed_origin("http://evil.example.com", 54321));
    }
}
