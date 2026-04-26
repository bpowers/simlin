// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end smoke test that runs the rebuilt `simlin-mcp` binary as a
//! subprocess and exchanges a single JSON-RPC `initialize` over its
//! stdin/stdout.  The goal is to lock down the wire surface (server
//! name, advertised capabilities, presence of instructions) at the
//! transport boundary — every other test exercises the library in
//! process.
//!
//! `env!("CARGO_BIN_EXE_simlin-mcp")` is set by Cargo only for
//! integration tests under `tests/`, which is why this file lives
//! here and not under `src/`.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: &str = "2025-11-25";
const BINARY_VERSION: &str = env!("CARGO_PKG_VERSION");

const BINARY: &str = env!("CARGO_BIN_EXE_simlin-mcp");

/// Format a single JSON-RPC line.  rmcp's stdio transport reads
/// newline-delimited JSON, so each request must end with `\n`.
fn rpc_line(value: serde_json::Value) -> String {
    let mut s = serde_json::to_string(&value).expect("serialize JSON-RPC value");
    s.push('\n');
    s
}

fn read_response_with_id(
    reader: &mut BufReader<std::process::ChildStdout>,
    expected_id: i64,
    deadline: Instant,
) -> serde_json::Value {
    loop {
        if Instant::now() > deadline {
            panic!("timed out waiting for JSON-RPC response with id={expected_id}");
        }
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .expect("failed to read from simlin-mcp stdout");
        if n == 0 {
            panic!("simlin-mcp closed stdout before responding");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => panic!("simlin-mcp wrote non-JSON line: {trimmed:?}: {e}"),
        };
        // rmcp may emit notifications (id missing) before our matching
        // response — skip those and keep reading.
        if value.get("id") == Some(&serde_json::Value::from(expected_id)) {
            return value;
        }
    }
}

#[test]
fn initialize_returns_expected_server_info_and_capabilities() {
    let mut child = Command::new(BINARY)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn simlin-mcp binary");

    let mut stdin = child.stdin.take().expect("stdin should be piped");
    let stdout = child.stdout.take().expect("stdout should be piped");
    let mut reader = BufReader::new(stdout);

    // The MCP `initialize` request advertises the protocol version we
    // expect to see echoed back in the response, plus an empty
    // capabilities object (we are a simple test client).
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "clientInfo": { "name": "simlin-mcp-smoke", "version": "0.0.0" },
            "capabilities": {}
        }
    });

    stdin
        .write_all(rpc_line(initialize).as_bytes())
        .expect("failed to write initialize request");
    stdin.flush().expect("failed to flush initialize request");

    let deadline = Instant::now() + Duration::from_secs(10);
    let response = read_response_with_id(&mut reader, 1, deadline);

    let result = response
        .get("result")
        .unwrap_or_else(|| panic!("expected `result` field in response, got {response}"));

    // Server identifies itself as `simlin-mcp` so an MCP host can
    // distinguish it from other servers in a multi-server config.
    assert_eq!(
        result["serverInfo"]["name"], "simlin-mcp",
        "serverInfo.name must be 'simlin-mcp', got: {result}"
    );

    // Tools and resources capabilities both advertised — this is the
    // contract that lets `tools/list` and `resources/list` work below.
    assert!(
        result["capabilities"]["tools"].is_object(),
        "tools capability must be an object, got: {}",
        result["capabilities"]
    );
    assert!(
        result["capabilities"]["resources"].is_object(),
        "resources capability must be an object, got: {}",
        result["capabilities"]
    );

    // The protocol version must exactly match the spec date simlin-mcp
    // targets, ensuring the binary's rmcp dependency is current.
    assert_eq!(
        result["protocolVersion"].as_str().unwrap_or(""),
        PROTOCOL_VERSION,
        "protocolVersion must be exactly {PROTOCOL_VERSION}, got: {}",
        result["protocolVersion"]
    );

    // serverInfo.version must match the binary's Cargo.toml version, not
    // the library crate version.
    assert_eq!(
        result["serverInfo"]["version"].as_str().unwrap_or(""),
        BINARY_VERSION,
        "serverInfo.version must be the binary version {BINARY_VERSION}, got: {}",
        result["serverInfo"]["version"]
    );

    // Instructions are non-empty: this is the OUT_DIR-substituted
    // instructions.md content the binary embeds at build time.
    let instructions = result["instructions"]
        .as_str()
        .expect("instructions must be a string");
    assert!(!instructions.is_empty(), "instructions must be non-empty");
    assert!(
        instructions.contains("ReadModel"),
        "instructions should mention ReadModel: {instructions:?}"
    );

    // Complete the handshake.
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    stdin
        .write_all(rpc_line(initialized).as_bytes())
        .expect("failed to write initialized notification");
    stdin.flush().expect("flush");

    // tools/list: assert exactly three PascalCase tool names.
    let tools_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    stdin
        .write_all(rpc_line(tools_req).as_bytes())
        .expect("failed to write tools/list request");
    stdin.flush().expect("flush");

    let tools_resp = read_response_with_id(&mut reader, 2, deadline);
    let tools = tools_resp["result"]["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("tools/list result must include a tools array: {tools_resp}"));
    let mut tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    tool_names.sort_unstable();
    assert_eq!(
        tool_names,
        vec!["CreateModel", "EditModel", "ReadModel"],
        "tools/list must return exactly three PascalCase tool names"
    );

    // resources/list: assert exactly four skill URIs.
    let resources_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "resources/list",
        "params": {}
    });
    stdin
        .write_all(rpc_line(resources_req).as_bytes())
        .expect("failed to write resources/list request");
    stdin.flush().expect("flush");

    let resources_resp = read_response_with_id(&mut reader, 3, deadline);
    let resources = resources_resp["result"]["resources"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("resources/list result must include a resources array: {resources_resp}")
        });
    let mut uris: Vec<&str> = resources.iter().filter_map(|r| r["uri"].as_str()).collect();
    uris.sort_unstable();
    assert_eq!(
        uris,
        vec![
            "simlin://skills/loop-dominance",
            "simlin://skills/pysimlin-basics",
            "simlin://skills/scenario-analysis",
            "simlin://skills/vensim-equation-syntax",
        ],
        "resources/list must return exactly the four production skill URIs"
    );

    drop(stdin);
    let _ = child.wait();
}
