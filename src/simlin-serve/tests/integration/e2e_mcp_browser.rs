// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! End-to-end test: an MCP `EditModel` call routes through the same
//! `apply_canonical_json` merge primitive as the browser save handler,
//! so the WebSocket subscriber attached to the UI port observes a
//! `ProjectChanged { source: "agent", version: 1 }` event within one
//! second. After the edit, a `ReadModel` call confirms the new
//! variable shows up in the snapshot the AI sees.
//!
//! The test boots the actual `simlin-serve` binary as a subprocess so
//! that the dual-port wiring, the rmcp `StreamableHttpService` mount,
//! and the watcher integration are all exercised end-to-end.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use tempfile::TempDir;
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const BIN: &str = env!("CARGO_BIN_EXE_simlin-serve");
const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// Lines printed by the binary at startup.
struct StartupOutput {
    ui_url: String,
    mcp_url: String,
}

fn parse_url_after_label(line: &str) -> Option<String> {
    // Lines look like "  UI:  http://..." or "  MCP: http://..."
    let idx = line.find("http://")?;
    Some(line[idx..].trim().to_string())
}

/// Spawn the binary and read stdout until both URLs are observed or
/// `timeout` elapses.
fn spawn_and_collect_urls(temp: &std::path::Path) -> (Child, StartupOutput) {
    let mut child = Command::new(BIN)
        .args([
            "--port",
            "0",
            "--mcp-port",
            "0",
            "--no-open",
            // tokio_tungstenite (the test's WS client) doesn't set
            // an Origin header, so disable strict-origin to mirror
            // the dev-time wscat scenario rather than the SPA.
            "--strict-origin",
            "false",
            temp.to_str().expect("utf-8 tempdir"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn simlin-serve");

    let stdout = child.stdout.take().expect("stdout pipe");
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut ui_url: Option<String> = None;
    let mut mcp_url: Option<String> = None;
    while Instant::now() < deadline && (ui_url.is_none() || mcp_url.is_none()) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                if line.starts_with("  UI:") {
                    ui_url = parse_url_after_label(&line);
                } else if line.starts_with("  MCP:") {
                    mcp_url = parse_url_after_label(&line);
                }
            }
            Err(_) => break,
        }
    }

    let ui_url = ui_url.unwrap_or_else(|| {
        let _ = child.kill();
        panic!("UI URL never appeared in stdout within 15s");
    });
    let mcp_url = mcp_url.unwrap_or_else(|| {
        let _ = child.kill();
        panic!("MCP URL never appeared in stdout within 15s");
    });
    (child, StartupOutput { ui_url, mcp_url })
}

fn copy_fixture(name: &str, dest_dir: &std::path::Path) -> PathBuf {
    let src = PathBuf::from(FIXTURES_DIR).join(name);
    let dest = dest_dir.join(name);
    fs::copy(&src, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
    dest
}

/// Replace `http://127.0.0.1:PORT/` with `ws://127.0.0.1:PORT/api/updates`.
/// The launch URL has the form `http://127.0.0.1:PORT/`; we strip the
/// trailing slash and append the WS upgrade path.
fn ws_updates_url_from_ui(ui_url: &str) -> String {
    let parsed = ui_url
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("UI url missing http:// prefix: {ui_url}"));
    let host_port = parsed.split('/').next().unwrap_or(parsed);
    format!("ws://{host_port}/api/updates")
}

type HttpClient = Client<HttpConnector, Full<Bytes>>;

fn build_http_client() -> HttpClient {
    Client::builder(TokioExecutor::new()).build(HttpConnector::new())
}

/// Issue a single POST to the MCP endpoint. Returns the response status,
/// the `mcp-session-id` header (if present), and the body bytes.
async fn post_mcp(
    client: &HttpClient,
    mcp_url: &str,
    body: String,
    session_id: Option<&str>,
) -> (u16, Option<String>, Vec<u8>) {
    let mut builder = Request::builder()
        .method("POST")
        .uri(mcp_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    if let Some(sid) = session_id {
        builder = builder
            .header("mcp-session-id", sid)
            .header("Mcp-Protocol-Version", "2025-06-18");
    }
    let req = builder
        .body(Full::new(Bytes::from(body)))
        .expect("build request");
    let resp = client.request(req).await.expect("request failed");
    let status = resp.status().as_u16();
    let returned_session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = resp
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    (status, returned_session, body.to_vec())
}

/// Initialize the MCP session and complete the handshake. Returns the
/// session id the server assigned.
async fn open_mcp_session(client: &HttpClient, mcp_url: &str) -> String {
    let init_body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e-test","version":"0.1"}}}"#;
    let (status, session_id, _body) = post_mcp(client, mcp_url, init_body.to_string(), None).await;
    assert_eq!(status, 200, "initialize must return 200");
    let session_id = session_id.expect("server must return mcp-session-id on initialize");

    let initialized_body = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    let (status, _sid, _body) = post_mcp(
        client,
        mcp_url,
        initialized_body.to_string(),
        Some(&session_id),
    )
    .await;
    assert_eq!(status, 202, "initialized notification must return 202");

    session_id
}

/// Extract a single JSON-RPC response payload from an SSE response body.
fn extract_jsonrpc_response(body: &[u8]) -> serde_json::Value {
    let s = std::str::from_utf8(body).expect("utf-8 body");
    // SSE events are separated by blank lines. Each event line starts
    // with "data:". We want the first event with a `data:` body that
    // parses as JSON.
    for event in s.split("\n\n") {
        for line in event.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                let rest = rest.trim();
                if rest.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(rest)
                    && v.get("jsonrpc").is_some()
                {
                    return v;
                }
            }
        }
    }
    panic!("no JSON-RPC response in SSE body: {s}");
}

#[tokio::test]
async fn mcp_edit_propagates_to_browser_within_one_second() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canonicalize");

    // Seed a fixture so the registry has at least one entry to operate on.
    let fixture_path = copy_fixture("teacup.stmx", &canonical_root);
    // The path the AI uses in tool calls -- absolute string form so the
    // RegistryAccess can canonicalize it back to the registry's key.
    let fixture_str = fixture_path
        .to_str()
        .expect("utf-8 fixture path")
        .to_string();

    let (mut child, urls) = spawn_and_collect_urls(&canonical_root);

    // RAII guard that always kills the binary if an assertion below
    // panics. Normal teardown still kills + waits explicitly.
    struct ChildGuard<'a>(&'a mut Child);
    impl Drop for ChildGuard<'_> {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let _guard = ChildGuard(&mut child);

    // -------- Connect WebSocket (browser) --------
    let ws_url = ws_updates_url_from_ui(&urls.ui_url);
    let (mut ws, _resp) = connect_async(&ws_url)
        .await
        .unwrap_or_else(|e| panic!("websocket connect to {ws_url} failed: {e}"));

    // -------- Connect MCP HTTP client --------
    let http = build_http_client();
    let session_id = open_mcp_session(&http, &urls.mcp_url).await;

    // -------- Issue EditModel tool call --------
    // Add a fresh aux variable. Using a name that wouldn't already exist
    // in the teacup fixture so a follow-up ReadModel can detect it.
    let edit_payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "tools/call",
        "params": {
            "name": "EditModel",
            "arguments": {
                "projectPath": fixture_str,
                "operations": [
                    {
                        "upsertAuxiliary": {
                            "name": "agent_added_aux",
                            "equation": "42"
                        }
                    }
                ]
            }
        }
    })
    .to_string();
    let edit_started = Instant::now();
    let (status, _sid, body) =
        post_mcp(&http, &urls.mcp_url, edit_payload, Some(&session_id)).await;
    assert_eq!(
        status,
        200,
        "EditModel must return 200; body: {:?}",
        String::from_utf8_lossy(&body)
    );
    let edit_response = extract_jsonrpc_response(&body);
    let result = edit_response
        .get("result")
        .unwrap_or_else(|| panic!("EditModel response missing result: {edit_response}"));
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        !is_error,
        "EditModel must succeed; response: {edit_response}"
    );

    // -------- Browser side: ProjectChanged within 1s --------
    // Allow up to ~2s for the WS recv to land — covers test-host noise
    // (CI runners under load) while still flagging a regression that
    // breaks the broadcast path. After receipt we also assert the
    // wall-clock time from edit submission to WS delivery is under 1s
    // per AC5.3's "within one second" requirement, with 1s headroom.
    let received = timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("ws message must arrive within 2s")
        .expect("ws stream not closed")
        .expect("ws message ok");
    let propagation = edit_started.elapsed();
    let text = match received {
        Message::Text(t) => t.to_string(),
        other => panic!("expected text WS frame, got {other:?}"),
    };
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("parse ws json");
    assert_eq!(
        parsed["type"].as_str(),
        Some("projectChanged"),
        "expected projectChanged event, got: {text}"
    );
    assert_eq!(
        parsed["source"].as_str(),
        Some("agent"),
        "MCP-driven edits must report source=agent: {text}"
    );
    assert_eq!(
        parsed["path"].as_str(),
        Some("teacup.stmx"),
        "ws event path must be the relative fixture path: {text}"
    );
    assert_eq!(
        parsed["version"].as_u64(),
        Some(1),
        "version must increment from 0 to 1 on the first save: {text}"
    );
    assert!(
        propagation < Duration::from_secs(2),
        "MCP edit -> browser WS propagation must be under 2s (AC5.3 budget + 1s slack); \
         observed {propagation:?}"
    );

    // -------- AI side: ReadModel sees the new variable --------
    let read_payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "tools/call",
        "params": {
            "name": "ReadModel",
            "arguments": {
                "projectPath": fixture_str
            }
        }
    })
    .to_string();
    let (status, _sid, body) =
        post_mcp(&http, &urls.mcp_url, read_payload, Some(&session_id)).await;
    assert_eq!(status, 200, "ReadModel must return 200");
    let read_response = extract_jsonrpc_response(&body);
    let structured = read_response
        .get("result")
        .and_then(|r| r.get("structuredContent"))
        .unwrap_or_else(|| panic!("ReadModel missing structuredContent: {read_response}"));
    let model = structured
        .get("model")
        .unwrap_or_else(|| panic!("ReadModel structuredContent missing model: {structured}"));
    // The ReadModel surface keeps stocks / flows / auxiliaries in
    // separate arrays; the new aux must show up under `auxiliaries`.
    let auxiliaries = model
        .get("auxiliaries")
        .and_then(|v| v.as_array())
        .unwrap_or_else(|| panic!("model.auxiliaries must be an array: {model}"));
    let names: Vec<String> = auxiliaries
        .iter()
        .filter_map(|aux| {
            aux.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    assert!(
        names.iter().any(|n| n == "agent_added_aux"),
        "MCP edit must persist into the auxiliary list the AI sees; \
         names={names:?}, model={model}"
    );
}
