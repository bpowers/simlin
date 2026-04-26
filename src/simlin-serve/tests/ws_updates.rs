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
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::ProjectRegistry;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

/// Test harness: bind a port, spawn the server, return (state, address).
async fn spawn_server(token: &str) -> (AppState, String) {
    let dir = TempDir::new().expect("tempdir");
    let canonical = dir.path().canonicalize().expect("canonicalize");
    // Leak the tempdir so the registry's root stays valid for the lifetime
    // of the spawned server task. Tests are short-lived and don't read
    // any files from the root, so this is acceptable.
    std::mem::forget(dir);

    let state = AppState {
        registry: Arc::new(ProjectRegistry::new(canonical.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(canonical),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(token.to_string()),
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let router = build_router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    (state, format!("127.0.0.1:{}", addr.port()))
}

#[tokio::test]
async fn happy_path_receives_published_project_changed() {
    let (state, addr) = spawn_server("secret-token").await;
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
    let (_state, addr) = spawn_server("real-token").await;
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
    let (_state, addr) = spawn_server("any-token").await;
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
    let (state, addr) = spawn_server("k").await;
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
    let (_state, addr) = spawn_server("token").await;
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
