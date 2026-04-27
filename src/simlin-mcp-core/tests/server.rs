// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! Tests for the rmcp `ServerHandler` impl owned by `simlin-mcp-core`.
//!
//! Server-info metadata (`get_info`) is exercised via a synchronous
//! handler call.  Resource list and read operations are exercised through
//! a real rmcp client/server duplex pair so the actual `ServerHandler`
//! trait methods (including `ErrorCode::RESOURCE_NOT_FOUND = -32002`) are
//! tested rather than internal helper methods.

use std::path::Path;

use rmcp::model::{PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams};
use rmcp::{ServerHandler, ServiceError, ServiceExt};
use simlin_engine::datamodel;
use simlin_mcp_core::access::{OpenedProject, ProjectAccess};
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::server::{ResourceContent, SimlinMcpServer};
use simlin_mcp_core::types::SourceFormat;

/// Stub access impl for handler-surface tests; tools are never invoked
/// from these tests so every method can be `unreachable!()`.
struct MockAccess;

impl ProjectAccess for MockAccess {
    async fn open(&self, _abs_path: &Path) -> Result<OpenedProject, AccessError> {
        unreachable!("server tests do not invoke tools")
    }

    async fn save(
        &self,
        _abs_path: &Path,
        _project: &datamodel::Project,
        _format: SourceFormat,
        _expected_version: Option<u64>,
    ) -> Result<u64, AccessError> {
        unreachable!("server tests do not invoke tools")
    }

    async fn create(
        &self,
        _abs_path: &Path,
        _project: &datamodel::Project,
        _format: SourceFormat,
    ) -> Result<(), AccessError> {
        unreachable!("server tests do not invoke tools")
    }
}

fn sample_resource() -> ResourceContent {
    ResourceContent {
        uri: "simlin://skills/test".into(),
        name: "test resource".into(),
        description: "synthetic resource for handler tests".into(),
        mime_type: "text/markdown".into(),
        body: "# hello\nworld\n".into(),
    }
}

async fn spawn_server_pair_with_resources(
    resources: Vec<ResourceContent>,
) -> (
    rmcp::service::RunningService<rmcp::RoleClient, ()>,
    rmcp::service::RunningService<rmcp::RoleServer, SimlinMcpServer<MockAccess>>,
) {
    let (server_io, client_io) = tokio::io::duplex(65536);
    let server = SimlinMcpServer::new(
        MockAccess,
        "Test instructions".into(),
        resources,
        "0.0.0".into(),
    );
    let server_task = tokio::spawn(async move { server.serve(server_io).await });
    let client = ().serve(client_io).await.expect("client failed to initialize");
    let server = server_task
        .await
        .expect("server task panicked")
        .expect("server failed to initialize");
    (client, server)
}

#[test]
fn get_info_advertises_latest_protocol_and_capabilities() {
    let server = SimlinMcpServer::new(
        MockAccess,
        "Test instructions".to_string(),
        vec![],
        "1.2.3".into(),
    );
    let info = server.get_info();
    assert_eq!(info.protocol_version, ProtocolVersion::LATEST);
    assert_eq!(info.server_info.name, "simlin-mcp");
    assert_eq!(info.server_info.version, "1.2.3");
    assert_eq!(info.instructions.as_deref(), Some("Test instructions"));
    assert!(info.capabilities.tools.is_some(), "tools must be enabled");
    assert!(
        info.capabilities.resources.is_some(),
        "resources must be enabled"
    );
}

#[tokio::test]
async fn list_resources_returns_provided_entries() {
    let (client, server) = spawn_server_pair_with_resources(vec![sample_resource()]).await;

    let result = client
        .peer()
        .list_resources(None)
        .await
        .expect("resources/list must succeed");

    assert_eq!(result.resources.len(), 1);
    assert_eq!(result.resources[0].raw.uri, "simlin://skills/test");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn read_resource_returns_body_for_known_uri() {
    let (client, server) = spawn_server_pair_with_resources(vec![sample_resource()]).await;

    let result = client
        .peer()
        .read_resource(ReadResourceRequestParams::new("simlin://skills/test"))
        .await
        .expect("resources/read must succeed for known URI");

    assert_eq!(result.contents.len(), 1);
    let body = match &result.contents[0] {
        rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
        other => panic!("expected TextResourceContents, got {other:?}"),
    };
    assert_eq!(body, "# hello\nworld\n");

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn read_resource_returns_error_code_for_unknown_uri() {
    let (client, server) = spawn_server_pair_with_resources(vec![sample_resource()]).await;

    let result = client
        .peer()
        .read_resource(ReadResourceRequestParams::new("simlin://skills/missing"))
        .await;

    match result {
        Err(ServiceError::McpError(e)) => {
            // ErrorCode::RESOURCE_NOT_FOUND is -32002 in the MCP spec.
            assert_eq!(
                e.code.0, -32002,
                "unknown URI must surface as RESOURCE_NOT_FOUND (-32002), got code {}",
                e.code.0
            );
        }
        Err(other) => panic!("expected McpError(-32002), got {other:?}"),
        Ok(_) => panic!("expected an error for unknown URI, got Ok"),
    }

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn list_resources_empty_when_no_resources_registered() {
    let (client, server) = spawn_server_pair_with_resources(vec![]).await;

    let result = client
        .peer()
        .list_resources(None::<PaginatedRequestParams>)
        .await
        .expect("resources/list must succeed even with empty list");

    assert!(
        result.resources.is_empty(),
        "no resources registered => empty list"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
