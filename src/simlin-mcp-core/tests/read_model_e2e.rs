// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for `read_model` against a filesystem-backed
//! `ProjectAccess` impl.  The test-local impl is intentionally minimal:
//! Task 7 promotes the canonical `FileSystemAccess` into the binary,
//! and this test will switch over then.  Until then, it lives here so
//! Task 4 can be verified without depending on Subcomponent C.

use std::path::Path;

use simlin_mcp_core::access::{OpenedProject, ProjectAccess};
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::tools::read_model::{ReadModelInput, read_model};
use simlin_mcp_core::types::SourceFormat;

/// Test-local stateless filesystem impl. Mirrors what Task 7 will
/// promote into the simlin-mcp binary.
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
        _abs_path: &Path,
        _project: &simlin_engine::datamodel::Project,
        _format: SourceFormat,
        _expected_version: Option<u64>,
    ) -> Result<u64, AccessError> {
        unreachable!("read_model never calls save")
    }

    async fn create(
        &self,
        _abs_path: &Path,
        _project: &simlin_engine::datamodel::Project,
        _format: SourceFormat,
    ) -> Result<(), AccessError> {
        unreachable!("read_model never calls create")
    }
}

#[tokio::test]
async fn read_model_returns_clean_xmile_snapshot() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/logistic_growth_ltm/logistic_growth.stmx"
    );
    let input = ReadModelInput {
        project_path: path.into(),
        model_name: None,
    };
    let output = read_model(&FsAccess, input).await.unwrap();
    assert!(
        output.errors.is_empty(),
        "clean fixture must produce no errors: {:?}",
        output.errors
    );
    assert!(
        !output.loop_dominance.is_empty(),
        "logistic growth fixture must have at least one loop"
    );
}

#[tokio::test]
async fn read_model_missing_file_returns_not_found() {
    let input = ReadModelInput {
        project_path: "/does/not/exist/model.sd.json".into(),
        model_name: None,
    };
    let result = read_model(&FsAccess, input).await;
    match result {
        Err(AccessError::NotFound { .. }) => {}
        Err(other) => panic!("expected AccessError::NotFound, got: {other:?}"),
        Ok(_) => panic!("expected AccessError::NotFound, got Ok"),
    }
}

#[tokio::test]
async fn read_model_native_json_format() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/logistic-growth.sd.json"
    );
    let input = ReadModelInput {
        project_path: path.into(),
        model_name: None,
    };
    let output = read_model(&FsAccess, input).await.unwrap();
    // The output must include a model snapshot regardless of source format.
    let value = serde_json::to_value(&output).unwrap();
    assert!(value["model"].is_object());
    assert!(value["time"].is_array());
}

#[tokio::test]
async fn read_model_broken_equations_surface_errors() {
    let broken = serde_json::json!({
        "name": "broken",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 10.0,
            "dt": "1",
            "method": "euler"
        },
        "models": [{
            "name": "main",
            "auxiliaries": [
                {"uid": 1, "name": "bad", "equation": "missing_var + 1"}
            ]
        }]
    });

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.sd.json");
    std::fs::write(&path, broken.to_string()).unwrap();

    let input = ReadModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
    };
    let output = read_model(&FsAccess, input).await.unwrap();
    assert!(
        !output.errors.is_empty(),
        "broken model must surface errors"
    );
    let value = serde_json::to_value(&output).unwrap();
    let errors = value["errors"].as_array().unwrap();
    assert!(errors[0]["code"].is_string());
    assert!(errors[0]["kind"].is_string());
}
