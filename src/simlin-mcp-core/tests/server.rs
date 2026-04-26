// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! Tests for the rmcp `ServerHandler` impl owned by `simlin-mcp-core`.
//! These exercise the static surface (`get_info`, list/read resources)
//! against a no-op `MockAccess` so we never touch the filesystem.  Tool
//! call dispatch is exercised end-to-end by the binary's transport
//! tests in Subcomponent C; here we only assert the metadata surface.

use std::path::Path;

use rmcp::ServerHandler;
use rmcp::model::ProtocolVersion;
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

#[test]
fn get_info_advertises_latest_protocol_and_capabilities() {
    let server = SimlinMcpServer::new(MockAccess, "Test instructions".to_string(), vec![]);
    let info = server.get_info();
    assert_eq!(info.protocol_version, ProtocolVersion::LATEST);
    assert_eq!(info.server_info.name, "simlin-mcp");
    assert_eq!(info.instructions.as_deref(), Some("Test instructions"));
    assert!(info.capabilities.tools.is_some(), "tools must be enabled");
    assert!(
        info.capabilities.resources.is_some(),
        "resources must be enabled"
    );
}

#[tokio::test]
async fn list_resources_returns_provided_entries() {
    let server = SimlinMcpServer::new(
        MockAccess,
        "Test instructions".to_string(),
        vec![sample_resource()],
    );
    // Calling list_resources directly uses the test-only helper that
    // wraps the inherent method; the rmcp dispatch path is tested in
    // the binary's smoke tests.
    let listed = server.list_resources_for_test();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].raw.uri, "simlin://skills/test");
}

#[tokio::test]
async fn read_resource_returns_body_for_known_uri() {
    let server = SimlinMcpServer::new(
        MockAccess,
        "Test instructions".to_string(),
        vec![sample_resource()],
    );
    let body = server
        .read_resource_for_test("simlin://skills/test")
        .expect("known URI must resolve");
    assert_eq!(body, "# hello\nworld\n");
}

#[tokio::test]
async fn read_resource_returns_none_for_unknown_uri() {
    let server = SimlinMcpServer::new(MockAccess, "x".into(), vec![sample_resource()]);
    assert!(
        server
            .read_resource_for_test("simlin://skills/missing")
            .is_none(),
        "unknown URI must surface as None so the rmcp handler can map to ErrorCode(-32002)"
    );
}
