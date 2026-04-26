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
use std::time::{Duration, SystemTime};

use rmcp::model::{CallToolRequestParams, CustomNotification};
use rmcp::service::NotificationContext;
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use simlin_serve::events::{ChangeSource, EventBus, ValidationError, WsMessage};
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::mcp::{RegistryAccess, SimlinServeMcpServer};
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use tempfile::TempDir;
use tokio::sync::Mutex;

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
            last_diagnostic_keys: std::collections::BTreeSet::new(),
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
async fn list_projects_returns_registry_snapshot() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs_xmile = copy_fixture("teacup.xmile", &canonical_root);
    let abs_stmx = copy_fixture("teacup.stmx", &canonical_root);
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &abs_xmile, ProjectFormat::Xmile);
    seed_registry(&state, &abs_stmx, ProjectFormat::Stmx);

    let (client, server) = spawn_server_pair(state).await;

    let mut params = CallToolRequestParams::new("ListProjects");
    params = params.with_arguments(serde_json::Map::new());
    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("ListProjects call_tool succeeds");

    assert_ne!(
        result.is_error,
        Some(true),
        "successful ListProjects must not mark is_error"
    );
    let structured = result
        .structured_content
        .expect("ListProjects must return structured content");

    let projects = structured
        .get("projects")
        .and_then(|v| v.as_array())
        .expect("projects field must be an array");
    assert_eq!(projects.len(), 2, "expected two seeded entries");

    let names: Vec<&str> = projects
        .iter()
        .map(|p| p.get("path").and_then(|v| v.as_str()).expect("path"))
        .collect();
    assert!(names.iter().any(|n| n.ends_with("teacup.xmile")));
    assert!(names.iter().any(|n| n.ends_with("teacup.stmx")));

    for entry in projects {
        assert!(
            entry.get("format").and_then(|v| v.as_str()).is_some(),
            "format must be a string: {entry}"
        );
        assert!(
            entry.get("git").is_some(),
            "git state must be present: {entry}"
        );
        assert_eq!(
            entry.get("version").and_then(|v| v.as_u64()),
            Some(0),
            "freshly seeded entries report version 0: {entry}"
        );
        assert!(
            entry.get("mtime").is_none(),
            "mtime should not appear on the AI surface: {entry}"
        );
        assert!(
            entry.get("size").is_none(),
            "size should not appear on the AI surface: {entry}"
        );
    }

    assert_eq!(
        structured.get("gitAvailable").and_then(|v| v.as_bool()),
        Some(false),
        "test GitProbe is unavailable_for_tests"
    );
    let root = structured
        .get("root")
        .and_then(|v| v.as_str())
        .expect("root field must be a string");
    assert_eq!(root, canonical_root.display().to_string());

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn list_projects_advertised_in_tools_list() {
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
    assert!(
        names.contains(&"ListProjects"),
        "tools/list must advertise ListProjects; got: {names:?}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn simulate_returns_time_series_for_teacup_fixture() {
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
    let mut params = CallToolRequestParams::new("Simulate");
    if let Some(args) = arguments_obj {
        params = params.with_arguments(args);
    }

    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("Simulate call_tool succeeds");
    assert_ne!(
        result.is_error,
        Some(true),
        "successful Simulate must not set is_error: response: {:?}",
        result.structured_content
    );
    let structured = result
        .structured_content
        .expect("Simulate must return structured content");

    let time = structured
        .get("time")
        .and_then(|v| v.as_array())
        .expect("time field must be an array");
    assert!(time.len() > 1, "expected multiple time steps");
    assert_eq!(
        time.first().and_then(|v| v.as_f64()),
        Some(0.0),
        "time series starts at start_time = 0.0"
    );

    let variables = structured
        .get("variables")
        .and_then(|v| v.as_object())
        .expect("variables field must be a map");
    let teacup_series = variables
        .get("teacup_temperature")
        .and_then(|v| v.as_array())
        .expect("teacup_temperature column must be present");
    assert_eq!(
        teacup_series.len(),
        time.len(),
        "every variable column has the same length as time"
    );
    let initial = teacup_series.first().and_then(|v| v.as_f64()).unwrap();
    let final_val = teacup_series.last().and_then(|v| v.as_f64()).unwrap();
    assert!(
        initial > final_val,
        "teacup cools toward room temperature: initial={initial}, final={final_val}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn simulate_filters_variables_when_requested() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = copy_fixture("teacup.xmile", &canonical_root);
    let state = build_state(canonical_root);
    seed_registry(&state, &abs, ProjectFormat::Xmile);

    let (client, server) = spawn_server_pair(state).await;

    let arguments = serde_json::json!({
        "projectPath": abs.to_str().unwrap(),
        "variables": ["teacup_temperature"],
    });
    let arguments_obj = match arguments {
        serde_json::Value::Object(map) => Some(map),
        _ => unreachable!("arguments is constructed as an object literal"),
    };
    let mut params = CallToolRequestParams::new("Simulate");
    if let Some(args) = arguments_obj {
        params = params.with_arguments(args);
    }

    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("Simulate call succeeds");
    let structured = result.structured_content.expect("structured content");
    let variables = structured
        .get("variables")
        .and_then(|v| v.as_object())
        .expect("variables map");
    assert_eq!(variables.len(), 1, "filter narrows the response to one var");
    assert!(variables.contains_key("teacup_temperature"));

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn simulate_advertised_in_tools_list() {
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
    assert!(
        names.contains(&"Simulate"),
        "tools/list must advertise Simulate; got: {names:?}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn simulate_overrides_change_initial_stock_value() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = copy_fixture("teacup.xmile", &canonical_root);
    let state = build_state(canonical_root);
    seed_registry(&state, &abs, ProjectFormat::Xmile);

    let (client, server) = spawn_server_pair(state).await;

    // Override the teacup stock with a wildly different initial. The
    // overridden series must differ from the baseline at t=0.
    let arguments = serde_json::json!({
        "projectPath": abs.to_str().unwrap(),
        "overrides": [{
            "upsertStock": {
                "name": "Teacup Temperature",
                "initialEquation": "10",
                "outflows": ["Heat Loss to Room"],
            }
        }],
        "variables": ["teacup_temperature"],
    });
    let arguments_obj = match arguments {
        serde_json::Value::Object(map) => Some(map),
        _ => unreachable!(),
    };
    let mut params = CallToolRequestParams::new("Simulate");
    if let Some(args) = arguments_obj {
        params = params.with_arguments(args);
    }
    let result = client
        .peer()
        .call_tool(params)
        .await
        .expect("Simulate call succeeds");
    assert_ne!(
        result.is_error,
        Some(true),
        "override must not error: {:?}",
        result.structured_content
    );
    let structured = result.structured_content.expect("structured content");
    let teacup_series = structured["variables"]["teacup_temperature"]
        .as_array()
        .expect("teacup series");
    let initial = teacup_series[0].as_f64().expect("initial");
    assert!(
        (initial - 10.0).abs() < 1e-9,
        "overridden initial must be 10.0, got {initial}"
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

/// One captured custom notification: (method, params).
type CapturedEvent = (String, Option<serde_json::Value>);

/// Test client that records every custom notification received from the
/// server. Each new notification appends to `events`; tests poll the
/// vector with a short timeout rather than wiring per-event signaling
/// because some tests publish multiple events back-to-back.
#[derive(Clone, Default)]
struct NotificationCapture {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl ClientHandler for NotificationCapture {
    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) {
        let CustomNotification { method, params, .. } = notification;
        self.events.lock().await.push((method, params));
    }
}

async fn spawn_server_pair_with_capture(
    state: Arc<AppState>,
) -> (
    rmcp::service::RunningService<RoleClient, NotificationCapture>,
    rmcp::service::RunningService<rmcp::RoleServer, SimlinServeMcpServer<RegistryAccess>>,
    NotificationCapture,
) {
    let (server_io, client_io) = tokio::io::duplex(65536);
    let server = SimlinServeMcpServer::<RegistryAccess>::new(state);
    let server_task = tokio::spawn(async move { server.serve(server_io).await });
    let capture = NotificationCapture::default();
    let client = capture
        .clone()
        .serve(client_io)
        .await
        .expect("client failed to initialize");
    let server = server_task
        .await
        .expect("server task panicked")
        .expect("server failed to initialize");
    (client, server, capture)
}

/// Wait until `predicate` returns true on the captured events, polling
/// every 25ms up to `timeout`. Returns the captured snapshot at the time
/// the predicate first held, or panics if the predicate never held.
async fn wait_for_events<F>(capture: &NotificationCapture, timeout: Duration, mut predicate: F)
where
    F: FnMut(&[CapturedEvent]) -> bool,
{
    let deadline = std::time::Instant::now() + timeout;
    loop {
        {
            let events = capture.events.lock().await;
            if predicate(&events) {
                return;
            }
        }
        if std::time::Instant::now() >= deadline {
            let events = capture.events.lock().await;
            panic!(
                "predicate did not hold within {:?}; captured: {:#?}",
                timeout, *events
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn project_changed_event_arrives_as_simlin_project_changed_notification() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root);

    let (client, server, capture) = spawn_server_pair_with_capture(state.clone()).await;

    state.events.publish(WsMessage::ProjectChanged {
        path: "models/teacup.xmile".into(),
        version: 3,
        source: ChangeSource::User,
    });

    wait_for_events(&capture, Duration::from_secs(2), |events| {
        events
            .iter()
            .any(|(method, _)| method == "simlin/projectChanged")
    })
    .await;

    let events = capture.events.lock().await;
    let entry = events
        .iter()
        .find(|(method, _)| method == "simlin/projectChanged")
        .expect("projectChanged notification recorded");
    let params = entry.1.as_ref().expect("params present");
    assert_eq!(params["path"].as_str(), Some("models/teacup.xmile"));
    assert_eq!(params["version"].as_u64(), Some(3));
    assert_eq!(params["source"].as_str(), Some("user"));
    drop(events);

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn project_focused_event_arrives_as_simlin_project_focused_notification() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root);

    let (client, server, capture) = spawn_server_pair_with_capture(state.clone()).await;

    state.events.publish(WsMessage::ProjectFocused {
        path: "a.stmx".into(),
    });

    wait_for_events(&capture, Duration::from_secs(2), |events| {
        events
            .iter()
            .any(|(method, _)| method == "simlin/projectFocused")
    })
    .await;

    let events = capture.events.lock().await;
    let entry = events
        .iter()
        .find(|(method, _)| method == "simlin/projectFocused")
        .expect("projectFocused notification recorded");
    let params = entry.1.as_ref().expect("params present");
    assert_eq!(params["path"].as_str(), Some("a.stmx"));
    drop(events);

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn selection_changed_event_arrives_with_camel_case_idents() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root);

    let (client, server, capture) = spawn_server_pair_with_capture(state.clone()).await;

    state.events.publish(WsMessage::SelectionChanged {
        path: "x.stmx".into(),
        variable_idents: vec!["alpha".into(), "beta".into()],
    });

    wait_for_events(&capture, Duration::from_secs(2), |events| {
        events
            .iter()
            .any(|(method, _)| method == "simlin/selectionChanged")
    })
    .await;

    let events = capture.events.lock().await;
    let entry = events
        .iter()
        .find(|(method, _)| method == "simlin/selectionChanged")
        .expect("selectionChanged notification recorded");
    let params = entry.1.as_ref().expect("params present");
    assert_eq!(params["path"].as_str(), Some("x.stmx"));
    let idents = params["variableIdents"]
        .as_array()
        .expect("variableIdents is an array");
    assert_eq!(idents.len(), 2);
    assert_eq!(idents[0].as_str(), Some("alpha"));
    assert_eq!(idents[1].as_str(), Some("beta"));
    drop(events);

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn diagnostics_changed_event_arrives_with_error_list() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root);

    let (client, server, capture) = spawn_server_pair_with_capture(state.clone()).await;

    state.events.publish(WsMessage::DiagnosticsChanged {
        path: "models/teacup.xmile".into(),
        errors: vec![ValidationError {
            code: "syntax".into(),
            message: "bad equation".into(),
            model_name: Some("main".into()),
            variable_name: Some("y".into()),
            kind: "variable".into(),
        }],
    });

    wait_for_events(&capture, Duration::from_secs(2), |events| {
        events
            .iter()
            .any(|(method, _)| method == "simlin/diagnosticsChanged")
    })
    .await;

    let events = capture.events.lock().await;
    let entry = events
        .iter()
        .find(|(method, _)| method == "simlin/diagnosticsChanged")
        .expect("diagnosticsChanged notification recorded");
    let params = entry.1.as_ref().expect("params present");
    assert_eq!(params["path"].as_str(), Some("models/teacup.xmile"));
    let errors = params["errors"].as_array().expect("errors is an array");
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["code"].as_str(), Some("syntax"));
    assert_eq!(errors[0]["modelName"].as_str(), Some("main"));
    drop(events);

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn project_removed_event_arrives_as_simlin_project_removed_notification() {
    // Phase 4's ProjectRemoved variant flows through the same forwarder
    // path. Asserts the wire_pair coverage stays exhaustive across all
    // five WsMessage variants.
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root);

    let (client, server, capture) = spawn_server_pair_with_capture(state.clone()).await;

    state.events.publish(WsMessage::ProjectRemoved {
        path: "deleted.stmx".into(),
    });

    wait_for_events(&capture, Duration::from_secs(2), |events| {
        events
            .iter()
            .any(|(method, _)| method == "simlin/projectRemoved")
    })
    .await;

    let events = capture.events.lock().await;
    let entry = events
        .iter()
        .find(|(method, _)| method == "simlin/projectRemoved")
        .expect("projectRemoved notification recorded");
    let params = entry.1.as_ref().expect("params present");
    assert_eq!(params["path"].as_str(), Some("deleted.stmx"));
    drop(events);

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}

#[tokio::test]
async fn forwarder_subscribes_during_initialize_and_unsubscribes_on_disconnect() {
    // The per-session pattern's contract: each connected MCP session
    // owns exactly one broadcast receiver, which Drop-cleans when the
    // forwarder task exits. After the client disconnects and the
    // session ends, receiver_count() must drop back to its baseline.
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root);

    let baseline = state.events.receiver_count();

    let (client, server, _capture) = spawn_server_pair_with_capture(state.clone()).await;

    // After initialize, the forwarder task holds one receiver. Other
    // session-internal subscribers are not expected, so the increment
    // is exactly 1 — but allow a wider check so unrelated future
    // additions don't make the test brittle.
    let connected = state.events.receiver_count();
    assert!(
        connected > baseline,
        "forwarder must subscribe during initialize: baseline={baseline}, connected={connected}"
    );

    let _ = client.cancel().await;
    let _ = server.cancel().await;

    // The forwarder loop wakes only on the next recv() or on session
    // shutdown; nudge it with a publish so it observes TransportClosed
    // (or RecvError::Closed) and exits, then poll until the receiver
    // count drops. Without this nudge a clean cancel still works but
    // can take longer to clear.
    state.events.publish(WsMessage::ProjectFocused {
        path: "shutdown-nudge".into(),
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let count = state.events.receiver_count();
        if count <= baseline {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "receiver count did not return to baseline within 2s: \
                 baseline={baseline}, current={count}"
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn multiple_events_arrive_in_order_to_a_single_session() {
    // Sanity check that ordering is preserved per-session: a sequence
    // of three publishes lands at the client in the same order. (This
    // is *not* the cross-tool-response ordering covered by Phase 7
    // Note 3 — that one is intentionally not guaranteed.)
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let state = build_state(canonical_root);

    let (client, server, capture) = spawn_server_pair_with_capture(state.clone()).await;

    state.events.publish(WsMessage::ProjectFocused {
        path: "first".into(),
    });
    state.events.publish(WsMessage::ProjectChanged {
        path: "first".into(),
        version: 1,
        source: ChangeSource::Agent,
    });
    state.events.publish(WsMessage::ProjectFocused {
        path: "second".into(),
    });

    wait_for_events(&capture, Duration::from_secs(2), |events| events.len() >= 3).await;

    let events = capture.events.lock().await;
    let methods: Vec<&str> = events.iter().map(|(m, _)| m.as_str()).collect();
    let first_focused = methods
        .iter()
        .position(|m| *m == "simlin/projectFocused")
        .expect("first projectFocused");
    let project_changed = methods
        .iter()
        .position(|m| *m == "simlin/projectChanged")
        .expect("projectChanged");
    let second_focused = methods
        .iter()
        .rposition(|m| *m == "simlin/projectFocused")
        .expect("second projectFocused");
    assert!(
        first_focused < project_changed && project_changed < second_focused,
        "events out of order: {methods:?}"
    );
    drop(events);

    let _ = client.cancel().await;
    let _ = server.cancel().await;
}
