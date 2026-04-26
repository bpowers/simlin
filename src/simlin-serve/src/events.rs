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

use serde::Serialize;
use tokio::sync::broadcast;

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
        }
    }
}
