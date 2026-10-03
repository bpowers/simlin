// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::datamodel::Visibility;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::behavior::shape;
use crate::tools::test_support::{Host, inventory};

/// Call `run_tests`, which must answer, and check the answer against the
/// output schema the catalog publishes and against its own counts.
fn run(host: &mut Host, session: &mut Session, input: Value) -> Value {
    let output = host.call(session, "run_tests", input);
    let catalog: Value = serde_json::from_str(crate::tools::catalog_json()).unwrap();
    let schema = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "run_tests")
        .unwrap()["outputSchema"]
        .clone();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&output)
        .map(|e| format!("{e} at {}", e.instance_path))
        .collect();
    assert!(errors.is_empty(), "{errors:?}\n{output}");
    for summary in output["tests"].as_array().unwrap() {
        let count = |field: &str| summary[field].as_u64().unwrap_or(0);
        assert_eq!(
            count("checks"),
            ["passed", "failed", "flagged", "observed", "notRun"]
                .iter()
                .map(|f| count(f))
                .sum::<u64>(),
            "{summary}"
        );
    }
    output
}

/// The listed result of `test` changing `variable` by `condition`.
fn result<'a>(output: &'a Value, test: &str, variable: &str, condition: &str) -> &'a Value {
    output["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["test"] == test && r["variable"] == variable && r["condition"] == condition)
        .unwrap_or_else(|| panic!("{test} of {variable} at {condition} is listed: {output}"))
}

fn summary<'a>(output: &'a Value, test: &str) -> &'a Value {
    output["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["test"] == test)
        .unwrap_or_else(|| panic!("{test} ran: {output}"))
}

/// The time constants of `project`'s model, each with why and its stages.
fn time_constants_of(project: &TestProject) -> Vec<(String, TimeConstantEvidence, f64)> {
    time_constants_in(project.build_datamodel())
}

/// The time constants of `project`'s first model.
fn time_constants_in(project: datamodel::Project) -> Vec<(String, TimeConstantEvidence, f64)> {
    let mut host = Host::new(project);
    let ws = host.workspace();
    let resolved = resolve_model(ws.project, ws.db, "main").ok().unwrap();
    let graph = Graph::of(ws.db, &resolved);
    let units = Units::of(&ws, &resolved);
    let parsed = Parsed::of(resolved.model);
    let mut found: Vec<(String, TimeConstantEvidence, f64)> =
        time_constants(resolved.model, &graph, &units, &parsed)
            .0
            .into_iter()
            .map(|(name, tc)| (name, tc.evidence, tc.stages))
            .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn logistic() -> TestProject {
    TestProject::new("logistic")
        .with_sim_time(0.0, 40.0, 0.125)
        .with_time_units("year")
        .stock("population", "1", &["births"], &[], None)
        .flow("births", "population * fractional_birth_rate", None)
        .aux(
            "fractional_birth_rate",
            "max_rate * (1 - population / capacity)",
            None,
        )
        .aux("max_rate", "0.5", None)
        .aux("capacity", "100", None)
}

/// A workforce whose members leave after a tenure, with a budget split per
/// head: at a tenure of DT the workforce empties in one step, and the split
/// divides by zero.
fn workforce() -> TestProject {
    TestProject::new("workforce")
        .with_sim_time(0.0, 10.0, 0.25)
        .with_time_units("Months")
        .stock("people", "100", &[], &["leaving"], None)
        .flow("leaving", "people / tenure", None)
        .aux("tenure", "5", None)
        .aux("per_head", "budget / head_count", None)
        .aux("budget", "10", None)
        .aux("head_count", "people / 2", None)
}

#[test]
fn a_time_constant_is_known_by_its_units_or_its_role() {
    let table = crate::datamodel::GraphicalFunction {
        kind: crate::datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0]),
        y_points: vec![0.0, 1.0],
        x_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    let project = TestProject::new("roles")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_time_units("Months")
        .stock(
            "level",
            "0",
            &["adjustment", "births"],
            &["attrition"],
            None,
        )
        .flow("adjustment", "MAX(0, correction + inflow_base)", None)
        .aux("correction", "(goal - level) / adjustment_time", None)
        .aux("inflow_base", "1", None)
        .flow("attrition", "level / average_stay * effect", None)
        .aux("perceived", "SMTH1(level, perception_time)", None)
        .aux("expected", "SMTH3(level, expectation_time)", None)
        .aux("expectation_time", "6", None)
        .aux("stated", "7", Some("month"))
        .aux("goal", "10", None)
        .aux("adjustment_time", "3", None)
        .aux("average_stay", "12", None)
        .aux("perception_time", "2", None)
        .flow("births", "level * 0.1 * (1 - level / capacity)", None)
        .aux("capacity", "50", None)
        .aux_with_gf("effect", "level / normal_level", table)
        .aux("normal_level", "10", None);
    let found = time_constants_of(&project);
    let found: Vec<(&str, TimeConstantEvidence, f64)> =
        found.iter().map(|(n, e, s)| (n.as_str(), *e, *s)).collect();
    use TimeConstantEvidence::*;
    assert_eq!(
        found,
        [
            ("adjustment_time", DividesARate, 1.0),
            ("average_stay", DividesARate, 1.0),
            ("expectation_time", DelayTime, 3.0),
            ("perception_time", DelayTime, 1.0),
            ("stated", TimeUnits, 1.0)
        ],
        "a divisor in what a rate adds up or a factor of it, a smooth's time (three DTs for a \
         third-order smooth's), and the model's time units; not a divisor within a sum that is \
         a factor (capacity) or a lookup's input (normal_level)"
    );

    // A reciprocal that multiplies nothing a stock moves is no time
    // constant: a fixed inflow of one a spacing.
    let project = TestProject::new("arrivals")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("queue", "0", &["arriving"], &[], None)
        .flow("arriving", "10 * (1 / spacing)", None)
        .aux("spacing", "4", None);
    assert_eq!(time_constants_of(&project), []);

    // A flow that is the division itself.
    let project = TestProject::new("drain")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "10", &[], &["draining"], None)
        .flow("draining", "level / residence_time", None)
        .aux("residence_time", "4", None);
    assert_eq!(
        time_constants_of(&project),
        [(
            "residence_time".to_string(),
            TimeConstantEvidence::DividesARate,
            1.0
        )]
    );
}

/// A time constant is known by its units where it has them, in any unit of
/// time, and a constant whose units are not time is a scale whatever its
/// role: a divisor in the units of what it divides.
#[test]
fn a_constant_with_units_is_a_time_constant_only_in_units_of_time() {
    let project = TestProject::new("tank")
        .with_sim_time(0.0, 10.0, 0.25)
        .with_time_units("Months")
        .stock("water", "10", &[], &["draining"], Some("liters"))
        .flow("draining", "water / residence_time", Some("liters/Months"))
        .aux("residence_time", "2", Some("weeks"))
        .stock("salt", "5", &[], &["settling"], Some("kg"))
        .flow("settling", "salt / capacity", None)
        .aux("capacity", "50", Some("kg"));
    assert_eq!(
        time_constants_of(&project),
        [(
            "residence_time".to_string(),
            TimeConstantEvidence::TimeUnits,
            1.0
        )],
        "weeks are time in a model of months; a capacity in kilograms is a scale"
    );
}

/// The forms a modeler writes a time constant in without units, each tried
/// at a short time, where zero would divide by it: the textbook one; the same
/// through algebra that only spells it differently (a product divisor, times
/// one over it, a power of -1, a division by one), which is read as the
/// division it is; and a fractional rate of one over a lifetime as its own
/// auxiliary.
#[test]
fn a_time_constant_without_units_is_known_in_the_forms_it_is_written_in() {
    use TimeConstantEvidence::*;
    for (equation, fraction, evidence) in [
        ("population / average_lifetime", None, DividesARate),
        ("population / (average_lifetime * 1)", None, DividesARate),
        ("population * (1 / average_lifetime)", None, DividesARate),
        ("1 / average_lifetime * population", None, DividesARate),
        ("population * average_lifetime ^ -1", None, DividesARate),
        ("population / average_lifetime / 1", None, DividesARate),
        (
            "population * fractional_death_rate",
            Some("1 / average_lifetime"),
            ReciprocalInARate,
        ),
    ] {
        let mut project = TestProject::new("deaths")
            .with_sim_time(0.0, 20.0, 0.25)
            .with_time_units("years")
            .stock("population", "100", &["births"], &["deaths"], None)
            .flow("births", "10", None)
            .flow("deaths", equation, None)
            .aux("average_lifetime", "10", None);
        if let Some(fraction) = fraction {
            project = project.aux("fractional_death_rate", fraction, None);
        }
        assert_eq!(
            time_constants_of(&project),
            [("average_lifetime".to_string(), evidence, 1.0)],
            "{equation}"
        );
        assert_eq!(
            extremes_of(&project, "average_lifetime"),
            [
                (ExtremeRule::ShortTime, 1.0),
                (ExtremeRule::TenTimes, 100.0)
            ],
            "{equation}: four DTs, which is a tenth of it too, and ten times"
        );
        let mut host = Host::from_test_project(&project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["extreme_conditions"], "targets": ["average_lifetime"]}),
        );
        assert_eq!(
            summary(&output, "extreme_conditions"),
            &json!({"test": "extreme_conditions", "checks": 3, "passed": 3}),
            "{equation}: the model's own run and both extremes, no division by zero"
        );
    }
}

/// Logistic growth spelled with the capacity dividing the whole term: the
/// capacity is in its own numerator's sum, so it is a scale, not a time
/// constant. Zero divides by it, so its low extreme is a tenth of it; the
/// call's own extreme of zero shows the division.
#[test]
fn a_divisor_in_its_own_numerators_sum_is_a_scale() {
    let project = TestProject::new("logistic")
        .with_sim_time(0.0, 40.0, 0.125)
        .with_time_units("year")
        .stock("population", "1", &["births"], &[], None)
        .flow(
            "births",
            "max_rate * population * (capacity - population) / capacity",
            None,
        )
        .aux("max_rate", "0.5", None)
        .aux("capacity", "100", None);
    assert_eq!(time_constants_of(&project), []);
    assert_eq!(
        extremes_of(&project, "capacity"),
        [(ExtremeRule::Tenth, 10.0), (ExtremeRule::TenTimes, 1000.0)]
    );
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({
            "tests": ["extreme_conditions"],
            "extremes": [{"variable": "capacity", "low": 0}]
        }),
    );
    let zero = result(&output, "extreme_conditions", "capacity", "low");
    assert_eq!(zero["extreme"], "given");
    assert_eq!(zero["value"], 0.0);
    assert_eq!(zero["outcome"], "failed");
    assert_eq!(zero["problems"][0]["kind"], "non_finite");
}

/// A table's equation is its input, not a rate: a divisor there is a scale,
/// read through a flow with a table or through a table a rate multiplies by.
#[test]
fn a_divisor_in_a_tables_input_is_a_scale() {
    let table = || crate::datamodel::GraphicalFunction {
        kind: crate::datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 2.0]),
        y_points: vec![0.0, 1.0],
        x_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
        y_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    let mut project = TestProject::new("tables")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "10", &[], &["outflow", "attrition"], None)
        .aux("normal_level", "10", None)
        .aux("reference_level", "10", None)
        .flow("attrition", "level * effect", None)
        .aux_with_gf("effect", "1 / reference_level", table());
    project = project.flow("outflow", "level / normal_level", None);
    let mut datamodel = project.build_datamodel();
    if let Some(Variable::Flow(flow)) = datamodel.models[0].get_variable_mut("outflow") {
        flow.gf = Some(table());
    }
    let host = Host::new(datamodel);
    let project = TestProject::from_datamodel(host.project.clone());
    assert_eq!(time_constants_of(&project), []);
}

/// A time constant's low extreme is a tenth of it and no less than four DTs
/// for each of its stages; one already there or below has no low extreme to
/// try. A division by it at zero, the call's own extreme, fails from the
/// run's first values.
#[test]
fn a_time_constant_is_tried_at_a_tenth_and_no_lower_than_four_dts_a_stage() {
    // Each row: the constant's value, the order of the smooth it is the
    // time of, and its low extreme with a DT of a quarter; `None` where the
    // constant is within four DTs a stage already.
    for (value, order, low) in [
        (100.0, 1, Some(10.0)),
        (5.0, 1, Some(1.0)),
        (1.0, 1, None),
        (0.6, 1, None),
        (100.0, 3, Some(10.0)),
        (6.0, 3, Some(3.0)),
        (3.0, 3, None),
    ] {
        let project = TestProject::new("smoothing")
            .with_sim_time(0.0, 10.0, 0.25)
            .stock("level", "100", &["filling"], &[], None)
            .flow("filling", "perceived / 50", None)
            .aux(
                "perceived",
                &format!("SMTH{order}(level, averaging_time)"),
                None,
            )
            .aux("averaging_time", &value.to_string(), None);
        let [(rule, at), _] = extremes_of(&project, "averaging_time");
        assert_eq!(rule, ExtremeRule::ShortTime, "{value}, order {order}");
        assert_eq!(at, low.unwrap_or(value), "{value}, order {order}");
        let mut host = Host::from_test_project(&project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["extreme_conditions"], "targets": ["averaging_time"]}),
        );
        // The model's own run, the high extreme, and the low one where
        // there is one.
        assert_eq!(
            summary(&output, "extreme_conditions")["checks"],
            2 + u64::from(low.is_some()),
            "{value}, order {order}: {output}"
        );
    }

    let mut host = Host::from_test_project(&workforce());
    let mut session = Session::new("main");
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions"]}),
    );
    assert_eq!(
        summary(&output, "extreme_conditions"),
        &json!({"test": "extreme_conditions", "checks": 3, "passed": 3}),
        "a tenure of four DTs, and of ten times, holds"
    );
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions"], "extremes": [{"variable": "tenure", "low": 0}]}),
    );
    let at_zero = result(&output, "extreme_conditions", "tenure", "low");
    assert_eq!(at_zero["outcome"], "failed");
    assert_eq!(at_zero["extreme"], "given");
    assert_eq!(
        at_zero.get("timeConstant"),
        None,
        "the call's extreme is no rule's"
    );
    assert_eq!(
        at_zero["problems"][0],
        json!({"kind": "non_finite", "variable": "leaving", "time": 0.0}),
    );
}

/// A constant some equation divides by has zero outside its domain: its low
/// extreme is a tenth of it. Any other constant's is zero.
#[test]
fn a_constant_an_equation_divides_by_is_tried_at_a_tenth_and_any_other_at_zero() {
    assert_eq!(
        extremes_of(&logistic(), "capacity"),
        [(ExtremeRule::Tenth, 10.0), (ExtremeRule::TenTimes, 1000.0)]
    );
    assert_eq!(
        extremes_of(&logistic(), "max_rate"),
        [(ExtremeRule::Zero, 0.0), (ExtremeRule::TenTimes, 5.0)]
    );
    let mut host = Host::from_test_project(&logistic());
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"]}),
    );
    assert_eq!(
        summary(&output, "extreme_conditions"),
        &json!({"test": "extreme_conditions", "checks": 5, "passed": 5}),
        "the model's own run, and each constant at both extremes"
    );
}

/// What an equation divides by: a factor of a divisor, through products,
/// quotients, powers, signs, IF branches, a sum over an array's elements,
/// and an auxiliary that is itself divided by; not a term of a sum, a
/// numerator, or the divisor of a guarded division.
#[test]
fn a_divisor_is_a_factor_of_what_an_equation_divides_by() {
    for (flow, scale, divisors) in [
        ("level / c", "1", vec!["c"]),
        ("level / (c * d)", "1", vec!["c", "d"]),
        ("level / (c / d)", "1", vec!["c", "d"]),
        ("level / (c + d)", "1", vec![]),
        ("level / -c", "1", vec!["c"]),
        ("level / c ^ 2", "1", vec!["c"]),
        ("level * c ^ -2", "1", vec!["c"]),
        ("level * c ^ 2 + d", "1", vec![]),
        ("level MOD c", "1", vec!["c"]),
        ("IF level > 1 THEN level / c ELSE d", "1", vec!["c"]),
        ("level / (IF level > 1 THEN c ELSE d)", "1", vec!["c", "d"]),
        ("SAFEDIV(level, c) + d", "1", vec![]),
        ("level / scale", "c * d", vec!["c", "d", "scale"]),
        ("level / scale", "c + d", vec!["scale"]),
        ("level / scale", "c / d", vec!["c", "d", "scale"]),
        ("level / scale", "c * (d + 1)", vec!["c", "scale"]),
        ("level * scale", "c * d", vec![]),
    ] {
        let project = TestProject::new("divisors")
            .with_sim_time(0.0, 2.0, 1.0)
            .stock("level", "10", &[], &["out"], None)
            .flow("out", flow, None)
            .aux("scale", scale, None)
            .aux("c", "2", None)
            .aux("d", "4", None);
        assert_eq!(
            constants_that(&project, |roles| roles.divisors.iter().cloned().collect()),
            divisors,
            "{flow}, with scale = {scale}"
        );
    }

    // A sum over an arrayed constant's elements is zero where they all are.
    let project = TestProject::new("shares")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_with_ranges("weight[region]", vec![("north", "1"), ("south", "3")])
        .stock("level", "10", &[], &["out"], None)
        .flow("out", "level / SUM(weight[*])", None);
    assert_eq!(
        constants_that(&project, |roles| roles.divisors.iter().cloned().collect()),
        ["weight"]
    );
    assert_eq!(low_rule(&project, "weight"), ExtremeRule::Tenth);
}

/// The one listed check of the model's own run.
fn own_run(output: &Value) -> Option<&Value> {
    output["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["condition"] == "own_run")
}

/// Shipments that ignore what is on hand: a stock of 10 drained by `orders`
/// a step, whatever it holds.
fn shipping(non_negative: bool, orders: &str) -> TestProject {
    TestProject::new("shipping")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock_with_options(
            "stock",
            "10",
            &[],
            &["shipments"],
            None,
            "",
            non_negative,
            false,
            Visibility::Private,
            None,
        )
        .flow("shipments", "orders", None)
        .aux("orders", orders, None)
        .stock("backlog", "-5", &["more"], &[], None)
        .flow("more", "orders", None)
}

/// Under an extreme, a stock that is never below zero in the model's own run
/// and goes below zero is flagged, saying when and how far, and no cause; a
/// stock the model marks non-negative says the marking is one this engine
/// does not enforce. A stock already below zero in the model's own run is not
/// judged.
#[test]
fn a_stock_that_goes_below_zero_under_an_extreme_is_flagged_saying_only_what_happened() {
    for non_negative in [false, true] {
        let mut host = Host::from_test_project(&shipping(non_negative, "2"));
        let mut session = Session::new("main");
        let output = run(
            &mut host,
            &mut session,
            json!({"tests": ["extreme_conditions"], "targets": ["orders"]}),
        );
        let check = result(&output, "extreme_conditions", "orders", "high");
        assert_eq!(check["outcome"], "flagged", "{check}");
        assert_eq!(check["extreme"], "ten_times", "{check}");
        let mut problem =
            json!({"kind": "goes_negative", "variable": "stock", "time": 1.0, "value": -70.0});
        if non_negative {
            problem["nonNegative"] = json!(true);
            assert!(
                check["note"].as_str().unwrap().contains("does not enforce"),
                "{check}"
            );
        } else {
            assert_eq!(check.get("note"), None, "no cause is stated: {check}");
        }
        assert_eq!(
            check["problems"],
            json!([problem]),
            "the backlog, negative in the model's run already, is not a problem"
        );
        assert_eq!(
            own_run(&output),
            None,
            "the model's own run holds: {output}"
        );
    }
}

/// A first-order drain at ten times its rate empties its stock in less than
/// a step of the model's DT, and Euler takes it below zero: the
/// integration's doing, which the same check at a tenth of the DT does not
/// show. It passes, and the test says how many did so.
#[test]
fn what_a_check_finds_only_at_the_models_dt_is_no_finding() {
    let project = TestProject::new("drain")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("level", "100", &[], &["draining"], None)
        .flow("draining", "level * rate", None)
        .aux("rate", "0.5", None);
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"]}),
    );
    let tested = summary(&output, "extreme_conditions");
    assert_eq!(tested["checks"], 3, "{output}");
    assert_eq!(tested["passed"], 3, "{output}");
    let note = tested["note"]
        .as_str()
        .unwrap_or_else(|| panic!("{output}"));
    assert!(
        note.starts_with("1 check found something at the model's DT and nothing at a tenth"),
        "{note}"
    );

    // An outflow that does not depend on its stock drains it at any DT.
    let mut host = Host::from_test_project(&shipping(false, "2"));
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"], "targets": ["orders"]}),
    );
    assert_eq!(
        summary(&output, "extreme_conditions")["note"],
        Value::Null,
        "{output}"
    );
}

/// Going below zero is judged element by element, for every stock and for a
/// flow the model marks non-negative, wherever the model's own run keeps it
/// at or above zero: an unmarked flow, and a stock with an element below
/// zero in the model's own run, are quantities with a sign.
#[test]
fn going_below_zero_is_judged_element_by_element_for_stocks_and_marked_flows() {
    let high = |project: &TestProject, target: &str| {
        let mut host = Host::from_test_project(project);
        run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["extreme_conditions"], "targets": [target]}),
        )
    };
    // Each region's balance is judged on its own: at ten times the outlay
    // the small one goes below zero and the large one does not. Where one
    // region is below zero in the model's own run, the balance is a quantity
    // with a sign, and neither is judged.
    for (north, flagged) in [("100", true), ("-5", false)] {
        let regions = TestProject::new("regions")
            .with_sim_time(0.0, 4.0, 1.0)
            .named_dimension("region", &["north", "south"])
            .array_with_ranges("opening[region]", vec![("north", north), ("south", "10")])
            .array_stock("balance[region]", "opening", &[], &["spending"], None)
            .array_flow("spending[region]", "outlay", None)
            .aux("outlay", "2", None);
        let output = high(&regions, "outlay");
        if flagged {
            let check = result(&output, "extreme_conditions", "outlay", "high");
            assert_eq!(
                check["problems"],
                json!([{"kind": "goes_negative", "variable": "balance[south]", "time": 1.0, "value": -70.0}]),
                "{output}"
            );
        } else {
            let tested = summary(&output, "extreme_conditions");
            assert_eq!(tested["passed"], tested["checks"], "{output}");
        }
    }

    // The same flow, draining a stock it does not depend on: the stock is
    // judged; the flow below zero is judged only where the model marks it.
    for marked in [false, true] {
        let mut project = TestProject::new("drained")
            .with_sim_time(0.0, 4.0, 1.0)
            .stock("tank", "1000", &["net"], &["use"], None)
            .flow("net", "supply - demand", None)
            .flow("use", "1", None)
            .aux("supply", "3", None)
            .aux("demand", "2", None)
            .build_datamodel();
        if let Some(Variable::Flow(flow)) = project.models[0].get_variable_mut("net") {
            flow.compat.non_negative = marked;
        }
        let mut host = Host::new(project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["extreme_conditions"], "targets": ["demand"]}),
        );
        let tested = summary(&output, "extreme_conditions");
        if marked {
            let check = result(&output, "extreme_conditions", "demand", "high");
            assert_eq!(check["problems"][0]["variable"], "net", "{check}");
        } else {
            assert_eq!(tested["passed"], tested["checks"], "{output}");
        }
    }
}

/// The model's own run is read with nothing changed: a stock or flow the
/// model marks non-negative that is below zero there is flagged, saying the
/// marking is one this engine does not enforce. An unmarked stock is not
/// judged in its own run, where negative may be a value of its: a stock four
/// a step drains below zero, a temperature relaxing toward a freezer at -18,
/// and a drain Euler overshoots at a time constant under one DT (which the
/// integration check names) all pass it.
#[test]
fn the_models_own_run_is_read_for_what_it_marks_non_negative() {
    let own = |project: datamodel::Project| {
        let mut host = Host::new(project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["extreme_conditions"], "targets": []}),
        );
        own_run(&output).cloned()
    };
    let celsius = TestProject::new("celsius")
        .with_sim_time(0.0, 60.0, 0.25)
        .stock("temperature", "20", &[], &["cooling"], None)
        .flow("cooling", "(temperature - freezer) / tau", None)
        .aux("freezer", "-18", None)
        .aux("tau", "10", None);
    let overshoot = TestProject::new("overshoot")
        .with_sim_time(0.0, 20.0, 1.0)
        .stock("level", "100", &[], &["draining"], None)
        .flow("draining", "level / tau", None)
        .aux("tau", "0.8", None);
    for (what, project) in [
        ("drained by four a step", shipping(false, "4")),
        ("relaxing toward -18", celsius),
        ("overshot by Euler", overshoot),
    ] {
        assert_eq!(own(project.build_datamodel()), None, "{what}");
    }

    // Marked, a stock drained below zero is flagged.
    let check = own(shipping(true, "4").build_datamodel()).expect("flagged");
    assert_eq!(check["outcome"], "flagged");
    assert_eq!(check.get("variable"), None);
    assert_eq!(
        check["problems"],
        json!([{"kind": "goes_negative", "variable": "stock", "time": 3.0, "value": -6.0, "nonNegative": true}])
    );
    assert_eq!(
        check["note"],
        "stock is marked non-negative, which this engine does not enforce: a tool that \
         enforces the marking would hold it at zero."
    );

    // A marked flow below zero as the model stands.
    let mut project = TestProject::new("marked")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("tank", "100", &["net"], &[], None)
        .flow("net", "supply - demand", None)
        .aux("supply", "2", None)
        .aux("demand", "3", None)
        .build_datamodel();
    if let Some(Variable::Flow(flow)) = project.models[0].get_variable_mut("net") {
        flow.compat.non_negative = true;
    }
    let check = own(project.clone()).expect("flagged");
    assert_eq!(
        check["problems"],
        json!([{"kind": "goes_negative", "variable": "net", "time": 0.0, "value": -1.0, "nonNegative": true}])
    );
    // Said there, it is judged by no check: ten times the demand drains the
    // tank, and `net` is not named again.
    let output = run(
        &mut Host::new(project),
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"], "targets": ["demand"]}),
    );
    assert_eq!(
        result(&output, "extreme_conditions", "demand", "high")["problems"],
        json!([{"kind": "goes_negative", "variable": "tank", "time": 4.0, "value": -12.0}]),
        "{output}"
    );

    // A marked flow whose arithmetic reaches zero leaves residue below it
    // (0.3 - 0.1 * 3 is -5.6e-17): residue of its own magnitude, though its
    // equation, a product, says no scale of its own.
    let mut project = TestProject::new("closing")
        .with_sim_time(0.0, 3.0, 1.0)
        .stock("tank", "10", &[], &["closing"], None)
        .flow("closing", "(0.3 - 0.1 * TIME) * 2", None)
        .build_datamodel();
    if let Some(Variable::Flow(flow)) = project.models[0].get_variable_mut("closing") {
        flow.compat.non_negative = true;
    }
    assert_eq!(own(project), None);
}

/// Below zero is below it by more than the residue floating point leaves at
/// the series' scale (`behavior::residue_bound`), and at a scale of zero by
/// anything: the bound decides, the value exactly at it is residue, and a
/// model whose quantities are all tiny goes below zero as it moves.
#[test]
fn residue_below_zero_is_not_negative() {
    let bound = residue_bound(100.0);
    for (values, scale, row) in [
        // A drain from a hundred that leaves a part in 1e17.
        (vec![100.0, 40.0, -1e-15, 0.0], 100.0, None),
        (vec![100.0, 40.0, -1e-15, 0.0], 0.0, Some(2)),
        (vec![100.0, -bound], 100.0, None),
        (vec![100.0, -bound * 1.01], 100.0, Some(1)),
        (vec![100.0, 40.0, -2e-7, 5.0], 100.0, Some(2)),
        // A small model a thousandth of its magnitude below zero.
        (vec![1e-6, 5e-7, -1e-9], 1e-6, Some(2)),
        (vec![0.0, -0.0, 0.0], 0.0, None),
        (vec![3.0, 2.0, -1.0, -5.0], 3.0, Some(2)),
    ] {
        assert_eq!(goes_negative(&values, scale), row, "{values:?} at {scale}");
    }
}

/// Whether a series went below zero is one rule wherever an agent reads it:
/// a summary says it went negative exactly when a `goes_negative` citation of
/// it holds, residue included, since both report the numbers the run holds.
/// The battery judges a stock its model marks non-negative for passing zero
/// in its own run only beyond the residue of its scale, so it flags what the
/// summary shows going negative and never what it shows staying at or above
/// zero; residue below zero alone it leaves be.
#[test]
fn a_summary_a_citation_and_the_battery_agree_on_going_below_zero() {
    // Each row: what drains a stock of 0.3 a step, whatever it holds, and
    // whether its summary, a citation, and the battery's own run say it went
    // below zero. Three steps of 0.1 leave -2.8e-17, which is residue.
    for (draining, below_zero, flagged) in [
        ("IF TIME < 3 THEN 0.1 ELSE 0", true, false),
        ("IF TIME < 3 THEN 0.11 ELSE 0", true, true),
        ("IF TIME < 3 THEN 0.09 ELSE 0", false, false),
    ] {
        let project = TestProject::new("drain")
            .with_sim_time(0.0, 6.0, 1.0)
            .stock_with_options(
                "tank",
                "0.3",
                &[],
                &["draining"],
                None,
                "",
                true,
                false,
                Visibility::Private,
                None,
            )
            .flow("draining", draining, None);
        let mut host = Host::from_test_project(&project);
        let mut session = Session::new("main");
        let summary = host.call(
            &mut session,
            "read_behavior",
            json!({"variables": ["tank"]}),
        );
        assert_eq!(
            summary["series"][0].get("negativeFrom").is_some(),
            below_zero,
            "{draining}: {summary}"
        );
        let verdict = host.call(
            &mut session,
            "verify_findings",
            json!({"findings": [{
                "kind": "observation",
                "claim": "the tank goes below zero",
                "citations": [{"cites": "goes_negative", "variable": "tank"}]
            }]}),
        );
        assert_eq!(
            verdict["findings"][0]["holds"], below_zero,
            "{draining}: {verdict}"
        );
        let output = run(
            &mut host,
            &mut session,
            json!({"tests": ["extreme_conditions"], "targets": []}),
        );
        assert_eq!(own_run(&output).is_some(), flagged, "{draining}: {output}");
        assert!(
            !flagged || below_zero,
            "the battery flags only what went below zero"
        );
    }
}

/// The one listed integration error check.
fn integration(output: &Value) -> &Value {
    output["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["test"] == "integration_error")
        .unwrap_or_else(|| panic!("the integration error check is listed: {output}"))
}

/// Integration error passes under INTEGRATION_TOLERANCE of a stock's scale,
/// is flagged for the modeler to weigh up to INTEGRATION_FAILURE, and fails
/// above it: exponential decay under Euler at a DT of 0.125, at time
/// constants that put its estimated error just either side of each threshold
/// (0.997% and 1.0001%; 4.990% and 5.021%). One measurement, one check, and a
/// straight line integrates exactly.
#[test]
fn integration_error_passes_is_flagged_or_fails_by_the_size_of_its_estimate() {
    for (tau, outcome) in [
        ("2.345", "passed"),
        ("2.338", "flagged"),
        ("0.503", "flagged"),
        ("0.5", "failed"),
    ] {
        let project = TestProject::new("decay")
            .with_sim_time(0.0, 30.0, 0.125)
            .stock("level", "100", &[], &["draining"], None)
            .flow("draining", &format!("level / {tau}"), None);
        let mut host = Host::from_test_project(&project);
        let mut session = Session::new("main");
        session.outline_budget = usize::MAX;
        let output = run(
            &mut host,
            &mut session,
            json!({"tests": ["integration_error"]}),
        );
        let tested = summary(&output, "integration_error");
        assert_eq!(tested["checks"], 1, "{output}");
        assert_eq!(tested[outcome], 1, "tau {tau}: {output}");
        if outcome != "passed" {
            let check = integration(&output);
            let estimate = check["differences"][0]["difference"].as_f64().unwrap();
            let order = check["order"].as_f64().unwrap();
            assert!(
                (order - 1.0).abs() < 0.2,
                "Euler converges at order one: {order}"
            );
            assert_eq!(
                estimate > INTEGRATION_FAILURE,
                outcome == "failed",
                "tau {tau}: {estimate}"
            );
        }
    }

    // A straight line integrates exactly, under any method.
    let line = TestProject::new("line")
        .with_sim_time(0.0, 10.0, 0.5)
        .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
        .stock("tank", "0", &["filling"], &[], None)
        .flow("filling", "3", None);
    let mut host = Host::from_test_project(&line);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    assert_eq!(
        summary(&output, "integration_error"),
        &json!({"test": "integration_error", "checks": 1, "passed": 1})
    );
    assert_eq!(output["results"], json!([]));
}

/// The estimate is Richardson's: for exponential decay, whose answer is
/// known, the error the check reports is the run's true error, and the order
/// it reports is about the method's, under each method (each at a DT coarse
/// enough for its error to show).
#[test]
fn the_estimated_error_is_the_runs_true_error_under_each_method() {
    use crate::datamodel::SimMethod;
    for method in [
        SimMethod::Euler,
        SimMethod::RungeKutta2,
        SimMethod::RungeKutta4,
    ] {
        // Per-variant facts: the order each method converges at, and a DT at
        // which a time constant of 2 shows its error.
        let (nominal, results_method, dt) = match method {
            SimMethod::Euler => (1.0, crate::results::Method::Euler, 0.5),
            SimMethod::RungeKutta2 => (2.0, crate::results::Method::RungeKutta2, 1.0),
            SimMethod::RungeKutta4 => (4.0, crate::results::Method::RungeKutta4, 3.0),
        };
        assert_eq!(nominal_order(results_method), nominal);
        let project = TestProject::new("decay")
            .with_sim_time(0.0, 30.0, dt)
            .with_sim_method(method)
            .stock("level", "100", &[], &["draining"], None)
            .flow("draining", "level / 2", None);
        // The run's true error: its largest distance from 100 e^(-t/2), as a
        // fraction of the series' scale.
        let truth = read_as_the_battery(project.build_datamodel(), |model, _, base| {
            let (series, _) = element_series_upto(base, model, "level", None, usize::MAX);
            base.times()
                .iter()
                .zip(&series[0].1)
                .map(|(t, v)| (v - 100.0 * (-t / 2.0).exp()).abs() / 100.0)
                .fold(0.0_f64, f64::max)
        });
        assert!(truth > INTEGRATION_TOLERANCE, "{method:?}: {truth}");
        let mut host = Host::from_test_project(&project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["integration_error"]}),
        );
        let check = integration(&output);
        let estimate = check["differences"][0]["difference"].as_f64().unwrap();
        assert_eq!(
            check["outcome"],
            if estimate > INTEGRATION_FAILURE {
                "failed"
            } else {
                "flagged"
            },
            "{method:?}"
        );
        assert!(
            (estimate / truth - 1.0).abs() < 0.1,
            "{method:?}: estimated {estimate}, true {truth}"
        );
        let order = check["order"].as_f64().unwrap();
        assert!(
            (order - nominal).abs() < 1.0,
            "{method:?}: order {order}, nominally {nominal}"
        );
    }
}

/// With a save step off the DT grid the model's rows are the first steps at
/// or after their save times (DT 0.6, save step 0.7: rows at 0, 1.2, 1.8,
/// ...), which a finer run saving at 0.7 would put at other times (0.9, 1.5,
/// ...). The finer runs are read at the model's own row times, so the check
/// estimates the run's true error and not its movement between two times:
/// for exponential decay, whose answer is known, the estimate is the true
/// error at the model's saved rows.
#[test]
fn integration_error_off_the_dt_grid_compares_the_model_s_own_row_times() {
    let project = TestProject::new("decay")
        .with_sim_time(0.0, 30.0, 0.6)
        .with_save_step(0.7)
        .stock("level", "100", &[], &["draining"], None)
        .flow("draining", "level / 2", None);
    let (truth, times) = read_as_the_battery(project.build_datamodel(), |model, _, base| {
        let (series, _) = element_series_upto(base, model, "level", None, usize::MAX);
        let times = base.times();
        let truth = times
            .iter()
            .zip(&series[0].1)
            .map(|(t, v)| (v - 100.0 * (-t / 2.0).exp()).abs() / 100.0)
            .fold(0.0_f64, f64::max);
        (truth, times)
    });
    assert!(
        (times[1] - 1.2).abs() < 1e-9,
        "the premise: the second row is at the step after 0.7: {times:?}"
    );
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    let check = integration(&output);
    let estimate = check["differences"][0]["difference"].as_f64().unwrap();
    assert!(
        (estimate / truth - 1.0).abs() < 0.1,
        "estimated {estimate}, true {truth}: {check}"
    );
    let order = check["order"].as_f64().unwrap();
    assert!(
        (order - 1.0).abs() < 0.2,
        "Euler converges at order one: {order}"
    );
}

/// Where the runs at finer DTs do not converge there is no error to
/// estimate: a model whose equations read DT is a model of each DT, and a
/// chaotic one has no trajectory to converge on. The check is flagged saying
/// which, not failed. A model that reads DT and converges all the same is
/// judged like any other.
#[test]
fn a_model_whose_finer_runs_do_not_converge_is_flagged_saying_why() {
    // A stock refilled to its goal in exactly one step, whatever the step.
    let discrete = TestProject::new("discrete")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("count", "0", &["counting"], &[], None)
        .flow("counting", "pick / DT", None)
        .aux("pick", "IF TIME MOD 2 < 1 THEN 1 ELSE 0", None);
    let mut host = Host::from_test_project(&discrete);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    let check = integration(&output);
    assert_eq!(check["outcome"], "flagged", "{check}");
    let note = check["note"].as_str().unwrap();
    assert!(
        note.starts_with("The runs at finer DTs do not converge, and counting reads DT"),
        "{note}"
    );

    // An equation that reads DT moves the run 0.5% at half the DT and 5%
    // at a quarter: the first refinement is under the tolerance, and the
    // runs still do not converge.
    let stepped = TestProject::new("stepped")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", "0", &["filling"], &[], None)
        .flow(
            "filling",
            "IF DT < 0.2 THEN 1.055 ELSE IF DT < 0.3 THEN 1.005 ELSE 1",
            None,
        );
    let mut host = Host::from_test_project(&stepped);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    let check = integration(&output);
    assert_eq!(check["outcome"], "flagged", "{output}");
    assert!(
        check["note"]
            .as_str()
            .is_some_and(|note| note.starts_with("The runs at finer DTs do not converge")),
        "{check}"
    );

    // The Lorenz system: the runs part ways whatever the DT.
    let lorenz = TestProject::new("lorenz")
        .with_sim_time(0.0, 30.0, 0.01)
        .stock("x", "1", &["dx"], &[], None)
        .stock("y", "1", &["dy"], &[], None)
        .stock("z", "1", &["dz"], &[], None)
        .flow("dx", "10 * (y - x)", None)
        .flow("dy", "x * (28 - z) - y", None)
        .flow("dz", "x * y - 8 / 3 * z", None);
    let mut host = Host::from_test_project(&lorenz);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    let check = integration(&output);
    assert_eq!(check["outcome"], "flagged", "{check}");
    assert_eq!(
        check.get("order"),
        None,
        "runs that do not converge have no order of convergence: {check}"
    );
    let note = check["note"].as_str().unwrap();
    assert!(note.contains("chaotic"), "{note}");
    assert!(!note.contains("read"), "nothing reads DT: {note}");

    // A drain limited to what the stock holds in a step reads DT, and
    // converges: an error of integration like any other.
    let limited = TestProject::new("limited")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", "100", &[], &["draining"], None)
        .flow("draining", "MIN(level / DT, level / 2)", None);
    let mut host = Host::from_test_project(&limited);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    let check = integration(&output);
    assert_eq!(check["outcome"], "failed", "{check}");
    assert_eq!(check.get("note"), None, "{check}");
}

/// A failed check names the time constant DT is too large for: the one with
/// the fewest DTs to each of its stages, when that is fewer than four, in
/// the model's own unit of time and no unit conversion.
#[test]
fn a_failed_integration_names_the_time_constant_dt_is_too_large_for() {
    let project = TestProject::new("water")
        .with_sim_time(0.0, 20.0, 1.0)
        .with_time_units("month")
        .stock("water", "0", &["filling"], &[], None)
        .flow("filling", "(target - water) / adjustment_time", None)
        .aux("target", "1", None)
        .aux("adjustment_time", "2", None)
        .aux("perceived", "SMTH3(water, perception_time)", None)
        .aux("perception_time", "9", None)
        .aux("review_time", "30", Some("days"))
        .aux("one_month", "1", Some("month"))
        // A ratio in months that sets no stock's pace: a capital-output
        // ratio divides a stock into an output, which is no rate.
        .aux("output", "water / output_ratio", None)
        .aux("output_ratio", "1", Some("month"));
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    let check = integration(&output);
    assert_eq!(check["outcome"], "failed", "{check}");
    assert_eq!(
        check["note"],
        "The shortest time constant is adjustment_time = 2, 2 DT: DT should be at most a \
         quarter of it.",
        "not the smooth's time of three DTs a stage, the review time in days, the output \
         ratio, which paces no stock, or the conversion"
    );

    // With the adjustment at four DTs, the third-order smooth's time, three
    // DTs to each of its stages, is the shortest.
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("adjustment_time")
            .unwrap()
            .set_scalar_equation("4")
    });
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["integration_error"]}),
    );
    let check = integration(&output);
    assert_eq!(
        check["note"],
        "The shortest time constant is perception_time = 9, 3 DT for each of its 3 stages: DT \
         should be at most a quarter of a stage."
    );

    // A residence time in weeks paces the stock it drains in a model of
    // months: 6 weeks is 1.38 months, 1.38 DT.
    let project = TestProject::new("tank")
        .with_sim_time(0.0, 20.0, 1.0)
        .with_time_units("month")
        .stock("water", "100", &[], &["draining"], None)
        .flow("draining", "water / residence_time", None)
        .aux("residence_time", "6", Some("weeks"));
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    let note = integration(&output)["note"].as_str().unwrap().to_string();
    assert_eq!(
        note,
        "The shortest time constant is residence_time = 6, 1.3799 DT: DT should be at most a \
         quarter of it."
    );

    // A time written into a rate's equation is named as one; a number an
    // equation that is no rate divides a stock by is none, nor one a rate
    // divides a constant by.
    let project = TestProject::new("backlog")
        .with_sim_time(0.0, 20.0, 1.0)
        .stock("backlog", "10", &["orders"], &["shipping"], None)
        .flow("orders", "demand / 0.1", None)
        .aux("demand", "0.5", None)
        .flow("shipping", "MIN(backlog / 0.5, capacity)", None)
        .aux("capacity", "20", None)
        .aux("per_crew", "backlog / 0.25", None);
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    assert_eq!(
        integration(&output)["note"],
        "The shortest time constant is the 0.5 shipping divides by, 0.5 DT: DT should be at \
         most a quarter of it."
    );
}

/// A row is read at a time on another grid within the clocks' rounding of it,
/// either side, and never at the next row.
#[test]
fn a_row_is_found_at_its_time_on_another_grid() {
    let saved = [0.0, 0.30000000000000004, 0.6, 0.8999999999999999, 1.2];
    let times = [0.0, 0.3, 0.6000000000000001, 0.9, 1.2];
    assert_eq!(rows_at(&saved, &times, 0.3), [0, 1, 2, 3, 4]);
    // A finer run's rows between the model's are passed over.
    let finer = [0.0, 0.15, 0.3, 0.45, 0.6];
    assert_eq!(rows_at(&finer, &[0.0, 0.3, 0.6], 0.3), [0, 2, 4]);
}

/// A finer run saves the model's own times among its rows, so it is read at
/// them: exactly the model's rows for a save step that is a whole number of
/// DTs, and every step of the model's grid for one that is not, whose rows
/// the finer grid would otherwise save a fraction of a DT early.
#[test]
fn a_finer_run_saves_the_models_own_times() {
    for (dt, save_step, saved_every, as_many_rows) in [
        (0.25, 0.25, 0.25, true),
        (0.25, 1.0, 1.0, true),
        (1.0 / 128.0, 0.1, 1.0 / 128.0, false),
        // A save step under DT saves every step.
        (0.5, 0.1, 0.5, true),
    ] {
        let project = TestProject::new("saved")
            .with_sim_time(0.0, 4.0, dt)
            .with_save_step(save_step)
            .stock("level", "100", &[], &["draining"], None)
            .flow("draining", "level / 2", None);
        let mut host = Host::from_test_project(&project);
        let model = host.project.models[0].clone();
        let base = Session::new("main")
            .runs
            .current(&mut host.workspace(), &model)
            .ok()
            .unwrap();
        let base_times = base.times();
        for divisor in [2.0, 4.0, 10.0] {
            let finer = finer_specs(&base.results.specs, divisor);
            assert_eq!(finer.dt, Some(dt / divisor), "{dt} {save_step}");
            assert_eq!(finer.save_step, Some(saved_every), "{dt} {save_step}");
            assert_eq!((finer.start, finer.stop, finer.method), (None, None, None));
            let plan = RunPlan {
                specs: finer,
                ..RunPlan::default()
            };
            let results = runs::execute(&mut host.workspace(), &model, &plan)
                .ok()
                .unwrap();
            let run = Run::new(String::new(), 0, 0, plan, results);
            let times = run.times();
            assert_eq!(
                times.len() == base_times.len(),
                as_many_rows,
                "{dt} {save_step}"
            );
            for (&t, row) in base_times.iter().zip(rows_at(&times, &base_times, dt)) {
                assert!(
                    (times[row] - t).abs() <= 1e-12 * t.abs().max(1.0),
                    "{dt} {save_step} /{divisor}: the model's row at {t} read at {}",
                    times[row]
                );
            }
        }
        // The battery's finer runs line up with the model's: its checks run.
        let mut host = Host::from_test_project(&project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["integration_error"]}),
        );
        assert_eq!(
            summary(&output, "integration_error")["checks"],
            1,
            "{output}"
        );
        assert_eq!(summary(&output, "integration_error").get("notRun"), None);
    }
}

/// A second-order goal seeker, critically damped: goal seeking at its
/// damping, and past its goal by a sixth and back at half of it.
fn damped() -> TestProject {
    TestProject::new("damped")
        .with_sim_time(0.0, 60.0, 0.0625)
        .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
        .stock("x", "0", &["dx"], &[], None)
        .stock("v", "0", &["dv"], &[], None)
        .flow("dx", "v", None)
        .flow("dv", "0.25 * (goal - x) - damping * v", None)
        .aux("goal", "100", None)
        .aux("damping", "1", None)
}

#[test]
fn sensitivity_flags_a_change_of_behavior_mode_and_lists_the_strongest_responses() {
    let mut host = Host::from_test_project(&damped());
    let mut session = Session::new("main");
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "record": ["x"]}),
    );
    let half = result(&output, "sensitivity", "damping", "half");
    assert_eq!(half["outcome"], "flagged");
    assert_eq!(half["value"], 0.5);
    assert_eq!(half["responses"][0]["variable"], "x");
    assert_eq!(half["responses"][0]["was"], "goal_seeking");
    assert_ne!(half["responses"][0]["mode"], "goal_seeking");
    assert_eq!(half["responses"][0]["wasFamily"], "rising");
    assert_eq!(half["responses"][0]["family"], "rises_then_falls");
    let double = result(&output, "sensitivity", "damping", "double");
    assert_eq!(
        double["outcome"], "passed",
        "slower, and still goal seeking"
    );
    assert_eq!(double["responses"][0].get("family"), None, "{double}");

    // The goal moves where x ends, and so is the strongest of the passes.
    let passed: Vec<&Value> = output["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["outcome"] == "passed")
        .collect();
    assert_eq!(passed[0]["variable"], "goal");
    assert_eq!(passed[0]["responses"][0]["change"], 1.0);
    assert_eq!(
        output["results"][0]["outcome"], "flagged",
        "flags before passes"
    );
}

#[test]
fn a_knockout_names_the_links_and_loops_it_cuts_and_how_behavior_changes() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let loops = host.call(&mut session, "analyze_loops", json!({}));
    let crowding = loops["partitions"][0]["loops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["polarity"] == "balancing")
        .unwrap()["id"]
        .clone();
    let output = run(
        &mut host,
        &mut session,
        json!({
            "tests": ["loop_knockout"],
            "targets": ["fractional_birth_rate", "population", "capacity"]
        }),
    );
    let held = result(&output, "loop_knockout", "fractional_birth_rate", "held");
    assert_eq!(held["outcome"], "observed");
    assert_eq!(held["value"], 0.495, "its value in the model's first step");
    assert_eq!(held["loops"], json!([crowding]));
    assert_eq!(held["cutLinks"].as_array().unwrap().len(), 3);
    assert_eq!(held["responses"][0]["mode"], "exponential");
    assert_eq!(held["responses"][0]["was"], "s_shaped");
    for (target, says) in [("population", "is a stock"), ("capacity", "is a constant")] {
        let check = result(&output, "loop_knockout", target, "held");
        assert_eq!(check["outcome"], "not_run");
        assert!(check["reason"].as_str().unwrap().contains(says), "{check}");
    }

    let output = run(&mut host, &mut session, json!({"tests": ["loop_knockout"]}));
    assert!(
        summary(&output, "loop_knockout")["skipped"]
            .as_str()
            .unwrap()
            .contains("targets")
    );
}

#[test]
fn a_disturbance_shows_a_model_at_rest_its_loops() {
    let at_rest = TestProject::new("at rest")
        .with_sim_time(0.0, 20.0, 0.25)
        .stock("level", "100", &["adjustment"], &[], None)
        .flow("adjustment", "(goal - level) / adjustment_time", None)
        .aux("goal", "100", None)
        .aux("adjustment_time", "4", None)
        .aux("idle", "0", None)
        .flow("unused", "idle", None);
    let mut host = Host::from_test_project(&at_rest);
    let mut session = Session::new("main");
    let output = run(&mut host, &mut session, json!({"tests": ["disturbance"]}));
    let step = result(&output, "disturbance", "goal", "step");
    assert_eq!(step["outcome"], "observed");
    assert_eq!(step["value"], 110.0);
    assert_eq!(step["fromTime"], 2.0, "a tenth of the way into the run");
    assert_eq!(step["loops"], json!(["L1"]));
    let response = &step["responses"][0];
    assert_eq!(
        (&response["variable"], &response["mode"], &response["was"]),
        (&json!("level"), &json!("goal_seeking"), &json!("at_rest"))
    );
    let change = response["change"].as_f64().unwrap();
    assert!(
        (0.095..0.1).contains(&change),
        "most of the way to a goal a tenth higher by the end: {change}"
    );
    assert_eq!(
        summary(&output, "disturbance")["checks"],
        2,
        "the time constant and the goal; a constant at zero is no default"
    );
    let loops = host.call(&mut session, "analyze_loops", json!({"loops": ["L1"]}));
    assert_eq!(loops["partitions"][0]["loops"][0]["polarity"], "balancing");

    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["disturbance"], "targets": ["idle"]}),
    );
    let idle = result(&output, "disturbance", "idle", "step");
    assert_eq!(idle["outcome"], "not_run");
    assert!(idle["reason"].as_str().unwrap().contains("is zero"));
}

#[test]
fn the_units_check_lists_the_units_diagnostics_by_the_ids_read_model_gives_them() {
    let project = TestProject::new("units")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_time_units("month")
        .stock("widgets", "0", &["making"], &[], Some("widget"))
        .flow("making", "rate", Some("widget/month"))
        .aux("rate", "3", Some("widget"));
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let outline = host.call(&mut session, "read_model", json!({}));
    let ids: Vec<&Value> = outline["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["category"].as_str().unwrap().starts_with("unit"))
        .map(|d| &d["id"])
        .collect();
    assert!(!ids.is_empty(), "the premise: {outline}");
    let output = run(&mut host, &mut session, json!({"tests": ["units"]}));
    let units = &output["results"][0];
    assert_eq!(units["test"], "units");
    assert_eq!(units["outcome"], "failed");
    let listed: Vec<&Value> = units["diagnostics"].as_array().unwrap().iter().collect();
    assert_eq!(listed, ids);

    let mut host = Host::from_test_project(&inventory());
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["units"]}),
    );
    assert_eq!(
        summary(&output, "units"),
        &json!({"test": "units", "checks": 1, "passed": 1})
    );
}

#[test]
fn a_check_keeps_its_id_when_run_again_after_an_edit() {
    let mut host = Host::from_test_project(&workforce());
    let mut session = Session::new("main");
    let first = run(&mut host, &mut session, json!({}));
    let ids = |output: &Value| -> Vec<(String, String)> {
        output["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    format!("{} {} {}", r["test"], r["variable"], r["condition"]),
                    r["id"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("budget")
            .unwrap()
            .set_scalar_equation("20")
    });
    let again = run(&mut host, &mut session, json!({}));
    assert_eq!(again["revision"], 1);
    let first_ids = ids(&first);
    for (check, id) in ids(&again) {
        if let Some((_, was)) = first_ids.iter().find(|(c, _)| *c == check) {
            assert_eq!(&id, was, "{check}");
        }
    }
}

#[test]
fn a_model_that_does_not_simulate_is_checked_for_units_and_skips_the_rest() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    let mut host = Host::new(project);
    let output = run(&mut host, &mut Session::new("main"), json!({}));
    let reason = output["runFails"]
        .as_str()
        .unwrap_or_else(|| panic!("{output}"));
    assert!(reason.contains("shipments"), "the reason, once: {reason}");
    // Said once, the answer stays small: the reason is not repeated per test.
    let bytes = output.to_string().len();
    assert!(bytes <= 1_500, "{bytes} bytes: {output}");
    for test in TestName::ALL {
        let summary = summary(
            &output,
            serde_json::to_value(test).unwrap().as_str().unwrap(),
        );
        if test == TestName::Units {
            assert_eq!(summary["checks"], 1);
        } else {
            assert_eq!(
                summary["skipped"], "the model does not simulate (runFails says why)",
                "{output}"
            );
        }
    }
}

#[test]
fn names_the_model_lacks_come_back_with_suggestions_and_limits_are_refused() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "targets": ["capacty"], "record": ["populaton"]}),
    );
    let not_found: Vec<&Value> = output["notFound"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| &n["suggestions"][0])
        .collect();
    assert_eq!(not_found, [&json!("capacity"), &json!("population")]);

    let many: Vec<String> = (0..=MAX_TARGETS).map(|i| format!("v{i}")).collect();
    let refusal = host.refuse(&mut session, "run_tests", json!({"targets": many}));
    assert!(
        refusal["error"].as_str().unwrap().contains("at most 12"),
        "{refusal}"
    );
    let refusal = host.refuse(&mut session, "run_tests", json!({"tests": ["everything"]}));
    assert!(refusal["error"].as_str().unwrap().contains("run_tests"));
}

/// A model with `n` constants, each of which the growth takes the logarithm
/// of: at zero, every one fails.
fn logarithms(n: usize) -> TestProject {
    let mut project = TestProject::new("many").with_sim_time(0.0, 2.0, 1.0).stock(
        "level",
        "1",
        &["growth"],
        &[],
        None,
    );
    let terms: Vec<String> = (0..n).map(|i| format!("LN(c{i})")).collect();
    project = project.flow("growth", &format!("0.001 * ({})", terms.join(" + ")), None);
    for i in 0..n {
        project = project.aux(&format!("c{i}"), "1", None);
    }
    project
}

#[test]
fn an_answer_lists_at_most_its_limit_failures_first() {
    let mut host = Host::from_test_project(&logarithms(25));
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"]}),
    );
    assert_eq!(summary(&output, "extreme_conditions")["failed"], 25);
    assert_eq!(
        summary(&output, "extreme_conditions").get("note"),
        None,
        "a value not a number from the run's first values is no matter of integration, and is \
         not run again: {output}"
    );
    assert_eq!(output["results"].as_array().unwrap().len(), MAX_RESULTS);
    assert_eq!(output["omitted"], 25 - MAX_RESULTS);
    assert!(
        output["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["outcome"] == "failed")
    );
}

/// The list is shared between the tests in turn: a test with more findings
/// than the list holds leaves room for every other test's, each test's
/// strongest first, and what is listed comes failures first.
#[test]
fn the_list_is_shared_between_the_tests_so_one_cannot_crowd_out_another() {
    let check = |test: TestName, outcome: Outcome, strength: f64| {
        let mut check = Check::new(test, None, None);
        check.result.outcome = outcome;
        check.strength = strength;
        check
    };
    // More than the list holds from each of two tests, a few from the rest,
    // and every outcome among them.
    let mut checks: Vec<Check> = Vec::new();
    for i in 0..30 {
        checks.push(check(TestName::ExtremeConditions, Outcome::Failed, 0.0));
        checks.push(check(TestName::Sensitivity, Outcome::Flagged, f64::from(i)));
    }
    for i in 0..8 {
        checks.push(check(TestName::Sensitivity, Outcome::Passed, f64::from(i)));
    }
    checks.push(check(TestName::Units, Outcome::Failed, 0.0));
    checks.push(check(TestName::IntegrationError, Outcome::Failed, 0.0));
    checks.push(check(TestName::LoopKnockout, Outcome::NotRun, 0.0));
    checks.push(check(TestName::LoopKnockout, Outcome::Observed, 0.0));
    for _ in 0..3 {
        checks.push(check(TestName::Disturbance, Outcome::Observed, 0.0));
    }
    let worth = checks
        .iter()
        .filter(|c| c.result.outcome != Outcome::Passed)
        .count();
    let (shown, omitted) = listed(checks);
    assert_eq!(shown.len(), MAX_RESULTS);
    assert_eq!(
        omitted,
        worth - MAX_RESULTS + MAX_STRONGEST,
        "what is worth reading and unlisted, the strongest passes among it"
    );
    let count = |test: TestName| shown.iter().filter(|c| c.key.test == test).count();
    for test in TestName::ALL {
        let expected = match test {
            TestName::Units | TestName::IntegrationError => 1,
            TestName::LoopKnockout => 2,
            TestName::Disturbance => 3,
            // The thirteen places left, taken in turn.
            TestName::ExtremeConditions => 7,
            TestName::Sensitivity => 6,
        };
        assert_eq!(count(test), expected, "{test:?}");
    }
    let rank = |outcome: Outcome| Outcome::ALL.iter().position(|o| *o == outcome);
    assert!(
        shown
            .windows(2)
            .all(|w| rank(w[0].result.outcome) <= rank(w[1].result.outcome)),
        "failures first, then what could not run, flagged, observed"
    );
    let strengths: Vec<f64> = shown
        .iter()
        .filter(|c| c.key.test == TestName::Sensitivity)
        .map(|c| c.strength)
        .collect();
    assert_eq!(
        strengths,
        [29.0, 28.0, 27.0, 26.0, 25.0, 24.0],
        "a test's strongest first"
    );

    // With room to spare, sensitivity's strongest passes fill it, the
    // strongest first and no more than their limit.
    let few: Vec<Check> = (0..8)
        .map(|i| check(TestName::Sensitivity, Outcome::Passed, f64::from(i)))
        .chain([check(TestName::Units, Outcome::Failed, 0.0)])
        .collect();
    let (shown, omitted) = listed(few);
    assert_eq!(omitted, 0);
    let strengths: Vec<f64> = shown.iter().map(|c| c.strength).collect();
    assert_eq!(strengths, [0.0, 7.0, 6.0, 5.0, 4.0, 3.0]);
}

/// The corpus's two largest models are tested within bounds: of time; of the
/// checks made, at most two runs a constant for each test that changes the
/// constants the model's flows read and one or a few for the others; of the
/// results the battery's batches hold at once (`runs::concurrent_runs`); and
/// of the answer, which keeps to the budget and lists at most its limit.
///
/// The time bounds are several times what a release build takes on a
/// 32-core host (World3 1.4 s, C-LEARN 12.7 s with six runs at a time), so a
/// slower runner passes and a regression of that order fails.
#[test]
#[ignore = "runs World3 and C-LEARN through the whole battery; run under the gates profile"]
fn the_largest_corpus_models_are_tested_within_bounds() {
    for (path, seconds) in [
        ("test/metasd/WRLD3-03/wrld3-03.mdl", 30.0),
        ("test/xmutil_test_models/C-LEARN v77 for Vensim.mdl", 180.0),
    ] {
        let project = corpus_model(path);
        let (constants, run_bytes) =
            read_as_the_battery(project.clone(), |model, reading, base| {
                let results = &base.results;
                let run_bytes =
                    results.specs.n_chunks * results.step_size * std::mem::size_of::<f64>();
                (reading.graph.feeding_flows(model).len(), run_bytes)
            });
        let at_once = runs::concurrent_runs(run_bytes);
        assert!(
            at_once == 1 || at_once * run_bytes <= runs::MAX_BATCH_BYTES,
            "{path}: {at_once} runs of {run_bytes} bytes at once"
        );
        let mut host = Host::new(project);
        let mut session = Session::new("main");
        let started = std::time::Instant::now();
        let output = host.call_raw(&mut session, "run_tests", "{}");
        let took = started.elapsed();
        assert!(!output.is_error, "{}", output.json);
        let answer: Value = serde_json::from_str(&output.json).unwrap();
        eprintln!(
            "{path}: {} bytes in {took:?}; {constants} constants feed flows; {} results listed, \
             {} omitted; runs of {run_bytes} bytes, {at_once} at once",
            output.json.len(),
            answer["results"].as_array().unwrap().len(),
            answer["omitted"],
        );
        assert!(
            took.as_secs_f64() <= seconds,
            "{path}: {took:?}, over {seconds} s"
        );
        assert!(
            output.json.len() <= crate::tools::OUTLINE_BUDGET,
            "{path}: {} bytes",
            output.json.len()
        );
        assert!(answer["results"].as_array().unwrap().len() <= MAX_RESULTS);
        for test in TestName::ALL {
            let name = serde_json::to_value(test).unwrap();
            let tested = summary(&answer, name.as_str().unwrap());
            let checks = tested["checks"].as_u64().unwrap() as usize;
            // Per-variant bounds: what each test makes checks of.
            let most = match test {
                TestName::Units | TestName::IntegrationError => 1,
                TestName::ExtremeConditions => 1 + 2 * constants,
                TestName::Sensitivity => 2 * constants,
                TestName::LoopKnockout => 0,
                TestName::Disturbance => MAX_DEFAULT_DISTURBANCES,
            };
            assert!(checks <= most, "{path}: {tested}, at most {most}");
        }
    }
}

/// A batch of value-only runs makes as many at once as its byte budget holds
/// the results of, and one however large a run is: C-LEARN's ten-megabyte
/// runs go six at a time. On a small model under a test budget of two runs'
/// results, no more than two run at once, however many threads there are.
#[test]
fn a_batch_holds_no_more_results_at_once_than_its_budget() {
    for (run_bytes, at_once) in [(10_419_512, 6), (1_296_032, 51), (usize::MAX, 1)] {
        assert_eq!(runs::concurrent_runs(run_bytes), at_once, "{run_bytes}");
    }

    let project = TestProject::new("small")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "100", &[], &["draining"], None)
        .flow("draining", "level / 2", None);
    let mut host = Host::from_test_project(&project);
    let model = host.project.models[0].clone();
    let base = Session::new("main")
        .runs
        .current(&mut host.workspace(), &model)
        .ok()
        .unwrap();
    let run_bytes =
        base.results.specs.n_chunks * base.results.step_size * std::mem::size_of::<f64>();
    runs::TEST_BATCH_BYTES.with(|budget| budget.set(2 * run_bytes + run_bytes / 2));
    let (active, peak) = (
        std::sync::atomic::AtomicUsize::new(0),
        std::sync::atomic::AtomicUsize::new(0),
    );
    let plans = vec![RunPlan::default(); 8];
    let outcomes = runs::execute_values(&mut host.workspace(), &model, &plans, |_, _| {
        use std::sync::atomic::Ordering::SeqCst;
        let now = active.fetch_add(1, SeqCst) + 1;
        peak.fetch_max(now, SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(20));
        active.fetch_sub(1, SeqCst);
    })
    .ok()
    .unwrap();
    runs::TEST_BATCH_BYTES.with(|budget| budget.set(runs::MAX_BATCH_BYTES));
    assert!(outcomes.iter().all(Result::is_ok));
    let threads = rayon::current_num_threads();
    assert_eq!(
        peak.into_inner(),
        threads.min(2),
        "runs at once on {threads} threads"
    );
}

/// A series of 101 points over 0..100 from its formula.
fn series(f: impl Fn(f64) -> f64) -> (Vec<f64>, Vec<f64>) {
    let times: Vec<f64> = (0..=100).map(f64::from).collect();
    let values = times.iter().map(|&t| f(t)).collect();
    (times, values)
}

/// The classifier's label for a series, and the family the battery reads
/// from it.
fn family_of(f: impl Fn(f64) -> f64) -> (ModeKind, Option<BehaviorFamily>) {
    let (times, values) = series(f);
    let mode = shape(&times, &values).mode;
    (mode.kind, BehaviorFamily::of(&mode, &values))
}

/// A rise to 1 at 60, then a fall of `back`, then a rise of `again`.
fn rise_fall_rise(back: f64, again: f64) -> impl Fn(f64) -> f64 {
    move |t| {
        if t <= 60.0 {
            t / 60.0
        } else if t <= 80.0 {
            1.0 - back * (t - 60.0) / 20.0
        } else {
            1.0 - back + again * (t - 80.0) / 20.0
        }
    }
}

/// Past its level by `past` of the way it rose, and back to it: an overshoot
/// that comes back `past / (1 + past)` of its range.
fn overshooting(past: f64) -> impl Fn(f64) -> f64 {
    move |t| {
        if t <= 40.0 {
            (1.0 + past) * t / 40.0
        } else {
            1.0 + past * (-(t - 40.0) / 8.0).exp()
        }
    }
}

/// A family is the classifier's mode with pace set aside: every mode belongs
/// to one, either way it goes, and an overshoot is a turn only when it comes
/// back the materiality threshold of its range or more.
#[test]
fn a_family_is_the_classifiers_mode_with_pace_set_aside() {
    use BehaviorFamily::*;
    use std::f64::consts::PI;
    // Each mode, by a series the classifier names so, rising and then the
    // same series upside down, with the family of each: `None` for a mode
    // with no direction.
    for kind in ModeKind::ALL {
        let (f, rising, falling): (Box<dyn Fn(f64) -> f64>, _, _) = match kind {
            ModeKind::AtRest => (Box::new(|_| 7.0), Some(Still), None),
            ModeKind::Linear => (Box::new(|t| 3.0 * t), Some(Rising), Some(Falling)),
            ModeKind::Exponential => (Box::new(|t| (0.05 * t).exp()), Some(Rising), Some(Falling)),
            ModeKind::GoalSeeking => (
                Box::new(|t| 1.0 - (-0.08 * t).exp()),
                Some(Rising),
                Some(Falling),
            ),
            ModeKind::SShaped => (
                Box::new(|t| 1.0 / (1.0 + (-(t - 50.0) / 6.0).exp())),
                Some(Rising),
                Some(Falling),
            ),
            // Below its chord, then above, then below again.
            ModeKind::Other => (
                Box::new(|t| t + 12.0 * (2.0 * PI * t / 100.0).sin()),
                Some(Rising),
                Some(Falling),
            ),
            ModeKind::Overshoot => (
                Box::new(overshooting(0.2)),
                Some(RisesThenFalls),
                Some(FallsThenRises),
            ),
            ModeKind::RiseAndFall => (
                Box::new(|t| (PI * t / 100.0).sin()),
                Some(RisesThenFalls),
                None,
            ),
            ModeKind::FallAndRise => (
                Box::new(|t| -(PI * t / 100.0).sin()),
                Some(FallsThenRises),
                None,
            ),
            ModeKind::Oscillation => (
                Box::new(|t| (2.0 * PI * t / 25.0).sin()),
                Some(Oscillating),
                None,
            ),
            ModeKind::Undefined => (
                Box::new(|t| if t > 40.0 { f64::NAN } else { t }),
                None,
                None,
            ),
        };
        assert_eq!(
            family_of(&f),
            (kind, rising),
            "{kind:?}, the series the classifier calls this"
        );
        if let Some(falling) = falling {
            assert_eq!(
                family_of(|t| -f(t)),
                (kind, Some(falling)),
                "{kind:?}, falling"
            );
        }
    }

    // An overshoot is a turn from the materiality threshold of its range
    // up: come back a fraction `c` of the range, it went `c / (1 - c)` past.
    for (back, family) in [
        (MATERIAL_CHANGE * 0.9, Rising),
        (MATERIAL_CHANGE * 1.1, RisesThenFalls),
        (0.03, Rising),
        (0.3, RisesThenFalls),
    ] {
        let (kind, found) = family_of(overshooting(back / (1.0 - back)));
        assert_eq!(kind, ModeKind::Overshoot, "{back}");
        assert_eq!(found, Some(family), "an overshoot that comes back {back}");
    }
}

/// A behavior has changed when its family has, between every two families;
/// not to or from a series that is not a number; not for a goal seeker whose
/// goal moved to its other side; and only where the series' turns confirm
/// it at both readings: not where the classifier names both runs alike,
/// whatever their turns, nor where it counts a movement in one run that the
/// other run makes too.
#[test]
fn a_behavior_changes_when_its_mode_and_its_turns_change_family() {
    use std::f64::consts::PI;
    let changed = |now: &dyn Fn(f64) -> f64, was: &dyn Fn(f64) -> f64| {
        let (times, values) = series(now);
        let (_, base_values) = series(was);
        changed_family(
            (&shape(&times, &values), &values),
            (&shape(&times, &base_values), &base_values),
        )
    };
    let of = |family: BehaviorFamily| -> Box<dyn Fn(f64) -> f64> {
        match family {
            BehaviorFamily::Still => Box::new(|_| 7.0),
            BehaviorFamily::Rising => Box::new(|t| 3.0 * t),
            BehaviorFamily::Falling => Box::new(|t| -3.0 * t),
            BehaviorFamily::RisesThenFalls => Box::new(|t| (PI * t / 100.0).sin()),
            BehaviorFamily::FallsThenRises => Box::new(|t| -(PI * t / 100.0).sin()),
            BehaviorFamily::Oscillating => Box::new(|t| (2.0 * PI * t / 25.0).sin()),
        }
    };
    for now in BehaviorFamily::ALL {
        for was in BehaviorFamily::ALL {
            assert_eq!(
                changed(&of(now), &of(was)),
                (now != was).then_some((now, was)),
                "{now:?} from {was:?}"
            );
        }
        let undefined = |t: f64| if t > 40.0 { f64::NAN } else { t };
        assert_eq!(
            changed(&of(now), &undefined),
            None,
            "{now:?} from undefined"
        );
        assert_eq!(
            changed(&undefined, &of(now)),
            None,
            "undefined from {now:?}"
        );
    }

    let seeking = |goal: f64| move |t: f64| 50.0 + (goal - 50.0) * (1.0 - (-0.08 * t).exp());
    assert_eq!(
        changed(&seeking(0.0), &seeking(100.0)),
        None,
        "a goal seeker whose goal moved past its start still seeks it"
    );
    assert_eq!(
        changed(&|t| -3.0 * t, &seeking(100.0)),
        Some((BehaviorFamily::Falling, BehaviorFamily::Rising)),
        "growth toward a goal turned to a steady decline has changed"
    );

    // A fall and rise with a brief start-up rise of a sixth of its range,
    // which the classifier reads past: both runs are a fall and rise, though
    // the start-up rise is a material turn.
    let started = |t: f64| {
        if t <= 15.0 {
            0.15 * t / 15.0
        } else if t <= 55.0 {
            0.15 - (t - 15.0) / 40.0
        } else {
            -0.85 + 0.8 * (t - 55.0) / 45.0
        }
    };
    let plain = |t: f64| {
        if t <= 55.0 {
            -t / 55.0
        } else {
            -1.0 + 0.8 * (t - 55.0) / 45.0
        }
    };
    for (f, what) in [
        (&started as &dyn Fn(f64) -> f64, "with"),
        (&plain, "without"),
    ] {
        assert_eq!(
            family_of(f).0,
            ModeKind::FallAndRise,
            "the premise: {what} the start-up rise"
        );
    }
    assert_eq!(changed(&started, &plain), None);

    // A final rise of a fifth of the range is a movement of the classifier's,
    // and one of 9% a disturbance: the modes differ in family, and the
    // series makes the rise in both runs.
    assert_ne!(
        family_of(rise_fall_rise(0.5, 0.2)).1,
        family_of(rise_fall_rise(0.5, 0.09)).1,
        "the premise"
    );
    assert_eq!(
        changed(&rise_fall_rise(0.5, 0.2), &rise_fall_rise(0.5, 0.09)),
        None
    );

    // From a series that does not rise again, it has changed.
    assert_eq!(
        changed(&rise_fall_rise(0.5, 0.3), &rise_fall_rise(0.5, 0.0)),
        Some((BehaviorFamily::Oscillating, BehaviorFamily::RisesThenFalls))
    );
}

/// A change of family is material from MATERIAL_CHANGE of the series' scale
/// up, that fraction included.
#[test]
fn a_change_of_family_is_material_from_the_materiality_threshold() {
    let changed = Some((BehaviorFamily::Oscillating, BehaviorFamily::Rising));
    for (changed, largest, is_material) in [
        (changed, Some(0.2), true),
        (changed, Some(MATERIAL_CHANGE), true),
        (changed, Some(MATERIAL_CHANGE * 0.999), false),
        (changed, None, false),
        (None, Some(0.9), false),
    ] {
        assert_eq!(
            material_change(changed, largest),
            changed.filter(|_| is_material),
            "{changed:?} {largest:?}"
        );
    }
}

/// Both runs of a check are read at the scale of what the series is
/// computed from: a stock at rest at zero, whose two flows balance exactly at the
/// model's value of `k` and by 5.6e-17 a unit of time at double it (`k +
/// 0.2` against `(10 * k + 2) / 10`), stays at rest, and its change is
/// measured by its flows, not by the zero it holds.
#[test]
fn a_check_that_leaves_residue_in_a_series_at_rest_does_not_move_it() {
    let project = TestProject::new("balanced")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "0", &["filling"], &["draining"], None)
        .flow("filling", "k + 0.2", None)
        .flow("draining", "(10 * k + 2) / 10", None)
        .stock("mover", "0", &["moving"], &[], None)
        .flow("moving", "k", None)
        .stock("idle", "0", &["starting"], &[], None)
        .flow("starting", "k - 0.05", None)
        .aux("k", "0.05", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "targets": ["k"]}),
    );
    let double = result(&output, "sensitivity", "k", "double");
    let response = double["responses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["variable"] == "level")
        .unwrap_or_else(|| panic!("{double}"));
    assert_eq!(response["mode"], "at_rest", "{double}");
    assert_eq!(response.get("was"), None, "{double}");
    assert_eq!(response.get("family"), None, "{double}");
    assert_eq!(
        (&response["change"], &response["largestChange"]),
        (&json!(0.0), &json!(0.0)),
        "residue is no change: {double}"
    );
    // A stock the model's run holds at exactly zero that the check sets
    // moving: its change is a fraction of what it is computed from, not of
    // the zero it held.
    let idle = double["responses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["variable"] == "idle")
        .unwrap_or_else(|| panic!("{double}"));
    let largest = idle["largestChange"].as_f64().unwrap();
    assert!(largest > 0.0 && largest <= 1.0, "{idle}");
}

/// Plain exponential growth and plain goal seeking change pace, not family,
/// at half and double their rates and times, whatever the horizon makes of
/// their labels; and a goal seeker whose goal halves or doubles past its
/// start still seeks it.
#[test]
fn sensitivity_does_not_flag_a_change_of_pace() {
    for rate in ["0.08", "0.12", "0.2"] {
        let project = TestProject::new("growth")
            .with_sim_time(0.0, 10.0, 0.125)
            .stock("capital", "100", &["investment"], &[], None)
            .flow("investment", "capital * growth_rate", None)
            .aux("growth_rate", rate, None);
        let mut host = Host::from_test_project(&project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["sensitivity"]}),
        );
        assert_eq!(
            summary(&output, "sensitivity")["flagged"],
            Value::Null,
            "rate {rate}: {output}"
        );
    }
    for tau in ["4", "8", "12"] {
        let project = TestProject::new("goal")
            .with_sim_time(0.0, 20.0, 0.125)
            .stock("level", "0", &["adjustment"], &[], None)
            .flow("adjustment", "(100 - level) / adjustment_time", None)
            .aux("adjustment_time", tau, None);
        let mut host = Host::from_test_project(&project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["sensitivity"]}),
        );
        assert_eq!(
            summary(&output, "sensitivity")["flagged"],
            Value::Null,
            "tau {tau}: {output}"
        );
    }
    let project = TestProject::new("goal past its start")
        .with_sim_time(0.0, 20.0, 0.125)
        .stock("level", "60", &["adjustment"], &[], None)
        .flow("adjustment", "(goal - level) / 4", None)
        .aux("goal", "100", None);
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["sensitivity"], "targets": ["goal"]}),
    );
    assert_eq!(
        summary(&output, "sensitivity")["flagged"],
        Value::Null,
        "half the goal is below the start: {output}"
    );
}

#[test]
fn results_come_failures_first_then_not_run_flagged_observed_and_passed() {
    let mut host = Host::from_test_project(&workforce());
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({
            "targets": ["tenure", "budget", "head_count"],
            "extremes": [{"variable": "tenure", "low": 0}]
        }),
    );
    let rank = |outcome: &Value| {
        Outcome::ALL
            .iter()
            .position(|o| serde_json::to_value(o).unwrap() == *outcome)
            .unwrap()
    };
    let outcomes: Vec<usize> = output["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| rank(&r["outcome"]))
        .collect();
    assert!(outcomes.windows(2).all(|w| w[0] <= w[1]), "{output}");
    let distinct: HashSet<usize> = outcomes.iter().copied().collect();
    assert!(distinct.len() >= 4, "the fixture mixes outcomes: {output}");
    assert_eq!(output["results"][0]["outcome"], "failed");
}

/// Units the project reads for a constant, parsed as the battery parses them.
fn conversion(name: &str, units: &str, value: f64) -> bool {
    let project = TestProject::new("conversions")
        .with_sim_time(0.0, 1.0, 1.0)
        .unit_with_aliases("ton", &["tons"])
        .aux(name, &value.to_string(), Some(units));
    let mut host = Host::from_test_project(&project);
    let ws = host.workspace();
    let resolved = resolve_model(ws.project, ws.db, "main").ok().unwrap();
    let units = Units::of(&ws, &resolved);
    let var = resolved.model.get_variable(name).unwrap();
    let declared = units.of_variable(var).unwrap();
    is_unit_conversion(name, &declared, &[value], &units)
}

#[test]
fn a_constant_whose_value_is_one_of_its_units_converts_units() {
    for (name, units, value, converts) in [
        // A ratio of one unit at two scales, at their ratio.
        ("ton_per_mton", "tons/Mton", 1e6, true),
        ("tonco2_per_gtonco2", "tonsCO2/GtonsCO2", 1e9, true),
        ("billion_people", "people/billion_people", 1e9, true),
        (
            "million_per_trillion_dollars",
            "million_dollars/trillion_dollars",
            1e6,
            true,
        ),
        ("ppt_per_ppb", "ppt/ppb", 1000.0, true),
        ("grams_per_kilogram", "grams/kilogram", 1000.0, true),
        // One unit, named with its value.
        ("one_year", "year", 1.0, true),
        ("100_percent", "percent", 100.0, true),
        // A ratio at scales that is not theirs is a quantity.
        ("emission_factor", "Mton/Gton", 0.3, false),
        // One unit the name does not state is a parameter.
        ("adjustment_time", "year", 1.0, false),
        ("delay_years", "year", 1.0, false),
        ("ten_years", "year", 10.0, false),
        ("one_step", "year", 1.0, false),
        ("max_share", "percent", 100.0, false),
        // Different units are not scales of one another.
        ("ch4_per_c", "Mtons/MtonsC", 1.33, false),
        ("price", "dollars/widget", 1.0, false),
        // Two units of time at their known ratio, within the calendars'
        // differences.
        ("hours_per_week", "hour/week", 168.0, true),
        ("weeks_per_year", "weeks/year", 52.0, true),
        ("months_per_year", "months/year", 12.0, true),
        ("days_per_year", "days/year", 365.0, true),
        ("days_per_year", "day/year", 360.0, true),
        ("hours_per_day", "hours/day", 24.0, true),
        ("minutes_per_hour", "minute/hour", 60.0, true),
        // Two units of time at another ratio are a quantity.
        ("working_hours", "hour/week", 40.0, false),
        ("shifts_per_day", "hour/day", 3.0, false),
    ] {
        assert_eq!(
            conversion(name, units, value),
            converts,
            "{name} = {value} {units}"
        );
    }
}

/// A model's unit conversions are no default target: the defaults leave them
/// out and say so, and a call that names one tests it.
#[test]
fn unit_conversions_are_left_out_of_the_defaults_and_named_ones_tested() {
    let project = TestProject::new("emissions")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_time_units("year")
        .unit_with_aliases("ton", &["tons"])
        .stock("carbon", "100", &["emissions"], &[], Some("tons"))
        .flow(
            "emissions",
            "fuel * intensity * ton_per_mton",
            Some("tons/year"),
        )
        .aux("fuel", "3", Some("Mtons/year"))
        .aux("intensity", "0.5", Some("dmnl"))
        .aux("ton_per_mton", "1000000", Some("tons/Mton"));
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions", "sensitivity"]}),
    );
    let note = output["leftOut"]
        .as_str()
        .unwrap_or_else(|| panic!("{output}"));
    assert!(note.contains("1 unit conversion (ton_per_mton)"), "{note}");
    assert_eq!(
        note.matches("unit conversion").count(),
        1,
        "said once for both tests: {note}"
    );
    for test in ["extreme_conditions", "sensitivity"] {
        assert_eq!(summary(&output, test).get("note"), None, "{output}");
    }
    assert!(
        !output.to_string().contains("\"variable\":\"ton_per_mton\""),
        "{output}"
    );
    let named = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions"], "targets": ["ton_per_mton"]}),
    );
    assert_eq!(
        summary(&named, "extreme_conditions")["checks"],
        3,
        "the conversion at both extremes, and the model's own run: {named}"
    );
    assert_eq!(named.get("leftOut"), None, "{named}");
}

/// Read `project`'s model as the battery does, its run included, and answer
/// with what `read` makes of it.
fn read_as_the_battery<T>(
    project: datamodel::Project,
    read: impl FnOnce(&datamodel::Model, &Reading<'_>, &Run) -> T,
) -> T {
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let model = host.project.models[0].clone();
    let base = session
        .runs
        .current(&mut host.workspace(), &model)
        .ok()
        .unwrap();
    let ws = host.workspace();
    let resolved = resolve_model(ws.project, ws.db, "main").ok().unwrap();
    let reading = Reading::of(&ws, &resolved, &base);
    read(resolved.model, &reading, &base)
}

/// The names of `project`'s constants `pick` picks out of the battery's
/// roles, sorted.
fn constants_that(project: &TestProject, pick: impl Fn(&Roles) -> Vec<String>) -> Vec<String> {
    read_as_the_battery(project.build_datamodel(), |_, reading, _| {
        let mut names = pick(&reading.roles);
        names.sort();
        names
    })
}

/// The extremes the battery's rules choose for the constant `name`, low then
/// high: each rule, and the value it gives the constant's first element.
fn extremes_of(project: &TestProject, name: &str) -> [(ExtremeRule, f64); 2] {
    read_as_the_battery(project.build_datamodel(), |model, reading, base| {
        let var = model.get_variable(name).unwrap();
        let value = element_values(base, model, var)[0];
        let specs = &base.results.specs;
        let (low, _) = reading.roles.low(var, GivenExtremes::default(), specs);
        let high = reading.roles.high(var, GivenExtremes::default(), specs);
        [(low.rule, (low.to)(value)), (high.rule, (high.to)(value))]
    })
}

/// The rule that chooses the low extreme of the constant `name`.
fn low_rule(project: &TestProject, name: &str) -> ExtremeRule {
    extremes_of(project, name)[0].0
}

/// The shares of `project`'s model, each with its whole, by name.
fn shares_of(project: &TestProject) -> Vec<(String, f64)> {
    read_as_the_battery(project.build_datamodel(), |_, reading, _| {
        let mut shares: Vec<(String, f64)> = reading
            .roles
            .shares
            .iter()
            .map(|(name, whole)| (name.clone(), *whole))
            .collect();
        shares.sort_by(|a, b| a.0.cmp(&b.0));
        shares
    })
}

/// A model whose one outflow takes `term` of its stock, `term` reading the
/// constant `name` with `value` and `units`.
fn taking(term: &str, name: &str, value: &str, units: Option<&str>) -> TestProject {
    TestProject::new("shares")
        .with_sim_time(0.0, 1.0, 1.0)
        .stock("level", "100", &[], &["out"], None)
        .flow("out", &format!("level * {term}"), None)
        .aux(name, value, units)
}

#[test]
fn a_share_is_known_by_its_units_its_name_or_its_complement() {
    let project = TestProject::new("shares")
        .with_sim_time(0.0, 1.0, 1.0)
        .stock("level", "100", &[], &["out"], None)
        .flow(
            "out",
            "level * (path_share + spare + tax + (1 - kept) + elasticity + head_share + over_share)",
            None,
        )
        .aux("path_share", "0.8", None)
        .aux("spare", "0.3", Some("fraction"))
        .aux("tax", "30", Some("percent"))
        .aux("kept", "0.4", None)
        .aux("elasticity", "0.5", None)
        .aux("head_share", "0.5", Some("people"))
        .aux("over_share", "1.5", Some("dmnl"));
    let named = |pairs: &[(&str, f64)]| -> Vec<(String, f64)> {
        pairs
            .iter()
            .map(|(name, whole)| (name.to_string(), *whole))
            .collect()
    };
    assert_eq!(
        shares_of(&project),
        named(&[
            ("kept", 1.0),
            ("path_share", 1.0),
            ("spare", 1.0),
            ("tax", 100.0)
        ]),
        "not an elasticity no unit or name calls a share, a share of people, or one past its whole"
    );

    // Each word that names a share does, on a constant with no units; the
    // same constant under another name is none.
    for word in SHARE_WORDS {
        let name = format!("recycled_{word}");
        assert_eq!(
            shares_of(&taking(&name, &name, "0.8", None)),
            named(&[(&name, 1.0)]),
            "{word}"
        );
    }
    assert_eq!(
        shares_of(&taking("recycling", "recycling", "0.8", None)),
        []
    );

    // Each spelling of percent makes the whole 100.
    for units in ["percent", "%", "pct"] {
        assert_eq!(
            shares_of(&taking("levy / 100", "levy", "30", Some(units))),
            named(&[("levy", 100.0)]),
            "{units}"
        );
    }

    // A share of the whole of it is a share, already at its high extreme:
    // only its low one, and the model's own run, are checks.
    let all = taking("(1 - kept)", "kept", "1", None);
    assert_eq!(shares_of(&all), named(&[("kept", 1.0)]));
    assert_eq!(
        extremes_of(&all, "kept"),
        [(ExtremeRule::Zero, 0.0), (ExtremeRule::Whole, 1.0)]
    );
    let mut host = Host::from_test_project(&all);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions", "sensitivity"], "targets": ["kept"]}),
    );
    assert_eq!(
        summary(&output, "extreme_conditions")["checks"],
        2,
        "{output}"
    );
    assert_eq!(
        summary(&output, "sensitivity")["checks"],
        1,
        "half of it, and no double past the whole: {output}"
    );

    // The complement of something else than one says nothing.
    assert_eq!(
        shares_of(&taking("(2 - kept) / 2", "kept", "0.4", None)),
        []
    );
}

/// A share's high extreme is the whole, not ten times it, and doubling one
/// stops at the whole: a share of a whole never passes it.
#[test]
fn a_share_is_never_taken_past_the_whole() {
    // At the whole, nothing takes the other path, which the ratio divides by.
    let project = TestProject::new("paths")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("gas", "100", &[], &["chemical", "other"], None)
        .stock("products", "0", &["chemical"], &[], None)
        .flow("chemical", "gas * 0.1 * path_share", None)
        .flow("other", "gas * 0.1 * (1 - path_share)", None)
        .aux("path_share", "0.8", None)
        .aux("ratio", "chemical / other", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions", "sensitivity"], "targets": ["path_share"], "record": ["products"]}),
    );
    let whole = result(&output, "extreme_conditions", "path_share", "high");
    assert_eq!(whole["extreme"], "whole");
    assert_eq!(whole["value"], 1.0);
    assert_eq!(
        whole["outcome"], "failed",
        "the ratio divides by zero there"
    );
    assert!(!output.to_string().contains("ten_times"), "{output}");
    let double = result(&output, "sensitivity", "path_share", "double");
    assert_eq!(double["value"], 1.0, "doubled to at most the whole");
    assert_eq!(double["heldAt"], "whole", "{double}");
    let half = result(&output, "sensitivity", "path_share", "half");
    assert_eq!(half.get("heldAt"), None, "{half}");
}

/// A series not a number in the model's own run is said once, as a finding
/// of the own run's check, and judged by no other check: a check cannot make
/// it so.
#[test]
fn a_series_undefined_in_the_models_own_run_is_said_once_and_not_judged() {
    let project = TestProject::new("nan")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("level", "100", &["inflow"], &["outflow"], None)
        .flow("inflow", "rate", None)
        .flow("outflow", "level / 5", None)
        .aux("rate", "20", None)
        .aux("unfinished", "NaN", None)
        .aux("report", "unfinished * level", None);
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"]}),
    );
    let own = own_run(&output).unwrap_or_else(|| panic!("{output}"));
    assert_eq!(own["outcome"], "failed", "{output}");
    let undefined: Vec<&str> = own["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["variable"].as_str().unwrap())
        .collect();
    assert_eq!(
        undefined.iter().copied().collect::<HashSet<&str>>(),
        HashSet::from(["unfinished", "report"]),
        "{output}"
    );
    let tested = summary(&output, "extreme_conditions");
    assert_eq!(tested["failed"], 1, "only the own run: {output}");
    assert_eq!(tested.get("note"), None, "{output}");
}

/// Holding a variable with a table at its initial value holds its value --
/// the table's output -- not the table's input.
#[test]
fn a_knockout_of_a_variable_with_a_table_holds_its_value() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let base = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["effect_of_pressure"]}),
    );
    let start = base["series"][0]["start"].clone();
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["loop_knockout"], "targets": ["effect_of_pressure"], "record": ["effect_of_pressure"]}),
    );
    let held = result(&output, "loop_knockout", "effect_of_pressure", "held");
    assert_eq!(held["value"], start, "{held}");
    assert_eq!(held["responses"][0]["mode"], "at_rest", "{held}");
}

/// A save step off the DT grid leaves the engine's results with unwritten
/// rows at time zero; the battery reads the rows the run saved.
#[test]
fn a_save_step_off_the_dt_grid_is_read_to_its_last_saved_row() {
    let project = TestProject::new("pendulum")
        .with_sim_time(0.0, 2.0, 1.0 / 128.0)
        .with_save_step(0.1)
        .stock("angle", "0.1", &["turning"], &[], None)
        .stock("speed", "0", &["accelerating"], &[], None)
        .flow("turning", "speed", None)
        .flow("accelerating", "-gravity / length * angle", None)
        .aux("gravity", "9.8", None)
        .aux("length", "2", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "targets": ["length"], "record": ["angle"]}),
    );
    let double = result(&output, "sensitivity", "length", "double");
    assert_eq!(double["value"], 4.0, "twice the constant's value: {double}");
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions"], "extremes": [{"variable": "length", "low": 0}]}),
    );
    let zero = result(&output, "extreme_conditions", "length", "low");
    let problem = &zero["problems"][0];
    assert_eq!(problem["kind"], "non_finite", "{zero}");
    assert!(problem["time"].as_f64().unwrap() <= 2.0, "{zero}");
}

/// A constant at its extreme already has no check there: zero is at its low
/// extreme, and ten times, half and double of zero are zero. The model's own
/// run is then the extreme conditions test's one check, and sensitivity has
/// none. A step of a tenth of zero says why it does not run.
#[test]
fn a_constant_at_its_extreme_already_is_no_check() {
    let project = TestProject::new("idle")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", "10", &["filling"], &["draining"], None)
        .flow("filling", "idle", None)
        .flow("draining", "level / drain_time", None)
        .aux("idle", "0", None)
        .aux("drain_time", "0", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    for (target, rules) in [
        ("idle", [ExtremeRule::Zero, ExtremeRule::TenTimes]),
        // A time constant of zero is below any short time already.
        (
            "drain_time",
            [ExtremeRule::ShortTime, ExtremeRule::TenTimes],
        ),
    ] {
        assert_eq!(
            extremes_of(&project, target),
            [(rules[0], 0.0), (rules[1], 0.0)],
            "{target}"
        );
        let output = host.call(
            &mut session,
            "run_tests",
            json!({"tests": ["extreme_conditions", "sensitivity", "disturbance"], "targets": [target]}),
        );
        assert_eq!(
            summary(&output, "extreme_conditions")["checks"],
            1,
            "{target}: {output}"
        );
        assert_eq!(
            summary(&output, "sensitivity")["skipped"],
            "the model has nothing this test changes",
            "{target}: {output}"
        );
        let step = result(&output, "disturbance", target, "step");
        assert_eq!(step["outcome"], "not_run", "{target}");
        assert!(step["reason"].as_str().unwrap().contains("is zero"));
    }
}

/// An arrayed variable is held element by element, each at its own initial
/// value.
#[test]
fn a_knockout_holds_each_element_of_an_arrayed_variable() {
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 10.0, 0.5)
        .named_dimension("region", &["north", "south"])
        .array_with_ranges("start[region]", vec![("north", "100"), ("south", "300")])
        .array_stock("population[region]", "start", &["births"], &[], None)
        .array_flow("births[region]", "perceived * 0.1", None)
        .array_aux("perceived[region]", "population");
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let base = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["perceived"]}),
    );
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["loop_knockout"], "targets": ["perceived"], "record": ["perceived"]}),
    );
    let held = result(&output, "loop_knockout", "perceived", "held");
    assert_eq!(held["outcome"], "observed", "{held}");
    assert_eq!(
        held["value"],
        Value::Null,
        "an arrayed hold has no one value"
    );
    assert!(!held["cutLinks"].as_array().unwrap().is_empty(), "{held}");
    assert!(!held["loops"].as_array().unwrap().is_empty(), "{held}");
    // Held at its start, each element ends that far below where it grew to.
    for series in base["series"].as_array().unwrap() {
        let (start, end) = (
            series["start"].as_f64().unwrap(),
            series["end"].as_f64().unwrap(),
        );
        let response = held["responses"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["variable"] == series["variable"])
            .unwrap_or_else(|| panic!("{held}"));
        let change = response["change"].as_f64().unwrap();
        assert!(
            (change - (start - end) / end).abs() < 1e-4,
            "{}: {change} against {}",
            series["variable"],
            (start - end) / end
        );
    }
}

/// Integration error is judged at every element of an arrayed stock: here
/// only the last of twelve moves fast enough for DT to matter.
#[test]
fn integration_error_is_judged_at_every_element() {
    let elements: Vec<String> = (1..=12).map(|i| format!("e{i}")).collect();
    let names: Vec<&str> = elements.iter().map(String::as_str).collect();
    let rates: Vec<(&str, &str)> = names
        .iter()
        .map(|&e| (e, if e == "e12" { "0.5" } else { "0.0001" }))
        .collect();
    let project = TestProject::new("growth")
        .with_sim_time(0.0, 10.0, 0.5)
        .named_dimension("unit", &names)
        .array_with_ranges("rate[unit]", rates)
        .array_stock("capital[unit]", "100", &["investment"], &[], None)
        .array_flow("investment[unit]", "capital * rate", None);
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["integration_error"]}),
    );
    let half = output["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["condition"] == "finer_dt")
        .unwrap_or_else(|| panic!("{output}"));
    assert_eq!(half["outcome"], "failed", "{half}");
    assert_eq!(half["differences"][0]["variable"], "capital[e12]", "{half}");
}

/// An answer keeps to the session's budget, leaving out its last listed
/// checks and counting them, and gives the checks it leaves out no id.
#[test]
fn an_answer_keeps_to_its_budget_and_gives_what_it_leaves_out_no_id() {
    let mut host = Host::from_test_project(&logarithms(6));
    let input = json!({"tests": ["extreme_conditions"]});
    let mut roomy = Session::new("main");
    roomy.outline_budget = usize::MAX;
    let whole = run(&mut host, &mut roomy, input.clone());
    let listed = whole["results"].as_array().unwrap().len();
    assert_eq!(listed, 6, "{whole}");

    let mut tight = Session::new("main");
    tight.outline_budget = whole.to_string().len() / 2;
    let fitted = run(&mut host, &mut tight, input.clone());
    assert!(fitted.to_string().len() <= tight.outline_budget, "{fitted}");
    let kept = fitted["results"].as_array().unwrap().len();
    assert!((1..listed).contains(&kept), "{fitted}");
    assert_eq!(fitted["omitted"], listed - kept);

    // The checks left out were given no id: the next new check takes the
    // id after the last one listed.
    tight.outline_budget = usize::MAX;
    let next = run(
        &mut host,
        &mut tight,
        json!({"tests": ["loop_knockout"], "targets": ["level"]}),
    );
    assert_eq!(next["results"][0]["id"], format!("T{}", kept + 1), "{next}");
}

/// A constant declared dimensionless that a stock is divided by in a rate is
/// a time constant all the same: the declaration cannot balance the rate's
/// units (the engine warns on that equation), so the role decides, and the
/// constant is tried at DT rather than at zero. The corpus's two such rows.
#[test]
fn a_time_constant_declared_dimensionless_is_known_by_its_role() {
    for (path, target, flow) in [
        (
            "../../test/test-models/samples/Workforce/workforce.mdl",
            "Average Tenure",
            "Departure",
        ),
        (
            "../../test/bobby/vdf/econ/mark2.mdl",
            "insolvency adjustment time",
            "change in insolvency risk",
        ),
    ] {
        let mdl =
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
                .expect("the corpus model is present");
        let project = crate::compat::open_vensim(&mdl).expect("the corpus model parses");
        let mut host = Host::new(project);
        let mut session = Session::new("main");
        let read = host.call(
            &mut session,
            "read_variables",
            json!({"names": [target, flow]}),
        );
        assert!(
            read["variables"][0]["units"]
                .as_str()
                .is_none_or(|units| ["1", "dmnl"].contains(&units)),
            "{path}: {target} is declared dimensionless: {read}"
        );
        let canonical = crate::canonicalize(target).into_owned();
        let found = time_constants_in(host.project.clone());
        assert!(
            found.contains(&(canonical, TimeConstantEvidence::DividesARate, 1.0)),
            "{path}: {found:?}"
        );
        let output = run(
            &mut host,
            &mut session,
            json!({"tests": ["extreme_conditions"], "targets": [target]}),
        );
        let tested = summary(&output, "extreme_conditions");
        assert_eq!(
            tested["failed"],
            Value::Null,
            "{path}: tried at DT, it holds: {output}"
        );
    }
}

/// A constant declared dimensionless is a time constant by its role only
/// where the rate it divides carries a unit warning, the ruling's reason for
/// taking the declaration for a slip. Where the rate's units balance, the
/// declaration is right: a pipeline's stages and a yield are scales, tested
/// at zero, where each divides by zero. A tenure that a person-per-year rate
/// divides, with nothing to balance its units, is still a time constant.
#[test]
fn a_dimensionless_divisor_is_a_time_constant_only_where_its_rate_cannot_balance() {
    let stages = TestProject::new("stages")
        .with_sim_time(0.0, 20.0, 0.25)
        .with_time_units("month")
        .stock("pipeline", "100", &[], &["outflow"], Some("widget"))
        .flow(
            "outflow",
            "pipeline / delay_time / number_of_stages",
            Some("widget/month"),
        )
        .aux("delay_time", "6", Some("month"))
        .aux("number_of_stages", "3", Some("dmnl"));
    let yielding = TestProject::new("yield")
        .with_sim_time(0.0, 20.0, 0.25)
        .with_time_units("month")
        .stock("workforce", "10", &[], &[], Some("person"))
        .stock("inventory", "0", &["production"], &[], Some("widget"))
        .flow(
            "production",
            "workforce * productivity / yield_factor",
            Some("widget/month"),
        )
        .aux("productivity", "5", Some("widget/person/month"))
        .aux("yield_factor", "0.9", Some("dmnl"));
    let tenure = TestProject::new("tenure")
        .with_sim_time(0.0, 20.0, 0.25)
        .with_time_units("year")
        .stock("experts", "100", &[], &["retiring"], Some("person"))
        .flow("retiring", "experts / average_tenure", Some("person/year"))
        .aux("average_tenure", "10", Some("dmnl"));
    for (project, target, time_constant) in [
        (&stages, "number_of_stages", false),
        (&yielding, "yield_factor", false),
        (&tenure, "average_tenure", true),
    ] {
        let found = time_constants_of(project);
        assert_eq!(
            found.iter().any(|(name, _, _)| name == target),
            time_constant,
            "{target}: {found:?}"
        );
        let mut host = Host::from_test_project(project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["extreme_conditions"], "targets": [target]}),
        );
        let tested = summary(&output, "extreme_conditions");
        assert_eq!(
            tested["passed"], tested["checks"],
            "{target} holds at its extremes: {output}"
        );
        assert_eq!(
            low_rule(project, target),
            if time_constant {
                ExtremeRule::ShortTime
            } else {
                ExtremeRule::Tenth
            },
            "{target}: a divisor either way, of a rate or of a quantity"
        );
    }
}

/// A check whose run makes a series undefined, where the model's own run has
/// it a number throughout, has found something: the response's numbers are
/// left out, as the schema allows, and the check is flagged, never passed,
/// and listed first among its test's even when nothing else moved.
/// Sensitivity, loop knockouts and disturbances share it.
#[test]
fn a_check_that_makes_a_series_undefined_is_flagged_with_its_numbers_left_out() {
    // Doubling k divides by zero in level's inflow; other responds finitely.
    let both = TestProject::new("blowup")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("level", "10", &["inflow"], &[], None)
        .stock("other", "10", &["other_inflow"], &[], None)
        .flow("inflow", "level * 0.1 / (2 - k)", None)
        .flow("other_inflow", "other * 0.1 * k", None)
        .aux("k", "1", None);
    let mut host = Host::from_test_project(&both);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["sensitivity"]}),
    );
    let doubled = result(&output, "sensitivity", "k", "double");
    assert_eq!(doubled["outcome"], "flagged", "{output}");
    let level = &doubled["responses"][0];
    assert_eq!(level["variable"], "level", "the undefined first: {output}");
    assert_eq!(level["mode"], "undefined", "{output}");
    assert!(
        level.get("change").is_none() && level.get("largestChange").is_none(),
        "{level}"
    );
    assert!(doubled["responses"][1]["change"].is_number(), "{output}");
    assert_eq!(summary(&output, "sensitivity")["passed"], 1, "{output}");

    // The undefined response alone, not a number from its first step (zero
    // over zero), whose largest difference a maximum that skips NaN would
    // read as none: the check is listed all the same, its numbers left out.
    let alone = TestProject::new("blowup_alone")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("level", "10", &["inflow"], &[], None)
        .flow("inflow", "(level - 10) * 0.1 / (2 - k) + 0.1", None)
        .aux("k", "1", None);
    let mut host = Host::from_test_project(&alone);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["sensitivity"]}),
    );
    let doubled = result(&output, "sensitivity", "k", "double");
    assert_eq!(doubled["outcome"], "flagged", "{output}");
    assert!(
        doubled["responses"][0].get("largestChange").is_none(),
        "{doubled}"
    );

    // Five stocks made undefined: three listed, the rest counted.
    let mut five = TestProject::new("five")
        .with_sim_time(0.0, 10.0, 0.25)
        .aux("k", "1", None);
    for i in 0..5 {
        five = five
            .stock(&format!("s{i}"), "10", &[&format!("f{i}")], &[], None)
            .flow(&format!("f{i}"), &format!("s{i} * 0.1 / (2 - k)"), None);
    }
    let mut host = Host::from_test_project(&five);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["sensitivity"]}),
    );
    let doubled = result(&output, "sensitivity", "k", "double");
    assert_eq!(doubled["responses"].as_array().unwrap().len(), MAX_DETAILS);
    assert_eq!(doubled["moreChanges"], 2, "{doubled}");

    // A step of k to 1.1, where the flow divides by 1.1 - k.
    let stepped = TestProject::new("step_blowup")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("level", "10", &["inflow"], &[], None)
        .flow("inflow", "level * 0.1 / (1.1 - k)", None)
        .aux("k", "1", None);
    let mut host = Host::from_test_project(&stepped);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["disturbance"], "targets": ["k"]}),
    );
    let step = result(&output, "disturbance", "k", "step");
    assert_eq!(step["outcome"], "flagged", "{output}");
    assert!(step["responses"][0].get("change").is_none(), "{step}");

    // Holding ramp at its start while other rises to meet it.
    let held = TestProject::new("held_blowup")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("level", "10", &["inflow"], &[], None)
        .flow("inflow", "level * 0.01 / (ramp - other)", None)
        .aux("ramp", "1 + TIME", None)
        .aux("other", "0.5 + 0.5 * TIME", None);
    let mut host = Host::from_test_project(&held);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["loop_knockout"], "targets": ["ramp"]}),
    );
    let knocked = result(&output, "loop_knockout", "ramp", "held");
    assert_eq!(knocked["outcome"], "flagged", "{output}");
    assert!(knocked["responses"][0].get("change").is_none(), "{knocked}");
}

/// A conversion between units of time is left out of the extremes as a
/// conversion between scales is: hours per week at zero changes the units a
/// quantity is counted in, not the quantity.
#[test]
fn conversions_between_units_of_time_are_left_out_of_the_defaults() {
    let project = TestProject::new("conversions")
        .with_sim_time(0.0, 20.0, 0.25)
        .with_time_units("years")
        .stock("stock", "100", &["inflow"], &["outflow"], Some("widget"))
        .flow(
            "inflow",
            "10 * months_per_year / 12 * days_per_year / 365 * hours_per_week / 168",
            Some("widget/year"),
        )
        .flow("outflow", "stock / residence_time", Some("widget/year"))
        .aux("months_per_year", "12", Some("month/year"))
        .aux("days_per_year", "365", Some("day/year"))
        .aux("hours_per_week", "168", Some("hour/week"))
        .aux("residence_time", "5", Some("year"));
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"]}),
    );
    let tested = summary(&output, "extreme_conditions");
    assert_eq!(
        tested["checks"], 3,
        "the model's own run, and only residence_time: {output}"
    );
    let note = output["leftOut"].as_str().unwrap();
    assert!(
        note.contains("3 unit conversions") && note.contains("hours_per_week"),
        "{note}"
    );
}

/// A battery stops before each of its runs, and between the slices of each,
/// once other work waits for the project, and keeps nothing: the battery made
/// again answers as a fresh session's does, its ids and the loop ids of its
/// knockouts included. The places it stops are sampled, early and late, by
/// doubling how long the call runs before the work begins to wait.
#[test]
fn a_battery_stops_between_its_runs_when_other_work_waits_and_keeps_nothing() {
    let input = json!({"targets": ["coverage", "production"]});
    let fresh = run(
        &mut Host::from_test_project(&inventory()),
        &mut Session::new("main"),
        input.clone(),
    );
    let mut stopped = 0;
    for after in (0..).map(|n| 1usize << n) {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        let output = host.call_waiting(
            &mut session,
            "run_tests",
            input.clone(),
            &Host::waiting_after(after),
        );
        if !output.is_error {
            break;
        }
        assert!(output.interrupted, "{}", output.json);
        assert!(
            session.evidence.loop_key("L1").is_none(),
            "stopped after {after} checks, it gave out no loop id"
        );
        stopped += 1;
        let again = run(&mut host, &mut session, input.clone());
        assert_eq!(
            again, fresh,
            "stopped after {after} checks, it kept nothing"
        );
    }
    assert!(stopped >= 5, "places to stop, early and late: {stopped}");
}

/// A model of the years 2000 to 2020 whose one flow is `equation`, with the
/// auxiliaries `others` (name, equation, units).
fn dated(equation: &str, others: &[(&str, &str, Option<&str>)]) -> TestProject {
    let mut project = TestProject::new("dated")
        .with_sim_time(2000.0, 2020.0, 1.0)
        .with_time_units("year")
        .stock("level", "0", &["change"], &[], None)
        .flow("change", equation, None);
    for (name, equation, units) in others {
        project = project.aux(name, equation, *units);
    }
    project
}

/// The constants of [`dated`] models.
const DATED: [(&str, &str, Option<&str>); 4] = [
    ("height", "1", None),
    ("policy_year", "2010", None),
    ("end_year", "2015", None),
    ("interval", "5", None),
];

/// The constants the battery takes for dates, sorted.
fn dates_of(project: &TestProject) -> Vec<String> {
    constants_that(project, |roles| roles.dates.iter().cloned().collect())
}

/// The constants the battery takes for time constants, sorted.
fn durations_of(project: &TestProject) -> Vec<String> {
    constants_that(project, |roles| {
        roles.time_constants.keys().cloned().collect()
    })
}

/// A constant used as a point in time is a date: one an equation compares
/// with the time, by any comparison and on either side, or gives STEP, RAMP
/// or PULSE as the time it starts at (or a ramp ends at), and one an
/// auxiliary so used copies, selects, or holds the initial value of. Only a
/// bare reference is evidence: a sum compared with the time, or a
/// subtraction from it, does not say which operand is the date.
#[test]
fn a_constant_used_as_a_point_in_time_is_a_date() {
    let none: [&str; 0] = [];
    for op in [">", "<", ">=", "<=", "=", "<>"] {
        for equation in [
            format!("IF TIME {op} policy_year THEN height ELSE 0"),
            format!("IF policy_year {op} TIME THEN height ELSE 0"),
        ] {
            assert_eq!(
                dates_of(&dated(&equation, &DATED)),
                ["policy_year"],
                "{equation}"
            );
        }
    }
    for (equation, dates) in [
        ("STEP(height, policy_year)", vec!["policy_year"]),
        (
            "RAMP(height, policy_year, end_year)",
            vec!["end_year", "policy_year"],
        ),
        ("PULSE(height, policy_year, interval)", vec!["policy_year"]),
        ("height * (1 + STEP(1, policy_year))", vec!["policy_year"]),
        ("IF TIME > policy_year + interval THEN 1 ELSE 0", vec![]),
        ("(TIME - policy_year) * height", vec![]),
        ("IF level > policy_year THEN 1 ELSE 0", vec![]),
        ("height * policy_year", vec![]),
    ] {
        assert_eq!(dates_of(&dated(equation, &DATED)), dates, "{equation}");
    }

    // Through an auxiliary that is used as a point in time.
    for (carrier, dates) in [
        ("policy_year", vec!["policy_year"]),
        (
            "IF height > 0 THEN policy_year ELSE end_year",
            vec!["end_year", "policy_year"],
        ),
        ("INIT(policy_year)", vec!["policy_year"]),
        ("policy_year + interval", vec![]),
        ("MAX(policy_year, end_year)", vec![]),
    ] {
        let mut others = DATED.to_vec();
        others.push(("start", carrier, None));
        assert_eq!(
            dates_of(&dated("STEP(height, start)", &others)),
            dates,
            "start = {carrier}"
        );
    }

    // A date is in a unit of time or in none: a constant in other units an
    // equation compares with the time is a threshold on something else. A
    // date in a unit of time is no time constant for it.
    for (units, is_date) in [
        (None, true),
        (Some("year"), true),
        (Some("month"), true),
        (Some("widgets"), false),
        (Some("1/year"), false),
    ] {
        let project = dated(
            "STEP(height, policy_year)",
            &[("height", "1", None), ("policy_year", "2010", units)],
        );
        assert_eq!(dates_of(&project) == ["policy_year"], is_date, "{units:?}");
        assert_eq!(durations_of(&project), none, "{units:?}");
    }

    // A delay's time, or what a rate is divided by, is a duration however
    // else it is used.
    for equation in [
        "SMTH1(height, policy_year) + STEP(1, policy_year)",
        "level / policy_year + STEP(1, policy_year)",
    ] {
        let project = dated(equation, &DATED);
        assert_eq!(dates_of(&project), none, "{equation}");
        assert_eq!(durations_of(&project), ["policy_year"], "{equation}");
    }
}

/// A date's extremes are the run's start and a tenth of the run past its
/// stop: the policy on from the first step, and never. The tests that change
/// a constant by a factor leave dates out and say so, and do not run on one
/// the call names.
#[test]
fn a_date_is_tried_at_the_runs_start_and_past_its_stop_and_never_scaled() {
    // The divisions fail at each extreme, which shows the values tried.
    let project = dated(
        "STEP(height, policy_year) + 1 / (policy_year - 2000) + 1 / (policy_year - 2022)",
        &DATED,
    );
    assert_eq!(
        extremes_of(&project, "policy_year"),
        [
            (ExtremeRule::RunStart, 2000.0),
            (ExtremeRule::PastStop, 2022.0)
        ]
    );
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions", "sensitivity", "disturbance"]}),
    );
    for (condition, rule, value) in [("low", "run_start", 2000.0), ("high", "past_stop", 2022.0)] {
        let check = result(&output, "extreme_conditions", "policy_year", condition);
        assert_eq!(check["outcome"], "failed", "{check}");
        assert_eq!(check["extreme"], rule);
        assert_eq!(check["value"], value);
    }
    assert_eq!(summary(&output, "extreme_conditions")["note"], Value::Null);
    let note = output["leftOut"]
        .as_str()
        .unwrap_or_else(|| panic!("{output}"));
    assert!(
        note.starts_with("Sensitivity and disturbance leave out 1 date (policy_year)"),
        "{note}"
    );
    for test in ["sensitivity", "disturbance"] {
        assert!(
            output["results"]
                .as_array()
                .unwrap()
                .iter()
                .all(|r| r["test"] != test || r["variable"] != "policy_year"),
            "{output}"
        );
    }

    let named = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity", "disturbance"], "targets": ["policy_year"]}),
    );
    for (test, condition) in [
        ("sensitivity", "half"),
        ("sensitivity", "double"),
        ("disturbance", "step"),
    ] {
        let check = result(&named, test, "policy_year", condition);
        assert_eq!(check["outcome"], "not_run", "{check}");
        assert_eq!(
            check["reason"],
            "'policy_year' is used as a point in time, and a multiple of a date is no condition \
             of the system; move it with run_experiment"
        );
    }
}

/// A fractional rate, a constant in units of one over a unit of time, is
/// tried no higher than one over two DTs, in its own unit of time: past
/// that, Euler's step of a drain overshoots, and the run shows the
/// integration. Sensitivity's double stops there too.
#[test]
fn a_fractional_rate_is_tried_no_faster_than_one_over_two_dts() {
    let draining = |value: f64, units: Option<&str>| {
        TestProject::new("drain")
            .with_sim_time(0.0, 10.0, 0.25)
            .with_time_units("year")
            .stock("level", "100", &[], &["draining"], None)
            .flow("draining", "level * loss", None)
            .aux("loss", &value.to_string(), units)
    };
    use ExtremeRule::{FastestRate, TenTimes};
    for (units, value, high) in [
        // A DT of a quarter of a year: two a year at the fastest.
        (Some("1/year"), 0.5, (FastestRate, 2.0)),
        // Ten times is slower than the fastest.
        (Some("1/year"), 0.1, (FastestRate, 1.0)),
        // Faster than the fastest already: it stays where it is.
        (Some("1/year"), 3.0, (FastestRate, 3.0)),
        // A DT of three months: a sixth a month at the fastest.
        (Some("1/month"), 0.05, (FastestRate, 1.0 / 6.0)),
        // No rate by its units: ten times.
        (None, 0.5, (TenTimes, 5.0)),
        (Some("dmnl"), 0.5, (TenTimes, 5.0)),
        (Some("widgets/year"), 0.5, (TenTimes, 5.0)),
    ] {
        let (rule, at) = extremes_of(&draining(value, units), "loss")[1];
        assert_eq!(rule, high.0, "{value} {units:?}");
        assert!((at - high.1).abs() < 1e-12, "{value} {units:?}: {at}");
    }

    // At the fastest rate the drain holds; at ten times an undeclared rate,
    // the stock overshoots zero at the model's DT and not at a finer one.
    for (units, artifacts) in [(Some("1/year"), false), (None, true)] {
        let mut host = Host::from_test_project(&draining(0.5, units));
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["extreme_conditions"]}),
        );
        let tested = summary(&output, "extreme_conditions");
        assert_eq!(tested["passed"], 3, "{units:?}: {output}");
        assert_eq!(tested["note"].is_string(), artifacts, "{units:?}: {output}");
    }

    // Double of a rate stops at the fastest, and a rate there already has
    // no double.
    for (value, doubled) in [(0.5, Some(1.0)), (1.5, Some(2.0)), (3.0, None)] {
        let mut host = Host::from_test_project(&draining(value, Some("1/year")));
        let mut session = Session::new("main");
        session.outline_budget = usize::MAX;
        let output = run(&mut host, &mut session, json!({"tests": ["sensitivity"]}));
        match doubled {
            Some(doubled) => {
                let check = result(&output, "sensitivity", "loss", "double");
                assert_eq!(check["value"], doubled, "{value}");
                let held = (doubled != value * 2.0).then(|| json!("fastest_rate"));
                assert_eq!(check.get("heldAt"), held.as_ref(), "{value}: {check}");
            }
            None => assert_eq!(summary(&output, "sensitivity")["checks"], 1, "{value}"),
        }
    }
}

/// DT is put in a time constant's own unit before the two are compared: in
/// a model of months with a DT of a quarter of one, a residence time in
/// weeks is four DTs at 4.35 weeks. A constant in a unit whose length
/// against the model's is not known has no such floor, and is tried at a
/// tenth of it; one known by its role is in the model's own unit.
#[test]
fn a_time_constants_floor_is_dt_in_the_constants_own_unit() {
    let weeks_a_month = 2_629_746.0 / 604_800.0;
    for (model_time, units, value, low) in [
        ("month", Some("month"), 20.0, 2.0),
        ("month", Some("month"), 5.0, 1.0),
        ("month", Some("weeks"), 100.0, 10.0),
        ("month", Some("weeks"), 20.0, weeks_a_month),
        // Within four DTs already.
        ("month", Some("weeks"), 4.0, 4.0),
        ("month", Some("year"), 1.0, 0.1),
        ("month", Some("year"), 0.5, 1.0 / 12.0),
        // A clock with a unit of its own: weeks are time, of no known
        // length against it.
        ("tick", Some("weeks"), 20.0, 2.0),
        ("tick", Some("tick"), 5.0, 1.0),
        ("tick", None, 5.0, 1.0),
    ] {
        let project = TestProject::new("tank")
            .with_sim_time(0.0, 10.0, 0.25)
            .with_time_units(model_time)
            .stock("water", "10", &[], &["draining"], None)
            .flow("draining", "water / residence_time", None)
            .aux("residence_time", &value.to_string(), units);
        let [(rule, at), _] = extremes_of(&project, "residence_time");
        assert_eq!(rule, ExtremeRule::ShortTime, "{model_time} {units:?}");
        assert!(
            (at - low).abs() < 1e-9,
            "{value} {units:?} in a model of {model_time}s: {at}, not {low}"
        );
        assert!(at <= value, "a low extreme is never above the value");
    }

    // Sensitivity's half stops at the same floor.
    let project = TestProject::new("tank")
        .with_sim_time(0.0, 10.0, 0.25)
        .with_time_units("month")
        .stock("water", "10", &[], &["draining"], None)
        .flow("draining", "water / residence_time", None)
        .aux("residence_time", "6", Some("weeks"));
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(&mut host, &mut session, json!({"tests": ["sensitivity"]}));
    let half = result(&output, "sensitivity", "residence_time", "half");
    assert_eq!(half["heldAt"], "short_time", "{half}");
    let half = half["value"].as_f64().unwrap();
    assert!((half - weeks_a_month).abs() < 1e-3, "{half}");
}

/// The arguments of `call`, parsed.
fn arguments(call: &str) -> Vec<Expr0> {
    match Expr0::new(call, LexerType::Equation) {
        Ok(Some(Expr0::App(UntypedBuiltinFn(_, args), _))) => args.into_vec(),
        _ => panic!("{call} is a call"),
    }
}

/// Which argument of a delay, a smooth or a trend is its time, and how many
/// stages it spreads its input over, for every function the engine expands
/// to a stdlib model (`stdlib::MODEL_NAMES`) and every alias of one.
#[test]
fn a_delays_time_and_stages_are_known_for_each_function_the_engine_expands() {
    let aliases = ["delay", "delayn", "smthn"];
    for alias in aliases {
        assert!(crate::builtins::is_stdlib_module_function(alias), "{alias}");
    }
    for name in crate::stdlib::MODEL_NAMES.iter().chain(&aliases) {
        // What each function is, written here independently of the stdlib's
        // models: the argument that is its time, and its order when called
        // with a third argument of 3.
        let expected = match *name {
            "smth1" | "delay1" | "trend" => Some((1, 1.0)),
            "smth3" | "delay3" => Some((1, 3.0)),
            // The pipeline delay has no order: the engine runs it as a
            // first-order one.
            "delay" => Some((1, 1.0)),
            "delayn" | "smthn" => Some((1, 3.0)),
            // A net present value's second argument is a discount rate, and
            // the systems-format models are no functions an equation calls.
            "npv" | "systems_conversion" | "systems_leak" | "systems_rate" => None,
            other => panic!("{other} is a stdlib model: say here which argument is its time"),
        };
        assert_eq!(
            time_argument(name, &arguments(&format!("{name}(input, t, 3)"))),
            expected,
            "{name}"
        );
    }
    // The order an alias is given is its stages; one that is no number
    // leaves it first order.
    for (function, call, stages) in [
        ("smthn", "smthn(input, t, 1)", 1.0),
        ("delayn", "delayn(input, t, 3, 0)", 3.0),
        ("delayn", "delayn(input, t, n)", 1.0),
        ("delayn", "delayn(input, t)", 1.0),
        ("smthn", "smthn(input, t, 0)", 1.0),
    ] {
        assert_eq!(
            time_argument(function, &arguments(call)),
            Some((1, stages)),
            "{call}"
        );
    }

    // In a model, however the call is capitalized: the constant given as
    // the time is a time constant, of the delay's stages.
    for (call, stages) in [
        ("SMTH1(level, t)", 1.0),
        ("SMTH3(level, t)", 3.0),
        ("DELAY1(level, t)", 1.0),
        ("delay3(level, t)", 3.0),
        ("TREND(level, t, 0)", 1.0),
        ("SMTHN(level, t, 3)", 3.0),
        ("DELAYN(level, t, 3)", 3.0),
        ("Smthn(level, t, 1)", 1.0),
        ("NPV(level, t)", 0.0),
    ] {
        let project = TestProject::new("delays")
            .with_sim_time(0.0, 10.0, 0.25)
            .stock("level", "100", &["filling"], &[], None)
            .flow("filling", "delayed / 50", None)
            .aux("delayed", call, None)
            .aux("t", "8", None);
        let found = time_constants_of(&project);
        if stages == 0.0 {
            assert_eq!(found, [], "{call}");
        } else {
            assert_eq!(
                found,
                [("t".to_string(), TimeConstantEvidence::DelayTime, stages)],
                "{call}"
            );
        }
    }
}

/// A unit conversion named for its unit is one whatever the project calls
/// the unit: `one_year = 1 yr` where `year` is an alias of `yr`.
#[test]
fn a_conversion_names_its_unit_by_any_name_the_project_reads_as_it() {
    for (name, units, converts) in [
        ("one_year", "yr", true),
        ("one_year", "year", true),
        ("one_years", "yr", true),
        ("one_yr", "yr", true),
        ("one_person", "yr", false),
        ("one_year", "person", false),
    ] {
        let project = TestProject::new("conversions")
            .with_sim_time(0.0, 1.0, 1.0)
            .unit_with_aliases("yr", &["year", "years"])
            .aux(name, "1", Some(units));
        assert_eq!(
            constants_that(&project, |roles| roles
                .conversions
                .iter()
                .cloned()
                .collect()),
            if converts { vec![name] } else { vec![] },
            "{name} = 1 {units}"
        );
    }
}

/// What each word of the battery's tables means, written from the
/// definitions (SI prefixes; short-scale number words; parts per hundred,
/// thousand, million, billion and trillion; a mean Gregorian year of
/// 365.2425 days, a month a twelfth of it and a quarter a fourth), not read
/// from the tables, so a wrong value in either is caught. A word with two
/// meanings has both.
const DEFINITIONS: [(&str, f64); 31] = [
    ("hundred", 1e2),
    ("thousand", 1e3),
    ("million", 1e6),
    ("billion", 1e9),
    ("trillion", 1e12),
    ("kilo", 1e3),
    ("mega", 1e6),
    ("giga", 1e9),
    ("tera", 1e12),
    ("milli", 1e-3),
    ("micro", 1e-6),
    ("nano", 1e-9),
    ("k", 1e3),
    ("m", 1e6),
    ("m", 1e-3),
    ("g", 1e9),
    ("t", 1e12),
    ("u", 1e-6),
    ("percent", 1e-2),
    ("pct", 1e-2),
    ("permille", 1e-3),
    ("ppm", 1e-6),
    ("ppb", 1e-9),
    ("ppt", 1e-12),
    ("ppt", 1e-3),
    ("one", 1.0),
    ("nanosecond", 1e-9),
    ("microsecond", 1e-6),
    ("millisecond", 1e-3),
    ("second", 1.0),
    ("minute", 60.0),
];

/// The lengths of the units of time in seconds, from the same definitions.
const LENGTHS: [(&str, f64); 6] = [
    ("hour", 3_600.0),
    ("day", 86_400.0),
    ("week", 604_800.0),
    ("month", 365.2425 * 86_400.0 / 12.0),
    ("quarter", 365.2425 * 86_400.0 / 4.0),
    ("year", 365.2425 * 86_400.0),
];

/// Every row of the battery's word tables is the meaning its word has by
/// definition.
#[test]
fn every_word_in_the_tables_means_what_it_is_defined_to() {
    let defined = |word: &str, value: f64| {
        DEFINITIONS
            .iter()
            .chain(&LENGTHS)
            .any(|&(w, v)| w == word && (v / value - 1.0).abs() < 1e-12)
    };
    let tables: [&[(&str, f64)]; 4] = [&SCALES, &NUMBER_UNITS, &NUMBER_WORDS, &TIME_UNITS];
    for table in tables {
        for &(word, value) in table {
            assert!(defined(word, value), "{word} = {value}");
        }
    }
    // Number words a name states a value in are one, and the number words
    // of a scale that are one of a unit that is a number at a scale.
    let stated: Vec<&str> = NUMBER_WORDS.iter().map(|&(word, _)| word).collect();
    assert_eq!(
        stated,
        [
            "one", "hundred", "thousand", "million", "billion", "trillion"
        ]
    );
    assert_eq!(
        SHARE_WORDS,
        ["share", "fraction", "proportion", "percent", "percentage"]
    );
}

/// The tables a unit's name and a constant's name are read by, row by row:
/// each scale a unit's name carries, each unit that is a number at a scale,
/// each number word a name states its value in, and each unit of time
/// against a year.
#[test]
fn a_conversion_is_read_by_every_scale_number_unit_and_number_word() {
    for (prefix, scale) in SCALES {
        // How many widgets one scaled unit is: the constant's value in
        // widgets per scaled unit. A scale runs into its unit or joins it
        // with an underscore.
        let scaled = format!("{prefix}_widgets");
        assert!(
            conversion("widgets_per", &format!("widgets/{prefix}widgets"), scale),
            "{scale} widgets/{prefix}widgets"
        );
        assert!(
            conversion("widgets_per", &format!("widgets/{scaled}"), scale),
            "{scale} widgets/{scaled}"
        );
        assert!(
            !conversion("widgets_per", &format!("widgets/{scaled}"), scale * 3.0),
            "{} widgets/{scaled} is a quantity",
            scale * 3.0
        );
    }
    for (unit, scale) in NUMBER_UNITS {
        // A number at a scale against another: the ratio of the two scales.
        let (other, other_scale) = if unit == "ppm" {
            ("percent", 1e-2)
        } else {
            ("ppm", 1e-6)
        };
        let ratio = other_scale / scale;
        assert!(
            conversion("parts", &format!("{unit}/{other}"), ratio),
            "{ratio} {unit}/{other}"
        );
        assert!(
            !conversion("parts", &format!("{unit}/{other}"), ratio * 3.0),
            "{} {unit}/{other} is a quantity",
            ratio * 3.0
        );
    }
    for (word, value) in NUMBER_WORDS {
        // The unit that many of is one: a number at the scale of one over
        // it, or for one itself any unit.
        let unit = NUMBER_UNITS
            .iter()
            .find(|(_, scale)| (scale * value - 1.0).abs() < 1e-9)
            .map(|&(unit, _)| unit)
            .or((value == 1.0).then_some("year"))
            .unwrap_or_else(|| panic!("no unit is one at {word} of it: the word states nothing"));
        assert!(
            conversion(&format!("{word}_{unit}"), unit, value),
            "{word}_{unit} = {value} {unit}"
        );
        assert!(
            !conversion(&format!("normal_{unit}"), unit, value),
            "normal_{unit} = {value} {unit} states no value"
        );
    }
    let year = 31_556_952.0;
    for (unit, seconds) in TIME_UNITS {
        if unit == "year" {
            continue;
        }
        let per_year = year / seconds;
        assert!(
            conversion("per_year", &format!("{unit}/year"), per_year),
            "{per_year} {unit}/year"
        );
        // Calendars differ by about a percent and a half (a 360-day year
        // is 1.4% short of 365.2425 days), and by no more.
        assert!(
            conversion("per_year", &format!("{unit}/year"), per_year * 1.015),
            "{unit}"
        );
        assert!(
            !conversion("per_year", &format!("{unit}/year"), per_year * 1.03),
            "{unit}"
        );
    }
    // A value is one of its units to within rounding and no further.
    for (off_by, converts) in [(5e-7, true), (2e-6, false), (0.3, false)] {
        assert_eq!(
            conversion("ton_per_mton", "tons/Mton", 1e6 * (1.0 + off_by)),
            converts,
            "{off_by}"
        );
    }
}

/// A model that declares no units has nothing for the units check to check:
/// the test is skipped saying so, not passed. One declared unit is enough.
#[test]
fn the_units_check_is_skipped_where_the_model_declares_no_units() {
    for (units, checked) in [(None, false), (Some("  "), false), (Some("widgets"), true)] {
        let project = TestProject::new("bare")
            .with_sim_time(0.0, 10.0, 1.0)
            .with_time_units("year")
            .stock("level", "1", &["filling"], &[], units)
            .flow("filling", "2", None);
        let mut host = Host::from_test_project(&project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["units"]}),
        );
        let tested = summary(&output, "units");
        if checked {
            assert_eq!(tested["checks"], 1, "{units:?}: {tested}");
            assert_eq!(tested.get("skipped"), None, "{units:?}: {tested}");
        } else {
            assert_eq!(
                tested,
                &json!({
                    "test": "units",
                    "checks": 0,
                    "skipped": "the model declares no units, so there is nothing to check: \
                                units are checked once variables declare them"
                }),
                "{units:?}"
            );
        }
    }
}

/// A unit definition that does not parse is an error of the project whether
/// or not a variable uses it: a model that declares no units still has it
/// checked, and fails.
#[test]
fn a_broken_unit_definition_is_checked_where_no_variable_declares_units() {
    let project = TestProject::new("bare")
        .with_sim_time(0.0, 10.0, 1.0)
        .unit("widget", Some("1 /"))
        .stock("level", "1", &["filling"], &[], None)
        .flow("filling", "2", None);
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["units"]}),
    );
    let tested = summary(&output, "units");
    assert_eq!(tested.get("skipped"), None, "{output}");
    assert_eq!(tested["failed"], 1, "{output}");
}

/// A call gives a constant its own extremes: either or both, tried in place
/// of the battery's, on a constant the call need not also name in targets,
/// and tried again at the same values when the check is run again.
#[test]
fn a_call_gives_a_constant_its_own_extremes() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    // Zero divides by the capacity; the battery's own low extreme does not.
    let output = run(
        &mut host,
        &mut session,
        json!({
            "tests": ["extreme_conditions"],
            "targets": ["max_rate"],
            "extremes": [{"variable": "Capacity", "low": 0}]
        }),
    );
    assert_eq!(
        summary(&output, "extreme_conditions"),
        &json!({"test": "extreme_conditions", "checks": 5, "passed": 4, "failed": 1}),
        "the model's own run, the named target's extremes, and the capacity's: the one given \
         and the battery's high"
    );
    let low = result(&output, "extreme_conditions", "capacity", "low");
    assert_eq!(low["extreme"], "given");
    assert_eq!(low["value"], 0.0);

    // Both, on a named target: only its extremes are the call's.
    let output = run(
        &mut host,
        &mut session,
        json!({
            "tests": ["extreme_conditions"],
            "targets": ["capacity"],
            "extremes": [{"variable": "capacity", "low": 0, "high": 0}]
        }),
    );
    assert_eq!(
        summary(&output, "extreme_conditions"),
        &json!({"test": "extreme_conditions", "checks": 3, "failed": 2, "passed": 1})
    );
    let high = result(&output, "extreme_conditions", "capacity", "high");
    assert_eq!(high["extreme"], "given");
    assert_eq!(high["value"], 0.0);

    // An extreme at the constant's own value is no check.
    let output = run(
        &mut host,
        &mut session,
        json!({
            "tests": ["extreme_conditions"],
            "extremes": [{"variable": "capacity", "low": 100, "high": 100}]
        }),
    );
    assert_eq!(summary(&output, "extreme_conditions")["checks"], 1);

    // What a call may not give.
    for (extremes, refusal) in [
        (
            json!([{"variable": "capacity"}]),
            "extremes gives 'capacity' neither a low nor a high",
        ),
        (
            json!([{"variable": "capacity", "low": 5, "high": 1}]),
            "the low extreme of 'capacity', 5, is above its high one, 1",
        ),
        (
            Value::Array(
                (0..=MAX_TARGETS)
                    .map(|i| json!({"variable": format!("v{i}"), "low": 0}))
                    .collect(),
            ),
            "extremes names at most 12 variables (this call names 13)",
        ),
    ] {
        let refused = host.refuse(&mut session, "run_tests", json!({"extremes": extremes}));
        let error = refused["error"].as_str().unwrap();
        assert!(error.starts_with(refusal), "{error}");
    }
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions"], "extremes": [{"variable": "capacty", "low": 0}]}),
    );
    assert_eq!(output["notFound"][0]["suggestions"][0], "capacity");

    // JSON has no number a run cannot hold; a caller in Rust can give one.
    let input = RunTestsInput {
        tests: vec![],
        targets: vec![],
        record: vec![],
        extremes: vec![ExtremeInput {
            variable: "capacity".to_string(),
            low: Some(f64::NAN),
            high: None,
        }],
    };
    let refused = run_tests(&mut session, &mut host.workspace(), input);
    assert!(refused.is_err_and(|err| {
        err.error == "the extremes of 'capacity' must be numbers a run can hold"
    }));
}

/// The default targets are the constants that feed a flow, those that reach
/// the most stocks first; a disturbance steps the first few that are not
/// zero.
#[test]
fn a_disturbance_steps_by_default_the_constants_that_reach_the_most_stocks() {
    // Constants a to e feed one stock each, and `a` is zero; `wide` feeds
    // two; `unused` feeds nothing.
    let mut project = TestProject::new("wide")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("first", "1", &["filling"], &[], None)
        .stock("second", "1", &["topping"], &[], None)
        .flow("filling", "a + b + c + d + e + wide", None)
        .flow("topping", "wide", None)
        .aux("wide", "1", None)
        .aux("unused", "1", None);
    for (name, value) in [("a", "0"), ("b", "1"), ("c", "1"), ("d", "1"), ("e", "1")] {
        project = project.aux(name, value, None);
    }
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(&mut host, &mut session, json!({"tests": ["disturbance"]}));
    let stepped: Vec<&str> = output["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["variable"].as_str().unwrap())
        .collect();
    assert_eq!(stepped.len(), MAX_DEFAULT_DISTURBANCES);
    assert_eq!(stepped, ["wide", "b", "c"], "{output}");
    // A step is a tenth of the value, from a tenth of the way into the run.
    let step = &output["results"][0];
    assert_eq!(step["value"], 1.0 + STEP_FRACTION);
    assert_eq!(step["fromTime"], 1.0);

    // The other targeted tests take every one, in the same order.
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions"]}),
    );
    assert_eq!(
        summary(&output, "extreme_conditions")["checks"],
        1 + 5 * 2,
        "the model's own run and five constants at both extremes; zero is at both already"
    );
}

/// A test that would compare nothing is skipped saying why, never passed: a
/// model whose stocks are all inside modules has nothing the run-comparing
/// tests read yet.
#[test]
fn a_test_that_would_compare_nothing_is_skipped_saying_why() {
    let mut host = Host::new(corpus_model(
        "test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile",
    ));
    let output = run(&mut host, &mut Session::new("main"), json!({}));
    for test in [
        "extreme_conditions",
        "integration_error",
        "sensitivity",
        "disturbance",
    ] {
        let tested = summary(&output, test);
        assert_eq!(tested["checks"], 0, "{test}: {output}");
        assert!(
            tested["skipped"]
                .as_str()
                .is_some_and(|reason| reason.contains("inside modules")),
            "{test}: {output}"
        );
    }
}

/// A call gives one constant's extremes in one entry: a second entry for it,
/// under any spelling, is refused, as is a low above the high.
#[test]
fn a_calls_extremes_name_each_constant_once() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    for (extremes, refusal) in [
        (
            json!([{"variable": "capacity", "low": 0}, {"variable": "Capacity", "high": 0}]),
            "extremes names 'capacity' twice: give its low and high in one entry",
        ),
        (
            json!([{"variable": "capacity", "low": 5, "high": 1}]),
            "the low extreme of 'capacity', 5, is above its high one, 1",
        ),
    ] {
        let refused = host.refuse(&mut session, "run_tests", json!({"extremes": extremes}));
        assert_eq!(refused["error"], refusal, "{refused}");
    }
}

/// A damped oscillation that a change turns into a growing one has lost its
/// stability, and the response shows it in its damping. A third-order loop
/// of lags 2 is stable below a gain of 8.
#[test]
fn a_response_shows_an_oscillation_that_grows() {
    let project = TestProject::new("loop")
        .with_sim_time(0.0, 60.0, 0.0625)
        .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
        .stock("a", "0", &["da"], &[], None)
        .stock("b", "0", &["db"], &[], None)
        .stock("c", "0", &["dc"], &[], None)
        .flow("da", "(gain * (goal - c) - a) / lag", None)
        .flow("db", "(a - b) / lag", None)
        .flow("dc", "(b - c) / lag", None)
        .aux("gain", "5", None)
        .aux("goal", "10", None)
        .aux("lag", "2", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "targets": ["gain"], "record": ["c"]}),
    );
    let double = result(&output, "sensitivity", "gain", "double");
    assert_eq!(
        (
            &double["responses"][0]["damping"],
            &double["responses"][0]["wasDamping"]
        ),
        (&json!("growing"), &json!("damped")),
        "{double}"
    );
    // At half the gain it is damped in both runs, which the response says
    // once.
    let half = result(&output, "sensitivity", "gain", "half");
    assert_eq!(half["responses"][0]["damping"], "damped", "{half}");
    assert_eq!(half["responses"][0].get("wasDamping"), None, "{half}");
}

/// A series that barely moves beside its level in both runs has no change of
/// behavior to report, however much a check shifts its level: a reservoir of
/// 100 that trickles up by a thousandth, and down when its starting level
/// halves.
#[test]
fn a_change_of_family_needs_the_series_to_move() {
    let project = TestProject::new("reservoir")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "start", &["trickle"], &[], None)
        .flow("trickle", "0.0001 * (start - 75) / 25", None)
        .aux("start", "100", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "targets": ["start"]}),
    );
    let half = result(&output, "sensitivity", "start", "half");
    let response = &half["responses"][0];
    assert!(
        response["largestChange"].as_f64().unwrap() > MATERIAL_CHANGE,
        "{half}"
    );
    assert_ne!(response["mode"], response["was"], "the premise: {half}");
    assert_eq!(half["outcome"], "passed", "{half}");
    assert_eq!(response.get("family"), None, "{half}");
}

/// A check that makes a series overflow is read beside the model's run at
/// the model's run's own scale: the check's last numbers before infinity
/// would make the model's growth residue beside them.
#[test]
fn a_series_a_check_overflows_keeps_the_models_own_behavior() {
    let project = TestProject::new("compound")
        .with_sim_time(0.0, 1600.0, 1.0)
        .stock("x", "1", &["growth"], &[], None)
        .flow("growth", "r * x", None)
        .aux("r", "0.3", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "targets": ["r"]}),
    );
    let double = result(&output, "sensitivity", "r", "double");
    let response = &double["responses"][0];
    assert_eq!(response["mode"], "undefined", "the premise: {double}");
    assert_eq!(response["was"], "exponential", "{double}");
}

/// A series that moves in only one of the two runs has changed behavior when
/// it moves by MATERIAL_CHANGE of its scale or more: a level at rest that
/// falls 5 from 100 at double the constant is flagged, and one that rises
/// 2.5 at half is not.
#[test]
fn a_series_at_rest_that_starts_to_move_has_changed_behavior() {
    let project = TestProject::new("leak")
        .with_sim_time(0.0, 5.0, 1.0)
        .stock("level", "100", &[], &["leaking"], None)
        .flow("leaking", "c - 1", None)
        .aux("c", "1", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "targets": ["c"]}),
    );
    let double = result(&output, "sensitivity", "c", "double");
    assert_eq!(double["outcome"], "flagged", "{double}");
    assert_eq!(double["responses"][0]["largestChange"], 0.05, "{double}");
    assert_eq!(
        result(&output, "sensitivity", "c", "half")["outcome"],
        "passed",
        "{output}"
    );
}

/// What the model's own run shows is listed first, ahead of failures, and is
/// listed however many failures the other checks find: here more than the
/// list holds, each constant taking a logarithm of zero at both extremes.
#[test]
fn the_models_own_run_is_listed_first() {
    let mut project = shipping(true, "4");
    let mut targets = Vec::new();
    for i in 0..MAX_TARGETS {
        project = project
            .flow(
                &format!("growth_{i}"),
                &format!("0.001 * LN(c_{i}) * LN(10 - c_{i})"),
                None,
            )
            .stock(
                &format!("grown_{i}"),
                "0",
                &[&format!("growth_{i}")],
                &[],
                None,
            )
            .aux(&format!("c_{i}"), "1", None);
        targets.push(format!("c_{i}"));
    }
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"], "targets": targets}),
    );
    let failed = summary(&output, "extreme_conditions")["failed"]
        .as_u64()
        .unwrap();
    assert!(failed as usize > MAX_RESULTS, "{output}");
    assert_eq!(output["results"][0]["condition"], "own_run", "{output}");
    assert_eq!(output["results"][0]["outcome"], "flagged", "{output}");
    assert_eq!(output["results"][1]["outcome"], "failed", "{output}");
}

/// Sensitivity checks that flag the same change of the same series are one
/// finding: listed once, the strongest, naming the others. Half the duration
/// and half the contact infectivity both end SIR's epidemic.
#[test]
fn checks_that_flag_the_same_change_are_listed_once() {
    let mut host = Host::new(corpus_model("test/test-models/samples/SIR/SIR.mdl"));
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(&mut host, &mut session, json!({"tests": ["sensitivity"]}));
    assert_eq!(summary(&output, "sensitivity")["flagged"], 2, "{output}");
    let flagged: Vec<&Value> = output["results"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["outcome"] == "flagged")
        .collect();
    assert_eq!(flagged.len(), 1, "{output}");
    // Half the duration changes infectious more, and is the one listed.
    assert_eq!(flagged[0]["variable"], "duration", "{output}");
    assert_eq!(
        flagged[0]["sameChange"],
        json!([{"variable": "contact_infectivity", "condition": "half"}]),
        "{output}"
    );
}

/// At most MAX_CONFIRMATIONS checks are run again at a finer DT, and the
/// rest say they were not: one drain more than the limit, each at ten times
/// its rate overshooting zero at the model's DT and not at a tenth of it.
#[test]
fn checks_are_confirmed_at_a_finer_dt_up_to_their_limit() {
    let mut project = TestProject::new("drains").with_sim_time(0.0, 10.0, 0.25);
    for i in 0..=MAX_CONFIRMATIONS {
        project = project
            .stock(
                &format!("tank_{i}"),
                "100",
                &[],
                &[&format!("draining_{i}")],
                None,
            )
            .flow(
                &format!("draining_{i}"),
                &format!("tank_{i} * rate_{i}"),
                None,
            )
            .aux(&format!("rate_{i}"), "0.5", None);
    }
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"]}),
    );
    let note = summary(&output, "extreme_conditions")["note"]
        .as_str()
        .unwrap_or_else(|| panic!("{output}"))
        .to_string();
    assert!(
        note.starts_with(&format!(
            "{MAX_CONFIRMATIONS} checks found something at the model's DT and nothing at a tenth"
        )),
        "{note}"
    );
    assert!(
        note.contains("1 check that found something was not run again at a finer DT"),
        "{note}"
    );
}

/// A leg of exactly the materiality threshold is material: it is folded
/// only when it is smaller.
#[test]
fn a_leg_of_exactly_the_threshold_is_material() {
    assert_eq!(material_legs(&[0.0, 1.0, 0.75], &[1], 0.25), [1.0, -0.25]);
    assert_eq!(material_legs(&[0.0, 1.0, 0.76], &[1], 0.25), [0.76]);
}

/// The model's run is read at the shared scale too: where residue is in the
/// model's run (`k = 0.02` leaves 2.8e-17 a unit of time) and none in the
/// check's (half of it balances exactly), the stock is at rest in both.
#[test]
fn residue_in_the_models_run_is_read_at_the_shared_scale() {
    let project = TestProject::new("balanced")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "0", &["filling"], &["draining"], None)
        .flow("filling", "k + 0.2", None)
        .flow("draining", "(10 * k + 2) / 10", None)
        .stock("mover", "0", &["moving"], &[], None)
        .flow("moving", "k", None)
        .aux("k", "0.02", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["sensitivity"], "targets": ["k"]}),
    );
    let half = result(&output, "sensitivity", "k", "half");
    let level = half["responses"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["variable"] == "level")
        .unwrap_or_else(|| panic!("{half}"));
    assert_eq!(level["mode"], "at_rest", "{half}");
    assert_eq!(level.get("was"), None, "{half}");
}

/// Within an outcome, the listing takes the tests in their order, whatever
/// order their checks came in; and a sensitivity check that passed with no
/// response at all is not among the strongest.
#[test]
fn the_listing_orders_the_tests_and_lists_only_responses_that_moved() {
    let check = |test: TestName, outcome: Outcome, strength: f64| {
        let mut check = Check::new(test, None, None);
        check.result.outcome = outcome;
        check.strength = strength;
        check
    };
    let (shown, _) = listed(vec![
        check(TestName::Disturbance, Outcome::Observed, 0.0),
        check(TestName::Disturbance, Outcome::Observed, 0.0),
        check(TestName::LoopKnockout, Outcome::Observed, 0.0),
        check(TestName::LoopKnockout, Outcome::Observed, 0.0),
        check(TestName::Sensitivity, Outcome::Passed, 0.0),
        check(TestName::Sensitivity, Outcome::Passed, 0.5),
    ]);
    let order: Vec<(TestName, Outcome)> = shown
        .iter()
        .map(|c| (c.key.test, c.result.outcome))
        .collect();
    assert_eq!(
        order,
        [
            (TestName::LoopKnockout, Outcome::Observed),
            (TestName::LoopKnockout, Outcome::Observed),
            (TestName::Disturbance, Outcome::Observed),
            (TestName::Disturbance, Outcome::Observed),
            (TestName::Sensitivity, Outcome::Passed),
        ]
    );
}

/// A corpus model by its path from the repository's root.
fn corpus_model(path: &str) -> datamodel::Project {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(path);
    let bytes = std::fs::read(&full).unwrap_or_else(|err| panic!("{path}: {err}"));
    if path.ends_with(".mdl") {
        crate::compat::open_vensim(&String::from_utf8_lossy(&bytes))
            .unwrap_or_else(|err| panic!("{path}: {err}"))
    } else {
        crate::compat::open_xmile(&mut std::io::BufReader::new(&bytes[..]))
            .unwrap_or_else(|err| panic!("{path}: {err}"))
    }
}

/// The labeled corpus (`battery_corpus.json`): for each model, every check
/// the battery lists that did not pass, with whether a modeler reading it
/// would accept it as worth their attention, and why. The labels are one
/// reviewer's judgment, each with its reason, so a reader can disagree with a
/// specific one and change it.
#[derive(serde::Deserialize, serde::Serialize)]
struct LabeledCorpus {
    models: Vec<LabeledModel>,
}

#[derive(serde::Deserialize, serde::Serialize)]
struct LabeledModel {
    path: String,
    /// Whether the model is small enough for the default suite.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    small: bool,
    checks: Vec<LabeledCheck>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, PartialEq)]
struct LabeledCheck {
    test: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    variable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    condition: Option<String>,
    /// The value the check tried, so a rule that changes it cannot keep a
    /// label written for another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<f64>,
    outcome: String,
    /// Whether the check tells a modeler something about the model.
    accept: bool,
    why: String,
}

const CORPUS_LABELS: &str = "src/tools/battery_corpus.json";

/// The most checks the battery lists over the labeled corpus that a modeler
/// would not accept, and the fewest they would: a ratchet on the labels, which
/// every listed check must have. A change to the battery that lists another
/// check needs its label, and these bounds keep the new labels from adding a
/// false alarm or losing a finding. The rejected labels are of four kinds,
/// each a limit of the battery its `why` names: a start-up transient that a
/// parameter deepens until the classifier counts it as a movement; an
/// oscillation whose period moved past the run's horizon; a constant that is a definition or a
/// reference rather than a condition of the system (a unit carrier, a
/// reference concentration, an assignment matrix); and an integration
/// artifact of the model's own run that a change removes or amplifies.
const MAX_CORPUS_FALSE_ALARMS: usize = 16;
/// The corpus models, every check each lists labeled.
const CORPUS_MODELS: usize = 29;
const MIN_CORPUS_ACCEPTED: usize = 52;

/// Run the battery on each labeled model `include` picks, as an agent's call
/// with no arguments does, and hold what it lists against the labels: every
/// listed check that did not pass has a label, and every label a listed
/// check. Returns (models run, listed, accepted).
///
/// With `UPDATE_BATTERY_LABELS=1` the labels file is rewritten to what the
/// battery lists, keeping the labels it has; a check it adds has an empty
/// `why`, which this refuses until someone has judged it.
fn check_labeled_corpus(include: impl Fn(&LabeledModel) -> bool) -> (usize, usize, usize) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(CORPUS_LABELS);
    let text = std::fs::read_to_string(&path).expect("the labels are checked in");
    let mut corpus: LabeledCorpus = serde_json::from_str(&text).expect("the labels parse");
    let update = std::env::var_os("UPDATE_BATTERY_LABELS").is_some();
    let mut problems: Vec<String> = Vec::new();
    let mut table = String::new();
    let (mut listed, mut accepted) = (0, 0);
    for model in corpus.models.iter_mut().filter(|model| include(model)) {
        let mut host = Host::new(corpus_model(&model.path));
        let mut session = Session::new("main");
        let output = run(&mut host, &mut session, json!({}));
        let text = |value: &Value| value.as_str().map(str::to_string);
        let found: Vec<LabeledCheck> = output["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| ["failed", "flagged", "not_run"].contains(&r["outcome"].as_str().unwrap()))
            .map(|r| LabeledCheck {
                test: text(&r["test"]).unwrap(),
                variable: text(&r["variable"]),
                condition: text(&r["condition"]),
                value: r["value"].as_f64(),
                outcome: text(&r["outcome"]).unwrap(),
                accept: false,
                why: String::new(),
            })
            .collect();
        let same = |a: &LabeledCheck, b: &LabeledCheck| {
            (&a.test, &a.variable, &a.condition, a.value, &a.outcome)
                == (&b.test, &b.variable, &b.condition, b.value, &b.outcome)
        };
        let labeled: Vec<LabeledCheck> = found
            .iter()
            .map(|check| {
                model
                    .checks
                    .iter()
                    .find(|label| same(label, check))
                    .cloned()
                    .unwrap_or_else(|| check.clone())
            })
            .collect();
        for label in &model.checks {
            if !found.iter().any(|check| same(label, check)) {
                problems.push(format!(
                    "{}: a label for a check the battery does not list: {} {:?} {:?} {}",
                    model.path, label.test, label.variable, label.condition, label.outcome
                ));
            }
        }
        for check in labeled.iter().filter(|check| check.why.is_empty()) {
            let result = output["results"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| {
                    text(&r["test"]).as_ref() == Some(&check.test)
                        && text(&r["variable"]) == check.variable
                        && text(&r["condition"]) == check.condition
                })
                .map(Value::to_string)
                .unwrap_or_default();
            problems.push(format!(
                "{}: a listed check with no label: {result}",
                model.path
            ));
        }
        let (mut checks, mut failed, mut flagged) = (0, 0, 0);
        for summary in output["tests"].as_array().unwrap() {
            let count = |field: &str| summary[field].as_u64().unwrap_or(0);
            checks += count("checks");
            failed += count("failed");
            flagged += count("flagged");
        }
        let informative = labeled.iter().filter(|check| check.accept).count();
        table.push_str(&format!(
            "{:<72} checks {checks:>4}  failed {failed:>3}  flagged {flagged:>3}  listed {:>2}  \
             accepted {informative:>2}\n",
            model.path,
            labeled.len(),
        ));
        for summary in output["tests"].as_array().unwrap() {
            if let Some(note) = summary["note"].as_str() {
                table.push_str(&format!("    {}: {note}\n", summary["test"]));
            }
        }
        if let Some(note) = output["leftOut"].as_str() {
            table.push_str(&format!("    left out: {note}\n"));
        }
        listed += labeled.len();
        accepted += informative;
        model.checks = labeled;
    }
    eprintln!("{table}listed {listed}, accepted {accepted}");
    if update {
        let mut text = serde_json::to_string_pretty(&corpus).unwrap();
        text.push('\n');
        std::fs::write(&path, text).expect("the labels are written");
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
    let models = corpus.models.iter().filter(|model| include(model)).count();
    (models, listed, accepted)
}

/// What the battery lists for an agent is worth a modeler's attention: over
/// the labeled corpus, every listed check that did not pass has been judged,
/// and the ones a modeler would not accept are few.
///
/// Run with: cargo test --release -p simlin-engine --lib -- --ignored
/// the_batterys_findings_over_the_corpus_are_ones_a_modeler_accepts
#[test]
#[ignore = "runs the battery on 29 corpus models, World3 and C-LEARN among them; run under the \
            gates profile"]
fn the_batterys_findings_over_the_corpus_are_ones_a_modeler_accepts() {
    let (models, listed, accepted) = check_labeled_corpus(|_| true);
    assert_eq!(models, CORPUS_MODELS, "the labeled corpus's models");
    let rejected = listed - accepted;
    assert!(
        rejected <= MAX_CORPUS_FALSE_ALARMS,
        "{rejected} listed checks a modeler would not accept, of {listed}"
    );
    assert!(
        accepted >= MIN_CORPUS_ACCEPTED,
        "{accepted} of {listed} listed checks accepted"
    );
}

/// The labeled corpus's small models, in the default suite: the same check
/// of what is listed against the labels.
#[test]
fn the_batterys_findings_on_small_corpus_models_match_their_labels() {
    let (models, listed, _) = check_labeled_corpus(|model| model.small);
    assert_eq!(models, 3, "teacup, water and logistic growth");
    assert!(listed > 0, "the small models list something to judge");
}
