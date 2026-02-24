// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `ReadModel` MCP tool: reads a model file and returns a JSON snapshot
//! enriched with loop dominance analysis.

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json;

use super::types::{DominantPeriodOutput, LoopDominanceSummary};
use crate::tool::TypedTool;

/// Input for the `ReadModel` tool.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadModelInput {
    /// Absolute or relative path to the model file (XMILE .stmx/.xmile,
    /// Vensim .mdl, or Simlin .simlin JSON).
    pub project_path: String,

    /// Name of the model within the project to analyse.
    /// Defaults to "main" when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadModelOutput {
    model: json::Model,
    time: Vec<f64>,
    loop_dominance: Vec<LoopDominanceSummary>,
    dominant_loops_by_period: Vec<DominantPeriodOutput>,
}

pub fn tool() -> TypedTool<ReadModelInput> {
    TypedTool {
        name: "ReadModel",
        description: "Read a system dynamics model file and return its JSON snapshot \
             enriched with loop dominance analysis. \
             Supports XMILE (.stmx, .xmile), Vensim (.mdl), and Simlin JSON formats.",
        handler: handle_read_model,
    }
}

fn handle_read_model(input: ReadModelInput) -> anyhow::Result<serde_json::Value> {
    let path = std::path::Path::new(&input.project_path);
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read model file: {}", input.project_path))?;

    let project = super::open_project(path, &contents)?;
    let requested_name = input.model_name.as_deref().unwrap_or("main");
    let model_name = super::resolve_model_name(&project, requested_name);

    let analysis = simlin_engine::analysis::analyze_model(&project, model_name)
        .map_err(|e| anyhow::anyhow!("analysis failed: {e}"))?;

    let loop_dominance: Vec<LoopDominanceSummary> = analysis
        .loop_dominance
        .into_iter()
        .map(Into::into)
        .collect();

    let dominant_loops_by_period: Vec<DominantPeriodOutput> = analysis
        .dominant_loops_by_period
        .into_iter()
        .map(Into::into)
        .collect();

    let output = ReadModelOutput {
        model: analysis.model,
        time: analysis.time,
        loop_dominance,
        dominant_loops_by_period,
    };

    serde_json::to_value(&output).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    fn call_tool(input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        tool().call(input)
    }

    // ---- AC2.7: missing file returns isError ----

    #[test]
    fn ac2_7_missing_file_returns_error() {
        let result = call_tool(serde_json::json!({
            "projectPath": "/nonexistent/model.stmx"
        }));
        assert!(result.is_err());
    }

    // ---- AC2.3: default modelName = "main" ----

    #[test]
    fn ac2_3_default_model_name() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let result = call_tool(serde_json::json!({ "projectPath": path }));
        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    }

    // ---- AC2.1: loopDominance is non-empty for known feedback loops ----

    #[test]
    fn ac2_1_loop_dominance_non_empty() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let output = call_tool(serde_json::json!({ "projectPath": path })).unwrap();

        let loop_dominance = output["loopDominance"].as_array().unwrap();
        assert!(
            !loop_dominance.is_empty(),
            "expected non-empty loopDominance"
        );
    }

    // ---- AC2.2: time, importance arrays, dominant period bounds ----

    #[test]
    fn ac2_2_time_and_importance_consistency() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let output = call_tool(serde_json::json!({ "projectPath": path })).unwrap();

        let time = output["time"].as_array().unwrap();
        assert!(!time.is_empty(), "time array must not be empty");

        for loop_entry in output["loopDominance"].as_array().unwrap() {
            let importance = loop_entry["importance"].as_array().unwrap();
            assert_eq!(
                importance.len(),
                time.len(),
                "importance length must equal time length"
            );
        }

        let first_time = time.first().unwrap().as_f64().unwrap();
        let last_time = time.last().unwrap().as_f64().unwrap();
        for period in output["dominantLoopsByPeriod"].as_array().unwrap() {
            let start = period["startTime"].as_f64().unwrap();
            let end = period["endTime"].as_f64().unwrap();
            assert!(
                start >= first_time,
                "period startTime {start} is before simulation start {first_time}"
            );
            assert!(
                end <= last_time,
                "period endTime {end} is after simulation end {last_time}"
            );
        }
    }

    // ---- AC2.4: XMILE, Simlin JSON, and Vensim formats ----

    #[test]
    fn ac2_4_xmile_format() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let output = call_tool(serde_json::json!({ "projectPath": path })).unwrap();
        assert!(output["model"].is_object(), "expected model object");
    }

    #[test]
    fn ac2_4_simlin_json_format() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic-growth.sd.json"
        );
        let output = call_tool(serde_json::json!({ "projectPath": path })).unwrap();
        assert!(output["model"].is_object(), "expected model object");
    }

    #[test]
    fn ac2_4_vensim_mdl_format() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/elmcount/elmcount.mdl"
        );
        let output = call_tool(serde_json::json!({ "projectPath": path })).unwrap();
        assert!(output["model"].is_object(), "expected model object");
    }

    // ---- AC2.5: views field is empty ----

    #[test]
    fn ac2_5_views_are_absent_or_empty() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );
        let output = call_tool(serde_json::json!({ "projectPath": path })).unwrap();
        let views = &output["model"]["views"];
        assert!(
            views.is_null() || views.as_array().is_none_or(|v| v.is_empty()),
            "expected empty views, got: {views}"
        );
    }

    // ---- AC2.6: broken equations → empty loop arrays but model snapshot present ----

    #[test]
    fn ac2_6_broken_equations_return_empty_loops() {
        let broken_json = serde_json::json!({
            "name": "broken-test",
            "simSpecs": {
                "startTime": 0.0,
                "endTime": 10.0,
                "dt": "1",
                "method": "euler"
            },
            "models": [{
                "name": "main",
                "stocks": [{"name": "population", "initialEquation": "10", "inflows": ["births"], "outflows": []}],
                "flows": [{"name": "births", "equation": "nonexistent_variable * population"}]
            }]
        });

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("broken.simlin.json");
        std::fs::write(&file_path, broken_json.to_string()).unwrap();

        let output =
            call_tool(serde_json::json!({ "projectPath": file_path.to_str().unwrap() })).unwrap();

        assert!(
            output["model"].is_object(),
            "model snapshot must be present"
        );
        assert_eq!(
            output["loopDominance"].as_array().unwrap().len(),
            0,
            "loopDominance must be empty"
        );
        assert_eq!(
            output["dominantLoopsByPeriod"].as_array().unwrap().len(),
            0,
            "dominantLoopsByPeriod must be empty"
        );
        assert_eq!(
            output["time"].as_array().unwrap().len(),
            0,
            "time must be empty"
        );
    }

    // ---- schema check ----

    #[test]
    fn test_schema_has_project_path() {
        let t = tool();
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["projectPath"].is_object());
        assert_eq!(schema["properties"]["projectPath"]["type"], "string");
    }
}
