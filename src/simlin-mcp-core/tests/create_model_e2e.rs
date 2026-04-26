// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for `create_model` against a filesystem-backed
//! `ProjectAccess` impl.  See `read_model_e2e.rs` for the rationale of
//! defining a test-local `FsAccess` rather than depending on Task 7's
//! `FileSystemAccess`.

use std::io;
use std::path::Path;

use simlin_engine::datamodel;
use simlin_mcp_core::access::{OpenedProject, ProjectAccess};
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::tools::create_model::{CreateModelInput, create_model};
use simlin_mcp_core::types::SourceFormat;

/// Test-local stateless filesystem impl. Mirrors what Task 7 will
/// promote into the simlin-mcp binary; only `create` is exercised here.
struct FsAccess;

impl ProjectAccess for FsAccess {
    async fn open(&self, _abs_path: &Path) -> Result<OpenedProject, AccessError> {
        unreachable!("create_model never calls open")
    }

    async fn save(
        &self,
        _abs_path: &Path,
        _project: &datamodel::Project,
        _format: SourceFormat,
        _expected_version: Option<u64>,
    ) -> Result<u64, AccessError> {
        unreachable!("create_model never calls save")
    }

    async fn create(
        &self,
        abs_path: &Path,
        project: &datamodel::Project,
        _format: SourceFormat,
    ) -> Result<(), AccessError> {
        if abs_path.exists() {
            return Err(AccessError::WriteError(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("file already exists: {}", abs_path.display()),
            )));
        }
        if let Some(parent) = abs_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(AccessError::WriteError)?;
        }
        let json_project = simlin_engine::json::Project::from(project);
        let bytes = serde_json::to_vec_pretty(&json_project)
            .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize failed: {e}")))?;
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(())
    }
}

#[tokio::test]
async fn create_model_with_default_specs_writes_parseable_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new-model.sd.json");

    let input = CreateModelInput {
        project_path: path.to_str().unwrap().to_string(),
        sim_specs: None,
    };

    let output = create_model(&FsAccess, input).await.unwrap();
    assert_eq!(output.model_name, "main");
    assert!(path.exists(), "create must write the file");

    // The written file must be parseable native Simlin JSON.
    let contents = std::fs::read_to_string(&path).unwrap();
    let project: simlin_engine::json::Project = serde_json::from_str(&contents).unwrap();
    assert_eq!(project.name, "new-model");
    assert_eq!(project.models.len(), 1);
    assert_eq!(project.models[0].name, "main");
    assert_eq!(project.sim_specs.start_time, 0.0);
    assert_eq!(project.sim_specs.end_time, 100.0);
}

#[tokio::test]
async fn create_model_with_custom_sim_specs_persists_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("custom.sd.json");

    let input = CreateModelInput {
        project_path: path.to_str().unwrap().to_string(),
        sim_specs: Some(simlin_engine::json::SimSpecs {
            start_time: 10.0,
            end_time: 200.0,
            dt: "0.5".to_string(),
            save_step: 1.0,
            method: "euler".to_string(),
            time_units: String::new(),
        }),
    };

    let output = create_model(&FsAccess, input).await.unwrap();
    assert_eq!(output.sim_specs.start_time, 10.0);
    assert_eq!(output.sim_specs.end_time, 200.0);
    assert_eq!(output.sim_specs.dt, "0.5");

    let contents = std::fs::read_to_string(&path).unwrap();
    let project: simlin_engine::json::Project = serde_json::from_str(&contents).unwrap();
    assert_eq!(project.sim_specs.start_time, 10.0);
    assert_eq!(project.sim_specs.end_time, 200.0);
}

#[tokio::test]
async fn create_model_refuses_to_overwrite_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("existing.sd.json");
    std::fs::write(&path, "{}").unwrap();

    let input = CreateModelInput {
        project_path: path.to_str().unwrap().to_string(),
        sim_specs: None,
    };

    let result = create_model(&FsAccess, input).await;
    match result {
        Err(AccessError::WriteError(e)) => {
            assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        }
        Err(other) => panic!("expected WriteError, got: {other:?}"),
        Ok(_) => panic!("expected WriteError, got Ok"),
    }
}
