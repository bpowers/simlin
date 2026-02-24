// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `CreateModel` MCP tool: create a new empty model file.

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use simlin_engine::json as ejson;

use crate::tool::TypedTool;

/// Input for the `CreateModel` tool.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateModelInput {
    /// Path where the new `.simlin.json` file should be created.
    pub project_path: String,

    /// Optional simulation specifications.  If omitted, defaults are
    /// used (start=0, end=100, dt=1, euler method).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim_specs: Option<ejson::SimSpecs>,
}

/// Output from the `CreateModel` tool.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateModelOutput {
    project_path: String,
    sim_specs: ejson::SimSpecs,
    model_name: String,
}

pub fn tool() -> TypedTool<CreateModelInput> {
    TypedTool {
        name: "CreateModel",
        description: "Create a new empty system dynamics model file. \
             Produces a Simlin JSON file at the given path with a single \
             \"main\" model and the specified simulation specs.",
        handler: handle_create_model,
    }
}

fn handle_create_model(input: CreateModelInput) -> anyhow::Result<serde_json::Value> {
    let path = std::path::Path::new(&input.project_path);

    if path.exists() {
        anyhow::bail!("file already exists: {}", input.project_path);
    }

    let sim_specs = input.sim_specs.unwrap_or(ejson::SimSpecs {
        start_time: 0.0,
        end_time: 100.0,
        dt: "1".to_string(),
        save_step: 1.0,
        method: "euler".to_string(),
        time_units: String::new(),
    });

    // Derive the project name from the filename stem, stripping the
    // `.simlin.json` double-extension when present.
    let project_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            n.strip_suffix(".simlin.json")
                .unwrap_or_else(|| n.strip_suffix(".json").unwrap_or(n))
                .to_string()
        })
        .unwrap_or_else(|| "project".to_string());

    let model_name = "main".to_string();

    let models = vec![ejson::Model {
        name: model_name.clone(),
        stocks: vec![],
        flows: vec![],
        auxiliaries: vec![],
        modules: vec![],
        sim_specs: None,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
    }];

    let project = ejson::Project {
        name: project_name,
        sim_specs: sim_specs.clone(),
        models,
        dimensions: vec![],
        units: vec![],
        source: None,
    };

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let json_str = serde_json::to_string_pretty(&project)?;
    std::fs::write(path, &json_str)
        .with_context(|| format!("failed to write model to {}", input.project_path))?;

    let output = CreateModelOutput {
        project_path: input.project_path,
        sim_specs,
        model_name,
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
        assert!(props["projectPath"].is_object());
        assert!(props["simSpecs"].is_object());
        // Old fields must be gone
        assert!(props["modelPath"].is_null());
        assert!(props["projectName"].is_null());
        assert!(props["modelNames"].is_null());
    }

    #[test]
    fn test_create_model_schema_sim_specs_fields() {
        let t = tool();
        let schema = t.input_schema();
        let schema_str = serde_json::to_string_pretty(&schema).unwrap();

        assert!(schema_str.contains("startTime"));
        assert!(schema_str.contains("endTime"));
    }

    #[test]
    fn test_create_model_success_default_specs() {
        let t = tool();
        let dir = std::env::temp_dir().join("simlin-mcp-test-create");
        let _ = std::fs::remove_dir_all(&dir);
        let project_path = dir.join("my-model.simlin.json");

        let result = t
            .call(serde_json::json!({
                "projectPath": project_path.to_str().unwrap(),
            }))
            .unwrap();

        // Output must have the new shape
        assert_eq!(result["projectPath"], project_path.to_str().unwrap());
        assert_eq!(result["modelName"], "main");
        assert!(project_path.exists());

        let contents = std::fs::read_to_string(&project_path).unwrap();
        let project: ejson::Project = serde_json::from_str(&contents).unwrap();
        // Project name derived from filename stem
        assert_eq!(project.name, "my-model");
        assert_eq!(project.models.len(), 1);
        assert_eq!(project.models[0].name, "main");

        // Default sim specs
        assert_eq!(project.sim_specs.start_time, 0.0);
        assert_eq!(project.sim_specs.end_time, 100.0);
        assert_eq!(project.sim_specs.dt, "1");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_model_custom_sim_specs() {
        let t = tool();
        let dir = std::env::temp_dir().join("simlin-mcp-test-create-custom");
        let _ = std::fs::remove_dir_all(&dir);
        let project_path = dir.join("custom.simlin.json");

        let result = t
            .call(serde_json::json!({
                "projectPath": project_path.to_str().unwrap(),
                "simSpecs": {
                    "startTime": 10.0,
                    "endTime": 200.0,
                    "dt": "0.5",
                    "saveStep": 1.0,
                    "method": "euler",
                    "timeUnits": "",
                },
            }))
            .unwrap();

        assert_eq!(result["modelName"], "main");

        let contents = std::fs::read_to_string(&project_path).unwrap();
        let project: ejson::Project = serde_json::from_str(&contents).unwrap();
        assert_eq!(project.sim_specs.start_time, 10.0);
        assert_eq!(project.sim_specs.end_time, 200.0);
        assert_eq!(project.sim_specs.dt, "0.5");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_create_model_already_exists() {
        let t = tool();
        let dir = std::env::temp_dir().join("simlin-mcp-test-exists");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let project_path = dir.join("existing.simlin.json");
        std::fs::write(&project_path, "{}").unwrap();

        let result = t.call(serde_json::json!({
            "projectPath": project_path.to_str().unwrap(),
        }));
        assert!(result.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
