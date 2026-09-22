// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

/// Call `edit_model`, which must answer, and check the answer against the
/// output schema the catalog publishes.
fn edit(host: &mut Host, session: &mut Session, input: Value) -> Value {
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
    if output["verdict"] == "ready" {
        assert!(output["plan"].is_string(), "{output}");
    } else {
        assert!(output.get("plan").is_none(), "{output}");
    }
    output
}

/// An edit of `operations` with a summary.
fn operations(operations: Value) -> Value {
    json!({"summary": "an edit", "operations": operations})
}

/// Land the plan `id` the way a host does, which must land.
fn land(host: &mut Host, session: &mut Session, id: &Value) {
    host.land(session, id.as_str().unwrap())
        .unwrap_or_else(|reason| panic!("the plan lands: {reason}"));
}

fn read(host: &mut Host, session: &mut Session) {
    host.call(session, "read_model", json!({}));
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
fn an_edit_is_a_plan_that_changes_nothing_until_the_host_lands_it() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(&mut host, &mut session, backlog());
    assert_eq!(output["verdict"], "ready");
    assert_eq!(output["plan"], "P1");
    assert_eq!(output["simulates"], true);
    assert_eq!(
        output["changes"],
        json!([
            {"variable": "Backlog", "action": "added",
             "detail": "stock, initial value 0, filled by ordering, drained by fulfilling (widget)"},
            {"variable": "ordering", "action": "added", "detail": "flow = orders (widget/month)"},
            {"variable": "fulfilling", "action": "added",
             "detail": "flow = shipments (widget/month)"}
        ])
    );
    assert!(host.project.models[0].get_variable("backlog").is_none());
    assert_eq!(host.revision, 0);

    land(&mut host, &mut session, &output["plan"]);
    let model = &host.project.models[0];
    let Some(datamodel::Variable::Stock(stock)) = model.get_variable("backlog") else {
        panic!("the plan added the stock");
    };
    assert_eq!(stock.inflows, ["ordering"]);
    assert_eq!(stock.outflows, ["fulfilling"]);
    let datamodel::View::StockFlow(view) = &model.views[0];
    assert!(
        view.elements.iter().any(|e| matches!(
            e,
            datamodel::ViewElement::Stock(s) if s.name == "Backlog"
        )),
        "the plan places what it adds on the diagram"
    );
    let behavior = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["Backlog"]}),
    );
    assert_eq!(behavior["revision"], 1);
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
            {"variable": "Inventory", "action": "changed",
             "detail": "initial value desired_inventory * 2 (was desired_inventory)"},
            {"variable": "coverage", "action": "changed", "detail": "notes"}
        ])
    );
    land(&mut host, &mut session, &output["plan"]);
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
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([
            {"op": "set_equation", "variable": "coverage", "equation": "nothing_here * 2"}
        ])),
    );
    assert_eq!(output["verdict"], "refused");
    assert_eq!(output["simulates"], false);
    assert_eq!(
        output["diagnostics"][0],
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
    assert_eq!(output["verdict"], "ready", "{output}");
    assert_eq!(output["simulates"], false);
    assert!(
        output.get("diagnostics").is_none(),
        "the old error is not new"
    );
    // A second error in a model that did not simulate is still refused.
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "nowhere"}])),
    );
    assert_eq!(output["verdict"], "refused", "{output}");
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "shipments", "equation": "orders"}])),
    );
    assert_eq!(output["verdict"], "ready");
    assert_eq!(output["simulates"], true);
}

/// A warning an edit adds is listed. A unit warning in a model with one
/// already does not refuse it; a model's first, as a person's patch would
/// be, does.
#[test]
fn warnings_an_edit_adds_are_listed_and_a_models_first_unit_warning_refuses_it() {
    let mismatch = operations(json!([
        {"op": "set_equation", "variable": "shipments", "equation": "MIN(orders, Inventory)"}
    ]));
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(&mut host, &mut session, mismatch.clone());
    assert_eq!(output["verdict"], "refused", "{output}");
    assert!(
        output["note"].as_str().unwrap().contains("first"),
        "{output}"
    );
    let warnings = |output: &Value| -> usize {
        output["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["severity"] == "warning" && d["variable"] == "shipments")
            .count()
    };
    assert!(warnings(&output) > 0, "{output}");

    // With a unit warning already, another is listed and the edit plans.
    let mut project = inventory().build_datamodel();
    if let Some(Variable::Aux(aux)) = project.models[0].get_variable_mut("adjustment_time") {
        aux.units = Some("widget".to_string());
    }
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(&mut host, &mut session, mismatch);
    assert_eq!(output["verdict"], "ready", "{output}");
    assert!(warnings(&output) > 0, "{output}");
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
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "adjustment_time", "equation": "3"}])),
    );
    assert_eq!(output["verdict"], "ready");
    // Read again, and the edit plans.
    read(&mut host, &mut session);
    assert_eq!(edit(&mut host, &mut session, coverage)["verdict"], "ready");
}

#[test]
fn a_variable_the_sessions_own_plan_wrote_is_fresh_landed_or_not() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let set = |value: &str| {
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": value}]))
    };
    let first = edit(&mut host, &mut session, set("5"));
    land(&mut host, &mut session, &first["plan"]);
    // Landed: coverage is as the session's plan left it.
    let second = edit(&mut host, &mut session, set("6"));
    assert_eq!(second["verdict"], "ready");
    assert_eq!(
        second["changes"][0]["detail"], "equation 6 (was 5)",
        "planned against the model as it is"
    );
    // Not landed (the person declined): coverage is as the plan before left
    // it, and still fresh.
    let third = edit(&mut host, &mut session, set("7"));
    assert_eq!(third["verdict"], "ready");
}

#[test]
fn a_rename_rewrites_its_readers_and_the_plan_names_them() {
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
    land(&mut host, &mut session, &output["plan"]);
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
    land(&mut host, &mut session, &output["plan"]);
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
    assert!(refusal["error"].as_str().unwrap().contains("must increase"));
    let output = edit(
        &mut host,
        &mut session,
        operations(
            json!([{"op": "set_lookup", "variable": "effect_of_pressure",
                           "points": [[0, 0], [1, 0.8], [2, 1]], "kind": "extrapolate"}]),
        ),
    );
    assert_eq!(output["changes"][0]["detail"], "lookup");
    land(&mut host, &mut session, &output["plan"]);
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
    land(&mut host, &mut session, &output["plan"]);
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
    land(&mut host, &mut session, &output["plan"]);
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
fn an_arrayed_variables_element_takes_its_own_equation() {
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_aux("rate[region]", "0.1")
        .array_stock("pop[region]", "10", &["births"], &[], None)
        .array_flow("births[region]", "pop * rate", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(
            json!([{"op": "set_equation", "variable": "rate", "element": "South",
                           "equation": "0.2"}]),
        ),
    );
    assert_eq!(output["verdict"], "ready", "{output}");
    land(&mut host, &mut session, &output["plan"]);
    let Some(datamodel::Equation::Arrayed(_, elements, _, _)) = host.project.models[0]
        .get_variable("rate")
        .unwrap()
        .get_equation()
    else {
        panic!("per element now");
    };
    let equations: Vec<(&str, &str)> = elements
        .iter()
        .map(|(e, eq, _, _)| (e.as_str(), eq.as_str()))
        .collect();
    assert_eq!(equations, [("north", "0.1"), ("south", "0.2")]);
    let refusal = host.refuse(
        &mut session,
        "edit_model",
        operations(
            json!([{"op": "set_equation", "variable": "rate", "element": "east",
                           "equation": "0.3"}]),
        ),
    );
    assert_eq!(refusal["suggestions"], json!(["north", "south"]));
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

#[test]
fn a_session_keeps_its_last_plans() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    for i in 0..=MAX_PLANS {
        let output = edit(
            &mut host,
            &mut session,
            operations(
                json!([{"op": "set_notes", "variable": "coverage", "notes": format!("n{i}")}]),
            ),
        );
        assert_eq!(output["plan"], format!("P{}", i + 1));
    }
    assert!(session.plans.get("P1").is_none(), "the oldest is forgotten");
    assert!(session.plans.get(&format!("P{}", MAX_PLANS + 1)).is_some());
    assert!(session.plans.get("P99").is_none());
}

#[test]
fn an_edit_of_equations_edits_the_view_and_keeps_the_rest_of_it() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    // The first edit lays out the model, which has no diagram.
    let first = edit(&mut host, &mut session, backlog());
    let ops = &session.plans.get("P1").unwrap().patch().models[0].ops;
    assert!(matches!(
        ops.last(),
        Some(ModelOperation::UpsertView { .. })
    ));
    land(&mut host, &mut session, &first["plan"]);
    // What an MDL writer keeps in a view survives an edit of equations,
    // which adds a connector.
    host.edit(|p| {
        let datamodel::View::StockFlow(view) = &mut p.models[0].views[0];
        view.font = Some("Arial|12||0-0-0".to_string());
    });
    read(&mut host, &mut session);
    let second = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "ordering",
                           "equation": "orders + Backlog / coverage"}])),
    );
    let plan = session.plans.get(second["plan"].as_str().unwrap()).unwrap();
    let Some(ModelOperation::EditView { upsert, remove, .. }) = plan.patch().models[0].ops.last()
    else {
        panic!("the view is edited, not replaced");
    };
    assert!(remove.is_empty());
    assert!(
        upsert
            .iter()
            .any(|e| matches!(e, datamodel::ViewElement::Link(_))),
        "the connector the equation adds is drawn"
    );
    land(&mut host, &mut session, &second["plan"]);
    let datamodel::View::StockFlow(view) = &host.project.models[0].views[0];
    assert_eq!(view.font.as_deref(), Some("Arial|12||0-0-0"));
}

/// The change report is news for the agent: what its own plans left, once a
/// host landed them, is its own work, and what the person did is reported,
/// even to a variable the agent's plan wrote.
#[test]
fn the_change_report_leaves_out_what_the_agents_own_plans_left() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let planned = edit(&mut host, &mut session, backlog());
    land(&mut host, &mut session, &planned["plan"]);
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .is_none(),
        "only the agent's own edit landed"
    );

    // A plan that never landed explains nothing.
    let declined = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "adjustment_time", "equation": "5"}])),
    );
    assert_eq!(declined["verdict"], "ready");
    host.edit(|p| {
        let model = &mut p.models[0];
        model
            .get_variable_mut("adjustment_time")
            .unwrap()
            .set_scalar_equation("7");
        model
            .get_variable_mut("ordering")
            .unwrap()
            .set_scalar_equation("orders * 2");
    });
    let changes = session
        .changes_since_read(&host.project, host.revision)
        .expect("the person's edits are news");
    assert_eq!(
        changes.added,
        ["ordering"],
        "the person changed the flow the agent added"
    );
    assert_eq!(changes.changed.len(), 1, "{changes:?}");
    assert_eq!(changes.changed[0].name, "adjustment_time");
    assert!(
        changes.removed.is_empty() && !changes.specs_changed,
        "{changes:?}"
    );

    let outline = host.call(&mut session, "read_model", json!({}));
    assert_eq!(
        outline["changes"]["added"],
        json!(["ordering"]),
        "{outline}"
    );
    assert_eq!(
        outline["changes"]["changed"],
        json!([{"name": "adjustment_time", "fields": ["equation"]}])
    );
}

#[test]
fn a_landed_plan_explains_the_sim_specs_it_set() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let planned = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 40}])),
    );
    land(&mut host, &mut session, &planned["plan"]);
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .is_none()
    );

    host.edit(|p| p.sim_specs.dt = datamodel::Dt::Reciprocal(16.0));
    let changes = session
        .changes_since_read(&host.project, host.revision)
        .expect("the person's change of DT is news");
    assert!(changes.specs_changed);
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

/// A plan the host lands after the person moved a stock lands on the model
/// as it is: the move stays, and the plan's edit is made.
#[test]
fn a_plan_lands_on_the_diagram_as_the_person_left_it() {
    let mut host = drawn();
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "doubling_time",
                           "equation": "0.7 * coverage"}])),
    );
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
    land(&mut host, &mut session, &output["plan"]);
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
    let refusal = host
        .land(&mut session, output["plan"].as_str().unwrap())
        .unwrap_err();
    assert!(refusal.contains("landed already"), "{refusal}");
    assert!(host.land(&mut session, "P99").is_err());
}

/// A plan does not land over what the person changed since it was made, or
/// where the gate would now refuse it; nor when the model changed so that
/// the edit would do something other than what the person approved.
#[test]
fn a_plan_does_not_land_on_a_model_changed_under_it() {
    let set_coverage =
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "6"}]));
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(&mut host, &mut session, set_coverage);
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("coverage")
            .unwrap()
            .set_scalar_equation("5")
    });
    let refusal = host
        .land(&mut session, output["plan"].as_str().unwrap())
        .unwrap_err();
    assert!(
        refusal.contains("coverage changed since the plan was made"),
        "{refusal}"
    );

    // What the plan reads, the person deletes.
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "coverage",
                           "equation": "adjustment_time * 2"}])),
    );
    host.edit(|p| {
        p.models[0]
            .variables
            .retain(|v| v.get_ident() != "adjustment_time");
    });
    let refusal = host
        .land(&mut session, output["plan"].as_str().unwrap())
        .unwrap_err();
    assert!(refusal.contains("no longer passes the gate"), "{refusal}");
    assert!(!refusal.contains(".;"), "one sentence: {refusal}");

    // A rename lands only while it rewrites the readers it did when planned.
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "rename", "variable": "coverage", "to": "cover"}])),
    );
    host.edit(|p| {
        p.models[0].variables.push(
            TestProject::new("x")
                .aux("safety_stock", "coverage * 2", Some("month"))
                .build_datamodel()
                .models[0]
                .variables[0]
                .clone(),
        )
    });
    let refusal = host
        .land(&mut session, output["plan"].as_str().unwrap())
        .unwrap_err();
    assert!(refusal.contains("would now do something else"), "{refusal}");
    assert!(refusal.contains("safety_stock"), "{refusal}");
}

/// A plan made at one revision lands at a later one when the edits between
/// touched nothing it writes: the plan is made again on the model as it is.
#[test]
fn plans_made_together_land_one_after_another() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let first = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "6"}])),
    );
    let second = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "adjustment_time",
                           "equation": "3"}])),
    );
    let same = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "7"}])),
    );
    land(&mut host, &mut session, &first["plan"]);
    land(&mut host, &mut session, &second["plan"]);
    let model = &host.project.models[0];
    assert_eq!(
        model.get_variable("coverage").unwrap().get_equation(),
        Some(&datamodel::Equation::Scalar("6".to_string()))
    );
    assert_eq!(
        model
            .get_variable("adjustment_time")
            .unwrap()
            .get_equation(),
        Some(&datamodel::Equation::Scalar("3".to_string()))
    );
    // The third wrote coverage too, as it was before the first landed.
    let refusal = host
        .land(&mut session, same["plan"].as_str().unwrap())
        .unwrap_err();
    assert!(
        refusal.contains("coverage changed since the plan"),
        "{refusal}"
    );
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
    // The agent's own landed specs are its own to change again.
    read(&mut host, &mut session);
    let own = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 30}])),
    );
    land(&mut host, &mut session, &own["plan"]);
    let again = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 40}])),
    );
    assert_eq!(again["verdict"], "ready", "{again}");

    let loops = host.call(&mut session, "analyze_loops", json!({}));
    let id = loops["partitions"][0]["loops"][0]["id"].clone();
    read(&mut host, &mut session);
    let named = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "name_loop", "loop": id, "name": "restocking"}])),
    );
    let Some(ModelOperation::SetLoopName { variables, .. }) = session
        .plans
        .get(named["plan"].as_str().unwrap())
        .unwrap()
        .patch()
        .models[0]
        .ops
        .iter()
        .find(|op| matches!(op, ModelOperation::SetLoopName { .. }))
        .cloned()
    else {
        panic!("the plan names the loop");
    };
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
        let output = edit(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_equation", "variable": "coverage",
                               "equation": equation}])),
        );
        assert_eq!(output["verdict"], "refused", "{equation}: {output}");
        let non_finite: Vec<&Value> = output["diagnostics"]
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
            output["note"].as_str().unwrap().contains("not a number"),
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
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "5"}])),
    );
    assert_eq!(output["verdict"], "ready", "{output}");

    // An equation left unwritten, as an import stores one, is a warning,
    // not a unit one: listed.
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "placeholder", "equation": "NaN"}])),
    );
    assert!(
        output["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["severity"] == "warning" && d["code"] == "unfilled_equation"),
        "{output}"
    );
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
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "shipments", "equation": "orders"}])),
    );
    assert_eq!(output["verdict"], "ready", "{output}");
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
    assert_eq!(output["verdict"], "ready", "{output}");
    assert!(output.get("diagnostics").is_none(), "{output}");
}

/// Errors in a module's model are not the session's model's diagnostics:
/// they count through whether the model simulates, which refuses an edit
/// that stops a model that simulated.
#[test]
fn an_edit_that_stops_a_model_simulating_is_refused_whatever_its_diagnostics() {
    let mut host = Host::new(crate::test_common::two_instance_arrayed_submodel_project());
    let mut working = host.project.clone();
    working
        .get_model_mut("submodel")
        .unwrap()
        .get_variable_mut("w")
        .unwrap()
        .set_scalar_equation("nowhere");
    let mut session = Session::new("main");
    let model = host.project.models[0].clone();
    let base = session.runs.current(&mut host.workspace(), &model).unwrap();
    let Ok(gate) = gate(&mut host.workspace(), "main", &working, &[], Some(&base)) else {
        panic!("nothing waits for the project");
    };
    assert!(
        gate.new_errors.is_empty(),
        "the premise: none of main's own"
    );
    assert!(!gate.simulates);
    let refusal = gate.refusal(true).expect("refused");
    assert!(refusal.contains("would not simulate"), "{refusal}");
    assert_eq!(gate.refusal(false), None, "a model that did not simulate");
}

/// The stocks an edit rewires are what it writes: a stock the person changed
/// since the read refuses an edit that connects a flow to it or away from
/// it, or deletes a flow that filled it.
#[test]
fn the_stocks_an_edit_rewires_must_be_as_read() {
    for ops in [
        json!([{"op": "add_stock", "name": "Shipped", "initial": "0", "units": "widget"},
               {"op": "connect_flow", "flow": "shipments", "from": "Inventory", "to": "Shipped"}]),
        json!([{"op": "add_stock", "name": "Shipped", "initial": "0", "units": "widget"},
               {"op": "connect_flow", "flow": "shipments", "to": "Shipped"}]),
        json!([{"op": "delete", "variable": "production"}]),
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
        let refusal = host.refuse(&mut session, "edit_model", operations(ops.clone()));
        assert!(
            refusal["error"]
                .as_str()
                .unwrap()
                .contains("Inventory changed since you last read"),
            "{ops}: {refusal}"
        );
    }
}

/// A plan's lines keep to the answer's budget: a rename rewrites every
/// reader, each line's detail is cut, and the lines past the budget are
/// counted. The plan keeps them all, and lands.
#[test]
fn a_plans_lines_keep_to_the_budget() {
    let long = (0..40)
        .map(|i| format!("base * {i}"))
        .collect::<Vec<_>>()
        .join(" + ");
    let mut project = TestProject::new("wide")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "base", None)
        .aux("base", "1", None);
    for i in 0..60 {
        project = project.aux(&format!("reader_{i:02}"), &long, None);
    }
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "rename", "variable": "base", "to": "foundation"}])),
    );
    assert!(
        output.to_string().len() <= crate::tools::OUTLINE_BUDGET,
        "{output}"
    );
    let lines = output["changes"].as_array().unwrap();
    assert!(
        lines
            .iter()
            .all(|l| l["detail"].as_str().unwrap().chars().count() <= 240)
    );
    let omitted = output["omitted"].as_u64().unwrap() as usize;
    assert!(omitted > 0, "{output}");
    let plan = session.plans.get(output["plan"].as_str().unwrap()).unwrap();
    assert_eq!(plan.changes.len(), lines.len() + omitted);
    land(&mut host, &mut session, &output["plan"]);
    assert!(host.project.models[0].get_variable("foundation").is_some());
}

/// An agent's edit records what ISEE's AI information says of it: a
/// variable the AI made and it edits stays the AI's, one a person made is
/// now also edited by AI, and one it adds is made by AI. Provenance is no
/// change to whether the variable is as read: the agent's next edit of it
/// plans.
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
    let output = edit(&mut host, &mut session, operations(json!(ops)));
    land(&mut host, &mut session, &output["plan"]);
    assert_eq!(state(&host, "marketing_spend"), Some(AiState::C));
    assert_eq!(state(&host, "spend_ratio"), Some(AiState::C));
    if let Some(name) = &person_made {
        assert_eq!(state(&host, name), Some(AiState::H), "{name}");
    }
    let again = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_units", "variable": "marketing spend",
                           "units": "dollars/month"}])),
    );
    assert_eq!(again["verdict"], "ready", "{again}");
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
    let after_a_person = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_notes", "variable": "marketing spend", "notes": "y"}])),
    );
    assert_eq!(after_a_person["verdict"], "ready", "{after_a_person}");

    // A model that records no provenance gets none.
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_notes", "variable": "coverage", "notes": "x"}])),
    );
    land(&mut host, &mut session, &output["plan"]);
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
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "add_variable", "name": "reporting_delay", "equation": "3"}])),
    );
    land(&mut host, &mut session, &output["plan"]);
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
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "dt": 0.0001}])),
    );
    assert_eq!(output["verdict"], "refused", "{output}");
    let reason = output["note"].as_str().unwrap();
    assert!(
        reason.contains("asks more of a run than a run may") && reason.contains("a DT of at least"),
        "{reason}"
    );
    let output = edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_sim_specs", "stop": 30}])),
    );
    assert_eq!(output["verdict"], "ready", "{output}");
}

/// A plan stops before each stage of its gate -- the model's run it compares
/// with, the edit's diagnostics, the edit's run -- and between the slices of
/// the model's run, when other work waits for the project, keeps no plan,
/// and leaves the host's database as it found it: the staging guard restores
/// it as the plan stops.
#[test]
fn a_plan_stops_between_the_gates_stages_and_leaves_the_database_as_it_was() {
    let input = operations(json!([
        {"op": "set_equation", "variable": "coverage", "equation": "nothing_here * 2"}
    ]));
    let mut stopped = 0;
    for after in 1.. {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        read(&mut host, &mut session);
        let output = host.call_waiting(
            &mut session,
            "edit_model",
            input.clone(),
            &Host::waiting_after(after),
        );
        if !output.is_error {
            break;
        }
        assert!(output.interrupted, "{}", output.json);
        stopped += 1;
        let outline = host.call(&mut session, "read_model", json!({}));
        assert!(
            outline.get("diagnostics").is_none(),
            "the database is the model's, not the plan's: {outline}"
        );
        let planned = edit(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_equation", "variable": "coverage", "equation": "5"}])),
        );
        assert_eq!(
            planned["plan"], "P1",
            "the stopped plan kept no id: {planned}"
        );
    }
    // The edit does not simulate, so its own run is not sliced.
    let between_slices = crate::tools::runs::RUN_SLICES as usize - 1;
    assert_eq!(
        stopped,
        3 + between_slices,
        "three stages, and between the slices of the model's run"
    );
}
