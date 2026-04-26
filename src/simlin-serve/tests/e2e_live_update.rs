// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end smoke test for the Phase 3 save -> WebSocket -> live state
//! pipeline.
//!
//! Pattern: bind a real `TcpListener`, spawn the router via `axum::serve`,
//! connect a WS client, POST two saves against the same project, and
//! verify (a) each POST yields a `ProjectChanged` event over the WS in
//! version order, (b) the final GET returns a project that reflects
//! both edits.
//!
//! This is the composition test for AC4.1: it exercises the actual HTTP
//! and WebSocket surfaces (no router-direct shortcuts) so the wire
//! format and the broadcast plumbing are validated end-to-end.

#![deny(unsafe_code)]

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use futures_util::StreamExt;
use simlin_serve::build_router;
use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
const TOKEN: &str = "e2e-secret-token";

fn copy_fixture(name: &str, dest_dir: &std::path::Path) -> PathBuf {
    let src = PathBuf::from(FIXTURES_DIR).join(name);
    let dest = dest_dir.join(name);
    fs::copy(&src, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
    dest
}

/// Bind a server on a random port, seed the registry with the given
/// fixture, and return the listening address plus the AppState (so the
/// test can also seed the registry). The caller must keep `TempDir`
/// alive until the test completes; once it's dropped the fixture
/// directory is removed and the server starts returning 404.
async fn spawn_server(fixture: &str) -> (AppState, String, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let abs = copy_fixture(fixture, dir.path());
    let canonical_root = dir.path().canonicalize().expect("canonicalize root");
    let abs_canonical = abs.canonicalize().expect("canonicalize fixture");

    let metadata = fs::metadata(&abs_canonical).expect("metadata");
    let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let format = match abs_canonical
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
    {
        "stmx" => ProjectFormat::Stmx,
        "xmile" => ProjectFormat::Xmile,
        "mdl" => ProjectFormat::Mdl,
        other => panic!("unsupported fixture format: {other}"),
    };

    let state = AppState {
        registry: Arc::new(ProjectRegistry::new(canonical_root.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(canonical_root),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(TOKEN.to_string()),
    };
    state.registry.upsert(
        abs_canonical.clone(),
        ProjectMeta {
            path: PathBuf::new(),
            format,
            mtime,
            size: metadata.len(),
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: 0,
            last_diagnostic_keys: std::collections::BTreeSet::new(),
        },
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    let router = build_router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    (state, format!("127.0.0.1:{}", port), dir)
}

/// Minimal HTTP/1.1 over raw TCP: write the request, read until the
/// connection closes, return (status, body bytes). We use
/// `Connection: close` and a fixed `Content-Length` so the response
/// terminates without needing a chunked decoder. Adequate for the
/// loopback test surface; production-grade clients are out of scope.
async fn raw_http(addr: &str, request: &[u8]) -> (u16, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(request).await.expect("write");
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.expect("read");
    let header_end = find_subsequence(&buf, b"\r\n\r\n").expect("response has header terminator");
    let header_str = std::str::from_utf8(&buf[..header_end]).expect("headers utf8");
    let status_line = header_str.lines().next().expect("status line");
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .expect("status code");
    let body = buf[header_end + 4..].to_vec();
    (status, body)
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

async fn http_get(addr: &str, path: &str) -> (u16, serde_json::Value) {
    let request =
        format!("GET /api/projects/{path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    let (status, body) = raw_http(addr, request.as_bytes()).await;
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "GET response not json: {e}; body: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, value)
}

async fn post_save(addr: &str, path: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let body_bytes = body.to_string();
    let request = format!(
        "POST /api/projects/{path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        path = path,
        addr = addr,
        len = body_bytes.len(),
        body = body_bytes,
    );
    let (status, body) = raw_http(addr, request.as_bytes()).await;
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap_or_else(|e| {
        panic!(
            "POST response not json: {e}; body: {}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, value)
}

/// Read one text frame from the WS, decoding control frames silently.
/// Bounded by `timeout` so a hung test fails fast in CI.
async fn next_text_frame<S>(ws: &mut S, timeout: std::time::Duration) -> serde_json::Value
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let msg = tokio::time::timeout(remaining, ws.next())
            .await
            .expect("timeout waiting for ws frame")
            .expect("ws stream ended unexpectedly")
            .expect("ws read error");
        match msg {
            Message::Text(t) => {
                return serde_json::from_str(&t).expect("ws frame is valid json");
            }
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {
                continue;
            }
            Message::Close(_) => panic!("ws closed before delivering a text frame"),
        }
    }
}

/// Read text frames until one whose `type` field matches `expected` is
/// found, discarding any intervening frames. Useful when the test cares
/// about a specific notification kind in a stream that may include
/// adjacent variants (e.g. `projectChanged` interleaved with
/// `diagnosticsChanged`). Bounded by `timeout`.
async fn next_text_frame_of_type<S>(
    ws: &mut S,
    expected: &str,
    timeout: std::time::Duration,
) -> serde_json::Value
where
    S: futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for ws frame of type {expected:?}");
        }
        let frame = next_text_frame(ws, remaining).await;
        if frame["type"].as_str() == Some(expected) {
            return frame;
        }
    }
}

#[tokio::test]
async fn save_post_emits_project_changed_and_get_reflects_merged_state() {
    let (_state, addr, _dir) = spawn_server("teacup.xmile").await;

    // Connect the WS client AFTER the server is bound + registered. The
    // broadcast channel does not replay history, so any save fired
    // before subscription would be missed.
    let url = format!("ws://{}/api/updates?token={}", addr, TOKEN);
    let (mut ws, response) = connect_async(&url).await.expect("ws connect");
    assert_eq!(
        response.status().as_u16(),
        101,
        "ws upgrade should be 101 Switching Protocols"
    );

    // Read the current canonical JSON via HTTP so we have a valid body
    // to mutate. Going through the public API (rather than reading the
    // file) is what makes this an end-to-end test.
    let (status, get_body) = http_get(&addr, "teacup.xmile").await;
    assert_eq!(status, 200);
    let canonical_str = get_body["json"].as_str().expect("json string");
    let mut canonical: serde_json::Value =
        serde_json::from_str(canonical_str).expect("parse canonical");

    // First save: rename the project. Top-level scalar mutation is the
    // simplest variable-shape change that lets us assert "edit landed"
    // without depending on the exact stock layout.
    canonical["name"] = serde_json::Value::String("e2e-first-edit".to_string());
    let body = serde_json::json!({
        "json": serde_json::to_string(&canonical).unwrap(),
        "version": 0,
    });
    let (status, save_body) = post_save(&addr, "teacup.xmile", &body).await;
    assert_eq!(
        status, 200,
        "first save should succeed; body: {}",
        save_body
    );
    assert_eq!(save_body["version"].as_u64(), Some(1));

    // The WS subscriber must see the corresponding ProjectChanged
    // within a small timeout. The 5s budget is generous for CI; in
    // practice the publish happens microseconds after the response.
    //
    // Phase 7 may also produce a `diagnosticsChanged` AFTER this
    // `projectChanged` if the project's diagnostic set differs from
    // its cached snapshot. We filter to the variant we care about so
    // the test's intent (project change broadcast) stays focused.
    let ev =
        next_text_frame_of_type(&mut ws, "projectChanged", std::time::Duration::from_secs(5)).await;
    assert_eq!(ev["path"].as_str(), Some("teacup.xmile"));
    assert_eq!(ev["version"].as_u64(), Some(1));
    assert_eq!(ev["source"].as_str(), Some("user"));

    // Second save: mutate a different field. The merge primitive is
    // supposed to retain the first edit; we'll assert that the final
    // GET shows both.
    let (status, get_body2) = http_get(&addr, "teacup.xmile").await;
    assert_eq!(status, 200);
    let canonical_str2 = get_body2["json"]
        .as_str()
        .expect("json string after save 1");
    let mut canonical2: serde_json::Value =
        serde_json::from_str(canonical_str2).expect("parse canonical 2");
    // The first edit must already be visible on the round-trip GET so
    // we know we're working from the just-saved state, not a stale
    // copy. This is also what the SPA does after seeing a
    // ProjectChanged event.
    assert_eq!(
        canonical2["name"].as_str(),
        Some("e2e-first-edit"),
        "first edit must be visible in the post-save GET",
    );

    // Apply a separate mutation: bump simSpecs.dt. The teacup XMILE
    // serializes dt as a string in canonical JSON, so we set it that
    // way to avoid type-coercion ambiguity on the round-trip.
    canonical2["simSpecs"]["dt"] = serde_json::Value::String("0.25".to_string());
    let body2 = serde_json::json!({
        "json": serde_json::to_string(&canonical2).unwrap(),
        "version": 1,
    });
    let (status, save_body2) = post_save(&addr, "teacup.xmile", &body2).await;
    assert_eq!(
        status, 200,
        "second save should succeed; body: {}",
        save_body2
    );
    assert_eq!(save_body2["version"].as_u64(), Some(2));

    let ev2 =
        next_text_frame_of_type(&mut ws, "projectChanged", std::time::Duration::from_secs(5)).await;
    assert_eq!(ev2["path"].as_str(), Some("teacup.xmile"));
    assert_eq!(ev2["version"].as_u64(), Some(2));
    assert_eq!(ev2["source"].as_str(), Some("user"));

    // Final GET reflects both mutations.
    let (status, final_body) = http_get(&addr, "teacup.xmile").await;
    assert_eq!(status, 200);
    assert_eq!(final_body["version"].as_u64(), Some(2));
    let final_str = final_body["json"].as_str().expect("final json string");
    let final_canonical: serde_json::Value =
        serde_json::from_str(final_str).expect("parse final canonical");
    assert_eq!(
        final_canonical["name"].as_str(),
        Some("e2e-first-edit"),
        "first save's edit must persist through the second save",
    );
    // The simSpecs.dt round-trip can come back as either a string or
    // a number depending on how the json::Project field is typed.
    // Both representations of "0.25" are acceptable as long as the
    // value is what we wrote.
    let dt = &final_canonical["simSpecs"]["dt"];
    let dt_matches = dt
        .as_str()
        .map(|s| s == "0.25")
        .unwrap_or_else(|| dt.as_f64() == Some(0.25));
    assert!(
        dt_matches,
        "second save's edit must be visible; saw simSpecs.dt = {}",
        dt
    );
}
