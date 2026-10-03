// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Each tool's input schema as a host marking the tool `"strict": true` for
//! Anthropic's strict tool use sends it.
//!
//! Strict tool use constrains a model's tool call to its input schema, and
//! accepts a subset of JSON Schema. The subset, from the "JSON Schema
//! limitations" section of
//! <https://platform.claude.com/docs/en/build-with-claude/structured-outputs>:
//! the basic types, `enum`, `const`, `anyOf`, `allOf` (not with `$ref`),
//! `$ref` and `$defs` (no recursive schema), `default`, `required`,
//! `additionalProperties` set to `false` on every object, the string formats
//! [`STRICT_FORMATS`], and `minItems` of 0 or 1 only; numeric, string-length
//! and other array constraints are not supported, and `oneOf` is not listed.
//! The same page limits a request to 20 strict tools and, across every
//! strict tool in it, to 24 parameters left out of `required` and 16
//! parameters that use `anyOf` or a type array ([`strict_cost`] is a tool's
//! share). OpenAI's strict mode is
//! unverified: nothing here is checked against it.
//!
//! The catalog's schemas are the tools' contract: what the engine accepts,
//! which hosts and tests validate inputs and outputs against. This is a second
//! rendering of the same schemas, derived from the checked-in catalog when it
//! is asked for, not a second catalog: strict tool use cannot express the
//! whole contract (an answer's item bounds are real limits the engine
//! refuses past), so a catalog in the strict subset would understate what the
//! engine checks, and a second checked-in file could drift from the first.
//! The rendering changes only what strict tool use cannot take, each change
//! keeping what the schema means to the tool:
//!
//! - `oneOf` becomes `anyOf`. Every `oneOf` the catalog has is of variants no
//!   value matches two of (each a different constant, or an object whose tag
//!   property or required property no other variant can have), so the two
//!   accept the same inputs; a test holds every `oneOf` to that.
//! - An item bound strict tool use refuses (`maxItems`, `minItems` above 1)
//!   is left out and said in the schema's description (unless the
//!   description says it already), as Anthropic's SDK helpers do with a
//!   constraint they remove; the engine still refuses an input past it.
//! - A `$ref` keeps the `description` beside it, which JSON Schema 2020-12
//!   allows and the catalog's schemas carry for every field of a type of its
//!   own. Whether strict tool use reads a `description` beside a `$ref` is
//!   unverified: the page this module cites lists `$ref` and says nothing of
//!   its siblings, so nothing here depends on it being read.
//! - A `format` outside [`STRICT_FORMATS`] (`double`, `uint`: the Rust type a
//!   number came from) is left out; its type says what the tool reads.
//! - `title` is left out: the page names `description` and not `title`.
//! - A property an object does not require that is `null` or a value
//!   (`"type": ["string", "null"]`, or `anyOf` a value and `null`) is the
//!   value alone: the tool reads a missing property and a `null` one alike,
//!   so the `null` arm lets the call say nothing it cannot say by leaving the
//!   property out, and each such arm is a union the request limit counts.

use std::sync::OnceLock;

use serde_json::{Map, Value};

use super::catalog::{ToolName, catalog_json};

/// The string formats strict tool use accepts, from the page this module
/// cites.
pub const STRICT_FORMATS: [&str; 10] = [
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];

/// `tool`'s input schema in the subset strict tool use accepts.
pub fn strict_input_schema(tool: ToolName) -> &'static Value {
    static RENDERED: OnceLock<Vec<Value>> = OnceLock::new();
    let rendered = RENDERED.get_or_init(|| {
        let catalog: Value =
            serde_json::from_str(catalog_json()).expect("the embedded catalog is JSON");
        ToolName::ALL
            .into_iter()
            .map(|tool| {
                let entry = catalog["tools"]
                    .as_array()
                    .and_then(|tools| tools.iter().find(|entry| entry["name"] == tool.name()))
                    .expect("the catalog lists every tool");
                strict(&entry["inputSchema"])
            })
            .collect()
    });
    let index = ToolName::ALL
        .iter()
        .position(|t| *t == tool)
        .expect("every tool is in ToolName::ALL");
    &rendered[index]
}

/// What a tool's strict schema adds to the request limits strict tool use
/// sets across every strict tool in a request.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct StrictCost {
    /// Properties an object leaves out of `required`, at every depth.
    pub optional: usize,
    /// Schemas that are an `anyOf` (or, in a schema not yet rendered, a
    /// `oneOf`) or a type array, at every depth.
    pub unions: usize,
}

/// `tool`'s share of the request limits, counted over its strict schema. The
/// page counts "parameters" and does not say whether one inside a `$defs`
/// entry, an `anyOf` variant or an array's items counts; this counts every
/// optional property and every union schema wherever it is, the reading
/// that never undercounts.
pub fn strict_cost(tool: ToolName) -> StrictCost {
    let mut cost = StrictCost::default();
    count(strict_input_schema(tool), &mut cost);
    cost
}

fn count(schema: &Value, cost: &mut StrictCost) {
    let Some(object) = schema.as_object() else {
        return;
    };
    cost.unions += usize::from(
        object.contains_key("anyOf")
            || object.contains_key("oneOf")
            || object.get("type").is_some_and(Value::is_array),
    );
    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
        let required = required_of(object);
        cost.optional += properties
            .keys()
            .filter(|name| !required.contains(&name.as_str()))
            .count();
    }
    for child in subschemas(object) {
        count(child, cost);
    }
}

/// The names `object` requires.
fn required_of(object: &Map<String, Value>) -> Vec<&str> {
    object
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The schemas a schema holds: the values of `properties` and `$defs`, its
/// `items` and `additionalProperties` when they are schemas, and the
/// variants of its `anyOf`, `oneOf` and `allOf`. A keyword's data (`enum`,
/// `const`, `default`) and a property's name are never schemas, so a
/// property called `format` or `description` is not read as one.
pub(crate) fn subschemas(object: &Map<String, Value>) -> Vec<&Value> {
    let mut children = Vec::new();
    for key in ["properties", "$defs"] {
        if let Some(map) = object.get(key).and_then(Value::as_object) {
            children.extend(map.values());
        }
    }
    for key in ["items", "additionalProperties"] {
        if let Some(child) = object.get(key).filter(|v| v.is_object()) {
            children.push(child);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = object.get(key).and_then(Value::as_array) {
            children.extend(variants);
        }
    }
    children
}

/// `schema` rendered for strict tool use (the module's rules), in the
/// catalog's property order.
fn strict(schema: &Value) -> Value {
    let Some(object) = schema.as_object() else {
        return schema.clone();
    };
    let required = required_of(object);
    let mut bounds: Vec<String> = Vec::new();
    let min_items = object.get("minItems").and_then(Value::as_u64);
    let max_items = object.get("maxItems").and_then(Value::as_u64);
    match (min_items.filter(|&n| n > 1), max_items) {
        (Some(min), Some(max)) if min == max => bounds.push(format!("Exactly {min} items.")),
        (Some(min), Some(max)) => bounds.push(format!("From {min} to {max} items.")),
        (Some(min), None) => bounds.push(format!("At least {min} items.")),
        (None, Some(max)) => bounds.push(format!("At most {max} items.")),
        (None, None) => {}
    }
    // A description that states the bound already (the field's own doc
    // says "At most 12.") is not told it twice.
    let described = object
        .get("description")
        .and_then(Value::as_str)
        .map(|text| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .unwrap_or_default();
    if let Some(max) = max_items
        && described.contains(&format!("at most {max}"))
        && min_items.is_none_or(|n| n <= 1)
    {
        bounds.clear();
    }

    let mut out = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "oneOf" | "anyOf" | "allOf" => {
                let variants = value
                    .as_array()
                    .map_or_else(Vec::new, |variants| variants.iter().map(strict).collect());
                let key = if key == "oneOf" {
                    "anyOf"
                } else {
                    key.as_str()
                };
                out.insert(key.to_string(), Value::Array(variants));
            }
            "properties" => {
                let properties = value.as_object().map_or_else(Map::new, |properties| {
                    properties
                        .iter()
                        .map(|(name, property)| {
                            let property = if required.contains(&name.as_str()) {
                                strict(property)
                            } else {
                                strict(&without_null(property))
                            };
                            (name.clone(), property)
                        })
                        .collect()
                });
                out.insert(key.clone(), Value::Object(properties));
            }
            "$defs" => {
                let defs = value.as_object().map_or_else(Map::new, |defs| {
                    defs.iter()
                        .map(|(name, def)| (name.clone(), strict(def)))
                        .collect()
                });
                out.insert(key.clone(), Value::Object(defs));
            }
            "items" | "additionalProperties" => {
                out.insert(key.clone(), strict(value));
            }
            "minItems" if min_items.is_some_and(|n| n > 1) => {}
            "maxItems" | "title" => {}
            "format" if !value.as_str().is_some_and(|f| STRICT_FORMATS.contains(&f)) => {}
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    if !bounds.is_empty() {
        let description = match out.get("description").and_then(Value::as_str) {
            Some(description) => format!("{description} {}", bounds.join(" ")),
            None => bounds.join(" "),
        };
        out.insert("description".to_string(), Value::String(description));
    }
    Value::Object(out)
}

/// An optional property's schema without its `null` arm: a type array's
/// `"null"` (one type left is the type alone), an `anyOf`'s `{"type":
/// "null"}` variant (one variant left is that variant, beside the property's
/// own description), and a `default` of `null`, which then names a value the
/// schema no longer has.
fn without_null(property: &Value) -> Value {
    let Some(object) = property.as_object() else {
        return property.clone();
    };
    let is_null = |v: &Value| v.get("type").and_then(Value::as_str) == Some("null");
    let mut out = Map::new();
    let mut lone_variant: Option<Value> = None;
    for (key, value) in object {
        match (key.as_str(), value) {
            ("type", Value::Array(types)) => {
                let kept: Vec<Value> = types
                    .iter()
                    .filter(|t| t.as_str() != Some("null"))
                    .cloned()
                    .collect();
                let kept = match <[Value; 1]>::try_from(kept) {
                    Ok([one]) => one,
                    Err(kept) => Value::Array(kept),
                };
                out.insert(key.clone(), kept);
            }
            ("anyOf", Value::Array(variants)) if variants.iter().any(is_null) => {
                let kept: Vec<Value> = variants.iter().filter(|v| !is_null(v)).cloned().collect();
                match <[Value; 1]>::try_from(kept) {
                    Ok([one]) => lone_variant = Some(one),
                    Err(kept) => {
                        out.insert(key.clone(), Value::Array(kept));
                    }
                }
            }
            ("default", Value::Null) => {}
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    if let Some(Value::Object(variant)) = lone_variant {
        // The variant's own keywords, under the property's description.
        for (key, value) in variant {
            out.entry(key).or_insert(value);
        }
    }
    Value::Object(out)
}

#[cfg(test)]
#[path = "strict_schema_tests.rs"]
mod tests;
