// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! rmcp `ServerHandler` implementation shared by every transport.
//!
//! The struct is deliberately generic over `A: ProjectAccess` so the
//! stdio binary, the Phase 6 HTTP host, and any in-process test harness
//! can mount the same tool surface against their own backing store
//! without re-implementing dispatch.  rmcp's `tool_router` macro emits
//! a static dispatch table tied to `Self`, so each `A` concrete-type
//! instantiation gets its own router and we never need
//! `Arc<dyn ProjectAccess>`.

use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    AnnotateAble, CallToolResult, Content, ErrorCode, Implementation, ListResourcesResult,
    PaginatedRequestParams, ProtocolVersion, RawResource, ReadResourceRequestParams,
    ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, tool, tool_handler, tool_router};

use crate::access::ProjectAccess;
use crate::errors::AccessError;
use crate::tools::create_model::{CreateModelInput, create_model};
use crate::tools::edit_model::{EditModelInput, edit_model};
use crate::tools::read_model::{ReadModelInput, read_model};

/// A markdown skill resource embedded in the server's resource list.
///
/// The library holds the content as `String` so the binary can pass
/// either `include_str!`-loaded `&'static str` (cloned once at
/// startup) or runtime-loaded data without changing the type.  Both
/// `simlin-mcp` (stdio) and `simlin-serve` (HTTP) construct this list
/// themselves; the library is content-agnostic.
#[derive(Debug, Clone)]
pub struct ResourceContent {
    pub uri: String,
    pub name: String,
    pub description: String,
    pub mime_type: String,
    pub body: String,
}

/// rmcp `ServerHandler` implementation for the Simlin MCP tool surface.
///
/// Holds an `Arc<A>` rather than `A` directly so the handler can be
/// freely cloned by rmcp's session machinery (the streamable-HTTP
/// service factory in Phase 6 expects `Self: Clone`).  The instructions
/// string and resource list are likewise behind `Arc` so cloning is
/// cheap.
#[derive(Clone)]
pub struct SimlinMcpServer<A: ProjectAccess> {
    access: Arc<A>,
    instructions: Arc<String>,
    resources: Arc<Vec<ResourceContent>>,
    // The router is consumed by the rmcp `tool_handler` macro expansion,
    // not by direct method calls — silence the dead-code warning.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl<A: ProjectAccess> SimlinMcpServer<A> {
    pub fn new(access: A, instructions: String, resources: Vec<ResourceContent>) -> Self {
        Self {
            access: Arc::new(access),
            instructions: Arc::new(instructions),
            resources: Arc::new(resources),
            tool_router: Self::tool_router(),
        }
    }

    /// Test-only helper that returns the resource list as rmcp's
    /// `Resource` values.  The actual `list_resources` ServerHandler
    /// method wraps this; exposing it directly keeps unit tests free
    /// of `RequestContext` plumbing.
    #[doc(hidden)]
    pub fn list_resources_for_test(&self) -> Vec<Resource> {
        self.resources
            .iter()
            .map(|r| {
                let raw = RawResource::new(r.uri.clone(), r.name.clone());
                let raw = if r.description.is_empty() {
                    raw
                } else {
                    raw.with_description(r.description.clone())
                };
                let raw = if r.mime_type.is_empty() {
                    raw
                } else {
                    raw.with_mime_type(r.mime_type.clone())
                };
                raw.no_annotation()
            })
            .collect()
    }

    /// Test-only helper for `read_resource`.  Returns `Some(body)` for
    /// known URIs, `None` for unknown ones (the public ServerHandler
    /// method maps `None` to `ErrorCode::RESOURCE_NOT_FOUND`).
    #[doc(hidden)]
    pub fn read_resource_for_test(&self, uri: &str) -> Option<String> {
        self.resources
            .iter()
            .find(|r| r.uri == uri)
            .map(|r| r.body.clone())
    }
}

#[tool_router]
impl<A: ProjectAccess> SimlinMcpServer<A> {
    #[tool(
        description = "Read a system dynamics model file and return its JSON snapshot \
            enriched with loop dominance analysis. \
            Supports XMILE (.stmx, .xmile), Vensim (.mdl), and Simlin JSON formats."
    )]
    async fn read_model(
        &self,
        Parameters(input): Parameters<ReadModelInput>,
    ) -> Result<CallToolResult, McpError> {
        match read_model(&*self.access, input).await {
            Ok(output) => call_tool_success(&output),
            Err(err) => Ok(call_tool_error(&err)),
        }
    }

    #[tool(description = "Edit a system dynamics model by applying operations. \
            Supports upserting stocks, flows, and auxiliaries, removing variables, \
            and updating simulation specs. Returns a refreshed model snapshot \
            with loop dominance analysis after applying changes. \
            Upsert replaces the full variable definition; omitted optional fields \
            default to empty. Use ReadModel first to get current state, then \
            include all fields you want to preserve. \
            Note: Vensim .mdl files are read-only -- use ReadModel to inspect them, \
            then CreateModel to start a new .sd.json file you can edit.")]
    async fn edit_model(
        &self,
        Parameters(input): Parameters<EditModelInput>,
    ) -> Result<CallToolResult, McpError> {
        match edit_model(&*self.access, input).await {
            Ok(output) => call_tool_success(&output),
            Err(err) => Ok(call_tool_error(&err)),
        }
    }

    #[tool(description = "Create a new empty system dynamics model file. \
            Produces a Simlin JSON file at the given path with a single \
            \"main\" model and the specified simulation specs.")]
    async fn create_model(
        &self,
        Parameters(input): Parameters<CreateModelInput>,
    ) -> Result<CallToolResult, McpError> {
        match create_model(&*self.access, input).await {
            Ok(output) => call_tool_success(&output),
            Err(err) => Ok(call_tool_error(&err)),
        }
    }
}

#[tool_handler]
impl<A: ProjectAccess> ServerHandler for SimlinMcpServer<A> {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::LATEST)
        .with_server_info(Implementation::new(
            "simlin-mcp",
            env!("CARGO_PKG_VERSION").to_string(),
        ))
        .with_instructions(self.instructions.as_str())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let resources: Vec<Resource> = self
            .resources
            .iter()
            .map(|r| {
                let raw = RawResource::new(r.uri.clone(), r.name.clone());
                let raw = if r.description.is_empty() {
                    raw
                } else {
                    raw.with_description(r.description.clone())
                };
                let raw = if r.mime_type.is_empty() {
                    raw
                } else {
                    raw.with_mime_type(r.mime_type.clone())
                };
                raw.no_annotation()
            })
            .collect();
        Ok(ListResourcesResult::with_all_items(resources))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri;
        if let Some(content) = self.resources.iter().find(|r| r.uri == uri) {
            return Ok(ReadResourceResult::new(vec![ResourceContents::text(
                content.body.clone(),
                uri,
            )]));
        }
        // ErrorCode::RESOURCE_NOT_FOUND is -32002 in rmcp, matching the
        // value pre-refactor @simlin/mcp clients see for missing resources.
        Err(McpError::new(
            ErrorCode::RESOURCE_NOT_FOUND,
            format!("resource not found: {uri}"),
            Some(serde_json::json!({ "uri": uri })),
        ))
    }
}

/// Build a `CallToolResult` for the happy path.  Both an unstructured
/// text representation and a structured-content JSON value are
/// emitted; the legacy `@simlin/mcp` clients consume the latter, while
/// rmcp's spec compliance keeps the former around for older clients.
fn call_tool_success<T: serde::Serialize>(output: &T) -> Result<CallToolResult, McpError> {
    let value =
        serde_json::to_value(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::structured(value))
}

/// Build a `CallToolResult` for the error path.  Validation failures
/// surface as a structured `errors` array so an LLM can inspect them
/// programmatically; everything else collapses to a string message.
fn call_tool_error(err: &AccessError) -> CallToolResult {
    match err {
        AccessError::Validation { errors } => {
            let value = serde_json::json!({
                "error": "edit introduces compilation errors",
                "errors": errors,
            });
            CallToolResult::structured_error(value)
        }
        other => CallToolResult::error(vec![Content::text(other.to_string())]),
    }
}
