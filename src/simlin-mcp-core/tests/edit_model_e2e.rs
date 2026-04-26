// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for `edit_model` against a filesystem-backed
//! `ProjectAccess` impl.  See `read_model_e2e.rs` for the rationale of
//! defining a test-local `FsAccess` rather than depending on Task 7's
//! `FileSystemAccess`.  These tests exercise the validation gate
//! (post-edit diagnostics surface as `AccessError::Validation`) and
//! the `.mdl` read-only rejection.

use std::io;
use std::path::Path;

use simlin_engine::datamodel;
use simlin_engine::json as ejson;
use simlin_mcp_core::access::{OpenedProject, ProjectAccess};
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::tools::edit_model::{
    EditModelInput, EditOperation, UpsertAuxiliaryInput, UpsertFlowInput, UpsertStockInput,
    edit_model,
};
use simlin_mcp_core::types::SourceFormat;

/// Test-local stateless filesystem impl that supports open + save +
/// create.  Mirrors what Task 7 will promote into the simlin-mcp
/// binary; `edit_model` exercises both `open` and `save` so both
/// methods need real implementations here.
struct FsAccess;

impl ProjectAccess for FsAccess {
    async fn open(&self, abs_path: &Path) -> Result<OpenedProject, AccessError> {
        let contents = tokio::fs::read_to_string(abs_path).await.map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
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
        abs_path: &Path,
        project: &datamodel::Project,
        format: SourceFormat,
        _expected_version: Option<u64>,
    ) -> Result<u64, AccessError> {
        let bytes = match format {
            SourceFormat::Xmile => simlin_engine::to_xmile(project)
                .map_err(|e| {
                    AccessError::ParseError(anyhow::anyhow!("failed to serialize XMILE: {e:?}"))
                })?
                .into_bytes(),
            SourceFormat::NativeJson => {
                let json_project = ejson::Project::from(project);
                serde_json::to_vec_pretty(&json_project)
                    .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize: {e}")))?
            }
            SourceFormat::SdaiJson => {
                let sdai_model = simlin_engine::json_sdai::SdaiModel::from(project);
                serde_json::to_vec_pretty(&sdai_model)
                    .map_err(|e| AccessError::ParseError(anyhow::anyhow!("serialize: {e}")))?
            }
        };
        simlin_engine::io::atomic_write(abs_path, &bytes).map_err(AccessError::WriteError)?;
        Ok(0)
    }

    async fn create(
        &self,
        _abs_path: &Path,
        _project: &datamodel::Project,
        _format: SourceFormat,
    ) -> Result<(), AccessError> {
        unreachable!("edit_model never calls create")
    }
}

fn minimal_project_json() -> serde_json::Value {
    serde_json::json!({
        "name": "test",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 100.0,
            "dt": "1",
            "saveStep": 1.0,
            "method": "euler",
            "timeUnits": ""
        },
        "models": [{ "name": "main" }]
    })
}

fn write_model(dir: &Path, filename: &str, content: &serde_json::Value) -> std::path::PathBuf {
    let path = dir.join(filename);
    std::fs::write(&path, serde_json::to_string_pretty(content).unwrap()).unwrap();
    path
}

#[tokio::test]
async fn upsert_stock_writes_back_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "model.sd.json", &minimal_project_json());

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertStock(UpsertStockInput {
            name: "population".into(),
            initial_equation: "1000".into(),
            units: None,
            documentation: None,
            inflows: None,
            outflows: None,
            arrayed_equation: None,
        })]),
    };

    let output = edit_model(&FsAccess, input).await.unwrap();
    assert!(!output.dry_run);

    // The file on disk must reflect the new stock.
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let stocks = saved["models"][0]["stocks"].as_array().unwrap();
    assert!(
        stocks.iter().any(|s| s["name"] == "population"),
        "saved file must contain the new stock: {stocks:?}"
    );
}

#[tokio::test]
async fn edit_with_compilation_error_surfaces_validation_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "broken.sd.json", &minimal_project_json());
    let original_contents = std::fs::read_to_string(&path).unwrap();

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
            name: "bad".into(),
            equation: "missing_dependency + 1".into(),
            units: None,
            documentation: None,
            graphical_function: None,
            arrayed_equation: None,
        })]),
    };

    let result = edit_model(&FsAccess, input).await;
    match result {
        Err(AccessError::Validation { errors }) => {
            assert!(!errors.is_empty(), "validation must include error details");
            assert!(errors.iter().any(|e| !e.code.is_empty()));
        }
        Err(other) => panic!("expected AccessError::Validation, got: {other:?}"),
        Ok(_) => panic!("expected AccessError::Validation, got Ok"),
    }

    // The file on disk must be unchanged.
    let after_contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        original_contents, after_contents,
        "file must not be modified when edit introduces compilation errors"
    );
}

#[tokio::test]
async fn mdl_files_are_rejected() {
    let mdl_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/sdeverywhere/models/elmcount/elmcount.mdl"
    );

    let input = EditModelInput {
        project_path: mdl_path.into(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
            name: "new_var".into(),
            equation: "1".into(),
            units: None,
            documentation: None,
            graphical_function: None,
            arrayed_equation: None,
        })]),
    };

    let result = edit_model(&FsAccess, input).await;
    let err_msg = match result {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected error rejecting .mdl file, got Ok"),
    };
    assert!(
        err_msg.contains(".mdl"),
        "error message must mention .mdl format: {err_msg}"
    );
}

#[tokio::test]
async fn dry_run_does_not_write_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "model.sd.json", &minimal_project_json());
    let original_contents = std::fs::read_to_string(&path).unwrap();

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: Some(true),
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertFlow(UpsertFlowInput {
            name: "births".into(),
            equation: "0".into(),
            units: None,
            documentation: None,
            graphical_function: None,
            arrayed_equation: None,
        })]),
    };

    let output = edit_model(&FsAccess, input).await.unwrap();
    assert!(output.dry_run);

    let after_contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        original_contents, after_contents,
        "dry_run must not modify the file on disk"
    );
}
