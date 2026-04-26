// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// `EventBus` wraps a `tokio::sync::broadcast::Sender` and exposes a small,
// project-shaped surface: `subscribe()` for WebSocket handlers and
// `publish()` for save handlers (and Phase 4's file watcher). The bus
// itself owns no per-subscriber state; the broadcast channel handles
// fan-out and lag detection.

//! Internal pub/sub bus for `WsMessage` events.
//!
//! Phase 3 uses this for `ProjectChanged` notifications surfaced to the
//! browser via the WebSocket endpoint. Future phases add more variants
//! (`ProjectFocused`, `SelectionChanged`, `DiagnosticsChanged`) and more
//! `ChangeSource` discriminators (`Agent` from MCP in Phase 6, `Disk`
//! from the file watcher in Phase 4).
//!
//! Capacity is fixed at 64 messages: plenty for a handful of WebSocket
//! clients receiving infrequent project-change events. Slow subscribers
//! see `RecvError::Lagged(n)` (logged + ignored by the WS handler) and
//! auto-resume from the oldest retained message.

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// Wire-shape for a single validation diagnostic carried inside
/// [`WsMessage::DiagnosticsChanged`]. Re-exported from
/// `simlin_mcp_core::ErrorOutput` so the WebSocket and MCP surfaces emit
/// byte-identical error structures, and so callers building a message
/// don't need to know which crate the type lives in.
pub use simlin_mcp_core::ErrorOutput as ValidationError;

/// Bounded fan-out capacity for the broadcast channel. A subscriber that
/// falls 64 messages behind starts losing the oldest events; the receiver
/// surfaces this as `RecvError::Lagged(n)` exactly once and then auto-
/// resumes from the next live message.
const BUS_CAPACITY: usize = 64;

/// Broadcast hub for `WsMessage` events.
///
/// The bus owns the sender side; subscribers are created on demand via
/// `subscribe()` (each WS connection gets its own receiver). Cloning the
/// `EventBus` is cheap because `broadcast::Sender` is internally `Arc`-shared.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<WsMessage>,
}

impl EventBus {
    /// Construct a fresh bus with the standard `BUS_CAPACITY`. The receiver
    /// returned from `broadcast::channel` is dropped immediately —
    /// subscribers create their own via `subscribe()`. Without a live
    /// subscriber a `publish` is a no-op (the broadcast channel returns
    /// `SendError`, which we intentionally swallow).
    pub fn new() -> Self {
        let (tx, _initial_rx) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Hand out a fresh receiver. The receiver only sees messages
    /// `publish`-ed *after* it was created — there's no replay of the
    /// pre-subscribe history.
    pub fn subscribe(&self) -> broadcast::Receiver<WsMessage> {
        self.tx.subscribe()
    }

    /// Fan `msg` out to every live subscriber. With no subscribers the
    /// underlying `Sender::send` returns `Err(SendError)`; we ignore that
    /// because "nobody is listening" is not a failure condition for the
    /// publisher.
    pub fn publish(&self, msg: WsMessage) {
        let _ = self.tx.send(msg);
    }

    /// Number of live receivers currently subscribed. Surface from
    /// `tokio::sync::broadcast::Sender::receiver_count` so tests can
    /// assert that per-session forwarders unsubscribe when their MCP
    /// session closes.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Wire envelope for every server-pushed event.
///
/// The `serde(tag)` discriminator names the variant on the wire so the
/// browser can route by `msg.type` without library help. Field names
/// land as camelCase (matching the rest of the API surface).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum WsMessage {
    /// A project's in-memory state advanced to a new version. The browser
    /// uses this to remount the editor with the latest state.
    ProjectChanged {
        /// Forward-slash relative path of the project (matches the
        /// listing wire shape so the client can join over a single
        /// canonical key).
        path: String,
        /// New optimistic-lock version. Monotonic per project.
        version: u64,
        /// Provenance of the change. Phase 3 only emits `User`; future
        /// phases add `Agent` (MCP) and `Disk` (file watcher).
        source: ChangeSource,
    },
    /// A project file was removed from disk (e.g. `rm` from a terminal).
    /// Phase 4's file watcher emits this so the SPA can drop the entry
    /// from its sidebar list. There is no version field because the
    /// project no longer exists; the client treats the path as the
    /// authoritative key for matching against its in-memory project list.
    ProjectRemoved {
        /// Forward-slash relative path the project used to live at.
        /// Same wire shape as `ProjectChanged.path` so the client can
        /// look up the entry by string equality.
        path: String,
    },
    /// The browser focused (or switched to) a project. Phase 7's MCP
    /// notifications router fans this out as `simlin/projectFocused` so
    /// AI clients can track which project the user is currently looking
    /// at.
    ProjectFocused {
        /// Forward-slash relative path of the focused project.
        path: String,
    },
    /// The browser's variable selection changed inside the active
    /// project. The list of canonical idents lets MCP clients show
    /// "what's selected" without needing to round-trip through the
    /// browser's view state.
    #[serde(rename_all = "camelCase")]
    SelectionChanged {
        /// Forward-slash relative path of the project whose selection
        /// changed (the browser may have multiple projects open in
        /// tabs).
        path: String,
        /// Canonical idents of the currently selected named view
        /// elements. Empty when nothing is selected.
        variable_idents: Vec<String>,
    },
    /// The set of validation errors for a project changed (an edit
    /// introduced new errors, fixed existing ones, or both). Computed
    /// after every successful save / file-watcher merge and only
    /// published when the `(code, variable_name)` set actually
    /// differs from the previous snapshot.
    DiagnosticsChanged {
        /// Forward-slash relative path of the project whose diagnostics
        /// changed.
        path: String,
        /// Full formatted error list for the project (not just the
        /// delta). An empty vector signals "all errors fixed".
        errors: Vec<ValidationError>,
    },
}

/// Inbound (browser → server) message envelope. A separate enum from
/// [`WsMessage`] because the inbound and outbound surfaces don't share
/// every variant: `DiagnosticsChanged` and `ProjectChanged` are server-
/// computed and intentionally have no inbound counterpart.
///
/// The wire shape mirrors `WsMessage` (same `type` discriminator, same
/// camelCase field names) so a single JSON `type` field disambiguates
/// the variant in both directions.
///
/// `Deserialize` enforces this asymmetry at parse time: a frame with
/// `"type":"diagnosticsChanged"` will fail to deserialize as
/// `ClientWsMessage`, and `handle_socket` logs and continues serving
/// rather than break the connection.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientWsMessage {
    /// The browser focused (or switched to) a project. Mirrors
    /// `WsMessage::ProjectFocused`.
    ProjectFocused {
        /// Forward-slash relative path of the focused project.
        path: String,
    },
    /// The browser's variable selection changed. Mirrors
    /// `WsMessage::SelectionChanged`.
    #[serde(rename_all = "camelCase")]
    SelectionChanged {
        /// Forward-slash relative path of the project whose selection
        /// changed.
        path: String,
        /// Canonical idents of the currently selected named view
        /// elements.
        variable_idents: Vec<String>,
    },
}

/// Provenance discriminator carried on every `ProjectChanged` event.
///
/// Lowercased on the wire (`"user" | "agent" | "disk"`). The frontend
/// uses this to decide whether to surface a "your collaborator/agent
/// changed this" indicator vs treating it as the user's own save echo.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeSource {
    /// Browser-driven save (Phase 3).
    User,
    /// MCP-driven edit (Phase 6).
    Agent,
    /// File-watcher-driven reload (Phase 4).
    Disk,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::RecvError;

    #[tokio::test]
    async fn two_subscribers_both_receive_the_same_message() {
        let bus = EventBus::new();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();

        let msg = WsMessage::ProjectChanged {
            path: "demo.stmx".into(),
            version: 1,
            source: ChangeSource::User,
        };
        bus.publish(msg.clone());

        let got_a = a.recv().await.expect("recv a");
        let got_b = b.recv().await.expect("recv b");
        assert_eq!(got_a, msg);
        assert_eq!(got_b, msg);
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_is_silently_dropped() {
        let bus = EventBus::new();
        bus.publish(WsMessage::ProjectChanged {
            path: "x".into(),
            version: 1,
            source: ChangeSource::User,
        });
    }

    #[tokio::test]
    async fn lagged_receiver_sees_lagged_error_then_resumes() {
        // Capacity is 64. We publish 100 messages without reading once;
        // the channel keeps the most recent 64 and the next recv() yields
        // RecvError::Lagged(36) (i.e. 100 - 64). The receiver then resumes
        // from the oldest retained message.
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        for i in 0..100u64 {
            bus.publish(WsMessage::ProjectChanged {
                path: format!("p{i}.stmx"),
                version: i,
                source: ChangeSource::User,
            });
        }

        let first = rx.recv().await.expect_err("expected Lagged");
        match first {
            RecvError::Lagged(n) => {
                assert_eq!(n, 36, "expected Lagged(36) since cap=64 and we sent 100");
            }
            other => panic!("expected Lagged, got {other:?}"),
        }

        // Auto-resume: next recv() returns the oldest retained message,
        // which is the 36th publish (path "p36.stmx", version 36).
        let next = rx.recv().await.expect("auto-resume yields a value");
        match next {
            WsMessage::ProjectChanged { version, .. } => {
                assert_eq!(version, 36, "auto-resume must yield the oldest retained");
            }
            _ => panic!("expected ProjectChanged"),
        }
    }

    #[test]
    fn project_changed_serializes_with_camel_case_and_tag() {
        let msg = WsMessage::ProjectChanged {
            path: "sub/foo.stmx".into(),
            version: 7,
            source: ChangeSource::User,
        };
        let value = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(value["type"].as_str(), Some("projectChanged"));
        assert_eq!(value["path"].as_str(), Some("sub/foo.stmx"));
        assert_eq!(value["version"].as_u64(), Some(7));
        assert_eq!(value["source"].as_str(), Some("user"));
    }

    #[test]
    fn project_removed_serializes_with_camel_case_and_tag() {
        let msg = WsMessage::ProjectRemoved {
            path: "sub/foo.stmx".into(),
        };
        let value = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(value["type"].as_str(), Some("projectRemoved"));
        assert_eq!(value["path"].as_str(), Some("sub/foo.stmx"));
    }

    #[test]
    fn change_source_variants_serialize_lowercase() {
        assert_eq!(
            serde_json::to_value(ChangeSource::User).unwrap(),
            serde_json::json!("user")
        );
        assert_eq!(
            serde_json::to_value(ChangeSource::Agent).unwrap(),
            serde_json::json!("agent")
        );
        assert_eq!(
            serde_json::to_value(ChangeSource::Disk).unwrap(),
            serde_json::json!("disk")
        );
    }

    #[tokio::test]
    async fn subscribe_after_publish_does_not_replay() {
        // Late subscribers see only messages sent after they subscribe.
        // Messages published before `subscribe()` are not buffered for them
        // — broadcast channels do not replay history.
        let bus = EventBus::new();
        bus.publish(WsMessage::ProjectChanged {
            path: "early".into(),
            version: 1,
            source: ChangeSource::User,
        });

        let mut rx = bus.subscribe();
        bus.publish(WsMessage::ProjectChanged {
            path: "late".into(),
            version: 2,
            source: ChangeSource::User,
        });

        let got = rx.recv().await.expect("recv");
        match got {
            WsMessage::ProjectChanged { path, .. } => {
                assert_eq!(path, "late");
            }
            _ => panic!("expected ProjectChanged"),
        }
    }

    #[test]
    fn project_focused_serializes_with_camel_case_and_tag() {
        let msg = WsMessage::ProjectFocused {
            path: "sub/foo.stmx".into(),
        };
        let value = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(value["type"].as_str(), Some("projectFocused"));
        assert_eq!(value["path"].as_str(), Some("sub/foo.stmx"));
    }

    #[test]
    fn selection_changed_serializes_with_camel_case_and_tag() {
        let msg = WsMessage::SelectionChanged {
            path: "sub/foo.stmx".into(),
            variable_idents: vec!["teacup_temperature".into(), "ambient_temperature".into()],
        };
        let value = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(value["type"].as_str(), Some("selectionChanged"));
        assert_eq!(value["path"].as_str(), Some("sub/foo.stmx"));
        let idents = value["variableIdents"].as_array().expect("array");
        assert_eq!(idents.len(), 2);
        assert_eq!(idents[0].as_str(), Some("teacup_temperature"));
        assert_eq!(idents[1].as_str(), Some("ambient_temperature"));
    }

    #[test]
    fn diagnostics_changed_serializes_with_camel_case_and_tag() {
        let msg = WsMessage::DiagnosticsChanged {
            path: "models/teacup.xmile".into(),
            errors: vec![ValidationError {
                code: "unknown_dependency".into(),
                message: "variable 'x' references unknown 'bogus'".into(),
                model_name: Some("main".into()),
                variable_name: Some("x".into()),
                kind: "variable".into(),
            }],
        };
        let value = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(value["type"].as_str(), Some("diagnosticsChanged"));
        assert_eq!(value["path"].as_str(), Some("models/teacup.xmile"));
        let errors = value["errors"].as_array().expect("errors array");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["code"].as_str(), Some("unknown_dependency"));
        assert_eq!(errors[0]["modelName"].as_str(), Some("main"));
        assert_eq!(errors[0]["variableName"].as_str(), Some("x"));
        assert_eq!(errors[0]["kind"].as_str(), Some("variable"));
    }

    #[test]
    fn diagnostics_changed_with_empty_errors_serializes_cleanly() {
        // Emitted when all previously known errors have been fixed.
        let msg = WsMessage::DiagnosticsChanged {
            path: "models/teacup.xmile".into(),
            errors: vec![],
        };
        let value = serde_json::to_value(&msg).expect("serialize");
        assert_eq!(value["type"].as_str(), Some("diagnosticsChanged"));
        assert_eq!(value["errors"].as_array().expect("errors array").len(), 0);
    }

    #[tokio::test]
    async fn project_focused_round_trips_through_eventbus() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let msg = WsMessage::ProjectFocused {
            path: "demo.stmx".into(),
        };
        bus.publish(msg.clone());

        let got = rx.recv().await.expect("recv");
        assert_eq!(got, msg);
    }

    #[tokio::test]
    async fn selection_changed_round_trips_through_eventbus() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let msg = WsMessage::SelectionChanged {
            path: "demo.stmx".into(),
            variable_idents: vec!["a".into(), "b".into()],
        };
        bus.publish(msg.clone());

        let got = rx.recv().await.expect("recv");
        assert_eq!(got, msg);
    }

    #[tokio::test]
    async fn diagnostics_changed_round_trips_through_eventbus() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        let msg = WsMessage::DiagnosticsChanged {
            path: "demo.stmx".into(),
            errors: vec![ValidationError {
                code: "syntax".into(),
                message: "bad equation".into(),
                model_name: None,
                variable_name: Some("y".into()),
                kind: "variable".into(),
            }],
        };
        bus.publish(msg.clone());

        let got = rx.recv().await.expect("recv");
        assert_eq!(got, msg);
    }

    #[test]
    fn client_ws_message_parses_project_focused() {
        let json = r#"{"type":"projectFocused","path":"models/teacup.xmile"}"#;
        let parsed: ClientWsMessage = serde_json::from_str(json).expect("parse");
        assert_eq!(
            parsed,
            ClientWsMessage::ProjectFocused {
                path: "models/teacup.xmile".into()
            }
        );
    }

    #[test]
    fn client_ws_message_parses_selection_changed() {
        let json = r#"{"type":"selectionChanged","path":"a.stmx","variableIdents":["x","y"]}"#;
        let parsed: ClientWsMessage = serde_json::from_str(json).expect("parse");
        assert_eq!(
            parsed,
            ClientWsMessage::SelectionChanged {
                path: "a.stmx".into(),
                variable_idents: vec!["x".into(), "y".into()]
            }
        );
    }

    #[test]
    fn client_ws_message_rejects_diagnostics_changed_as_inbound() {
        let json = r#"{"type":"diagnosticsChanged","path":"a.stmx","errors":[]}"#;
        // diagnosticsChanged is server-only; clients aren't allowed to push it.
        let result: Result<ClientWsMessage, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "diagnosticsChanged must not be accepted as an inbound frame"
        );
    }

    #[test]
    fn client_ws_message_rejects_malformed_json() {
        let result: Result<ClientWsMessage, _> = serde_json::from_str("not json");
        assert!(result.is_err());
    }

    #[test]
    fn client_ws_message_rejects_missing_required_field() {
        // SelectionChanged requires variableIdents.
        let json = r#"{"type":"selectionChanged","path":"a.stmx"}"#;
        let result: Result<ClientWsMessage, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "missing variableIdents field must produce a parse error"
        );
    }
}
