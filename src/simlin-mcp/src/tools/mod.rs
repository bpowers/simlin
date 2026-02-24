// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP tool implementations for Simlin.
//!
//! Three tools are exposed:
//!
//! - `ReadModel`: Read a model file and return its JSON representation.
//! - `EditModel`: Apply a patch to an existing model file.
//! - `CreateModel`: Create a new empty model file.

mod create_model;
mod edit_model;
mod read_model;

use std::io::BufReader;
use std::path::Path;

use crate::tool::Registry;

/// Register all Simlin MCP tools in the given registry.
pub fn register_all(registry: &mut Registry) {
    registry.register(Box::new(read_model::tool()));
    registry.register(Box::new(edit_model::tool()));
    registry.register(Box::new(create_model::tool()));
}

/// Open a project from file contents, detecting format by extension.
fn open_project(path: &Path, contents: &str) -> anyhow::Result<simlin_engine::datamodel::Project> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "stmx" | "xmile" | "xml" => {
            let mut reader = BufReader::new(contents.as_bytes());
            simlin_engine::open_xmile(&mut reader)
                .map_err(|e| anyhow::anyhow!("failed to parse XMILE: {e:?}"))
        }
        "mdl" => simlin_engine::open_vensim(contents)
            .map_err(|e| anyhow::anyhow!("failed to parse Vensim: {e:?}")),
        _ => {
            let json_project: simlin_engine::json::Project = serde_json::from_str(contents)
                .map_err(|e| anyhow::anyhow!("failed to parse model as JSON: {e}"))?;
            Ok(json_project.into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_all() {
        let mut registry = Registry::new();
        register_all(&mut registry);

        assert!(registry.get("read_model").is_some());
        assert!(registry.get("edit_model").is_some());
        assert!(registry.get("CreateModel").is_some());

        let defs = registry.definitions();
        assert_eq!(defs.len(), 3);
    }

    #[test]
    fn test_all_tools_have_valid_schemas() {
        let mut registry = Registry::new();
        register_all(&mut registry);

        for def in registry.definitions() {
            assert!(
                def.input_schema.is_object(),
                "tool {} should have an object schema",
                def.name
            );
            assert_eq!(
                def.input_schema["type"], "object",
                "tool {} schema type should be 'object'",
                def.name
            );
            assert!(
                def.input_schema["properties"].is_object(),
                "tool {} should have properties",
                def.name
            );
        }
    }
}
