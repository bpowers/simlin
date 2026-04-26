// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Phase 8 Task 11 cross-platform smoke test.
//!
//! Spawns the actual `simlin-serve` binary in a tempdir seeded with the
//! three supported on-disk formats (XMILE, Vensim MDL, SD-AI JSON) plus
//! a nested-subdirectory fixture, then exercises both the browser HTTP
//! path (list/read/save) and the MCP tool path (ReadModel/CreateModel)
//! end-to-end. Verifies the canonical disk state after each mutation
//! to catch any regression where the API path returns success without
//! actually persisting bytes.
//!
//! Marked `#[ignore]` so the default `cargo test` workflow doesn't pay
//! the seconds-scale spawn cost on every invocation; the CI matrix job
//! and any developer who wants to exercise the full binary run it via:
//!
//!     cargo test --release --test smoke -- --ignored
//!
//! The test allocates ephemeral ports (`--port 0 --mcp-port 0`) so it
//! never collides with another `simlin-serve` instance on the same host.

#![deny(unsafe_code)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderValue};
use serde_json::{Value, json};
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_simlin-serve");

/// Repository-relative paths to the fixture files. The smoke test
/// reaches outside its package's `tests/fixtures/` directory because
/// the spec calls for the canonical engine fixtures (smallest valid
/// instances of each on-disk format) rather than the ad-hoc test
/// copies under `simlin-serve/tests/fixtures/`.
const TEACUP_XMILE: &str = "test/test-models/samples/teacup/teacup.xmile";
const TEACUP_MDL: &str = "test/test-models/samples/teacup/teacup.mdl";
const SDAI_SIMPLE: &str = "test/sd-ai-simple.sd.json";

/// Resolve a repository-relative fixture path to an absolute one. We
/// walk parents from `CARGO_MANIFEST_DIR` (this crate's root) until we
/// hit the workspace root that contains a `test/test-models` subtree.
/// The workspace root has no stable env var so a lookup is unavoidable.
fn workspace_fixture(rel: &str) -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut cursor = manifest_dir.as_path();
    loop {
        let candidate = cursor.join(rel);
        if candidate.is_file() {
            return candidate;
        }
        cursor = match cursor.parent() {
            Some(p) => p,
            None => panic!(
                "could not find fixture {rel} in any ancestor of {}",
                manifest_dir.display()
            ),
        };
    }
}

/// Lines printed by the binary at startup.
struct StartupOutput {
    ui_url: String,
    mcp_url: String,
}

/// `  UI:  http://127.0.0.1:54321/` -> the URL substring.
fn parse_url_after_label(line: &str) -> Option<String> {
    let idx = line.find("http://")?;
    Some(line[idx..].trim().to_string())
}

/// Spawn the binary and read stdout until both URLs are observed or
/// `timeout` elapses. Returns the running `Child` so the caller can
/// kill it at teardown.
fn spawn_and_collect_urls(temp: &Path, timeout: Duration) -> (Child, StartupOutput) {
    let mut child = Command::new(BIN)
        .args([
            "--port",
            "0",
            "--mcp-port",
            "0",
            "--no-open",
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

    let deadline = Instant::now() + timeout;
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
        panic!("UI URL never appeared in stdout within {timeout:?}");
    });
    let mcp_url = mcp_url.unwrap_or_else(|| {
        let _ = child.kill();
        panic!("MCP URL never appeared in stdout within {timeout:?}");
    });
    (child, StartupOutput { ui_url, mcp_url })
}

/// Strip the trailing `/` off the UI URL so callers can build their own
/// paths against the same origin.
fn ui_origin(ui_url: &str) -> String {
    let parsed = ui_url
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("UI url missing http:// prefix: {ui_url}"));
    let host_port = parsed.split_once('/').map(|(hp, _)| hp).unwrap_or(parsed);
    format!("http://{host_port}")
}

/// Extract a single JSON-RPC response payload from an SSE response
/// body. The MCP server uses Streamable HTTP, which serialises tool
/// responses as Server-Sent Events rather than a plain JSON body.
fn extract_jsonrpc_response(body: &str) -> Value {
    for event in body.split("\n\n") {
        for line in event.lines() {
            let Some(raw) = line.strip_prefix("data:") else {
                continue;
            };
            let rest = raw.trim();
            if !rest.is_empty()
                && let Ok(v) = serde_json::from_str::<Value>(rest)
                && v.get("jsonrpc").is_some()
            {
                return v;
            }
        }
    }
    panic!("no JSON-RPC response in SSE body: {body}");
}

/// RAII guard that always kills the child process and waits for it to
/// exit. Drop runs even on panic, so an assertion failure leaves no
/// orphan listeners on the developer's machine. On Windows, `kill()`
/// translates to `TerminateProcess`, which is sufficient for a child
/// that holds no descendants of its own (the simlin-serve binary
/// doesn't fork).
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore]
async fn smoke_end_to_end_browser_and_mcp_paths() {
    // ---- 1. Tempdir + fixtures ----
    let temp = TempDir::new().expect("tempdir");
    // Canonicalize so paths the binary advertises in stdout are
    // identical to paths we feed back into the MCP tool calls. On
    // macOS the tempdir is under `/var/folders/...` which symlinks to
    // `/private/var/folders/...` -- canonicalize collapses both onto
    // the same anchor the registry uses.
    let root = temp.path().canonicalize().expect("canonicalize tempdir");

    fs::copy(workspace_fixture(TEACUP_XMILE), root.join("teacup.xmile"))
        .expect("copy teacup.xmile");
    fs::copy(workspace_fixture(TEACUP_MDL), root.join("teacup.mdl")).expect("copy teacup.mdl");
    fs::copy(workspace_fixture(SDAI_SIMPLE), root.join("small.sd.json"))
        .expect("copy small.sd.json");
    let subdir = root.join("subdir");
    fs::create_dir(&subdir).expect("mkdir subdir");
    fs::copy(workspace_fixture(TEACUP_XMILE), subdir.join("nested.xmile"))
        .expect("copy nested.xmile");

    // ---- 2. Spawn the binary + parse URLs ----
    let (child, urls) = spawn_and_collect_urls(&root, Duration::from_secs(20));
    let _guard = ChildGuard(child);

    let origin = ui_origin(&urls.ui_url);
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("build reqwest client");

    // ---- 3. /healthz: trivial liveness check ----
    let resp = http
        .get(format!("{origin}/healthz"))
        .send()
        .await
        .expect("GET /healthz");
    assert_eq!(resp.status().as_u16(), 200, "/healthz must return 200");
    let body = resp.text().await.expect("/healthz body");
    assert_eq!(body, "ok", "/healthz body must be the literal string 'ok'");

    // ---- 4. GET /api/projects: registry lists all four entries ----
    let resp = http
        .get(format!("{origin}/api/projects"))
        .send()
        .await
        .expect("GET /api/projects");
    assert_eq!(resp.status().as_u16(), 200, "/api/projects must return 200");
    let listing: Value = resp.json().await.expect("/api/projects body");
    let projects = listing["projects"]
        .as_array()
        .expect("projects array")
        .clone();
    let mut paths: Vec<String> = projects
        .iter()
        .filter_map(|p| p["path"].as_str().map(|s| s.to_string()))
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        vec![
            "small.sd.json".to_string(),
            "subdir/nested.xmile".to_string(),
            "teacup.mdl".to_string(),
            "teacup.xmile".to_string(),
        ],
        "registry must list all four seeded fixtures (recursive scan)"
    );

    // ---- 5. GET /api/projects/teacup.xmile: full read path ----
    let resp = http
        .get(format!("{origin}/api/projects/teacup.xmile"))
        .send()
        .await
        .expect("GET teacup.xmile");
    assert_eq!(resp.status().as_u16(), 200);
    let payload: Value = resp.json().await.expect("teacup.xmile body");
    let initial_version = payload["version"].as_u64().expect("version");
    let json_str = payload["json"].as_str().expect("json field").to_string();
    let mut project: Value = serde_json::from_str(&json_str).expect("nested json");
    assert!(
        project["models"].is_array(),
        "canonical project JSON must expose a `models` array"
    );

    // ---- 6. POST /api/projects/teacup.xmile: mutate and save ----
    // Add a fresh aux to the first model. The teacup fixture doesn't
    // have a `smoke_added_aux` already so a follow-up GET can detect
    // the round-trip. The canonical engine JSON splits variables into
    // separate `stocks`, `flows`, `auxiliaries`, `modules` arrays
    // rather than one polymorphic list.
    let main_model_idx = project["models"]
        .as_array()
        .expect("models array")
        .iter()
        .position(|m| m["name"].as_str() == Some("main"))
        .unwrap_or(0);
    let new_aux = json!({
        "name": "smoke_added_aux",
        "equation": "42"
    });
    let aux_array = project["models"][main_model_idx]
        .as_object_mut()
        .expect("model object")
        .entry("auxiliaries")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .expect("auxiliaries array");
    aux_array.push(new_aux);

    let mutated_json = serde_json::to_string(&project).expect("serialize");
    let save_body = json!({
        "json": mutated_json,
        "version": initial_version,
    });
    let resp = http
        .post(format!("{origin}/api/projects/teacup.xmile"))
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(serde_json::to_vec(&save_body).expect("serialize save"))
        .send()
        .await
        .expect("POST save");
    assert_eq!(
        resp.status().as_u16(),
        200,
        "save must return 200, got {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
    let save_resp: Value = resp.json().await.expect("save body");
    let new_version = save_resp["version"].as_u64().expect("new version");
    assert_eq!(
        new_version,
        initial_version + 1,
        "version must increment by exactly one on a successful save"
    );

    // ---- 7. GET again: confirm the in-memory doc reflects the edit ----
    let resp = http
        .get(format!("{origin}/api/projects/teacup.xmile"))
        .send()
        .await
        .expect("GET teacup.xmile after save");
    assert_eq!(resp.status().as_u16(), 200);
    let after: Value = resp.json().await.expect("after body");
    assert_eq!(
        after["version"].as_u64(),
        Some(new_version),
        "GET after save must report the new version"
    );
    let after_project: Value =
        serde_json::from_str(after["json"].as_str().unwrap()).expect("after project");
    let aux_names: Vec<String> = after_project["models"][main_model_idx]["auxiliaries"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        aux_names.iter().any(|n| n == "smoke_added_aux"),
        "post-save GET must include the new aux: {aux_names:?}"
    );

    // ---- 8. Verify on disk: the .xmile file actually changed ----
    // Parse the on-disk XMILE and confirm the new aux is present.  Goes
    // through `simlin_engine::project_io::open_xmile` so the assertion
    // is robust to formatting / whitespace differences between the
    // round-trip and the original.
    let disk_bytes = fs::read(root.join("teacup.xmile")).expect("read teacup.xmile");
    let disk_project =
        simlin_engine::compat::open_xmile(&mut disk_bytes.as_slice()).expect("parse on-disk xmile");
    let disk_var_idents: Vec<String> = disk_project
        .models
        .iter()
        .flat_map(|m| m.variables.iter())
        .map(|v| v.get_ident().to_string())
        .collect();
    assert!(
        disk_var_idents.iter().any(|n| n == "smoke_added_aux"),
        "on-disk teacup.xmile must reflect the saved edit: {disk_var_idents:?}"
    );

    // ---- 9. MCP path: read_model + create_model ----
    let session_id = open_mcp_session(&http, &urls.mcp_url).await;

    let teacup_abs = root
        .join("teacup.xmile")
        .canonicalize()
        .expect("canonicalize teacup")
        .to_str()
        .expect("utf-8 path")
        .to_string();
    let read_payload = json!({
        "jsonrpc": "2.0",
        "id": 100,
        "method": "tools/call",
        "params": {
            "name": "ReadModel",
            "arguments": { "projectPath": teacup_abs }
        }
    });
    let resp = mcp_post(&http, &urls.mcp_url, &session_id, &read_payload).await;
    let read_response = extract_jsonrpc_response(&resp);
    let result = read_response
        .get("result")
        .unwrap_or_else(|| panic!("ReadModel missing result: {read_response}"));
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        !is_error,
        "MCP ReadModel must succeed against the seeded teacup.xmile: {read_response}"
    );

    // CreateModel: the AI builds a brand-new file outside any subdir.
    let new_file = root.join("mcp_created.sd.json");
    let new_path = new_file.to_str().expect("utf-8 path").to_string();
    let create_payload = json!({
        "jsonrpc": "2.0",
        "id": 101,
        "method": "tools/call",
        "params": {
            "name": "CreateModel",
            "arguments": { "projectPath": new_path }
        }
    });
    let resp = mcp_post(&http, &urls.mcp_url, &session_id, &create_payload).await;
    let create_response = extract_jsonrpc_response(&resp);
    let result = create_response
        .get("result")
        .unwrap_or_else(|| panic!("CreateModel missing result: {create_response}"));
    let is_error = result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(!is_error, "MCP CreateModel must succeed: {create_response}");
    assert!(
        new_file.is_file(),
        "MCP CreateModel must write the file to disk: {}",
        new_file.display()
    );
}

/// Issue a single POST to the MCP endpoint and return the response
/// body as a string. SSE text bodies parse correctly without a streaming
/// reader because each tool call writes a single event-stream frame and
/// closes the response.
async fn mcp_post(http: &reqwest::Client, mcp_url: &str, session_id: &str, body: &Value) -> String {
    let resp = http
        .post(mcp_url)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .header(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        )
        .header("mcp-session-id", session_id)
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(serde_json::to_vec(body).expect("serialize mcp body"))
        .send()
        .await
        .expect("MCP POST");
    let status = resp.status();
    let text = resp.text().await.expect("MCP body text");
    assert!(
        status.is_success(),
        "MCP POST must return 2xx, got {status}: {text}"
    );
    text
}

/// Initialize an MCP session and return the assigned session id. The
/// rmcp Streamable HTTP transport requires both the `initialize`
/// request *and* the `notifications/initialized` notification before
/// any tool call, even when the test runs against a tempdir-isolated
/// instance.
async fn open_mcp_session(http: &reqwest::Client, mcp_url: &str) -> String {
    let init_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "simlin-serve-smoke", "version": "0.1"}
        }
    });
    let resp = http
        .post(mcp_url)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .header(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        )
        .body(serde_json::to_vec(&init_body).expect("serialize init"))
        .send()
        .await
        .expect("POST initialize");
    assert_eq!(resp.status().as_u16(), 200, "initialize must return 200");
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .expect("server must assign mcp-session-id on initialize");
    let _ = resp.text().await;

    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    let resp = http
        .post(mcp_url)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .header(
            ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        )
        .header("mcp-session-id", session_id.clone())
        .header("Mcp-Protocol-Version", "2025-06-18")
        .body(serde_json::to_vec(&initialized).expect("serialize initialized"))
        .send()
        .await
        .expect("POST initialized");
    assert_eq!(
        resp.status().as_u16(),
        202,
        "initialized notification must return 202"
    );

    session_id
}
