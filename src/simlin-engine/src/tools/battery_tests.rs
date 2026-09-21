// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::datamodel::Visibility;
use crate::test_common::TestProject;
use crate::tools::Session;
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

/// The forms a modeler writes a time constant in without units, each tested
/// at DT, where zero would divide by it: the textbook one; the same through
/// algebra that only spells it differently (a product divisor, times one
/// over it, a power of -1, a division by one), which is read as the
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
        let mut host = Host::from_test_project(&project);
        let output = run(
            &mut host,
            &mut Session::new("main"),
            json!({"tests": ["extreme_conditions"], "targets": ["average_lifetime"]}),
        );
        assert_eq!(
            summary(&output, "extreme_conditions"),
            &json!({"test": "extreme_conditions", "checks": 2, "passed": 2}),
            "{equation}: at DT and ten times, no division by zero"
        );
    }
}

/// Logistic growth spelled with the capacity dividing the whole term: the
/// capacity is in its own numerator's sum, so it is a scale, and at zero it
/// divides by zero.
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
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"], "targets": ["capacity"]}),
    );
    let zero = result(&output, "extreme_conditions", "capacity", "zero");
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

#[test]
fn a_time_constant_at_its_low_extreme_is_at_dt_and_a_division_by_zero_fails() {
    let mut host = Host::from_test_project(&workforce());
    let mut session = Session::new("main");
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions"]}),
    );
    assert_eq!(
        summary(&output, "extreme_conditions"),
        &json!({"test": "extreme_conditions", "checks": 2, "passed": 1, "failed": 1})
    );
    let at_dt = result(&output, "extreme_conditions", "tenure", "dt");
    assert_eq!(at_dt["outcome"], "failed");
    assert_eq!(at_dt["value"], 0.25);
    assert_eq!(
        at_dt["timeConstant"], "divides_a_rate",
        "why it was taken for a time constant"
    );
    assert_eq!(
        at_dt["problems"],
        json!([{"kind": "non_finite", "variable": "per_head", "time": 0.25}]),
        "at a tenure of DT the workforce leaves in one step, and the split divides by zero"
    );
}

#[test]
fn a_scale_at_zero_is_at_zero_and_a_carrying_capacity_of_zero_divides_by_it() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["extreme_conditions"]}),
    );
    let zero = result(&output, "extreme_conditions", "capacity", "zero");
    assert_eq!(zero["outcome"], "failed");
    assert_eq!(zero["problems"][0]["kind"], "non_finite");
    assert_eq!(
        summary(&output, "extreme_conditions"),
        &json!({"test": "extreme_conditions", "checks": 4, "passed": 3, "failed": 1}),
        "a growth rate of zero, or ten times either, is a condition the model survives"
    );
}

/// A stock that goes negative is flagged for judgment. One the model marks
/// non-negative is flagged too, saying the engine does not enforce the
/// marking: a tool that does would hold it at zero, so the run is the
/// engine's, not the model's failure.
#[test]
fn a_stock_going_negative_is_flagged_and_an_unenforced_marking_is_said() {
    // Shipments that ignore what is on hand drain a stock below zero at ten
    // times their rate.
    let stocked = |non_negative: bool| {
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
            .aux("orders", "2", None)
            .stock("backlog", "-5", &["more"], &[], None)
            .flow("more", "orders", None)
    };
    for non_negative in [false, true] {
        let mut host = Host::from_test_project(&stocked(non_negative));
        let mut session = Session::new("main");
        let output = run(
            &mut host,
            &mut session,
            json!({"tests": ["extreme_conditions"], "targets": ["orders"]}),
        );
        let check = result(&output, "extreme_conditions", "orders", "ten_times");
        assert_eq!(check["outcome"], "flagged", "{check}");
        let note = check["note"].as_str();
        if non_negative {
            let note = note.unwrap_or_else(|| panic!("{check}"));
            assert!(note.contains("does not enforce"), "{note}");
        } else {
            assert_eq!(note, None, "{check}");
        }
        let mut problem =
            json!({"kind": "goes_negative", "variable": "stock", "time": 1.0, "value": -70.0});
        if non_negative {
            problem["nonNegative"] = json!(true);
        }
        assert_eq!(
            check["problems"],
            json!([problem]),
            "the backlog, negative in the model's run already, is not a problem"
        );
    }
}

#[test]
fn integration_error_fails_when_halving_dt_or_rk4_moves_a_stock_more_than_a_percent() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let output = run(
        &mut host,
        &mut session,
        json!({"tests": ["integration_error"]}),
    );
    for condition in ["half_dt", "rk4"] {
        let check = output["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["condition"] == condition)
            .unwrap();
        assert_eq!(check["outcome"], "failed", "{check}");
        assert_eq!(check["differences"][0]["variable"], "population");
        assert!(check["differences"][0]["difference"].as_f64().unwrap() > INTEGRATION_TOLERANCE);
    }

    // A straight line integrates exactly, and a model under RK4 has one
    // refinement left to try.
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

/// A second-order goal seeker, damped just past critical: goal seeking at
/// its damping, oscillating at half of it.
fn damped() -> TestProject {
    TestProject::new("damped")
        .with_sim_time(0.0, 60.0, 0.0625)
        .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
        .stock("x", "0", &["dx"], &[], None)
        .stock("v", "0", &["dv"], &[], None)
        .flow("dx", "v", None)
        .flow("dv", "0.25 * (goal - x) - damping * v", None)
        .aux("goal", "100", None)
        .aux("damping", "1.2", None)
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
    assert_eq!(half["value"], 0.6);
    assert_eq!(half["responses"][0]["variable"], "x");
    assert_eq!(half["responses"][0]["was"], "goal_seeking");
    assert_ne!(half["responses"][0]["mode"], "goal_seeking");

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
    for test in TestName::ALL {
        let summary = summary(
            &output,
            serde_json::to_value(test).unwrap().as_str().unwrap(),
        );
        if test == TestName::Units {
            assert_eq!(summary["checks"], 1);
        } else {
            let skipped = summary["skipped"].as_str().unwrap();
            assert!(skipped.contains("does not simulate"), "{skipped}");
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

#[test]
fn an_answer_lists_at_most_its_limit_failures_first() {
    // Each constant divides one: at zero, each check fails.
    let mut project = TestProject::new("many").with_sim_time(0.0, 2.0, 1.0).stock(
        "level",
        "1",
        &["growth"],
        &[],
        None,
    );
    let terms: Vec<String> = (0..25).map(|i| format!("1 / c{i}")).collect();
    project = project.flow("growth", &format!("0.001 * ({})", terms.join(" + ")), None);
    for i in 0..25 {
        project = project.aux(&format!("c{i}"), "1", None);
    }
    let mut host = Host::from_test_project(&project);
    let output = run(
        &mut host,
        &mut Session::new("main"),
        json!({"tests": ["extreme_conditions"]}),
    );
    assert_eq!(summary(&output, "extreme_conditions")["failed"], 25);
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

/// The corpus's two largest models are tested within bounds of time and size.
///
/// Run with: cargo test --release -p simlin-engine --features schema --lib --
/// --ignored the_largest_corpus_models_are_tested_within_bounds --nocapture
#[test]
#[ignore = "runs World3 and C-LEARN hundreds of times: minutes on a debug build"]
fn the_largest_corpus_models_are_tested_within_bounds() {
    for path in [
        "../../test/metasd/WRLD3-03/wrld3-03.mdl",
        "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let mdl = std::fs::read_to_string(&path).expect("the corpus model is present");
        let project = crate::compat::open_vensim(&mdl).expect("the corpus model parses");
        let mut host = Host::new(project);
        let mut session = Session::new("main");
        let started = std::time::Instant::now();
        let output = host.call_raw(&mut session, "run_tests", "{}");
        let took = started.elapsed();
        assert!(!output.is_error, "{}", output.json);
        let answer: Value = serde_json::from_str(&output.json).unwrap();
        let summaries: Vec<String> = answer["tests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.to_string())
            .collect();
        eprintln!(
            "{}: {} bytes in {took:?}; {} results listed, {} omitted\n  {}",
            path.display(),
            output.json.len(),
            answer["results"].as_array().unwrap().len(),
            answer["omitted"],
            summaries.join("\n  ")
        );
        assert!(
            output.json.len() <= crate::tools::OUTLINE_BUDGET,
            "{}: {} bytes",
            path.display(),
            output.json.len()
        );
    }
}

#[test]
fn a_change_of_behavior_is_material_between_families_when_the_series_moves() {
    use crate::tools::behavior::{BehaviorMode, Damping, Direction};
    let mode = |kind: ModeKind, direction: Option<Direction>| BehaviorMode {
        kind,
        direction,
        damping: (kind == ModeKind::Oscillation).then_some(Damping::Damped),
        starts_at: None,
        settles_at: None,
    };
    let up = Some(Direction::Rising);
    let down = Some(Direction::Falling);
    let response = |now: BehaviorMode, was: BehaviorMode, largest_change: f64| Response {
        variable: "x".to_string(),
        change: Some(0.0),
        largest_change: Some(largest_change),
        mode: now.kind,
        was: (now.kind != was.kind).then_some(was.kind),
        changed_family: changed_family(&now, &was),
        went_undefined: false,
    };
    use ModeKind::*;
    for (now, was, largest, material_change) in [
        (mode(Oscillation, None), mode(GoalSeeking, up), 0.2, true),
        (
            mode(Oscillation, None),
            mode(GoalSeeking, up),
            MATERIAL_CHANGE,
            true,
        ),
        // A label that flips while the series barely moves is the
        // classifier's boundary.
        (mode(Oscillation, None), mode(GoalSeeking, up), 0.01, false),
        // Growth read as linear or exponential over a horizon is one family.
        (mode(Linear, up), mode(Exponential, up), 0.5, false),
        (mode(SShaped, up), mode(GoalSeeking, up), 0.5, false),
        // A goal seeker whose goal moved past its start still seeks it.
        (mode(GoalSeeking, down), mode(GoalSeeking, up), 0.5, false),
        // Growth turned to decline, a turn, or stillness to motion is not.
        (mode(Exponential, down), mode(Exponential, up), 0.5, true),
        (mode(Linear, down), mode(SShaped, up), 0.5, true),
        (mode(GoalSeeking, down), mode(SShaped, up), 0.5, true),
        (mode(Overshoot, up), mode(GoalSeeking, up), 0.5, true),
        (mode(GoalSeeking, up), mode(AtRest, None), 0.5, true),
        // Unnamed modes are the classifier's indecision.
        (mode(SShaped, up), mode(Other, None), 0.5, false),
        (mode(Undefined, None), mode(GoalSeeking, up), 0.5, false),
        (mode(GoalSeeking, up), mode(GoalSeeking, up), 0.9, false),
    ] {
        assert_eq!(
            material(&response(now, was, largest)),
            material_change,
            "{:?} from {:?}, {largest}",
            now.kind,
            was.kind
        );
    }
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
        json!({"targets": ["tenure", "budget", "head_count"]}),
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
    assert!(distinct.len() >= 3, "the fixture mixes outcomes: {output}");
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
    for test in ["extreme_conditions", "sensitivity"] {
        let summary = summary(&output, test);
        let note = summary["note"]
            .as_str()
            .unwrap_or_else(|| panic!("{summary}"));
        assert!(note.contains("1 unit conversion (ton_per_mton)"), "{note}");
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
        2,
        "{named}"
    );
    assert_eq!(summary(&named, "extreme_conditions")["note"], Value::Null);
}

/// The roles the battery gives `project`'s constants.
fn roles_of(project: &TestProject) -> Roles {
    let mut host = Host::from_test_project(project);
    let mut session = Session::new("main");
    let model = host.project.models[0].clone();
    let base = session
        .runs
        .current(&mut host.workspace(), &model)
        .ok()
        .unwrap();
    let ws = host.workspace();
    let resolved = resolve_model(ws.project, ws.db, "main").ok().unwrap();
    let graph = Graph::of(ws.db, &resolved);
    let units = Units::of(&ws, &resolved);
    let parsed = Parsed::of(resolved.model);
    Roles::of(resolved.model, &graph, &units, &parsed, &base)
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
    let roles = roles_of(&project);
    let mut shares: Vec<(&str, f64)> = roles
        .shares
        .iter()
        .map(|(name, whole)| (name.as_str(), *whole))
        .collect();
    shares.sort_by(|a, b| a.0.cmp(b.0));
    assert_eq!(
        shares,
        [
            ("kept", 1.0),
            ("path_share", 1.0),
            ("spare", 1.0),
            ("tax", 100.0)
        ],
        "not an elasticity no unit or name calls a share, a share of people, or one past its whole"
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
    let whole = result(&output, "extreme_conditions", "path_share", "whole");
    assert_eq!(whole["value"], 1.0);
    assert_eq!(
        whole["outcome"], "failed",
        "the ratio divides by zero there"
    );
    assert!(!output.to_string().contains("ten_times"), "{output}");
    let double = result(&output, "sensitivity", "path_share", "double");
    assert_eq!(double["value"], 1.0, "doubled to at most the whole");
}

/// A series already not a number in the model's own run fails no check: a
/// check cannot make it so. The test says it left it out.
#[test]
fn a_series_undefined_in_the_models_own_run_is_not_judged() {
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
    let summary = summary(&output, "extreme_conditions");
    assert_eq!(summary["failed"], Value::Null, "{output}");
    assert!(summary["passed"].as_u64().unwrap() > 0, "{output}");
    let note = summary["note"].as_str().unwrap();
    assert!(
        note.contains("2 series are not a number in the model's own run"),
        "{note}"
    );
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
        json!({"tests": ["extreme_conditions"], "targets": ["length"]}),
    );
    let zero = result(&output, "extreme_conditions", "length", "zero");
    let problem = &zero["problems"][0];
    assert_eq!(problem["kind"], "non_finite", "{zero}");
    assert!(problem["time"].as_f64().unwrap() <= 2.0, "{zero}");
}

/// A constant at zero is at its low extreme already, and ten times zero is
/// zero: neither is a check. A time constant at zero is still tried at DT.
#[test]
fn a_zero_constant_is_no_check_but_a_zero_time_constant_is_tried_at_dt() {
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
    let output = host.call(
        &mut session,
        "run_tests",
        json!({"tests": ["extreme_conditions"], "targets": ["idle"]}),
    );
    assert_eq!(
        summary(&output, "extreme_conditions")["checks"],
        0,
        "{output}"
    );
    let output = host.call(
        &mut session,
        "run_tests",
        json!({"tests": ["extreme_conditions"], "targets": ["drain_time"]}),
    );
    assert_eq!(
        summary(&output, "extreme_conditions")["checks"],
        1,
        "{output}"
    );
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
        .find(|r| r["condition"] == "half_dt")
        .unwrap_or_else(|| panic!("{output}"));
    assert_eq!(half["outcome"], "failed", "{half}");
    assert_eq!(half["differences"][0]["variable"], "capital[e12]", "{half}");
}

/// An answer keeps to the session's budget, leaving out its last listed
/// checks and counting them, and gives the checks it leaves out no id.
#[test]
fn an_answer_keeps_to_its_budget_and_gives_what_it_leaves_out_no_id() {
    let mut project = TestProject::new("many").with_sim_time(0.0, 2.0, 1.0).stock(
        "level",
        "1",
        &["growth"],
        &[],
        None,
    );
    let terms: Vec<String> = (0..6).map(|i| format!("1 / c{i}")).collect();
    project = project.flow("growth", &format!("0.001 * ({})", terms.join(" + ")), None);
    for i in 0..6 {
        project = project.aux(&format!("c{i}"), "1", None);
    }
    let mut host = Host::from_test_project(&project);
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
        if time_constant {
            assert_eq!(
                tested["failed"],
                Value::Null,
                "{target} at DT holds: {output}"
            );
        } else {
            assert_eq!(
                result(&output, "extreme_conditions", target, "zero")["outcome"],
                "failed",
                "{target} at zero divides by zero: {output}"
            );
        }
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
    assert_eq!(tested["checks"], 2, "only residence_time: {output}");
    let note = tested["note"].as_str().unwrap();
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
