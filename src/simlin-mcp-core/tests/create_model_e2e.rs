// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for `create_model` against a filesystem-backed
//! `ProjectAccess` impl.

use std::io;

use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::test_support::TestFileSystemAccess;
use simlin_mcp_core::tools::create_model::{CreateModelInput, create_model};

#[tokio::test]
async fn create_model_with_default_specs_writes_parseable_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new-model.sd.json");

    let input = CreateModelInput {
        project_path: path.to_str().unwrap().to_string(),
        sim_specs: None,
    };

    let output = create_model(&TestFileSystemAccess, input).await.unwrap();
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

    let output = create_model(&TestFileSystemAccess, input).await.unwrap();
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

    let result = create_model(&TestFileSystemAccess, input).await;
    match result {
        Err(AccessError::WriteError(e)) => {
            assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        }
        Err(other) => panic!("expected WriteError, got: {other:?}"),
        Ok(_) => panic!("expected WriteError, got Ok"),
    }
}
