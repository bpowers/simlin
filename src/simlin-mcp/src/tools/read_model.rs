// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `ReadModel` MCP tool: reads a model file and returns a JSON snapshot
//! enriched with loop dominance analysis.

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json;

use super::types::{DominantPeriodOutput, ErrorOutput, LoopDominanceSummary};
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<ErrorOutput>,
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

    let (project, _source_format) = super::open_project(path, &contents)?;
    let requested_name = input.model_name.as_deref().unwrap_or("main");
    let model_name = super::resolve_model_name(&project, requested_name);

    let mut db = simlin_engine::db::SimlinDb::default();
    let sync = simlin_engine::db::sync_from_datamodel(&db, &project);
    let source_project = sync.project;

    let diagnostics = simlin_engine::db::collect_all_diagnostics(&db, &sync);
    let errors: Vec<ErrorOutput> = {
        let error_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| matches!(d.severity, simlin_engine::db::DiagnosticSeverity::Error))
            .cloned()
            .collect();
        if error_diags.is_empty() {
            vec![]
        } else {
            simlin_engine::errors::collect_formatted_errors(&error_diags, &project)
                .errors
                .iter()
                .map(ErrorOutput::from)
                .collect()
        }
    };

    let analysis =
        simlin_engine::analysis::analyze_model(&project, &mut db, source_project, model_name)
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
        errors,
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

    // ---- AC3.1: broken equations return model snapshot + non-empty errors array ----

    #[test]
    fn ac3_1_broken_equations_return_errors() {
        let broken_json = serde_json::json!({
            "name": "broken-errors-test",
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
        let file_path = dir.path().join("broken-errors.simlin.json");
        std::fs::write(&file_path, broken_json.to_string()).unwrap();

        let output =
            call_tool(serde_json::json!({ "projectPath": file_path.to_str().unwrap() })).unwrap();

        assert!(
            output["model"].is_object(),
            "model snapshot must be present even with errors"
        );
        let errors = output["errors"]
            .as_array()
            .expect("errors field must be present");
        assert!(
            !errors.is_empty(),
            "errors array must be non-empty for broken equations"
        );
    }

    // ---- AC3.2: each error has code, message, variableName, kind ----

    #[test]
    fn ac3_2_error_fields_present() {
        let broken_json = serde_json::json!({
            "name": "error-fields-test",
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
        let file_path = dir.path().join("error-fields.simlin.json");
        std::fs::write(&file_path, broken_json.to_string()).unwrap();

        let output =
            call_tool(serde_json::json!({ "projectPath": file_path.to_str().unwrap() })).unwrap();

        let errors = output["errors"].as_array().expect("errors must be present");
        for err in errors {
            assert!(
                err["code"].is_string(),
                "each error must have a string 'code' field"
            );
            assert!(
                err["message"].is_string(),
                "each error must have a string 'message' field"
            );
            assert!(
                err["variableName"].is_string(),
                "each error must have a string 'variableName' field"
            );
            assert!(
                err["kind"].is_string(),
                "each error must have a string 'kind' field"
            );
        }
    }

    // ---- AC3.3: clean model omits errors field ----

    #[test]
    fn ac3_3_clean_model_omits_errors() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic-growth.sd.json"
        );
        let output = call_tool(serde_json::json!({ "projectPath": path })).unwrap();

        assert!(
            output.get("errors").is_none(),
            "clean model must omit errors field entirely (skip_serializing_if)"
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

    // ---- AC7.1: ReadModel reads SD-AI JSON files ----

    #[test]
    fn ac7_1_read_model_sdai_json() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sd-ai-simple.sd.json"
        );
        let output = call_tool(serde_json::json!({ "projectPath": path })).unwrap();
        assert!(output["model"].is_object(), "expected model object");

        let stocks = output["model"]["stocks"].as_array().unwrap();
        assert!(
            stocks.iter().any(|s| s["name"] == "Population"),
            "SD-AI model must contain Population stock"
        );
    }

    // ---- AC6.4: loop names set via EditModel appear in ReadModel output ----

    #[test]
    fn ac6_4_loop_names_surface_in_read_model() {
        let edit_tool = super::super::edit_model::tool();

        // Build a logistic growth model with UIDs on variables so that
        // SetLoopName can resolve variable names to UIDs for loop_metadata.
        let model_json = serde_json::json!({
            "name": "loop-name-test",
            "simSpecs": {
                "startTime": 0.0,
                "endTime": 100.0,
                "dt": "1",
                "saveStep": 1.0,
                "method": "euler",
                "timeUnits": ""
            },
            "models": [{
                "name": "main",
                "stocks": [{
                    "uid": 1,
                    "name": "population",
                    "initialEquation": "5",
                    "inflows": ["net_birth_rate"],
                    "outflows": []
                }],
                "flows": [{
                    "uid": 2,
                    "name": "net_birth_rate",
                    "equation": "fractional_growth_rate * population"
                }],
                "auxiliaries": [
                    { "uid": 3, "name": "maximum_growth_rate", "equation": ".12" },
                    { "uid": 4, "name": "carrying_capacity", "equation": "1000" },
                    {
                        "uid": 5,
                        "name": "fractional_growth_rate",
                        "equation": "maximum_growth_rate * (1 - fraction_of_carrying_capacity_used)"
                    },
                    {
                        "uid": 6,
                        "name": "fraction_of_carrying_capacity_used",
                        "equation": "population/carrying_capacity"
                    }
                ]
            }]
        });

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("loop-name.simlin.json");
        std::fs::write(
            &file_path,
            serde_json::to_string_pretty(&model_json).unwrap(),
        )
        .unwrap();
        let path_str = file_path.to_str().unwrap();

        // Step 1: ReadModel to discover loops and their variables.
        let initial_output = call_tool(serde_json::json!({ "projectPath": path_str })).unwrap();
        let loops = initial_output["loopDominance"]
            .as_array()
            .expect("loopDominance must be present");
        assert!(
            !loops.is_empty(),
            "logistic growth model must produce at least one feedback loop"
        );

        // Pick the first discovered loop and note its variables.
        let first_loop = &loops[0];
        let loop_vars: Vec<String> = first_loop["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(
            first_loop["name"].is_null(),
            "loop should not have a name before SetLoopName"
        );

        // Step 2: EditModel with SetLoopName to name the loop.
        // The variables list in LoopSummary includes duplicates (first var
        // repeated at end to close the cycle), so deduplicate for the
        // SetLoopName call which just needs the participating variables.
        let unique_vars: Vec<String> = {
            let mut seen = std::collections::HashSet::new();
            loop_vars
                .into_iter()
                .filter(|v| seen.insert(v.clone()))
                .collect()
        };
        let loop_name = "Growth Feedback";
        let edit_result = edit_tool.call(serde_json::json!({
            "projectPath": path_str,
            "operations": [{
                "setLoopName": {
                    "variables": unique_vars,
                    "name": loop_name,
                    "description": "reinforcing growth loop"
                }
            }]
        }));
        assert!(
            edit_result.is_ok(),
            "EditModel SetLoopName must succeed: {:?}",
            edit_result.err()
        );

        // Step 3: ReadModel the same file and verify the loop name surfaces.
        let final_output = call_tool(serde_json::json!({ "projectPath": path_str })).unwrap();
        let final_loops = final_output["loopDominance"]
            .as_array()
            .expect("loopDominance must be present after naming");

        let named_loop = final_loops
            .iter()
            .find(|l| l["name"].as_str() == Some(loop_name));
        assert!(
            named_loop.is_some(),
            "ReadModel output must contain a loop named '{}' after SetLoopName; \
             found loops: {:?}",
            loop_name,
            final_loops
                .iter()
                .map(|l| format!(
                    "id={}, name={:?}, vars={:?}",
                    l["loopId"], l["name"], l["variables"]
                ))
                .collect::<Vec<_>>()
        );
    }

    // ---- AC7.4: unrecognized JSON returns descriptive error ----

    #[test]
    fn ac7_4_unrecognized_json_returns_descriptive_error() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("bad.sd.json");
        std::fs::write(&file_path, r#"{"unrelated": true}"#).unwrap();

        let result = call_tool(serde_json::json!({
            "projectPath": file_path.to_str().unwrap()
        }));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("models") && err_msg.contains("variables"),
            "error must mention expected formats: {err_msg}"
        );
    }
}
