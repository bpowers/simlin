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
pub(super) fn good_input(tool: ToolName) -> Value {
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
            "summary": "Ship no more than the cover on hand allows.",
            "operations": [
                {"op": "set_equation", "variable": "shipments",
                 "equation": "MIN(orders, Inventory / coverage)"}
            ]
        }),
    }
}

/// What a call of `tool` needs the session to have done first: an edit is
/// made of what the session read.
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

/// One row per way input fails to be what a tool takes: the refusal names
/// the place in the input, in the input's own terms, and never a line and a
/// column of text the agent did not write.
#[test]
fn a_refusal_of_input_names_where_in_the_input_the_mismatch_is() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    for (tool, input, place, reason) in [
        (
            "run_experiment",
            json!({"name": "x", "set": [
                {"variable": "coverage", "value": 5},
                {"variable": "a {tricky}, \"name\" [1]", "value": "5"}
            ]}),
            Some("set[1].value"),
            "invalid type: string \"5\", expected a number",
        ),
        (
            "edit_model",
            json!({"summary": "x", "operations": [
                {"op": "set_units", "variable": "coverage", "units": "month"},
                {"op": "set_equation", "variable": "coverage", "eqn": "5"}
            ]}),
            Some("operations[1].eqn"),
            "unknown field `eqn`",
        ),
        (
            "edit_model",
            json!({"summary": "x", "operations": [
                {"op": "set_equation", "variable": "coverage", "eqn": "5"},
                {"op": "set_units", "variable": "coverage", "units": "month"}
            ]}),
            Some("operations[0].eqn"),
            "unknown field `eqn`",
        ),
        (
            "verify_findings",
            json!({"findings": [{"kind": "flaw", "claim": "x", "citations": [
                {"cites": "variable", "variable": "coverage"},
                {"cites": "value", "variable": "coverage", "value": "four"}
            ]}]}),
            Some("findings[0].citations[1].value"),
            "invalid type: string \"four\"",
        ),
        (
            "edit_model",
            json!({"summary": "x", "operations": [
                {"op": "set_units", "variable": "coverage", "units": "month"},
                {"op": "add_stock", "name": "s", "initial": 5}
            ]}),
            Some("operations[1].initial"),
            "invalid type: integer `5`, expected a string",
        ),
        (
            "edit_model",
            json!({"summary": "x", "operations": [
                {"op": "set_lookup", "variable": "coverage", "points": [[0, 0], [1, "a"]]}
            ]}),
            Some("operations[0].points"),
            "invalid type: string \"a\", expected a number",
        ),
        (
            "edit_model",
            json!({"summary": "x", "operations": [
                {"op": 3, "variable": "coverage", "equation": "5"}
            ]}),
            Some("operations[0].op"),
            "invalid type: integer `3`, expected a name",
        ),
        (
            "edit_model",
            json!({"summary": "x", "operations": [["set_equation", "coverage", "8"]]}),
            Some("operations[0]"),
            "invalid type: sequence",
        ),
        (
            "edit_model",
            json!(["a summary", [{"op": "set_equation", "variable": "coverage", "equation": "7"}]]),
            None,
            "invalid type: sequence, expected an object",
        ),
        (
            "read_variables",
            json!([["coverage"]]),
            None,
            "invalid type: sequence, expected an object",
        ),
        (
            "read_model",
            json!([]),
            None,
            "invalid type: sequence, expected an object",
        ),
        (
            "read_variables",
            json!({"names": "coverage"}),
            Some("names"),
            "invalid type: string \"coverage\", expected a sequence",
        ),
        (
            "run_experiment",
            json!({"set": []}),
            None,
            "missing field `name`",
        ),
        (
            "find_variables",
            json!({"phrase": "x", "limit": 3}),
            Some("limit"),
            "unknown field `limit`",
        ),
    ] {
        let refusal = host.refuse(&mut session, tool, input.clone());
        let message = refusal["error"].as_str().unwrap();
        let expected = match place {
            Some(place) => {
                format!("the input does not match {tool}'s schema at `{place}`: {reason}")
            }
            None => format!("the input does not match {tool}'s schema: {reason}"),
        };
        assert!(message.starts_with(&expected), "{input}: {message}");
        assert!(
            !message.contains(" line ") && !message.contains("column"),
            "{message}"
        );
    }
    for (text, says) in [
        ("{\"a\": ", "the input to read_model is not JSON: "),
        (
            "{\"names\": [\"coverage\"], \"names\": [\"shipments\"]}",
            "the input to read_variables is not JSON: the key `names` appears twice",
        ),
    ] {
        let tool = if text.contains("names") {
            "read_variables"
        } else {
            "read_model"
        };
        let output = host.call_raw(&mut session, tool, text);
        let refusal: Value = serde_json::from_str(&output.json).unwrap();
        assert!(
            output.is_error && refusal["error"].as_str().unwrap().starts_with(says),
            "{refusal}"
        );
    }
}

/// Every refusal has the one shape the catalog publishes, whichever tool
/// makes it and whatever it carries: a row per tool for input it does not
/// take, and one each for a refusal with suggestions, a call that stopped
/// for other work, a call its host cancelled, and an edit its gate refused.
#[test]
fn every_refusal_matches_the_schema_the_catalog_publishes() {
    let catalog: Value = serde_json::from_str(catalog_json()).unwrap();
    let schema = &catalog["refusalSchema"];
    assert_eq!(schema["additionalProperties"], false, "{schema}");
    let validator = jsonschema::validator_for(schema).unwrap();
    let mut refusals: Vec<(String, Value)> = Vec::new();
    for tool in ToolName::ALL {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        refusals.push((
            format!("{} refusing its input", tool.name()),
            host.refuse(&mut session, tool.name(), json!({"notAField": 1})),
        ));
    }
    let mut host = Host::from_test_project(&inventory());
    let suggested = host.refuse(&mut Session::new("elsewhere"), "read_model", json!({}));
    assert!(suggested["suggestions"].is_array());
    refusals.push(("with suggestions".to_string(), suggested));
    let stopped = host.call_waited_on(&mut Session::new("main"), "read_model", json!({}));
    assert_eq!(stopped["interrupted"], true);
    refusals.push(("interrupted".to_string(), stopped));
    let cancelled = host.call_cancelled(
        &mut Session::new("main"),
        "read_model",
        json!({}),
        &|| true,
        &|| false,
    );
    let cancelled: Value = serde_json::from_str(&cancelled.json).unwrap();
    assert_eq!(cancelled["cancelled"], true);
    refusals.push(("cancelled".to_string(), cancelled));
    let mut session = Session::new("main");
    host.call(&mut session, "read_model", json!({}));
    let gated = host.refuse(&mut session, "edit_model", busy_call(ToolName::EditModel));
    assert!(gated["refusedEdit"]["rule"].is_string(), "{gated}");
    refusals.push(("an edit its gate refuses".to_string(), gated));

    for (label, refusal) in refusals {
        let errors: Vec<String> = validator
            .iter_errors(&refusal)
            .map(|e| e.to_string())
            .collect();
        assert!(errors.is_empty(), "{label}: {errors:?}: {refusal}");
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

/// `"main"` names the first model a person wrote: a macro's template, which
/// a project may list first, is no model to read.
#[test]
fn main_names_the_first_model_that_is_not_a_macro() {
    let mut project = inventory().build_datamodel();
    project.models[0].name = "Inventory Model".to_string();
    let body = |name: &str, equation: &str| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: name.to_string(),
            equation: datamodel::Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    };
    project.models.insert(
        0,
        datamodel::Model {
            name: "doubled".to_string(),
            sim_specs: None,
            variables: [body("x", "0"), body("out", "x * 2")].into_iter().collect(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: Some(datamodel::MacroSpec {
                parameters: vec!["x".to_string()],
                primary_output: "out".to_string(),
                additional_outputs: vec![],
            }),
        },
    );
    let mut host = Host::new(project);
    let outline = host.call(&mut Session::new("main"), "read_model", json!({}));
    assert_eq!(outline["model"], "Inventory Model", "{outline}");
    assert_eq!(outline["counts"]["stocks"], 1);
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
            session
                .changes_since_read(&host.project, host.revision)
                .unwrap(),
            None,
            "{}: an interrupted read is no read",
            tool.name()
        );
        // An edit is made of a read; the interrupted call left none.
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
            session
                .changes_since_read(&host.project, host.revision)
                .unwrap(),
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
            session.runs(&host.project, host.revision).is_empty(),
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

/// A read of a run's results and an edit the host cancels stop at a
/// checkpoint too, and say that they were cancelled; a cancelled edit is not
/// made. The same asks, not cancelled, answer.
#[test]
fn a_hosts_read_of_a_run_and_an_edit_stop_when_cancelled() {
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
    let edit = json!({"summary": "Hold more cover.", "operations": [
        {"op": "set_equation", "variable": "coverage", "equation": "5"}
    ]});
    let output = host.call_cancelled(
        &mut session,
        "edit_model",
        edit.clone(),
        &cancelled,
        &|| false,
    );
    assert!(output.cancelled && output.is_error, "{}", output.json);
    assert!(output.edited.is_none(), "a cancelled edit is not made");
    assert_eq!(host.revision, 0);
    host.call(&mut session, "edit_model", edit);
    assert_eq!(host.revision, 1);
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

/// What a later call answers shows everything a session keeps: the ids it
/// has given (diagnostics, loops, checks, findings), its named runs and what
/// it last read. Each answer is compared byte for byte.
fn answers(host: &mut Host, session: &mut Session, calls: &[(&str, Value)]) -> Vec<String> {
    calls
        .iter()
        .map(|(tool, input)| host.call_raw(session, tool, &input.to_string()).json)
        .collect()
}

/// A call of every tool, stopped at each of `checkpoints` it reaches (in
/// rising order, and no further once the call runs to its end), leaves its
/// session answering `follow_up` exactly as a session in which the call was
/// never made: a call that stops keeps nothing. The rows are the catalog's
/// tools times the checkpoints.
///
/// The rows share a host until one changes its project (a call that ran to
/// its end, or an edit among the answers compared), so each stopped call is
/// also followed on the db it stopped on: what it staged there is gone.
fn stopped_calls_keep_nothing(
    fixture: &crate::test_common::TestProject,
    call_of: impl Fn(ToolName) -> Value,
    follow_up: &[(&str, Value)],
    checkpoints: impl Fn() -> Box<dyn Iterator<Item = usize>>,
) {
    let read_first = |host: &mut Host| {
        let mut session = Session::new("main");
        // An edit is made of what the session last read, so every session
        // reads first.
        host.call(&mut session, "read_model", json!({}));
        session
    };
    let mut host = Host::from_test_project(fixture);
    let mut untouched = read_first(&mut host);
    let expected = answers(&mut host, &mut untouched, follow_up);

    for tool in ToolName::ALL {
        let mut stopped = 0;
        for after in checkpoints() {
            if host.revision != 0 {
                host = Host::from_test_project(fixture);
            }
            let mut session = read_first(&mut host);
            let waiting = Host::waiting_after(after);
            let output = host.call_waiting(&mut session, tool.name(), call_of(tool), &waiting);
            if !output.interrupted {
                break;
            }
            stopped += 1;
            let got = answers(&mut host, &mut session, follow_up);
            for (step, (got, expected)) in got.iter().zip(&expected).enumerate() {
                assert_eq!(
                    got,
                    expected,
                    "{} stopped at its checkpoint {after}: the answer to {} after it differs",
                    tool.name(),
                    follow_up[step].0
                );
            }
        }
        assert!(stopped > 0, "{} asks at least once", tool.name());
    }
}

/// A call that gives ids to several things and makes runs on the way, for
/// each tool: what a stop in the middle of it could leave behind.
pub(super) fn busy_call(tool: ToolName) -> Value {
    match tool {
        ToolName::ReadModel => json!({}),
        ToolName::ReadVariables => json!({"names": ["Inventory", "production"]}),
        ToolName::FindVariables => json!({"phrase": "inventory"}),
        ToolName::RunExperiment => json!({
            "name": "half adjustment",
            "set": [{"variable": "adjustment_time", "multiply": 0.5},
                    {"variable": "orders", "equation": "10 + STEP(4, 5)"}],
        }),
        ToolName::ReadBehavior => json!({"variables": ["Inventory", "orders"]}),
        ToolName::ListRuns => json!({}),
        ToolName::AnalyzeLoops => json!({}),
        ToolName::RunTests => json!({"tests": ["extreme_conditions", "disturbance"]}),
        ToolName::VerifyFindings => json!({"findings": [
            {"kind": "observation", "claim": "Inventory is a stock.",
             "citations": [{"cites": "variable", "variable": "Inventory"}]},
            {"kind": "observation", "claim": "No loop corrects coverage.",
             "citations": [{"cites": "no_loop_through", "variable": "coverage"}]},
            {"kind": "observation", "claim": "Inventory settles.",
             "citations": [{"cites": "behavior_mode", "variable": "Inventory", "mode": "goal_seeking"}]}
        ]}),
        ToolName::EditModel => json!({
            "summary": "Ship what is on hand.",
            "operations": [
                {"op": "set_equation", "variable": "shipments", "equation": "MIN(orders, Inventory)"}
            ]
        }),
    }
}

/// The inventory model with a unit warning, so a read gives a diagnostic id.
fn warned() -> crate::test_common::TestProject {
    inventory().aux("mislabeled", "orders * coverage", Some("month"))
}

/// The calls whose answers carry every kind of id and the session's runs.
fn every_store() -> Vec<(&'static str, Value)> {
    vec![
        ("read_model", json!({})),
        (
            "run_experiment",
            json!({"name": "doubled coverage", "set": [{"variable": "coverage", "multiply": 2}]}),
        ),
        ("list_runs", json!({})),
        ("analyze_loops", json!({})),
        (
            "run_tests",
            json!({"tests": ["extreme_conditions", "sensitivity"]}),
        ),
        (
            "verify_findings",
            json!({"findings": [{"kind": "observation", "claim": "Orders step up.",
                "citations": [{"cites": "variable", "variable": "orders"}]}]}),
        ),
        (
            "edit_model",
            json!({"summary": "More cover.", "operations": [
                {"op": "set_equation", "variable": "coverage", "equation": "5"}]}),
        ),
    ]
}

/// Each call stopped at each of its first three checkpoints and at one well into its work,
/// with answers that show the diagnostic, loop and finding ids and the runs.
/// Every checkpoint, and every tool's answer after it (the check ids and an
/// edit among them), is the gate below.
#[test]
fn a_call_stopped_at_a_checkpoint_leaves_the_session_as_it_was() {
    let follow_up: Vec<(&str, Value)> = every_store()
        .into_iter()
        .filter(|(tool, _)| !matches!(*tool, "run_tests" | "run_experiment" | "edit_model"))
        .collect();
    let quick = |tool: ToolName| match tool {
        ToolName::RunTests => json!({"tests": ["disturbance"], "targets": ["Inventory"]}),
        other => busy_call(other),
    };
    stopped_calls_keep_nothing(&warned(), quick, &follow_up, || {
        Box::new([0, 1, 2, 12].into_iter())
    });
}

#[test]
#[ignore = "every tool stopped at every checkpoint it reaches, each followed by every tool; run under the gates profile"]
fn a_call_stopped_at_any_checkpoint_leaves_the_session_as_it_was() {
    let full = |tool: ToolName| match tool {
        ToolName::RunTests => json!({}),
        other => busy_call(other),
    };
    stopped_calls_keep_nothing(&warned(), full, &every_store(), || Box::new(0..));
}

/// The record, both citations and the battery's default targets answer what a
/// model reads from one owner (`analysis::model_reads`), so a constant read
/// only as the model starts is read for all of them: `s0` sets where a stock
/// starts, and `base_rate` a rate frozen at the start (`INIT`). A step after
/// the start moves neither, so the battery disturbs neither by default.
#[test]
fn a_constant_read_only_at_the_start_is_read_for_every_consumer() {
    let mut host = Host::from_test_project(&test_support::initial_reads());
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    // Sensitivity lists its strongest responses whether or not they flag
    // anything, so what it lists is what it changed.
    let battery = host.call(&mut session, "run_tests", json!({"tests": ["sensitivity"]}));
    let mut targets: Vec<&str> = battery["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|result| result["variable"].as_str())
        .collect();
    targets.sort_unstable();
    targets.dedup();
    assert_eq!(targets, ["base_rate", "s0"], "{battery}");
    let disturbed = host.call(&mut session, "run_tests", json!({"tests": ["disturbance"]}));
    assert_eq!(
        disturbed["tests"][0]["skipped"], "the model has nothing this test changes",
        "{disturbed}"
    );
    for (constant, reader) in [("base_rate", "rate"), ("s0", "level")] {
        let record = host.call(&mut session, "read_variables", json!({"names": [constant]}));
        assert_eq!(
            record["variables"][0]["readers"][0]["name"], reader,
            "{record}"
        );

        let verdict = host.call(
            &mut session,
            "verify_findings",
            json!({"findings": [
                {"kind": "flaw", "claim": format!("Nothing reads {constant}."),
                 "citations": [{"cites": "readers", "variable": constant, "readers": []}]},
                {"kind": "observation", "claim": format!("{reader} reads {constant}."),
                 "citations": [{"cites": "reads", "variable": reader, "reads": constant}]},
                {"kind": "observation", "claim": format!("Only {reader} reads {constant}."),
                 "citations": [{"cites": "readers", "variable": constant, "readers": [reader]}]}
            ]}),
        );
        assert_eq!(verdict["findings"][0]["holds"], false, "{verdict}");
        assert_eq!(
            verdict["findings"][0]["failures"][0]["reason"],
            format!("{constant} is read by {reader}"),
        );
        assert_eq!(verdict["findings"][1]["holds"], true, "{verdict}");
        assert_eq!(verdict["findings"][2]["holds"], true, "{verdict}");
    }
}

/// The shape of a generated model: how many of each thing it has, how long
/// its names are, and the budget its answers are held to.
#[derive(Debug, Clone)]
struct Shape {
    stocks: usize,
    auxes: usize,
    sectors: usize,
    name_chars: usize,
    documentation_chars: usize,
    /// Equations that do not compile, each its own error.
    errors: usize,
    budget: usize,
}

/// A model of `shape`: stocks with a flow each, auxiliaries that each read
/// every stock (so a stock has many readers and an auxiliary many inputs and
/// a long equation), sectors of a few members each with every third one
/// empty, and `errors` equations that do not compile.
fn generated(shape: &Shape) -> datamodel::Project {
    let pad = "n".repeat(shape.name_chars);
    let stock = |i: usize| format!("{pad}_stock_{i}");
    let mut project = crate::test_common::TestProject::new("generated");
    for i in 0..shape.stocks {
        let flow = format!("{pad}_flow_{i}");
        project = project.stock(&stock(i), "1", &[&flow], &[], None).flow(
            &flow,
            &format!("{} * 0.1", stock(i)),
            None,
        );
    }
    let sum = (0..shape.stocks)
        .map(stock)
        .chain(["1".to_string()])
        .collect::<Vec<_>>()
        .join(" + ");
    for i in 0..shape.auxes {
        project = project.aux(&format!("{pad}_aux_{i}"), &sum, None);
    }
    for i in 0..shape.errors {
        project = project.aux(&format!("{pad}_broken_{i}"), "no_such_variable + 1", None);
    }
    let mut project = project.build_datamodel();
    let model = &mut project.models[0];
    let names: Vec<String> = model
        .variables
        .iter()
        .map(|var| var.get_ident().to_string())
        .collect();
    for name in &names {
        if let Some(var) = model.get_variable_mut(name) {
            var.set_documentation(&"d".repeat(shape.documentation_chars));
        }
    }
    model.groups = (0..shape.sectors)
        .map(|i| datamodel::ModelGroup {
            name: format!("{pad} sector {i}"),
            members: if i % 3 == 0 || names.is_empty() {
                vec![]
            } else {
                (0..3)
                    .map(|j| names[(i + j) % names.len()].clone())
                    .collect()
            },
            ..Default::default()
        })
        .collect();
    project
}

/// The most bytes an answer takes under a budget smaller than the least it
/// can say: a refusal cut to its floor (`ToolOutput::refusal`,
/// `evidence::ECHO_CHARS`), an edit's summary with its first line and
/// diagnostic, one run's one variable's behavior, one finding's verdict. Each
/// tool fits what it lists to the budget and keeps the first of each list,
/// so this floor is fixed whatever the model's size; the production budget
/// (`OUTLINE_BUDGET`) is ten times it. The outline and the records are held
/// to the budget itself: they refuse what does not fit.
const ANSWER_FLOOR: usize = 1_200;

/// What every answer must be under `shape`'s budget: within it, or within
/// [`ANSWER_FLOOR`] when the budget is below that -- except the read tools'
/// answers, which are within the budget itself or refused.
fn check_within_budget(shape: &Shape, tool: &str, output: &ToolOutput) -> Result<(), String> {
    let strict = !output.is_error && matches!(tool, "read_model" | "read_variables");
    let limit = if strict {
        shape.budget
    } else {
        shape.budget.max(ANSWER_FLOOR)
    };
    if output.json.len() > limit {
        return Err(format!(
            "{tool}{} under {shape:?} is {} bytes: {}",
            if output.is_error { "'s refusal" } else { "" },
            output.json.len(),
            output.json
        ));
    }
    Ok(())
}

/// Every tool's answers to `shape`'s model, each asked what makes its
/// answer longest (every name, every reader, a short phrase that matches
/// many, a failing citation per finding, an edit that breaks the model's
/// every reader), checked: each keeps to the budget, refusals included, and
/// an outline never lists an entry while it leaves an error out.
fn check_shape(shape: &Shape) -> Result<(), String> {
    let project = generated(shape);
    let names: Vec<String> = project.models[0]
        .variables
        .iter()
        .map(|var| var.get_ident().to_string())
        .collect();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    session.outline_budget = shape.budget;
    let mut call = |tool: &str, input: Value| {
        let output = host.call_raw(&mut session, tool, &input.to_string());
        check_within_budget(shape, tool, &output).map(|()| output)
    };

    let outline = call("read_model", json!({}))?;
    if !outline.is_error {
        let outline: Value = serde_json::from_str(&outline.json).map_err(|e| e.to_string())?;
        let errors = outline["diagnostics"]
            .as_array()
            .map_or(0, |d| d.iter().filter(|d| d["severity"] == "error").count());
        let entries = ["stocks", "flows", "variables", "constants"]
            .iter()
            .map(|list| outline[list].as_array().map_or(0, Vec::len))
            .sum::<usize>();
        if entries > 0 && errors < outline["counts"]["errors"].as_u64().unwrap_or(0) as usize {
            return Err(format!(
                "the outline under {shape:?} lists entries and leaves an error out: {outline}"
            ));
        }
    }
    // The first stock (many readers), the last auxiliary (many inputs, a
    // long equation), and both together with the broken one.
    let first = names.first().cloned().unwrap_or_default();
    let last = names.last().cloned().unwrap_or_default();
    for asked in [
        vec![first.clone()],
        vec![last.clone()],
        vec![first.clone(), last.clone()],
    ] {
        call("read_variables", json!({ "names": asked }))?;
    }
    let twelve: Vec<&String> = names.iter().take(12).collect();
    call("find_variables", json!({ "phrase": "n" }))?;
    call("find_variables", json!({ "phrase": first }))?;
    call(
        "run_experiment",
        json!({"name": "e", "set": [{"variable": first, "multiply": 2}], "record": twelve}),
    )?;
    call(
        "read_behavior",
        json!({ "variables": twelve, "runs": ["current", "e"] }),
    )?;
    call("list_runs", json!({}))?;
    call("analyze_loops", json!({}))?;
    call("run_tests", json!({}))?;
    let readers = json!({"cites": "readers", "variable": first, "readers": []});
    let finding = json!({"kind": "observation", "claim": "a claim", "citations": vec![readers; 8]});
    call("verify_findings", json!({ "findings": vec![finding; 12] }))?;
    // Units that disagree with every reader, and a delete every reader
    // still names: the longest made and refused edits.
    let units: Vec<Value> = names
        .iter()
        .take(24)
        .map(|name| json!({"op": "set_units", "variable": name, "units": "widget"}))
        .collect();
    call(
        "edit_model",
        json!({"summary": "units", "operations": units}),
    )?;
    call(
        "edit_model",
        json!({"summary": "delete", "operations": [{"op": "delete", "variable": first}]}),
    )?;
    Ok(())
}

/// The shapes that break a budget kept by listing: many sectors, long names,
/// many stocks beside an error, long documentation, and more errors than an
/// outline holds -- each through every tool, small enough for the default
/// suite because the budget is small too.
#[test]
fn every_tool_keeps_to_its_budget() {
    let base = Shape {
        stocks: 4,
        auxes: 4,
        sectors: 3,
        name_chars: 4,
        documentation_chars: 0,
        errors: 0,
        budget: 1_500,
    };
    for shape in [
        Shape {
            sectors: 60,
            ..base.clone()
        },
        Shape {
            name_chars: 150,
            stocks: 12,
            auxes: 12,
            ..base.clone()
        },
        Shape {
            stocks: 40,
            errors: 1,
            ..base.clone()
        },
        Shape {
            errors: 40,
            name_chars: 30,
            ..base.clone()
        },
        Shape {
            documentation_chars: 2_000,
            name_chars: 60,
            budget: 700,
            ..base.clone()
        },
    ] {
        check_shape(&shape).unwrap_or_else(|failure| panic!("{failure}"));
    }
}

mod generated_models {
    use proptest::prelude::*;

    use super::{Shape, check_shape};

    fn shape() -> impl Strategy<Value = Shape> {
        (
            0usize..80,
            0usize..40,
            0usize..120,
            1usize..200,
            0usize..1_500,
            prop_oneof![Just(0usize), 1usize..4, 4usize..120],
            150usize..6_000,
        )
            .prop_map(
                |(stocks, auxes, sectors, name_chars, documentation_chars, errors, budget)| Shape {
                    stocks,
                    auxes,
                    sectors,
                    name_chars,
                    documentation_chars,
                    errors,
                    budget,
                },
            )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        #[test]
        #[ignore = "every tool over 200 generated models; run under the gates profile"]
        fn every_tool_keeps_to_its_budget_on_generated_models(shape in shape()) {
            prop_assert_eq!(check_shape(&shape), Ok(()));
        }
    }
}

/// A refusal keeps to its budget whatever it carries: a reason written at
/// length is cut to `MAX_REFUSAL_CHARS`, suggestions to `MAX_NAMED`, and
/// under a budget smaller than both, suggestions go and the reason is
/// halved toward `evidence::ECHO_CHARS`.
#[test]
fn a_refusal_keeps_to_its_budget_whatever_it_carries() {
    let error = ToolError::new("a reason ".repeat(2_000))
        .with_suggestions((0..30).map(|i| format!("name_{i}")).collect());
    let whole = ToolOutput::refusal(&error, OUTLINE_BUDGET);
    let answer: Value = serde_json::from_str(&whole.json).unwrap();
    assert!(
        answer["error"].as_str().unwrap().chars().count() <= MAX_REFUSAL_CHARS + 1,
        "{answer}"
    );
    assert_eq!(answer["suggestions"].as_array().unwrap().len(), MAX_NAMED);
    let small = ToolOutput::refusal(&error, 400);
    assert!(small.json.len() <= 400, "{}", small.json);
    let answer: Value = serde_json::from_str(&small.json).unwrap();
    assert!(answer.get("suggestions").is_none(), "{answer}");
    assert!(
        answer["error"].as_str().unwrap().chars().count() >= crate::tools::evidence::ECHO_CHARS,
        "{answer}"
    );
}

/// An input's mismatch repeats what the input holds there (a field's name)
/// as an answer repeats a caller's text: cut, with its length.
#[test]
fn a_mismatch_echoes_the_input_cut() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let field = "f".repeat(5_000);
    let mut input = serde_json::Map::new();
    input.insert(field, json!(1));
    let refusal = host.refuse(&mut session, "read_model", Value::Object(input));
    let reason = refusal["error"].as_str().unwrap();
    assert!(
        reason.contains("characters)") && reason.len() < 600,
        "{reason}"
    );
}
