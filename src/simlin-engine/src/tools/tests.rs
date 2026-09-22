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
        ToolName::RunExperiment => json!({
            "name": "doubled coverage",
            "set": [{"variable": "coverage", "multiply": 2}],
            "record": ["Inventory", "desired_inventory"]
        }),
        ToolName::ReadBehavior => json!({"variables": ["Inventory", "orders"]}),
        ToolName::ListRuns => json!({}),
        ToolName::AnalyzeLoops => json!({"through": "Inventory"}),
        ToolName::RunTests => json!({"tests": ["units", "extreme_conditions"]}),
        ToolName::VerifyFindings => json!({"findings": [{
            "kind": "observation",
            "claim": "Inventory is a stock the model integrates.",
            "citations": [{"cites": "variable", "variable": "Inventory"}]
        }]}),
        ToolName::EditModel => json!({
            "summary": "Make shipments depend on what is on hand.",
            "operations": [
                {"op": "set_equation", "variable": "shipments", "equation": "MIN(orders, Inventory)"}
            ]
        }),
    }
}

/// What a call of `tool` needs the session to have done first: an edit is
/// planned against what the session read.
fn prepare(host: &mut Host, session: &mut Session, tool: ToolName) {
    if tool == ToolName::EditModel {
        host.call(session, "read_model", json!({}));
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
        prepare(&mut host, &mut session, tool);
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
        prepare(&mut host, &mut session, tool);
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
        // An edit is planned against a read; the interrupted call left none.
        if tool == ToolName::EditModel {
            host.call(&mut session, "read_model", json!({}));
        }
        host.call(&mut session, tool.name(), good_input(tool));
    }
}

/// A call its host cancels stops at its first checkpoint and answers that it
/// was cancelled and kept nothing -- not that it was interrupted, which a
/// host calls again -- and a later call, not cancelled, answers.
#[test]
fn a_call_its_host_cancels_stops_keeps_nothing_and_is_not_to_be_retried() {
    for tool in ToolName::ALL {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        let output = host.call_cancelled(
            &mut session,
            tool.name(),
            good_input(tool),
            &|| true,
            &|| false,
        );
        assert!(
            output.is_error && output.cancelled && !output.interrupted,
            "{}: {}",
            tool.name(),
            output.json
        );
        let refusal: Value = serde_json::from_str(&output.json).unwrap();
        assert_eq!(refusal["cancelled"], true, "{}: {refusal}", tool.name());
        assert!(
            refusal.get("interrupted").is_none(),
            "{}: {refusal}",
            tool.name()
        );
        let message = refusal["error"].as_str().unwrap();
        assert!(
            message.contains("cancelled") && message.contains("kept nothing"),
            "{}: {message}",
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
            "{}: a cancelled read is no read",
            tool.name()
        );
        prepare(&mut host, &mut session, tool);
        host.call(&mut session, tool.name(), good_input(tool));
    }
}

/// A cancel that comes during a call's work stops it at the next checkpoint,
/// a slice of a run included, and the call keeps nothing of what it had done:
/// a run it was making is not the session's. A call both cancelled and waited
/// on answers that it was cancelled, since no host is to call it again.
#[test]
fn a_call_cancelled_during_its_work_keeps_nothing_of_it() {
    let input = good_input(ToolName::RunExperiment);
    let mut stopped = 0;
    for after in 1.. {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        let cancelled = Host::waiting_after(after);
        let output = host.call_cancelled(
            &mut session,
            "run_experiment",
            input.clone(),
            &cancelled,
            &|| false,
        );
        if !output.is_error {
            break;
        }
        assert!(output.cancelled && !output.interrupted, "{}", output.json);
        stopped += 1;
        assert!(
            session.runs(&host.workspace()).is_empty(),
            "the run the call was making is not the session's"
        );
    }
    assert!(
        stopped > 2,
        "the call stops between the slices of its runs, not only before them ({stopped})"
    );

    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let output = host.call_cancelled(&mut session, "run_experiment", input, &|| true, &|| true);
    assert!(output.cancelled && !output.interrupted, "{}", output.json);
}

/// A run's results and a landing the host cancels stop at a checkpoint too,
/// and say that they were cancelled; the same asks, not cancelled, answer.
#[test]
fn a_hosts_read_of_a_run_and_a_landing_stop_when_cancelled() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let cancelled = || true;
    let ws = Workspace {
        cancelled: Some(&cancelled),
        ..host.workspace()
    };
    let refused = session.run_results(ws, "current").err().expect("it stops");
    assert!(refused.cancelled && !refused.interrupted, "{refused:?}");
    assert!(refused.reason.contains("cancelled"), "{refused:?}");
    assert!(session.run_results(host.workspace(), "current").is_ok());

    host.call(&mut session, "read_model", json!({}));
    let planned = host.call(
        &mut session,
        "edit_model",
        json!({"summary": "Hold more cover.", "operations": [
            {"op": "set_equation", "variable": "coverage", "equation": "5"}
        ]}),
    );
    let id = planned["plan"].as_str().expect("a plan").to_string();
    // At another revision the plan is planned again, which runs the model.
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("orders")
            .unwrap()
            .set_scalar_equation("10 + STEP(3, 5)")
    });
    let ws = Workspace {
        cancelled: Some(&cancelled),
        ..host.workspace()
    };
    match session.land_plan(ws, &id) {
        Some(Landing::Refused(reason)) => assert!(reason.contains("cancelled"), "{reason}"),
        _ => panic!("a cancelled landing lands nothing"),
    }
    host.land(&mut session, &id).expect("the plan lands");
}

/// The ids a cancelled call gave out on the way are not the session's: a
/// finding verified after a call a cancel stopped between two findings takes
/// the id the first would have had.
#[test]
fn a_cancelled_call_gives_out_no_ids() {
    let finding = |claim: &str, variable: &str| {
        json!({"kind": "observation", "claim": claim,
               "citations": [{"cites": "variable", "variable": variable}]})
    };
    let first = finding("Inventory is a stock the model integrates.", "Inventory");
    let second = finding("Orders drive production.", "orders");
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    // The call asks at its start and after each citation; the cancel comes
    // after the first finding has its id.
    let cancelled = Host::waiting_after(2);
    let output = host.call_cancelled(
        &mut session,
        "verify_findings",
        json!({"findings": [first, second.clone()]}),
        &cancelled,
        &|| false,
    );
    assert!(output.cancelled, "{}", output.json);
    let verified = host.call(
        &mut session,
        "verify_findings",
        json!({"findings": [second]}),
    );
    assert_eq!(verified["findings"][0]["id"], "F1", "{verified}");
}
