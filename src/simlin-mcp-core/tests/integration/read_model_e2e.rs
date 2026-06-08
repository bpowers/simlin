// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for `read_model` against a filesystem-backed
//! `ProjectAccess` impl.

use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::test_support::TestFileSystemAccess;
use simlin_mcp_core::tools::read_model::{ReadModelInput, read_model};

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
    let output = read_model(&TestFileSystemAccess, input).await.unwrap();
    assert!(
        output.errors.is_empty(),
        "clean fixture must produce no errors: {:?}",
        output.errors
    );
    assert!(
        !output.loop_dominance.is_empty(),
        "logistic growth fixture must have at least one loop"
    );
    // The cross-agg reducer-loop recovery budget is a structural-completeness
    // signal; a small scalar fixture has no cross-agg loops to recover, so it
    // is false and (by the skip_serializing_if) elided from the wire shape,
    // keeping the JSON byte-identical for the common case.
    assert!(
        !output.agg_recovery_truncated,
        "logistic growth fixture must not report agg-recovery truncation"
    );
    let value = serde_json::to_value(&output).unwrap();
    assert!(
        value.get("aggRecoveryTruncated").is_none(),
        "a false agg_recovery_truncated must be elided from the wire shape"
    );
}

#[tokio::test]
async fn read_model_missing_file_returns_not_found() {
    let input = ReadModelInput {
        project_path: "/does/not/exist/model.sd.json".into(),
        model_name: None,
    };
    let result = read_model(&TestFileSystemAccess, input).await;
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
    let output = read_model(&TestFileSystemAccess, input).await.unwrap();
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
    let output = read_model(&TestFileSystemAccess, input).await.unwrap();
    assert!(
        !output.errors.is_empty(),
        "broken model must surface errors"
    );
    let value = serde_json::to_value(&output).unwrap();
    let errors = value["errors"].as_array().unwrap();
    assert!(errors[0]["code"].is_string());
    assert!(errors[0]["kind"].is_string());
}
