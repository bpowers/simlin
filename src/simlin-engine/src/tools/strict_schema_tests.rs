// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::*;

/// The keywords a strict schema may hold: the subset the page the module
/// cites lists (`format`, `minItems` and `additionalProperties` only with the
/// values it allows, checked below), plus `description`, the annotation it
/// names.
const ALLOWED: [&str; 14] = [
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
    "anyOf",
    "allOf",
    "$ref",
    "$defs",
    "default",
    "description",
    "minItems",
];

fn input_schema_in_catalog(tool: ToolName) -> Value {
    let catalog: Value = serde_json::from_str(catalog_json()).unwrap();
    catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == tool.name())
        .unwrap()["inputSchema"]
        .clone()
}

/// Every schema in `root`, depth first, with the path to it.
fn every_schema(root: &Value) -> Vec<(String, &Value)> {
    let mut found = Vec::new();
    let mut stack = vec![(String::new(), root)];
    while let Some((path, schema)) = stack.pop() {
        found.push((path.clone(), schema));
        if let Some(object) = schema.as_object() {
            for (i, child) in subschemas(object).into_iter().enumerate() {
                stack.push((format!("{path}/{i}"), child));
            }
        }
    }
    found
}

/// What breaks the strict subset in one schema, if anything.
fn strict_violations(schema: &Value) -> Vec<String> {
    let Some(object) = schema.as_object() else {
        return vec![];
    };
    let mut violations: Vec<String> = object
        .keys()
        .filter(|key| !ALLOWED.contains(&key.as_str()) && key.as_str() != "format")
        .map(|key| format!("`{key}`"))
        .collect();
    if let Some(format) = object.get("format")
        && !format.as_str().is_some_and(|f| STRICT_FORMATS.contains(&f))
    {
        violations.push(format!("format {format}"));
    }
    if let Some(min) = object.get("minItems")
        && min.as_u64().is_none_or(|n| n > 1)
    {
        violations.push(format!("minItems {min}"));
    }
    let is_object = object.get("type") == Some(&json!("object"));
    if is_object && object.get("additionalProperties") != Some(&json!(false)) {
        violations.push("an object without additionalProperties false".to_string());
    }
    if let Some(extra) = object.get("additionalProperties")
        && extra != &json!(false)
    {
        violations.push(format!("additionalProperties {extra}"));
    }
    if let Some(all) = object.get("allOf").and_then(Value::as_array)
        && all.iter().any(|v| v.get("$ref").is_some())
    {
        violations.push("allOf with $ref".to_string());
    }
    violations
}

/// Every `$ref` in `root`, from the `$defs` entry (or the root, `""`) it is
/// in to the entry it names.
fn references(root: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut body = root.clone();
    let defs = body
        .as_object_mut()
        .and_then(|object| object.remove("$defs"))
        .unwrap_or_else(|| json!({}));
    let mut scopes = vec![(String::new(), body)];
    scopes.extend(
        defs.as_object()
            .into_iter()
            .flatten()
            .map(|(name, def)| (name.clone(), def.clone())),
    );
    scopes
        .iter()
        .map(|(scope, schema)| {
            let targets = every_schema(schema)
                .into_iter()
                .filter_map(|(_, inner)| inner.get("$ref").and_then(Value::as_str))
                .map(|target| {
                    target
                        .strip_prefix("#/$defs/")
                        .unwrap_or_else(|| panic!("{target} is a local definition"))
                        .to_string()
                })
                .collect();
            (scope.clone(), targets)
        })
        .collect()
}

/// A row per tool: its strict schema holds only what strict tool use
/// accepts, every `$ref` names a definition the schema has, and no
/// definition reaches itself (strict tool use takes no recursive schema).
#[test]
fn every_strict_schema_holds_only_what_strict_tool_use_accepts() {
    for tool in ToolName::ALL {
        let schema = strict_input_schema(tool);
        assert_eq!(schema["type"], "object", "{}", tool.name());
        for (path, inner) in every_schema(schema) {
            let violations = strict_violations(inner);
            assert!(
                violations.is_empty(),
                "{} at {path}: {violations:?} in {inner}",
                tool.name()
            );
        }

        let graph = references(schema);
        for (scope, targets) in &graph {
            for target in targets {
                assert!(
                    graph.contains_key(target),
                    "{}: {scope} names {target}, which is not defined",
                    tool.name()
                );
            }
        }
        for start in graph.keys() {
            let mut seen = BTreeSet::new();
            let mut stack: Vec<&String> = graph[start].iter().collect();
            while let Some(next) = stack.pop() {
                assert!(
                    next != start || start.is_empty(),
                    "{}: {start} reaches itself",
                    tool.name()
                );
                if seen.insert(next.clone()) {
                    stack.extend(graph.get(next).into_iter().flatten());
                }
            }
        }
    }
}

/// The value a schema pins, if it pins one: a `const`, or an `enum` of one.
fn pinned(schema: &Value) -> Option<&Value> {
    schema.get("const").or_else(|| {
        schema
            .get("enum")
            .and_then(Value::as_array)
            .filter(|values| values.len() == 1)
            .map(|values| &values[0])
    })
}

/// Whether no value matches both `a` and `b`: they pin different values, or
/// they are objects one of which pins a property to a value the other pins
/// differently, or requires a property the other, closed to properties it
/// does not list, cannot have.
fn disjoint(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (pinned(a), pinned(b)) {
        return x != y;
    }
    let properties = |s: &Value| s.get("properties").and_then(Value::as_object).cloned();
    let (Some(pa), Some(pb)) = (properties(a), properties(b)) else {
        return false;
    };
    let tagged_apart = pa.iter().any(|(name, sa)| {
        pb.get(name)
            .is_some_and(|sb| matches!((pinned(sa), pinned(sb)), (Some(x), Some(y)) if x != y))
    });
    let required = |s: &Value| -> Vec<String> {
        s.get("required")
            .and_then(Value::as_array)
            .map(|r| {
                r.iter()
                    .filter_map(|n| n.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let closed = |s: &Value| s.get("additionalProperties") == Some(&json!(false));
    let needs_what_other_lacks =
        |s: &Value, other_properties: &serde_json::Map<String, Value>, other: &Value| {
            closed(other)
                && required(s)
                    .iter()
                    .any(|n| !other_properties.contains_key(n))
        };
    tagged_apart || needs_what_other_lacks(a, &pb, b) || needs_what_other_lacks(b, &pa, a)
}

/// `oneOf` becomes `anyOf` in the strict rendering, which accepts the same
/// inputs only when no input matches two variants: every `oneOf` in the
/// catalog's input schemas is of pairwise disjoint variants.
#[test]
fn every_one_of_in_the_catalog_is_of_variants_no_input_matches_two_of() {
    let mut checked = 0;
    for tool in ToolName::ALL {
        let schema = input_schema_in_catalog(tool);
        for (path, inner) in every_schema(&schema) {
            let Some(variants) = inner.get("oneOf").and_then(Value::as_array) else {
                continue;
            };
            checked += 1;
            for (i, a) in variants.iter().enumerate() {
                for b in &variants[i + 1..] {
                    assert!(
                        disjoint(a, b),
                        "{} at {path}: {a} and {b} can match one input",
                        tool.name()
                    );
                }
            }
        }
    }
    assert!(checked > 0, "the catalog has a oneOf to check");
}

/// The disjointness the test above relies on, a row per way two variants are
/// or are not apart, so that it cannot pass by answering `true`.
#[test]
fn variants_are_apart_by_a_pinned_value_a_tag_or_a_property_the_other_cannot_have() {
    let closed = |required: &[&str], properties: Value| {
        json!({"type": "object", "properties": properties, "required": required,
               "additionalProperties": false})
    };
    for (a, b, apart) in [
        (json!({"const": "x"}), json!({"enum": ["y"]}), true),
        (json!({"const": "x"}), json!({"enum": ["x"]}), false),
        (json!({"type": "string"}), json!({"const": "x"}), false),
        (
            closed(&["op"], json!({"op": {"const": "add"}})),
            closed(&["op"], json!({"op": {"const": "set"}})),
            true,
        ),
        (
            closed(&["v", "value"], json!({"v": {}, "value": {}})),
            closed(&["v", "multiply"], json!({"v": {}, "multiply": {}})),
            true,
        ),
        (
            closed(&["v"], json!({"v": {}, "value": {}})),
            closed(&["v"], json!({"v": {}, "multiply": {}})),
            false,
        ),
        (
            json!({"type": "object", "properties": {"v": {}, "value": {}}, "required": ["v", "value"]}),
            json!({"type": "object", "properties": {"v": {}}, "required": ["v"]}),
            false,
        ),
    ] {
        assert_eq!(disjoint(&a, &b), apart, "{a} and {b}");
        assert_eq!(disjoint(&b, &a), apart, "{b} and {a}");
    }
}

/// A row per rule of the rendering, on one schema that has each.
#[test]
fn the_rendering_changes_only_what_strict_tool_use_cannot_take() {
    let schema = json!({
        "title": "Input",
        "type": "object",
        "properties": {
            "choice": {"oneOf": [{"const": "a"}, {"const": "b"}]},
            "pair": {"type": "array", "items": {"type": "number", "format": "double"},
                     "minItems": 2, "maxItems": 2},
            "names": {"description": "Names.", "type": "array", "items": {"type": "string"},
                      "minItems": 1, "maxItems": 12},
            "when": {"type": "string", "format": "date-time"},
            "note": {"type": ["string", "null"], "default": null},
            "kind": {"description": "A kind.", "anyOf": [{"$ref": "#/$defs/Kind"}, {"type": "null"}],
                     "default": null},
            "needed": {"type": ["string", "null"]},
            "description": {"type": "string", "format": "uint"}
        },
        "required": ["choice", "needed"],
        "additionalProperties": false,
        "$defs": {"Kind": {"title": "Kind", "type": "string", "enum": ["x", "y"]}}
    });
    assert_eq!(
        strict(&schema),
        json!({
            "type": "object",
            "properties": {
                "choice": {"anyOf": [{"const": "a"}, {"const": "b"}]},
                "pair": {"type": "array", "items": {"type": "number"},
                         "description": "Exactly 2 items."},
                "names": {"description": "Names. At most 12 items.", "type": "array",
                          "items": {"type": "string"}, "minItems": 1},
                "when": {"type": "string", "format": "date-time"},
                "note": {"type": "string"},
                "kind": {"description": "A kind.", "$ref": "#/$defs/Kind"},
                "needed": {"type": ["string", "null"]},
                "description": {"type": "string"}
            },
            "required": ["choice", "needed"],
            "additionalProperties": false,
            "$defs": {"Kind": {"type": "string", "enum": ["x", "y"]}}
        })
    );
}

/// What a tool takes it still takes in the strict rendering, each tool's
/// sample calls included; an item bound left out is the engine's to refuse.
#[test]
fn the_strict_schema_accepts_every_call_the_catalog_does_without_a_null() {
    let mut item_bounds = 0;
    for tool in ToolName::ALL {
        let strict = jsonschema::validator_for(strict_input_schema(tool)).unwrap();
        let catalog = jsonschema::validator_for(&input_schema_in_catalog(tool)).unwrap();
        for input in [
            crate::tools::tests::good_input(tool),
            crate::tools::tests::busy_call(tool),
        ] {
            assert!(catalog.is_valid(&input), "{}: {input}", tool.name());
            let errors: Vec<String> = strict.iter_errors(&input).map(|e| e.to_string()).collect();
            assert!(errors.is_empty(), "{}: {input}: {errors:?}", tool.name());
        }
        // One name too many for a bounded list: the catalog refuses it, the
        // strict schema says the bound in words.
        let catalog_schema = input_schema_in_catalog(tool);
        for (name, property) in catalog_schema["properties"]
            .as_object()
            .into_iter()
            .flatten()
        {
            let Some(max) = property.get("maxItems").and_then(Value::as_u64) else {
                continue;
            };
            if property["items"]["type"] != "string" {
                continue;
            }
            item_bounds += 1;
            let mut input = crate::tools::tests::good_input(tool);
            let names: Vec<String> = (0..=max).map(|i| format!("v{i}")).collect();
            input[name] = json!(names);
            assert!(!catalog.is_valid(&input), "{}.{name}", tool.name());
            assert!(strict.is_valid(&input), "{}.{name}", tool.name());
            let description = strict_input_schema(tool)["properties"][name]["description"]
                .as_str()
                .unwrap_or_default();
            // Said once, by the field's doc or by the rendering.
            let said = description
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
                .matches(&format!("at most {max}"))
                .count();
            assert_eq!(said, 1, "{}.{name}: {description}", tool.name());
        }
    }
    assert!(item_bounds > 0, "some tool bounds a list of names");
}

/// The request limits a tool's strict schema counts toward, on a schema
/// with one of each thing counted and each thing that is not.
#[test]
fn a_strict_cost_counts_optional_and_union_properties_at_every_depth() {
    let schema = json!({
        "type": "object",
        "properties": {
            "required_plain": {"type": "string"},
            "optional_plain": {"type": "string"},
            "required_union": {"type": ["string", "null"]},
            "optional_any": {"anyOf": [{"const": 1}, {"const": 2}]},
            "list": {"type": "array", "items": {"$ref": "#/$defs/Item"}}
        },
        "required": ["required_plain", "required_union", "list"],
        "$defs": {"Item": {"type": "object", "properties": {
            "tag": {"const": "x"}, "extra": {"type": "number"}
        }, "required": ["tag"]}}
    });
    let mut cost = StrictCost::default();
    count(&schema, &mut cost);
    assert_eq!(
        cost,
        StrictCost {
            optional: 3,
            unions: 2
        }
    );
}

/// A tool with nothing optional costs nothing, and the rendering lowers the
/// union count of every tool with an optional `null`able property.
#[test]
fn the_strict_rendering_costs_no_more_than_the_catalog() {
    for tool in ToolName::ALL {
        let mut catalog_cost = StrictCost::default();
        count(&input_schema_in_catalog(tool), &mut catalog_cost);
        let strict_cost = strict_cost(tool);
        assert!(
            strict_cost.optional == catalog_cost.optional
                && strict_cost.unions <= catalog_cost.unions,
            "{}: {strict_cost:?} against {catalog_cost:?}",
            tool.name()
        );
    }
    assert_eq!(strict_cost(ToolName::ReadModel), StrictCost::default());
}

/// A bound strict tool use cannot take is said in the description once: a
/// field whose own doc says it already (`operations`, "At most 24.") is not
/// told it again, and one whose doc does not is.
#[test]
fn a_bound_the_description_says_is_not_said_twice() {
    let edit = strict_input_schema(ToolName::EditModel);
    let operations = edit["properties"]["operations"]["description"]
        .as_str()
        .unwrap()
        .to_lowercase();
    let said = operations
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .matches("at most 24")
        .count();
    assert_eq!(said, 1, "{operations}");
    // A bound no doc states is said by the rendering.
    let rendered = strict(&json!({"type": "array", "maxItems": 3, "description": "Names."}));
    assert_eq!(rendered["description"], "Names. At most 3 items.");
}
