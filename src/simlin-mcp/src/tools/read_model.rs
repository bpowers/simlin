// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `read_model` MCP tool: reads a model file and returns its JSON
//! representation.

use anyhow::Context as _;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tool::TypedTool;

/// Input for the `read_model` tool.
#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadModelInput {
    /// Absolute or relative path to the model file (XMILE .stmx/.xmile,
    /// Vensim .mdl, or Simlin .simlin JSON).
    pub model_path: String,

    /// Optional name of a specific model within the project to return.
    /// If omitted, the entire project (all models) is returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_name: Option<String>,
}

pub fn tool() -> TypedTool<ReadModelInput> {
    TypedTool {
        name: "read_model",
        description: "Read a system dynamics model file and return its JSON representation. \
             Supports XMILE (.stmx, .xmile), Vensim (.mdl), and Simlin JSON formats.",
        handler: handle_read_model,
    }
}

fn handle_read_model(input: ReadModelInput) -> anyhow::Result<serde_json::Value> {
    let path = std::path::Path::new(&input.model_path);
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read model file: {}", input.model_path))?;

    let project = super::open_project(path, &contents)?;
    let json_project = simlin_engine::json::Project::from(project.clone());

    if let Some(model_name) = &input.model_name {
        let model = json_project
            .models
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(model_name))
            .with_context(|| format!("model '{}' not found in project", model_name))?;
        serde_json::to_value(model).map_err(Into::into)
    } else {
        serde_json::to_value(&json_project).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::Tool;

    #[test]
    fn test_read_model_schema() {
        let t = tool();
        let schema = t.input_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["modelPath"].is_object());
        assert_eq!(schema["properties"]["modelPath"]["type"], "string");
        assert!(
            schema["properties"]["modelPath"]["description"]
                .as_str()
                .unwrap()
                .contains("path")
        );
    }

    #[test]
    fn test_read_model_missing_file() {
        let t = tool();
        let result = t.call(serde_json::json!({
            "modelPath": "/nonexistent/model.stmx"
        }));
        assert!(result.is_err());
    }

    #[test]
    fn test_read_model_json_file() {
        let t = tool();
        let test_model = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/logistic-growth.sd.json"
        );
        let result = t
            .call(serde_json::json!({ "modelPath": test_model }))
            .unwrap();
        assert_eq!(result["name"], "logistic-growth");
        assert!(result["models"].is_array());
        assert!(!result["models"].as_array().unwrap().is_empty());
    }
}
