// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::behavior::ModeKind;
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

/// A rounded number prints with the digits it was rounded to and no more, at
/// every magnitude a float has, and is within half a unit of its last digit
/// of the number it rounds.
#[test]
fn a_rounded_number_prints_with_the_digits_it_was_rounded_to() {
    let printed_digits = |x: f64| {
        let printed = format!("{:e}", x.abs());
        let mantissa = printed.split('e').next().unwrap_or_default();
        mantissa.chars().filter(char::is_ascii_digit).count()
    };
    // Numbers of five digits already are themselves.
    for x in [
        1.6e9, 3.4634e9, 7.4668e9, 1.4752e9, 2.6058e12, 0.18382, 123.45,
    ] {
        assert_eq!(round(x), x);
        assert!(printed_digits(round(x)) <= 5, "{x} prints as {}", round(x));
    }
    for mantissa in [1.6, 3.4634, 7.46681234, 1.47525, 9.99996, 1.00000491, 5.5] {
        for exponent in (-300..=300).step_by(3) {
            for sign in [1.0, -1.0] {
                let x: f64 = sign * mantissa * 10f64.powi(exponent);
                let rounded = round(x);
                assert!(
                    printed_digits(rounded) <= SIGNIFICANT_DIGITS as usize,
                    "{x} prints as {rounded}"
                );
                let unit = 10f64.powi(x.abs().log10().floor() as i32 - 4);
                assert!((rounded - x).abs() <= 0.5 * unit * (1.0 + 1e-9), "{x}");
            }
        }
    }
}

fn steps(n: usize) -> Vec<f64> {
    (0..n).map(|i| i as f64).collect()
}

/// A summary says a series went negative exactly when a number it reports
/// of the series is negative, and every number it reports is the series'
/// own, rounded: the residue an equilibrium leaves below zero is reported as
/// the residue it is, and is a time the series was below zero.
///
/// The series are written out: `summarize` reads a run's rows as two slices
/// and nothing else of the run.
#[test]
fn a_summary_reports_a_negative_number_exactly_when_the_series_went_negative() {
    let rows: [(&[f64], Option<f64>); 8] = [
        // Residue of a drain from a hundred: a part in 1e17.
        (&[100.0, 40.0, -1e-15, 0.0], Some(2.0)),
        (&[100.0, 40.0, -2e-7, 5.0], Some(2.0)),
        // A small model: a thousandth of its own magnitude below zero.
        (&[1e-6, 5e-7, -1e-9, 2e-7], Some(2.0)),
        (&[0.0, -3.0, -5.0, -1.0], Some(1.0)),
        (&[-4.0, -3.0, 2.0, 5.0], Some(0.0)),
        (&[0.0, 0.0, 0.0, 0.0], None),
        // Zero below zero is not.
        (&[3.0, -0.0, 1.0, 0.0], None),
        (&[3.0, 2.0, 1.0, 0.0], None),
    ];
    for (values, negative_from) in rows {
        let core = SeriesCore::at(&steps(values.len()), values, 0.0);
        assert_eq!(core.negative_from, negative_from, "{values:?}");
        let reported = [
            core.start,
            core.end,
            core.min.map(|point| point.value),
            core.max.map(|point| point.value),
        ];
        let least = reported
            .iter()
            .flatten()
            .fold(f64::INFINITY, |a, &b| a.min(b));
        assert_eq!(least < 0.0, negative_from.is_some(), "{values:?}");
        assert_eq!(
            core.min.map(|point| point.value < 0.0),
            Some(negative_from.is_some()),
            "{values:?}"
        );
        assert_eq!(
            core.min.map(|point| point.value),
            values.iter().copied().map(round).reduce(f64::min),
            "{values:?}"
        );
    }
    // The residue is reported as itself wherever it is reported: extremes,
    // turning points and samples.
    let values = [50.0, 0.0, -1e-15, 30.0, -2e-15, 40.0];
    let (core, turns, samples) = summarize(&steps(values.len()), &values, 0.0, true);
    assert_eq!(core.negative_from, Some(2.0));
    assert_eq!(core.min.map(|point| point.value), Some(-2e-15));
    assert!(!turns.is_empty());
    for point in turns.iter().chain(&samples) {
        let i = point.time as usize;
        assert_eq!(point.value, round(values[i]), "at {i}");
    }
}

/// An extreme is reported at the time the series first reached it: a series
/// that holds its greatest value peaks where it got there.
#[test]
fn an_extreme_is_reported_where_the_series_first_reached_it() {
    let values = [0.0, 5.0, 5.0, 3.0, 0.0, 0.0, 5.0];
    let core = SeriesCore::at(&steps(values.len()), &values, 0.0);
    let at = |point: Option<Point>| point.map(|point| (point.time, point.value));
    assert_eq!(at(core.max), Some((1.0, 5.0)));
    assert_eq!(at(core.min), Some((0.0, 0.0)));
}

/// What a summary says of a series' movement agrees with the numbers beside
/// it, at any scale: a series that is not at rest reports a least and a
/// greatest value that differ, and one at rest reports them no more than one
/// unit in the last digit apart.
#[test]
fn a_summarys_mode_agrees_with_its_extremes() {
    let mut moving = 0;
    let mut resting = 0;
    for base in [
        1e-12, 4.4e-5, 1.0, 1.23455, 99.9996, 123.45, 6.02e23, -7300.0,
    ] {
        for spread in [
            0.0, 1e-9, 3e-6, 0.5e-4, 0.99e-4, 1.01e-4, 2e-4, 1e-3, 0.05, 1.0, 40.0,
        ] {
            let values: Vec<f64> = (0..20)
                .map(|i| base * (1.0 + spread * f64::from(i) / 19.0))
                .collect();
            let core = SeriesCore::at(&steps(values.len()), &values, 0.0);
            let (Some(min), Some(max)) = (core.min, core.max) else {
                panic!("{base} {spread}: a finite series has extremes");
            };
            let what = format!(
                "base {base} spread {spread}: {} to {}",
                min.value, max.value
            );
            if core.mode.kind == ModeKind::AtRest {
                resting += 1;
                let unit =
                    10f64.powi(max.value.abs().max(min.value.abs()).log10().floor() as i32 - 4);
                assert!(max.value - min.value <= unit * (1.0 + 1e-6), "{what}");
            } else {
                moving += 1;
                assert_ne!(min.value, max.value, "{what}");
            }
        }
    }
    assert!(
        moving >= 30 && resting >= 30,
        "{moving} moving, {resting} at rest"
    );
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

/// A model in equilibrium whose flows do not cancel to the last bit: `gap`
/// is fed `demand * fraction` and drained `demand / 10`, which differ by
/// 5.5e-17, so it creeps to 1e-15 over the run.
fn in_equilibrium_but_for_residue() -> TestProject {
    in_equilibrium_but_for_residue_over(20.0, 0.25)
}

/// [`in_equilibrium_but_for_residue`] run for `stop` time units at `dt`: the
/// residue grows with the run.
fn in_equilibrium_but_for_residue_over(stop: f64, dt: f64) -> TestProject {
    TestProject::new("eq")
        .with_sim_time(0.0, stop, dt)
        .aux("demand", "3", None)
        .aux("fraction", "0.1", None)
        .flow("orders", "demand * fraction", None)
        .flow("fulfilled", "demand / 10", None)
        .stock("gap", "0", &["orders"], &["fulfilled"], None)
        .aux("net", "orders - fulfilled", None)
}

/// The same structure with every quantity a trillionth the size, and a gap
/// that does open: `fraction` is 0.2, so the stock rises to 6e-12.
fn tiny_and_moving() -> TestProject {
    TestProject::new("tiny")
        .with_sim_time(0.0, 20.0, 0.25)
        .aux("demand", "3e-12", None)
        .aux("fraction", "0.2", None)
        .flow("orders", "demand * fraction", None)
        .flow("fulfilled", "demand / 10", None)
        .stock("gap", "0", &["orders"], &["fulfilled"], None)
        .aux("net", "orders - fulfilled", None)
}

/// A variable's scale in a run is what its values are sums and differences
/// of: a stock's flows over the horizon, the terms of an equation that is a
/// sum. A product, a quotient, a lone reference, a module and a name the
/// model lacks have none. An element's scale is its own.
#[test]
fn a_variables_scale_is_what_it_is_a_sum_of() {
    let project = TestProject::new("scales")
        .with_sim_time(0.0, 20.0, 0.25)
        .named_dimension("region", &["north", "south"])
        .aux("big", "5000", None)
        .aux("small", "0.25", None)
        .flow("filling", "40", None)
        .flow("draining", "big / 1000", None)
        .stock("level", "0", &["filling"], &["draining"], None)
        .aux("difference", "big - small", None)
        .aux("negated_sum", "-(small + big) - 7", None)
        .aux("less_a_number", "small - 7000", None)
        .aux("with_a_product", "small + big * small", None)
        .aux("product", "big * small", None)
        .aux("quotient", "small / big", None)
        .aux("alias", "big", None)
        .array_aux("by_region[region]", "big - 1")
        .stock("still", "3", &[], &[], None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let Ok(run) = session.run_results(host.workspace(), "current") else {
        panic!("the model runs");
    };
    let model = &host.project.models[0];
    for (variable, scale) in [
        // The larger flow, 40, over a horizon of 20.
        ("level", 800.0),
        ("difference", 5000.0),
        // A sign in front of a sum is the sum's: its terms are the terms.
        ("negated_sum", 5000.0),
        // A number is a term like any other.
        ("less_a_number", 7000.0),
        // The product is a term the scale cannot see: the variable term's.
        ("with_a_product", 0.25),
        ("product", 0.0),
        ("quotient", 0.0),
        ("alias", 0.0),
        ("by_region", 5000.0),
        ("by_region[north]", 5000.0),
        ("still", 0.0),
        ("no_such_variable", 0.0),
    ] {
        assert_eq!(
            scale_in_run(&run.results, model, variable),
            scale,
            "{variable}"
        );
    }

    // Each element is read at the scale of the same element of what it is
    // computed from, a term a subscript pins at the element it names, and a
    // term of another shape not at all.
    let project = TestProject::new("elements")
        .with_sim_time(0.0, 100.0, 0.5)
        .named_dimension("region", &["big", "small"])
        .array_with_ranges("gain[region]", vec![("big", "1e12"), ("small", "1")])
        .array_with_ranges(
            "start_level[region]",
            vec![("big", "1e13"), ("small", "10")],
        )
        .array_stock("level[region]", "start_level", &["fill"], &[], None)
        .array_flow("fill[region]", "gain", None)
        .array_aux("short[region]", "start_level * 2 - level")
        .array_aux("pinned[region]", "level[small] - 1")
        .array_aux("named[region]", "level[region] - 1")
        .aux("scalar_term", "3", None)
        .array_aux("with_scalar[region]", "scalar_term - level")
        .array_with_ranges(
            "by_element[region]",
            vec![("big", "gain - 1"), ("small", "gain * 2")],
        );
    let mut host = Host::from_test_project(&project);
    let Ok(run) = Session::new("main").run_results(host.workspace(), "current") else {
        panic!("the model runs");
    };
    let model = &host.project.models[0];
    for (key, scale) in [
        ("level[big]", 1e12 * 100.0),
        ("level[small]", 100.0),
        ("short[big]", 1e12 * 100.0 + 1e13),
        ("short[small]", 110.0),
        ("pinned[big]", 110.0),
        ("pinned[small]", 110.0),
        ("named[big]", 1e12 * 100.0 + 1e13),
        ("named[small]", 110.0),
        ("with_scalar[small]", 110.0),
        // The element's own equation: a sum for one element, a product for
        // the other.
        ("by_element[big]", 1e12),
        ("by_element[small]", 0.0),
    ] {
        assert_eq!(scale_in_run(&run.results, model, key), scale, "{key}");
    }
}

/// Gross flows of 1e9 that differ by `net`, from 0 for `stop` time units:
/// the stock moves `net * stop`, a real change however small beside them.
fn drift(inflow: &str, outflow: &str, stop: f64) -> TestProject {
    TestProject::new("drift")
        .with_sim_time(0.0, stop, 0.25)
        .flow("in_flow", inflow, None)
        .flow("out_flow", outflow, None)
        .stock("balance", "0", &["in_flow"], &["out_flow"], None)
        .aux("net_flow", "in_flow - out_flow", None)
}

/// Flows of 1e9 in and out of a stock that cancel exactly, and a real
/// inflow of `leak` beside them, for 100 time units.
fn held(leak: &str) -> TestProject {
    TestProject::new("held")
        .with_sim_time(0.0, 100.0, 0.25)
        .flow("through_in", "1e9", None)
        .flow("through_out", "1e9", None)
        .flow("leak_in", leak, None)
        .stock(
            "held",
            "0",
            &["through_in", "leak_in"],
            &["through_out"],
            None,
        )
}

/// An error that is zero but for rounding (`0.30000000000000004` against
/// `0.3`), integrated by a stock.
fn integral_of_an_error() -> TestProject {
    TestProject::new("integral")
        .with_sim_time(0.0, 100.0, 0.125)
        .aux("capacity", "3", None)
        .aux("fraction", "0.1", None)
        .aux("target", "capacity * fraction", None)
        .aux("measurement", "capacity / 10", None)
        .flow("error", "target - measurement", None)
        .stock("error_integral", "0", &["error"], &[], None)
}

/// An arrayed stock whose small element moves by 1e-3 beside a big element
/// fed 1e12 a time unit.
fn tiny_beside_huge() -> TestProject {
    TestProject::new("elements")
        .with_sim_time(0.0, 100.0, 0.5)
        .named_dimension("region", &["big", "small"])
        .array_with_ranges("gain[region]", vec![("big", "1e12"), ("small", "1e-5")])
        .array_with_ranges("start_level[region]", vec![("big", "1e13"), ("small", "0")])
        .array_stock("level[region]", "start_level", &["fill"], &[], None)
        .array_flow("fill[region]", "gain", None)
}

/// What every tool that reads a variable's behavior says of it in the
/// model's run, after checking they say the same: the mode, and the numbers.
///
/// `read_behavior`, the behavior line of `read_variables` and
/// `run_experiment`'s record report the same mode and the same numbers, each
/// the series' own rounded; `verify_findings` holds a citation of that mode
/// and of the reported end, holds one of `at_rest` exactly when that is the
/// mode, and says a series at rest has no peak exactly when it is at rest.
fn read_by_every_tool(project: &TestProject, variable: &str) -> (String, Value) {
    let mut host = Host::from_test_project(project);
    let mut session = Session::new("main");
    let read = behavior(&mut host, &mut session, json!({"variables": [variable]}));
    let summary = read["series"][0].clone();
    let kind = summary["mode"]["kind"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let numbers = |core: &Value| {
        json!([
            core["start"],
            core["end"],
            core["min"],
            core["max"],
            core["negativeFrom"]
        ])
    };

    let record = host.call(&mut session, "read_variables", json!({"names": [variable]}));
    let line = &record["variables"][0]["behavior"];
    assert_eq!(line["mode"], summary["mode"], "{variable}: {record}");
    assert_eq!(numbers(line), numbers(&summary), "{variable}: {record}");

    let base = variable.split('[').next().unwrap_or(variable);
    let experiment = host.call(
        &mut session,
        "run_experiment",
        json!({"name": "again", "record": [base]}),
    );
    let compared = experiment["behavior"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["variable"] == variable))
        .unwrap_or_else(|| panic!("{variable} is recorded: {experiment}"));
    for side in ["this", "base"] {
        assert_eq!(
            compared[side]["mode"]["kind"],
            kind.as_str(),
            "{variable}: {compared}"
        );
        assert_eq!(
            numbers(&compared[side]),
            numbers(&summary),
            "{variable}: {compared}"
        );
    }

    let verify = |session: &mut Session, host: &mut Host, citation: Value| -> Value {
        host.call(
            session,
            "verify_findings",
            json!({"findings": [{"kind": "observation", "claim": "c", "citations": [citation]}]}),
        )["findings"][0]
            .clone()
    };
    let holds = |finding: &Value| finding["holds"] == true;
    let cited = verify(
        &mut session,
        &mut host,
        json!({"cites": "behavior_mode", "variable": variable, "mode": kind}),
    );
    assert!(holds(&cited), "{variable}: {cited}");
    let at_rest = verify(
        &mut session,
        &mut host,
        json!({"cites": "behavior_mode", "variable": variable, "mode": "at_rest"}),
    );
    assert_eq!(holds(&at_rest), kind == "at_rest", "{variable}: {at_rest}");
    let ends = verify(
        &mut session,
        &mut host,
        json!({"cites": "ends_near", "variable": variable, "value": summary["end"]}),
    );
    assert!(holds(&ends), "{variable}: {ends}");
    let peak = verify(
        &mut session,
        &mut host,
        json!({"cites": "peaks_at", "variable": variable, "time": 0}),
    );
    let no_peak = peak.to_string().contains("so it has no peak");
    assert_eq!(no_peak, kind == "at_rest", "{variable}: {peak}");
    if no_peak {
        // It names the value it holds at as the summary does.
        let start = summary["start"].as_f64().unwrap_or(f64::NAN);
        let says = format!("holds at {start} throughout");
        assert!(
            peak.to_string().contains(&says),
            "{variable}: {peak} says {says}"
        );
    }
    (kind, summary)
}

/// A series is at rest when it is the residue of quantities that cancel --
/// at most the rounding floating point leaves at the scale of what it is
/// computed from -- and moving whenever it moves by more, however large the
/// quantities beside it; every tool says the same of it, with the series'
/// own numbers.
#[test]
fn every_tool_sees_residue_at_rest_and_every_real_movement_move() {
    let seeking = || {
        TestProject::new("seeking")
            .with_sim_time(0.0, 100.0, 0.25)
            .aux("goal", "50", None)
            .flow("in_flow", "1e9 + (goal - balance) / 10", None)
            .flow("out_flow", "1e9", None)
            .stock("balance", "0", &["in_flow"], &["out_flow"], None)
            .aux("gap", "goal - balance", None)
    };
    let difference = || {
        TestProject::new("difference")
            .with_sim_time(0.0, 100.0, 0.5)
            .aux("capacity", "1e9", None)
            .aux("demand", "1e9 + TIME / 200", None)
            .aux("shortfall", "demand - capacity", None)
            .aux("swing", "1e9 + 0.4 * SIN(TIME / 5)", None)
            .aux("imbalance", "swing - capacity", None)
    };
    let arrayed = || {
        TestProject::new("arrayed")
            .with_sim_time(0.0, 100.0, 0.5)
            .named_dimension("region", &["big", "small"])
            .array_with_ranges("gain[region]", vec![("big", "1e12"), ("small", "1")])
            .array_with_ranges(
                "start_level[region]",
                vec![("big", "1e13"), ("small", "10")],
            )
            .array_stock("level[region]", "start_level", &["fill"], &[], None)
            .array_flow("fill[region]", "gain", None)
            .array_aux("target[region]", "start_level * 2")
            .array_aux("short[region]", "target - level")
    };
    let backlog = || {
        TestProject::new("backlog")
            .with_sim_time(0.0, 20.0, 0.25)
            .aux("demand", "3", None)
            .aux("fraction", "0.1", None)
            .flow("orders", "demand * fraction", None)
            .flow("fulfilled", "demand / 10 + backlog / 5", None)
            .stock("backlog", "0", &["orders"], &["fulfilled"], None)
    };
    // (model, variable, mode, where it ends)
    let rows: Vec<(TestProject, &str, &str, f64)> = vec![
        // Residue: a stock that creeps to 1e-15 on flows of 0.3 that differ
        // by 5.5e-17, the difference of those flows, and a stock in a loop
        // that settles at 3e-16.
        (
            in_equilibrium_but_for_residue(),
            "gap",
            "at_rest",
            1.1102e-15,
        ),
        (
            in_equilibrium_but_for_residue(),
            "net",
            "at_rest",
            5.5511e-17,
        ),
        (in_equilibrium_but_for_residue(), "orders", "at_rest", 0.3),
        // Run ten times as long, the stock's residue is ten times as large,
        // and still within the rounding of its flows over the run.
        (
            in_equilibrium_but_for_residue_over(200.0, 1.0),
            "gap",
            "at_rest",
            1.1102e-14,
        ),
        (backlog(), "backlog", "at_rest", 1.3878e-16),
        // A model whose quantities are all of order 1e-12 moves.
        (tiny_and_moving(), "gap", "linear", 6e-12),
        // Gross flows of 1e9 that differ by 0.5 for 100 time units, by 1 for
        // 10; of 1e6 by 5e-4; and by a net that grows with the clock.
        (drift("1e9 + 0.5", "1e9", 100.0), "balance", "linear", 50.0),
        (drift("1e9 + 0.5", "1e9", 100.0), "net_flow", "at_rest", 0.5),
        (drift("1e9 + 1", "1e9", 10.0), "balance", "linear", 10.0),
        (drift("1e6 + 5e-4", "1e6", 100.0), "balance", "linear", 0.05),
        (
            drift("1e9 + TIME / 100", "1e9", 100.0),
            "balance",
            "exponential",
            49.875,
        ),
        // Goal seeking from 0 to 50 beside them, in a loop.
        (seeking(), "balance", "goal_seeking", 49.998),
        (seeking(), "gap", "goal_seeking", 0.001999),
        // The difference of two variables of 1e9: a rise of 0.5, a swing of
        // 0.4.
        (difference(), "shortfall", "linear", 0.5),
        (difference(), "imbalance", "oscillation", 0.36518),
        // An element is read at its own scale: the small one moves by 100
        // beside the big one's 1e14.
        (arrayed(), "level[small]", "linear", 110.0),
        (arrayed(), "level[big]", "linear", 1.1e14),
        (arrayed(), "short[small]", "linear", -90.0),
        // So is one that moves by 1e-3 beside an element of 1e12, which
        // read at its variable's scale would be at rest.
        (tiny_beside_huge(), "level[small]", "linear", 0.001),
        // A stock that integrates a difference that cancels but for
        // rounding is at the scale of its flow's terms: at rest.
        (
            integral_of_an_error(),
            "error_integral",
            "at_rest",
            5.5511e-15,
        ),
        // Beside flows of 1e9 that cancel exactly, a real inflow of 2e-5
        // over 100 time units moves 90 units of rounding of the stock's
        // scale, and is seen moving; one of 1e-5 moves 45, and is residue.
        (held("2e-5"), "held", "linear", 0.0020027),
        (held("1e-5"), "held", "at_rest", 0.0010014),
    ];
    for (project, variable, mode, end) in rows {
        let (kind, summary) = read_by_every_tool(&project, variable);
        assert_eq!(kind, mode, "{variable}: {summary}");
        assert_eq!(summary["end"], end, "{variable}: {summary}");
    }
}

/// The loop analysis reads the run's stocks at the scale every tool does: a
/// stock in a loop that settles at residue is at rest, and the loops come
/// from structure; one that goal-seeks by 50 beside flows of 1e9 moves, and
/// the run's loops are analyzed.
#[test]
fn the_loop_analysis_reads_stocks_at_rest_as_every_tool_does() {
    let backlog = TestProject::new("backlog")
        .with_sim_time(0.0, 20.0, 0.25)
        .aux("demand", "3", None)
        .aux("fraction", "0.1", None)
        .flow("orders", "demand * fraction", None)
        .flow("fulfilled", "demand / 10 + backlog / 5", None)
        .stock("backlog", "0", &["orders"], &["fulfilled"], None);
    let seeking = TestProject::new("seeking")
        .with_sim_time(0.0, 100.0, 0.25)
        .aux("goal", "50", None)
        .flow("in_flow", "1e9 + (goal - balance) / 10", None)
        .flow("out_flow", "1e9", None)
        .stock("balance", "0", &["in_flow"], &["out_flow"], None);
    // A loop through an arrayed stock: its big element held exactly where
    // it is by flows of 1e12, its small one seeking a level of 1e-4. Each
    // element is read at its own scale, so the small one moves.
    let elements = TestProject::new("elements")
        .with_sim_time(0.0, 100.0, 0.5)
        .named_dimension("region", &["big", "small"])
        .array_with_ranges("gain[region]", vec![("big", "1e12"), ("small", "1e-5")])
        .array_with_ranges("start_level[region]", vec![("big", "1e13"), ("small", "0")])
        .array_stock("level[region]", "start_level", &["fill"], &["drain"], None)
        .array_flow("fill[region]", "gain", None)
        .array_flow("drain[region]", "level * 0.1", None);
    for (project, basis) in [
        (&backlog, "structure"),
        (&seeking, "run"),
        (&elements, "run"),
    ] {
        let mut host = Host::from_test_project(project);
        let loops = host.call(&mut Session::new("main"), "analyze_loops", json!({}));
        assert_eq!(loops["basis"], basis, "{loops}");
        assert_eq!(loops["found"], 1, "{loops}");
    }
}
