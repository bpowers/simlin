// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for the rmcp tool dispatch surface.
//!
//! Spawns a `SimlinMcpServer<FsAccess>` against an in-memory duplex pair
//! and uses an rmcp client (`().serve(...)`) to issue real `tools/call`
//! requests.  This is the only path that exercises the `is_error: true`
//! contract for validation failures end-to-end through rmcp's macros —
//! the per-tool e2e suites (`read_model_e2e.rs`, `edit_model_e2e.rs`,
//! `create_model_e2e.rs`) call the tool functions directly and don't
//! touch the `CallToolResult` shape.

use std::path::Path;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use simlin_engine::datamodel;
use simlin_engine::json as ejson;
use simlin_mcp_core::access::{OpenedProject, ProjectAccess};
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::server::SimlinMcpServer;
use simlin_mcp_core::types::SourceFormat;

/// Test-local stateless filesystem impl mirroring `read_model_e2e.rs`'s
/// `FsAccess`.  We need open + save here so `edit_model` can run end to
/// end and surface a validation error from the diagnostic gate.
struct FsAccess;

impl ProjectAccess for FsAccess {
    async fn open(&self, abs_path: &Path) -> Result<OpenedProject, AccessError> {
        let contents = tokio::fs::read_to_string(abs_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AccessError::NotFound {
                    path: abs_path.to_path_buf(),
                }
            } else {
                AccessError::IoError(e)
            }
        })?;
        let (project, source_format) = simlin_mcp_core::open::open_project(abs_path, &contents)?;
        Ok(OpenedProject {
            project,
            source_format,
            version: 0,
        })
    }

    async fn save(
        &self,
        abs_path: &Path,
        project: &datamodel::Project,
        format: SourceFormat,
        _expected_version: Option<u64>,
    ) -> Result<u64, AccessError> {
        let bytes = match format {
            SourceFormat::Xmile => simlin_engine::to_xmile(project)
                .map_err(|e| {
                    AccessError::ParseError(anyhow::anyhow!("failed to serialize XMILE: {e:?}"))
                })?
                .into_bytes(),
            SourceFormat::NativeJson => {
                let json_project = ejson::Project::from(project);
                serde_json::to_vec_pretty(&json_project)
                    .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize: {e}")))?
            }
            SourceFormat::SdaiJson => {
                let sdai_model = simlin_engine::json_sdai::SdaiModel::from(project);
                serde_json::to_vec_pretty(&sdai_model)
                    .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize: {e}")))?
            }
        };
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(0)
    }

    async fn create(
        &self,
        _abs_path: &Path,
        _project: &datamodel::Project,
        _format: SourceFormat,
    ) -> Result<(), AccessError> {
        unreachable!("tool_dispatch tests do not call create")
    }
}

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
    rmcp::service::RunningService<rmcp::RoleServer, SimlinMcpServer<FsAccess>>,
) {
    // Tokio's duplex provides two AsyncRead+AsyncWrite halves connected
    // in memory.  The 64KiB buffer is well above the size of any single
    // JSON-RPC message we exchange in these tests.
    let (server_io, client_io) = tokio::io::duplex(65536);

    let server = SimlinMcpServer::new(FsAccess, "Test instructions".into(), vec![]);

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

    let mut params = CallToolRequestParams::new("edit_model");
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

    let mut params = CallToolRequestParams::new("read_model");
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
