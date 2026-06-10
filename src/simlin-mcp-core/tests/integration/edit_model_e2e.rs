// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for `edit_model` against a filesystem-backed
//! `ProjectAccess` impl.  These tests exercise the validation gate
//! (post-edit diagnostics surface as `AccessError::Validation`) and
//! the `.mdl` read-only rejection.

use std::path::Path;

use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::test_support::{TestFileSystemAccess, chain_scc_project_json};
use simlin_mcp_core::tools::edit_model::{
    EditModelInput, EditOperation, UpsertAuxiliaryInput, UpsertFlowInput, UpsertStockInput,
    edit_model,
};

fn broken_project_json() -> serde_json::Value {
    serde_json::json!({
        "name": "broken",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 10.0,
            "dt": "1",
            "saveStep": 1.0,
            "method": "euler",
            "timeUnits": ""
        },
        "models": [{
            "name": "main",
            "auxiliaries": [
                {"uid": 1, "name": "bad", "equation": "undefined_var + 1"}
            ]
        }]
    })
}

fn project_named(model_name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": "test",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 10.0,
            "dt": "1",
            "saveStep": 1.0,
            "method": "euler",
            "timeUnits": ""
        },
        "models": [{ "name": model_name }]
    })
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

    let output = edit_model(&TestFileSystemAccess, input).await.unwrap();
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

    let result = edit_model(&TestFileSystemAccess, input).await;
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

    let result = edit_model(&TestFileSystemAccess, input).await;
    let err_msg = match result {
        Err(e) => e.to_string(),
        Ok(_) => panic!("expected error rejecting .mdl file, got Ok"),
    };
    assert_eq!(
        err_msg,
        "Vensim .mdl files are read-only. Use ReadModel to inspect a .mdl file, \
         then CreateModel to start a new .sd.json file you can edit.",
        "canonical .mdl rejection message must be exact (no prefix): {err_msg}"
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

    let output = edit_model(&TestFileSystemAccess, input).await.unwrap();
    assert!(output.dry_run);

    let after_contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        original_contents, after_contents,
        "dry_run must not modify the file on disk"
    );
}

#[tokio::test]
async fn mdl_files_rejected_even_for_dry_run() {
    let mdl_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/sdeverywhere/models/elmcount/elmcount.mdl"
    );

    let input = EditModelInput {
        project_path: mdl_path.into(),
        model_name: None,
        dry_run: Some(true),
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

    let result = edit_model(&TestFileSystemAccess, input).await;
    match result {
        Err(e) => {
            let err_msg = e.to_string();
            assert_eq!(
                err_msg,
                "Vensim .mdl files are read-only. Use ReadModel to inspect a .mdl file, \
                 then CreateModel to start a new .sd.json file you can edit.",
                "canonical .mdl rejection message must be exact (no prefix): {err_msg}"
            );
        }
        Ok(_) => panic!(".mdl file must be rejected even for dry_run=true, got Ok"),
    }
}

#[tokio::test]
async fn error_gate_allows_edit_on_already_broken_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "broken.sd.json", &broken_project_json());

    // The model already has `bad = undefined_var + 1`. Adding another
    // valid aux (the equation is "1" which has no dependencies) must
    // succeed because no NEW (code, variable_name) pair is introduced —
    // the pre-existing error on `bad` was already there.
    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
            name: "good".into(),
            equation: "1".into(),
            units: None,
            documentation: None,
            graphical_function: None,
            arrayed_equation: None,
        })]),
    };

    let result = edit_model(&TestFileSystemAccess, input).await;
    if let Err(ref e) = result {
        panic!("edit on already-broken model that adds no new errors must succeed; got: {e:?}");
    }
}

#[tokio::test]
async fn error_gate_rejects_edit_that_swaps_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "broken.sd.json", &broken_project_json());

    // Replace `bad = undefined_var + 1` with `bad = other_missing + 1`.
    // The old error key was (code, "bad") for `undefined_var`; after the
    // edit the error key is still (code, "bad") but for `other_missing`.
    // Depending on whether the error code is the same, this may or may not
    // be rejected — but *adding* `another_bad = also_missing` on top of
    // the existing error on `bad` introduces a new (code, "another_bad")
    // key and must be rejected.
    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
            name: "another_bad".into(),
            equation: "also_missing + 2".into(),
            units: None,
            documentation: None,
            graphical_function: None,
            arrayed_equation: None,
        })]),
    };

    let result = edit_model(&TestFileSystemAccess, input).await;
    match result {
        Err(AccessError::Validation { errors }) => {
            assert!(
                !errors.is_empty(),
                "rejection due to new error must include error details"
            );
        }
        Err(other) => panic!("expected AccessError::Validation, got: {other:?}"),
        Ok(_) => panic!("edit introducing new error on broken model must be rejected"),
    }
}

#[tokio::test]
async fn error_gate_rejects_edit_that_adds_new_error_on_broken_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "broken.sd.json", &broken_project_json());
    let original_contents = std::fs::read_to_string(&path).unwrap();

    // The model already has an error on "bad". Adding a new aux with a
    // broken equation adds a NEW (code, variable_name) pair and must be
    // rejected.
    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
            name: "new_bad".into(),
            equation: "yet_another_missing + 1".into(),
            units: None,
            documentation: None,
            graphical_function: None,
            arrayed_equation: None,
        })]),
    };

    let result = edit_model(&TestFileSystemAccess, input).await;
    assert!(
        result.is_err(),
        "edit introducing new error must be rejected"
    );

    let after_contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        original_contents, after_contents,
        "rejected edit must not modify file on disk"
    );
}

#[tokio::test]
async fn edit_model_defaults_to_first_model_when_no_main() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "custom.sd.json", &project_named("mymodel"));

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
            name: "x".into(),
            equation: "1".into(),
            units: None,
            documentation: None,
            graphical_function: None,
            arrayed_equation: None,
        })]),
    };

    let output = edit_model(&TestFileSystemAccess, input).await.unwrap();

    // The output model should be "mymodel" (first model), not "main".
    assert_eq!(
        output.project_path,
        path.to_str().unwrap(),
        "project_path in output must match input path"
    );
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let auxes = saved["models"][0]["auxiliaries"].as_array().unwrap();
    assert!(
        auxes.iter().any(|a| a["name"] == "x"),
        "edit must have applied to the first model ('mymodel'): {auxes:?}"
    );
}

#[tokio::test]
async fn upsert_stock_is_full_replacement() {
    let dir = tempfile::tempdir().unwrap();
    // Build a project that already has "births" as a flow so the first
    // upsert can reference it in inflows without introducing an error.
    let project = serde_json::json!({
        "name": "test",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 10.0,
            "dt": "1",
            "saveStep": 1.0,
            "method": "euler",
            "timeUnits": ""
        },
        "models": [{
            "name": "main",
            "flows": [{"uid": 2, "name": "births", "equation": "0"}]
        }]
    });
    let path = write_model(dir.path(), "model.sd.json", &project);

    // First upsert: create a stock with explicit inflows and documentation.
    let input1 = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertStock(UpsertStockInput {
            name: "pop".into(),
            initial_equation: "100".into(),
            units: Some("people".into()),
            documentation: Some("original doc".into()),
            inflows: Some(vec!["births".into()]),
            outflows: None,
            arrayed_equation: None,
        })]),
    };
    edit_model(&TestFileSystemAccess, input1).await.unwrap();

    // Second upsert with the same name but different fields — must fully
    // replace, not merge.  The new upsert omits inflows (defaults to empty)
    // and changes the equation, so the inflows list must be cleared.
    let input2 = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertStock(UpsertStockInput {
            name: "pop".into(),
            initial_equation: "200".into(),
            units: None,
            documentation: None,
            inflows: None,
            outflows: None,
            arrayed_equation: None,
        })]),
    };
    edit_model(&TestFileSystemAccess, input2).await.unwrap();

    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let stocks = saved["models"][0]["stocks"].as_array().unwrap();
    let pop = stocks.iter().find(|s| s["name"] == "pop").unwrap();

    // Equation must be replaced.
    assert_eq!(
        pop["initialEquation"].as_str().unwrap_or(""),
        "200",
        "second upsert must replace initial equation: {pop}"
    );
    // Inflows must be empty after the second upsert because it omitted
    // them — upsert replaces the full variable definition.
    let inflows = pop["inflows"].as_array().map(|a| a.len()).unwrap_or(0);
    assert_eq!(
        inflows, 0,
        "second upsert with no inflows must clear the inflows list: {pop}"
    );
}

/// GH #662: edit_model collected its post-edit diagnostics with
/// `ltm_enabled = false`, so the LTM auto-flip advisory never reached MCP
/// callers even though edit_model always runs LTM analysis. The
/// diagnostic-collection passes now transiently enable LTM, and the advisory
/// surfaces in the success response's `warnings` field. A dry-run edit that
/// adds one unrelated aux keeps the 51-node SCC intact and introduces no new
/// error, so the edit succeeds and carries the warning.
#[tokio::test]
async fn edit_model_surfaces_ltm_auto_flip_warning() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "chain_scc.sd.json", &chain_scc_project_json(51));

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: Some(true),
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
            name: "unrelated".into(),
            equation: "1".into(),
            units: None,
            documentation: None,
            graphical_function: None,
            arrayed_equation: None,
        })]),
    };

    let output = edit_model(&TestFileSystemAccess, input)
        .await
        .expect("a clean dry-run edit on an auto-flip model must succeed");

    let has_auto_flip = output
        .warnings
        .iter()
        .any(|w| w.message.contains("discovery mode"));
    assert!(
        has_auto_flip,
        "the LTM auto-flip advisory must surface in edit_model warnings; got: {:?}",
        output.warnings
    );

    // And it must reach the serialized wire shape.
    let value = serde_json::to_value(&output).unwrap();
    let warnings = value["warnings"]
        .as_array()
        .expect("warnings must serialize as an array");
    assert!(
        warnings.iter().any(|w| w["message"]
            .as_str()
            .is_some_and(|m| m.contains("discovery mode"))),
        "serialized edit_model warnings must carry the auto-flip advisory"
    );
}
