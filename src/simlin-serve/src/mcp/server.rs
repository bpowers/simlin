// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! rmcp `ServerHandler` mounted by `simlin-serve`.
//!
//! Generic over `A: ProjectAccess` so production code uses
//! [`RegistryAccess`] (sharing the in-memory `LoroDoc`/registry with the
//! HTTP/UI handlers) while tests can pass any `ProjectAccess` impl. The
//! `#[tool_router]` block below delegates the three reused tools
//! (`ReadModel` / `EditModel` / `CreateModel`) to `simlin-mcp-core`'s free
//! async functions; new tools added by simlin-serve (`ListProjects`,
//! `Simulate`) live in sibling modules and are wired in further along
//! Subcomponent B.

use std::path::PathBuf;
use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ErrorCode, Implementation, ListResourcesResult, PaginatedRequestParams,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResult, ServerCapabilities, ServerInfo,
};
use rmcp::service::RequestContext;
use rmcp::{RoleServer, ServerHandler, tool, tool_handler, tool_router};
use simlin_mcp_core::access::ProjectAccess;
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::tools::create_model::{CreateModelInput, create_model};
use simlin_mcp_core::tools::edit_model::{EditModelInput, edit_model};
use simlin_mcp_core::tools::read_model::{ReadModelInput, read_model};

use crate::handlers::AppState;
use crate::mcp::access::RegistryAccess;
use crate::mcp::list_projects::{ListProjectsInput, run as run_list_projects};
use crate::mcp::simulate::{SimulateInput, run as run_simulate};

/// rmcp `ServerHandler` impl exposing the simlin-serve tool surface.
///
/// Each rmcp session constructs a fresh `SimlinServeMcpServer` (rmcp's
/// streamable-HTTP factory closure pattern), but every instance shares
/// the same `Arc<AppState>` so tools always see the latest registry
/// state. `Arc<A>` for the access impl mirrors simlin-mcp-core's pattern
/// — rmcp's session machinery requires `Self: Clone`, and cloning an
/// `Arc` is cheaper than cloning an arbitrary `A` even when `A: Clone`.
#[derive(Clone)]
pub struct SimlinServeMcpServer<A: ProjectAccess> {
    /// Backing access impl — `RegistryAccess` in production, mockable in tests.
    access: Arc<A>,
    /// Process-wide state. Held alongside `access` so `list_projects` and
    /// `simulate` (added in Tasks 5/6) can consult the registry without
    /// going back through `RegistryAccess`.
    state: Arc<AppState>,
    /// Captured at construction so `get_info`'s instructions string is
    /// stable across the session. Kept as `Arc<PathBuf>` rather than
    /// re-borrowing `state.root` because rmcp's `Self: Clone` requires
    /// every field to be cloneable cheaply.
    root: Arc<PathBuf>,
    /// The router is consumed by the `tool_handler` macro expansion, not
    /// by direct method calls — silence the dead-code warning.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl SimlinServeMcpServer<RegistryAccess> {
    /// Production constructor used by the binary.
    pub fn new(state: Arc<AppState>) -> Self {
        let access = Arc::new(RegistryAccess::new(state.clone()));
        let root = Arc::clone(&state.root);
        Self {
            access,
            state,
            root,
            tool_router: Self::tool_router(),
        }
    }
}

impl<A: ProjectAccess> SimlinServeMcpServer<A> {
    /// Generic constructor for tests so they can pair the server with a
    /// mock `ProjectAccess`. Production code should use
    /// [`SimlinServeMcpServer::new`] which wires in [`RegistryAccess`].
    pub fn with_access(access: A, state: Arc<AppState>) -> Self {
        let root = Arc::clone(&state.root);
        Self {
            access: Arc::new(access),
            state,
            root,
            tool_router: Self::tool_router(),
        }
    }

    /// Borrow the underlying `AppState` so sibling tools (added in
    /// Tasks 5/6) can read registry/event state.
    #[allow(dead_code)]
    pub(crate) fn state(&self) -> &AppState {
        &self.state
    }

    /// Borrow the captured registry root for tool implementations that
    /// need to render a workspace-relative path.
    #[allow(dead_code)]
    pub(crate) fn root(&self) -> &PathBuf {
        &self.root
    }
}

#[tool_router]
impl<A: ProjectAccess> SimlinServeMcpServer<A> {
    #[tool(
        name = "ReadModel",
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

    #[tool(
        name = "EditModel",
        description = "Edit a system dynamics model by applying operations. \
            Supports upserting stocks, flows, and auxiliaries, removing variables, \
            and updating simulation specs. Returns a refreshed model snapshot \
            with loop dominance analysis after applying changes. \
            Upsert replaces the full variable definition; omitted optional fields \
            default to empty. Use ReadModel first to get current state, then \
            include all fields you want to preserve. \
            Note: Vensim .mdl files are read-only -- use ReadModel to inspect them, \
            then CreateModel to start a new .sd.json file you can edit."
    )]
    async fn edit_model(
        &self,
        Parameters(input): Parameters<EditModelInput>,
    ) -> Result<CallToolResult, McpError> {
        match edit_model(&*self.access, input).await {
            Ok(output) => call_tool_success(&output),
            Err(err) => Ok(call_tool_error(&err)),
        }
    }

    #[tool(
        name = "CreateModel",
        description = "Create a new empty system dynamics model file. \
            Produces a Simlin JSON file at the given path with a single \
            \"main\" model and the specified simulation specs."
    )]
    async fn create_model(
        &self,
        Parameters(input): Parameters<CreateModelInput>,
    ) -> Result<CallToolResult, McpError> {
        match create_model(&*self.access, input).await {
            Ok(output) => call_tool_success(&output),
            Err(err) => Ok(call_tool_error(&err)),
        }
    }

    #[tool(
        name = "ListProjects",
        description = "List all system-dynamics model files in the working \
            directory tree, with their format, version, and git status. \
            Returns the absolute working-directory root so the AI can build \
            paths relative to it for use with read_model / edit_model / \
            simulate."
    )]
    async fn list_projects(
        &self,
        Parameters(_input): Parameters<ListProjectsInput>,
    ) -> Result<CallToolResult, McpError> {
        let output = run_list_projects(&self.state);
        call_tool_success(&output)
    }

    #[tool(
        name = "Simulate",
        description = "Run a system-dynamics simulation and return the \
            time series for every (or selected) variable. \
            Supports optional `overrides` (the same EditOperation enum as \
            edit_model -- applied to a clone, not persisted) and \
            `simSpecsOverride` for exploring scenarios without touching \
            on-disk state. The `variables` whitelist keeps responses \
            small for large models."
    )]
    async fn simulate(
        &self,
        Parameters(input): Parameters<SimulateInput>,
    ) -> Result<CallToolResult, McpError> {
        match run_simulate(&*self.access, input).await {
            Ok(output) => call_tool_success(&output),
            Err(err) => Err(err),
        }
    }
}

#[tool_handler]
impl<A: ProjectAccess> ServerHandler for SimlinServeMcpServer<A> {
    fn get_info(&self) -> ServerInfo {
        // Per Phase 6 Note 1: `roots` is a *client* capability, not a
        // server one. The MCP-spec-conformant way to communicate the
        // working directory to LLM clients is via the `instructions`
        // field at initialize time — clients that surface this to the
        // LLM (Claude Code, Claude Desktop) will include it as context.
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_logging()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::LATEST)
        .with_server_info(Implementation::new(
            "simlin-serve",
            env!("CARGO_PKG_VERSION"),
        ))
        .with_instructions(format!(
            "Simlin model server. Operating on file://{}. Use list_projects to enumerate \
             available models, then read_model / edit_model / simulate to interact with them.",
            self.root.display()
        ))
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        // simlin-serve does not expose its own MCP resources for V1.
        Ok(ListResourcesResult::with_all_items(vec![]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        // No resources are exposed; every URI is unknown.
        Err(McpError::new(
            ErrorCode::RESOURCE_NOT_FOUND,
            format!("resource not found: {}", request.uri),
            Some(serde_json::json!({ "uri": request.uri })),
        ))
    }
}

/// Build a `CallToolResult` for the happy path. Mirrors the helper
/// simlin-mcp-core uses so the wire shape on success is identical
/// across stdio and HTTP transports.
fn call_tool_success<T: serde::Serialize>(output: &T) -> Result<CallToolResult, McpError> {
    let value =
        serde_json::to_value(output).map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(CallToolResult::structured(value))
}

/// Build a `CallToolResult` for the error path. Validation failures carry
/// the structured `errors` array so LLM clients can inspect each
/// engine-level diagnostic; everything else carries a plain `error`
/// string. Same contract as simlin-mcp-core's stdio impl.
fn call_tool_error(err: &AccessError) -> CallToolResult {
    match err {
        AccessError::Validation { errors } => {
            let value = serde_json::json!({
                "error": "edit introduces compilation errors",
                "errors": errors,
            });
            CallToolResult::structured_error(value)
        }
        other => {
            let value = serde_json::json!({ "error": other.to_string() });
            CallToolResult::structured_error(value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;
    use std::path::PathBuf;

    use simlin_engine::datamodel;
    use simlin_mcp_core::access::OpenedProject;
    use simlin_mcp_core::types::SourceFormat;
    use tempfile::TempDir;

    use crate::events::EventBus;
    use crate::git::GitProbe;
    use crate::registry::ProjectRegistry;

    /// Minimal `ProjectAccess` impl that always reports `NotFound`.
    /// Sufficient for `get_info` testing — the trait methods are never
    /// invoked.
    struct MockAccess;

    impl ProjectAccess for MockAccess {
        async fn open(&self, abs_path: &Path) -> Result<OpenedProject, AccessError> {
            Err(AccessError::NotFound {
                path: abs_path.to_path_buf(),
            })
        }
        async fn save(
            &self,
            _abs_path: &Path,
            _project: &datamodel::Project,
            _format: SourceFormat,
            _expected_version: Option<u64>,
        ) -> Result<u64, AccessError> {
            unreachable!("server unit tests do not invoke save")
        }
        async fn create(
            &self,
            _abs_path: &Path,
            _project: &datamodel::Project,
            _format: SourceFormat,
        ) -> Result<(), AccessError> {
            unreachable!("server unit tests do not invoke create")
        }
    }

    fn build_state(root: PathBuf) -> Arc<AppState> {
        Arc::new(AppState {
            registry: Arc::new(ProjectRegistry::new(root.clone())),
            git: Arc::new(GitProbe::unavailable_for_tests()),
            root: Arc::new(root),
            events: Arc::new(EventBus::new()),
            launch_token: Arc::new(String::new()),
        })
    }

    #[test]
    fn get_info_advertises_tools_resources_logging_capabilities() {
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let state = build_state(canonical_root);

        let server = SimlinServeMcpServer::with_access(MockAccess, state);
        let info = server.get_info();

        assert_eq!(info.protocol_version, ProtocolVersion::LATEST);
        assert!(
            info.capabilities.tools.is_some(),
            "tools capability must be advertised"
        );
        assert!(
            info.capabilities.resources.is_some(),
            "resources capability must be advertised"
        );
        assert!(
            info.capabilities.logging.is_some(),
            "logging capability must be advertised"
        );
        assert_eq!(info.server_info.name, "simlin-serve");
    }

    #[test]
    fn get_info_instructions_include_root_path() {
        let temp = TempDir::new().expect("tempdir");
        let canonical_root = temp.path().canonicalize().expect("canon root");
        let state = build_state(canonical_root.clone());

        let server = SimlinServeMcpServer::with_access(MockAccess, state);
        let info = server.get_info();

        let instructions = info
            .instructions
            .expect("instructions must be set so clients see the workspace dir");
        assert!(
            instructions.contains(&canonical_root.display().to_string()),
            "instructions must include workspace path: {instructions:?}"
        );
    }
}
