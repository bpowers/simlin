// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! Integration tests for `RegistryAccess` (the simlin-serve impl of
//! `simlin_mcp_core::ProjectAccess`).
//!
//! Browser-side and MCP-side write paths share the same `ProjectRegistry`
//! and `LoroDoc`, so a single test process can exercise both surfaces and
//! assert that they merge through the same primitive — covering AC4.1
//! (no-data-loss concurrent edits browser+MCP) and AC5.4 (identical end
//! states) end-to-end.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::Value;
use simlin_mcp_core::access::ProjectAccess;
use simlin_mcp_core::types::SourceFormat;
use simlin_serve::build_router;
use simlin_serve::events::{ChangeSource, EventBus, WsMessage};
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::mcp::RegistryAccess;
use simlin_serve::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};
use tempfile::TempDir;
use tower::ServiceExt;

const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn copy_fixture(name: &str, dest_dir: &std::path::Path) -> PathBuf {
    let src = PathBuf::from(FIXTURES_DIR).join(name);
    let dest = dest_dir.join(name);
    fs::copy(&src, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", src.display()));
    dest
}

// Synthetic ports for the host validator middleware (Phase 8 Task 8).
const TEST_UI_PORT: u16 = 12345;
const TEST_MCP_PORT: u16 = 12346;

fn build_state(root: PathBuf) -> Arc<AppState> {
    Arc::new(AppState {
        registry: Arc::new(ProjectRegistry::new(root.clone())),
        git: Arc::new(GitProbe::unavailable_for_tests()),
        root: Arc::new(root),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(String::new()),
        ui_port: TEST_UI_PORT,
        mcp_port: TEST_MCP_PORT,
        strict_origin: true,
    })
}

fn seed_registry(state: &AppState, abs_path: &std::path::Path, format: ProjectFormat) {
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

async fn fetch(state: AppState, method: &str, uri: &str, body: Body) -> (StatusCode, Vec<u8>) {
    let app = build_router(state);
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, format!("127.0.0.1:{TEST_UI_PORT}"))
                .header("content-type", "application/json")
                .body(body)
                .expect("request build"),
        )
        .await
        .expect("router response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 16 * 1024 * 1024)
        .await
        .expect("body bytes");
    (status, body.to_vec())
}

fn parse_body(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or_else(|e| {
        panic!(
            "response json: {e}; body: {}",
            String::from_utf8_lossy(body)
        )
    })
}

/// Helper: GET a project via the HTTP router, returning `(version, json)`
/// from the response.
async fn http_get_project(state: Arc<AppState>, uri: &str) -> (u64, String) {
    let (status, body) = fetch((*state).clone(), "GET", uri, Body::empty()).await;
    assert_eq!(status, StatusCode::OK, "GET {uri} failed");
    let value = parse_body(&body);
    (
        value["version"].as_u64().expect("version"),
        value["json"].as_str().expect("json").to_string(),
    )
}

/// Helper: clone a `datamodel::Project` and rename its first model's first
/// auxiliary equation, returning the modified project.
fn rewrite_first_aux_equation(
    project: &simlin_engine::datamodel::Project,
    new_eq: &str,
) -> simlin_engine::datamodel::Project {
    let mut clone = project.clone();
    let model = clone.models.first_mut().expect("at least one model");
    for var in &mut model.variables {
        if let simlin_engine::datamodel::Variable::Aux(aux) = var {
            aux.equation = simlin_engine::datamodel::Equation::Scalar(new_eq.to_string());
            return clone;
        }
    }
    panic!("fixture has no auxiliary variable to rewrite");
}

/// Helper: subscribe to the bus, then drain pending notifications until
/// `predicate` matches one or `tries` attempts elapse.  Returns the matched
/// message; panics on timeout.
async fn await_event<F>(
    rx: &mut tokio::sync::broadcast::Receiver<WsMessage>,
    predicate: F,
) -> WsMessage
where
    F: Fn(&WsMessage) -> bool,
{
    let timeout = tokio::time::Duration::from_secs(2);
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(tokio::time::Duration::from_millis(1));
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(msg)) if predicate(&msg) => return msg,
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => panic!("event bus error: {e}"),
            Err(_) => panic!("timed out waiting for matching event"),
        }
    }
}

// ---- AC5.4: MCP save flows through the same merge primitive as a browser save ----

#[tokio::test]
async fn mcp_save_increments_version_and_updates_doc() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = copy_fixture("teacup.xmile", &canonical_root);
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &abs, ProjectFormat::Xmile);

    let access = RegistryAccess::new(state.clone());

    let opened = access.open(&abs).await.expect("open");
    assert_eq!(opened.version, 0);
    let edited = rewrite_first_aux_equation(&opened.project, "999");

    let new_version = access
        .save(&abs, &edited, opened.source_format, Some(0))
        .await
        .expect("save");
    assert_eq!(new_version, 1);

    // Re-opening sees the new version and the new equation.
    let reopened = access.open(&abs).await.expect("re-open");
    assert_eq!(reopened.version, 1);
    let model = &reopened.project.models[0];
    let aux = model
        .variables
        .iter()
        .find_map(|v| match v {
            simlin_engine::datamodel::Variable::Aux(a) => Some(a),
            _ => None,
        })
        .expect("aux present");
    match &aux.equation {
        simlin_engine::datamodel::Equation::Scalar(eq) => {
            assert_eq!(eq, "999", "merged doc must reflect the MCP edit");
        }
        other => panic!("expected scalar equation, got {other:?}"),
    }
}

// ---- AC6.3: agent source on the broadcast bus ----

#[tokio::test]
async fn mcp_save_broadcasts_project_changed_with_agent_source() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = copy_fixture("teacup.xmile", &canonical_root);
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &abs, ProjectFormat::Xmile);

    let access = RegistryAccess::new(state.clone());
    let mut rx = state.events.subscribe();

    let opened = access.open(&abs).await.expect("open");
    let edited = rewrite_first_aux_equation(&opened.project, "42");
    let new_version = access
        .save(&abs, &edited, opened.source_format, Some(0))
        .await
        .expect("save");
    assert_eq!(new_version, 1);

    let event = await_event(&mut rx, |_msg| true).await;
    match event {
        WsMessage::ProjectChanged {
            path,
            version,
            source,
        } => {
            assert_eq!(version, 1);
            assert_eq!(source, ChangeSource::Agent);
            assert_eq!(path, "teacup.xmile");
        }
        other => panic!("expected ProjectChanged, got {other:?}"),
    }
}

// ---- AC4.1 + AC5.4: browser save and MCP save converge on the same Loro doc ----

#[tokio::test]
async fn browser_save_and_mcp_save_are_both_observable_in_the_loro_doc() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = copy_fixture("teacup.xmile", &canonical_root);
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &abs, ProjectFormat::Xmile);

    // 1) Browser save: rename the project to "via-browser".
    let (initial_version, initial_json) =
        http_get_project(state.clone(), "/api/projects/teacup.xmile").await;
    assert_eq!(initial_version, 0);

    let mut browser_value: Value = serde_json::from_str(&initial_json).expect("parse");
    browser_value["name"] = serde_json::json!("via-browser");
    let browser_payload = serde_json::json!({
        "json": serde_json::to_string(&browser_value).unwrap(),
        "version": initial_version,
    });
    let (status, body) = fetch(
        (*state).clone(),
        "POST",
        "/api/projects/teacup.xmile",
        Body::from(serde_json::to_vec(&browser_payload).unwrap()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "browser save failed: {}",
        String::from_utf8_lossy(&body)
    );
    let response = parse_body(&body);
    let after_browser_version = response["version"].as_u64().expect("version");
    assert_eq!(after_browser_version, 1);

    // 2) MCP save: rewrite the first auxiliary's equation. The MCP path
    //    fetches the latest version off the registry, so passing
    //    expected_version = None is correct here.
    let access = RegistryAccess::new(state.clone());
    let opened = access.open(&abs).await.expect("open after browser save");
    assert_eq!(opened.version, after_browser_version);
    assert_eq!(
        opened.project.name, "via-browser",
        "MCP open must observe the prior browser save"
    );
    let edited = rewrite_first_aux_equation(&opened.project, "1234");
    let mcp_version = access
        .save(&abs, &edited, opened.source_format, None)
        .await
        .expect("mcp save");
    assert_eq!(mcp_version, 2);

    // 3) Final state: both edits are present.
    let (final_version, final_json) =
        http_get_project(state.clone(), "/api/projects/teacup.xmile").await;
    assert_eq!(final_version, 2);
    let final_value: Value = serde_json::from_str(&final_json).expect("parse final");
    assert_eq!(
        final_value["name"].as_str(),
        Some("via-browser"),
        "browser-set name must survive the MCP save"
    );
    let model0 = &final_value["models"][0];
    let auxes = model0["auxiliaries"]
        .as_array()
        .expect("models[0].auxiliaries is an array");
    let mcp_aux = auxes
        .iter()
        .find(|v| v.get("equation").and_then(|e| e.as_str()) == Some("1234"))
        .expect("MCP-set equation must be present in the final canonical JSON");
    assert_eq!(mcp_aux["equation"].as_str(), Some("1234"));
}

// ---- AC5.4 (reverse order): MCP save first, then browser save ----

/// AC5.4 complement: performs the edits in the reverse order — MCP writes
/// first, browser writes second — and verifies both edits survive in the
/// final merged state. The original test exercises browser-then-MCP; this
/// one confirms the merge is symmetric (independent of operation order).
#[tokio::test]
async fn mcp_save_first_then_browser_save_both_observable_in_loro_doc() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = copy_fixture("teacup.xmile", &canonical_root);
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &abs, ProjectFormat::Xmile);

    // 1) MCP save first: rewrite the first aux equation to "5678".
    let access = RegistryAccess::new(state.clone());
    let opened = access.open(&abs).await.expect("open");
    assert_eq!(opened.version, 0);
    let mcp_edited = rewrite_first_aux_equation(&opened.project, "5678");
    let mcp_version = access
        .save(&abs, &mcp_edited, opened.source_format, Some(0))
        .await
        .expect("mcp save");
    assert_eq!(mcp_version, 1);

    // 2) Browser save: rename the project to "via-browser-second".
    let (after_mcp_version, after_mcp_json) =
        http_get_project(state.clone(), "/api/projects/teacup.xmile").await;
    assert_eq!(after_mcp_version, 1);

    let mut browser_value: Value = serde_json::from_str(&after_mcp_json).expect("parse");
    browser_value["name"] = serde_json::json!("via-browser-second");
    let browser_payload = serde_json::json!({
        "json": serde_json::to_string(&browser_value).unwrap(),
        "version": after_mcp_version,
    });
    let (status, body) = fetch(
        (*state).clone(),
        "POST",
        "/api/projects/teacup.xmile",
        Body::from(serde_json::to_vec(&browser_payload).unwrap()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "browser save failed: {}",
        String::from_utf8_lossy(&body)
    );
    let response = parse_body(&body);
    let after_browser_version = response["version"].as_u64().expect("version");
    assert_eq!(after_browser_version, 2);

    // 3) Final state: both edits are present.
    let (final_version, final_json) =
        http_get_project(state.clone(), "/api/projects/teacup.xmile").await;
    assert_eq!(final_version, 2);
    let final_value: Value = serde_json::from_str(&final_json).expect("parse final");
    assert_eq!(
        final_value["name"].as_str(),
        Some("via-browser-second"),
        "browser-set name must survive in the final state"
    );
    let model0 = &final_value["models"][0];
    let auxes = model0["auxiliaries"]
        .as_array()
        .expect("models[0].auxiliaries is an array");
    let mcp_aux = auxes
        .iter()
        .find(|v| v.get("equation").and_then(|e| e.as_str()) == Some("5678"))
        .expect("MCP-set equation must be present in the final canonical JSON");
    assert_eq!(mcp_aux["equation"].as_str(), Some("5678"));
}

// ---- Path-traversal defense: save and create ----

#[tokio::test]
async fn save_rejects_paths_outside_root() {
    let temp = TempDir::new().expect("tempdir");
    let outer = temp.path().canonicalize().expect("canon outer");
    let inner = outer.join("subroot");
    fs::create_dir(&inner).expect("create subroot");

    // Plant the escape file outside inner root so canonicalize() can
    // resolve the path (canonicalize_within_root checks after resolution).
    let outside = outer.join("escape.xmile");
    fs::copy(
        std::path::PathBuf::from(FIXTURES_DIR).join("teacup.xmile"),
        &outside,
    )
    .expect("seed escape file");

    let state = build_state(inner.clone());
    // Seed the outside file into the registry so the save path reaches
    // the traversal check (not the NotFound-from-registry path).
    seed_registry(&state, &outside, ProjectFormat::Xmile);

    let access = RegistryAccess::new(state);
    let opened_project = {
        let json_body = r#"{"name":"escape","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        let json_project: simlin_engine::json::Project =
            serde_json::from_str(json_body).expect("parse");
        simlin_engine::datamodel::Project::from(json_project)
    };

    // Attempt to save via a `..` traversal path that resolves outside inner.
    let attempted = inner.join("..").join("escape.xmile");
    match access
        .save(&attempted, &opened_project, SourceFormat::Xmile, None)
        .await
    {
        Err(simlin_mcp_core::errors::AccessError::NotFound { .. }) => {}
        Err(other) => panic!("expected NotFound for path outside root, got {other:?}"),
        Ok(_) => panic!("expected NotFound for path outside root, got Ok"),
    }
}

#[tokio::test]
async fn create_rejects_paths_with_dotdot_remainder() {
    let temp = TempDir::new().expect("tempdir");
    let outer = temp.path().canonicalize().expect("canon outer");
    let inner = outer.join("subroot");
    fs::create_dir(&inner).expect("create subroot");
    // `inner/sub/` does not exist, so the deepest existing ancestor is `inner`.
    // The remainder after stripping `inner` contains `..`, which must be rejected.
    let state = build_state(inner.clone());
    let access = RegistryAccess::new(state);

    let project = {
        let json_body = r#"{"name":"escape","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        let json_project: simlin_engine::json::Project =
            serde_json::from_str(json_body).expect("parse");
        simlin_engine::datamodel::Project::from(json_project)
    };

    // `inner/sub/../../escape.stmx` — sub/ doesn't exist; after walking up
    // to `inner`, the remainder is `sub/../../escape.stmx` which contains `..`.
    let attempted = inner.join("sub").join("..").join("..").join("escape.stmx");
    match access
        .create(&attempted, &project, SourceFormat::Xmile)
        .await
    {
        Err(simlin_mcp_core::errors::AccessError::NotFound { .. }) => {}
        Err(other) => panic!("expected NotFound for dotdot remainder, got {other:?}"),
        Ok(_) => panic!("expected NotFound for dotdot remainder, got Ok"),
    }
}

#[tokio::test]
async fn create_rejects_paths_resolving_outside_root() {
    let temp = TempDir::new().expect("tempdir");
    let outer = temp.path().canonicalize().expect("canon outer");
    let inner = outer.join("subroot");
    fs::create_dir(&inner).expect("create subroot");

    // The deepest existing ancestor will be `outer` (outside `inner`).
    // canonicalize(outer) won't start_with(canonicalize(inner)), so it must be rejected.
    let state = build_state(inner.clone());
    let access = RegistryAccess::new(state);

    let project = {
        let json_body = r#"{"name":"escape","simSpecs":{"startTime":0,"endTime":10,"dt":"1","method":"euler"},"models":[{"name":"main"}]}"#;
        let json_project: simlin_engine::json::Project =
            serde_json::from_str(json_body).expect("parse");
        simlin_engine::datamodel::Project::from(json_project)
    };

    // `inner/../escape.stmx` — the parent `outer` exists and canonicalizes,
    // but it is outside `inner`. The remainder after stripping `outer` would
    // be `escape.stmx` but the ancestor check fires first.
    let attempted = inner.join("..").join("escape.stmx");
    match access
        .create(&attempted, &project, SourceFormat::Xmile)
        .await
    {
        Err(simlin_mcp_core::errors::AccessError::NotFound { .. }) => {}
        Err(other) => panic!("expected NotFound for path outside root, got {other:?}"),
        Ok(_) => panic!("expected NotFound for path outside root, got Ok"),
    }
}

// ---- Task 3: RegistryAccess::create ----

/// Build a minimal valid native-JSON project for create tests. Uses the
/// engine's json::Project so the conversion path is exercised end-to-end.
fn minimal_project(name: &str) -> simlin_engine::datamodel::Project {
    let json_body = format!(
        r#"{{
            "name": "{name}",
            "simSpecs": {{"startTime": 0, "endTime": 10, "dt": "1", "method": "euler"}},
            "models": [{{"name": "main"}}]
        }}"#
    );
    let json_project: simlin_engine::json::Project =
        serde_json::from_str(&json_body).expect("test fixture parses");
    json_project.into()
}

#[tokio::test]
async fn create_writes_new_sd_json_and_registers_entry() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("brand-new.sd.json");
    let state = build_state(canonical_root.clone());

    let access = RegistryAccess::new(state.clone());
    let project = minimal_project("brand-new");
    access
        .create(&abs, &project, SourceFormat::NativeJson)
        .await
        .expect("create");

    assert!(abs.is_file(), "create must place the file on disk");
    let bytes = fs::read(&abs).expect("read created file");
    let parsed: simlin_engine::json::Project =
        serde_json::from_slice(&bytes).expect("created file parses as native JSON");
    assert_eq!(parsed.name, "brand-new");

    let meta = state
        .registry
        .get(&abs)
        .expect("registry must hold an entry for the created path");
    assert_eq!(meta.format, ProjectFormat::SdJson);
    assert_eq!(meta.version, 0);
    assert_eq!(meta.size, bytes.len() as u64);
}

#[tokio::test]
async fn create_writes_xmile_for_stmx_extension() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("nested").join("model.stmx");
    let state = build_state(canonical_root.clone());

    let access = RegistryAccess::new(state.clone());
    let project = minimal_project("xmile-creation");
    access
        .create(&abs, &project, SourceFormat::Xmile)
        .await
        .expect("create");

    assert!(abs.is_file(), "create must mkdir-p the parent directory");
    let bytes = fs::read(&abs).expect("read created file");
    let mut reader = std::io::Cursor::new(&bytes[..]);
    let parsed = simlin_engine::open_xmile(&mut reader).expect("created XMILE parses");
    assert_eq!(parsed.name, "xmile-creation");

    let meta = state.registry.get(&abs).expect("registry holds entry");
    assert_eq!(meta.format, ProjectFormat::Stmx);
    assert_eq!(meta.version, 0);
}

#[tokio::test]
async fn create_rejects_existing_file() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("preexisting.sd.json");
    fs::write(&abs, b"{}").expect("seed existing file");
    let state = build_state(canonical_root.clone());

    let access = RegistryAccess::new(state.clone());
    let project = minimal_project("collision");
    match access
        .create(&abs, &project, SourceFormat::NativeJson)
        .await
    {
        Err(simlin_mcp_core::errors::AccessError::IoError(io_err))
        | Err(simlin_mcp_core::errors::AccessError::WriteError(io_err)) => {
            assert_eq!(io_err.kind(), std::io::ErrorKind::AlreadyExists);
        }
        Err(other) => panic!("expected AlreadyExists, got {other:?}"),
        Ok(_) => panic!("expected AlreadyExists, got Ok"),
    }
}

#[tokio::test]
async fn create_broadcasts_project_changed_with_agent_source_and_version_zero() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let abs = canonical_root.join("announce.sd.json");
    let state = build_state(canonical_root.clone());

    let access = RegistryAccess::new(state.clone());
    let mut rx = state.events.subscribe();

    access
        .create(&abs, &minimal_project("announce"), SourceFormat::NativeJson)
        .await
        .expect("create");

    let event = await_event(&mut rx, |_| true).await;
    match event {
        WsMessage::ProjectChanged {
            path,
            version,
            source,
        } => {
            assert_eq!(version, 0, "newly-created entries are at version 0");
            assert_eq!(source, ChangeSource::Agent);
            assert_eq!(path, "announce.sd.json");
        }
        other => panic!("expected ProjectChanged, got {other:?}"),
    }
}

#[tokio::test]
async fn mcp_save_for_mdl_writes_sidecar_and_leaves_mdl_unchanged() {
    let temp = TempDir::new().expect("tempdir");
    let canonical_root = temp.path().canonicalize().expect("canon root");
    let mdl_abs = copy_fixture("teacup.mdl", &canonical_root);
    let original_mdl_bytes = fs::read(&mdl_abs).expect("read original mdl");
    let sidecar_abs = canonical_root.join("teacup.sd.json");
    let state = build_state(canonical_root.clone());
    seed_registry(&state, &mdl_abs, ProjectFormat::Mdl);

    let access = RegistryAccess::new(state.clone());
    let opened = access.open(&mdl_abs).await.expect("open mdl");
    // .mdl reads come back as XMILE-shaped projects.
    assert_eq!(opened.source_format, SourceFormat::Xmile);

    let edited = rewrite_first_aux_equation(&opened.project, "77");
    let new_version = access
        .save(&mdl_abs, &edited, opened.source_format, Some(0))
        .await
        .expect("mcp save mdl");
    assert_eq!(new_version, 1);

    // The original .mdl must be byte-untouched.
    let post_mdl_bytes = fs::read(&mdl_abs).expect("read post mdl");
    assert_eq!(
        post_mdl_bytes, original_mdl_bytes,
        ".mdl file must not be modified by an MCP-driven save"
    );
    // The sidecar must exist.
    assert!(
        sidecar_abs.is_file(),
        "sidecar .sd.json must be created on save"
    );
    // The registry must now point at the sidecar, not the .mdl.
    assert!(
        state.registry.get(&mdl_abs).is_none(),
        "registry must drop the .mdl entry"
    );
    let sidecar_meta = state
        .registry
        .get(&sidecar_abs)
        .expect("registry must hold sidecar entry");
    assert_eq!(sidecar_meta.format, ProjectFormat::SdJson);
    assert_eq!(sidecar_meta.version, 1);
}
