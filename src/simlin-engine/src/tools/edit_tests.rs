// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

/// Call `edit_model` as a host does, making the edit it makes the project's
/// contents; it must answer, and the answer is checked against the output
/// schema the catalog publishes.
pub(super) fn edit(host: &mut Host, session: &mut Session, input: Value) -> Value {
    let output = host.call(session, "edit_model", input);
    let catalog: Value = serde_json::from_str(crate::tools::catalog_json()).unwrap();
    let schema = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "edit_model")
        .unwrap()["outputSchema"]
        .clone();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&output)
        .map(|e| format!("{e} at {}", e.instance_path))
        .collect();
    assert!(errors.is_empty(), "{errors:?}\n{output}");
    output
}

/// Call `edit_model` for an edit its gate refuses, as a host does: the
/// answer is a refusal (`is_error`) in the one shape the catalog publishes,
/// naming the rule that refused it, and the call hands the host nothing, so
/// the project and its revision are as they were.
pub(super) fn refused(host: &mut Host, session: &mut Session, input: Value) -> Value {
    let (before, revision) = (host.project.clone(), host.revision);
    let output = host.call_raw(session, "edit_model", &input.to_string());
    assert!(output.is_error, "{input}: {}", output.json);
    assert!(output.edited.is_none(), "{input}");
    assert!(
        host.project == before && host.revision == revision,
        "{input}"
    );
    let refusal: Value = serde_json::from_str(&output.json).unwrap();
    let catalog: Value = serde_json::from_str(crate::tools::catalog_json()).unwrap();
    let validator = jsonschema::validator_for(&catalog["refusalSchema"]).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&refusal)
        .map(|e| format!("{e} at {}", e.instance_path))
        .collect();
    assert!(errors.is_empty(), "{errors:?}\n{refusal}");
    assert!(
        refusal["refusedEdit"]["rule"].is_string(),
        "a refusal by the gate names its rule: {refusal}"
    );
    refusal
}

/// An edit of `operations` with a summary.
pub(super) fn operations(operations: Value) -> Value {
    json!({"summary": "an edit", "operations": operations})
}

pub(super) fn read(host: &mut Host, session: &mut Session) -> Value {
    host.call(session, "read_model", json!({}))
}

fn backlog() -> Value {
    operations(json!([
        {"op": "add_stock", "name": "Backlog", "initial": "0", "units": "widget"},
        {"op": "add_flow", "name": "ordering", "equation": "orders", "to": "Backlog",
         "units": "widget/month"},
        {"op": "add_flow", "name": "fulfilling", "equation": "shipments", "from": "Backlog",
         "units": "widget/month"}
    ]))
}

#[test]
fn a_field_edit_keeps_every_field_it_does_not_set() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let datamodel::Variable::Stock(before) = host.project.models[0]
        .get_variable("Inventory")
        .unwrap()
        .clone()
    else {
        panic!("a stock");
    };
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "set_equation", "variable": "inventory", "equation": "desired_inventory * 2"},
            {"op": "set_notes", "variable": "coverage", "notes": "Months of orders to hold."}
        ])),
    );
    assert_eq!(
        output["changes"],
        json!([
            {"variable": "coverage", "action": "changed", "detail": "notes"},
            {"variable": "Inventory", "action": "changed",
             "detail": "initial value desired_inventory * 2 (was desired_inventory)"}
        ]),
        "a line per variable, in the order of their names"
    );
    let datamodel::Variable::Stock(after) =
        host.project.models[0].get_variable("Inventory").unwrap()
    else {
        panic!("still a stock");
    };
    assert_eq!(after.units, before.units);
    assert_eq!(after.documentation, before.documentation);
    assert_eq!(after.inflows, before.inflows);
    assert_eq!(after.compat, before.compat, "its non-negative marking too");
    assert_eq!(
        after.equation,
        datamodel::Equation::Scalar("desired_inventory * 2".to_string())
    );
}

#[test]
fn the_gate_refuses_an_edit_that_adds_an_error_and_tolerates_one_the_model_had() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = refused(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "set_equation", "variable": "coverage", "equation": "nothing_here * 2"}
        ])),
    );
    assert_eq!(output["refusedEdit"]["simulates"], false);
    assert_eq!(
        output["refusedEdit"]["diagnostics"][0],
        json!({
            "severity": "error", "category": "equation", "code": "unknown_dependency",
            "variable": "coverage",
            "reason": "'nothing_here' is not a variable of model 'main', at `nothing_here` in \
                       `nothing_here * 2`"
        })
    );

    // A model broken already takes an edit elsewhere, and its repair.
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "5"}])),
    );
    assert_eq!(output["simulates"], false);
    assert!(
        output.get("diagnostics").is_none(),
        "the old error is not new"
    );
    // A second error in a model that did not simulate is still refused.
    refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "nowhere"}])),
    );
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "shipments", "equation": "orders"}])),
    );
    assert_eq!(output["simulates"], true);
}

/// A warning an edit adds is listed. A unit warning in a model with one
/// already does not refuse it; a model's first, as a person's patch would
/// be, does. A diagnostic an edit leaves in the model is listed under the id
/// a read gives it; a refused edit's, which no model has, under none.
#[test]
fn warnings_an_edit_adds_are_listed_and_a_models_first_unit_warning_refuses_it() {
    let mismatch = operations(json!([
        {"op": "set_equation", "variable": "shipments", "equation": "MIN(orders, Inventory)"}
    ]));
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = refused(&mut host, &mut session, mismatch.clone());
    assert!(
        output["error"].as_str().unwrap().contains("first"),
        "{output}"
    );
    let warnings = |diagnostics: &Value| -> usize {
        diagnostics
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["severity"] == "warning" && d["variable"] == "shipments")
            .count()
    };
    assert!(
        warnings(&output["refusedEdit"]["diagnostics"]) > 0,
        "{output}"
    );
    for diagnostic in output["refusedEdit"]["diagnostics"].as_array().unwrap() {
        assert!(diagnostic.get("id").is_none(), "{output}");
    }

    // With a unit warning already, another is listed and the edit is made.
    let mut project = inventory().build_datamodel();
    if let Some(Variable::Aux(aux)) = project.models[0].get_variable_mut("adjustment_time") {
        aux.units = Some("widget".to_string());
    }
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(&mut host, &mut session, mismatch);
    assert!(warnings(&output["diagnostics"]) > 0, "{output}");
    let outline = read(&mut host, &mut session);
    let reported = outline["diagnostics"].as_array().unwrap();
    for listed in output["diagnostics"].as_array().unwrap() {
        assert!(listed["id"].is_string(), "{output}");
        let read = reported
            .iter()
            .find(|d| d["id"] == listed["id"])
            .unwrap_or_else(|| panic!("a read reports {listed} under its id: {outline}"));
        for field in ["severity", "category", "code", "variable", "reason"] {
            assert_eq!(read[field], listed[field], "{field} of {listed}");
        }
    }
}

#[test]
fn an_edit_before_a_read_or_of_a_variable_changed_since_is_refused() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let coverage =
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "6"}]));
    let refusal = host.refuse(&mut session, "edit_model", coverage.clone());
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("read_model first")
    );

    read(&mut host, &mut session);
    // The person changes coverage.
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("coverage")
            .unwrap()
            .set_scalar_equation("5")
    });
    let refusal = host.refuse(&mut session, "edit_model", coverage.clone());
    let message = refusal["error"].as_str().unwrap();
    assert!(
        message.contains("coverage changed since you last read") && message.contains("read_model"),
        "{message}"
    );
    // A variable the person left alone may still be edited.
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "adjustment_time", "equation": "3"}])),
    );
    // Read again, and the edit is made.
    read(&mut host, &mut session);
    edit(&mut host, &mut session, coverage);
}

#[test]
fn a_rename_rewrites_its_readers_and_the_answer_names_them() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "rename", "variable": "coverage", "to": "inventory coverage"}])),
    );
    let changes = output["changes"].as_array().unwrap();
    assert!(changes.contains(&json!({
        "variable": "coverage", "action": "renamed", "detail": "renamed to inventory coverage"
    })));
    assert!(changes.contains(&json!({
        "variable": "desired_inventory", "action": "changed",
        "detail": "equation orders * inventory_coverage (was orders * coverage)"
    })));
    assert!(
        host.project.models[0]
            .get_variable("inventory_coverage")
            .is_some()
    );
    assert!(host.project.models[0].get_variable("coverage").is_none());
}

#[test]
fn connecting_and_deleting_a_flow_keep_its_stocks_lists_whole() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "add_stock", "name": "Shipped", "initial": "0"},
            {"op": "connect_flow", "flow": "shipments", "from": "Inventory", "to": "Shipped"},
            {"op": "delete", "variable": "production"}
        ])),
    );
    let changes = output["changes"].as_array().unwrap();
    assert!(
        changes.contains(&json!({
            "variable": "Inventory", "action": "changed", "detail": "inflows none (was production)"
        })),
        "{output}"
    );
    assert!(changes.contains(&json!({
        "variable": "production", "action": "deleted", "detail": "deleted flow"
    })));
    let Some(datamodel::Variable::Stock(shipped)) = host.project.models[0].get_variable("shipped")
    else {
        panic!("added");
    };
    assert_eq!(shipped.inflows, ["shipments"]);
}

#[test]
fn a_lookup_takes_increasing_points() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(
            json!([{"op": "set_lookup", "variable": "effect_of_pressure",
                           "points": [[0, 0], [2, 1], [1, 2]]}]),
        ),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("must not decrease")
    );
    let output = edit(
        &mut host,
        &mut session,
        operations(
            json!([{"op": "set_lookup", "variable": "effect_of_pressure",
                           "points": [[0, 0], [1, 0.8], [2, 1]], "kind": "extrapolate"}]),
        ),
    );
    assert_eq!(
        output["changes"][0]["detail"],
        "an extrapolating lookup of 3 points, x 0 to 2, y 0 to 1 (was a lookup of 3 points, x 0 \
         to 2, y 0 to 1)"
    );
    let datamodel::Variable::Aux(aux) = host.project.models[0]
        .get_variable("effect_of_pressure")
        .unwrap()
    else {
        panic!("an auxiliary");
    };
    let gf = aux.gf.as_ref().unwrap();
    assert_eq!(gf.y_points, [0.0, 0.8, 1.0]);
    assert_eq!(gf.kind, datamodel::GraphicalFunctionKind::Extrapolate);
}

#[test]
fn a_loop_is_named_by_the_id_analyze_loops_gave_it() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let loops = host.call(&mut session, "analyze_loops", json!({}));
    let id = loops["partitions"][0]["loops"][0]["id"].clone();
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "name_loop", "loop": id, "name": "restocking"}])),
    );
    assert_eq!(output["changes"][0]["action"], "loop_named");
    assert_eq!(output["changes"][0]["variable"], "restocking");
    let loops = host.call(&mut session, "analyze_loops", json!({"loops": [id]}));
    assert_eq!(loops["partitions"][0]["loops"][0]["name"], "restocking");
}

#[test]
fn the_sim_specs_change_as_the_edit_says() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "dt": 0.125, "stop": 30, "method": "rk4"}])),
    );
    assert_eq!(
        output["changes"],
        json!([{"variable": "sim specs", "action": "sim_specs",
                "detail": "stop 30 (was 20); dt 0.125 (was 0.25); method rk4 (was euler)"}])
    );
    assert_eq!(host.project.sim_specs.stop, 30.0);
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(json!([{"op": "set_sim_specs", "stop": -1}])),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("after the start")
    );
}

#[test]
fn an_operation_that_cannot_apply_is_refused_naming_it() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    for (ops, says) in [
        (
            json!([{"op": "set_units", "variable": "coverage", "units": "month"},
                   {"op": "set_equation", "variable": "coverge", "equation": "5"}]),
            "operation 2 (set_equation): the model has no variable 'coverge'",
        ),
        (
            json!([{"op": "add_variable", "name": "Coverage", "equation": "5"}]),
            "already has a variable",
        ),
        (
            json!([{"op": "add_variable", "name": "a+b", "equation": "5"}]),
            "not a name a variable can have",
        ),
        (
            json!([{"op": "add_flow", "name": "f", "equation": "1", "to": "orders"}]),
            "'orders' is not a stock",
        ),
        (
            json!([{"op": "name_loop", "loop": "L9", "name": "x"}]),
            "no loop has the id 'L9'",
        ),
        (
            json!([{"op": "set_equation", "variable": "coverage", "equation": "5", "extra": 1}]),
            "extra",
        ),
    ] {
        let refusal = host.refuse(&mut session, "edit_model", operations(ops.clone()));
        let message = refusal["error"].as_str().unwrap();
        assert!(message.contains(says), "{ops}: {message}");
    }
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(json!([{"op": "set_equation", "variable": "coverge", "equation": "5"}])),
    );
    assert_eq!(refusal["suggestions"][0], "coverage");
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        json!({"summary": " ", "operations": [{"op": "delete", "variable": "coverage"}]}),
    );
    assert!(refusal["error"].as_str().unwrap().contains("summary"));
    let many: Vec<Value> = (0..=MAX_OPERATIONS)
        .map(|_| json!({"op": "set_notes", "variable": "coverage", "notes": "x"}))
        .collect();
    let refusal = host.refuse(&mut session, "edit_model", operations(json!(many)));
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("between 1 and 24")
    );
}

/// An edit places what it adds and draws the connectors its equations
/// imply, and leaves the rest of the view as it was: the elements it did not
/// touch where they were, and the view's own fields -- its name, zoom,
/// polarity lettering and font, the fields `datamodel::StockFlow` holds
/// beside its elements and the box around them.
#[test]
fn an_edit_changes_the_views_elements_and_keeps_the_rest_of_the_view() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    // The first edit lays out the model, which has no diagram.
    edit(&mut host, &mut session, backlog());
    host.edit(|p| {
        let datamodel::View::StockFlow(view) = &mut p.models[0].views[0];
        view.name = Some("Supply".to_string());
        view.zoom = 1.75;
        // Where the person panned to, in a canvas of their window's size.
        view.view_box = datamodel::Rect {
            x: -40.0,
            y: 25.0,
            width: 1280.0,
            height: 720.0,
        };
        view.use_lettered_polarity = true;
        view.font = Some("Arial|12||0-0-0".to_string());
    });
    read(&mut host, &mut session);
    for (ops, adds) in [
        // An edit of equations adds a connector.
        (
            json!([{"op": "set_equation", "variable": "ordering",
                    "equation": "orders + Backlog / coverage"}]),
            "a connector",
        ),
        // A structural edit adds a variable.
        (
            json!([{"op": "add_variable", "name": "reporting_delay", "equation": "3"}]),
            "an auxiliary",
        ),
    ] {
        let before = {
            let datamodel::View::StockFlow(view) = &host.project.models[0].views[0];
            view.clone()
        };
        edit(&mut host, &mut session, operations(ops));
        let datamodel::View::StockFlow(after) = &host.project.models[0].views[0];
        // Every field but the one a layout decides, by a destructuring that
        // a field added to the view breaks until it is listed here.
        let datamodel::StockFlow {
            name,
            elements,
            view_box,
            zoom,
            use_lettered_polarity,
            font,
            sketch_compat,
        } = after;
        assert_eq!(name.as_deref(), Some("Supply"), "{adds}");
        assert_eq!(*zoom, 1.75, "{adds}");
        assert!(*view_box == before.view_box, "{adds}: the viewport moved");
        assert!(*use_lettered_polarity, "{adds}");
        assert_eq!(font.as_deref(), Some("Arial|12||0-0-0"), "{adds}");
        assert!(*sketch_compat == before.sketch_compat, "{adds}");
        assert!(
            elements.len() > before.elements.len(),
            "the edit draws {adds}"
        );
        for element in before.elements.iter() {
            // A connector may turn with what the edit attaches; a variable
            // the edit did not name stays where it was.
            if !matches!(element, datamodel::ViewElement::Link(_)) {
                let kept = elements.iter().find(|e| e.get_uid() == element.get_uid());
                assert!(kept == Some(element), "{adds}: an untouched element moved");
            }
        }
    }
}

/// The inventory model, drawn.
fn drawn() -> Host {
    Host::new(crate::tools::test_support::with_diagram(
        inventory().build_datamodel(),
    ))
}

/// The x of the stock `name` on the model's diagram.
fn stock_x(host: &Host, name: &str) -> f64 {
    let datamodel::View::StockFlow(view) = &host.project.models[0].views[0];
    view.elements
        .iter()
        .find_map(|e| match e {
            datamodel::ViewElement::Stock(s) if s.name == name => Some(s.x),
            _ => None,
        })
        .expect("the stock is drawn")
}

/// The sim specs and a loop's name are what an edit writes too: one the
/// person changed since the agent's read is not overwritten unseen.
#[test]
fn the_specs_and_a_loop_name_the_person_changed_are_not_overwritten_unseen() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    host.edit(|p| p.sim_specs.dt = datamodel::Dt::Dt(0.5));
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(json!([{"op": "set_sim_specs", "dt": 0.125}])),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("the sim specs changed since you last read"),
        "{refusal}"
    );
    // The specs the agent's own edit set are its own to change again.
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 30}])),
    );
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 40}])),
    );

    let loops = host.call(&mut session, "analyze_loops", json!({}));
    let id = loops["partitions"][0]["loops"][0]["id"].clone();
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "name_loop", "loop": id, "name": "restocking"}])),
    );
    let variables: Vec<String> = loops["partitions"][0]["loops"][0]["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|link| link["variable"].as_str().unwrap().to_string())
        .collect();
    // The person names the same loop first.
    host.edit(|p| {
        crate::apply_patch(
            p,
            ProjectPatch {
                project_ops: vec![],
                models: vec![ModelPatch {
                    name: "main".to_string(),
                    ops: vec![ModelOperation::SetLoopName {
                        variables,
                        name: "supply line".to_string(),
                        description: None,
                    }],
                }],
            },
        )
        .unwrap()
    });
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(json!([{"op": "name_loop", "loop": id, "name": "restocking"}])),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("the name of the loop through"),
        "{refusal}"
    );
}

/// An edit that makes a value not a number is refused, though the engine's
/// diagnostics say nothing of it, and every warning an edit adds is listed.
#[test]
fn the_gate_refuses_a_value_that_is_not_a_number_and_lists_every_warning() {
    for equation in ["NaN", "orders / 0", "0 / 0"] {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        let output = refused(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_equation", "variable": "coverage",
                               "equation": equation}])),
        );
        let non_finite: Vec<&Value> = output["refusedEdit"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["code"] == "non_finite")
            .collect();
        assert!(
            non_finite.iter().any(|d| d["variable"] == "coverage"),
            "{equation}: {output}"
        );
        assert!(
            output["error"].as_str().unwrap().contains("not a number"),
            "{output}"
        );
    }

    // A value not a number in the model's own run already is not the edit's.
    let mut project = inventory().build_datamodel();
    project.models[0].variables.push(
        TestProject::new("x")
            .aux("unfinished", "NaN", None)
            .build_datamodel()
            .models[0]
            .variables[0]
            .clone(),
    );
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "5"}])),
    );

    // An equation left unwritten, as an import stores one, is a warning,
    // not a unit one: listed, beside its value, which the values rule
    // refuses.
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "placeholder", "equation": "NaN"}])),
    );
    assert_eq!(output["refusedEdit"]["rule"], "values", "{output}");
    assert!(
        output["refusedEdit"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["severity"] == "warning" && d["code"] == "unfilled_equation"),
        "{output}"
    );
}

/// The gate reads a variable's series whatever its name holds: a `$`, a
/// `[`, or the word `time`, whose series has the results' `time` key.
#[test]
fn the_gate_refuses_a_value_that_is_not_a_number_whatever_the_name() {
    for name in ["$x", "cost [usd]", "time", "plain"] {
        let project = inventory().aux(name, "1", None);
        let mut host = Host::from_test_project(&project);
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        let output = refused(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_equation", "variable": name, "equation": "0 / 0"}])),
        );
        assert_eq!(output["refusedEdit"]["rule"], "values", "{name}: {output}");
        assert!(
            output["refusedEdit"]["diagnostics"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["code"] == "non_finite" && d["variable"] == name),
            "{name}: {output}"
        );
    }
}

/// A model that did not simulate has no run to compare with, so a value
/// that is not a number there is not the edit's doing: a repair of a
/// learner's broken model is not refused for one.
#[test]
fn a_repair_of_a_model_that_did_not_simulate_is_not_refused_for_its_values() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    project.models[0]
        .get_variable_mut("coverage")
        .unwrap()
        .set_scalar_equation("NaN");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "shipments", "equation": "orders"}])),
    );
}

/// Renaming a variable that has an error keeps the error the model's own:
/// tidying a learner's broken model is not refused for it.
#[test]
fn renaming_a_variable_with_an_error_is_not_refused_for_it() {
    let project = TestProject::new("broken")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "rate", None)
        .aux("rate", "missing_input * 2", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "rename", "variable": "rate", "to": "growth_rate"}])),
    );
    assert!(output.get("diagnostics").is_none(), "{output}");
}

/// The stocks an edit rewires are what it changes: a stock the person
/// changed since the read refuses an edit that takes a flow away from it or
/// deletes a flow that filled it. An edit that leaves the stock's flows as
/// they are changes nothing of it, and overwrites nothing.
#[test]
fn the_stocks_an_edit_rewires_must_be_as_read() {
    for (ops, rewires) in [
        // `shipments` drains Inventory already.
        (
            json!([{"op": "add_stock", "name": "Shipped", "initial": "0", "units": "widget"},
                   {"op": "connect_flow", "flow": "shipments", "from": "Inventory",
                    "to": "Shipped"}]),
            false,
        ),
        (
            json!([{"op": "add_stock", "name": "Shipped", "initial": "0", "units": "widget"},
                   {"op": "connect_flow", "flow": "shipments", "to": "Shipped"}]),
            true,
        ),
        (json!([{"op": "delete", "variable": "production"}]), true),
    ] {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        host.edit(|p| {
            p.models[0]
                .get_variable_mut("Inventory")
                .unwrap()
                .set_scalar_equation("desired_inventory * 1.5")
        });
        let output = host.call_raw(
            &mut session,
            "edit_model",
            &operations(ops.clone()).to_string(),
        );
        assert_eq!(output.is_error, rewires, "{ops}: {}", output.json);
        if rewires {
            assert!(
                output
                    .json
                    .contains("Inventory changed since you last read"),
                "{ops}: {}",
                output.json
            );
        }
    }
}

/// An answer's lines keep to its budget, a made edit's and a refused one's:
/// a rename rewrites every reader, each line's detail is cut, and the lines
/// past the budget are counted. The edit is made whole.
#[test]
fn an_answers_lines_keep_to_the_budget() {
    let long = (0..40)
        .map(|i| format!("base * {i}"))
        .collect::<Vec<_>>()
        .join(" + ");
    let mut project = TestProject::new("wide")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "base", None)
        .aux("base", "1", None)
        .aux("long_reader", &long, None);
    for i in 0..6 {
        project = project.aux(&format!("reader_{i}"), "base * 2", None);
    }
    // Every reader, the renamed variable and its flow.
    const LINES: usize = 9;
    let rename = operations(json!([{"op": "rename", "variable": "base", "to": "foundation"}]));

    let mut host = Host::from_test_project(&project);
    let mut roomy = Session::new("main");
    read(&mut host, &mut roomy);
    roomy.outline_budget = usize::MAX;
    let whole = edit(&mut host, &mut roomy, rename.clone());
    let lines = whole["changes"].as_array().unwrap();
    assert_eq!(lines.len(), LINES, "{whole}");
    assert!(whole.get("omitted").is_none(), "{whole}");
    assert!(
        lines
            .iter()
            .all(|l| l["detail"].as_str().unwrap().chars().count() <= 240)
    );
    assert!(
        lines
            .iter()
            .any(|l| l["detail"].as_str().unwrap().ends_with("...")),
        "a long line is cut"
    );

    let mut host = Host::from_test_project(&project);
    let mut tight = Session::new("main");
    read(&mut host, &mut tight);
    tight.outline_budget = whole.to_string().len() / 2;
    let output = edit(&mut host, &mut tight, rename);
    assert!(output.to_string().len() <= tight.outline_budget, "{output}");
    let omitted = output["omitted"].as_u64().unwrap() as usize;
    assert!(omitted > 0, "{output}");
    assert_eq!(
        output["changes"].as_array().unwrap().len() + omitted,
        LINES,
        "every line is listed or counted"
    );
    let model = &host.project.models[0];
    assert!(model.get_variable("foundation").is_some());
    assert!(
        model.variables.iter().all(|v| {
            v.get_equation()
                .is_none_or(|e| !e.source_text().contains("base"))
        }),
        "the readers past the budget are rewritten too"
    );

    // The same rename with an error beside it, which the gate refuses: the
    // refusal carries the lines the edit would have had, fitted the same way.
    let broken = operations(json!([
        {"op": "rename", "variable": "base", "to": "foundation"},
        {"op": "add_variable", "name": "broken", "equation": "nowhere * 2"}
    ]));
    let mut host = Host::from_test_project(&project);
    let mut roomy = Session::new("main");
    read(&mut host, &mut roomy);
    roomy.outline_budget = usize::MAX;
    let whole = refused(&mut host, &mut roomy, broken.clone());
    assert_eq!(
        whole["refusedEdit"]["changes"].as_array().unwrap().len(),
        LINES + 1,
        "{whole}"
    );
    let mut tight = Session::new("main");
    read(&mut host, &mut tight);
    tight.outline_budget = whole.to_string().len() / 2;
    let refusal = refused(&mut host, &mut tight, broken);
    assert!(
        refusal.to_string().len() <= tight.outline_budget,
        "{refusal}"
    );
    let omitted = refusal["refusedEdit"]["omitted"].as_u64().unwrap() as usize;
    assert!(omitted > 0, "{refusal}");
    assert_eq!(
        refusal["refusedEdit"]["changes"].as_array().unwrap().len() + omitted,
        LINES + 1,
        "every line is listed or counted"
    );
}

/// An agent's edit of a project that records who made its variables marks
/// them: a variable an AI made and it edits stays the AI's, one a person
/// made is also edited by AI, and one it adds is made by AI. Provenance is
/// no change to whether the variable is as read: the agent's next edit of it
/// is made.
#[test]
fn an_agents_edit_records_its_provenance_and_stays_its_own() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/ai-information/GeneratedByAIThenEdited.stmx");
    let file = std::fs::File::open(path).expect("the model is in the corpus");
    let project = crate::xmile::project_from_reader(&mut std::io::BufReader::new(file)).unwrap();
    let mut host = Host::new(project);
    let state = |host: &Host, name: &str| {
        host.project.models[0]
            .get_variable(name)
            .and_then(Variable::get_ai_state)
    };
    assert_eq!(
        state(&host, "marketing_spend"),
        Some(AiState::C),
        "the premise"
    );
    let person_made = host.project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ai_state() == Some(AiState::F))
        .map(|v| v.get_ident().to_string());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let mut ops = vec![
        json!({"op": "set_notes", "variable": "marketing spend", "notes": "what marketing costs"}),
        json!({"op": "add_variable", "name": "spend_ratio", "equation": "marketing_spend / 2"}),
    ];
    if let Some(name) = &person_made {
        ops.push(json!({"op": "set_notes", "variable": name, "notes": "noted by the agent"}));
    }
    edit(&mut host, &mut session, operations(json!(ops)));
    assert_eq!(state(&host, "marketing_spend"), Some(AiState::C));
    assert_eq!(state(&host, "spend_ratio"), Some(AiState::C));
    if let Some(name) = &person_made {
        assert_eq!(state(&host, name), Some(AiState::H), "{name}");
    }
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_units", "variable": "marketing spend",
                           "units": "dollars/month"}])),
    );
    // Nor does a change of provenance alone make a variable changed.
    read(&mut host, &mut session);
    host.edit(
        |p| match p.models[0].get_variable_mut("marketing_spend").unwrap() {
            Variable::Stock(v) => v.ai_state = Some(AiState::G),
            Variable::Flow(v) => v.ai_state = Some(AiState::G),
            Variable::Aux(v) => v.ai_state = Some(AiState::G),
            Variable::Module(v) => v.ai_state = Some(AiState::G),
        },
    );
    assert_eq!(state(&host, "marketing_spend"), Some(AiState::G));
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_notes", "variable": "marketing spend", "notes": "y"}])),
    );

    // A model that records no provenance gets none.
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_notes", "variable": "coverage", "notes": "x"}])),
    );
    assert_eq!(state(&host, "coverage"), None);
}

/// A structural edit of an imported Vensim model keeps what its writer needs
/// of the view: its font and its sketch metadata.
#[test]
fn a_structural_edit_keeps_what_a_vensim_writer_needs_of_the_view() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/test-models/samples/SIR/SIR.mdl");
    let mdl = std::fs::read_to_string(path).expect("the model is in the corpus");
    let mut project = crate::compat::open_vensim(&mdl).unwrap();
    let view = |p: &datamodel::Project| {
        let datamodel::View::StockFlow(view) = &p.models[0].views[0];
        (view.font.clone(), view.sketch_compat.clone())
    };
    {
        let datamodel::View::StockFlow(v) = &mut project.models[0].views[0];
        v.font.get_or_insert_with(|| "Arial|12||0-0-0".to_string());
    }
    let before = view(&project);
    assert!(
        before.1.is_some(),
        "the premise: the import keeps sketch metadata"
    );
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "reporting_delay", "equation": "3"}])),
    );
    assert_eq!(view(&host.project), before);
}

/// An edit of the specs that would make a run hold more than a run may is
/// refused with the numbers and specs that fit; the model's own specs are
/// never refused for what they already cost.
#[test]
fn an_edit_whose_specs_cost_more_than_a_run_may_is_refused() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "dt": 0.0001}])),
    );
    let reason = output["error"].as_str().unwrap();
    assert!(
        reason.contains("asks more of a run than a run may") && reason.contains("a DT of at least"),
        "{reason}"
    );
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 30}])),
    );
}

/// A model of the corpus.
pub(super) fn corpus(path: &str) -> datamodel::Project {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test")
        .join(path);
    if path.ends_with(".mdl") {
        let mdl = std::fs::read_to_string(&full).expect("the model is in the corpus");
        crate::compat::open_vensim(&mdl).unwrap()
    } else {
        let file = std::fs::File::open(&full).expect("the model is in the corpus");
        crate::compat::open_xmile(&mut std::io::BufReader::new(file)).unwrap()
    }
}

/// Two populations as modules of a root model, wired through it: Stella's
/// hares and lynxes, as its XMILE spells module wiring.
fn hares_and_lynxes() -> datamodel::Project {
    corpus("test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile")
}

/// Whether the project's root model simulates.
fn root_simulates(host: &mut Host) -> bool {
    let mut session = Session::new("main");
    !host
        .call_raw(
            &mut session,
            "read_behavior",
            &json!({"variables": ["area"]}).to_string(),
        )
        .is_error
}

/// A root model with one of each kind of variable -- a stock, a flow, an
/// auxiliary and a module, wired as the editor writes wiring (a bare source,
/// a `module·port` target) -- and the model the module instantiates.
fn one_of_each_kind() -> datamodel::Project {
    let mut project = TestProject::new("kinds")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("Level", "10", &["filling"], &[], None)
        .flow("filling", "rate * 2", None)
        .aux("rate", "3", None)
        .aux("idle", "1", None)
        // The module is read, so an edit of it reaches a reader.
        .aux("observed", "part.output", None)
        .build_datamodel();
    let aux = |ident: &str, equation: &str, input: bool| {
        Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input: input,
                ..datamodel::Compat::default()
            },
        })
    };
    project.models[0]
        .variables
        .push(Variable::Module(datamodel::Module {
            ident: "part".to_string(),
            model_name: "component".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "idle".to_string(),
                dst: "part\u{00B7}input".to_string(),
            }],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    project.models.push(datamodel::Model {
        name: "component".to_string(),
        sim_specs: None,
        variables: vec![aux("input", "500", true), aux("output", "input * 2", false)].into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project
}

/// The gate's refusal of `ops` on `project`'s first model under `policy`,
/// with what it lists.
fn refusal_under(
    project: &datamodel::Project,
    ops: Value,
    policy: &GatePolicy,
) -> (Option<(GateRule, String)>, Vec<EditDiagnostic>) {
    let mut host = Host::new(project.clone());
    let mut session = Session::new("main");
    let operations: Vec<EditOperation> = serde_json::from_value(ops).unwrap();
    let project = host.project.clone();
    let model = &project.models[0];
    let built = Built::new(&project, model, &session.evidence, &operations).expect("it applies");
    let Ok(made) = built.finish(&mut session.runs, &mut host.workspace(), model, policy) else {
        panic!("nothing waits for the project");
    };
    let listed = made.diagnostics.into_iter().map(|found| found.diagnostic);
    (made.refusal, listed.collect())
}

/// The shape of an equation, by a match a new variant breaks.
fn shape(equation: &Equation) -> &'static str {
    match equation {
        Equation::Scalar(_) => "scalar",
        Equation::ApplyToAll(_, _) => "apply_to_all",
        Equation::Arrayed(_, _, _, _) => "arrayed",
    }
}

/// `set_equation` on each shape of equation, for one element and for all:
/// what it sets is set, and everything else the equation holds stays -- the
/// other elements' equations, the `:EXCEPT:` default and the flag that
/// applies it to the elements with no arm of their own.
#[test]
fn set_equation_keeps_what_it_does_not_set_for_every_shape_of_equation() {
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("region", &["north", "south", "east"])
        .aux("scalar", "1", None)
        .array_aux("all[region]", "0.1")
        .array_with_default_and_overrides("partial[region]", "0.1", vec![("north", "0.5")])
        .build_datamodel();
    let equation = |host: &Host, name: &str| {
        host.project.models[0]
            .get_variable(name)
            .unwrap()
            .get_equation()
            .unwrap()
            .clone()
    };
    let region = || vec!["region".to_string()];
    let arm = |element: &str, text: &str| (element.to_string(), text.to_string(), None, None);
    // (variable, element, what the equation is after, each element's value)
    type Row = (
        &'static str,
        Option<&'static str>,
        Result<Equation, &'static str>,
        Vec<(&'static str, f64)>,
    );
    let rows: Vec<Row> = vec![
        (
            "scalar",
            None,
            Ok(Equation::Scalar("7".to_string())),
            vec![("scalar", 7.0)],
        ),
        ("scalar", Some("north"), Err("is not arrayed"), vec![]),
        (
            "all",
            None,
            Ok(Equation::ApplyToAll(region(), "7".to_string())),
            vec![("all[north]", 7.0), ("all[south]", 7.0)],
        ),
        (
            "all",
            Some("South"),
            Ok(Equation::Arrayed(
                region(),
                vec![arm("north", "0.1"), arm("south", "7"), arm("east", "0.1")],
                None,
                false,
            )),
            vec![("all[north]", 0.1), ("all[south]", 7.0), ("all[east]", 0.1)],
        ),
        (
            "partial",
            None,
            Ok(Equation::ApplyToAll(region(), "7".to_string())),
            vec![("partial[north]", 7.0), ("partial[east]", 7.0)],
        ),
        // An arm the equation has: the default still applies to the rest.
        (
            "partial",
            Some("north"),
            Ok(Equation::Arrayed(
                region(),
                vec![arm("north", "7")],
                Some("0.1".to_string()),
                true,
            )),
            vec![
                ("partial[north]", 7.0),
                ("partial[south]", 0.1),
                ("partial[east]", 0.1),
            ],
        ),
        // An element the default covered gets an arm of its own.
        (
            "partial",
            Some("east"),
            Ok(Equation::Arrayed(
                region(),
                vec![arm("north", "0.5"), arm("east", "7")],
                Some("0.1".to_string()),
                true,
            )),
            vec![
                ("partial[north]", 0.5),
                ("partial[south]", 0.1),
                ("partial[east]", 7.0),
            ],
        ),
    ];
    let mut shapes = std::collections::BTreeSet::new();
    for (variable, element, expected, values) in rows {
        let mut host = Host::new(project.clone());
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        shapes.insert(shape(&equation(&host, variable)));
        let mut op = json!({"op": "set_equation", "variable": variable, "equation": "7"});
        if let Some(element) = element {
            op["element"] = json!(element);
        }
        let output = host.call_raw(
            &mut session,
            "edit_model",
            &operations(json!([op])).to_string(),
        );
        let row = format!("{variable} {element:?}");
        match expected {
            Err(says) => assert!(
                output.is_error && output.json.contains(says),
                "{row}: {}",
                output.json
            ),
            Ok(expected) => {
                let output: Value = serde_json::from_str(&output.json).unwrap();
                assert!(output.get("diagnostics").is_none(), "{row}: {output}");
                assert!(equation(&host, variable) == expected, "{row}");
                for (series, value) in values {
                    let behavior = host.call(
                        &mut session,
                        "read_behavior",
                        json!({"variables": [series]}),
                    );
                    assert_eq!(
                        behavior["series"][0]["end"],
                        json!(value),
                        "{row}: {series}"
                    );
                }
            }
        }
    }
    assert_eq!(
        shapes.into_iter().collect::<Vec<_>>(),
        ["apply_to_all", "arrayed", "scalar"],
        "a row for every shape of equation"
    );
}

/// An element is named as the read tools name one: a space after a comma,
/// case and underscores aside. A name that is no element is refused with the
/// elements there are.
#[test]
fn an_element_is_named_as_the_read_tools_name_it() {
    let mut host = Host::new(corpus("test-models/tests/except/test_except.mdl"));
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let read_it = host.call(
        &mut session,
        "read_variables",
        json!({"names": ["sales[S1, p1]"]}),
    );
    assert!(read_it.get("notFound").is_none(), "{read_it}");
    edit(
        &mut host,
        &mut session,
        operations(
            json!([{"op": "set_equation", "variable": "sales", "element": "S1, p1",
                           "equation": "7"}]),
        ),
    );
    let behavior = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["sales[s1,p1]"]}),
    );
    assert_eq!(behavior["series"][0]["end"], json!(7.0));
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(
            json!([{"op": "set_equation", "variable": "sales", "element": "s1, p9",
                           "equation": "7"}]),
        ),
    );
    let message = refusal["error"].as_str().unwrap();
    assert!(
        message.contains("has no element 's1, p9'") && message.contains("is not an element of"),
        "{message}"
    );
    assert_eq!(refusal["suggestions"][0], "s1,p1");
}

/// The gate judges the project: an edit of a module's model that stops the
/// project's root model simulating, or adds an error to it, is refused,
/// though the module's own model has nothing new to say.
#[test]
fn an_edit_of_a_modules_model_that_breaks_the_root_model_is_refused() {
    let mut host = Host::new(hares_and_lynxes());
    assert!(root_simulates(&mut host), "the premise");
    let mut session = Session::new("hares");
    read(&mut host, &mut session);
    // The root reads `hares.hare_density`: deleting it breaks the root.
    let output = refused(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "set_equation", "variable": "hares killed per lynx", "equation": "0.1"},
            {"op": "delete", "variable": "hare density"}
        ])),
    );
    let error = &output["refusedEdit"]["diagnostics"][0];
    assert_eq!(error["severity"], "error", "{output}");
    assert_eq!(
        error["model"], "main",
        "the error is the root model's: {output}"
    );
    assert!(
        output["error"].as_str().unwrap().contains("in main"),
        "{output}"
    );
    assert!(root_simulates(&mut host));

    // With the rule against new errors off, the root model no longer
    // simulating refuses it; with that off too, nothing does.
    let ops = json!([
        {"op": "set_equation", "variable": "hares killed per lynx", "equation": "0.1"},
        {"op": "delete", "variable": "hare density"}
    ]);
    let mut project = hares_and_lynxes();
    let hares = project
        .models
        .iter()
        .position(|m| m.name == "hares")
        .unwrap();
    project.models.swap(0, hares);
    let no_errors = GatePolicy {
        errors: false,
        ..GatePolicy::AGENT_EDIT
    };
    let (refusal, _) = refusal_under(&project, ops.clone(), &no_errors);
    let (rule, refusal) = refusal.expect("the root model stops simulating");
    assert_eq!(rule, GateRule::Simulation);
    assert!(
        refusal.contains("The project's model 'main' would not simulate"),
        "{refusal}"
    );
    let neither = GatePolicy {
        simulation: false,
        ..no_errors
    };
    assert_eq!(refusal_under(&project, ops, &neither).0, None);

    // A module's model that simulates by itself before the edit and after
    // it: the root model, which reads what the edit deletes, is the one that
    // stops. (The rule refuses only for a model that simulated, so the
    // refusal shows the root did.)
    let mut project = one_of_each_kind();
    let reading = Variable::Aux(datamodel::Aux {
        ident: "reading".to_string(),
        equation: Equation::Scalar("part.output".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    project.models[0].variables.push(reading);
    project.models.swap(0, 1);
    assert_eq!(project.models[0].name, "component");
    let ops = json!([{"op": "delete", "variable": "output"}]);
    let simulation_alone = GatePolicy {
        simulation: true,
        errors: false,
        unit_warnings: false,
        values: false,
    };
    let (rule, refusal) = refusal_under(&project, ops, &simulation_alone)
        .0
        .expect("the root model stops simulating");
    assert_eq!(rule, GateRule::Simulation);
    assert!(
        refusal.contains("The project's model 'main' would not simulate"),
        "{refusal}"
    );
}

/// An edit only `rule` refuses on the inventory model, and what its
/// refusal says: a row per rule of the gate, by a match a new rule breaks.
fn refused_only_by(rule: GateRule) -> (Value, &'static str) {
    let set = |equation: &str| json!([{"op": "set_equation", "variable": "coverage", "equation": equation}]);
    match rule {
        // An unknown name is an error, and the model stops simulating too:
        // the errors rule is asked first.
        GateRule::Errors => (set("nothing_here * 2"), "It adds an error"),
        GateRule::RunCost => (
            json!([{"op": "set_sim_specs", "dt": 0.0001}]),
            "asks more of a run than a run may",
        ),
        // The same edit, which the simulation rule refuses when the errors
        // rule is not asked.
        GateRule::Simulation => (set("nothing_here * 2"), "The model would not simulate"),
        GateRule::Values => (set("0 / 0"), "not a number"),
        GateRule::UnitWarnings => (
            json!([{"op": "set_equation", "variable": "shipments",
                    "equation": "MIN(orders, Inventory)"}]),
            "its first",
        ),
    }
}

/// The policy that asks for `rule` alone. The run's cost is asked under
/// every policy, so it is the policy that asks for nothing else.
fn only(rule: GateRule) -> GatePolicy {
    let nothing = GatePolicy {
        errors: false,
        unit_warnings: false,
        simulation: false,
        values: false,
    };
    match rule {
        GateRule::Errors => GatePolicy {
            errors: true,
            ..nothing
        },
        GateRule::RunCost => nothing,
        GateRule::Simulation => GatePolicy {
            simulation: true,
            ..nothing
        },
        GateRule::Values => GatePolicy {
            values: true,
            ..nothing
        },
        GateRule::UnitWarnings => GatePolicy {
            unit_warnings: true,
            ..nothing
        },
    }
}

/// Each rule of the gate refuses on its own, names itself, and refuses only
/// when asked for: the rows are `GateRule::ALL`, and `GatePolicy`'s fields
/// are destructured so a new field breaks the test until it has a rule.
#[test]
fn each_rule_of_the_gate_is_asked_for_by_name() {
    let GatePolicy {
        errors,
        unit_warnings,
        simulation,
        values,
    } = GatePolicy::AGENT_EDIT;
    assert!(errors && unit_warnings && simulation && values);
    assert!(
        GateRule::ALL
            .iter()
            .all(|rule| GatePolicy::AGENT_EDIT.asks(*rule))
    );
    let project = inventory().build_datamodel();
    for rule in GateRule::ALL {
        let (ops, says) = refused_only_by(rule);
        let policy = only(rule);
        let (refusal, listed) = refusal_under(&project, ops.clone(), &policy);
        let (by, refusal) = refusal.unwrap_or_else(|| panic!("{rule:?}: not refused"));
        assert_eq!(by, rule, "{refusal}");
        assert!(refusal.contains(says), "{rule:?}: {refusal}");
        if rule != GateRule::RunCost {
            assert!(!listed.is_empty(), "{rule:?}: what refuses is listed");
            // The policy that asks only for the run's cost asks for none of
            // the rules this edit trips.
            assert_eq!(
                refusal_under(&project, ops, &only(GateRule::RunCost)).0,
                None,
                "{rule:?}: refused by no rule that was not asked for"
            );
        }
    }
}

/// An edit the gate refuses is a refusal like every other tool's: through
/// `Session::call`, as a host calls it, a row per rule asserts `is_error`,
/// the one refusal shape the catalog publishes, the rule named in
/// `refusedEdit`, and that nothing changed -- the project, its revision,
/// and the session, whose answers afterwards are a twin session's that never
/// made the call. The simulation rule is asked after the errors rule, and
/// every edit found to stop a model simulating also adds an error, so under
/// the agent's policy it is not reached here; its refusal through the gate
/// is `each_rule_of_the_gate_is_asked_for_by_name`'s row, and the hares and
/// lynxes rows of `an_edit_of_a_modules_model_that_breaks_the_root_model_is_refused`.
#[test]
fn a_refused_edit_is_an_error_answer_naming_its_rule_and_changes_nothing() {
    // An edit first: it is refused if what the session holds as read moved,
    // which a read would put back before it could show.
    let after = [
        (
            "edit_model",
            operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "5"}])),
        ),
        ("list_runs", json!({})),
        ("read_model", json!({})),
    ];
    for rule in GateRule::ALL {
        if rule == GateRule::Simulation {
            continue;
        }
        let (ops, says) = refused_only_by(rule);
        let mut host = Host::from_test_project(&inventory());
        let mut twin_host = Host::from_test_project(&inventory());
        let (mut session, mut twin) = (Session::new("main"), Session::new("main"));
        read(&mut host, &mut session);
        read(&mut twin_host, &mut twin);

        let refusal = refused(&mut host, &mut session, operations(ops));
        assert_eq!(
            refusal["refusedEdit"]["rule"],
            serde_json::to_value(rule).unwrap(),
            "{refusal}"
        );
        assert!(
            refusal["error"].as_str().unwrap().contains(says)
                && refusal["error"]
                    .as_str()
                    .unwrap()
                    .contains("Nothing changed"),
            "{rule:?}: {refusal}"
        );
        assert!(
            session
                .changes_since_read(&host.project, host.revision)
                .unwrap()
                .is_none(),
            "{rule:?}"
        );
        for (tool, input) in &after {
            assert_eq!(
                host.call(&mut session, tool, input.clone()),
                twin_host.call(&mut twin, tool, input.clone()),
                "{rule:?}: {tool} after the refusal"
            );
        }
    }
}

/// An equation the edit writes must stand on its own. A variable that had
/// an error and still has one, in the equation the edit wrote for it, is
/// refused and the error listed, whether its reason changed or not; an
/// error a variable the edit did not write still has is the model's own.
#[test]
fn an_error_in_an_equation_the_edit_writes_is_refused() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    for (equation, names) in [
        // A repair that names another variable the model lacks.
        ("orderz", "orderz"),
        // The same missing name, in another equation.
        ("ordrs * 2", "ordrs"),
    ] {
        let output = refused(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_equation", "variable": "shipments",
                               "equation": equation}])),
        );
        let note = output["error"].as_str().unwrap();
        assert!(
            note.contains("It leaves an error in an equation it writes") && note.contains(names),
            "{note}"
        );
        assert_eq!(
            output["refusedEdit"]["diagnostics"][0]["variable"],
            "shipments"
        );
        assert_eq!(
            output["refusedEdit"]["diagnostics"][0]["code"],
            "unknown_dependency"
        );
    }
    // Its notes are not its equation: the error stays the model's own.
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_notes", "variable": "shipments", "notes": "What leaves."}])),
    );
    assert!(output.get("diagnostics").is_none(), "{output}");
}

/// The engine reports one error of an arrayed equation, its first failing
/// element's, so an element that failed already hides one the edit breaks:
/// the gate looks at the elements the edit wrote on their own.
#[test]
fn an_element_the_edit_breaks_is_refused_behind_one_broken_already() {
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("region", &["north", "south", "east"])
        .array_with_default_and_overrides(
            "rate[region]",
            "0.1",
            vec![("north", "nowhere"), ("south", "0.2")],
        )
        .build_datamodel();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let outline = read(&mut host, &mut session);
    assert_eq!(outline["diagnostics"].as_array().unwrap().len(), 1);
    let set = |element: &str, equation: &str| {
        operations(
            json!([{"op": "set_equation", "variable": "rate", "element": element,
                           "equation": equation}]),
        )
    };
    let output = refused(&mut host, &mut session, set("south", "nowhere_else"));
    let note = output["error"].as_str().unwrap();
    assert!(
        note.contains("It leaves an error in an equation it writes")
            && note.contains("'nowhere_else'"),
        "{note}"
    );
    // An element with no arm of its own, the default covering it.
    refused(&mut host, &mut session, set("east", "nowhere_else"));
    // An element the edit writes well is not refused for the broken one.
    let output = edit(&mut host, &mut session, set("south", "0.3"));
    assert!(output.get("diagnostics").is_none(), "{output}");
    // And the broken one's repair is made.
    let output = edit(&mut host, &mut session, set("north", "0.4"));
    assert_eq!(output["simulates"], true);

    // A default that fails is no more the edit's than an element is.
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("region", &["north", "south", "east"])
        .array_with_default_and_overrides("rate[region]", "nowhere", vec![("north", "0.5")])
        .build_datamodel();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(&mut host, &mut session, set("north", "0.6"));
    refused(&mut host, &mut session, set("south", "nowhere_else"));
}

/// The engine reports one error of an equation at a time, so fixing what one
/// names can bring out the next: on a variable the edit does not write, that
/// is the model's own next problem, tolerated, and listed so the agent sees
/// it.
#[test]
fn the_next_error_of_a_variable_the_edit_does_not_write_is_listed_not_refused() {
    let project = TestProject::new("broken")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("total", "first_part + second_part", None)
        .build_datamodel();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let outline = read(&mut host, &mut session);
    let reported = outline["diagnostics"][0]["reason"].as_str().unwrap();
    let (reported, other) = if reported.contains("first_part") {
        ("first_part", "second_part")
    } else {
        ("second_part", "first_part")
    };
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": reported, "equation": "1"}])),
    );
    let listed = &output["diagnostics"][0];
    assert_eq!(listed["variable"], "total");
    assert!(
        listed["reason"].as_str().unwrap().contains(other),
        "{output}"
    );
    // It is the model's error now, under the id a read reports it by, which
    // is not the id of the error the edit repaired.
    assert_ne!(listed["id"], outline["diagnostics"][0]["id"]);
    let outline = read(&mut host, &mut session);
    assert_eq!(outline["diagnostics"][0]["id"], listed["id"], "{outline}");
    let note = output["note"].as_str().unwrap();
    assert!(
        note.contains("An error remains") && note.contains(other),
        "{note}"
    );
    // The same error again is not news.
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "unrelated", "equation": "1"}])),
    );
    assert!(output.get("diagnostics").is_none(), "{output}");
}

/// A name is written in an equation as the equation language spells it. An
/// equation that writes one the way the model displays it, in several words,
/// is refused with how to spell it.
#[test]
fn a_name_written_as_displayed_is_refused_with_how_to_spell_it() {
    let mut host = Host::new(corpus("test-models/samples/teacup/teacup.xmile"));
    let mut session = Session::new("main");
    let outline = read(&mut host, &mut session);
    assert_eq!(outline["constants"][0]["name"], "Characteristic Time");
    for (equation, hinted) in [
        ("Characteristic Time * 2", true),
        ("characteristic time * 2", true),
        // Two names an operator stands between are two names.
        ("Characteristic * Time", false),
        ("\"Characteristic Time\" * nowhere", false),
    ] {
        let output = refused(
            &mut host,
            &mut session,
            operations(json!([{"op": "add_variable", "name": "doubled", "equation": equation}])),
        );
        let reason = output["refusedEdit"]["diagnostics"][0]["reason"]
            .as_str()
            .unwrap();
        assert_eq!(
            reason.contains(
                "'Characteristic Time' is one variable: write it characteristic_time in an \
                 equation"
            ),
            hinted,
            "{equation}: {reason}"
        );
    }
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "doubled",
                           "equation": "\"Characteristic Time\" * 2"}])),
    );
    // A name of one word is written as it is displayed: no hint for it.
    let output = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "tripled",
                           "equation": "doubled * nowhere"}])),
    );
    let reason = output["refusedEdit"]["diagnostics"][0]["reason"]
        .as_str()
        .unwrap();
    assert!(!reason.contains("is one variable"), "{reason}");
}

/// What a variable can be named: a new name in quotes is the name without
/// them, a name that differs only in how it is written restamps it, and a
/// name that is another variable's, a module's variable's, or no identifier
/// is refused.
#[test]
fn a_rename_takes_a_name_a_variable_can_have() {
    let rename = |to: &str| operations(json!([{"op": "rename", "variable": "coverage", "to": to}]));
    let fresh = || {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        (host, session)
    };
    let (mut host, mut session) = fresh();
    for (to, says) in [
        ("coverage", "is its name already"),
        ("Adjustment Time", "already has a variable"),
        ("part.coverage", "is not a name a variable can have"),
        ("coverage!", "is not a name a variable can have"),
        ("if", "is not a name a variable can have"),
        // The rename's own rule: a clock slot's name is the run's.
        ("time", "the simulation's own clock names"),
        ("dt", "the simulation's own clock names"),
        ("5", "a name is words of letters, digits and underscores"),
        ("a\"b", "is not a name a variable can have"),
        ("", "is not a name a variable can have"),
    ] {
        let refusal = host.refuse(&mut session, "edit_model", rename(to));
        assert!(
            refusal["error"].as_str().unwrap().contains(says),
            "{to}: {refusal}"
        );
    }
    // Case and spacing alone: the name is restamped, no equation rewritten.
    let output = edit(&mut host, &mut session, rename("Coverage"));
    assert_eq!(
        output["changes"],
        json!([{"variable": "coverage", "action": "changed",
                "detail": "written Coverage (was coverage)"}])
    );
    let model = &host.project.models[0];
    assert_eq!(
        model.get_variable("coverage").unwrap().get_ident(),
        "Coverage"
    );
    assert_eq!(
        model
            .get_variable("desired_inventory")
            .unwrap()
            .get_equation(),
        Some(&Equation::Scalar("orders * coverage".to_string()))
    );
    // Quotes are how an equation writes a name with spaces, not the name.
    let (mut host, mut session) = fresh();
    edit(&mut host, &mut session, rename("\"months of cover\""));
    let model = &host.project.models[0];
    assert_eq!(
        model.get_variable("months_of_cover").unwrap().get_ident(),
        "months of cover"
    );
    // A name an equation has to quote is a name: its readers quote it.
    let (mut host, mut session) = fresh();
    edit(&mut host, &mut session, rename("4 months"));
    let model = &host.project.models[0];
    assert_eq!(
        model.get_variable("4_months").unwrap().get_ident(),
        "4 months"
    );
    assert_eq!(
        model
            .get_variable("desired_inventory")
            .unwrap()
            .get_equation(),
        Some(&Equation::Scalar("orders * \"4_months\"".to_string()))
    );
}

/// Deleting a variable a module input is wired from leaves the input
/// unwired: no error says so, and the input takes the value its own model
/// gives it, so the answer says so, in the module's line and as a warning.
#[test]
fn a_delete_that_unwires_a_module_input_says_so() {
    let mut host = Host::new(one_of_each_kind());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "delete", "variable": "idle"}])),
    );
    assert!(
        output["changes"].as_array().unwrap().contains(&json!({
            "variable": "part", "action": "changed",
            "detail": "input input unwired (was from idle)"
        })),
        "{output}"
    );
    assert_eq!(
        output["diagnostics"],
        json!([{
            "severity": "warning", "category": "model", "code": "module_input_unwired",
            "variable": "part",
            "reason": "its input 'input' is no longer wired from 'idle': it takes the value the \
                       equation of 'input' in model 'component' gives it"
        }])
    );
    // A rename rewires it, and says which input reads the new name.
    let mut host = Host::new(one_of_each_kind());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "rename", "variable": "idle", "to": "resting"}])),
    );
    assert!(
        output["changes"].as_array().unwrap().contains(&json!({
            "variable": "part", "action": "changed",
            "detail": "input input from resting (was idle)"
        })),
        "{output}"
    );
    assert!(output.get("diagnostics").is_none(), "{output}");
}

/// What a session's own edit changed is its own, whatever the edit changed
/// it through: the readers a rename rewrote are fresh for its next edit and
/// no news in the change report.
#[test]
fn the_readers_a_sessions_own_rename_rewrote_are_its_own() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "rename", "variable": "coverage", "to": "cover"}])),
    );
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none()
    );
    edit(
        &mut host,
        &mut session,
        operations(
            json!([{"op": "set_equation", "variable": "desired_inventory",
                           "equation": "orders * cover * 2"}]),
        ),
    );
}

/// A rename rewrites its readers, so a reader the person changed since the
/// read is something the edit would overwrite unseen.
#[test]
fn a_rename_of_a_variable_whose_reader_changed_since_the_read_is_refused() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("desired_inventory")
            .unwrap()
            .set_scalar_equation("orders * coverage * 1.5")
    });
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(json!([{"op": "rename", "variable": "coverage", "to": "cover"}])),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("desired_inventory changed since you last read"),
        "{refusal}"
    );
}

/// A variable's uid is not the variable: naming a loop gives uids to the
/// variables of a model whose import gave them none, and they stay as read.
#[test]
fn the_variables_of_a_loop_the_session_named_are_as_read() {
    let mut host = Host::new(corpus("test-models/samples/SIR/SIR.mdl"));
    assert!(
        host.project.models[0]
            .variables
            .iter()
            .all(|v| crate::patch::variable_uid(v).is_none()),
        "the premise: a Vensim import gives no uids"
    );
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let loops = host.call(&mut session, "analyze_loops", json!({}));
    let found = &loops["partitions"][0]["loops"][0];
    let through = found["chain"][0]["variable"].as_str().unwrap().to_string();
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "name_loop", "loop": found["id"], "name": "contagion"}])),
    );
    assert!(
        crate::patch::variable_uid(host.project.models[0].get_variable(&through).unwrap())
            .is_some(),
        "the premise: naming the loop gave its variables uids"
    );
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none()
    );
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_notes", "variable": through, "notes": "x"}])),
    );
    // Its own loop name is its own to change again; one the person changed
    // since is not.
    let again = operations(json!([{"op": "name_loop", "loop": found["id"], "name": "spread"}]));
    edit(&mut host, &mut session, again.clone());
    host.edit(|p| p.models[0].loop_metadata[0].name = "the person's".to_string());
    let refusal = host.refuse(&mut session, "edit_model", again);
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("the name of the loop through"),
        "{refusal}"
    );
}

/// Every kind of variable, by a match a new kind breaks.
fn kind_of(var: &Variable) -> usize {
    match var {
        Variable::Stock(_) => 0,
        Variable::Flow(_) => 1,
        Variable::Aux(_) => 2,
        Variable::Module(_) => 3,
    }
}

/// Every operation, by a match a new operation breaks.
fn operation_row(op: &EditOperation) -> usize {
    match op {
        EditOperation::AddStock { .. } => 0,
        EditOperation::AddFlow { .. } => 1,
        EditOperation::AddVariable { .. } => 2,
        EditOperation::SetEquation { .. } => 3,
        EditOperation::SetUnits { .. } => 4,
        EditOperation::SetNotes { .. } => 5,
        EditOperation::SetLookup { .. } => 6,
        EditOperation::ConnectFlow { .. } => 7,
        EditOperation::Rename { .. } => 8,
        EditOperation::Delete { .. } => 9,
        EditOperation::NameLoop { .. } => 10,
        EditOperation::SetSimSpecs { .. } => 11,
    }
}

/// The refusal of an edit one of whose operations cannot apply: no gate
/// rule, and nothing changed.
fn rejected(host: &mut Host, session: &mut Session, input: Value) -> Value {
    let (before, revision) = (host.project.clone(), host.revision);
    let output = host.call_raw(session, "edit_model", &input.to_string());
    assert!(
        output.is_error && output.edited.is_none(),
        "{input}: {}",
        output.json
    );
    assert!(
        host.project == before && host.revision == revision,
        "{input}"
    );
    let refusal: Value = serde_json::from_str(&output.json).unwrap();
    assert!(refusal["refusedEdit"].is_null(), "{refusal}");
    refusal
}

/// A goal-seeking level, a growing stock beside it, and a constant read by
/// nothing: the loops `name_loop` rows name.
fn two_loops() -> TestProject {
    TestProject::new("loops")
        .stock("Level", "0", &["filling"], &[], None)
        .flow("filling", "gap * 0.1", None)
        .aux("gap", "target - Level", None)
        .aux("target", "10", None)
        .stock("B", "1", &["fb"], &[], None)
        .flow("fb", "B * 0.1", None)
        .aux("spare", "1", None)
}

/// A loop is named only when its variables are one feedback loop of the
/// model as the edit leaves it. The engine reads a named loop's variables as
/// a set (`db::model_pinned_loops`), so any order names it; what is no loop
/// is refused naming the link it lacks, before the gate, as an operation that
/// cannot apply is.
#[test]
fn a_named_loop_is_one_feedback_loop_of_the_model() {
    use std::result::Result::{Err as Refused, Ok as Made};
    let rows: Vec<(Value, Result<(), &str>)> = vec![
        (json!(["Level", "filling", "gap"]), Made(())),
        (json!(["gap", "filling", "Level"]), Made(())),
        (json!(["B", "fb"]), Made(())),
        (
            json!(["Level", "filling"]),
            Refused("none of the others reads 'Level'"),
        ),
        (
            json!(["Level", "filling", "gap", "target"]),
            Refused("'target' reads none of the others"),
        ),
        (
            json!(["spare", "B"]),
            Refused("none of the others reads 'spare'"),
        ),
        (json!(["spare"]), Refused("at least two variables")),
        (
            json!(["Level", "filling", "gap", "B", "fb"]),
            Refused("are no feedback loop the engine scores"),
        ),
    ];
    for (variables, comes) in rows {
        let mut host = Host::from_test_project(&two_loops());
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        let op = json!({"op": "name_loop", "variables": variables, "name": "named"});
        match comes {
            Made(()) => {
                edit(&mut host, &mut session, operations(json!([op])));
                assert_eq!(
                    host.project.models[0].loop_metadata[0].name, "named",
                    "{variables}"
                );
            }
            Refused(says) => {
                let refusal = rejected(&mut host, &mut session, operations(json!([op])));
                let reason = refusal["error"].as_str().unwrap();
                assert!(
                    reason.starts_with("operation 1 (name_loop): ") && reason.contains(says),
                    "{variables}: {reason}"
                );
            }
        }
    }
    // A loop the same edit makes is named; one a later operation breaks is
    // not.
    let mut host = Host::from_test_project(&two_loops());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "add_stock", "name": "S", "initial": "1"},
            {"op": "add_flow", "name": "f", "equation": "S * 0.1", "to": "S"},
            {"op": "name_loop", "variables": ["S", "f"], "name": "made here"}
        ])),
    );
    let refusal = rejected(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "name_loop", "variables": ["B", "fb"], "name": "broken later"},
            {"op": "set_equation", "variable": "fb", "equation": "0.1"}
        ])),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("none of the others reads 'B'"),
        "{refusal}"
    );
}

/// What a row of the operations table comes to.
enum Comes {
    /// An edit that is made, and leaves the project so.
    Made(fn(&datamodel::Project, &str)),
    /// An edit the gate refuses, its refusal saying so.
    Refused(&'static str),
    /// No edit: the operation cannot apply.
    Rejected(&'static str),
}

/// Every operation on every kind of variable: what it comes to, and what
/// it leaves when it is made. An edit that is not made changes nothing.
#[test]
fn every_operation_on_every_kind_of_variable() {
    use Comes::{Made, Refused, Rejected};
    // Drawn, as a model a person works on is: an edit places what it adds
    // around the diagram, where a model with none is laid out whole.
    let project = crate::tools::test_support::with_diagram(one_of_each_kind());
    // The variable of each kind, in `kind_of`'s order.
    let targets = ["Level", "filling", "rate", "part"];
    for (i, target) in targets.iter().enumerate() {
        assert_eq!(kind_of(project.models[0].get_variable(target).unwrap()), i);
    }
    // Per operation (in `operation_row`'s order): the operation on `target`,
    // and what it comes to on a stock, a flow, an auxiliary and a module.
    type Row = (fn(&str) -> Value, [Comes; 4]);
    let rows: Vec<Row> = vec![
        (
            |t| json!({"op": "add_stock", "name": t, "initial": "1"}),
            [
                Rejected("already has a variable"),
                Rejected("already has a variable"),
                Rejected("already has a variable"),
                Rejected("already has a variable"),
            ],
        ),
        (
            |t| json!({"op": "add_flow", "name": t, "equation": "1"}),
            [
                Rejected("already has a variable"),
                Rejected("already has a variable"),
                Rejected("already has a variable"),
                Rejected("already has a variable"),
            ],
        ),
        (
            |t| json!({"op": "add_variable", "name": t, "equation": "1"}),
            [
                Rejected("already has a variable"),
                Rejected("already has a variable"),
                Rejected("already has a variable"),
                Rejected("already has a variable"),
            ],
        ),
        (
            |t| json!({"op": "set_equation", "variable": t, "equation": "4"}),
            [
                Made(|p, t| {
                    let v = p.models[0].get_variable(t).unwrap();
                    assert_eq!(v.get_equation(), Some(&Equation::Scalar("4".to_string())));
                    let Variable::Stock(stock) = v else {
                        panic!("still a stock")
                    };
                    assert_eq!(stock.inflows, ["filling"]);
                }),
                Made(|p, t| {
                    let v = p.models[0].get_variable(t).unwrap();
                    assert_eq!(v.get_equation(), Some(&Equation::Scalar("4".to_string())));
                }),
                Made(|p, t| {
                    let v = p.models[0].get_variable(t).unwrap();
                    assert_eq!(v.get_equation(), Some(&Equation::Scalar("4".to_string())));
                }),
                Rejected("has no equation"),
            ],
        ),
        (
            |t| json!({"op": "set_units", "variable": t, "units": "widget"}),
            [
                Made(|p, t| assert_eq!(units_of(p, t).as_deref(), Some("widget"))),
                Made(|p, t| assert_eq!(units_of(p, t).as_deref(), Some("widget"))),
                Made(|p, t| assert_eq!(units_of(p, t).as_deref(), Some("widget"))),
                Made(|p, t| assert_eq!(units_of(p, t).as_deref(), Some("widget"))),
            ],
        ),
        (
            |t| json!({"op": "set_notes", "variable": t, "notes": " What it is. "}),
            [
                Made(|p, t| assert_eq!(notes_of(p, t), "What it is.")),
                Made(|p, t| assert_eq!(notes_of(p, t), "What it is.")),
                Made(|p, t| assert_eq!(notes_of(p, t), "What it is.")),
                Made(|p, t| assert_eq!(notes_of(p, t), "What it is.")),
            ],
        ),
        (
            |t| json!({"op": "set_lookup", "variable": t, "points": [[0, 0], [10, 5]]}),
            [
                Rejected("cannot be a lookup"),
                Made(|p, t| assert!(table_of(p, t).is_some())),
                Made(|p, t| assert!(table_of(p, t).is_some())),
                Rejected("cannot be a lookup"),
            ],
        ),
        (
            |t| json!({"op": "connect_flow", "flow": t}),
            [
                Rejected("is not a flow"),
                // Both ends clouds: the flow leaves the stock it filled.
                Made(|p, _| {
                    let Some(Variable::Stock(stock)) = p.models[0].get_variable("Level") else {
                        panic!("a stock")
                    };
                    assert!(stock.inflows.is_empty() && stock.outflows.is_empty());
                }),
                Rejected("is not a flow"),
                Rejected("is not a flow"),
            ],
        ),
        (
            |t| json!({"op": "rename", "variable": t, "to": "renamed"}),
            [
                Made(|p, t| {
                    assert!(p.models[0].get_variable(t).is_none());
                    assert_eq!(kind_of(p.models[0].get_variable("renamed").unwrap()), 0);
                }),
                Made(|p, t| {
                    assert!(p.models[0].get_variable(t).is_none());
                    let Some(Variable::Stock(stock)) = p.models[0].get_variable("Level") else {
                        panic!("a stock")
                    };
                    assert_eq!(stock.inflows, ["renamed"]);
                }),
                Made(|p, t| {
                    assert!(p.models[0].get_variable(t).is_none());
                    assert_eq!(
                        p.models[0].get_variable("filling").unwrap().get_equation(),
                        Some(&Equation::Scalar("renamed * 2".to_string()))
                    );
                }),
                Made(|p, t| {
                    assert!(p.models[0].get_variable(t).is_none());
                    let Some(Variable::Module(module)) = p.models[0].get_variable("renamed") else {
                        panic!("a module")
                    };
                    assert_eq!(module.references[0].dst, "renamed\u{00B7}input");
                    // Its reader reads it by its new name.
                    assert_eq!(
                        p.models[0].get_variable("observed").unwrap().get_equation(),
                        Some(&Equation::Scalar("renamed\u{00B7}output".to_string()))
                    );
                }),
            ],
        ),
        (
            |t| json!({"op": "delete", "variable": t}),
            [
                Made(|p, t| assert!(p.models[0].get_variable(t).is_none())),
                Made(|p, t| {
                    assert!(p.models[0].get_variable(t).is_none());
                    let Some(Variable::Stock(stock)) = p.models[0].get_variable("Level") else {
                        panic!("a stock")
                    };
                    assert!(stock.inflows.is_empty());
                }),
                // The flow reads it.
                Refused("It adds an error (filling"),
                // `observed` reads it.
                Refused("It adds an error (observed"),
            ],
        ),
        (
            // One variable is no loop, whatever its kind (a loop of each
            // kind: `a_named_loop_is_one_feedback_loop_of_the_model`).
            |t| json!({"op": "name_loop", "variables": [t], "name": "a loop"}),
            [
                Rejected("at least two variables"),
                Rejected("at least two variables"),
                Rejected("at least two variables"),
                Rejected("at least two variables"),
            ],
        ),
        (
            |_| json!({"op": "set_sim_specs", "stop": 8}),
            [
                Made(|p, _| assert_eq!(p.sim_specs.stop, 8.0)),
                Made(|p, _| assert_eq!(p.sim_specs.stop, 8.0)),
                Made(|p, _| assert_eq!(p.sim_specs.stop, 8.0)),
                Made(|p, _| assert_eq!(p.sim_specs.stop, 8.0)),
            ],
        ),
    ];
    for (row, (operation, comes)) in rows.into_iter().enumerate() {
        for (target, comes) in targets.iter().zip(comes) {
            let op = operation(target);
            let parsed: EditOperation = serde_json::from_value(op.clone()).unwrap();
            assert_eq!(
                operation_row(&parsed),
                row,
                "the rows are in the operations' order"
            );
            let mut host = Host::new(project.clone());
            let mut session = Session::new("main");
            read(&mut host, &mut session);
            let before = host.project.clone();
            let output = host.call_raw(
                &mut session,
                "edit_model",
                &operations(json!([op])).to_string(),
            );
            let answer: Value = serde_json::from_str(&output.json).unwrap();
            match comes {
                Rejected(says) => {
                    assert!(output.is_error, "{op}: {answer}");
                    assert!(
                        answer["error"].as_str().unwrap().contains(says),
                        "{op}: {answer}"
                    );
                    assert!(output.edited.is_none() && host.project == before, "{op}");
                }
                Refused(says) => {
                    assert!(output.is_error, "{op}: {answer}");
                    assert!(answer["refusedEdit"]["rule"].is_string(), "{op}: {answer}");
                    assert!(
                        answer["error"].as_str().unwrap().contains(says),
                        "{op}: {answer}"
                    );
                    assert!(output.edited.is_none() && host.project == before, "{op}");
                }
                Made(check) => {
                    assert!(output.edited.as_ref() == Some(&host.project), "{op}");
                    check(&host.project, target);
                }
            }
        }
    }
}

fn units_of(project: &datamodel::Project, name: &str) -> Option<String> {
    project.models[0].get_variable(name)?.get_units().cloned()
}

fn notes_of(project: &datamodel::Project, name: &str) -> String {
    match project.models[0].get_variable(name).unwrap() {
        Variable::Stock(v) => v.documentation.clone(),
        Variable::Flow(v) => v.documentation.clone(),
        Variable::Aux(v) => v.documentation.clone(),
        Variable::Module(v) => v.documentation.clone(),
    }
}

fn table_of(project: &datamodel::Project, name: &str) -> Option<datamodel::GraphicalFunction> {
    match project.models[0].get_variable(name)? {
        Variable::Flow(v) => v.gf.clone(),
        Variable::Aux(v) => v.gf.clone(),
        Variable::Stock(_) | Variable::Module(_) => None,
    }
}

/// What an operation refuses of its own arguments, beyond the kind of
/// variable it is given.
#[test]
fn an_operations_arguments_are_checked() {
    let mut project = inventory().build_datamodel();
    project.sim_specs.save_step = Some(datamodel::Dt::Dt(0.5));
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    for (ops, says) in [
        (
            json!([{"op": "connect_flow", "flow": "shipments", "from": "Inventory",
                    "to": "inventory"}]),
            "a flow cannot drain and fill the same stock",
        ),
        (
            json!([{"op": "set_equation", "variable": "coverage", "equation": "  "}]),
            "an equation that is not empty",
        ),
        (
            json!([{"op": "set_lookup", "variable": "coverage", "points": [[0, 1]]}]),
            "needs two points at least",
        ),
        (
            json!([{"op": "set_lookup", "variable": "coverage", "points": [[0, 1], [1, null]]}]),
            "schema",
        ),
        (
            json!([{"op": "name_loop", "name": "x"}]),
            "by its id (loop) or by its variables",
        ),
        (
            json!([{"op": "name_loop", "variables": ["coverage"], "name": " "}]),
            "a name that is not empty",
        ),
        (
            json!([{"op": "set_sim_specs", "dt": 0}]),
            "dt must be a number more than zero",
        ),
    ] {
        let refusal = host.refuse(&mut session, "edit_model", operations(ops.clone()));
        let message = refusal["error"].as_str().unwrap();
        assert!(message.contains(says), "{ops}: {message}");
    }
    // A DT past the save step leaves no save step: results are saved every
    // step. One within it keeps it.
    for (dt, save_step) in [(1.0, None), (0.125, Some(datamodel::Dt::Dt(0.5)))] {
        let mut host = Host::new(host.project.clone());
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        edit(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_sim_specs", "dt": dt}])),
        );
        assert!(host.project.sim_specs.save_step == save_step, "dt {dt}");
    }
    // A model with sim specs of its own is not edited through the
    // project's.
    let mut project = inventory().build_datamodel();
    project.models[0].sim_specs = Some(project.sim_specs.clone());
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(json!([{"op": "set_sim_specs", "stop": 30}])),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("sim specs of its own"),
        "{refusal}"
    );
    // It says what the agent can do instead.
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("run_experiment takes `specs`"),
        "{refusal}"
    );
}

/// A flow connected elsewhere leaves the stock it filled or drained, and
/// the rest of each stock's lists stay.
#[test]
fn a_flow_connected_elsewhere_leaves_the_stocks_it_left() {
    let mut host = Host::new(corpus("test-models/samples/SIR/SIR.xmile"));
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(
            json!([{"op": "connect_flow", "flow": "recovering", "from": "infectious",
                           "to": "susceptible"}]),
        ),
    );
    let flows = |name: &str| match host.project.models[0].get_variable(name) {
        Some(Variable::Stock(stock)) => (stock.inflows.clone(), stock.outflows.clone()),
        _ => panic!("a stock"),
    };
    assert_eq!(flows("recovered"), (vec![], vec![]));
    assert_eq!(
        flows("susceptible"),
        (
            vec!["recovering".to_string()],
            vec!["succumbing".to_string()]
        )
    );
    assert_eq!(
        flows("infectious"),
        (
            vec!["succumbing".to_string()],
            vec!["recovering".to_string()]
        )
    );
}

/// Every provenance letter, by a match a new one breaks.
fn every_state() -> [Option<AiState>; 9] {
    let covered = |state: AiState| match state {
        AiState::A
        | AiState::B
        | AiState::C
        | AiState::D
        | AiState::E
        | AiState::F
        | AiState::G
        | AiState::H => Some(state),
    };
    [
        None,
        covered(AiState::A),
        covered(AiState::B),
        covered(AiState::C),
        covered(AiState::D),
        covered(AiState::E),
        covered(AiState::F),
        covered(AiState::G),
        covered(AiState::H),
    ]
}

/// What an AI's edit makes of each provenance a variable can have: a row
/// per letter, each a variable of one project that one edit edits.
#[test]
fn an_edit_marks_every_provenance_a_variable_can_have() {
    let rows: Vec<(String, Option<AiState>, AiState)> = every_state()
        .into_iter()
        .enumerate()
        .map(|(i, state)| {
            let expected = match state {
                None | Some(AiState::A) | Some(AiState::B) | Some(AiState::D) => AiState::D,
                Some(AiState::C) => AiState::C,
                Some(AiState::E) | Some(AiState::F) | Some(AiState::H) => AiState::H,
                Some(AiState::G) => AiState::G,
            };
            (format!("row_{i}"), state, expected)
        })
        .collect();
    let mut project = TestProject::new("provenance")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("untouched", "1", None)
        .aux("unmarked", "1", None);
    for (name, _, _) in &rows {
        project = project.aux(name, "1", None);
    }
    let mut project = project.build_datamodel();
    let set = |project: &mut datamodel::Project, name: &str, state: Option<AiState>| {
        if let Some(Variable::Aux(aux)) = project.models[0].get_variable_mut(name) {
            aux.ai_state = state;
        }
    };
    for (name, state, _) in &rows {
        set(&mut project, name, *state);
    }
    // A letter on a variable is what says the project records them.
    set(&mut project, "untouched", Some(AiState::F));
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let mut ops: Vec<Value> = rows
        .iter()
        .map(|(name, _, _)| json!({"op": "set_notes", "variable": name, "notes": "noted"}))
        .collect();
    // Added, then renamed: still what the edit added.
    ops.extend([
        json!({"op": "add_variable", "name": "first_name", "equation": "1"}),
        json!({"op": "rename", "variable": "first_name", "to": "second_name"}),
        json!({"op": "rename", "variable": "second_name", "to": "third_name"}),
    ]);
    edit(&mut host, &mut session, operations(Value::Array(ops)));
    let letter = |name: &str| {
        host.project.models[0]
            .get_variable(name)
            .and_then(Variable::get_ai_state)
    };
    for (name, state, expected) in &rows {
        assert!(
            letter(name) == Some(*expected),
            "{name}, which had {}",
            if state.is_some() { "a letter" } else { "none" }
        );
    }
    assert!(letter("third_name") == Some(AiState::C));
    assert!(
        letter("untouched") == Some(AiState::F),
        "a variable the edit does not name keeps its letter"
    );
    assert!(letter("unmarked").is_none(), "and one with none gets none");
}

/// Renames made one after another in one edit are one rename: a line from
/// the first name to the last, the readers rewritten once, and an error the
/// variable had still its own.
#[test]
fn renames_in_a_row_are_one_rename() {
    let project = TestProject::new("broken")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "rate", None)
        .aux("rate", "missing_input * 2", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "rename", "variable": "rate", "to": "pace"},
            {"op": "rename", "variable": "pace", "to": "tempo"}
        ])),
    );
    assert!(output.get("diagnostics").is_none(), "{output}");
    assert_eq!(
        output["changes"],
        json!([
            {"variable": "inflow", "action": "changed", "detail": "equation tempo (was rate)"},
            {"variable": "rate", "action": "renamed", "detail": "renamed to tempo"}
        ])
    );
}

/// A value that is not a number is the edit's doing when the run before it
/// had a number there, or had no such variable; one that was not a number
/// already, under the name the edit renames it from, is the model's own.
#[test]
fn a_value_not_a_number_is_judged_against_the_run_before_the_edit() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "ratio", "equation": "0 / 0"}])),
    );
    assert!(
        output["error"]
            .as_str()
            .unwrap()
            .contains("It makes ratio not a number"),
        "{output}"
    );

    let mut project = inventory().build_datamodel();
    project.models[0].variables.push(
        TestProject::new("x")
            .aux("undefined", "0 / 0", None)
            .build_datamodel()
            .models[0]
            .variables[0]
            .clone(),
    );
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "rename", "variable": "undefined", "to": "not_yet_defined"}])),
    );
}

/// Whether a warning of each category refuses an edit as a model's first:
/// the unit categories do, the others are listed. A match a new category
/// breaks says which is which.
#[test]
fn a_first_warning_of_each_unit_category_refuses_the_edit() {
    let refuses = |category: DiagnosticCategoryName| match category {
        DiagnosticCategoryName::UnitDefinition
        | DiagnosticCategoryName::UnitConsistency
        | DiagnosticCategoryName::UnitInference => true,
        DiagnosticCategoryName::Model => false,
        // No warning of these categories exists: the engine's equation and
        // assembly diagnostics are errors, and a value that is not a number
        // is the gate's own, an error.
        DiagnosticCategoryName::Equation
        | DiagnosticCategoryName::Assembly
        | DiagnosticCategoryName::Value => false,
    };
    // (the category, an edit that gives the unit-clean model a warning of it)
    let rows = [
        (
            DiagnosticCategoryName::UnitConsistency,
            json!([{"op": "set_equation", "variable": "shipments",
                    "equation": "MIN(orders, Inventory)"}]),
        ),
        (
            DiagnosticCategoryName::UnitDefinition,
            json!([{"op": "add_stock", "name": "Shipped", "initial": "0", "units": "gadget"},
                   {"op": "connect_flow", "flow": "shipments", "from": "Inventory",
                    "to": "Shipped"}]),
        ),
        (
            DiagnosticCategoryName::UnitInference,
            json!([{"op": "add_stock", "name": "Shipped", "initial": "coverage"},
                   {"op": "connect_flow", "flow": "shipments", "from": "Inventory",
                    "to": "Shipped"}]),
        ),
        (
            DiagnosticCategoryName::Model,
            json!([{"op": "add_variable", "name": "placeholder", "equation": "NaN"}]),
        ),
    ];
    // A placeholder's value is not a number, which the values rule would
    // refuse before the warnings are asked about.
    let policy = GatePolicy {
        values: false,
        ..GatePolicy::AGENT_EDIT
    };
    let project = inventory().build_datamodel();
    for (category, ops) in rows {
        let (refusal, listed) = refusal_under(&project, ops.clone(), &policy);
        assert!(
            listed
                .iter()
                .any(|d| d.severity == Severity::Warning && d.category == category),
            "{ops}: the premise, a warning of its category"
        );
        if refuses(category) {
            let (rule, refusal) = refusal.unwrap_or_else(|| panic!("{ops}: not refused"));
            assert_eq!(rule, GateRule::UnitWarnings, "{refusal}");
            assert!(refusal.contains("its first"), "{refusal}");
        } else {
            assert_eq!(refusal, None, "{ops}");
        }
    }
}

/// A unit warning is a model's first when that model had none, whatever
/// the project's other models have.
#[test]
fn a_first_unit_warning_is_the_first_of_its_own_model() {
    let mut project = one_of_each_kind();
    // The module's model declares units that disagree: a warning of its own.
    for (name, units) in [("input", "widget"), ("output", "gadget")] {
        if let Some(Variable::Aux(aux)) = project.models[1].get_variable_mut(name) {
            aux.units = Some(units.to_string());
        }
    }
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let outline = read(&mut host, &mut session);
    assert!(
        outline.get("diagnostics").is_none(),
        "the premise: the root model has no warning of its own: {outline}"
    );
    let mut component = Session::new("component");
    let outline = read(&mut host, &mut component);
    assert!(
        outline["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["category"] == "unit_consistency"),
        "the premise: the module's model has a unit warning: {outline}"
    );
    let output = refused(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "set_units", "variable": "rate", "units": "widget"},
            {"op": "set_units", "variable": "idle", "units": "gadget"},
            {"op": "add_variable", "name": "mixed", "equation": "rate + idle", "units": "widget"}
        ])),
    );
    assert!(
        output["error"].as_str().unwrap().contains("its first"),
        "{output}"
    );
}

/// An edit the gate passes is made: the call answers with the project as
/// the edit leaves it, for its host to make the project's contents, and
/// changes no project itself. A refused edit, an operation that cannot
/// apply and an edit that changes nothing hand the host nothing.
#[test]
fn an_edit_is_made_when_its_gate_passes_and_handed_to_the_host() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let before = host.project.clone();
    // The call alone, without the host's half.
    let output = session
        .call(host.workspace(), "edit_model", &backlog().to_string())
        .unwrap();
    assert!(!output.is_error, "{}", output.json);
    let answer: Value = serde_json::from_str(&output.json).unwrap();
    assert_eq!(answer["revision"], 0);
    assert_eq!(answer["simulates"], true);
    assert!(answer.get("unchanged").is_none(), "{answer}");
    assert_eq!(
        answer["changes"],
        json!([
            {"variable": "Backlog", "action": "added",
             "detail": "stock, initial value 0, filled by ordering, drained by fulfilling (widget)"},
            {"variable": "fulfilling", "action": "added",
             "detail": "flow = shipments (widget/month)"},
            {"variable": "ordering", "action": "added", "detail": "flow = orders (widget/month)"}
        ])
    );
    assert!(host.project == before, "the engine changes no project");
    let edited = output
        .edited
        .clone()
        .expect("the edit is the host's to make");
    let model = &edited.models[0];
    let Some(datamodel::Variable::Stock(stock)) = model.get_variable("backlog") else {
        panic!("the edit added the stock");
    };
    assert_eq!(stock.inflows, ["ordering"]);
    assert_eq!(stock.outflows, ["fulfilling"]);
    let datamodel::View::StockFlow(view) = &model.views[0];
    assert!(
        view.elements.iter().any(|e| matches!(
            e,
            datamodel::ViewElement::Stock(s) if s.name == "Backlog"
        )),
        "the edit places what it adds on the diagram"
    );
    // The host's half: the contents replaced in one edit, at a new revision.
    host.commit(&output);
    let behavior = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["Backlog"]}),
    );
    assert_eq!(behavior["revision"], 1);

    let before = host.project.clone();
    for (input, is_error) in [
        // The gate refuses it.
        (
            operations(json!([{"op": "set_equation", "variable": "coverage",
                               "equation": "nothing_here"}])),
            true,
        ),
        // It cannot apply.
        (
            operations(
                json!([{"op": "set_units", "variable": "coverage", "units": "week"},
                              {"op": "delete", "variable": "no_such_variable"}]),
            ),
            true,
        ),
        // It changes nothing.
        (
            operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "4"}])),
            false,
        ),
    ] {
        let output = host.call_raw(&mut session, "edit_model", &input.to_string());
        assert_eq!(output.is_error, is_error, "{input}: {}", output.json);
        assert!(output.edited.is_none(), "{input}");
        assert!(host.project == before, "{input}");
        assert_eq!(host.revision, 1, "{input}");
    }
    let unchanged = host.call(
        &mut session,
        "edit_model",
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "4"}])),
    );
    assert_eq!(unchanged["changes"], json!([]));
    assert_eq!(unchanged["unchanged"], true, "{unchanged}");
    assert!(
        unchanged["note"]
            .as_str()
            .unwrap()
            .contains("nothing changed"),
        "{unchanged}"
    );
}

/// `edit_model` is the one tool whose effect is an edit, and no other tool
/// hands its host a project.
#[test]
fn edit_model_is_the_one_tool_that_edits() {
    use crate::tools::{ToolEffect, ToolName};
    for tool in ToolName::ALL {
        let expected = match tool.effect() {
            ToolEffect::Edit => tool == ToolName::EditModel,
            ToolEffect::Read => tool != ToolName::EditModel,
        };
        assert!(expected, "{}", tool.name());
    }
    let catalog: Value = serde_json::from_str(crate::tools::catalog_json()).unwrap();
    let effects: Vec<(&str, &str)> = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["name"].as_str().unwrap(), t["effect"].as_str().unwrap()))
        .collect();
    for (name, effect) in effects {
        assert_eq!(effect == "edit", name == "edit_model", "{name}: {effect}");
        assert!(effect == "edit" || effect == "read", "{name}: {effect}");
    }
}

/// What a session's own edit changed is as read from then on: its next edit
/// of it is made with no read between, and the change report has only what
/// someone else changed since. An edit the gate refused changed nothing, so
/// it leaves what the session read as it was.
#[test]
fn what_a_sessions_own_edit_changed_is_as_read() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let set = |value: &str| {
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": value}]))
    };
    edit(&mut host, &mut session, set("5"));
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none(),
        "the agent's own edit is no news to it"
    );
    let second = edit(&mut host, &mut session, set("6"));
    assert_eq!(second["changes"][0]["detail"], "equation 6 (was 5)");
    refused(&mut host, &mut session, set("nothing_here"));
    let third = edit(&mut host, &mut session, set("7"));
    assert_eq!(third["changes"][0]["detail"], "equation 7 (was 6)");

    // What the agent added, the sim specs it set and a variable it deleted
    // are its own too.
    edit(&mut host, &mut session, backlog());
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 40},
                          {"op": "delete", "variable": "fulfilling"}])),
    );
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none()
    );

    // The person's changes since are news, to a variable the agent's edit
    // changed as to any other.
    host.edit(|p| {
        let model = &mut p.models[0];
        model
            .get_variable_mut("coverage")
            .unwrap()
            .set_scalar_equation("8");
        model
            .get_variable_mut("ordering")
            .unwrap()
            .set_scalar_equation("orders * 2");
        p.sim_specs.dt = datamodel::Dt::Reciprocal(16.0);
    });
    let changes = session
        .changes_since_read(&host.project, host.revision)
        .unwrap()
        .expect("the person's edits are news");
    let changed: Vec<&str> = changes.changed.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(changed, ["coverage", "ordering"]);
    assert!(changes.added.is_empty() && changes.removed.is_empty());
    assert!(changes.specs_changed);
    let outline = read(&mut host, &mut session);
    assert_eq!(
        outline["changes"]["changed"],
        json!([{"name": "coverage", "fields": ["equation"]},
               {"name": "ordering", "fields": ["equation"]}])
    );
    // And an edit of one of them, unread, is refused.
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("coverage")
            .unwrap()
            .set_scalar_equation("9")
    });
    let refusal = host.refuse(&mut session, "edit_model", set("10"));
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("coverage changed since you last read"),
        "{refusal}"
    );
}

/// `"main"` names the project's first model when none is called that: an
/// edit is made of that model, and what it changed is as read.
#[test]
fn a_session_named_main_edits_a_first_model_of_another_name() {
    let mut project = inventory().build_datamodel();
    project.models[0].name = "inventory".to_string();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let set = |value: &str| {
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": value}]))
    };
    edit(&mut host, &mut session, set("6"));
    assert_eq!(
        host.project.models[0]
            .get_variable("coverage")
            .unwrap()
            .get_equation(),
        Some(&Equation::Scalar("6".to_string()))
    );
    let again = edit(&mut host, &mut session, set("7"));
    assert_eq!(again["changes"][0]["detail"], "equation 7 (was 6)");
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none()
    );
}

/// The diagram is the person's: an edit is made around it as it is, whatever
/// the person moved since the agent read the model, and a diagram edit makes
/// nothing the agent read stale.
#[test]
fn an_edit_is_made_on_the_diagram_as_the_person_left_it() {
    let mut host = drawn();
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let moved = stock_x(&host, "Inventory") + 200.0;
    host.edit(|p| {
        let datamodel::View::StockFlow(view) = &mut p.models[0].views[0];
        view.elements.edit_where(
            |e| matches!(e, datamodel::ViewElement::Stock(_)),
            |e| {
                if let datamodel::ViewElement::Stock(s) = e {
                    s.x = moved;
                }
            },
        );
    });
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "doubling_time",
                           "equation": "0.7 * coverage"}])),
    );
    assert_eq!(
        stock_x(&host, "Inventory"),
        moved,
        "the person's move stays"
    );
    assert!(
        host.project.models[0]
            .get_variable("doubling_time")
            .is_some()
    );
}

/// An edit's changes reach the models that instantiate the one it is of: a
/// renamed input is rewired in its parents, which the answer lists with
/// their model, which must be as read, and which are the session's own once
/// the edit is made.
#[test]
fn a_rename_of_a_module_input_rewires_the_parents_and_lists_them() {
    let mut host = Host::new(hares_and_lynxes());
    let mut session = Session::new("hares");
    read(&mut host, &mut session);
    let rename =
        |to: &str, from: &str| operations(json!([{"op": "rename", "variable": from, "to": to}]));
    // The parent's module, changed since the read, is not overwritten.
    let before = host.project.clone();
    host.edit(|p| {
        if let Some(Variable::Module(module)) = p.models[0].get_variable_mut("hares") {
            module.documentation = "the person's note".to_string();
        }
    });
    let refusal = host.refuse(&mut session, "edit_model", rename("range", "area"));
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("hares (in main) changed since you last read"),
        "{refusal}"
    );
    host.edit(|p| *p = before.clone());
    // Nor is the input itself, in the session's model, which is not the
    // project's first.
    host.edit(|p| {
        let model = p.models.iter_mut().find(|m| m.name == "hares").unwrap();
        if let Some(Variable::Aux(aux)) = model.get_variable_mut("area") {
            aux.documentation = "the person's note".to_string();
        }
    });
    let refusal = host.refuse(&mut session, "edit_model", rename("range", "area"));
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .starts_with("area changed since you last read"),
        "{refusal}"
    );
    host.edit(|p| *p = before);

    let output = edit(&mut host, &mut session, rename("range", "area"));
    // The session's model's lines, then the other models'.
    let changes = output["changes"].as_array().unwrap();
    assert_eq!(changes[0]["variable"], "area", "{output}");
    assert_eq!(changes[0]["action"], "renamed", "{output}");
    assert_eq!(
        changes.last().unwrap(),
        &json!({
            "variable": "hares (in main)", "action": "changed",
            "detail": "input range from area (was input area)"
        }),
        "{output}"
    );
    // The model's error on the input ("'area' has no equation") names it, so
    // its reason changes with the name: the same error still, and no news.
    assert!(output.get("diagnostics").is_none(), "{output}");
    assert!(root_simulates(&mut host));
    // The parent's module is the session's own now: renaming the input
    // again rewires it again, with no read between.
    edit(&mut host, &mut session, rename("territory", "range"));
    assert!(root_simulates(&mut host));
}

/// A name a rename leaves is free for the same edit to take: the renamed
/// variable has its line and the one added under its old name has its own,
/// and both are the session's as it made them.
#[test]
fn a_name_a_rename_leaves_is_taken_by_a_variable_with_a_line_of_its_own() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "rename", "variable": "coverage", "to": "months held"},
            {"op": "add_variable", "name": "coverage", "equation": "3"}
        ])),
    );
    let lines: Vec<(&str, &str, &str)> = output["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["variable"] == "coverage")
        .map(|line| {
            (
                line["variable"].as_str().unwrap(),
                line["action"].as_str().unwrap(),
                line["detail"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        lines,
        [
            ("coverage", "renamed", "renamed to months held"),
            ("coverage", "added", "variable = 3"),
        ],
        "{output}"
    );
    let model = &host.project.models[0];
    assert_eq!(equation_text_of(model, "months_held"), "4");
    assert_eq!(equation_text_of(model, "coverage"), "3");
    edit(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "set_equation", "variable": "coverage", "equation": "5"},
            {"op": "set_equation", "variable": "months held", "equation": "6"}
        ])),
    );
}

fn equation_text_of(model: &datamodel::Model, name: &str) -> String {
    match model.get_variable(name).and_then(Variable::get_equation) {
        Some(Equation::Scalar(text)) => text.clone(),
        _ => panic!("{name} has a scalar equation"),
    }
}

/// A call that stops for other work, at any stage of its gate -- the model's
/// run it compares with, the edit's diagnostics, the edit's run -- or between
/// the slices of the model's run, makes no edit and leaves the host's
/// database and the session as it found them: the staging guard restores the
/// database as the call stops, and what the session read is as it was.
#[test]
fn an_edit_that_stops_changes_nothing() {
    let broken = operations(json!([
        {"op": "set_equation", "variable": "coverage", "equation": "nothing_here * 2"}
    ]));
    let good = operations(json!([{"op": "set_notes", "variable": "coverage", "notes": "noted"}]));
    let mut stopped = 0;
    // One host for every stop: each leaves it as it found it. A session of
    // its own for each, so each makes the model's run anew.
    let mut host = Host::from_test_project(&inventory());
    for after in 1.. {
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        let output = host.call_waiting(
            &mut session,
            "edit_model",
            broken.clone(),
            &Host::waiting_after(after),
        );
        assert!(output.is_error, "the gate refuses it: {}", output.json);
        assert!(output.edited.is_none());
        assert_eq!(host.revision, 0);
        if !output.interrupted {
            let refusal: Value = serde_json::from_str(&output.json).unwrap();
            assert_eq!(refusal["refusedEdit"]["rule"], "errors", "{refusal}");
            break;
        }
        stopped += 1;
        let outline = read(&mut host, &mut session);
        assert!(
            outline.get("diagnostics").is_none(),
            "the database is the model's, not the edit's: {outline}"
        );
    }
    // The broken edit does not simulate, so its own run is not sliced.
    let between_slices = crate::tools::runs::RUN_SLICES as usize - 1;
    assert_eq!(
        stopped,
        3 + between_slices,
        "three stages, and between the slices of the model's run"
    );

    // An edit the gate would pass, stopped at points across the call (every
    // fifth of the checkpoints it asks at, its last stages among them):
    // never made, and made when it is called again.
    for after in (1..).step_by(5) {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        let output = host.call_waiting(
            &mut session,
            "edit_model",
            good.clone(),
            &Host::waiting_after(after),
        );
        if !output.is_error {
            assert!(output.edited.is_some());
            break;
        }
        assert!(
            output.interrupted && output.edited.is_none(),
            "{}",
            output.json
        );
        assert_eq!(host.revision, 0);
        let made = edit(&mut host, &mut session, good.clone());
        assert_eq!(made["changes"][0]["detail"], "notes");
    }
}

/// An edit on the corpus's largest models: a rename of a constant, which
/// rewrites its readers and is gated on the whole project, is made, and the
/// next edit of what it changed needs no read. The times are printed.
#[test]
#[ignore = "edits World3 and C-LEARN; run under the gates profile"]
fn an_edit_is_made_on_the_largest_models() {
    for path in [
        "metasd/WRLD3-03/wrld3-03.mdl",
        "xmutil_test_models/C-LEARN v77 for Vensim.mdl",
    ] {
        let mut host = Host::new(corpus(path));
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        let constant = host.project.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                Variable::Aux(aux)
                    if matches!(&aux.equation, Equation::Scalar(text)
                        if text.trim().parse::<f64>().is_ok()) =>
                {
                    Some(aux.ident.clone())
                }
                _ => None,
            })
            .expect("the model has a constant");
        let renamed = format!("{constant} renamed");
        let started = std::time::Instant::now();
        edit(
            &mut host,
            &mut session,
            operations(json!([{"op": "rename", "variable": constant, "to": renamed}])),
        );
        let first = started.elapsed();
        assert!(host.project.models[0].get_variable(&renamed).is_some());
        let started = std::time::Instant::now();
        edit(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_notes", "variable": renamed, "notes": "renamed"}])),
        );
        eprintln!(
            "{path}: {} variables; a rename made in {first:?}, a note in {:?}",
            host.project.models[0].variables.len(),
            started.elapsed()
        );
    }
}

/// An edit that changes no record is no edit, whatever its operations wrote:
/// in a project that records provenance it marks none, on a model with no
/// diagram it draws none, and an empty units string where there were none
/// is no change. Each answers `unchanged` and hands the host nothing.
#[test]
fn an_edit_that_changes_no_record_marks_nothing_and_draws_nothing() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/ai-information/GeneratedByAIThenEdited.stmx");
    let file = std::fs::File::open(path).expect("the model is in the corpus");
    let project = crate::xmile::project_from_reader(&mut std::io::BufReader::new(file)).unwrap();
    let viewless = crate::test_common::TestProject::new("viewless")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("level", "10", &["inflow"], &[], None)
        .flow("inflow", "level * rate", None)
        .aux("rate", "0.1", None)
        .build_datamodel();
    assert!(viewless.models[0].views.is_empty(), "the premise");
    let marketing = project.models[0]
        .get_variable("marketing_spend")
        .and_then(Variable::get_equation)
        .map(|equation| equation.source_text().to_string())
        .unwrap();
    for (project, ops) in [
        (
            project,
            json!([{"op": "set_equation", "variable": "marketing spend", "equation": marketing}]),
        ),
        (
            viewless.clone(),
            json!([{"op": "set_equation", "variable": "inflow", "equation": "level * rate"}]),
        ),
        (
            viewless,
            json!([{"op": "set_units", "variable": "rate", "units": ""}]),
        ),
    ] {
        let mut host = Host::new(project);
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        let before = host.project.clone();
        let output = host.call_raw(
            &mut session,
            "edit_model",
            &operations(ops.clone()).to_string(),
        );
        assert!(!output.is_error, "{ops}: {}", output.json);
        assert!(output.edited.is_none(), "{ops}: the host is handed nothing");
        assert!(host.project == before, "{ops}");
        let answer: Value = serde_json::from_str(&output.json).unwrap();
        assert_eq!(answer["unchanged"], true, "{ops}: {answer}");
    }
}

/// The gate asks its rules in order (`GateRule::ALL`): an edit that adds an
/// error and asks more of a run than a run may is refused for the error,
/// the one a repair starts from. The session's model here is not the root,
/// so the root still runs, and costs.
#[test]
fn an_edit_that_breaks_a_rule_and_an_earlier_one_is_refused_by_the_earlier() {
    let mut project = inventory().build_datamodel();
    let mut other = TestProject::new("other")
        .aux("spare", "1", None)
        .build_datamodel()
        .models
        .remove(0);
    other.name = "other".to_string();
    project.models.push(other);
    let mut host = Host::new(project);
    let mut session = Session::new("other");
    read(&mut host, &mut session);
    let refusal = refused(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "add_variable", "name": "broken", "equation": "no_such_input + 1"},
            {"op": "set_sim_specs", "dt": 0.0000001}
        ])),
    );
    assert_eq!(refusal["refusedEdit"]["rule"], "errors", "{refusal}");
    // Each alone is refused by its own rule.
    let refusal = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "dt": 0.0000001}])),
    );
    assert_eq!(refusal["refusedEdit"]["rule"], "run_cost", "{refusal}");
}

/// An error the model had before the edit is no news when the edit adds
/// another of its kind: the refusal lists the one the edit adds, and the
/// one the model had is left out.
#[test]
fn an_error_the_model_had_is_not_listed_beside_a_new_one_of_its_kind() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("coverage")
        .unwrap()
        .set_scalar_equation("no_such_cover");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let refusal = refused(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "add_variable", "name": "broken", "equation": "no_such_input + 1"}
        ])),
    );
    let listed: Vec<&str> = refusal["refusedEdit"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["severity"] == "error")
        .map(|d| d["variable"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(listed, ["broken"], "{refusal}");
}

/// A lookup's x values never decrease: two neighbouring points at one x are
/// a vertical step, which the engine reads, and points that go back are
/// refused.
#[test]
fn a_lookup_may_step_at_one_x_and_may_not_go_back() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    for points in [json!([[1, 0], [0, 1]]), json!([[0, 0], [2, 1], [1, 2]])] {
        let refusal = rejected(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_lookup", "variable": "coverage", "points": points}])),
        );
        assert!(
            refusal["error"]
                .as_str()
                .unwrap()
                .contains("must not decrease"),
            "{points}: {refusal}"
        );
    }
    for points in [
        json!([[0, 0], [1, 0], [1, 2], [2, 2]]),
        json!([[0, 0], [1, 1]]),
    ] {
        edit(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_lookup", "variable": "coverage", "points": points}])),
        );
    }
}

/// A refused edit gives no ids: the diagnostics it would have added are
/// not the model's, so the next problem the model has is numbered as though
/// the edit had never been asked.
#[test]
fn a_refused_edit_gives_its_diagnostics_no_ids() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    refused(
        &mut host,
        &mut session,
        operations(
            json!([{"op": "set_equation", "variable": "coverage", "equation": "no_such_cover"}]),
        ),
    );
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("adjustment_time")
            .unwrap()
            .set_scalar_equation("no_such_time");
    });
    let outline = read(&mut host, &mut session);
    let diagnostics = outline["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{outline}");
    assert_eq!(diagnostics[0]["id"], "D1", "{outline}");
}

/// Only an edit that can change what the diagram draws is drawn
/// (`EditOperation::structural`): each operation that cannot leaves the
/// diagram as it is, even with a variable undrawn that a structural edit
/// places.
#[test]
fn an_edit_the_diagram_does_not_show_draws_nothing() {
    let mut project = crate::tools::test_support::with_diagram(inventory().build_datamodel());
    let undrawn = Variable::Aux(datamodel::Aux {
        ident: "undrawn".to_string(),
        equation: Equation::Scalar("1".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    project.models[0].variables.push(undrawn);
    let views = project.models[0].views.clone();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    // A row per operation that is not structural.
    for op in [
        json!({"op": "set_units", "variable": "undrawn", "units": "widget"}),
        json!({"op": "set_notes", "variable": "undrawn", "notes": "a note"}),
        json!({"op": "set_sim_specs", "stop": 10}),
        json!({"op": "name_loop", "variables": ["Inventory", "production"], "name": "a loop"}),
    ] {
        let parsed: EditOperation = serde_json::from_value(op.clone()).unwrap();
        assert!(!parsed.structural(), "{op}");
        edit(&mut host, &mut session, operations(json!([op])));
        assert_eq!(host.project.models[0].views, views, "{op}");
    }
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "drawn", "equation": "undrawn"}])),
    );
    assert_ne!(
        host.project.models[0].views, views,
        "a structural edit is drawn"
    );
}

/// What an answer's lines say: names as the model spells them, an added
/// variable's lookup, what a variable whose kind changed is now computed
/// from, a lookup's points, a save step a larger DT drops, and numbers
/// written as a person reads them.
#[test]
fn a_line_says_what_changed_as_the_model_and_a_person_read_it() {
    let mut project = TestProject::new("lines")
        .with_sim_time(0.0, 20.0, 1.0)
        .flow("New Orders", "1", None)
        .aux("spare", "1", None)
        .build_datamodel();
    project.sim_specs.save_step = Some(datamodel::Dt::Dt(2.0));
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let detail = |output: &Value, variable: &str| -> String {
        output["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|line| line["variable"] == variable)
            .unwrap_or_else(|| panic!("{variable}: {output}"))["detail"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "add_stock", "name": "Backlog", "initial": "0", "inflows": ["new orders"]},
            {"op": "add_variable", "name": "effect", "equation": "spare"},
            {"op": "set_lookup", "variable": "effect", "points": [[0, 0], [1, 2], [2, 3]]},
            {"op": "delete", "variable": "spare"},
            {"op": "add_stock", "name": "spare", "initial": "5"}
        ])),
    );
    assert_eq!(
        detail(&output, "Backlog"),
        "stock, initial value 0, filled by New Orders"
    );
    assert!(
        detail(&output, "effect").contains("through a lookup of 3 points, x 0 to 2, y 0 to 3"),
        "{output}"
    );
    assert_eq!(
        detail(&output, "spare"),
        "now a stock (was a variable), initial value 5 (was equation 1)"
    );
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_lookup", "variable": "effect", "points": [[0, 0], [4, 1]]}])),
    );
    assert_eq!(
        detail(&output, "effect"),
        "a lookup of 2 points, x 0 to 4, y 0 to 1 (was a lookup of 3 points, x 0 to 2, y 0 to 3)"
    );
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_lookup", "variable": "effect", "points": [[0, 1], [4, 0]]}])),
    );
    assert_eq!(
        detail(&output, "effect"),
        "a lookup of 2 points, x 0 to 4, y 0 to 1, its points moved"
    );
    // A number too large to read written out is written with an exponent,
    // in the lines and in the advice of the refusal.
    let refusal = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 1e17}])),
    );
    assert_eq!(
        detail(&refusal["refusedEdit"], "sim specs"),
        "stop 1e17 (was 20)"
    );
    assert!(
        refusal["error"].as_str().unwrap().contains("4e17 numbers"),
        "{refusal}"
    );
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "dt": 4, "stop": 40}])),
    );
    assert_eq!(
        detail(&output, "sim specs"),
        "stop 40 (was 20); dt 4 (was 1); saves every step (was every 2)"
    );
}

/// Specs no run can be made under are refused where they are written: a
/// DT longer than the run takes no step, and a run's length must be a
/// number; an experiment's are refused as an edit's are.
#[test]
fn specs_no_run_can_be_made_under_are_refused() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    for (specs, says) in [
        (json!({"dt": 100}), "is longer than the run"),
        (json!({"dt": 1e300}), "is longer than the run"),
        (
            json!({"start": -1e308, "stop": 1e308}),
            "not one a computer's numbers can take",
        ),
    ] {
        let mut op = json!({"op": "set_sim_specs"});
        op.as_object_mut()
            .unwrap()
            .extend(specs.as_object().unwrap().clone());
        let refusal = rejected(&mut host, &mut session, operations(json!([op])));
        let reason = refusal["error"].as_str().unwrap();
        assert!(
            reason.starts_with("operation 1 (set_sim_specs): ") && reason.contains(says),
            "{specs}: {reason}"
        );
        assert!(!reason.contains("NaN") && reason.len() < 300, "{reason}");
        let refusal = host.refuse(
            &mut session,
            "run_experiment",
            json!({"name": "e", "specs": specs}),
        );
        let reason = refusal["error"].as_str().unwrap();
        assert!(reason.contains(says), "{specs}: {reason}");
    }
}

/// An element asked of a variable with none says so, and suggests no
/// element: a scalar has no element names to offer.
#[test]
fn an_element_of_a_scalar_suggests_nothing() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let refusal = rejected(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "set_equation", "variable": "coverage", "element": "north", "equation": "1"}
        ])),
    );
    assert!(
        refusal["error"].as_str().unwrap().contains("not arrayed"),
        "{refusal}"
    );
    assert!(
        refusal["suggestions"].as_array().is_none_or(|names| names
            .iter()
            .all(|n| n.as_str().is_some_and(|n| !n.is_empty()))),
        "{refusal}"
    );
    // An element written into the name is pointed at the field it goes in.
    let refusal = rejected(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "set_equation", "variable": "coverage[north]", "equation": "1"}
        ])),
    );
    assert!(
        refusal["error"].as_str().unwrap().contains("`element`"),
        "{refusal}"
    );
}

/// A hub many variables read, each also naming what the model lacks (so a
/// delete of the hub adds them no error, and is refused for what still
/// names it): what a delete of it, and units every reader disagrees with,
/// answer at most.
fn hub_with_readers(readers: usize, broken: bool) -> TestProject {
    let mut project = TestProject::new("hub").aux("hub", "5", Some("widget"));
    let equation = if broken {
        "hub * 2 + missing_thing"
    } else {
        "hub * 2"
    };
    for i in 0..readers {
        project = project.aux(
            &format!("reader_number_{i}_of_the_hub"),
            equation,
            Some("widget"),
        );
    }
    project
}

/// A delete every reader still names is refused naming a few of them and
/// counting the rest; an edit's diagnostics are fitted to the budget, the
/// last left out and counted, as its lines are.
#[test]
fn an_edits_answer_names_and_lists_what_fits_and_counts_the_rest() {
    let mut host = Host::from_test_project(&hub_with_readers(40, true));
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let refusal = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "delete", "variable": "hub"}])),
    );
    let reason = refusal["error"].as_str().unwrap();
    assert!(
        reason.contains(&format!("and {} more", 40 - crate::tools::MAX_NAMED)),
        "{reason}"
    );
    let named = reason.split("still name").next().unwrap();
    assert_eq!(
        named.matches("reader_number_").count(),
        crate::tools::MAX_NAMED,
        "{reason}"
    );

    let mut host = Host::from_test_project(&hub_with_readers(40, false));
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    session.outline_budget = 1_500;
    let output = host.call_raw(
        &mut session,
        "edit_model",
        &operations(json!([{"op": "set_units", "variable": "hub", "units": "gadget"}])).to_string(),
    );
    assert!(output.json.len() <= 1_500, "{}", output.json.len());
    let answer: Value = serde_json::from_str(&output.json).unwrap();
    let answer = if output.is_error {
        &answer["refusedEdit"]
    } else {
        &answer
    };
    let listed = answer["diagnostics"].as_array().map_or(0, Vec::len);
    let omitted = answer["omittedDiagnostics"].as_u64().unwrap_or(0) as usize;
    assert!(listed >= 1 && omitted > 0, "{answer}");
}

/// What changed since the read is named up to a few, the rest counted: a
/// person's large edit refuses an agent's in a sentence, not a list.
#[test]
fn a_stale_edit_names_a_few_of_what_changed_and_counts_the_rest() {
    let mut host = Host::from_test_project(&hub_with_readers(20, false));
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    host.edit(|p| {
        for i in 0..20 {
            p.models[0]
                .get_variable_mut(&format!("reader_number_{i}_of_the_hub"))
                .unwrap()
                .set_scalar_equation("hub * 3");
        }
    });
    // A rename of the hub respells every reader, each changed since the
    // read.
    let refusal = rejected(
        &mut host,
        &mut session,
        operations(json!([{"op": "rename", "variable": "hub", "to": "center"}])),
    );
    let reason = refusal["error"].as_str().unwrap();
    assert!(reason.contains("changed since you last read"), "{reason}");
    assert_eq!(
        reason.matches("reader_number_").count(),
        crate::tools::MAX_NAMED,
        "{reason}"
    );
    assert!(
        reason.contains(&format!("and {} more", 20 - crate::tools::MAX_NAMED)),
        "{reason}"
    );
}

/// A repair that brings a model back to life says which series are not a
/// number in its run, as warnings: no rule refuses them (the model had no
/// run to compare with), and the agent should not read `simulates` as all
/// is well.
#[test]
fn a_model_brought_back_to_life_names_its_undefined_series() {
    let project = TestProject::new("revived")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("level", "1", &["inflow"], &[], None)
        .flow("inflow", "helper", None)
        .aux("helper", "0/0", None)
        .aux("broken", "no_such_input + 1", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "broken", "equation": "1"}])),
    );
    assert_eq!(output["simulates"], true, "{output}");
    let undefined: Vec<&str> = output["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "non_finite" && d["severity"] == "warning")
        .map(|d| d["variable"].as_str().unwrap())
        .collect();
    assert!(
        undefined.contains(&"level") && undefined.contains(&"helper"),
        "{output}"
    );
}

/// One equation written over per-element ones says it now holds for every
/// element.
#[test]
fn one_equation_over_per_element_ones_says_every_element() {
    let project = TestProject::new("regions")
        .named_dimension("region", &["north", "south"])
        .array_with_ranges("growth[region]", vec![("north", "10"), ("south", "20")]);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "growth", "equation": "5"}])),
    );
    let detail = output["changes"][0]["detail"].as_str().unwrap();
    assert!(
        detail.starts_with("equation 5 for every element (was "),
        "{detail}"
    );
}
