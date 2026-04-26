// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Helpers for binding the dual UI/MCP listeners with friendly diagnostics.
//!
//! `bind_or_die` wraps `TcpListener::bind` so a port conflict surfaces as
//! a single-line, actionable error mentioning the affected port and (for
//! the MCP listener) the override flag. The HTTP/UI listener defaults to
//! an OS-chosen ephemeral port so port conflicts are usually MCP-only;
//! when they happen there, the operator needs to know they can pass
//! `--mcp-port` to retry.

use std::io::ErrorKind;

use tokio::net::{TcpListener, ToSocketAddrs};

/// Bind a TCP listener with a labelled error when the port is in use.
///
/// `label` is a human-readable noun phrase like "HTTP/UI server" used in
/// the resulting error. `port_hint` is an optional CLI flag (e.g.
/// `--mcp-port`) that the user could pass to retry; when set, it appears
/// in the error message so the operator's next step is obvious.
///
/// Errors other than `AddrInUse` are wrapped with the same `label` prefix
/// so the caller can distinguish UI-vs-MCP failures even when the cause
/// is something else (permission denied on a privileged port, EAFNOSUPPORT
/// on systems without IPv4, etc.).
pub async fn bind_or_die<A: ToSocketAddrs>(
    addr: A,
    label: &str,
    port_hint: Option<&str>,
) -> anyhow::Result<TcpListener> {
    TcpListener::bind(addr).await.map_err(|e| {
        if e.kind() == ErrorKind::AddrInUse {
            let hint = port_hint
                .map(|h| format!(" Pass {h} to use a different port."))
                .unwrap_or_default();
            anyhow::anyhow!("Cannot start {label}: address already in use.{hint}")
        } else {
            anyhow::anyhow!("Cannot start {label}: {e}")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bind_or_die` succeeds on an ephemeral port and returns a listener
    /// whose address is reachable.
    #[tokio::test]
    async fn binds_an_ephemeral_port() {
        let listener = bind_or_die(("127.0.0.1", 0_u16), "test server", None)
            .await
            .expect("ephemeral bind should succeed");
        let addr = listener.local_addr().expect("local_addr");
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert!(addr.port() > 0, "ephemeral port must be non-zero");
    }

    /// When the requested port is occupied, the error message must mention
    /// the label and "address already in use" so the operator can identify
    /// which listener failed.
    #[tokio::test]
    async fn addr_in_use_error_mentions_label_and_diagnostic() {
        // Bind an ephemeral port first to occupy it.
        let occupy = TcpListener::bind(("127.0.0.1", 0_u16))
            .await
            .expect("occupy ephemeral");
        let port = occupy.local_addr().expect("local_addr").port();

        // Try to bind the same port — must fail with our friendly error.
        let result = bind_or_die(("127.0.0.1", port), "MCP server", Some("--mcp-port")).await;

        let err = result.expect_err("rebinding the same port must fail");
        let msg = format!("{err}");
        assert!(
            msg.contains("MCP server"),
            "error must include the label: {msg}"
        );
        assert!(
            msg.contains("address already in use"),
            "error must say 'address already in use': {msg}"
        );
        assert!(
            msg.contains("--mcp-port"),
            "error must include the port-override hint: {msg}"
        );
    }

    /// Without a `port_hint`, the error must NOT mention any flag — the
    /// HTTP/UI listener is bound on an ephemeral port by default, so a
    /// conflict there typically means the operator passed `--port`
    /// explicitly and we have no specific override to suggest.
    #[tokio::test]
    async fn addr_in_use_error_omits_hint_when_none() {
        let occupy = TcpListener::bind(("127.0.0.1", 0_u16))
            .await
            .expect("occupy ephemeral");
        let port = occupy.local_addr().expect("local_addr").port();

        let result = bind_or_die(("127.0.0.1", port), "HTTP/UI server", None).await;
        let err = result.expect_err("rebinding the same port must fail");
        let msg = format!("{err}");
        assert!(
            !msg.contains("Pass "),
            "error should not contain a 'Pass <flag>' hint when port_hint is None: {msg}"
        );
        assert!(
            msg.contains("HTTP/UI server"),
            "error must include the label even without a hint: {msg}"
        );
    }
}
