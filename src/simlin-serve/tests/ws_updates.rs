// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for the `/api/updates` WebSocket endpoint.
//!
//! Pattern: bind a real `TcpListener` on `127.0.0.1:0`, spawn the router
//! via `axum::serve`, then connect with `tokio_tungstenite`. We exercise
//! the auth gate (correct/wrong/missing tokens), the happy path
//! (subscribe -> receive a published `ProjectChanged`), the lagged-client
//! recovery path, and a graceful close.

#![deny(unsafe_code)]

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use simlin_serve::build_router;
use simlin_serve::events::{ChangeSource, EventBus, WsMessage};
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::test_support::unavailable_git_probe;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;

/// Test harness: bind a port, spawn the server, return (state, address, dir).
/// The caller must keep `TempDir` alive; dropping it removes the root and the
/// server starts returning 404 for project lookups.
///
/// Bind comes BEFORE state construction so the actual ephemeral port
/// can be passed into AppState.ui_port — the host-validator middleware
/// uses that to compute the per-launch allowlist, and tokio_tungstenite
/// always sets `Host: 127.0.0.1:<port>` based on the URL.
async fn spawn_server(token: &str) -> (AppState, String, TempDir) {
    spawn_server_with_strict_origin(token, false).await
}

async fn spawn_server_with_strict_origin(
    token: &str,
    strict_origin: bool,
) -> (AppState, String, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let canonical = dir.path().canonicalize().expect("canonicalize");

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    let state = AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(unavailable_git_probe()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(token.to_string()),
        ui_port: addr.port(),
        mcp_port: 0,
        strict_origin,
    };

    let router = build_router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    (state, format!("127.0.0.1:{}", addr.port()), dir)
}

#[tokio::test]
async fn happy_path_receives_published_project_changed() {
    let (state, addr, _dir) = spawn_server("secret-token").await;
    let url = format!("ws://{}/api/updates?token=secret-token", addr);

    let (mut ws, response) = connect_async(&url).await.expect("connect");
    assert_eq!(
        response.status().as_u16(),
        101,
        "websocket upgrade should respond with 101 Switching Protocols"
    );

    // Publish on the bus AFTER the WS subscribed; broadcast does not
    // replay history so we must wait for connection success first.
    state.events.publish(WsMessage::ProjectChanged {
        path: "demo.stmx".into(),
        version: 7,
        source: ChangeSource::User,
    });

    let received = ws.next().await.expect("recv").expect("ws message");
    let text = match received {
        Message::Text(t) => t,
        other => panic!("expected text, got {other:?}"),
    };
    let value: serde_json::Value = serde_json::from_str(&text).expect("parse json");
    assert_eq!(value["type"].as_str(), Some("projectChanged"));
    assert_eq!(value["path"].as_str(), Some("demo.stmx"));
    assert_eq!(value["version"].as_u64(), Some(7));
    assert_eq!(value["source"].as_str(), Some("user"));
}

#[tokio::test]
async fn wrong_token_is_rejected_with_401() {
    let (_state, addr, _dir) = spawn_server("real-token").await;
    let url = format!("ws://{}/api/updates?token=wrong", addr);

    let result = connect_async(&url).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status().as_u16(),
                401,
                "wrong token must produce 401 Unauthorized"
            );
        }
        Err(other) => panic!("expected HTTP error, got {other:?}"),
        Ok(_) => panic!("connection should have been rejected"),
    }
}

#[tokio::test]
async fn missing_token_is_rejected_with_400() {
    // axum's Query<TokenParams> extractor rejects missing required fields
    // before we get to the handler body, which surfaces as 400 Bad Request.
    let (_state, addr, _dir) = spawn_server("any-token").await;
    let url = format!("ws://{}/api/updates", addr);

    let result = connect_async(&url).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status().as_u16(),
                400,
                "missing token must produce 400 Bad Request"
            );
        }
        Err(other) => panic!("expected HTTP error, got {other:?}"),
        Ok(_) => panic!("connection should have been rejected"),
    }
}

#[tokio::test]
async fn lagged_client_does_not_panic_and_keeps_receiving() {
    // Connect, do not read for a while. Publish 100 messages. The server
    // sends them serially as fast as the broadcast buffer drains; if the
    // client falls behind, the receiver yields RecvError::Lagged(n) which
    // the WS handler logs + ignores, then auto-resumes. We then read
    // until we get something non-lagged. The test passes if the read
    // loop terminates with a successful frame and no panic.
    let (state, addr, _dir) = spawn_server("k").await;
    let url = format!("ws://{}/api/updates?token=k", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect");

    for i in 0..100u64 {
        state.events.publish(WsMessage::ProjectChanged {
            path: format!("p{i}.stmx"),
            version: i,
            source: ChangeSource::User,
        });
    }

    // Read all available messages with a small timeout. We expect to
    // receive at least the cap-many tail of the publish burst (older
    // messages are dropped by the broadcast channel) and no panic.
    let mut received = 0;
    while let Ok(Some(msg)) =
        tokio::time::timeout(std::time::Duration::from_millis(500), ws.next()).await
    {
        match msg {
            Ok(Message::Text(t)) => {
                let _: serde_json::Value = serde_json::from_str(&t).expect("valid json");
                received += 1;
            }
            Ok(Message::Close(_)) | Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => continue,
            Ok(Message::Binary(_)) | Ok(Message::Frame(_)) => continue,
            Err(e) => panic!("ws error: {e:?}"),
        }
    }
    assert!(
        received > 0,
        "the lagged client must still receive at least one message"
    );
}

#[tokio::test]
async fn client_close_terminates_server_task_cleanly() {
    // Open a connection, then have the client send Close. The server
    // task should exit without error. We verify by closing the bus
    // (drop the only Sender) and observing the next recv() yields None
    // — but a simpler check is to send Close and then see the server
    // close its side; tokio_tungstenite reports `None` from the Stream.
    let (_state, addr, _dir) = spawn_server("token").await;
    let url = format!("ws://{}/api/updates?token=token", addr);

    let (mut ws, _) = connect_async(&url).await.expect("connect");
    ws.send(Message::Close(None)).await.expect("send close");

    // After we send Close, the server should also close. The next recv()
    // should yield either a Close frame or end of stream.
    let next = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .expect("server should respond to client close within timeout");
    match next {
        Some(Ok(Message::Close(_))) => {} // server echoed close
        None => {}                        // server closed connection
        Some(Err(_)) => {}                // connection errored after close
        Some(Ok(other)) => panic!("expected Close or end-of-stream, got {other:?}"),
    }
}

/// Helper that drains a broadcast receiver until it sees a non-Lagged
/// frame or the deadline expires. Tests use this to read events
/// published as a side-effect of the WebSocket handler — we don't care
/// if a few tracing-related messages slipped in first, only that the
/// expected variant arrives within the timeout.
async fn await_published(
    rx: &mut tokio::sync::broadcast::Receiver<WsMessage>,
    timeout: std::time::Duration,
) -> Option<WsMessage> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return None;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(msg)) => return Some(msg),
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => return None,
            Err(_) => return None,
        }
    }
}

#[tokio::test]
async fn inbound_project_focused_is_published_on_eventbus() {
    let (state, addr, _dir) = spawn_server("k").await;
    let url = format!("ws://{}/api/updates?token=k", addr);
    let mut bus_rx = state.events.subscribe();

    let (mut ws, _) = connect_async(&url).await.expect("connect");
    let frame = r#"{"type":"projectFocused","path":"models/teacup.xmile"}"#;
    ws.send(Message::Text(frame.into()))
        .await
        .expect("send text");

    let msg = await_published(&mut bus_rx, std::time::Duration::from_secs(2))
        .await
        .expect("server must publish ProjectFocused within 2s");
    match msg {
        WsMessage::ProjectFocused { path } => {
            assert_eq!(path, "models/teacup.xmile");
        }
        other => panic!("expected ProjectFocused, got {other:?}"),
    }
}

#[tokio::test]
async fn inbound_selection_changed_is_published_on_eventbus() {
    let (state, addr, _dir) = spawn_server("k").await;
    let url = format!("ws://{}/api/updates?token=k", addr);
    let mut bus_rx = state.events.subscribe();

    let (mut ws, _) = connect_async(&url).await.expect("connect");
    let frame = r#"{"type":"selectionChanged","path":"a.stmx","variableIdents":["x","y"]}"#;
    ws.send(Message::Text(frame.into()))
        .await
        .expect("send text");

    let msg = await_published(&mut bus_rx, std::time::Duration::from_secs(2))
        .await
        .expect("server must publish SelectionChanged within 2s");
    match msg {
        WsMessage::SelectionChanged {
            path,
            variable_idents,
        } => {
            assert_eq!(path, "a.stmx");
            assert_eq!(variable_idents, vec!["x".to_string(), "y".to_string()]);
        }
        other => panic!("expected SelectionChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_inbound_frame_does_not_close_connection() {
    // The server must log and continue serving — a buggy client should not
    // be able to terminate the WebSocket session by sending garbage. After
    // the malformed frame we send a valid one and verify the bus saw it.
    let (state, addr, _dir) = spawn_server("k").await;
    let url = format!("ws://{}/api/updates?token=k", addr);
    let mut bus_rx = state.events.subscribe();

    let (mut ws, _) = connect_async(&url).await.expect("connect");
    ws.send(Message::Text("not valid json".into()))
        .await
        .expect("send garbage");

    // Connection still alive: send a valid frame and observe it.
    let frame = r#"{"type":"projectFocused","path":"survived.stmx"}"#;
    ws.send(Message::Text(frame.into()))
        .await
        .expect("send text after garbage");

    let msg = await_published(&mut bus_rx, std::time::Duration::from_secs(2))
        .await
        .expect("server must publish after recovering from garbage");
    match msg {
        WsMessage::ProjectFocused { path } => assert_eq!(path, "survived.stmx"),
        other => panic!("expected ProjectFocused, got {other:?}"),
    }
}

#[tokio::test]
async fn strict_origin_rejects_upgrade_with_no_origin_header() {
    // The default production posture is `strict_origin=true`. The SPA
    // always sets Origin; a request with no Origin is hostile or
    // malformed, so the upgrade must be refused with 403.
    let (_state, addr, _dir) = spawn_server_with_strict_origin("k", true).await;
    let url = format!("ws://{}/api/updates?token=k", addr);

    let result = connect_async(&url).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status().as_u16(),
                403,
                "missing Origin under strict-origin must produce 403 Forbidden"
            );
        }
        Err(other) => panic!("expected HTTP error, got {other:?}"),
        Ok(_) => panic!("strict-origin should refuse this upgrade"),
    }
}

#[tokio::test]
async fn allowed_origin_passes_under_strict_origin() {
    let (state, addr, _dir) = spawn_server_with_strict_origin("k", true).await;
    let url = format!("ws://{}/api/updates?token=k", addr);
    let port = state.ui_port;

    // Build a request with an Origin matching the loopback allowlist.
    let mut request = url.clone().into_client_request().expect("build request");
    request.headers_mut().insert(
        header::ORIGIN,
        format!("http://127.0.0.1:{port}").parse().expect("origin"),
    );

    let (mut ws, response) = connect_async(request).await.expect("connect");
    assert_eq!(
        response.status().as_u16(),
        101,
        "loopback Origin must be accepted under strict-origin"
    );
    let _ = ws.close(None).await;
}

#[tokio::test]
async fn cross_origin_attacker_is_rejected_under_strict_origin() {
    let (_state, addr, _dir) = spawn_server_with_strict_origin("k", true).await;
    let url = format!("ws://{}/api/updates?token=k", addr);

    let mut request = url.into_client_request().expect("build request");
    request.headers_mut().insert(
        header::ORIGIN,
        "http://evil.example.com".parse().expect("origin"),
    );

    let result = connect_async(request).await;
    match result {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(
                resp.status().as_u16(),
                403,
                "cross-origin must produce 403 Forbidden"
            );
        }
        Err(other) => panic!("expected HTTP error, got {other:?}"),
        Ok(_) => panic!("cross-origin upgrade should be rejected"),
    }
}

/// With `strict_origin=false` (the developer convenience mode), an upgrade
/// that carries no `Origin` header must succeed. Browser-native WebSocket
/// always sets Origin; this path is exercised by non-browser clients such as
/// `wscat` or raw `curl --no-buffer` where setting Origin is inconvenient.
#[tokio::test]
async fn non_strict_origin_accepts_no_origin_header() {
    let (_state, addr, _dir) = spawn_server_with_strict_origin("k", false).await;
    let url = format!("ws://{}/api/updates?token=k", addr);

    // tokio_tungstenite does not set an Origin header by default, which
    // exercises the `None` origin arm in the handler with strict_origin=false.
    let result = connect_async(&url).await;
    match result {
        Ok((mut ws, response)) => {
            assert_eq!(
                response.status().as_u16(),
                101,
                "non-strict-origin must accept upgrades with no Origin header"
            );
            let _ = ws.close(None).await;
        }
        Err(e) => panic!("expected successful upgrade with strict_origin=false, got: {e:?}"),
    }
}

#[tokio::test]
async fn unknown_inbound_variant_is_logged_and_ignored() {
    // diagnosticsChanged is server-only (no inbound counterpart). A client
    // attempting to push it should be ignored at the parse layer; the
    // connection stays open and a follow-up valid frame is processed.
    let (state, addr, _dir) = spawn_server("k").await;
    let url = format!("ws://{}/api/updates?token=k", addr);
    let mut bus_rx = state.events.subscribe();

    let (mut ws, _) = connect_async(&url).await.expect("connect");
    let bogus = r#"{"type":"diagnosticsChanged","path":"a.stmx","errors":[]}"#;
    ws.send(Message::Text(bogus.into()))
        .await
        .expect("send bogus");

    // Bus must NOT see DiagnosticsChanged from this client. We probe by
    // sending a follow-up valid frame and checking the next bus event is
    // the valid one (not a slipped-in DiagnosticsChanged).
    let frame = r#"{"type":"projectFocused","path":"after_bogus.stmx"}"#;
    ws.send(Message::Text(frame.into()))
        .await
        .expect("send follow-up");

    let msg = await_published(&mut bus_rx, std::time::Duration::from_secs(2))
        .await
        .expect("follow-up frame must be published");
    match msg {
        WsMessage::ProjectFocused { path } => assert_eq!(path, "after_bogus.stmx"),
        WsMessage::DiagnosticsChanged { .. } => {
            panic!("client must not be able to inject DiagnosticsChanged")
        }
        other => panic!("expected ProjectFocused, got {other:?}"),
    }
}
