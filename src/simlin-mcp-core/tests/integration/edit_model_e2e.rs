// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for `edit_model` against a filesystem-backed
//! `ProjectAccess` impl.  These tests exercise the validation gate
//! (post-edit diagnostics surface as `AccessError::Validation`) and the
//! in-place Vensim `.mdl` write path (regenerated MDL text with the sketch,
//! export lossiness surfaced as warnings).

use std::path::Path;

use simlin_engine::datamodel;
use simlin_mcp_core::access::ProjectAccess;
use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::test_support::{TestFileSystemAccess, chain_scc_project_json};
use simlin_mcp_core::tools::edit_model::{
    EditModelInput, EditOperation, UpsertAuxiliaryInput, UpsertFlowInput, UpsertStockInput,
    edit_model,
};
use simlin_mcp_core::types::SourceFormat;

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

/// AC6.1: `EditModel` reports the same discovery-completeness counters
/// `ReadModel` does, so a client editing a model in a loop is not left
/// guessing whether the `loopDominance` it just got back is exhaustive.
///
/// The edit builds a reinforcing population loop from nothing, which is small
/// enough that discovery ENUMERATES its whole universe -- the exact arm. The
/// sampled arm (`enumerationComplete == false`, `universeLoops` elided) is not
/// reachable from either MCP tool, neither of which takes a discovery budget;
/// its wire shape is pinned in `read_model_e2e.rs` and the engine behaviour
/// behind it in `ltm_finding_tests::the_fallback_reports_no_universe`.
#[tokio::test]
async fn edit_reports_discovery_completeness_counters() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_model(dir.path(), "model.sd.json", &minimal_project_json());

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![
            EditOperation::UpsertStock(UpsertStockInput {
                name: "population".into(),
                initial_equation: "100".into(),
                units: None,
                documentation: None,
                inflows: Some(vec!["births".into()]),
                outflows: None,
                arrayed_equation: None,
            }),
            EditOperation::UpsertFlow(UpsertFlowInput {
                name: "births".into(),
                equation: "population * 0.1".into(),
                units: None,
                documentation: None,
                graphical_function: None,
                arrayed_equation: None,
            }),
        ]),
    };

    let output = edit_model(&TestFileSystemAccess, input).await.unwrap();
    assert!(
        !output.loop_dominance.is_empty(),
        "the edited model has a reinforcing loop"
    );
    assert!(
        output.enumeration_complete,
        "a two-variable loop is enumerated exactly"
    );
    assert_eq!(output.retained_loops, output.loop_dominance.len());
    let universe = output
        .universe_loops
        .expect("an exact run names its universe");
    assert!(universe >= output.loop_dominance.len());

    let value = serde_json::to_value(&output).unwrap();
    assert_eq!(
        value.get("enumerationComplete"),
        Some(&serde_json::Value::Bool(true)),
        "enumerationComplete must always appear on the wire shape"
    );
    assert_eq!(
        value["retainedLoops"].as_u64(),
        Some(output.retained_loops as u64)
    );
    assert_eq!(value["universeLoops"].as_u64(), Some(universe as u64));
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

const TEACUP_MDL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test/test-models/samples/teacup/teacup.mdl"
);

fn copy_teacup_mdl(dir: &Path) -> std::path::PathBuf {
    let dest = dir.join("teacup.mdl");
    std::fs::copy(TEACUP_MDL, &dest).expect("copy teacup.mdl fixture");
    dest
}

fn variable_names(project: &datamodel::Project) -> Vec<String> {
    let mut names: Vec<String> = project.models[0]
        .variables
        .iter()
        .map(|v| v.get_ident().to_string())
        .collect();
    names.sort();
    names
}

fn view_element_names(project: &datamodel::Project) -> Vec<String> {
    let mut names: Vec<String> = project.models[0]
        .views
        .iter()
        .flat_map(|v| match v {
            datamodel::View::StockFlow(sf) => sf.elements.iter(),
        })
        .filter_map(|e| match e {
            datamodel::ViewElement::Aux(a) => Some(a.name.clone()),
            datamodel::ViewElement::Stock(st) => Some(st.name.clone()),
            datamodel::ViewElement::Flow(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect();
    names.sort();
    names
}

fn upsert_aux(name: &str, equation: &str) -> EditOperation {
    EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
        name: name.into(),
        equation: equation.into(),
        units: None,
        documentation: None,
        graphical_function: None,
        arrayed_equation: None,
    })
}

/// A `.mdl` is a first-class read/write format: an edit rewrites the file
/// in place as Vensim text (no sidecar, no rejection) with the sketch
/// carried through, and reopening it yields the original variables plus the
/// new one, with the diagram elements -- including one for the new
/// variable -- intact.  This is the MCP counterpart of pysimlin's
/// `test_ac1_2_mdl_edit_rewrites_file_in_mdl_with_sketch`.
#[tokio::test]
async fn mdl_edit_rewrites_the_file_in_place_and_reopens_with_views_intact() {
    let dir = tempfile::tempdir().unwrap();
    let path = copy_teacup_mdl(dir.path());

    let before = TestFileSystemAccess.open(&path).await.expect("open mdl");
    assert_eq!(before.source_format, SourceFormat::Mdl);
    let vars_before = variable_names(&before.project);
    let elements_before = view_element_names(&before.project);
    assert!(
        !elements_before.is_empty(),
        "fixture must carry a sketch, or this test cannot see it survive"
    );

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![upsert_aux("new aux", "room_temperature * 2")]),
    };
    let output = edit_model(&TestFileSystemAccess, input)
        .await
        .expect("edit mdl");
    assert!(!output.dry_run);
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.message.starts_with("MDL export:")),
        "a Vensim-expressible edit must not report export lossiness: {:?}",
        output.warnings
    );

    // The bytes on disk are Vensim text, not JSON or XMILE: display names
    // and the sketch banner both appear.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("{UTF-8}"),
        "file must be MDL text: {text:.60}"
    );
    assert!(
        text.lines().any(|l| l.trim_start().starts_with("new aux")),
        "new variable must be written under its Vensim display name"
    );
    assert!(
        text.contains("Sketch information"),
        "sketch must be preserved"
    );

    let after = TestFileSystemAccess.open(&path).await.expect("reopen mdl");
    assert_eq!(after.source_format, SourceFormat::Mdl);
    let mut expected_vars = vars_before.clone();
    expected_vars.push("new_aux".to_string());
    expected_vars.sort();
    assert_eq!(variable_names(&after.project), expected_vars);

    let elements_after = view_element_names(&after.project);
    for name in &elements_before {
        assert!(
            elements_after.contains(name),
            "view element '{name}' must survive the rewrite; got {elements_after:?}"
        );
    }
    assert!(
        elements_after
            .iter()
            .any(|n| n == "new_aux" || n == "new aux"),
        "the new variable must get a sketch element: {elements_after:?}"
    );
}

/// The MDL writer's lossiness channel reaches the tool result: an edit that
/// introduces a construct Vensim cannot express (a discrete graphical
/// function, emitted continuous) still saves, and the result's `warnings`
/// names the degradation with the `MDL export:` prefix so the agent can
/// tell it apart from a model diagnostic.
#[tokio::test]
async fn mdl_edit_surfaces_export_lossiness_as_warnings_and_still_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = copy_teacup_mdl(dir.path());

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: None,
        sim_specs: None,
        operations: Some(vec![EditOperation::UpsertAuxiliary(UpsertAuxiliaryInput {
            name: "stepped".into(),
            equation: "teacup_temperature".into(),
            units: None,
            documentation: None,
            graphical_function: Some(simlin_engine::json::GraphicalFunction {
                points: vec![[0.0, 0.0], [100.0, 1.0], [200.0, 2.0]],
                y_points: vec![],
                kind: "discrete".into(),
                x_scale: None,
                y_scale: None,
            }),
            arrayed_equation: None,
        })]),
    };
    let output = edit_model(&TestFileSystemAccess, input)
        .await
        .expect("edit mdl");

    let export_warnings: Vec<_> = output
        .warnings
        .iter()
        .filter(|w| w.message.starts_with("MDL export:"))
        .collect();
    assert!(
        export_warnings
            .iter()
            .any(|w| w.message.contains("stepped") && w.message.contains("discrete")),
        "the discrete-GF degradation must be reported against its variable: {:?}",
        output.warnings
    );
    for w in &export_warnings {
        assert_eq!(w.code, "generic");
        assert_eq!(w.kind, "model");
        assert!(w.model_name.is_some(), "export warnings are model-scoped");
    }

    // The write went through despite the warning, and the file reparses.
    let after = TestFileSystemAccess.open(&path).await.expect("reopen mdl");
    assert!(variable_names(&after.project).contains(&"stepped".to_string()));
}

/// A dry run on a `.mdl` previews without writing, and because nothing was
/// serialised it carries no export warnings even for a lossy edit -- the
/// warnings describe what landed on disk, not what would.
#[tokio::test]
async fn mdl_dry_run_leaves_the_file_alone_and_reports_no_export_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let path = copy_teacup_mdl(dir.path());
    let original = std::fs::read(&path).unwrap();

    let input = EditModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
        dry_run: Some(true),
        sim_specs: None,
        operations: Some(vec![upsert_aux("preview only", "1")]),
    };
    let output = edit_model(&TestFileSystemAccess, input)
        .await
        .expect("dry run");
    assert!(output.dry_run);
    assert!(
        !output
            .warnings
            .iter()
            .any(|w| w.message.starts_with("MDL export:")),
        "dry run must not report export warnings: {:?}",
        output.warnings
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        original,
        "dry run must not touch the .mdl"
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
