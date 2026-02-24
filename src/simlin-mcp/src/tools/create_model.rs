// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `create_model` MCP tool: create a new empty model file.

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json as ejson;

use crate::tool::TypedTool;

/// Input for the `create_model` tool.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateModelInput {
    /// Path where the new model file should be created.
    /// Must end in `.simlin.json`.
    pub model_path: String,

    /// Name of the project.
    pub project_name: String,

    /// Optional simulation specifications.  If omitted, defaults are
    /// used (start=0, end=100, dt=1, euler method).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim_specs: Option<ejson::SimSpecs>,

    /// Optional list of model names to create within the project.
    /// If omitted, a single model named "main" is created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_names: Option<Vec<String>>,
}

/// Output from the `create_model` tool.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateModelOutput {
    success: bool,
    model_path: String,
    project: ejson::Project,
}

pub fn tool() -> TypedTool<CreateModelInput> {
    TypedTool {
        name: "create_model",
        description: "Create a new empty system dynamics model file. \
             Produces a Simlin JSON file with the specified project name, \
             simulation specs, and model structure.",
        handler: handle_create_model,
    }
}

fn handle_create_model(input: CreateModelInput) -> anyhow::Result<serde_json::Value> {
    let path = std::path::Path::new(&input.model_path);

    // Don't overwrite existing files
    if path.exists() {
        anyhow::bail!("file already exists: {}", input.model_path);
    }

    let sim_specs = input.sim_specs.unwrap_or(ejson::SimSpecs {
        start_time: 0.0,
        end_time: 100.0,
        dt: "1".to_string(),
        save_step: 1.0,
        method: "euler".to_string(),
        time_units: String::new(),
    });

    let model_names = input
        .model_names
        .unwrap_or_else(|| vec!["main".to_string()]);

    let models: Vec<ejson::Model> = model_names
        .into_iter()
        .map(|name| ejson::Model {
            name,
            stocks: vec![],
            flows: vec![],
            auxiliaries: vec![],
            modules: vec![],
            sim_specs: None,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
        })
        .collect();

    let project = ejson::Project {
        name: input.project_name,
        sim_specs,
        models,
        dimensions: vec![],
        units: vec![],
        source: None,
    };

    // Ensure parent directory exists
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let json_str = serde_json::to_string_pretty(&project)?;
    std::fs::write(path, &json_str)
        .with_context(|| format!("failed to write model to {}", input.model_path))?;

    let output = CreateModelOutput {
        success: true,
        model_path: input.model_path,
        project,
    };
    serde_json::to_value(output).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    #[test]
    fn test_create_model_schema() {
        let t = tool();
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");

        let props = &schema["properties"];
        assert!(props["modelPath"].is_object());
        assert!(props["projectName"].is_object());
        assert!(props["simSpecs"].is_object());
        assert!(props["modelNames"].is_object());
    }

    #[test]
    fn test_create_model_schema_sim_specs_fields() {
        let t = tool();
        let schema = t.input_schema();
        let schema_str = serde_json::to_string_pretty(&schema).unwrap();

        // SimSpecs fields should be visible in the schema
        assert!(schema_str.contains("startTime"));
        assert!(schema_str.contains("endTime"));
    }

    #[test]
    fn test_create_model_success() {
        let t = tool();
        let dir = std::env::temp_dir().join("simlin-mcp-test-create");
        let _ = std::fs::remove_dir_all(&dir);
        let model_path = dir.join("test.simlin.json");

        let result = t
            .call(serde_json::json!({
                "modelPath": model_path.to_str().unwrap(),
                "projectName": "Test Project",
            }))
            .unwrap();

        assert_eq!(result["success"], true);
        assert!(model_path.exists());

        // Verify the file is valid JSON
        let contents = std::fs::read_to_string(&model_path).unwrap();
        let project: ejson::Project = serde_json::from_str(&contents).unwrap();
        assert_eq!(project.name, "Test Project");
        assert_eq!(project.models.len(), 1);
        assert_eq!(project.models[0].name, "main");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_model_already_exists() {
        let t = tool();
        let dir = std::env::temp_dir().join("simlin-mcp-test-exists");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let model_path = dir.join("existing.simlin.json");
        std::fs::write(&model_path, "{}").unwrap();

        let result = t.call(serde_json::json!({
            "modelPath": model_path.to_str().unwrap(),
            "projectName": "Test",
        }));
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
