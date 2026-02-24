// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `edit_model` MCP tool: apply a patch to an existing model file.
//!
//! This is a thin wrapper around the `apply_patch` engine API.  The
//! patch JSON schema is automatically derived from the Rust types via
//! `schemars`, so the full schema (including all variable types,
//! operations, sim specs, etc.) is visible in the MCP tool definition.

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json as ejson;

use crate::tool::TypedTool;

// ── Patch input types with JsonSchema ────────────────────────────────
//
// These mirror the libsimlin JSON patch format but add `JsonSchema`
// derives so the schema flows into the MCP tool definition.

/// A project-level operation.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum ProjectOperation {
    /// Update simulation specifications (start/end time, dt, method).
    SetSimSpecs {
        /// The new simulation specifications.
        #[serde(rename = "simSpecs")]
        sim_specs: ejson::SimSpecs,
    },
    /// Add a new empty model to the project.
    AddModel {
        /// Name of the new model.
        name: String,
    },
}

/// An operation on a single model within the project.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum ModelOperation {
    /// Create or update an auxiliary variable.
    UpsertAux {
        /// The auxiliary variable definition.
        aux: ejson::Auxiliary,
    },
    /// Create or update a stock (accumulator) variable.
    UpsertStock {
        /// The stock variable definition.
        stock: ejson::Stock,
    },
    /// Create or update a flow (rate) variable.
    UpsertFlow {
        /// The flow variable definition.
        flow: ejson::Flow,
    },
    /// Create or update a module reference.
    UpsertModule {
        /// The module definition.
        module: ejson::Module,
    },
    /// Delete a variable by name.
    DeleteVariable {
        /// The variable identifier to delete.
        ident: String,
    },
    /// Rename a variable, updating all references.
    RenameVariable {
        /// Current variable name.
        from: String,
        /// New variable name.
        to: String,
    },
    /// Create or update a diagram view.
    UpsertView {
        /// Zero-based index of the view to create/replace.
        index: u32,
        /// The view definition.
        view: ejson::View,
    },
    /// Delete a diagram view by index.
    DeleteView {
        /// Zero-based index of the view to delete.
        index: u32,
    },
    /// Replace a stock's inflow/outflow connections.
    UpdateStockFlows {
        /// The stock identifier.
        ident: String,
        /// New list of inflow names.
        inflows: Vec<String>,
        /// New list of outflow names.
        outflows: Vec<String>,
    },
}

/// A set of operations targeting a specific model within the project.
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelPatch {
    /// Name of the model to patch.
    pub name: String,
    /// Operations to apply to this model.
    #[serde(default)]
    pub ops: Vec<ModelOperation>,
}

/// Input for the `edit_model` tool.
///
/// Contains a path to the model file and a patch describing the
/// changes to apply.  The patch format supports project-level
/// operations (like changing simulation specs or adding models) and
/// model-level operations (like upserting variables).
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EditModelInput {
    /// Absolute or relative path to the model file.
    pub model_path: String,

    /// Project-level operations to apply before model operations.
    #[serde(default)]
    pub project_ops: Vec<ProjectOperation>,

    /// Per-model patches, each targeting a named model.
    #[serde(default)]
    pub models: Vec<ModelPatch>,

    /// If true, validate the patch without writing changes to disk.
    #[serde(default)]
    pub dry_run: bool,
}

/// Output from the `edit_model` tool.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditModelOutput {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<ejson::Project>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<String>,
}

pub fn tool() -> TypedTool<EditModelInput> {
    TypedTool {
        name: "edit_model",
        description: "Edit a system dynamics model by applying a patch. \
             Supports upserting variables (stocks, flows, auxiliaries, modules), \
             deleting/renaming variables, updating stock flows, modifying \
             simulation specs, and managing diagram views.",
        handler: handle_edit_model,
    }
}

fn handle_edit_model(input: EditModelInput) -> anyhow::Result<serde_json::Value> {
    let path = std::path::Path::new(&input.model_path);
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read model file: {}", input.model_path))?;

    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut project = super::open_project(path, &contents)?;

    // Convert MCP patch types to engine patch types and apply
    let engine_patch = convert_patch(&input);
    simlin_engine::apply_patch(&mut project, engine_patch)
        .map_err(|e| anyhow::anyhow!("patch application failed: {e:?}"))?;

    if input.dry_run {
        let output = EditModelOutput {
            success: true,
            project: None,
            diagnostics: vec![],
        };
        return serde_json::to_value(output).map_err(Into::into);
    }

    let json_project = ejson::Project::from(project.clone());

    // Write back as JSON (the canonical format for MCP-managed models)
    let json_str = serde_json::to_string_pretty(&json_project)?;
    let write_path = if ext == "stmx" || ext == "xmile" || ext == "xml" || ext == "mdl" {
        path.with_extension("simlin.json")
    } else {
        path.to_path_buf()
    };
    std::fs::write(&write_path, &json_str)
        .with_context(|| format!("failed to write model to {}", write_path.display()))?;

    let output = EditModelOutput {
        success: true,
        project: Some(json_project),
        diagnostics: vec![],
    };
    serde_json::to_value(output).map_err(Into::into)
}

fn convert_patch(input: &EditModelInput) -> simlin_engine::ProjectPatch {
    let project_ops = input
        .project_ops
        .iter()
        .map(|op| match op {
            ProjectOperation::SetSimSpecs { sim_specs } => {
                simlin_engine::ProjectOperation::SetSimSpecs(sim_specs.clone().into())
            }
            ProjectOperation::AddModel { name } => {
                simlin_engine::ProjectOperation::AddModel { name: name.clone() }
            }
        })
        .collect();

    let models = input
        .models
        .iter()
        .map(|mp| {
            let ops = mp
                .ops
                .iter()
                .map(|op| match op {
                    ModelOperation::UpsertAux { aux } => {
                        simlin_engine::ModelOperation::UpsertAux(aux.clone().into())
                    }
                    ModelOperation::UpsertStock { stock } => {
                        simlin_engine::ModelOperation::UpsertStock(stock.clone().into())
                    }
                    ModelOperation::UpsertFlow { flow } => {
                        simlin_engine::ModelOperation::UpsertFlow(flow.clone().into())
                    }
                    ModelOperation::UpsertModule { module } => {
                        simlin_engine::ModelOperation::UpsertModule(module.clone().into())
                    }
                    ModelOperation::DeleteVariable { ident } => {
                        simlin_engine::ModelOperation::DeleteVariable {
                            ident: ident.clone(),
                        }
                    }
                    ModelOperation::RenameVariable { from, to } => {
                        simlin_engine::ModelOperation::RenameVariable {
                            from: from.clone(),
                            to: to.clone(),
                        }
                    }
                    ModelOperation::UpsertView { index, view } => {
                        simlin_engine::ModelOperation::UpsertView {
                            index: *index,
                            view: view.clone().into(),
                        }
                    }
                    ModelOperation::DeleteView { index } => {
                        simlin_engine::ModelOperation::DeleteView { index: *index }
                    }
                    ModelOperation::UpdateStockFlows {
                        ident,
                        inflows,
                        outflows,
                    } => simlin_engine::ModelOperation::UpdateStockFlows {
                        ident: ident.clone(),
                        inflows: inflows.clone(),
                        outflows: outflows.clone(),
                    },
                })
                .collect();

            simlin_engine::ModelPatch {
                name: mp.name.clone(),
                ops,
            }
        })
        .collect();

    simlin_engine::ProjectPatch {
        project_ops,
        models,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    #[test]
    fn test_edit_model_schema_has_patch_operations() {
        let t = tool();
        let schema = t.input_schema();

        assert_eq!(schema["type"], "object");
        let props = &schema["properties"];

        assert!(props["modelPath"].is_object());
        assert!(props["projectOps"].is_object());
        assert!(props["models"].is_object());
        assert!(props["dryRun"].is_object());
    }

    #[test]
    fn test_edit_model_schema_model_operations_visible() {
        let t = tool();
        let schema = t.input_schema();
        let schema_str = serde_json::to_string_pretty(&schema).unwrap();

        // With schemars v1 and adjacently-tagged enums (tag="type",
        // content="payload", rename_all="camelCase"), variant names
        // appear in camelCase.
        for op in [
            "upsertAux",
            "upsertStock",
            "upsertFlow",
            "deleteVariable",
            "renameVariable",
        ] {
            assert!(
                schema_str.contains(op),
                "schema should contain {op} variant"
            );
        }
    }

    #[test]
    fn test_edit_model_schema_variable_fields_visible() {
        let t = tool();
        let schema = t.input_schema();
        let schema_str = serde_json::to_string_pretty(&schema).unwrap();

        assert!(
            schema_str.contains("equation"),
            "schema should contain equation field"
        );
        assert!(
            schema_str.contains("initialEquation"),
            "schema should contain initialEquation field"
        );
        assert!(
            schema_str.contains("nonNegative"),
            "schema should contain nonNegative field"
        );
    }

    #[test]
    fn test_edit_model_missing_file() {
        let t = tool();
        let result = t.call(serde_json::json!({
            "modelPath": "/nonexistent/model.json",
            "models": [{
                "name": "main",
                "ops": [{
                    "type": "upsertAux",
                    "payload": {
                        "aux": { "name": "x", "equation": "1" }
                    }
                }]
            }]
        }));
        assert!(result.is_err());
    }

    #[test]
    fn test_edit_model_end_to_end() {
        // Create a temp model, then edit it via the tool
        let dir = std::env::temp_dir().join("simlin-mcp-test-edit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model_path = dir.join("test.simlin.json");

        // Create a minimal model
        let project = serde_json::json!({
            "name": "test",
            "simSpecs": {
                "startTime": 0,
                "endTime": 100,
                "dt": "1",
                "saveStep": 1,
                "method": "euler"
            },
            "models": [{
                "name": "main"
            }]
        });
        std::fs::write(&model_path, serde_json::to_string_pretty(&project).unwrap()).unwrap();

        let t = tool();
        let result = t
            .call(serde_json::json!({
                "modelPath": model_path.to_str().unwrap(),
                "models": [{
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsertAux",
                            "payload": {
                                "aux": { "name": "growth_rate", "equation": "0.05" }
                            }
                        },
                        {
                            "type": "upsertStock",
                            "payload": {
                                "stock": {
                                    "name": "population",
                                    "initialEquation": "100",
                                    "inflows": ["births"],
                                    "outflows": []
                                }
                            }
                        },
                        {
                            "type": "upsertFlow",
                            "payload": {
                                "flow": {
                                    "name": "births",
                                    "equation": "population * growth_rate"
                                }
                            }
                        }
                    ]
                }]
            }))
            .unwrap();

        assert_eq!(result["success"], true);
        let proj = &result["project"];
        assert_eq!(proj["name"], "test");

        // Verify the model has the variables
        let model = &proj["models"][0];
        assert!(!model["stocks"].as_array().unwrap().is_empty());
        assert!(!model["flows"].as_array().unwrap().is_empty());
        assert!(!model["auxiliaries"].as_array().unwrap().is_empty());

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
