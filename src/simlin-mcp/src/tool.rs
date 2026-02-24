// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tool trait, registry, and helper macro for defining MCP tools.
//!
//! Each tool declares its name, description, and JSON Schema for its
//! input (derived automatically from Rust types via `schemars`).  The
//! registry collects tools and produces the `tools/list` response.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::protocol::ToolDefinition;

// ── Tool trait ───────────────────────────────────────────────────────

/// A single MCP tool that can be listed and called.
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;

    /// JSON Schema for the tool's input, as a `serde_json::Value`.
    /// Should be an object with `"type": "object"` at the top level.
    fn input_schema(&self) -> Value;

    /// Execute the tool with the given JSON input.  The input has
    /// already been normalized (null/missing → `{}`).
    fn call(&self, input: Value) -> anyhow::Result<Value>;
}

// ── Registry ─────────────────────────────────────────────────────────

/// Thread-safe registry mapping tool names to implementations.
pub struct Registry {
    tools: Vec<Box<dyn Tool>>,
}

impl Registry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// Produce the list of tool definitions for `tools/list`.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
            })
            .collect()
    }
}

// ── Schema helper ────────────────────────────────────────────────────

/// Generate the MCP-compatible input schema for a type that implements
/// `JsonSchema`.  Inlines subschemas and strips the outer
/// `$schema`/`title` keys so the result is a plain JSON Schema object
/// with `properties`, `required`, `type`, and `$defs`.
pub fn input_schema_for<T: JsonSchema>() -> Value {
    let settings = schemars::generate::SchemaSettings::draft2019_09().with(|s| {
        s.inline_subschemas = true;
    });
    let generator = settings.into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let value = serde_json::to_value(&schema).expect("schema serialization should never fail");

    // Keep only the keys MCP clients expect.
    let obj = match value {
        Value::Object(map) => map,
        _ => unreachable!("schema must be an object"),
    };
    let mut out = serde_json::Map::new();
    for key in ["type", "properties", "required", "$defs", "definitions"] {
        if let Some(v) = obj.get(key) {
            out.insert(key.to_string(), v.clone());
        }
    }
    Value::Object(out)
}

// ── define_tool! macro ───────────────────────────────────────────────

/// Define an MCP tool with automatic JSON Schema generation from the
/// input type.
///
/// # Example
///
/// ```ignore
/// use schemars::JsonSchema;
/// use serde::Deserialize;
///
/// #[derive(Deserialize, JsonSchema)]
/// struct MyInput {
///     /// The name of the thing
///     name: String,
/// }
///
/// define_tool! {
///     name: "my_tool",
///     description: "does something useful",
///     input: MyInput,
///     handler: |input: MyInput| {
///         Ok(serde_json::json!({ "result": input.name }))
///     },
/// }
/// ```
///
/// This generates a struct (named by CamelCase-ing the tool name) that
/// implements `Tool`.  The input type must implement `Deserialize` and
/// `JsonSchema`.  The handler closure receives the deserialized input
/// and returns `anyhow::Result<serde_json::Value>`.
#[macro_export]
macro_rules! define_tool {
    (
        name: $name:expr,
        description: $desc:expr,
        input: $input_ty:ty,
        handler: $handler:expr $(,)?
    ) => {
        // Use paste-style name mangling via concat_idents isn't stable,
        // so we use a nested module to avoid name collisions.
        mod _tool_impl {
            use super::*;
            use $crate::tool::{Tool, input_schema_for};

            pub struct Instance;

            impl Tool for Instance {
                fn name(&self) -> &str {
                    $name
                }
                fn description(&self) -> &str {
                    $desc
                }
                fn input_schema(&self) -> serde_json::Value {
                    input_schema_for::<$input_ty>()
                }
                fn call(&self, input: serde_json::Value) -> anyhow::Result<serde_json::Value> {
                    let parsed: $input_ty = serde_json::from_value(input)?;
                    let handler: fn($input_ty) -> anyhow::Result<serde_json::Value> = $handler;
                    handler(parsed)
                }
            }
        }
    };
}

/// Helper to create a boxed tool from a typed handler function.
/// This is the non-macro approach for when tools need state or more
/// complex setup.
pub struct TypedTool<I> {
    pub name: &'static str,
    pub description: &'static str,
    pub handler: fn(I) -> anyhow::Result<Value>,
}

impl<I: JsonSchema + DeserializeOwned + 'static> Tool for TypedTool<I> {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        self.description
    }

    fn input_schema(&self) -> Value {
        input_schema_for::<I>()
    }

    fn call(&self, input: Value) -> anyhow::Result<Value> {
        let parsed: I = serde_json::from_value(input)?;
        (self.handler)(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::Deserialize;

    #[derive(Deserialize, JsonSchema)]
    struct TestInput {
        /// A greeting message
        msg: String,
    }

    #[test]
    fn test_input_schema_for() {
        let schema = input_schema_for::<TestInput>();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["msg"].is_object());
        assert_eq!(schema["properties"]["msg"]["type"], "string");
        // schemars extracts doc comments as descriptions
        assert_eq!(
            schema["properties"]["msg"]["description"],
            "A greeting message"
        );
    }

    #[test]
    fn test_typed_tool() {
        let tool = TypedTool::<TestInput> {
            name: "greet",
            description: "greets someone",
            handler: |input| Ok(serde_json::json!({ "greeting": format!("hello {}", input.msg) })),
        };

        assert_eq!(tool.name(), "greet");
        assert_eq!(tool.description(), "greets someone");

        let schema = tool.input_schema();
        assert_eq!(schema["type"], "object");

        let result = tool.call(serde_json::json!({ "msg": "world" })).unwrap();
        assert_eq!(result["greeting"], "hello world");
    }

    #[test]
    fn test_typed_tool_bad_input() {
        let tool = TypedTool::<TestInput> {
            name: "greet",
            description: "greets someone",
            handler: |_| Ok(serde_json::json!({})),
        };

        // Missing required field should fail deserialization
        let result = tool.call(serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn test_registry() {
        let mut registry = Registry::new();
        let tool = TypedTool::<TestInput> {
            name: "greet",
            description: "greets someone",
            handler: |_| Ok(serde_json::json!({})),
        };
        registry.register(Box::new(tool));

        assert!(registry.get("greet").is_some());
        assert!(registry.get("nonexistent").is_none());

        let defs = registry.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "greet");
    }
}
