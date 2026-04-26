// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! End-to-end tests for the rmcp tool surface exposed by `SimlinServeMcpServer`.
//!
//! Spawns the server against an in-memory duplex pair (same pattern as
//! simlin-mcp-core's `tool_dispatch.rs`) and uses an rmcp client to issue
//! real `tools/call` requests so the macro-generated dispatch is
//! exercised end-to-end.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::mcp::{RegistryAccess, SimlinServeMcpServer};
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use tempfile::TempDir;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn copy_fixture(name: &str, dest_dir: &Path) -> PathBuf {
    let src = PathBuf::from(FIXTURES_DIR).join(name);
    let dest = dest_dir.join(name);
    fs::copy(&src, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
    dest
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

fn seed_registry(state: &AppState, abs_path: &Path, format: ProjectFormat) {
    let metadata = fs::metadata(abs_path).expect("file exists");
    let mtime = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    state.registry.upsert(
        abs_path.to_path_buf(),
        ProjectMeta {
            path: PathBuf::new(),
            format,
            mtime,
            size: metadata.len(),
            git: GitState::Untracked,
            version: 0,
            doc: Default::default(),
            last_disk_hash: 0,
        },
    );
}

async fn spawn_server_pair(
    state: Arc<AppState>,
) -> (
    rmcp::service::RunningService<rmcp::RoleClient, ()>,
    rmcp::service::RunningService<rmcp::RoleServer, SimlinServeMcpServer<RegistryAccess>>,
) {
    let (server_io, client_io) = tokio::io::duplex(65536);
    let server = SimlinServeMcpServer::<RegistryAccess>::new(state);
    let server_task = tokio::spawn(async move { server.serve(server_io).await });
    let client = ().serve(client_io).await.expect("client failed to initialize");
    let server = server_task
        .await
        .expect("server task panicked")
        .expect("server failed to initialize");
    (client, server)
}

#[tokio::test]
async fn tools_list_advertises_pascal_case_names() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root);

    let (client, server) = spawn_server_pair(state).await;

    let result = client
        .peer()
        .list_tools(None)
        .await
        .expect("tools/list must succeed");

    let names: Vec<&str> = result.tools.iter().map(|t| t.name.as_ref()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    // Subcomponent B's full surface is the three delegated tools plus
    // ListProjects and Simulate (added in Tasks 5/6); for Task 4 we
    // just assert the three delegated names are present.
    for required in ["CreateModel", "EditModel", "ReadModel"] {
        assert!(
            sorted.contains(&required),
            "tools/list must advertise {required}; got: {names:?}"
        );
    }

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn read_model_delegates_to_simlin_mcp_core() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = copy_fixture("teacup.xmile", &canonical_root);
    let state = build_state(canonical_root);
    seed_registry(&state, &abs, ProjectFormat::Xmile);

    let (client, server) = spawn_server_pair(state).await;

    let arguments = serde_json::json!({
        "projectPath": abs.to_str().unwrap(),
    });
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

    assert_ne!(
        result.is_error,
        Some(true),
        "successful read_model must not set is_error: true"
    );
    let structured = result
        .structured_content
        .expect("read_model success must include structured content");
    assert!(
        structured.get("model").is_some(),
        "structured content must include a model snapshot: {structured}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn get_info_includes_workspace_dir_in_instructions() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root.clone());

    let server = SimlinServeMcpServer::<RegistryAccess>::new(state);
    use rmcp::ServerHandler;
    let info = server.get_info();

    assert!(
        info.instructions.is_some(),
        "instructions must be set so AI clients see the workspace dir"
    );
    let instructions = info.instructions.unwrap();
    let display = canonical_root.display().to_string();
    assert!(
        instructions.contains(&display),
        "instructions must contain the workspace dir; got: {instructions:?}"
    );
}
