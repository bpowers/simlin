// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Per-session forwarder that bridges the in-process [`EventBus`] to MCP
//! notifications.
//!
//! `forward_events_to_peer` is spawned once per MCP session inside the
//! `initialize` request handler. It owns one broadcast receiver and one
//! `Peer<RoleServer>` clone; when the session ends the peer's transport
//! closes and `send_notification` returns `ServiceError::TransportClosed`,
//! which is the loop's natural exit. No global registry of peers is
//! needed: the per-session pattern scales with sessions automatically.
//!
//! ### Ordering caveat (rmcp 1.5.x)
//!
//! Notifications and tool responses travel on parallel paths inside rmcp's
//! service layer (the `sink_proxy_tx` for responses, `Peer.tx` for
//! notifications). On the Streamable HTTP transport the client may
//! observe a notification *before* the tool response that triggered it.
//! Notifications are intentionally designed as idempotent re-fetch hints
//! — the client should treat each one as "the relevant project may have
//! moved on, re-read if you care about latest state" rather than as
//! authoritative delivery of new state. See `docs/design-plans/
//! 2026-04-05-server-rewrite.md` Phase 7 for the full rationale.

use rmcp::RoleServer;
use rmcp::model::{CustomNotification, ServerNotification};
use rmcp::service::{Peer, ServiceError};
use serde_json::json;
use tokio::sync::broadcast;

use crate::events::WsMessage;

/// Drain the broadcast receiver and forward every event to `peer` as a
/// `simlin/*` custom notification.
///
/// Exit conditions:
/// - `Err(broadcast::error::RecvError::Closed)` — the EventBus was
///   dropped (process shutdown).
/// - `Err(ServiceError::TransportClosed)` — the MCP session ended.
/// - A non-`TransportClosed` peer error when `peer.is_transport_closed()`
///   is true — guards against variants such as `TransportSend` that can
///   surface on a broken transport while the inner mpsc channel is still
///   open, which would otherwise cause the loop to spin burning CPU.
///
/// Other non-fatal branches log and continue: a `Lagged(n)` receiver
/// auto-resumes from the oldest retained message, and non-`TransportClosed`
/// peer errors where the transport is still alive are advisory (the next
/// notification may still go through).
///
/// Note: idle disconnects (sessions that go quiet before TransportClosed
/// surfaces) are detected only on the next recv() wake-up. Idle sessions
/// hold their broadcast subscription until the bus publishes again or the
/// process shuts down. Phase 8 may add a periodic poll if this proves
/// noticeable in practice.
pub async fn forward_events_to_peer(
    peer: Peer<RoleServer>,
    mut rx: broadcast::Receiver<WsMessage>,
) {
    tracing::info!("mcp notification forwarder: started");
    loop {
        match rx.recv().await {
            Ok(msg) => {
                let (method, params) = wire_pair(msg);
                let notif = CustomNotification::new(method, Some(params));
                match peer
                    .send_notification(ServerNotification::CustomNotification(notif))
                    .await
                {
                    Ok(()) => {}
                    Err(ServiceError::TransportClosed) => {
                        tracing::info!("mcp notification forwarder: exiting (transport closed)");
                        break;
                    }
                    Err(err) => {
                        if peer.is_transport_closed() {
                            tracing::info!(
                                ?err,
                                "mcp notification forwarder: exiting (transport closed after send error)"
                            );
                            break;
                        }
                        tracing::warn!(
                            error = %err,
                            "mcp notification forwarder: send failed (continuing)"
                        );
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                // Slow consumer: the broadcast channel auto-resumes from
                // the oldest retained message on the next recv(). Log so
                // operators can spot a backlogged session.
                tracing::warn!(lagged = n, "mcp notification forwarder lagged");
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::info!("mcp notification forwarder: exiting (event bus closed)");
                break;
            }
        }
    }
}

/// Map an in-process [`WsMessage`] to its outbound MCP method + params
/// pair. The method strings carry the `simlin/` namespace prefix to
/// avoid collision with future MCP-standard notification methods.
///
/// The params object intentionally mirrors the camelCase wire shape
/// that the WebSocket frontend already sees, minus the `type`
/// discriminator (the method name supersedes that role on the MCP
/// side). Keeping the field set identical means an MCP client that
/// already understands the WS payload doesn't need a second parser.
pub fn wire_pair(msg: WsMessage) -> (&'static str, serde_json::Value) {
    match msg {
        WsMessage::ProjectChanged {
            path,
            version,
            source,
        } => (
            "simlin/projectChanged",
            json!({
                "path": path,
                "version": version,
                "source": source,
            }),
        ),
        WsMessage::ProjectRemoved { path } => (
            "simlin/projectRemoved",
            json!({
                "path": path,
            }),
        ),
        WsMessage::ProjectRenamed { from, to } => (
            "simlin/projectRenamed",
            json!({
                "from": from,
                "to": to,
            }),
        ),
        WsMessage::ProjectFocused { path } => (
            "simlin/projectFocused",
            json!({
                "path": path,
            }),
        ),
        WsMessage::SelectionChanged {
            path,
            variable_idents,
        } => (
            "simlin/selectionChanged",
            json!({
                "path": path,
                "variableIdents": variable_idents,
            }),
        ),
        WsMessage::DiagnosticsChanged { path, errors } => (
            "simlin/diagnosticsChanged",
            json!({
                "path": path,
                "errors": errors,
            }),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::events::{ChangeSource, ValidationError};

    #[test]
    fn wire_pair_project_changed() {
        let (method, params) = wire_pair(WsMessage::ProjectChanged {
            path: "models/teacup.xmile".into(),
            version: 7,
            source: ChangeSource::Agent,
        });
        assert_eq!(method, "simlin/projectChanged");
        assert_eq!(params["path"].as_str(), Some("models/teacup.xmile"));
        assert_eq!(params["version"].as_u64(), Some(7));
        assert_eq!(params["source"].as_str(), Some("agent"));
    }

    #[test]
    fn wire_pair_project_removed() {
        let (method, params) = wire_pair(WsMessage::ProjectRemoved {
            path: "old/foo.stmx".into(),
        });
        assert_eq!(method, "simlin/projectRemoved");
        assert_eq!(params["path"].as_str(), Some("old/foo.stmx"));
    }

    #[test]
    fn wire_pair_project_renamed() {
        let (method, params) = wire_pair(WsMessage::ProjectRenamed {
            from: "old.stmx".into(),
            to: "new.stmx".into(),
        });
        assert_eq!(method, "simlin/projectRenamed");
        assert_eq!(params["from"].as_str(), Some("old.stmx"));
        assert_eq!(params["to"].as_str(), Some("new.stmx"));
    }

    #[test]
    fn wire_pair_project_focused() {
        let (method, params) = wire_pair(WsMessage::ProjectFocused {
            path: "a.stmx".into(),
        });
        assert_eq!(method, "simlin/projectFocused");
        assert_eq!(params["path"].as_str(), Some("a.stmx"));
    }

    #[test]
    fn wire_pair_selection_changed_uses_camel_case() {
        let (method, params) = wire_pair(WsMessage::SelectionChanged {
            path: "x.stmx".into(),
            variable_idents: vec!["foo".into(), "bar".into()],
        });
        assert_eq!(method, "simlin/selectionChanged");
        assert_eq!(params["path"].as_str(), Some("x.stmx"));
        let idents = params["variableIdents"]
            .as_array()
            .expect("variableIdents is an array");
        assert_eq!(idents.len(), 2);
        assert_eq!(idents[0].as_str(), Some("foo"));
        assert_eq!(idents[1].as_str(), Some("bar"));
        // Snake-case alias must not leak through; the field is camelCase
        // because that's the wire convention shared with the WS surface.
        assert!(params.get("variable_idents").is_none());
    }

    #[test]
    fn wire_pair_diagnostics_changed() {
        let err = ValidationError {
            code: "syntax".into(),
            message: "bad".into(),
            model_name: Some("main".into()),
            variable_name: Some("y".into()),
            kind: "variable".into(),
        };
        let (method, params) = wire_pair(WsMessage::DiagnosticsChanged {
            path: "models/teacup.xmile".into(),
            errors: vec![err],
        });
        assert_eq!(method, "simlin/diagnosticsChanged");
        assert_eq!(params["path"].as_str(), Some("models/teacup.xmile"));
        let errors = params["errors"].as_array().expect("errors is an array");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["code"].as_str(), Some("syntax"));
        assert_eq!(errors[0]["modelName"].as_str(), Some("main"));
    }

    #[test]
    fn wire_pair_diagnostics_changed_empty_errors() {
        let (method, params) = wire_pair(WsMessage::DiagnosticsChanged {
            path: "x.stmx".into(),
            errors: vec![],
        });
        assert_eq!(method, "simlin/diagnosticsChanged");
        assert_eq!(params["errors"].as_array().expect("array").len(), 0);
    }
}
