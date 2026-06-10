// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! End-to-end test for `read_model` against a filesystem-backed
//! `ProjectAccess` impl.

use simlin_mcp_core::errors::AccessError;
use simlin_mcp_core::test_support::{TestFileSystemAccess, chain_scc_project_json};
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
    // GH #495: every loop carries a polarity_confidence ratio in [0, 1].
    for loop_summary in &output.loop_dominance {
        assert!(
            (0.0..=1.0).contains(&loop_summary.polarity_confidence),
            "polarity_confidence must be in [0, 1], got {}",
            loop_summary.polarity_confidence
        );
    }
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
    // The polarityConfidence field is present on each loopDominance entry.
    let first_loop = &value["loopDominance"][0];
    assert!(
        first_loop.get("polarityConfidence").is_some(),
        "polarityConfidence must appear on the loopDominance wire shape"
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

/// A two-independent-loop model must surface cycle-partition metadata on the
/// analyze output (GH #685): each `loopDominance` entry carries a `partition`
/// index and the top-level `partitions` list holds the partition stock sets.
#[tokio::test]
async fn read_model_surfaces_cycle_partitions() {
    const TWO_PARTITION_XMILE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><vendor>Test</vendor><product version="1.0">Test</product></header>
  <sim_specs method="euler"><start>0</start><stop>5</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <stock name="pop_a"><eqn>100</eqn><inflow>births_a</inflow></stock>
      <flow name="births_a"><eqn>pop_a * 0.02</eqn></flow>
      <stock name="pop_b"><eqn>50</eqn><inflow>births_b</inflow></stock>
      <flow name="births_b"><eqn>pop_b * 0.03</eqn></flow>
    </variables>
  </model>
</xmile>"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two_partition.stmx");
    std::fs::write(&path, TWO_PARTITION_XMILE).unwrap();

    let input = ReadModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
    };
    let output = read_model(&TestFileSystemAccess, input).await.unwrap();
    assert_eq!(
        output.partitions.len(),
        2,
        "two disjoint stock loops must produce two partitions"
    );
    // Every loop summary carries a partition index into `partitions`.
    for ls in &output.loop_dominance {
        let idx = ls.partition.expect("loop must carry a partition");
        assert!(idx < output.partitions.len());
    }
    // The two partitions' stock sets are pop_a and pop_b.
    let stock_sets: Vec<std::collections::BTreeSet<&str>> = output
        .partitions
        .iter()
        .map(|p| p.stocks.iter().map(|s| s.as_str()).collect())
        .collect();
    assert!(
        stock_sets
            .iter()
            .any(|s| s.iter().any(|x| x.contains("pop_a")))
    );
    assert!(
        stock_sets
            .iter()
            .any(|s| s.iter().any(|x| x.contains("pop_b")))
    );

    // Wire-shape assertions: partition appears on loop summaries and the
    // partitions list appears in the serialized output.
    let value = serde_json::to_value(&output).unwrap();
    assert!(
        value.get("partitions").is_some(),
        "partitions must appear in the analyze output wire shape"
    );
    assert!(
        value["loopDominance"][0].get("partition").is_some(),
        "partition must appear on the loopDominance wire shape"
    );
}

/// GH #660: an RK4 model with a stock in a loop cannot be compiled for LTM
/// analysis (the flow-to-stock link-score formula assumes Euler; GH #486).
/// Before #660 the read_model surface returned an empty `loopDominance` with
/// no hint why; now the actionable Euler guidance must reach the caller via
/// the `analysisError` field so an agent asking "what loops?" understands the
/// model needs Euler (or LTM disabled).
#[tokio::test]
async fn read_model_rk4_loop_surfaces_euler_analysis_error() {
    let rk4_model = serde_json::json!({
        "name": "rk4_loop",
        "simSpecs": {
            "startTime": 0.0,
            "endTime": 10.0,
            "dt": "1",
            "method": "rk4"
        },
        "models": [{
            "name": "main",
            "stocks": [
                {"uid": 1, "name": "population", "initialEquation": "100",
                 "inflows": ["births"], "outflows": []}
            ],
            "flows": [
                {"uid": 2, "name": "births", "equation": "population * 0.02"}
            ]
        }]
    });

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rk4_loop.sd.json");
    std::fs::write(&path, rk4_model.to_string()).unwrap();

    let input = ReadModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
    };
    let output = read_model(&TestFileSystemAccess, input).await.unwrap();

    let msg = output
        .analysis_error
        .as_deref()
        .expect("RK4 + LTM read_model must surface an analysisError");
    assert!(
        msg.contains("Euler"),
        "analysisError must reference the Euler assumption, got: {msg}"
    );
    assert!(
        output.loop_dominance.is_empty(),
        "loop_dominance must be empty when the model can't be compiled for LTM"
    );

    // It must also reach the wire (serialized) shape under camelCase.
    let value = serde_json::to_value(&output).unwrap();
    assert!(
        value["analysisError"]
            .as_str()
            .is_some_and(|s| s.contains("Euler")),
        "serialized analysisError must carry the Euler guidance"
    );
}

/// GH #662: read_model collected diagnostics with `ltm_enabled = false`, so the
/// LTM auto-flip-to-discovery advisory (a Warning that only accumulates when
/// LTM is enabled) never reached MCP callers, even though read_model always
/// runs LTM loop analysis via `analyze_model`. Now the diagnostic-collection
/// pass transiently enables LTM, and LTM warnings surface in the output's
/// `warnings` field.
#[tokio::test]
async fn read_model_surfaces_ltm_auto_flip_warning() {
    // A 51-node SCC trips the engine's MAX_LTM_SCC_NODES = 50 auto-flip gate.
    let project = chain_scc_project_json(51);

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chain_scc.sd.json");
    std::fs::write(&path, project.to_string()).unwrap();

    let input = ReadModelInput {
        project_path: path.to_str().unwrap().to_string(),
        model_name: None,
    };
    let output = read_model(&TestFileSystemAccess, input).await.unwrap();

    // The model compiles cleanly without LTM, so there must be no errors.
    assert!(
        output.errors.is_empty(),
        "auto-flip model compiles without LTM, so it must have no errors: {:?}",
        output.errors
    );

    let has_auto_flip = output
        .warnings
        .iter()
        .any(|w| w.message.contains("discovery mode"));
    assert!(
        has_auto_flip,
        "the LTM auto-flip advisory must surface as a warning; got: {:?}",
        output.warnings
    );

    // It must also reach the serialized wire shape.
    let value = serde_json::to_value(&output).unwrap();
    let warnings = value["warnings"]
        .as_array()
        .expect("warnings must serialize as an array");
    assert!(
        warnings.iter().any(|w| w["message"]
            .as_str()
            .is_some_and(|m| m.contains("discovery mode"))),
        "serialized warnings must carry the auto-flip advisory"
    );
}
