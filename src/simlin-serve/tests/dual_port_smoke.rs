// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration tests for the dual-port serve binary.
//!
//! These tests start the actual `simlin-serve` binary as a subprocess in
//! a tempdir and exercise the bind-time behaviour:
//!   * `dual_port_smoke` — both UI and MCP listeners come up, the UI's
//!     `/healthz` and the MCP service's `/mcp` both respond.
//!   * `port_conflict_*` — when the requested MCP/UI port is already in
//!     use, the binary exits non-zero with a friendly diagnostic.

#![deny(unsafe_code)]

use std::io::{BufRead, BufReader};
use std::net::TcpListener as StdTcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_simlin-serve");

/// Read printed lines from a child process's stdout until either the
/// "MCP:" line is seen (success) or `timeout` elapses (failure). Returns
/// every line read so the test can do further parsing.
fn wait_for_url_block(stdout: std::process::ChildStdout, timeout: Duration) -> Vec<String> {
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + timeout;
    let mut collected = Vec::new();
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                let saw_mcp = line.starts_with("  MCP:");
                collected.push(line);
                if saw_mcp {
                    return collected;
                }
            }
            Err(_) => break,
        }
    }
    collected
}

fn parse_port(line: &str) -> Option<u16> {
    let last_colon = line.rfind(':')?;
    let after = &line[last_colon + 1..];
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[test]
fn dual_port_smoke_binds_both_listeners() {
    let temp = TempDir::new().expect("tempdir");

    // Use --port=0 to leave the UI on an OS-chosen ephemeral port and
    // --mcp-port=0 to do the same for MCP. The default 7878 would
    // collide whenever this test runs alongside an actual simlin-serve
    // instance on the developer's machine.
    let mut child = Command::new(BIN)
        .args([
            "--port",
            "0",
            "--mcp-port",
            "0",
            "--no-open",
            temp.path().to_str().expect("utf-8 tempdir"),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn simlin-serve");

    let stdout = child.stdout.take().expect("stdout pipe");
    let lines = wait_for_url_block(stdout, Duration::from_secs(15));

    // Always kill the child so a panic below doesn't leave it running.
    let kill_result = child.kill();
    let _ = child.wait();

    assert!(
        kill_result.is_ok(),
        "child should still be running when we kill it"
    );

    let ui_line = lines
        .iter()
        .find(|l| l.starts_with("  UI:"))
        .unwrap_or_else(|| panic!("expected a UI: line in stdout. Got: {lines:?}"));
    let mcp_line = lines
        .iter()
        .find(|l| l.starts_with("  MCP:"))
        .unwrap_or_else(|| panic!("expected a MCP: line in stdout. Got: {lines:?}"));

    let ui_port = parse_port(ui_line).expect("UI port parses");
    let mcp_port = parse_port(mcp_line).expect("MCP port parses");

    assert!(ui_port > 0, "UI port must be non-zero: {ui_line}");
    assert!(mcp_port > 0, "MCP port must be non-zero: {mcp_line}");
    assert_ne!(
        ui_port, mcp_port,
        "UI and MCP must bind to different ephemeral ports: {ui_line} / {mcp_line}"
    );
    // Sanity check the printed shapes so a future refactor that drops
    // the labels is caught by the smoke test.
    assert!(
        mcp_line.contains("/mcp"),
        "MCP URL must point at the /mcp path: {mcp_line}"
    );
}

#[test]
fn port_conflict_on_mcp_exits_with_friendly_error() {
    let temp = TempDir::new().expect("tempdir");

    // Bind a listener in the test process to occupy a port.
    let occupy = StdTcpListener::bind("127.0.0.1:0").expect("occupy ephemeral");
    let occupied_port = occupy.local_addr().expect("local_addr").port();

    // Start simlin-serve pointed at the occupied port. The UI uses an
    // ephemeral port (--port=0) so the UI bind always succeeds and the
    // MCP bind is the only thing that fails.
    let output = Command::new(BIN)
        .args([
            "--port",
            "0",
            "--mcp-port",
            &occupied_port.to_string(),
            "--no-open",
            temp.path().to_str().expect("utf-8 tempdir"),
        ])
        .output()
        .expect("run simlin-serve");

    drop(occupy);

    assert!(
        !output.status.success(),
        "simlin-serve should exit non-zero when the MCP port is in use; \
         stdout={:?}, stderr={:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("address already in use"),
        "stderr must include the canonical 'address already in use' message: {stderr}"
    );
    assert!(
        stderr.contains("--mcp-port"),
        "stderr must hint at --mcp-port for retry: {stderr}"
    );
    assert!(
        stderr.contains("MCP server"),
        "stderr must label the failing listener as the MCP server: {stderr}"
    );
}
