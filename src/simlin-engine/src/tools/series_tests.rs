// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

fn behavior(host: &mut Host, session: &mut Session, input: Value) -> Value {
    host.call(session, "read_behavior", input)
}

#[test]
fn numbers_keep_five_significant_digits() {
    for (x, rounded) in [
        (123_456.789, 123_460.0),
        (0.000_123_456_7, 0.000_123_46),
        (-98.765_43, -98.765),
        (1.0, 1.0),
        (0.1 + 0.2, 0.3),
        (0.0, 0.0),
    ] {
        assert_eq!(round(x), rounded, "{x}");
    }
    assert!(round(f64::NAN).is_nan());
    assert_eq!(round(f64::INFINITY), f64::INFINITY);
}

#[test]
fn a_summary_says_where_a_series_went_when_and_how() {
    let project = TestProject::new("goal")
        .with_sim_time(0.0, 40.0, 0.25)
        .stock("s", "0", &["f"], &[], None)
        .flow("f", "(100 - s) / 5", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = behavior(&mut host, &mut session, json!({"variables": ["s"]}));
    let summary = &output["series"][0];
    assert_eq!(summary["variable"], "s");
    assert_eq!(summary["run"], "current");
    assert_eq!(summary["start"], 0.0);
    assert_eq!(summary["min"], json!({"time": 0.0, "value": 0.0}));
    assert_eq!(summary["max"]["time"], 40.0);
    assert!(summary["end"].as_f64().unwrap() > 99.0);
    assert_eq!(summary["mode"]["kind"], "goal_seeking");
    assert!(summary.get("turns").is_none() && summary.get("negativeFrom").is_none());
    let samples = summary["samples"].as_array().unwrap();
    assert_eq!(samples.len(), SAMPLES);
    assert_eq!(samples[0], summary["min"]);
    assert_eq!(samples[SAMPLES - 1]["time"], 40.0);
}

#[test]
fn a_stock_that_goes_negative_says_when_and_one_at_rest_does_not() {
    let project = TestProject::new("drained")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("s", "5", &[], &["f"], None)
        .flow("f", "1", None)
        .stock("still", "100", &["g"], &[], None)
        .flow("g", "(100 - still) / 4", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = behavior(
        &mut host,
        &mut session,
        json!({"variables": ["s", "still"]}),
    );
    assert_eq!(output["series"][0]["negativeFrom"], 6.0);
    assert!(output["series"][1].get("negativeFrom").is_none());
    assert_eq!(output["series"][1]["mode"]["kind"], "at_rest");
}

#[test]
fn an_oscillation_lists_its_turning_points_up_to_six() {
    let project = TestProject::new("oscillator")
        .with_sim_time(0.0, 80.0, 0.0625)
        .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
        .stock("x", "0", &["dx"], &[], None)
        .stock("v", "0", &["dv"], &[], None)
        .flow("dx", "v", None)
        .flow("dv", "0.25 * (100 - x) - 0.1 * v", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = behavior(&mut host, &mut session, json!({"variables": ["x"]}));
    let turns = output["series"][0]["turns"].as_array().unwrap();
    assert_eq!(turns.len(), MAX_TURNS);
    assert!(
        turns[0]["value"].as_f64().unwrap() > 100.0,
        "the first is the overshoot"
    );
    assert!(turns[1]["value"].as_f64().unwrap() < 100.0);
}

#[test]
fn an_arrayed_variable_is_summarized_element_by_element_up_to_the_limit() {
    let elements: Vec<String> = (1..=10).map(|i| format!("e{i}")).collect();
    let refs: Vec<&str> = elements.iter().map(String::as_str).collect();
    let project = TestProject::new("wide")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("d", &refs)
        .array_stock("stock[d]", "1", &["inflow"], &[], None)
        .array_flow("inflow[d]", "stock * 0.1", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = behavior(&mut host, &mut session, json!({"variables": ["stock"]}));
    let labels: Vec<&str> = output["series"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["variable"].as_str().unwrap())
        .collect();
    assert_eq!(labels.len(), MAX_ELEMENTS);
    assert_eq!(labels[0], "stock[e1]");
    assert_eq!(
        output["omittedElements"],
        json!([{"variable": "stock", "count": 2}])
    );
}

#[test]
fn runs_are_read_side_by_side_and_a_run_of_an_earlier_model_says_so() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "slow", "set": [{"variable": "adjustment_time", "value": 8}]}),
    );
    let output = behavior(
        &mut host,
        &mut session,
        json!({"variables": ["Inventory", "production"], "runs": ["current", "slow"]}),
    );
    let pairs: Vec<(&str, &str)> = output["series"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["variable"].as_str().unwrap(), s["run"].as_str().unwrap()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("Inventory", "current"),
            ("Inventory", "slow"),
            ("production", "current"),
            ("production", "slow")
        ]
    );
    assert!(output.get("staleRuns").is_none());

    host.edit(|p| {
        p.models[0]
            .get_variable_mut("coverage")
            .unwrap()
            .set_scalar_equation("5")
    });
    let output = behavior(
        &mut host,
        &mut session,
        json!({"variables": ["Inventory"], "runs": ["current", "slow"]}),
    );
    assert_eq!(
        output["staleRuns"],
        json!([{"run": "slow", "revision": 0}]),
        "the current run follows the model; the experiment was made before the edit"
    );
}

#[test]
fn read_behavior_refuses_too_much_and_answers_names_it_does_not_know() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let many: Vec<String> = (0..=MAX_BEHAVIOR_VARIABLES)
        .map(|i| format!("v{i}"))
        .collect();
    for (input, says) in [
        (json!({"variables": many}), "between 1 and 12"),
        (json!({"variables": []}), "between 1 and 12"),
        (
            json!({"variables": ["Inventory"], "runs": ["a", "b", "c", "d", "e"]}),
            "at most 4",
        ),
        (
            json!({"variables": ["Inventory"], "runs": ["nowhere"]}),
            "no run named",
        ),
    ] {
        let refusal = host.refuse(&mut session, "read_behavior", input.clone());
        assert!(
            refusal["error"].as_str().unwrap().contains(says),
            "{input}: {refusal}"
        );
    }
    let refusal = host.refuse(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"], "runs": ["nowhere"]}),
    );
    assert_eq!(refusal["suggestions"], json!(["current"]));

    let output = behavior(
        &mut host,
        &mut session,
        json!({"variables": ["Inventory", "invntory"]}),
    );
    assert_eq!(output["series"].as_array().unwrap().len(), 1);
    assert_eq!(output["notFound"][0]["name"], "invntory");
}

#[test]
fn a_model_that_does_not_simulate_is_refused_with_the_reason() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let refusal = host.refuse(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"]}),
    );
    let message = refusal["error"].as_str().unwrap();
    assert!(
        message.contains("does not simulate") && message.contains("read_model"),
        "{message}"
    );
}

/// A series that moves less than a summary's five digits show is at rest,
/// and says so: every number it reports reads the same.
#[test]
fn a_series_flat_to_every_reported_digit_is_at_rest() {
    let project = TestProject::new("flat")
        .with_sim_time(0.0, 20.0, 1.0)
        .stock("s", "100", &["f"], &[], None)
        .flow("f", "(100.0001 - s) / 5", None);
    let mut host = Host::from_test_project(&project);
    let output = behavior(
        &mut host,
        &mut Session::new("main"),
        json!({"variables": ["s"]}),
    );
    let s = &output["series"][0];
    assert_eq!(s["mode"]["kind"], "at_rest", "{s}");
    assert_eq!(s["start"], 100.0);
}

#[test]
fn an_element_named_is_summarized_alone_past_the_list_limit() {
    let elements: Vec<String> = (1..=10).map(|i| format!("r{i}")).collect();
    let refs: Vec<&str> = elements.iter().map(String::as_str).collect();
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("region", &refs)
        .array_stock("population[region]", "100", &["births"], &[], None)
        .array_flow("births[region]", "population * rate", None)
        .aux("rate", "0.1", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = behavior(
        &mut host,
        &mut session,
        json!({"variables": ["population[r10]", "population[R9]", "population[r11]"]}),
    );
    let series = output["series"].as_array().unwrap();
    assert_eq!(series.len(), 2, "{output}");
    assert_eq!(series[0]["variable"], "population[r10]");
    assert_eq!(series[1]["variable"], "population[r9]");
    assert_eq!(output["notFound"][0]["name"], "population[r11]");
}

/// An answer over its budget leaves out samples first, then turning points,
/// then elements, then whole variables, and says what it left out.
#[test]
fn an_answer_over_its_budget_leaves_out_samples_then_turns_then_elements_then_variables() {
    let elements: Vec<String> = (1..=8).map(|i| format!("r{i}")).collect();
    let refs: Vec<&str> = elements.iter().map(String::as_str).collect();
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 40.0, 0.25)
        .named_dimension("region", &refs)
        .array_stock("population[region]", "100", &["births"], &["deaths"], None)
        .array_flow("births[region]", "population * 0.1", None)
        .array_flow("deaths[region]", "population * 0.1 * (1 + SIN(TIME))", None)
        .aux("rate", "0.1", None);
    let input = json!({"variables": ["population", "births", "deaths"]});
    let mut host = Host::from_test_project(&project);
    let mut roomy = Session::new("main");
    roomy.outline_budget = usize::MAX;
    let whole = behavior(&mut host, &mut roomy, input.clone());
    let whole_len = whole.to_string().len();
    assert!(whole.get("leftOut").is_none());

    // A budget that dropping the samples alone meets keeps the turning points.
    let mut without_samples = whole.clone();
    for series in without_samples["series"].as_array_mut().unwrap() {
        series.as_object_mut().unwrap().remove("samples");
    }
    without_samples["leftOut"] = json!({"samples": true});
    let mut session = Session::new("main");
    session.outline_budget = without_samples.to_string().len();
    let output = behavior(&mut host, &mut session, input.clone());
    assert_eq!(
        output["leftOut"],
        json!({"samples": true}),
        "only the samples go"
    );
    assert_eq!(output["series"], without_samples["series"]);

    let mut previous = usize::MAX;
    for budget in [
        whole_len - 1,
        whole_len / 2,
        whole_len / 4,
        whole_len / 16,
        1,
    ] {
        let mut session = Session::new("main");
        session.outline_budget = budget;
        let output = behavior(&mut host, &mut session, input.clone());
        let len = output.to_string().len();
        assert!(len <= previous, "a smaller budget never answers more");
        previous = len;
        let left = &output["leftOut"];
        assert_eq!(left["samples"], true, "samples go first: budget {budget}");
        let series = output["series"].as_array().unwrap();
        assert!(series.iter().all(|s| s.get("samples").is_none()));
        if left["turns"] != true {
            assert!(output.get("omittedElements").is_none());
        }
        if left.get("variables").is_some() {
            assert_eq!(left["turns"], true);
            assert!(!series.is_empty(), "one variable always answers");
        }
        if budget > 1 && left.get("variables").is_none() {
            assert!(len <= budget, "budget {budget}: {len}");
        }
    }
}
