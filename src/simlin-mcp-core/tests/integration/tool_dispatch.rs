// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for the rmcp tool dispatch surface.
//!
//! Spawns a `SimlinMcpServer<TestFileSystemAccess>` against an in-memory
//! duplex pair and uses an rmcp client (`().serve(...)`) to issue real
//! `tools/call` requests.  This is the only path that exercises the
//! `is_error: true` contract for validation failures end-to-end through
//! rmcp's macros — the per-tool e2e suites (`read_model_e2e.rs`,
//! `edit_model_e2e.rs`, `create_model_e2e.rs`) call the tool functions
//! directly and don't touch the `CallToolResult` shape.

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use simlin_mcp_core::server::SimlinMcpServer;
use simlin_mcp_core::test_support::TestFileSystemAccess;

fn minimal_project_json() -> serde_json::Value {
    serde_json::json!({
        "name": "test",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 100.0,
            "dt": "1",
            "saveStep": 1.0,
            "method": "euler",
            "timeUnits": ""
        },
        "models": [{ "name": "main" }]
    })
}

async fn spawn_server_pair() -> (
    rmcp::service::RunningService<rmcp::RoleClient, ()>,
    rmcp::service::RunningService<rmcp::RoleServer, SimlinMcpServer<TestFileSystemAccess>>,
) {
    // Tokio's duplex provides two AsyncRead+AsyncWrite halves connected
    // in memory.  The 64KiB buffer is well above the size of any single
    // JSON-RPC message we exchange in these tests.
    let (server_io, client_io) = tokio::io::duplex(65536);

    let server = SimlinMcpServer::new(
        TestFileSystemAccess,
        "Test instructions".into(),
        vec![],
        "0.0.0".into(),
    );

    // Server-side: spawn `.serve(...)` on the duplex's server half.
    // Client-side: `()` is the rmcp idiom for a no-op handler that only
    // wants to issue requests; pairing it with the duplex's client half
    // gives us a `Peer<RoleClient>` we can call methods on.
    let server_task = tokio::spawn(async move { server.serve(server_io).await });
    let client = ().serve(client_io).await.expect("client failed to initialize");
    let server = server_task
        .await
        .expect("server task panicked")
        .expect("server failed to initialize");
    (client, server)
}

#[tokio::test]
async fn edit_model_validation_error_returns_is_error_true_with_structured_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.sd.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&minimal_project_json()).unwrap(),
    )
    .unwrap();

    let (client, server) = spawn_server_pair().await;

    // The aux references an undefined dependency.  edit_model's
    // diagnostic gate should reject this before writing the file and
    // surface AccessError::Validation, which rmcp must translate to a
    // CallToolResult with is_error: true and structured content.
    let arguments = serde_json::json!({
        "projectPath": path.to_str().unwrap(),
        "operations": [{
            "upsertAuxiliary": {
                "name": "bad",
                "equation": "missing_dependency + 1"
            }
        }]
    });
    let arguments_obj = match arguments {
        serde_json::Value::Object(map) => Some(map),
        _ => unreachable!("arguments is constructed as an object literal"),
    };

    let mut params = CallToolRequestParams::new("EditModel");
    if let Some(args) = arguments_obj {
        params = params.with_arguments(args);
    }
    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("call_tool must return a CallToolResult, not a transport error");

    assert_eq!(
        result.is_error,
        Some(true),
        "validation failure must mark the response with is_error: true"
    );

    let structured = result
        .structured_content
        .expect("validation failure must include structured content");
    let errors = structured
        .get("errors")
        .and_then(|v| v.as_array())
        .expect("structured content must include an `errors` array");
    assert!(
        !errors.is_empty(),
        "validation errors array must include at least one entry, got: {structured}"
    );
    let first = &errors[0];
    assert!(
        first.get("code").and_then(|v| v.as_str()).is_some(),
        "each error must carry a code string: {first}"
    );
    assert!(
        first.get("kind").and_then(|v| v.as_str()).is_some(),
        "each error must carry a kind string: {first}"
    );

    // The file on disk must not have been written.
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(
        on_disk["models"][0].get("auxiliaries").is_none()
            || on_disk["models"][0]["auxiliaries"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true),
        "validation rejection must not write the broken aux to disk"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn read_model_success_returns_is_error_false_with_structured_content() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/logistic_growth_ltm/logistic_growth.stmx"
    );

    let (client, server) = spawn_server_pair().await;

    let arguments = serde_json::json!({ "projectPath": path });
    let arguments_obj = match arguments {
        serde_json::Value::Object(map) => Some(map),
        _ => unreachable!("arguments is constructed as an object literal"),
    };

    let mut params = CallToolRequestParams::new("ReadModel");
    if let Some(args) = arguments_obj {
        params = params.with_arguments(args);
    }
    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("call_tool must succeed");

    // CallToolResult::structured sets is_error to Some(false) for the
    // success path; we accept either Some(false) or None (some MCP
    // clients elide the field on success).
    assert_ne!(
        result.is_error,
        Some(true),
        "successful read_model must not set is_error: true"
    );
    assert!(
        result.structured_content.is_some(),
        "successful read_model must include structured content"
    );
    let structured = result.structured_content.unwrap();
    assert!(
        structured.get("model").is_some(),
        "structured content must include a model snapshot: {structured}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn read_model_missing_file_returns_is_error_true_with_structured_content() {
    let (client, server) = spawn_server_pair().await;

    let arguments = serde_json::json!({ "projectPath": "/does/not/exist/model.sd.json" });
    let arguments_obj = match arguments {
        serde_json::Value::Object(map) => Some(map),
        _ => unreachable!("arguments is constructed as an object literal"),
    };

    let mut params = CallToolRequestParams::new("ReadModel");
    if let Some(args) = arguments_obj {
        params = params.with_arguments(args);
    }
    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("call_tool must return a CallToolResult, not a transport error");

    assert_eq!(
        result.is_error,
        Some(true),
        "missing-file error must set is_error: true"
    );
    let structured = result
        .structured_content
        .expect("missing-file error must include structured content");
    assert!(
        structured.get("error").and_then(|v| v.as_str()).is_some(),
        "structured content must carry an error string: {structured}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn edit_model_mdl_rejection_returns_is_error_true_with_structured_content() {
    let mdl_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/sdeverywhere/models/elmcount/elmcount.mdl"
    );

    let (client, server) = spawn_server_pair().await;

    let arguments = serde_json::json!({
        "projectPath": mdl_path,
        "operations": [{"upsertAuxiliary": {"name": "x", "equation": "1"}}]
    });
    let arguments_obj = match arguments {
        serde_json::Value::Object(map) => Some(map),
        _ => unreachable!("arguments is constructed as an object literal"),
    };

    let mut params = CallToolRequestParams::new("EditModel");
    if let Some(args) = arguments_obj {
        params = params.with_arguments(args);
    }
    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("call_tool must return a CallToolResult, not a transport error");

    assert_eq!(
        result.is_error,
        Some(true),
        ".mdl rejection must set is_error: true"
    );
    let structured = result
        .structured_content
        .expect(".mdl rejection must include structured content");
    let err_str = structured
        .get("error")
        .and_then(|v| v.as_str())
        .expect("structured content must carry an error string");
    assert!(
        err_str.contains(".mdl"),
        ".mdl rejection error must mention .mdl format: {err_str}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn tools_list_returns_pascal_case_names() {
    let (client, server) = spawn_server_pair().await;

    let result = client
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");

    let names: Vec<&str> = result.tools.iter().map(|t| t.name.as_ref()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        vec!["CreateModel", "EditModel", "ReadModel"],
        "wire-level tool names must be PascalCase to preserve @simlin/mcp client compatibility; got: {names:?}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
