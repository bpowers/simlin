// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `EditModel` MCP tool: apply curated LLM-friendly operations to an existing model file.
//!
//! The operation types deliberately exclude `uid`, `compat`, and `aiState` fields
//! that are internal bookkeeping and not meaningful to LLM authors.

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json as ejson;

use super::types::{DominantPeriodOutput, LoopDominanceSummary};
use crate::tool::TypedTool;

// ── Curated input types ───────────────────────────────────────────────────────
//
// These types expose only the fields meaningful to an LLM building a model.
// `uid`, `compat`, and `aiState` are intentionally excluded.

/// An element equation for one subscript element of an arrayed variable.
/// Excludes `compat` (internal compatibility field).
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ElementEquationInput {
    /// Subscript label for this element.
    pub subscript: String,
    /// Equation for this element.
    pub equation: String,
    /// Optional graphical (table) function for this element.
    #[serde(default)]
    pub graphical_function: Option<ejson::GraphicalFunction>,
}

/// Equation for an arrayed (subscripted) variable.
/// Excludes `compat` (internal compatibility field).
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArrayedEquationInput {
    /// Dimension names for this array.
    pub dimensions: Vec<String>,
    /// Apply-to-all equation (when the same equation applies to every element).
    #[serde(default)]
    pub equation: Option<String>,
    /// Per-element equations (for element-wise overrides).
    #[serde(default)]
    pub elements: Option<Vec<ElementEquationInput>>,
}

/// Create or update a stock (accumulator) variable.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpsertStockInput {
    /// Variable name (used as identifier).
    pub name: String,
    /// Initial value equation for the stock.
    pub initial_equation: String,
    /// Optional units string.
    #[serde(default)]
    pub units: Option<String>,
    /// Optional documentation / description.
    #[serde(default)]
    pub documentation: Option<String>,
    /// Names of flows that flow into this stock.
    #[serde(default)]
    pub inflows: Option<Vec<String>>,
    /// Names of flows that flow out of this stock.
    #[serde(default)]
    pub outflows: Option<Vec<String>>,
    /// Equation for arrayed (subscripted) stocks.
    #[serde(default)]
    pub arrayed_equation: Option<ArrayedEquationInput>,
}

/// Create or update a flow (rate) variable.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpsertFlowInput {
    /// Variable name (used as identifier).
    pub name: String,
    /// Rate equation for this flow.
    pub equation: String,
    /// Optional units string.
    #[serde(default)]
    pub units: Option<String>,
    /// Optional documentation / description.
    #[serde(default)]
    pub documentation: Option<String>,
    /// Optional graphical (table) function.
    #[serde(default)]
    pub graphical_function: Option<ejson::GraphicalFunction>,
    /// Equation for arrayed (subscripted) flows.
    #[serde(default)]
    pub arrayed_equation: Option<ArrayedEquationInput>,
}

/// Create or update an auxiliary (constant or computed) variable.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UpsertAuxiliaryInput {
    /// Variable name (used as identifier).
    pub name: String,
    /// Equation for this auxiliary variable.
    pub equation: String,
    /// Optional units string.
    #[serde(default)]
    pub units: Option<String>,
    /// Optional documentation / description.
    #[serde(default)]
    pub documentation: Option<String>,
    /// Optional graphical (table) function.
    #[serde(default)]
    pub graphical_function: Option<ejson::GraphicalFunction>,
    /// Equation for arrayed (subscripted) auxiliaries.
    #[serde(default)]
    pub arrayed_equation: Option<ArrayedEquationInput>,
}

/// Remove a variable from the model by name.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoveVariableInput {
    /// Name of the variable to remove.
    pub name: String,
}

/// An edit operation on a model.
///
/// Serde's default externally-tagged representation produces JSON like:
/// `{ "upsertStock": { "name": "population", ... } }`
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EditOperation {
    UpsertStock(UpsertStockInput),
    UpsertFlow(UpsertFlowInput),
    UpsertAuxiliary(UpsertAuxiliaryInput),
    RemoveVariable(RemoveVariableInput),
}

/// Input for the `EditModel` tool.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditModelInput {
    /// Absolute or relative path to the model file.
    pub project_path: String,

    /// Name of the model within the project to edit. Defaults to "main".
    #[serde(default)]
    pub model_name: Option<String>,

    /// If true, validate and preview changes without writing to disk.
    #[serde(default)]
    pub dry_run: Option<bool>,

    /// Optional simulation spec changes to apply before variable operations.
    #[serde(default)]
    pub sim_specs: Option<ejson::SimSpecs>,

    /// Variable operations to apply in order.
    #[serde(default)]
    pub operations: Option<Vec<EditOperation>>,
}

/// Output from the `EditModel` tool -- matches `ReadModel` shape plus `dryRun` flag.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditModelOutput {
    /// Path where the model was written (or the input path on dry-run).
    /// For non-JSON source files (.stmx, .xmile, .mdl) this will differ from
    /// the input path, pointing to the generated `.simlin.json` file instead.
    project_path: String,
    model: ejson::Model,
    time: Vec<f64>,
    loop_dominance: Vec<LoopDominanceSummary>,
    dominant_loops_by_period: Vec<DominantPeriodOutput>,
    dry_run: bool,
}

pub fn tool() -> TypedTool<EditModelInput> {
    TypedTool {
        name: "EditModel",
        description: "Edit a system dynamics model by applying operations. \
             Supports upserting stocks, flows, and auxiliaries, removing variables, \
             and updating simulation specs. Returns a refreshed model snapshot \
             with loop dominance analysis after applying changes. \
             Upsert replaces the full variable definition; omitted optional fields \
             default to empty. Use ReadModel first to get current state, then \
             include all fields you want to preserve.",
        handler: handle_edit_model,
    }
}

fn handle_edit_model(input: EditModelInput) -> anyhow::Result<serde_json::Value> {
    let path = std::path::Path::new(&input.project_path);
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read model file: {}", input.project_path))?;

    let mut project = super::open_project(path, &contents)?;
    let requested_name = input.model_name.as_deref().unwrap_or("main");
    let model_name = super::resolve_model_name(&project, requested_name).to_string();
    let dry_run = input.dry_run.unwrap_or(false);

    let has_variable_ops = input.operations.as_ref().is_some_and(|ops| !ops.is_empty());
    let patch = build_patch(&model_name, input.sim_specs, input.operations);
    simlin_engine::apply_patch(&mut project, patch)
        .map_err(|e| anyhow::anyhow!("patch application failed: {e:?}"))?;

    if !dry_run && has_variable_ops {
        sync_diagram(&mut project, &model_name);
    }

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let write_path = if matches!(ext.as_str(), "stmx" | "xmile" | "xml" | "mdl") {
        path.with_extension("simlin.json")
    } else {
        path.to_path_buf()
    };

    let mut db = simlin_engine::db::SimlinDb::default();
    let sync = simlin_engine::db::sync_from_datamodel(&db, &project);
    let source_project = sync.project;

    let analysis =
        simlin_engine::analysis::analyze_model(&project, &mut db, source_project, &model_name)
            .map_err(|e| anyhow::anyhow!("analysis failed: {e}"))?;

    if !dry_run {
        let json_project = ejson::Project::from(project);
        let json_str = serde_json::to_string_pretty(&json_project)?;
        std::fs::write(&write_path, &json_str)
            .with_context(|| format!("failed to write model to {}", write_path.display()))?;
    }

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

    let output = EditModelOutput {
        project_path: write_path.display().to_string(),
        model: analysis.model,
        time: analysis.time,
        loop_dominance,
        dominant_loops_by_period,
        dry_run,
    };

    serde_json::to_value(output).map_err(Into::into)
}

/// Regenerate the diagram layout for the named model, replacing its views
/// in-place.  Preserves the existing zoom level when the model already has
/// a view.  Layout failures are silently ignored -- a missing diagram is
/// non-fatal and the model data is still correct.
fn sync_diagram(project: &mut simlin_engine::datamodel::Project, model_name: &str) {
    let existing_zoom = project
        .get_model(model_name)
        .and_then(|m| m.views.first())
        .map(|v| match v {
            simlin_engine::datamodel::View::StockFlow(sf) => sf.zoom,
        })
        .filter(|&z| z > 0.0);

    let mut layout = match simlin_engine::layout::generate_best_layout(project, model_name, None) {
        Ok(l) => l,
        Err(_) => return,
    };

    if let Some(zoom) = existing_zoom {
        layout.zoom = zoom;
    }

    if let Some(model) = project.get_model_mut(model_name) {
        model.views = vec![simlin_engine::datamodel::View::StockFlow(layout)];
    }
}

/// Build an engine `ProjectPatch` from the curated MCP inputs.
///
/// `simSpecs` changes are added as a project-level operation first (AC3.7),
/// then variable operations are added as model-level operations.
fn build_patch(
    model_name: &str,
    sim_specs: Option<ejson::SimSpecs>,
    operations: Option<Vec<EditOperation>>,
) -> simlin_engine::ProjectPatch {
    let project_ops = sim_specs
        .map(|ss| vec![simlin_engine::ProjectOperation::SetSimSpecs(ss.into())])
        .unwrap_or_default();

    let model_ops: Vec<simlin_engine::ModelOperation> = operations
        .unwrap_or_default()
        .into_iter()
        .map(convert_operation)
        .collect();

    let models = if model_ops.is_empty() {
        vec![]
    } else {
        vec![simlin_engine::ModelPatch {
            name: model_name.to_string(),
            ops: model_ops,
        }]
    };

    simlin_engine::ProjectPatch {
        project_ops,
        models,
    }
}

/// Convert a curated `ArrayedEquationInput` to an `ejson::ArrayedEquation`,
/// filling in `compat: None` for the excluded field.
fn convert_arrayed_equation(a: ArrayedEquationInput) -> ejson::ArrayedEquation {
    // EXCEPT semantics: a default equation with per-element overrides.
    // When both are present, the default applies to unspecified elements.
    let has_except_default = if a.equation.is_some() && a.elements.is_some() {
        Some(true)
    } else {
        None
    };
    ejson::ArrayedEquation {
        dimensions: a.dimensions,
        equation: a.equation,
        compat: None,
        elements: a.elements.map(|els| {
            els.into_iter()
                .map(|el| ejson::ElementEquation {
                    subscript: el.subscript,
                    equation: el.equation,
                    compat: None,
                    graphical_function: el.graphical_function,
                })
                .collect()
        }),
        has_except_default,
    }
}

/// Convert a curated `EditOperation` to an engine `ModelOperation`.
///
/// Excluded fields (`uid`, `compat`) are filled with engine defaults (0 / None).
fn convert_operation(op: EditOperation) -> simlin_engine::ModelOperation {
    match op {
        EditOperation::UpsertStock(s) => {
            let json_stock = ejson::Stock {
                uid: 0,
                name: s.name,
                initial_equation: s.initial_equation,
                units: s.units.unwrap_or_default(),
                inflows: s.inflows.unwrap_or_default(),
                outflows: s.outflows.unwrap_or_default(),
                documentation: s.documentation.unwrap_or_default(),
                arrayed_equation: s.arrayed_equation.map(convert_arrayed_equation),
                compat: None,
                non_negative: false,
                can_be_module_input: false,
                is_public: false,
            };
            simlin_engine::ModelOperation::UpsertStock(json_stock.into())
        }
        EditOperation::UpsertFlow(f) => {
            let json_flow = ejson::Flow {
                uid: 0,
                name: f.name,
                equation: f.equation,
                units: f.units.unwrap_or_default(),
                graphical_function: f.graphical_function,
                documentation: f.documentation.unwrap_or_default(),
                arrayed_equation: f.arrayed_equation.map(convert_arrayed_equation),
                compat: None,
                non_negative: false,
                can_be_module_input: false,
                is_public: false,
            };
            simlin_engine::ModelOperation::UpsertFlow(json_flow.into())
        }
        EditOperation::UpsertAuxiliary(a) => {
            let json_aux = ejson::Auxiliary {
                uid: 0,
                name: a.name,
                equation: a.equation,
                units: a.units.unwrap_or_default(),
                graphical_function: a.graphical_function,
                documentation: a.documentation.unwrap_or_default(),
                arrayed_equation: a.arrayed_equation.map(convert_arrayed_equation),
                compat: None,
                can_be_module_input: false,
                is_public: false,
            };
            simlin_engine::ModelOperation::UpsertAux(json_aux.into())
        }
        EditOperation::RemoveVariable(r) => {
            simlin_engine::ModelOperation::DeleteVariable { ident: r.name }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    #[test]
    fn test_convert_arrayed_equation_infers_except_default() {
        let input = ArrayedEquationInput {
            dimensions: vec!["DimA".to_string()],
            equation: Some("default_eq".to_string()),
            elements: Some(vec![ElementEquationInput {
                subscript: "A1".to_string(),
                equation: "override_eq".to_string(),
                graphical_function: None,
            }]),
        };
        let result = convert_arrayed_equation(input);
        assert_eq!(result.has_except_default, Some(true));
    }

    #[test]
    fn test_convert_arrayed_equation_no_except_without_both() {
        // equation only, no elements -> not EXCEPT
        let input = ArrayedEquationInput {
            dimensions: vec!["DimA".to_string()],
            equation: Some("eq".to_string()),
            elements: None,
        };
        assert_eq!(convert_arrayed_equation(input).has_except_default, None);

        // elements only, no default equation -> not EXCEPT
        let input = ArrayedEquationInput {
            dimensions: vec!["DimA".to_string()],
            equation: None,
            elements: Some(vec![ElementEquationInput {
                subscript: "A1".to_string(),
                equation: "eq".to_string(),
                graphical_function: None,
            }]),
        };
        assert_eq!(convert_arrayed_equation(input).has_except_default, None);
    }

    fn call_tool(input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        tool().call(input)
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

    fn write_model(
        dir: &std::path::Path,
        filename: &str,
        content: &serde_json::Value,
    ) -> std::path::PathBuf {
        let path = dir.join(filename);
        std::fs::write(&path, serde_json::to_string_pretty(content).unwrap()).unwrap();
        path
    }

    // ---- AC3.10: schema excludes uid, compat, aiState ----

    #[test]
    fn ac3_10_schema_excludes_internal_fields() {
        let t = tool();
        let schema = t.input_schema();
        let schema_str = serde_json::to_string(&schema).unwrap();

        for banned in ["\"uid\"", "\"compat\"", "\"aiState\""] {
            assert!(
                !schema_str.contains(banned),
                "schema should NOT contain {banned} but it does"
            );
        }
    }

    #[test]
    fn ac3_10_schema_has_expected_operation_variants() {
        let t = tool();
        let schema = t.input_schema();
        let schema_str = serde_json::to_string(&schema).unwrap();

        for variant in [
            "upsertStock",
            "upsertFlow",
            "upsertAuxiliary",
            "removeVariable",
        ] {
            assert!(
                schema_str.contains(variant),
                "schema should contain {variant}"
            );
        }
    }

    // ---- AC3.11: missing file returns isError ----

    #[test]
    fn ac3_11_missing_file_returns_error() {
        let result = call_tool(serde_json::json!({
            "projectPath": "/nonexistent/model.simlin.json",
            "operations": [{ "upsertAuxiliary": { "name": "x", "equation": "1" } }]
        }));
        assert!(result.is_err());
    }

    // ---- AC3.4: upsertStock with all optional fields ----

    #[test]
    fn ac3_4_upsert_stock() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": [{
                "upsertStock": {
                    "name": "population",
                    "initialEquation": "1000",
                    "units": "people",
                    "documentation": "Total population",
                    "inflows": ["births"],
                    "outflows": ["deaths"]
                }
            }]
        }))
        .unwrap();

        let model = &result["model"];
        let stocks = model["stocks"].as_array().unwrap();
        assert_eq!(stocks.len(), 1);
        assert_eq!(stocks[0]["name"], "population");
        assert_eq!(stocks[0]["initialEquation"], "1000");
        assert_eq!(stocks[0]["units"], "people");
        assert_eq!(stocks[0]["inflows"][0], "births");
        assert_eq!(stocks[0]["outflows"][0], "deaths");
    }

    // ---- AC3.5: upsertFlow and upsertAuxiliary ----

    #[test]
    fn ac3_5_upsert_flow_and_auxiliary() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": [
                {
                    "upsertFlow": {
                        "name": "births",
                        "equation": "population * birth_rate",
                        "units": "people/year"
                    }
                },
                {
                    "upsertAuxiliary": {
                        "name": "birth_rate",
                        "equation": "0.03",
                        "documentation": "Annual birth rate",
                        "graphicalFunction": {
                            "points": [[0.0, 0.01], [0.5, 0.03], [1.0, 0.05]],
                            "kind": "continuous"
                        }
                    }
                }
            ]
        }))
        .unwrap();

        let model = &result["model"];
        let flows = model["flows"].as_array().unwrap();
        assert_eq!(flows.len(), 1);
        assert_eq!(flows[0]["name"], "births");
        assert_eq!(flows[0]["equation"], "population * birth_rate");

        let auxes = model["auxiliaries"].as_array().unwrap();
        assert_eq!(auxes.len(), 1);
        assert_eq!(auxes[0]["name"], "birth_rate");

        // The graphical function must appear on the returned auxiliary.
        let gf = &auxes[0]["graphicalFunction"];
        assert!(
            gf.is_object(),
            "graphicalFunction should be present on the auxiliary"
        );
        let points = gf["points"].as_array().unwrap();
        assert_eq!(points.len(), 3, "expected 3 graphical function points");
        assert_eq!(gf["kind"], "continuous");
    }

    // ---- AC3.6: removeVariable ----

    #[test]
    fn ac3_6_remove_variable() {
        let dir = tempfile::tempdir().unwrap();

        // First add a variable
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());
        call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": [{ "upsertAuxiliary": { "name": "temp_var", "equation": "42" } }]
        }))
        .unwrap();

        // Then remove it
        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": [{ "removeVariable": { "name": "temp_var" } }]
        }))
        .unwrap();

        let model = &result["model"];
        // auxiliaries may be absent or empty when no auxiliaries remain
        let auxes = model["auxiliaries"]
            .as_array()
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        assert!(
            auxes.iter().all(|a| a["name"] != "temp_var"),
            "temp_var should have been removed"
        );
    }

    // ---- AC3.7: simSpecs applied before variable operations ----

    #[test]
    fn ac3_7_sim_specs_applied_before_variables() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "simSpecs": {
                "startTime": 0.0,
                "endTime": 200.0,
                "dt": "0.5",
                "saveStep": 1.0,
                "method": "euler",
                "timeUnits": ""
            },
            "operations": [{
                "upsertAuxiliary": { "name": "growth_rate", "equation": "0.05" }
            }]
        }))
        .unwrap();

        // Both the sim spec change and the new variable must appear in the response
        let model = &result["model"];
        let sim_specs = &model["simSpecs"];
        // The model-level simSpecs may be null (project-level changed); check time array length
        // as a proxy for the new endTime=200 being in effect.
        let time = result["time"].as_array().unwrap();
        // With endTime=200, dt=0.5, saveStep=1 we expect 201 time points (0..200 inclusive)
        assert!(
            time.len() >= 200,
            "time array should reflect updated endTime=200, got {} points",
            time.len()
        );
        // The new variable must also be present
        let auxes = model["auxiliaries"].as_array().unwrap();
        assert!(
            auxes.iter().any(|a| a["name"] == "growth_rate"),
            "growth_rate auxiliary must be present after variable operation"
        );
        // Silence unused variable warning in the sim_specs check path
        let _ = sim_specs;
    }

    // ---- AC3.8: response has model, time, loopDominance, dominantLoopsByPeriod ----

    #[test]
    fn ac3_8_response_shape() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": []
        }))
        .unwrap();

        assert!(
            result["projectPath"].is_string(),
            "projectPath must be present"
        );
        assert!(result["model"].is_object(), "model must be present");
        assert!(result["time"].is_array(), "time must be present");
        assert!(
            result["loopDominance"].is_array(),
            "loopDominance must be present"
        );
        assert!(
            result["dominantLoopsByPeriod"].is_array(),
            "dominantLoopsByPeriod must be present"
        );
        assert!(result["dryRun"].is_boolean(), "dryRun flag must be present");
    }

    // ---- projectPath in output ----

    #[test]
    fn project_path_in_output_for_json_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": []
        }))
        .unwrap();

        assert_eq!(result["projectPath"], path.to_str().unwrap());
    }

    #[test]
    fn project_path_in_output_redirects_for_stmx_file() {
        let stmx_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("logistic_growth.stmx");
        std::fs::copy(stmx_path, &dest).unwrap();

        let result = call_tool(serde_json::json!({
            "projectPath": dest.to_str().unwrap(),
            "operations": []
        }))
        .unwrap();

        let expected_json_path = dir.path().join("logistic_growth.simlin.json");
        assert_eq!(
            result["projectPath"],
            expected_json_path.to_str().unwrap(),
            "editing a .stmx file must report the .simlin.json write path"
        );
        assert!(
            expected_json_path.exists(),
            ".simlin.json must be written to disk"
        );
    }

    #[test]
    fn project_path_in_output_is_input_path_on_dry_run() {
        let stmx_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );

        let result = call_tool(serde_json::json!({
            "projectPath": stmx_path,
            "dryRun": true,
            "operations": []
        }))
        .unwrap();

        let expected_json_path = std::path::Path::new(stmx_path).with_extension("simlin.json");
        assert_eq!(
            result["projectPath"],
            expected_json_path.to_str().unwrap(),
            "dry-run on a .stmx file must still report the .simlin.json path"
        );
        assert!(
            !expected_json_path.exists(),
            ".simlin.json must NOT be written to disk on dry-run"
        );
    }

    // ---- AC3.9: dry-run does not write to disk ----

    #[test]
    fn ac3_9_dry_run_does_not_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        let original_contents = std::fs::read_to_string(&path).unwrap();

        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "dryRun": true,
            "operations": [{
                "upsertAuxiliary": { "name": "new_var", "equation": "99" }
            }]
        }))
        .unwrap();

        // File on disk must be unchanged
        let after_contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            original_contents, after_contents,
            "dry_run must not modify the file on disk"
        );

        // Response must still include model snapshot and analysis
        assert!(result["model"].is_object());
        assert!(result["time"].is_array());
        assert!(result["loopDominance"].is_array());
        assert!(result["dominantLoopsByPeriod"].is_array());
        assert_eq!(result["dryRun"], true);
    }

    // ---- upsert is full replacement ----

    #[test]
    fn upsert_stock_is_full_replacement() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        // Create a stock with inflows, outflows, units, and documentation.
        call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": [{
                "upsertStock": {
                    "name": "population",
                    "initialEquation": "1000",
                    "units": "people",
                    "documentation": "pop count",
                    "inflows": ["births"],
                    "outflows": ["deaths"]
                }
            }]
        }))
        .unwrap();

        // Upsert with only name and initialEquation -- all other fields must be cleared.
        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": [{
                "upsertStock": {
                    "name": "population",
                    "initialEquation": "2000"
                }
            }]
        }))
        .unwrap();

        let stocks = result["model"]["stocks"].as_array().unwrap();
        let pop = stocks.iter().find(|s| s["name"] == "population").unwrap();

        assert_eq!(
            pop["initialEquation"], "2000",
            "initialEquation must be updated"
        );
        let units = pop["units"].as_str().unwrap_or("");
        assert!(
            units.is_empty(),
            "units must be empty after full-replacement upsert"
        );
        let docs = pop["documentation"].as_str().unwrap_or("");
        assert!(
            docs.is_empty(),
            "documentation must be empty after full-replacement upsert"
        );
        let inflows = pop["inflows"].as_array().map(|v| v.len()).unwrap_or(0);
        assert_eq!(
            inflows, 0,
            "inflows must be empty after full-replacement upsert"
        );
        let outflows = pop["outflows"].as_array().map(|v| v.len()).unwrap_or(0);
        assert_eq!(
            outflows, 0,
            "outflows must be empty after full-replacement upsert"
        );
    }

    // ---- analysis before write: failed analysis leaves file unchanged ----

    #[test]
    fn failed_analysis_does_not_mutate_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        let original_contents = std::fs::read_to_string(&path).unwrap();

        // Requesting a nonexistent model name will cause analyze_model to fail.
        // The simSpecs change should NOT be persisted to disk.
        let result = call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "modelName": "nonexistent",
            "simSpecs": {
                "startTime": 0.0,
                "endTime": 999.0,
                "dt": "1",
                "saveStep": 1.0,
                "method": "euler",
                "timeUnits": ""
            }
        }));

        assert!(
            result.is_err(),
            "EditModel with a nonexistent model name must return an error"
        );

        let after_contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            original_contents, after_contents,
            "file must not be modified when analysis fails"
        );
    }

    // ---- diagram regeneration after edits ----

    #[test]
    fn edit_regenerates_diagram_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        // Add a stock with a flow -- should produce a diagram with view elements.
        call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "operations": [
                {
                    "upsertStock": {
                        "name": "population",
                        "initialEquation": "100",
                        "inflows": ["births"]
                    }
                },
                {
                    "upsertFlow": {
                        "name": "births",
                        "equation": "population * 0.1"
                    }
                }
            ]
        }))
        .unwrap();

        // Read the file back and verify it has non-empty views.
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let views = saved["models"][0]["views"]
            .as_array()
            .expect("views must be an array");
        assert!(
            !views.is_empty(),
            "saved file must contain regenerated views after edit"
        );
        // The view should contain elements for the stock and flow.
        let elements = views[0]["elements"]
            .as_array()
            .expect("view must have elements");
        assert!(
            elements.len() >= 2,
            "view must have at least 2 elements (stock + flow), got {}",
            elements.len()
        );
    }

    #[test]
    fn sim_specs_only_edit_does_not_regenerate_diagram() {
        let dir = tempfile::tempdir().unwrap();
        // Start with a project that has hand-arranged views.
        let project_with_views = serde_json::json!({
            "name": "test",
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
                "views": [{
                    "kind": "stock_flow",
                    "elements": [
                        {"uid": 1, "type": "aux", "name": "hand_placed", "x": 999.0, "y": 999.0, "labelSide": "bottom"}
                    ]
                }]
            }]
        });
        let path = write_model(dir.path(), "model.simlin.json", &project_with_views);

        // Edit only simSpecs -- views should be preserved as-is.
        call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "simSpecs": {
                "startTime": 0.0,
                "endTime": 200.0,
                "dt": "1",
                "saveStep": 1.0,
                "method": "euler",
                "timeUnits": ""
            }
        }))
        .unwrap();

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let views = saved["models"][0]["views"]
            .as_array()
            .expect("views must be preserved");
        assert!(
            !views.is_empty(),
            "simSpecs-only edit must not destroy existing views"
        );
        let elements = views[0]["elements"].as_array().unwrap();
        assert!(
            elements.iter().any(|e| e["name"] == "hand_placed"),
            "hand-placed elements must be preserved for simSpecs-only edits"
        );
    }

    #[test]
    fn dry_run_does_not_regenerate_diagram_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_model(dir.path(), "model.simlin.json", &minimal_project_json());

        call_tool(serde_json::json!({
            "projectPath": path.to_str().unwrap(),
            "dryRun": true,
            "operations": [{
                "upsertStock": {
                    "name": "population",
                    "initialEquation": "100"
                }
            }]
        }))
        .unwrap();

        // File should be unchanged (empty model, no views).
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let views = saved["models"][0]["views"]
            .as_array()
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(views, 0, "dry-run must not write regenerated views to disk");
    }

    // ---- model name resolution falls back to first model when no "main" ----

    #[test]
    fn edit_model_defaults_to_first_model_when_no_main() {
        let stmx_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic_growth_ltm/logistic_growth.stmx"
        );

        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("logistic_growth.stmx");
        std::fs::copy(stmx_path, &dest).unwrap();

        // No modelName supplied -- default "main" should fall back to the first model.
        let result = call_tool(serde_json::json!({
            "projectPath": dest.to_str().unwrap(),
            "operations": [{
                "upsertAuxiliary": { "name": "extra_aux", "equation": "42" }
            }]
        }));

        assert!(
            result.is_ok(),
            "EditModel with no modelName must succeed for a project with no model named 'main': {:?}",
            result
        );
        let value = result.unwrap();
        let auxes = value["model"]["auxiliaries"].as_array().unwrap();
        assert!(
            auxes.iter().any(|a| a["name"] == "extra_aux"),
            "upserted auxiliary must appear in the response model"
        );
    }
}
