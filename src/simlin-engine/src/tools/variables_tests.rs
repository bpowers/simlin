// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

fn read(project: datamodel::Project, names: &[&str]) -> Value {
    let mut host = Host::new(project);
    host.call(
        &mut Session::new("main"),
        "read_variables",
        json!({ "names": names }),
    )
}

fn hares_and_foxes() -> datamodel::Project {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx"
    );
    let file = std::fs::File::open(path).expect("the hares and foxes model is in the corpus");
    crate::xmile::project_from_reader(&mut std::io::BufReader::new(file)).unwrap()
}

/// One variable of each kind, and what its record must hold.
#[test]
fn every_kind_of_variable_is_read_whole() {
    for kind in VariableKind::ALL {
        let (project, name) = match kind {
            VariableKind::Stock => (inventory().build_datamodel(), "Inventory"),
            VariableKind::Flow => (inventory().build_datamodel(), "production"),
            VariableKind::Auxiliary => (inventory().build_datamodel(), "effect_of_pressure"),
            VariableKind::Constant => (inventory().build_datamodel(), "coverage"),
            VariableKind::Lookup => (inventory().build_datamodel(), "pressure_table"),
            VariableKind::Module => (hares_and_foxes(), "hares"),
        };
        let output = read(project, &[name]);
        let record = &output["variables"][0];
        assert_eq!(
            record["kind"],
            serde_json::to_value(kind).unwrap(),
            "{name}: {record}"
        );
        match kind {
            VariableKind::Stock => {
                assert_eq!(record["initial"], "desired_inventory");
                assert_eq!(record["documentation"], "Widgets on hand.");
                assert_eq!(record["inflows"], json!(["production"]));
                assert_eq!(record["outflows"], json!(["shipments"]));
                assert_eq!(record["nonNegative"], true);
                assert!(record.get("equation").is_none());
            }
            VariableKind::Flow => {
                assert_eq!(
                    record["equation"],
                    "MAX(0, orders + (desired_inventory - Inventory) / adjustment_time)"
                );
                assert_eq!(record["fills"], json!(["Inventory"]));
                assert!(record.get("drains").is_none());
            }
            VariableKind::Auxiliary => {
                assert_eq!(record["equation"], "Inventory / desired_inventory");
                assert_eq!(
                    record["lookup"],
                    json!({"kind": "continuous", "x": [0.0, 1.0, 2.0], "y": [0.0, 0.5, 1.0]})
                );
            }
            VariableKind::Constant => {
                assert_eq!(record["equation"], "4");
                assert_eq!(record["units"], "month");
            }
            VariableKind::Lookup => {
                assert!(
                    record.get("equation").is_none(),
                    "a table's placeholder equation is not a formula: {record}"
                );
                assert_eq!(record["lookup"]["y"], json!([0.0, 0.5, 1.0]));
            }
            VariableKind::Module => {
                assert_eq!(record["module"]["model"], "hares");
                assert!(
                    record["module"]["inputs"]
                        .as_array()
                        .unwrap()
                        .contains(&json!({"from": "·area", "to": "hares·area"}))
                );
            }
        }
    }
}

#[test]
fn inputs_and_readers_carry_each_links_polarity() {
    let output = read(inventory().build_datamodel(), &["Inventory", "production"]);
    let inventory = &output["variables"][0];
    assert_eq!(
        inventory["inputs"],
        json!([{"name": "production", "polarity": "+"}, {"name": "shipments", "polarity": "-"}])
    );
    assert!(
        inventory["readers"]
            .as_array()
            .unwrap()
            .contains(&json!({"name": "production", "polarity": "-"}))
    );
    let production = &output["variables"][1];
    assert!(
        production["inputs"]
            .as_array()
            .unwrap()
            .contains(&json!({"name": "adjustment_time", "polarity": "-"}))
    );
}

#[test]
fn a_links_through_a_smooth_are_the_links_between_the_variables_a_modeler_wrote() {
    let project = TestProject::new("p")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "(target - level) / 2", None)
        .aux("target", "10", None)
        .aux("perceived", "SMTH1(level, 3)", None)
        .build_datamodel();
    let output = read(project, &["perceived", "level"]);
    let perceived = &output["variables"][0];
    assert_eq!(
        perceived["inputs"],
        json!([{"name": "level", "polarity": "+"}])
    );
    let level = &output["variables"][1];
    assert!(
        level["readers"]
            .as_array()
            .unwrap()
            .contains(&json!({"name": "perceived", "polarity": "+"})),
        "{level}"
    );
    assert!(
        !output.to_string().contains('$'),
        "no internal node leaks: {output}"
    );
}

#[test]
fn a_flow_names_the_stocks_it_fills_and_drains() {
    let project = TestProject::new("p")
        .stock("here", "10", &[], &["transfer"], None)
        .stock("there", "0", &["transfer"], &[], None)
        .flow("transfer", "here * 0.1", None)
        .build_datamodel();
    let record = &read(project, &["transfer"])["variables"][0];
    assert_eq!(record["drains"], json!(["here"]));
    assert_eq!(record["fills"], json!(["there"]));
}

#[test]
fn per_element_equations_come_with_the_equation_of_every_other_element() {
    let project = TestProject::new("p")
        .named_dimension("region", &["north", "south", "east"])
        .array_with_default_and_overrides("price[region]", "10", vec![("south", "12")])
        .build_datamodel();
    let record = &read(project, &["price"])["variables"][0];
    assert_eq!(record["dimensions"], json!(["region"]));
    assert_eq!(
        record["elements"],
        json!([{"element": "south", "equation": "12"}])
    );
    assert_eq!(record["otherElements"], "10");
    assert!(record.get("equation").is_none());
}

#[test]
fn a_table_without_x_points_is_read_with_evenly_spaced_x() {
    let gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Extrapolate,
        x_points: None,
        y_points: vec![1.0, 2.0, 4.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 4.0 },
    };
    let project = TestProject::new("p")
        .aux_with_gf("table", "", gf)
        .build_datamodel();
    let record = &read(project, &["table"])["variables"][0];
    assert_eq!(
        record["lookup"],
        json!({"kind": "extrapolate", "x": [0.0, 5.0, 10.0], "y": [1.0, 2.0, 4.0]})
    );
}

#[test]
fn a_name_that_matches_nothing_comes_back_with_suggestions_and_a_repeat_is_read_once() {
    let output = read(
        inventory().build_datamodel(),
        &["Inventory", "inventory", "invntory", "zzzz"],
    );
    assert_eq!(output["variables"].as_array().unwrap().len(), 1);
    assert_eq!(
        output["notFound"],
        json!([
            {"name": "invntory", "suggestions": ["Inventory", "desired_inventory"]},
            {"name": "zzzz"}
        ])
    );
}

#[test]
fn a_call_that_names_too_many_or_no_variables_is_refused() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let too_many: Vec<String> = (0..=MAX_READ_VARIABLES).map(|i| format!("v{i}")).collect();
    let refusal = host.refuse(&mut session, "read_variables", json!({ "names": too_many }));
    assert!(
        refusal["error"].as_str().unwrap().contains("at most 12"),
        "{refusal}"
    );
    let refusal = host.refuse(&mut session, "read_variables", json!({ "names": [] }));
    assert!(refusal["error"].as_str().unwrap().contains("at least one"));
}

#[test]
fn blank_documentation_and_units_are_left_out() {
    let project = TestProject::new("p")
        .aux("x", "1", Some(""))
        .build_datamodel();
    let record = &read(project, &["x"])["variables"][0];
    assert!(record.get("units").is_none() && record.get("documentation").is_none());
}

#[test]
fn a_record_carries_the_same_diagnostic_ids_the_outline_does() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let outline = host.call(&mut session, "read_model", json!({}));
    let record = host.call(
        &mut session,
        "read_variables",
        json!({"names": ["shipments"]}),
    );
    // The record carries each diagnostic whole, under the outline's id.
    let ids: Vec<Value> = record["variables"][0]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["id"].clone())
        .collect();
    assert_eq!(Value::from(ids), outline["flows"][1]["diagnostics"]);
    let first = &record["variables"][0]["diagnostics"][0];
    let listed = outline["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == first["id"])
        .unwrap();
    assert_eq!(first["code"], listed["code"]);
    assert_eq!(first["reason"], listed["reason"]);
    assert!(
        first.get("variable").is_none(),
        "the record names its variable already"
    );
}

fn regions(count: usize) -> TestProject {
    let elements: Vec<String> = (1..=count).map(|i| format!("r{i}")).collect();
    let refs: Vec<&str> = elements.iter().map(String::as_str).collect();
    TestProject::new("regions")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("region", &refs)
}

#[test]
fn an_element_is_read_by_naming_it() {
    let overrides: Vec<(&str, &str)> = vec![("r2", "12"), ("r30", "30")];
    let project = regions(30)
        .array_with_default_and_overrides("price[region]", "10", overrides)
        .array_aux("level[region]", "price * 2")
        .build_datamodel();
    let output = read(
        project.clone(),
        &["price[R2]", "price[r3]", "price[r30]", "level[r5]"],
    );
    let records = output["variables"].as_array().unwrap();
    let equation_of = |i: usize| {
        (
            records[i]["element"].clone(),
            records[i]["equation"].clone(),
        )
    };
    assert_eq!(equation_of(0), (json!("r2"), json!("12")));
    assert_eq!(equation_of(1), (json!("r3"), json!("10")), "the default");
    assert_eq!(equation_of(2), (json!("r30"), json!("30")));
    assert_eq!(equation_of(3), (json!("r5"), json!("price * 2")));
    assert!(records[0].get("elements").is_none());

    let output = read(project, &["price[r31]", "price[r1, r2]", "level[x]"]);
    let not_found = output["notFound"].as_array().unwrap();
    assert_eq!(not_found.len(), 3, "{output}");
    let reason = not_found[0]["suggestions"][0].as_str().unwrap();
    assert!(
        reason.contains("`r31` is not an element of region"),
        "{reason}"
    );
    assert!(
        not_found[1]["suggestions"][0]
            .as_str()
            .unwrap()
            .contains("1 subscript"),
        "{output}"
    );
}

#[test]
fn a_record_lists_at_most_24_elements_and_links_and_says_how_many_more() {
    let elements: Vec<String> = (1..=30).map(|i| format!("r{i}")).collect();
    let overrides: Vec<(&str, &str)> = elements.iter().map(|e| (e.as_str(), "1")).collect();
    let mut project =
        regions(30).array_with_default_and_overrides("price[region]", "10", overrides);
    for i in 0..30 {
        project = project.aux(&format!("reader_{i:02}"), "SUM(price[*])", None);
    }
    let long = "x".repeat(2 * MAX_DOCUMENTATION_CHARS);
    let mut project = project.build_datamodel();
    project.models[0]
        .get_variable_mut("price")
        .unwrap()
        .set_documentation(&long);
    let record = &read(project, &["price"])["variables"][0];
    assert_eq!(
        record["elements"].as_array().unwrap().len(),
        MAX_RECORD_ELEMENTS
    );
    assert_eq!(record["moreElements"], 30 - MAX_RECORD_ELEMENTS);
    assert_eq!(
        record["readers"].as_array().unwrap().len(),
        MAX_RECORD_LINKS
    );
    assert_eq!(record["moreReaders"], 30 - MAX_RECORD_LINKS);
    let documentation = record["documentation"].as_str().unwrap();
    assert_eq!(
        documentation.chars().count(),
        MAX_DOCUMENTATION_CHARS + 1,
        "cut, with …"
    );
    assert!(documentation.ends_with('…'));
}

#[test]
fn a_lookup_lists_at_most_64_points() {
    let n = MAX_LOOKUP_POINTS + 6;
    let gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some((0..n).map(|i| i as f64).collect()),
        y_points: (0..n).map(|i| i as f64).collect(),
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: n as f64,
        },
        y_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: n as f64,
        },
    };
    let project = TestProject::new("p")
        .aux_with_gf("table", "", gf)
        .build_datamodel();
    let lookup = &read(project, &["table"])["variables"][0]["lookup"];
    assert_eq!(lookup["x"].as_array().unwrap().len(), MAX_LOOKUP_POINTS);
    assert_eq!(lookup["y"].as_array().unwrap().len(), MAX_LOOKUP_POINTS);
    assert_eq!(lookup["morePoints"], 6);
}

/// Records that do not fit the budget are named for another call, in order;
/// the first always fits, since its own caps bound it.
#[test]
fn an_answer_over_its_budget_names_the_records_it_left_out() {
    let names = ["Inventory", "production", "shipments", "orders", "coverage"];
    let mut host = Host::from_test_project(&inventory());
    let mut whole = Session::new("main");
    let all = host.call(&mut whole, "read_variables", json!({ "names": names }));
    let whole_len = all.to_string().len();
    assert!(all.get("omitted").is_none());

    for budget in [1, whole_len / 2, whole_len - 1] {
        let mut session = Session::new("main");
        session.outline_budget = budget;
        let output = host.call(&mut session, "read_variables", json!({ "names": names }));
        let listed: Vec<String> = output["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect();
        let omitted: Vec<String> = serde_json::from_value(output["omitted"].clone()).unwrap();
        assert!(!listed.is_empty(), "budget {budget}");
        assert_eq!(
            listed.iter().chain(&omitted).cloned().collect::<Vec<_>>(),
            names,
            "budget {budget}: in order, nothing lost"
        );
        if listed.len() > 1 {
            assert!(output.to_string().len() <= budget, "budget {budget}");
        }
    }
}

#[test]
fn matches_are_closest_first_capped_and_scored_to_two_places() {
    let mut project = TestProject::new("p");
    for i in 0..(MAX_MATCHES + 5) {
        project = project.aux(&format!("inventory_{i:02}"), "1", None);
    }
    let mut host = Host::from_test_project(&project);
    let output = host.call(
        &mut Session::new("main"),
        "find_variables",
        json!({"phrase": "inventory"}),
    );
    let matches = output["matches"].as_array().unwrap();
    assert_eq!(matches.len(), MAX_MATCHES);
    let scores: Vec<f64> = matches
        .iter()
        .map(|m| m["score"].as_f64().unwrap())
        .collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{scores:?}");
    assert!(scores.iter().all(|s| (s * 100.0).fract() == 0.0));
    assert_eq!(matches[0]["kind"], "constant");
}

#[test]
fn a_phrase_like_nothing_finds_nothing_and_an_empty_phrase_is_refused() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let output = host.call(&mut session, "find_variables", json!({"phrase": "xqzv"}));
    assert_eq!(output["matches"], json!([]));
    let refusal = host.refuse(&mut session, "find_variables", json!({"phrase": "  "}));
    assert!(refusal["error"].as_str().unwrap().contains("phrase"));
}

/// A large arrayed variable's record summarizes its elements: long
/// equations are cut, and the list stops at 2,400 characters of them; an
/// element named reads its own whole.
#[test]
fn long_per_element_equations_are_cut_and_the_list_bounded() {
    let long = format!("1 + {}", "x * 0 + ".repeat(60))
        .trim_end_matches(" + ")
        .to_string();
    let elements: Vec<String> = (1..=20).map(|i| format!("r{i}")).collect();
    let overrides: Vec<(&str, &str)> = elements
        .iter()
        .map(|e| (e.as_str(), long.as_str()))
        .collect();
    let project = regions(20)
        .aux("x", "1", None)
        .array_with_default_and_overrides("price[region]", "10", overrides)
        .build_datamodel();
    let output = read(project, &["price", "price[r20]"]);
    let summary = &output["variables"][0];
    let listed = summary["elements"].as_array().unwrap();
    assert!(listed.len() < 20);
    assert_eq!(summary["moreElements"], 20 - listed.len());
    let chars: usize = listed
        .iter()
        .map(|e| e["equation"].as_str().unwrap().chars().count())
        .sum();
    assert!(chars <= MAX_ELEMENT_TEXT_CHARS, "{chars}");
    assert!(listed[0]["equation"].as_str().unwrap().ends_with('…'));
    assert_eq!(
        output["variables"][1]["equation"],
        long.as_str(),
        "read whole"
    );
}
