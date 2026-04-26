// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
//! MCP tool implementations for Simlin.
//!
//! Three tools are exposed:
//!
//! - `ReadModel`: Read a model file and return its JSON representation.
//! - `EditModel`: Apply a patch to an existing model file.
//! - `CreateModel`: Create a new empty model file.
//!
//! Shared logic (`open_project`, `resolve_model_name`, `ensure_variable_uids`,
//! `SourceFormat`, output types) lives in `simlin-mcp-core`.  This module
//! re-exports those symbols so the existing tool implementations keep
//! their `super::open_project(...)` call sites unchanged while the binary
//! transitions to the rmcp-based async surface.

mod create_model;
mod edit_model;
mod read_model;
pub mod types;

// Re-export shared logic from simlin-mcp-core so existing call sites
// (`super::open_project`, `super::resolve_model_name`, `super::SourceFormat`)
// in this binary's tool wrappers compile unchanged.  The binary's sync
// `Tool::call` path keeps using these helpers; the new async library
// functions use them directly from `simlin-mcp-core` after Task 8.
pub(crate) use simlin_mcp_core::open::{open_project, resolve_model_name};
pub(crate) use simlin_mcp_core::types::SourceFormat;

use crate::tool::Registry;

/// Register all Simlin MCP tools in the given registry.
pub fn register_all(registry: &mut Registry) {
    registry.register(Box::new(read_model::tool()));
    registry.register(Box::new(edit_model::tool()));
    registry.register(Box::new(create_model::tool()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_all() {
        let mut registry = Registry::new();
        register_all(&mut registry);

        assert!(registry.get("ReadModel").is_some());
        assert!(registry.get("EditModel").is_some());
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
