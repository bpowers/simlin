// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;

fn catalog() -> serde_json::Value {
    serde_json::from_str(catalog_json()).expect("the catalog is JSON")
}

fn entries() -> Vec<serde_json::Value> {
    catalog()["tools"]
        .as_array()
        .expect("the catalog lists its tools")
        .clone()
}

/// `catalog.json` must equal what `generate_catalog_json` produces today -- a
/// drift guard, not a generator: the test writes only when asked
/// (`UPDATE_TOOL_CATALOG=1`), so a stale file fails the build rather than being
/// rewritten mid-run, the crate's `UPDATE_SCHEMA` convention.
#[cfg(feature = "schema")]
#[test]
fn tool_catalog_matches_the_checked_in_file() {
    let generated = generate_catalog_json();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tools/catalog.json");
    if std::env::var_os("UPDATE_TOOL_CATALOG").is_some() {
        std::fs::write(&path, &generated).expect("failed to write the catalog");
        return;
    }
    assert_eq!(
        generated,
        catalog_json(),
        "src/tools/catalog.json is stale; regenerate with \
         `UPDATE_TOOL_CATALOG=1 cargo test -p simlin-engine tool_catalog_matches_the_checked_in_file`"
    );
}

#[test]
fn the_catalog_lists_every_tool_once_in_order_with_its_description_and_effect() {
    let listed: Vec<String> = entries()
        .iter()
        .map(|entry| entry["name"].as_str().expect("a name").to_string())
        .collect();
    let expected: Vec<String> = ToolName::ALL
        .iter()
        .map(|tool| tool.name().to_string())
        .collect();
    assert_eq!(listed, expected);
    for (tool, entry) in ToolName::ALL.into_iter().zip(entries()) {
        assert_eq!(entry["description"], tool.description(), "{}", tool.name());
        assert_eq!(
            entry["effect"],
            serde_json::to_value(tool.effect()).unwrap(),
            "{}",
            tool.name()
        );
    }
}

#[test]
fn a_tool_name_round_trips_and_an_unknown_name_names_no_tool() {
    for tool in ToolName::ALL {
        assert_eq!(ToolName::from_name(tool.name()), Some(tool));
    }
    for unknown in ["", "ReadModel", "read-model", "read_models"] {
        assert_eq!(ToolName::from_name(unknown), None, "{unknown:?}");
    }
}

#[test]
fn every_schema_compiles_and_every_input_is_an_object_that_refuses_unknown_fields() {
    for entry in entries() {
        let name = entry["name"].as_str().unwrap().to_string();
        for key in ["inputSchema", "outputSchema"] {
            assert!(
                jsonschema::validator_for(&entry[key]).is_ok(),
                "{name}'s {key} does not compile"
            );
        }
        let input = &entry["inputSchema"];
        assert_eq!(input["type"], "object", "{name}");
        let validator = jsonschema::validator_for(input).unwrap();
        assert!(
            !validator.is_valid(&serde_json::json!({"notAField": 1})),
            "{name}'s input schema accepts an unknown field"
        );
    }
}
