// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The session: dispatch, refusals, the model a session is bound to, and the
//! agreement between what every tool returns and the schema the catalog
//! publishes for it. Each tool's own behavior is tested beside it.

use serde_json::{Value, json};

use super::test_support::{Host, inventory};
use super::*;

/// A call every tool answers on the inventory fixture: its rows are the
/// catalog's tools, so a tool added without one fails here.
fn good_input(tool: ToolName) -> Value {
    match tool {
        ToolName::ReadModel => json!({}),
        ToolName::ReadVariables => json!({"names": ["Inventory", "production"]}),
        ToolName::FindVariables => json!({"phrase": "inventory"}),
    }
}

fn catalog_schema(tool: ToolName, key: &str) -> Value {
    let catalog: Value = serde_json::from_str(catalog_json()).unwrap();
    catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == tool.name())
        .unwrap_or_else(|| panic!("{} is in the catalog", tool.name()))[key]
        .clone()
}

#[test]
fn a_tool_the_catalog_does_not_list_is_the_hosts_error() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let err = session
        .call(host.workspace(), "read_everything", "{}")
        .unwrap_err();
    assert_eq!(err, UnknownTool("read_everything".to_string()));
}

#[test]
fn every_tools_input_and_output_agree_with_the_schemas_the_catalog_publishes() {
    for tool in ToolName::ALL {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        let input = good_input(tool);
        let input_schema = jsonschema::validator_for(&catalog_schema(tool, "inputSchema")).unwrap();
        assert!(
            input_schema.is_valid(&input),
            "{}'s test input does not match its own schema",
            tool.name()
        );
        let output = host.call(&mut session, tool.name(), input);
        let output_schema =
            jsonschema::validator_for(&catalog_schema(tool, "outputSchema")).unwrap();
        let errors: Vec<String> = output_schema
            .iter_errors(&output)
            .map(|e| format!("{e} at {}", e.instance_path))
            .collect();
        assert!(
            errors.is_empty(),
            "{}'s output does not match its schema: {errors:?}\n{output}",
            tool.name()
        );
        assert_eq!(output["revision"], 0, "{}", tool.name());
    }
}

#[test]
fn every_tool_refuses_input_its_schema_does_not_allow_naming_itself_and_the_field() {
    for tool in ToolName::ALL {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        let refusal = host.refuse(&mut session, tool.name(), json!({"notAField": 1}));
        let message = refusal["error"].as_str().unwrap();
        assert!(
            message.contains(tool.name()) && message.contains("notAField"),
            "{}: {message}",
            tool.name()
        );
        let refusal = host.refuse(&mut session, tool.name(), json!("not an object"));
        assert!(refusal["error"].is_string(), "{}", tool.name());
    }
}

#[test]
fn an_empty_input_is_an_empty_object() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    for empty in ["", "  \n"] {
        let output = host.call_raw(&mut session, "read_model", empty);
        assert!(!output.is_error, "{empty:?}: {}", output.json);
    }
    // A tool with a required field says so rather than guessing.
    let output = host.call_raw(&mut session, "read_variables", "");
    assert!(output.is_error);
    assert!(output.json.contains("names"), "{}", output.json);
}

#[test]
fn a_session_over_a_model_the_project_lacks_refuses_every_tool_naming_the_models_it_has() {
    for tool in ToolName::ALL {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("elsewhere");
        let refusal = host.refuse(&mut session, tool.name(), good_input(tool));
        assert!(
            refusal["error"].as_str().unwrap().contains("elsewhere"),
            "{}: {refusal}",
            tool.name()
        );
        assert_eq!(refusal["suggestions"], json!(["main"]), "{}", tool.name());
    }
}

#[test]
fn main_names_the_first_model_when_none_is_called_main() {
    let mut project = inventory().build_datamodel();
    project.models[0].name = "Inventory Model".to_string();
    let mut host = Host::new(project);
    for name in ["main", "Inventory Model"] {
        let mut session = Session::new(name);
        let outline = host.call(&mut session, "read_model", json!({}));
        assert_eq!(outline["model"], "Inventory Model", "{name}");
    }
}

#[test]
fn every_output_names_the_revision_it_read() {
    for tool in ToolName::ALL {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        host.edit(|p| {
            p.models[0]
                .get_variable_mut("coverage")
                .unwrap()
                .set_scalar_equation("5")
        });
        host.edit(|_| {});
        let output = host.call(&mut session, tool.name(), good_input(tool));
        assert_eq!(output["revision"], 2, "{}", tool.name());
    }
}

/// A call that other work on the project waits for stops and answers that it
/// kept nothing: a read is not a read, so the change report still waits for
/// the first one, and the ids a later read gives start where they would have.
#[test]
fn a_call_other_work_waits_for_stops_and_keeps_nothing() {
    for tool in ToolName::ALL {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        let refusal = host.call_waited_on(&mut session, tool.name(), good_input(tool));
        assert_eq!(refusal["interrupted"], true, "{}: {refusal}", tool.name());
        assert!(
            refusal["error"].as_str().unwrap().contains("kept nothing"),
            "{}: {refusal}",
            tool.name()
        );
        host.edit(|p| {
            p.models[0]
                .get_variable_mut("coverage")
                .unwrap()
                .set_scalar_equation("5")
        });
        assert_eq!(
            session.changes_since_read(&host.project, host.revision),
            None,
            "{}: an interrupted read is no read",
            tool.name()
        );
        host.call(&mut session, tool.name(), good_input(tool));
    }
}
