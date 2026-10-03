// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, initial_reads, inventory};

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
        json!([
            {"name": "desired_inventory", "polarity": "?", "startOnly": true},
            {"name": "production", "polarity": "+"},
            {"name": "shipments", "polarity": "-"}
        ]),
        "a stock reads its flows, and its initial value only at the start"
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

/// A constant the model reads only as it starts -- in a stock's initial value,
/// or inside `INIT` -- has a reader, and the reader an input, each marked.
#[test]
fn a_read_made_only_at_the_start_is_an_input_and_a_reader_marked_so() {
    let project = initial_reads().build_datamodel();
    let output = read(project, &["base_rate", "rate", "s0", "level"]);
    let record = |name: &str| {
        output["variables"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .unwrap_or_else(|| panic!("{name} is read: {output}"))
    };
    let start = |name: &str| json!([{"name": name, "polarity": "?", "startOnly": true}]);
    assert_eq!(record("rate")["inputs"], start("base_rate"));
    assert_eq!(record("base_rate")["readers"], start("rate"));
    assert_eq!(record("s0")["readers"], start("level"));
    assert_eq!(
        record("rate")["readers"],
        json!([{"name": "growth", "polarity": "+"}]),
        "a read made every step is not marked"
    );
}

/// A name in an answer is spelled one way, the model's: a stock's flows as
/// the flows are named, whatever spelling the stock's own lists hold (an
/// import writes them canonically).
#[test]
fn a_stocks_flows_are_spelled_as_the_flows_are_named() {
    let project = TestProject::new("teacup")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock(
            "Teacup Temperature",
            "180",
            &[],
            &["heat_loss_to_room"],
            None,
        )
        .flow("Heat Loss to Room", "Teacup_Temperature / 10", None)
        .build_datamodel();
    let output = read(project, &["teacup temperature", "heat loss to room"]);
    let stock = &output["variables"][0];
    assert_eq!(stock["outflows"], json!(["Heat Loss to Room"]));
    assert_eq!(stock["inputs"][0]["name"], "Heat Loss to Room");
    assert_eq!(
        output["variables"][1]["drains"],
        json!(["Teacup Temperature"])
    );
}

/// A constant is its value throughout a run, so its record says nothing of
/// its behavior; a variable that happens to hold still says so.
#[test]
fn a_constant_has_no_behavior_to_report() {
    let project = inventory()
        .aux("held", "coverage * 1", None)
        .build_datamodel();
    let output = read(project, &["coverage", "held"]);
    let (constant, held) = (&output["variables"][0], &output["variables"][1]);
    assert_eq!(constant["kind"], "constant");
    assert!(constant.get("behavior").is_none(), "{constant}");
    assert_eq!(held["behavior"]["mode"]["kind"], "at_rest", "{held}");
}

/// One row per thing an importer leaves in documentation that is not prose.
#[test]
fn documentation_is_read_as_prose() {
    for (written, read_as) in [
        ("Plain words.", "Plain words."),
        (
            "A Vensim comment wrapped in \\\n\t\tits file, twice \\\r\n\t\tover.",
            "A Vensim comment wrapped in its file, twice over.",
        ),
        (
            "A line broken\n\t\t         in the middle  of a   sentence.",
            "A line broken in the middle of a sentence.",
        ),
        ("One paragraph.\n\n  Another.", "One paragraph.\nAnother."),
        ("  \n\t ", ""),
    ] {
        assert_eq!(tidy(written), read_as, "{written:?}");
    }
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("coverage")
        .unwrap()
        .set_documentation("Months of orders \\\n\t\theld as stock.");
    let output = read(project, &["coverage"]);
    assert_eq!(
        output["variables"][0]["documentation"],
        "Months of orders held as stock."
    );
}

/// A table is read by the variable that looks it up, so "what uses this
/// table" has an answer.
#[test]
fn a_table_names_the_variables_that_look_it_up() {
    let project = inventory()
        .aux(
            "pressure_effect",
            "LOOKUP(pressure_table, Inventory / 40)",
            None,
        )
        .build_datamodel();
    let output = read(project, &["pressure_table", "pressure_effect"]);
    assert_eq!(
        output["variables"][0]["readers"],
        json!([{"name": "pressure_effect", "polarity": "?"}])
    );
    assert_eq!(
        output["variables"][1]["inputs"],
        json!([
            {"name": "Inventory", "polarity": "+"},
            {"name": "pressure_table", "polarity": "?"}
        ])
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
    let reason = not_found[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("`r31` is not an element of region"),
        "{reason}"
    );
    for unread in not_found {
        assert!(
            unread.get("suggestions").is_none(),
            "suggestions are names of variables, never a sentence: {unread}"
        );
    }
    assert!(
        not_found[1]["reason"]
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

/// Records that do not fit the budget are named for another call, in order,
/// and the answer keeps to the budget.
#[test]
fn an_answer_over_its_budget_names_the_records_it_left_out() {
    let names = ["Inventory", "production", "shipments", "orders", "coverage"];
    let mut host = Host::from_test_project(&inventory());
    let mut whole = Session::new("main");
    let all = host.call(&mut whole, "read_variables", json!({ "names": names }));
    let whole_len = all.to_string().len();
    assert!(all.get("omitted").is_none());
    let first_len = host
        .call(&mut whole, "read_variables", json!({ "names": names[..1] }))
        .to_string()
        .len();

    for budget in [first_len + 60, whole_len / 2, whole_len - 1] {
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
        assert!(!listed.is_empty() && !omitted.is_empty(), "budget {budget}");
        assert_eq!(
            listed.iter().chain(&omitted).cloned().collect::<Vec<_>>(),
            names,
            "budget {budget}: in order, nothing lost"
        );
        assert!(output.to_string().len() <= budget, "budget {budget}");
        assert!(
            output["variables"][0].get("moreReaders").is_none(),
            "budget {budget}: whole records are left out before any record is cut"
        );
    }
}

/// A variable with readers, inputs, a unit warning, documentation and a long
/// equation.
fn hub() -> datamodel::Project {
    let inputs: Vec<String> = (0..12)
        .map(|i| format!("an_input_of_the_hub_{i}"))
        .collect();
    // The sum, written out enough times to be a long equation.
    let equation = vec![inputs.join(" + "); 4].join(" + ");
    let mut project = TestProject::new("hub").aux("hub", &equation, None);
    for input in &inputs {
        project = project.aux(input, "1", Some("month"));
    }
    for i in 0..12 {
        project = project.aux(&format!("a_reader_of_the_hub_{i}"), "hub * 2", None);
    }
    let mut project = project.build_datamodel();
    let hub = project.models[0].get_variable_mut("hub").unwrap();
    hub.set_units("widget");
    hub.set_documentation(&"What the hub is for, at some length. ".repeat(8));
    project
}

/// A variable whose value is looked up in a table of twenty points.
fn table_hub() -> datamodel::Project {
    let gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some((0..20).map(f64::from).collect()),
        y_points: (0..20).map(f64::from).collect(),
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 19.0,
        },
        y_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 19.0,
        },
    };
    TestProject::new("table_hub")
        .aux_with_gf("hub", "TIME", gf)
        .build_datamodel()
}

/// A variable with per-element equations and a long equation for the rest.
fn arrayed_hub() -> datamodel::Project {
    let elements: Vec<String> = (0..12).map(|i| format!("e{i}")).collect();
    let names: Vec<&str> = elements.iter().map(String::as_str).collect();
    let mut project = TestProject::new("arrayed_hub")
        .named_dimension("letters", &names)
        .aux("an_input", "3", None)
        .aux("reader", "SUM(hub[*])", None)
        .build_datamodel();
    let per_element = elements[..8]
        .iter()
        .map(|e| (e.clone(), format!("an_input * {}", e.len()), None, None))
        .collect();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "hub".to_string(),
            equation: datamodel::Equation::Arrayed(
                vec!["letters".to_string()],
                per_element,
                Some(vec!["an_input"; 80].join(" + ")),
                true,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    project
}

/// How much of what a cut names a record still carries: a row per cut, so a
/// cut added to the ladder does not compile here until it is measured.
fn carried(cut: Cut, record: &Value) -> usize {
    let len = |key: &str| record[key].as_array().map_or(0, Vec::len);
    let chars = |key: &str| record[key].as_str().map_or(0, |text| text.chars().count());
    match cut {
        Cut::Readers => len("readers"),
        Cut::Inputs => len("inputs"),
        Cut::Diagnostics => len("diagnostics"),
        Cut::Elements => len("elements"),
        Cut::LookupPoints => record["lookup"]["x"].as_array().map_or(0, Vec::len),
        Cut::Documentation => usize::from(record.get("documentation").is_some()),
        Cut::Text => chars("equation") + chars("initial") + chars("otherElements"),
    }
}

/// Where a record counts what a cut left out, for the cuts that count.
fn counted(cut: Cut) -> Option<&'static str> {
    match cut {
        Cut::Readers => Some("moreReaders"),
        Cut::Inputs => Some("moreInputs"),
        Cut::Diagnostics => Some("moreDiagnostics"),
        Cut::Elements => Some("moreElements"),
        Cut::LookupPoints => None,
        Cut::Documentation | Cut::Text => None,
    }
}

/// Who reads a variable goes before what it reads: what it reads is how it
/// is computed, and its readers are a call away (their own records). The
/// order [`Cut::ALL`] holds is pinned here by name, which the ladder test
/// below, reading the order from `Cut::ALL`, cannot do.
#[test]
fn a_records_readers_are_cut_before_its_inputs() {
    let mut host = Host::new(hub());
    let read = |host: &mut Host, budget: usize| -> Value {
        let mut session = Session::new("main");
        session.outline_budget = budget;
        let output = host.call_raw(&mut session, "read_variables", r#"{"names": ["hub"]}"#);
        serde_json::from_str(&output.json).unwrap()
    };
    let whole = read(&mut host, usize::MAX)["variables"][0].clone();
    let (readers, inputs) = (carried(Cut::Readers, &whole), carried(Cut::Inputs, &whole));
    assert!(readers > 1 && inputs > 1, "{whole}");
    let size = serde_json::to_string(&json!({"variables": [&whole]}))
        .unwrap()
        .len();
    let first_cut = (1..size)
        .rev()
        .map(|budget| read(&mut host, budget)["variables"][0].clone())
        .find(|record| carried(Cut::Readers, record) < readers)
        .expect("a budget cuts the readers");
    assert_eq!(carried(Cut::Inputs, &first_cut), inputs, "{first_cut}");
}

/// A record that does not fit an answer alone is cut in one order, least
/// important first, each cut counted, and the answer keeps to its budget
/// under every budget until nothing is left to cut, when it is refused.
#[test]
fn a_record_that_does_not_fit_alone_is_cut_in_order_and_then_refused() {
    let mut cut_somewhere = [false; Cut::ALL.len()];
    for (label, project) in [
        ("scalar", hub()),
        ("table", table_hub()),
        ("arrayed", arrayed_hub()),
    ] {
        let mut host = Host::new(project);
        let read = |host: &mut Host, budget: usize| {
            let mut session = Session::new("main");
            session.outline_budget = budget;
            host.call_raw(&mut session, "read_variables", r#"{"names": ["hub"]}"#)
        };
        let whole_output = read(&mut host, usize::MAX);
        let whole: Value = serde_json::from_str(&whole_output.json).unwrap();
        let whole = &whole["variables"][0];
        assert!(whole.get("truncated").is_none(), "{label}: {whole}");

        let mut refused = false;
        for budget in (1..whole_output.json.len()).rev().step_by(41) {
            let output = read(&mut host, budget);
            if output.is_error {
                refused = true;
                let refusal: Value = serde_json::from_str(&output.json).unwrap();
                let message = refusal["error"].as_str().unwrap();
                assert!(
                    message.contains("does not fit") && message.contains(&budget.to_string()),
                    "{label}: {message}"
                );
                continue;
            }
            assert!(!refused, "{label}: a larger budget was refused");
            assert!(output.json.len() <= budget, "{label}: budget {budget}");
            let answer: Value = serde_json::from_str(&output.json).unwrap();
            let record = &answer["variables"][0];
            let mut earlier_gone = true;
            for (i, cut) in Cut::ALL.into_iter().enumerate() {
                let (left, all) = (carried(cut, record), carried(cut, whole));
                assert!(
                    left == all || earlier_gone,
                    "{label}: budget {budget}: a part is cut only once every part before it is gone: {record}"
                );
                cut_somewhere[i] |= left < all;
                if let Some(count) = counted(cut) {
                    let more = |r: &Value| r[count].as_u64().unwrap_or(0) as usize;
                    assert_eq!(
                        left + more(record),
                        all + more(whole),
                        "{label}: budget {budget}: {count}"
                    );
                }
                earlier_gone &= left == 0;
            }
            assert_eq!(
                record.get("truncated").is_some(),
                carried(Cut::Text, record) < carried(Cut::Text, whole),
                "{label}: budget {budget}: a cut equation says so"
            );
            if record.get("truncated").is_some() {
                let text = record["equation"]
                    .as_str()
                    .or(record["otherElements"].as_str())
                    .unwrap();
                assert!(
                    text.ends_with('…') && text.chars().count() >= MIN_CUT_CHARS,
                    "{label}: {text}"
                );
            }
        }
        assert!(refused, "{label}: the smallest budgets are refused");
    }
    assert!(
        cut_somewhere.iter().all(|&cut| cut),
        "the fixtures and the budgets tried exercise every cut: {cut_somewhere:?}"
    );
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

/// The model the find rows ask: names that share letters without sharing
/// words.
fn shared_letters() -> TestProject {
    [
        "age",
        "wage",
        "stage",
        "usage",
        "average",
        "average life of land",
        "pope",
        "population",
        "popcorn sales",
    ]
    .iter()
    .fold(TestProject::new("letters"), |project, name| {
        project.aux(name, "1", None)
    })
}

/// What each phrase finds, in order: the name itself first, then names
/// whose words it starts, then names like it; a phrase under four
/// characters is like nothing.
#[test]
fn a_name_and_its_word_starts_outrank_names_like_it() {
    let mut host = Host::from_test_project(&shared_letters());
    let mut session = Session::new("main");
    for (phrase, found) in [
        ("age", vec!["age"]),
        ("the", vec![]),
        ("a", vec!["age", "average", "average life of land"]),
        ("pop", vec!["pope", "population", "popcorn sales"]),
        ("aver", vec!["average", "average life of land", "age"]),
        ("populaton", vec!["population"]),
        ("life of land", vec!["average life of land"]),
        ("wage", vec!["wage", "age", "stage", "usage", "average"]),
    ] {
        let output = host.call(&mut session, "find_variables", json!({ "phrase": phrase }));
        let names: Vec<&str> = output["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, found, "{phrase}: {output}");
        let scores: Vec<f64> = output["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["score"].as_f64().unwrap())
            .collect();
        assert!(
            scores.windows(2).all(|w| w[0] >= w[1]),
            "{phrase}: {scores:?}"
        );
    }
}

/// A phrase of the longest length is searched; one longer is refused,
/// saying the limit and its length.
#[test]
fn a_phrase_longer_than_a_name_could_be_is_refused() {
    let mut host = Host::from_test_project(&shared_letters());
    let mut session = Session::new("main");
    let longest = "a".repeat(names::MAX_QUERY_CHARS);
    host.call(&mut session, "find_variables", json!({ "phrase": longest }));
    let longer = format!("{longest}a");
    let refusal = host.refuse(&mut session, "find_variables", json!({ "phrase": longer }));
    let reason = refusal["error"].as_str().unwrap();
    assert!(
        reason.contains(&names::MAX_QUERY_CHARS.to_string())
            && reason.contains(&(names::MAX_QUERY_CHARS + 1).to_string()),
        "{reason}"
    );
}

/// Under a budget too small for every match, the closest are listed and
/// the rest counted.
#[test]
fn matches_that_do_not_fit_are_counted() {
    let mut project = TestProject::new("p");
    for i in 0..MAX_MATCHES {
        project = project.aux(&format!("inventory_{}_{i:02}", "n".repeat(100)), "1", None);
    }
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = 600;
    let output = host.call(
        &mut session,
        "find_variables",
        json!({"phrase": "inventory"}),
    );
    let listed = output["matches"].as_array().unwrap().len();
    assert!(listed < MAX_MATCHES && listed > 0, "{output}");
    assert_eq!(output["omitted"], MAX_MATCHES - listed);
    assert!(output.to_string().len() <= 600);
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

#[test]
fn a_scalar_record_says_what_it_did_in_the_current_run() {
    let project = TestProject::new("goal")
        .with_sim_time(0.0, 40.0, 0.25)
        .stock("s", "0", &["f"], &[], None)
        .flow("f", "(100 - s) / 5", None)
        .named_dimension("d", &["a", "b"])
        .array_const("wide[d]", 1.0);
    let output = read(project.build_datamodel(), &["s", "wide"]);
    let s = &output["variables"][0];
    assert_eq!(s["behavior"]["mode"]["kind"], "goal_seeking");
    assert_eq!(s["behavior"]["start"], 0.0);
    assert!(
        s["behavior"].get("samples").is_none(),
        "a record carries the core; read_behavior has the rest"
    );
    assert!(
        output["variables"][1].get("behavior").is_none(),
        "an arrayed variable's behavior is read_behavior's"
    );
    assert!(output.get("behaviorUnavailable").is_none());
}

#[test]
fn records_of_a_model_that_does_not_simulate_say_why_they_carry_no_behavior() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    let output = read(project, &["Inventory"]);
    assert!(output["variables"][0].get("behavior").is_none());
    assert!(
        output["behaviorUnavailable"]
            .as_str()
            .unwrap()
            .contains("does not simulate")
    );
}

/// A name the caller sent is repeated at most `evidence::ECHO_CHARS`
/// characters of, with its length, wherever an answer repeats it.
#[test]
fn a_long_name_a_caller_sent_is_echoed_cut() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let long = "n".repeat(5_000);
    let answer = host.call(&mut session, "read_variables", json!({ "names": [long] }));
    let echoed = answer["notFound"][0]["name"].as_str().unwrap();
    assert!(echoed.ends_with("(5000 characters)"), "{echoed}");
    assert!(echoed.chars().count() < 200, "{echoed}");
    let refusal = host.refuse(
        &mut session,
        "run_experiment",
        json!({"name": "e", "set": [{"variable": long, "value": 1}]}),
    );
    let reason = refusal["error"].as_str().unwrap();
    assert!(
        reason.contains("(5000 characters)") && reason.len() < 400,
        "{reason}"
    );
}

/// A phrase that starts a name's words outranks every name merely like it,
/// however long the name it starts; a short phrase finds a description by
/// the words it starts, never by a likeness.
#[test]
fn a_word_start_outranks_a_likeness_and_a_short_phrase_has_none_in_documentation() {
    let mut project = TestProject::new("ranks")
        .aux("irate", "1", None)
        .aux("rate_of_the_birth_of_new_people", "1", None)
        .aux("levy", "1", None)
        .build_datamodel();
    project.models[0]
        .get_variable_mut("levy")
        .unwrap()
        .set_documentation("The tax on each sale.");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let found = |host: &mut Host, session: &mut Session, phrase: &str| -> Vec<String> {
        host.call(session, "find_variables", json!({ "phrase": phrase }))["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        found(&mut host, &mut session, "rate"),
        ["rate_of_the_birth_of_new_people", "irate"]
    );
    assert_eq!(found(&mut host, &mut session, "tax"), ["levy"]);
    assert!(found(&mut host, &mut session, "tex").is_empty());
}
