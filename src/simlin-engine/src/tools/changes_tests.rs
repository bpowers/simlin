// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::json;

use super::*;
use crate::datamodel::{Compat, Equation, GraphicalFunction, GraphicalFunctionKind};
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

fn aux(name: &str, equation: &str) -> Variable {
    TestProject::new("p")
        .aux(name, equation, Some("widget"))
        .build_datamodel()
        .models
        .remove(0)
        .variables
        .remove(0)
}

fn stock(name: &str) -> Variable {
    TestProject::new("p")
        .stock(name, "10", &["inflow"], &["outflow"], Some("widget"))
        .build_datamodel()
        .models
        .remove(0)
        .variables
        .remove(0)
}

fn module(model_name: &str) -> Variable {
    Variable::Module(datamodel::Module {
        ident: "sub".to_string(),
        model_name: model_name.to_string(),
        documentation: String::new(),
        units: None,
        references: vec![],
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn table() -> GraphicalFunction {
    GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: None,
        y_points: vec![0.0, 1.0],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    }
}

/// A variable before and after an edit that changes exactly `field`.
fn edited(field: ChangedField) -> (Variable, Variable) {
    match field {
        ChangedField::Kind => (aux("x", "1"), stock("x")),
        ChangedField::Equation => (aux("x", "1"), aux("x", "2")),
        ChangedField::Units => {
            let before = aux("x", "1");
            let mut after = before.clone();
            after.set_units("gadget");
            (before, after)
        }
        ChangedField::Documentation => {
            let before = aux("x", "1");
            let mut after = before.clone();
            after.set_documentation("what x is");
            (before, after)
        }
        ChangedField::Lookup => {
            let before = aux("x", "1");
            let mut after = before.clone();
            after.set_graphical_function(Some(table()));
            (before, after)
        }
        ChangedField::Inflows | ChangedField::Outflows => {
            let before = stock("s");
            let mut after = before.clone();
            if let Variable::Stock(s) = &mut after {
                let list = if field == ChangedField::Inflows {
                    &mut s.inflows
                } else {
                    &mut s.outflows
                };
                list.push("another".to_string());
            }
            (before, after)
        }
        ChangedField::NonNegative => {
            let before = stock("s");
            let mut after = before.clone();
            if let Variable::Stock(s) = &mut after {
                s.compat.non_negative = true;
            }
            (before, after)
        }
        ChangedField::Module => (module("one"), module("two")),
        ChangedField::Name => (aux("rate", "1"), aux("Rate", "1")),
        ChangedField::Other => {
            let before = aux("x", "1");
            let mut after = before.clone();
            if let Variable::Aux(a) = &mut after {
                a.compat.active_initial = Some("0".to_string());
            }
            (before, after)
        }
    }
}

#[test]
fn every_field_a_change_report_names_is_reported_alone_when_it_alone_changes() {
    for field in ChangedField::ALL {
        let (before, after) = edited(field);
        assert_eq!(changed_fields(&before, &after), vec![field], "{field:?}");
        assert!(
            changed_fields(&before, &before).is_empty(),
            "{field:?}: a record equals itself"
        );
    }
}

#[test]
fn a_stocks_initial_value_is_its_equation_and_several_fields_come_in_field_order() {
    let before = stock("s");
    let mut after = before.clone();
    if let Variable::Stock(s) = &mut after {
        s.equation = Equation::Scalar("20".to_string());
        s.outflows.clear();
        s.units = Some("gadget".to_string());
    }
    assert_eq!(
        changed_fields(&before, &after),
        vec![
            ChangedField::Equation,
            ChangedField::Units,
            ChangedField::Outflows
        ]
    );
}

#[test]
fn a_flow_listed_twice_is_the_same_list_as_the_flow_listed_once() {
    let before = stock("s");
    let mut after = before.clone();
    if let Variable::Stock(s) = &mut after {
        s.inflows = vec!["inflow".to_string(), "Inflow".to_string()];
    }
    assert!(changed_fields(&before, &after).is_empty());
}

#[test]
fn uids_and_provenance_are_not_changes() {
    let before = aux("x", "1");
    let mut after = before.clone();
    if let Variable::Aux(a) = &mut after {
        a.uid = Some(42);
        a.ai_state = Some(datamodel::AiState::C);
    }
    assert!(changed_fields(&before, &after).is_empty());
}

#[test]
fn the_diff_names_added_removed_and_changed_variables_as_the_model_spells_them() {
    let project = inventory().build_datamodel();
    let snapshot = ReadSnapshot::new(3, &project, &project.models[0]);
    let mut edited = project.clone();
    let model = &mut edited.models[0];
    model.variables.retain(|v| v.get_ident() != "shipments");
    model
        .get_variable_mut("coverage")
        .unwrap()
        .set_scalar_equation("6");
    model.variables.push(aux("Backlog Level", "0"));

    let changes = diff(&snapshot, &edited, &edited.models[0]);
    assert_eq!(changes.since_revision, 3);
    assert_eq!(changes.added, ["Backlog Level"]);
    assert_eq!(changes.removed, ["shipments"]);
    assert_eq!(
        changes.changed,
        [ChangedVariable {
            name: "coverage".to_string(),
            fields: vec![ChangedField::Equation],
        }]
    );
    assert!(!changes.specs_changed && !changes.is_empty());
}

#[test]
fn a_view_edit_changes_nothing_an_agent_read_and_a_specs_edit_does() {
    let project = inventory().build_datamodel();
    let snapshot = ReadSnapshot::new(0, &project, &project.models[0]);

    let mut moved = project.clone();
    moved.models[0]
        .views
        .push(datamodel::View::StockFlow(datamodel::StockFlow {
            name: None,
            elements: vec![].into(),
            view_box: datamodel::Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        }));
    assert!(diff(&snapshot, &moved, &moved.models[0]).is_empty());

    let mut respecified = project.clone();
    respecified.sim_specs.stop = 40.0;
    let changes = diff(&snapshot, &respecified, &respecified.models[0]);
    assert!(changes.specs_changed && changes.added.is_empty() && changes.changed.is_empty());

    // A model's own specs override the project's, so they are what count.
    let mut own = project.clone();
    let mut specs = own.sim_specs.clone();
    specs.dt = datamodel::Dt::Reciprocal(8.0);
    own.models[0].sim_specs = Some(specs);
    assert!(diff(&snapshot, &own, &own.models[0]).specs_changed);
}

#[test]
fn a_change_list_is_capped_and_counted() {
    let project = inventory().build_datamodel();
    let snapshot = ReadSnapshot::new(0, &project, &project.models[0]);
    let mut grown = project.clone();
    for i in 0..(MAX_CHANGED_NAMES + 5) {
        grown.models[0]
            .variables
            .push(aux(&format!("added_{i:02}"), "1"));
    }
    let changes = diff(&snapshot, &grown, &grown.models[0]);
    assert_eq!(changes.added.len(), MAX_CHANGED_NAMES);
    assert_eq!(changes.added_count, Some(MAX_CHANGED_NAMES + 5));
    assert_eq!(changes.removed_count, None);
}

#[test]
fn a_session_reports_the_changes_since_its_last_read_once_the_revision_moves() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none(),
        "nothing was read yet"
    );
    host.call(&mut session, "read_model", json!({}));
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none()
    );

    // A layout-only edit advances the revision and changes nothing read.
    host.edit(|p| p.models[0].views.clear());
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none()
    );

    host.edit(|p| {
        p.models[0]
            .get_variable_mut("adjustment_time")
            .unwrap()
            .set_scalar_equation("3")
    });
    let changes = session
        .changes_since_read(&host.project, host.revision)
        .unwrap()
        .expect("an equation edit is a change");
    assert_eq!(changes.since_revision, 0);
    assert_eq!(changes.changed[0].name, "adjustment_time");

    // The next read reports them and becomes the new baseline.
    let outline = host.call(&mut session, "read_model", json!({}));
    assert_eq!(
        outline["changes"]["changed"],
        json!([{"name": "adjustment_time", "fields": ["equation"]}])
    );
    assert!(
        session
            .changes_since_read(&host.project, host.revision)
            .unwrap()
            .is_none()
    );
    let outline = host.call(&mut session, "read_model", json!({}));
    assert!(outline.get("changes").is_none(), "{outline}");

    // A project that no longer has the session's model is refused, as a tool
    // refuses it, rather than reported as unchanged.
    // ("main" names the first model when none is called that, so the
    // project has none.)
    host.edit(|p| p.models.clear());
    let refused = session
        .changes_since_read(&host.project, host.revision)
        .expect_err("the session's model is gone");
    assert_eq!(refused, "the project has no model named 'main'");
}

/// What a model's variables rest on changes too: its dimensions, the
/// project's unit definitions, and the models a module instantiates.
#[test]
fn the_diff_names_what_the_models_variables_rest_on() {
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_aux("rate[region]", "0.1")
        .aux("x", "1", None)
        .build_datamodel();
    let mut project = project;
    let mut sub = project.models[0].clone();
    sub.name = "sub".to_string();
    sub.variables.retain(|v| v.get_ident() == "x");
    project.models.push(sub);
    let snapshot = ReadSnapshot::new(0, &project, &project.models[0]);
    assert!(diff(&snapshot, &project, &project.models[0]).is_empty());

    let mut dims = project.clone();
    dims.dimensions[0] = datamodel::Dimension::named(
        "region".to_string(),
        vec!["north".to_string(), "south".to_string(), "east".to_string()],
    );
    let changes = diff(&snapshot, &dims, &dims.models[0]);
    assert!(changes.dimensions_changed, "an element added");
    assert!(!changes.is_empty());

    let mut units = project.clone();
    units.units.push(datamodel::Unit {
        name: "widget".to_string(),
        equation: None,
        disabled: false,
        aliases: vec![],
    });
    assert!(diff(&snapshot, &units, &units.models[0]).unit_definitions_changed);

    let mut other = project.clone();
    other.models[1]
        .get_variable_mut("x")
        .unwrap()
        .set_scalar_equation("2");
    let changes = diff(&snapshot, &other, &other.models[0]);
    assert_eq!(changes.models_changed, ["sub"]);

    // The other model's diagram is no change.
    let mut moved = project.clone();
    moved.models[1].views.clear();
    assert!(diff(&snapshot, &moved, &moved.models[0]).is_empty());

    let mut gone = project.clone();
    gone.models.pop();
    assert_eq!(
        diff(&snapshot, &gone, &gone.models[0]).models_changed,
        ["sub"]
    );
}

#[test]
fn a_change_of_case_is_a_change_of_name() {
    let project = inventory().build_datamodel();
    let snapshot = ReadSnapshot::new(0, &project, &project.models[0]);
    let mut edited = project.clone();
    if let Variable::Aux(aux) = edited.models[0].get_variable_mut("coverage").unwrap() {
        aux.ident = "Coverage".to_string();
    }
    let changes = diff(&snapshot, &edited, &edited.models[0]);
    assert!(
        changes.added.is_empty() && changes.removed.is_empty(),
        "{changes:?}"
    );
    assert_eq!(changes.changed.len(), 1);
    assert_eq!(changes.changed[0].name, "Coverage");
    assert_eq!(changes.changed[0].fields, [ChangedField::Name]);
}

/// What a session's own edit leaves of another model is held as the read
/// would have given it: no provenance (which every edit marks, and a read
/// does not compare) and no variable the edit removed. The model the edit
/// left is then no change since the read.
#[test]
fn another_models_variable_as_the_edit_left_it_is_no_change() {
    let mut project = TestProject::new("main")
        .aux("x", "1", None)
        .build_datamodel();
    let mut sub = project.models[0].clone();
    sub.name = "sub".to_string();
    sub.variables.push(aux("y", "2"));
    project.models.push(sub);
    let main = project.models[0].clone();

    // The edit marks who made `x` in `sub`.
    let mut marked = project.clone();
    let made_by_agent = |var: &mut Variable| match var {
        Variable::Aux(aux) => aux.ai_state = Some(datamodel::AiState::A),
        _ => unreachable!("an auxiliary"),
    };
    made_by_agent(marked.models[1].get_variable_mut("x").unwrap());
    let mut snapshot = ReadSnapshot::new(0, &project, &main);
    snapshot.absorb_variable(Some("sub"), "x", marked.models[1].get_variable("x"));
    assert!(diff(&snapshot, &marked, &marked.models[0]).is_empty());

    // The edit removes `y` from `sub`.
    let mut removed = project.clone();
    removed.models[1].variables.retain(|v| v.get_ident() != "y");
    let mut snapshot = ReadSnapshot::new(0, &project, &main);
    snapshot.absorb_variable(Some("sub"), "y", None);
    assert!(diff(&snapshot, &removed, &removed.models[0]).is_empty());
    // Absorbed or not, a change the edit did not make is still one.
    removed.models[1]
        .get_variable_mut("x")
        .unwrap()
        .set_scalar_equation("3");
    assert_eq!(
        diff(&snapshot, &removed, &removed.models[0]).models_changed,
        ["sub"]
    );
}

/// The revision is what a read is of: at the revision it was made at, a
/// session has nothing to report, whatever project a host passes with it.
#[test]
fn at_the_revision_of_the_read_nothing_changed_since_it() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    host.call(&mut session, "read_model", json!({}));
    let mut other = host.project.clone();
    other.models[0]
        .get_variable_mut("adjustment_time")
        .unwrap()
        .set_scalar_equation("3");
    assert!(
        session
            .changes_since_read(&other, host.revision)
            .unwrap()
            .is_none()
    );
    assert!(
        session
            .changes_since_read(&other, host.revision + 1)
            .unwrap()
            .is_some()
    );
}
