// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell

//! `Simulate` MCP tool — runs a simulation and returns time-series JSON.
//!
//! Simulations are stateless: every call constructs a fresh `SimlinDb`,
//! syncs the project, compiles, and runs to completion. The `overrides`
//! and `sim_specs_override` inputs let an AI explore different scenarios
//! without persisting any changes — overrides are applied to a cloned
//! project so the in-memory `LoroDoc` stays untouched.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use simlin_engine::Vm;
use simlin_engine::datamodel;
use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use simlin_engine::json as ejson;
use simlin_mcp_core::access::ProjectAccess;
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::tools::edit_model::{EditOperation, build_patch};

/// Failure modes for the `Simulate` tool.
///
/// `Access` mirrors `AccessError` so the rmcp wrapper can surface it via
/// the same `Ok(call_tool_error(&err))` path every other tool uses; that
/// preserves the wire-shape parity called out in the
/// "byte-identical tool wire shape" contract. `Engine` covers
/// simulation-pipeline failures (compile / VM / patch) which are not
/// `AccessError`s but still belong in a structured tool-level error
/// payload rather than a JSON-RPC protocol error.
#[derive(Debug)]
pub enum SimulateError {
    Access(AccessError),
    Engine(String),
}

impl From<AccessError> for SimulateError {
    fn from(err: AccessError) -> Self {
        SimulateError::Access(err)
    }
}

impl std::fmt::Display for SimulateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimulateError::Access(err) => write!(f, "{err}"),
            SimulateError::Engine(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for SimulateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SimulateError::Access(err) => Some(err),
            SimulateError::Engine(_) => None,
        }
    }
}

/// Input for the `Simulate` tool.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SimulateInput {
    /// Absolute or relative path to the model file (XMILE .stmx/.xmile,
    /// Vensim .mdl, or Simlin .sd.json).
    pub project_path: String,

    /// Name of the model within the project to simulate. Defaults to
    /// `"main"` when omitted.
    #[serde(default)]
    pub model_name: Option<String>,

    /// Optional list of operations applied to the project before running
    /// the simulation. Reuses `EditOperation` so the schema is identical
    /// across `edit_model` and `simulate`. Overrides are *not* persisted
    /// — the in-memory `LoroDoc` is unaffected.
    #[serde(default)]
    pub overrides: Option<Vec<EditOperation>>,

    /// Optional simulation-spec override. Applied after `overrides` so
    /// callers can test a model with both edits and different time
    /// bounds without affecting on-disk state.
    #[serde(default)]
    pub sim_specs_override: Option<ejson::SimSpecs>,

    /// Optional whitelist of variable names to include in the response.
    /// Useful for keeping responses small when simulating large models;
    /// when `None`, every variable is returned.
    #[serde(default)]
    pub variables: Option<Vec<String>>,
}

/// Output of the `Simulate` tool.
///
/// `time` is the time column extracted from the results buffer; every
/// entry in `variables` is a `Vec<f64>` of the same length. Using
/// `serde_json::Map` rather than a typed `BTreeMap<String, Vec<f64>>`
/// because the rmcp pipeline serialises through `serde_json::Value`
/// anyway — going via `Map` saves a round-trip through serde.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimulateOutput {
    pub time: Vec<f64>,
    pub variables: Map<String, Value>,
}

/// Run the simulation, returning either the structured output or a
/// `SimulateError` the rmcp wrapper translates into a structured
/// `CallToolResult`. Access-layer failures (NotFound, VersionMismatch)
/// flow through `?` from `access.open()` so the wrapper can surface them
/// via the shared `call_tool_error` helper; engine-pipeline failures
/// surface as `Engine(msg)`.
pub async fn run<A: ProjectAccess>(
    access: &A,
    input: SimulateInput,
) -> Result<SimulateOutput, SimulateError> {
    let path = Path::new(&input.project_path);
    let opened = access.open(path).await?;

    let mut project: datamodel::Project = opened.project;
    let model_name = input
        .model_name
        .clone()
        .unwrap_or_else(|| "main".to_string());

    if let Some(overrides) = input.overrides {
        let patch = build_patch(&model_name, None, Some(overrides));
        simlin_engine::apply_patch(&mut project, patch)
            .map_err(|e| SimulateError::Engine(format!("apply override patch: {e:?}")))?;
    }

    if let Some(ss_override) = input.sim_specs_override {
        project.sim_specs = ss_override.into();
    }

    // Build the canonical filter set before moving `project` into the
    // blocking task. The engine's `Results::offsets` keys come from
    // `simlin_engine::common::canonicalize`, so we must apply the same
    // transform to user-supplied variable names — plain `to_lowercase`
    // would miss whitespace-to-underscore substitution and other rules.
    let filter: Option<std::collections::HashSet<String>> = input.variables.map(|names| {
        names
            .into_iter()
            .map(|n| simlin_engine::canonicalize(&n).into_owned())
            .collect::<std::collections::HashSet<_>>()
    });

    // The compile/run/extract pipeline is CPU-bound and synchronous.
    // Running it directly on the tokio executor would block the thread
    // for the duration of the simulation, starving other async tasks on
    // the same worker. `spawn_blocking` moves the work onto tokio's
    // dedicated blocking-thread pool.
    //
    // Trade-off: `project` must be cloned into the closure because
    // `datamodel::Project` does not implement `Send + 'static` lazily —
    // the owned value is moved in. For typical SD models (< 1 MB of
    // project data) the clone cost is negligible compared to simulation
    // time.
    let output = tokio::task::spawn_blocking(move || simulate_sync(project, &model_name, filter))
        .await
        .map_err(|e| SimulateError::Engine(format!("blocking task panicked: {e}")))??;

    Ok(output)
}

/// Synchronous compile-run-extract pipeline, suitable for
/// `tokio::task::spawn_blocking`.
fn simulate_sync(
    project: datamodel::Project,
    model_name: &str,
    filter: Option<std::collections::HashSet<String>>,
) -> Result<SimulateOutput, SimulateError> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, model_name)
        .map_err(|e| SimulateError::Engine(format!("compile error: {e}")))?;
    let mut vm = Vm::new(compiled).map_err(|e| SimulateError::Engine(format!("vm error: {e}")))?;
    vm.run_to_end()
        .map_err(|e| SimulateError::Engine(format!("sim error: {e}")))?;
    let results = vm.into_results();

    // The engine stores the time column under the key "time" (offset 0).
    // We look it up through `offsets` rather than hard-coding index 0 so
    // this code stays correct if the engine ever reorders slots.
    // See src/simlin-engine/src/results.rs (TIME_OFF) for the engine side.
    let time_offset = results
        .offsets
        .iter()
        .find_map(|(name, off)| {
            if name.as_str() == "time" {
                Some(*off)
            } else {
                None
            }
        })
        .ok_or_else(|| SimulateError::Engine("simulation results missing 'time' column".into()))?;

    let mut time: Vec<f64> = Vec::with_capacity(results.step_count);
    for row in results.iter() {
        time.push(row[time_offset]);
    }

    let mut variables = Map::new();
    // Sort by name so the wire output is deterministic across runs —
    // helps tests and debugging without affecting correctness.
    let mut sorted_offsets: Vec<(
        &simlin_engine::common::Ident<simlin_engine::common::Canonical>,
        &usize,
    )> = results.offsets.iter().collect();
    sorted_offsets.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));

    for (name, offset) in sorted_offsets {
        let name_str = name.as_str();
        if let Some(filter) = filter.as_ref()
            && !filter.contains(name_str)
        {
            continue;
        }
        let mut col: Vec<f64> = Vec::with_capacity(results.step_count);
        for row in results.iter() {
            col.push(row[*offset]);
        }
        let json_array: Vec<Value> = col
            .into_iter()
            .map(|v| {
                serde_json::Number::from_f64(v)
                    .map(Value::Number)
                    // NaN/Inf are not representable in JSON; emit null
                    // so the response stays well-formed.
                    .unwrap_or(Value::Null)
            })
            .collect();
        variables.insert(name_str.to_string(), Value::Array(json_array));
    }

    Ok(SimulateOutput { time, variables })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::SystemTime;

    use tempfile::TempDir;

    use crate::events::EventBus;
    use crate::git::GitProbe;
    use crate::handlers::AppState;
    use crate::mcp::access::RegistryAccess;
    use crate::registry::{GitState, ProjectFormat, ProjectMeta, ProjectRegistry};

    fn build_state(root: PathBuf) -> Arc<AppState> {
        Arc::new(AppState {
            registry: Arc::new(ProjectRegistry::new(root.clone())),
            git: Arc::new(GitProbe::new_unavailable()),
            root: Arc::new(root),
            events: Arc::new(EventBus::new()),
            ui_port: 0,
            mcp_port: 0,
            strict_origin: true,
        })
    }

    fn seed_registry(state: &AppState, abs_path: &Path, format: ProjectFormat) {
        let metadata = std::fs::metadata(abs_path).expect("file exists");
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

    const FIXTURES_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    fn copy_fixture(name: &str, dest_dir: &Path) -> PathBuf {
        let src = PathBuf::from(FIXTURES_DIR).join(name);
        let dest = dest_dir.join(name);
        std::fs::copy(&src, &dest).expect("copy fixture");
        dest
    }

    #[tokio::test]
    async fn run_returns_time_and_variables_for_teacup() {
        let temp = TempDir::new().expect("tempdir");
        let canonical = temp.path().canonicalize().expect("canon");
        let abs = copy_fixture("teacup.xmile", &canonical);
        let state = build_state(canonical);
        seed_registry(&state, &abs, ProjectFormat::Xmile);

        let access = RegistryAccess::new(state);
        let input = SimulateInput {
            project_path: abs.display().to_string(),
            model_name: None,
            overrides: None,
            sim_specs_override: None,
            variables: None,
        };
        let out = run(&access, input).await.expect("simulate succeeds");
        assert!(out.time.len() > 1);
        assert!(out.variables.contains_key("teacup_temperature"));
    }

    #[tokio::test]
    async fn run_filters_variables_by_name() {
        let temp = TempDir::new().expect("tempdir");
        let canonical = temp.path().canonicalize().expect("canon");
        let abs = copy_fixture("teacup.xmile", &canonical);
        let state = build_state(canonical);
        seed_registry(&state, &abs, ProjectFormat::Xmile);

        let access = RegistryAccess::new(state);
        let input = SimulateInput {
            project_path: abs.display().to_string(),
            model_name: None,
            overrides: None,
            sim_specs_override: None,
            variables: Some(vec!["teacup_temperature".into()]),
        };
        let out = run(&access, input).await.expect("simulate succeeds");
        assert_eq!(out.variables.len(), 1);
        assert!(out.variables.contains_key("teacup_temperature"));
    }

    #[tokio::test]
    async fn run_surfaces_access_error_when_path_missing() {
        let temp = TempDir::new().expect("tempdir");
        let canonical = temp.path().canonicalize().expect("canon");
        let state = build_state(canonical);
        let access = RegistryAccess::new(state);

        let input = SimulateInput {
            project_path: "/does/not/exist.xmile".to_string(),
            model_name: None,
            overrides: None,
            sim_specs_override: None,
            variables: None,
        };
        // `run` returns the raw `AccessError` so the rmcp wrapper can
        // route it through `call_tool_error` for wire-shape parity with
        // the other tools. Engine-pipeline failures take the `Engine`
        // arm; access failures take `Access`.
        let err = run(&access, input).await.expect_err("missing path errors");
        match err {
            SimulateError::Access(AccessError::NotFound { .. }) => {}
            other => panic!("expected Access(NotFound), got {other:?}"),
        }
    }
}
