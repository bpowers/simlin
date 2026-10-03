// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

fn experiment(host: &mut Host, session: &mut Session, input: Value) -> Value {
    host.call(session, "run_experiment", input)
}

fn comparison<'a>(output: &'a Value, variable: &str) -> &'a Value {
    output["behavior"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["variable"] == variable)
        .unwrap_or_else(|| panic!("{variable} is recorded: {output}"))
}

fn samples(host: &mut Host, session: &mut Session, variable: &str, run: &str) -> Vec<f64> {
    let output = host.call(
        session,
        "read_behavior",
        json!({"variables": [variable], "runs": [run]}),
    );
    output["series"][0]["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["value"].as_f64().unwrap())
        .collect()
}

#[test]
fn a_value_change_is_applied_reported_and_kept_under_its_name() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "slow adjustment", "set": [{"variable": "adjustment time", "value": 8}]}),
    );
    assert_eq!(output["run"], "slow adjustment");
    assert_eq!(output["from"], "current");
    assert_eq!(
        output["applied"],
        json!([{"variable": "adjustment_time", "value": 8.0, "was": 2.0}])
    );
    // The stocks are recorded by default, beside the run they started from.
    let inventory = comparison(&output, "Inventory");
    assert!(inventory["this"]["mode"]["kind"].is_string());
    assert!(inventory["base"]["end"].is_number());
    // The run is kept: another tool reads it by name.
    assert_eq!(
        samples(&mut host, &mut session, "Inventory", "slow adjustment").len(),
        12
    );
}

#[test]
fn a_multiplier_scales_the_value_in_the_run_it_starts_from() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "longer coverage", "set": [{"variable": "coverage", "multiply": 1.5}]}),
    );
    assert_eq!(
        output["applied"][0],
        json!({"variable": "coverage", "value": 6.0, "was": 4.0})
    );

    // Chained: a multiplier on a run that already changed the constant
    // multiplies that run's value.
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "longer still", "from": "longer coverage",
               "set": [{"variable": "coverage", "multiply": 2}]}),
    );
    assert_eq!(
        output["applied"][0],
        json!({"variable": "coverage", "value": 12.0, "was": 6.0})
    );
}

#[test]
fn an_arrayed_constants_elements_each_take_the_change() {
    let project = TestProject::new("arrayed")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_with_ranges("capacity[region]", vec![("north", "10"), ("south", "20")])
        .array_stock("stock[region]", "0", &["inflow"], &[], None)
        .array_flow("inflow[region]", "capacity", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "doubled", "set": [{"variable": "capacity", "multiply": 2}]}),
    );
    assert_eq!(
        output["applied"][0]["elements"],
        json!([
            {"element": "north", "value": 20.0, "was": 10.0},
            {"element": "south", "value": 40.0, "was": 20.0}
        ])
    );
    let north = comparison(&output, "stock[north]");
    assert_eq!(north["this"]["end"], 200.0);
    assert_eq!(north["base"]["end"], 100.0);
}

#[test]
fn an_equation_change_runs_on_a_copy_and_leaves_the_model_as_it_was() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let before = host.call(&mut session, "read_model", json!({}));
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "fixed production", "set": [{"variable": "production", "equation": "10"}]}),
    );
    assert_eq!(
        output["applied"][0],
        json!({"variable": "production", "equation": "10",
               "wasEquation": "MAX(0, orders + (desired_inventory - Inventory) / adjustment_time)"})
    );
    let after = host.call(&mut session, "read_model", json!({}));
    assert_eq!(after["flows"], before["flows"], "the model is unchanged");
    assert_eq!(after["revision"], before["revision"]);
    assert!(after.get("changes").is_none());

    // The db is back where it was: the model as it is still runs as before.
    let current = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["production"]}),
    );
    assert_ne!(current["series"][0]["end"], 10.0);
}

#[test]
fn a_value_from_a_time_on_leaves_the_run_as_it_was_before_that_time() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "late", "fromTime": 10, "set": [{"variable": "coverage", "value": 8}],
               "record": ["desired_inventory"]}),
    );
    assert_eq!(output["applied"][0]["fromTime"], 10.0);
    let late = samples(&mut host, &mut session, "desired_inventory", "late");
    let base = samples(&mut host, &mut session, "desired_inventory", "current");
    // Twelve samples over 0..20: the first five fall before t = 10.
    assert_eq!(&late[..5], &base[..5]);
    assert!(late[11] > base[11], "{late:?} {base:?}");
}

#[test]
fn a_run_that_starts_from_another_keeps_its_changes_unless_it_changes_the_same_variables() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    experiment(
        &mut host,
        &mut session,
        json!({"name": "a", "set": [{"variable": "coverage", "value": 8},
                                     {"variable": "adjustment_time", "value": 4}]}),
    );
    let b = experiment(
        &mut host,
        &mut session,
        json!({"name": "b", "from": "a", "set": [{"variable": "coverage", "value": 2}],
               "record": ["desired_inventory"]}),
    );
    assert_eq!(b["from"], "a");
    // coverage is b's (2, against a's 8); adjustment_time is still a's.
    assert_eq!(
        b["applied"][0],
        json!({"variable": "coverage", "value": 2.0, "was": 8.0})
    );
    let desired = comparison(&b, "desired_inventory");
    assert!(desired["this"]["end"].as_f64() < desired["base"]["end"].as_f64());
    let a_adjust = samples(&mut host, &mut session, "adjustment_time", "b");
    assert!(a_adjust.iter().all(|&v| v == 4.0), "{a_adjust:?}");
}

/// A change from a time on, over a run that changed the same constant
/// earlier, keeps the earlier change until that time: the run is the one it
/// started from up to there, and the answer's `was` is the value the constant
/// has in this run just before the change. A multiplier is of that value, the
/// constant's in the starting run at the time the change takes effect.
#[test]
fn a_timed_change_over_a_run_keeps_what_that_run_set_before_it() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    experiment(
        &mut host,
        &mut session,
        json!({"name": "base", "set": [{"variable": "coverage", "value": 8}]}),
    );
    // The model's coverage is 4; base's is 8 from the start.
    let late = experiment(
        &mut host,
        &mut session,
        json!({"name": "late", "from": "base", "fromTime": 10,
               "set": [{"variable": "coverage", "multiply": 2}]}),
    );
    assert_eq!(
        late["applied"][0],
        json!({"variable": "coverage", "value": 16.0, "was": 8.0, "fromTime": 10.0})
    );
    let coverage = samples(&mut host, &mut session, "coverage", "late");
    // Twelve samples over 0..20: the first five fall before t = 10.
    assert!(coverage[..5].iter().all(|&v| v == 8.0), "{coverage:?}");
    assert_eq!(coverage[11], 16.0, "{coverage:?}");

    // Over `late`, whose coverage is 8 until 10 and 16 after: a multiplier
    // from 15 is of 16, and one from 5 of 8.
    for (from_time, was) in [(15.0, 16.0), (5.0, 8.0)] {
        let again = experiment(
            &mut host,
            &mut session,
            json!({"name": "again", "from": "late", "fromTime": from_time,
                   "set": [{"variable": "coverage", "multiply": 0.5}]}),
        );
        assert_eq!(again["applied"][0]["was"], was, "from {from_time}");
        assert_eq!(again["applied"][0]["value"], was / 2.0, "from {from_time}");
    }
    // The change from 5 replaced late's change at 10, which came after it,
    // and kept base's from the start.
    let coverage = samples(&mut host, &mut session, "coverage", "again");
    assert_eq!(coverage[0], 8.0, "{coverage:?}");
    assert!(coverage[3..].iter().all(|&v| v == 4.0), "{coverage:?}");
}

/// A value from a time is set on the variable as the starting run has it. A
/// run that replaced the variable's equation with one that is not a number
/// left nothing to set, and the experiment is refused with the repair; one
/// that replaced it with a number left a constant, which takes the value.
#[test]
fn a_timed_value_over_a_runs_replaced_equation_is_refused_unless_it_is_a_number() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    experiment(
        &mut host,
        &mut session,
        json!({"name": "base", "set": [{"variable": "coverage", "equation": "4 + TIME / 10"}]}),
    );
    for change in [json!({"value": 8}), json!({"multiply": 2})] {
        let mut set = change.clone();
        set["variable"] = json!("coverage");
        let refusal = host.refuse(
            &mut session,
            "run_experiment",
            json!({"name": "late", "from": "base", "fromTime": 10, "set": [set]}),
        );
        assert_eq!(
            refusal["error"],
            "run 'base' gave 'coverage' the equation '4 + TIME / 10', so it is computed there \
             and has no value to set from time 10; give it an equation with the time in it \
             instead (IF TIME >= 10 THEN new ELSE 4 + TIME / 10)",
            "{change}"
        );
    }
    // From the start, the value replaces the run's equation whole.
    let whole = experiment(
        &mut host,
        &mut session,
        json!({"name": "whole", "from": "base",
               "set": [{"variable": "coverage", "value": 8}]}),
    );
    assert_eq!(whole["applied"][0]["value"], 8.0, "{whole}");
    let coverage = samples(&mut host, &mut session, "coverage", "whole");
    assert!(coverage.iter().all(|&v| v == 8.0), "{coverage:?}");

    experiment(
        &mut host,
        &mut session,
        json!({"name": "six", "set": [{"variable": "coverage", "equation": "6"}]}),
    );
    let late = experiment(
        &mut host,
        &mut session,
        json!({"name": "late", "from": "six", "fromTime": 10,
               "set": [{"variable": "coverage", "value": 8}]}),
    );
    assert_eq!(late["applied"][0]["was"], 6.0, "{late}");
    let coverage = samples(&mut host, &mut session, "coverage", "late");
    assert!(coverage[..5].iter().all(|&v| v == 6.0), "{coverage:?}");
    assert_eq!(coverage[11], 8.0, "{coverage:?}");
}

/// A save step longer than the run saves the first step alone, and every
/// tool that runs the model answers for that one row.
#[test]
fn a_run_of_one_row_is_read_by_every_tool() {
    let mut project = inventory().build_datamodel();
    project.sim_specs.save_step = Some(crate::datamodel::Dt::Dt(1e308));
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let behavior = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"]}),
    );
    let samples = behavior["series"][0]["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 1, "{behavior}");
    assert_eq!(samples[0]["time"], 0.0);
    let changed = experiment(
        &mut host,
        &mut session,
        json!({"name": "x", "set": [{"variable": "coverage", "value": 8}]}),
    );
    assert_eq!(changed["applied"][0]["value"], 8.0, "{changed}");
    host.call(&mut session, "analyze_loops", json!({}));
    host.call(&mut session, "run_tests", json!({}));
}

/// A run whose clock's last place is coarser than its DT is the whole run:
/// it is taken in slices counted in steps, as the VM counts them, so it
/// ends however many of its steps share one time. Each call answers within
/// a deadline on its own thread, so a run that never ends fails here rather
/// than hanging the suite.
#[test]
fn a_run_whose_clock_is_coarser_than_its_dt_ends() {
    for method in ["euler", "rk4"] {
        for (start, stop, dt) in [
            (1e16, 1e16 + 4.0, 1.0),
            (9007199254740992.0, 9007199254740996.0, 1.0),
            (-1e16, -1e16 + 10.0, 1.0),
            (1e18, 1e18 + 128.0, 0.5),
            (1e22, 1e22 + 1e7, 1e6),
        ] {
            let specs = json!({"start": start, "stop": stop, "dt": dt, "method": method});
            let (tx, rx) = std::sync::mpsc::channel();
            let input = json!({"name": "x", "specs": specs});
            std::thread::spawn(move || {
                let mut host = Host::from_test_project(&inventory());
                let mut session = Session::new("main");
                let output = host.call(&mut session, "run_experiment", input);
                let rows = session
                    .run_results(host.workspace(), "x")
                    .map(|run| (run.results.step_count, run.results.specs.final_step()));
                let _ = tx.send((output, rows.ok()));
            });
            let Ok((output, rows)) = rx.recv_timeout(std::time::Duration::from_secs(30)) else {
                panic!("{specs}: the run did not end");
            };
            let Some((saved, final_step)) = rows else {
                panic!("{specs}: the run is kept: {output}");
            };
            assert_eq!(saved as u64, final_step + 1, "{specs}: every step saved");
        }
    }
}

/// An experiment's two runs share their series' magnitudes. A stock that holds
/// arithmetic residue in the model's run and opens to 6 in the experiment
/// was at rest and is rising: the residue is not a behavior of its own that
/// the experiment merely scaled.
#[test]
fn a_run_of_residue_beside_a_run_that_moves_is_at_rest() {
    let project = TestProject::new("eq")
        .with_sim_time(0.0, 20.0, 0.25)
        .aux("demand", "3", None)
        .aux("fraction", "0.1", None)
        .aux("margin", "demand * fraction - demand / 10", None)
        .aux("shown", "margin * 4", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "doubled", "set": [{"variable": "fraction", "multiply": 2}],
               "record": ["shown"]}),
    );
    let shown = comparison(&output, "shown");
    // A product of residue: nothing in the model says what scale it is at,
    // and the experiment's own run does.
    assert_eq!(shown["this"]["end"], 1.2, "{shown}");
    assert_eq!(shown["base"]["mode"]["kind"], "at_rest", "{shown}");
    // At rest, with the residue it holds: four times 5.5511e-17.
    assert_eq!(shown["base"]["end"], 2.2204e-16, "{shown}");
    assert_eq!(shown["base"]["max"]["value"], 2.2204e-16, "{shown}");
}

/// And the other way: residue in the experiment's run beside a movement in
/// the model's is at rest. The experiment closes a gap the model opens, so
/// what is left of it creeps to 1e-15, read at the scale of the movement it
/// is compared with; read alone it would be a line.
#[test]
fn residue_in_the_experiments_run_beside_a_movement_in_the_models_is_at_rest() {
    let project = TestProject::new("gap")
        .with_sim_time(0.0, 20.0, 0.25)
        .aux("demand", "3", None)
        .aux("fraction", "0.2", None)
        .flow("orders", "demand * fraction", None)
        .flow("fulfilled", "demand / 10", None)
        .stock("gap", "0", &["orders"], &["fulfilled"], None)
        .aux("shown", "gap * 4", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "closed", "set": [{"variable": "fraction", "value": 0.1}],
               "record": ["shown"]}),
    );
    let shown = comparison(&output, "shown");
    assert_eq!(shown["base"]["mode"]["kind"], "linear", "{shown}");
    assert_eq!(shown["this"]["mode"]["kind"], "at_rest", "{shown}");
    let end = shown["this"]["end"].as_f64().unwrap_or(0.0);
    assert!(end != 0.0 && end.abs() < 1e-12, "it holds residue: {shown}");
}

/// A stock with an inflow of `inflow`, drained a millionth of itself a time
/// unit (a loop, so the loop analysis reads whether its stock moves), and
/// `x` for the inflow to use.
fn draining(inflow: &str) -> TestProject {
    TestProject::new("draining")
        .with_sim_time(0.0, 100.0, 0.25)
        .aux("x", "0", None)
        .flow("in_flow", inflow, None)
        .flow("out_flow", "balance * 1e-6", None)
        .stock("balance", "0", &["in_flow"], &["out_flow"], None)
}

/// What every tool that reads a run's behavior says of `balance` in the run
/// an experiment made with `set`: its mode in the experiment's record, in
/// `read_behavior` and in a finding that cites it, and whether the loop
/// analysis reads the run's stocks as at rest. They agree.
fn reads_of_balance(project: &TestProject, set: Value) -> (String, bool) {
    let mut host = Host::from_test_project(project);
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "x", "set": [set], "record": ["balance"]}),
    );
    let kind = comparison(&output, "balance")["this"]["mode"]["kind"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let read = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["balance"], "runs": ["x"]}),
    );
    assert_eq!(read["series"][0]["mode"]["kind"], kind.as_str(), "{read}");
    let cited = host.call(
        &mut session,
        "verify_findings",
        json!({"findings": [{"kind": "observation", "claim": "c", "citations": [
            {"cites": "behavior_mode", "variable": "balance", "mode": kind, "run": "x"}]}]}),
    );
    assert_eq!(cited["findings"][0]["holds"], true, "{cited}");
    let loops = host.call(&mut session, "analyze_loops", json!({"run": "x"}));
    let at_rest = loops["basis"] == "structure";
    assert_eq!(at_rest, kind == "at_rest", "{loops}");
    (kind, at_rest)
}

/// A run is read at the scale of its own equations, the experiment's
/// replacements applied, by every tool. A stock fed `1e9 + x - 1e9` (zero:
/// at rest) and, in the experiment, a real inflow of 1e-5 moves 1e-3 there,
/// which beside the model's terms of 1e9 would be rounding; and the other
/// way, an inflow of 1e-5 replaced by a difference of terms of 1e9 leaves
/// only rounding of them. A value set on `x` changes no equation, so its
/// run is read at the model's terms.
#[test]
fn a_run_is_read_at_the_scale_of_its_own_equations() {
    let cancelling = draining("1e9 + x - 1e9");
    let (kind, _) = reads_of_balance(
        &cancelling,
        json!({"variable": "in_flow", "equation": "1e-5"}),
    );
    assert_eq!(kind, "linear");
    let (kind, _) = reads_of_balance(
        &draining("1e-5"),
        json!({
            "variable": "in_flow", "equation": "1e9 + 1e-5 - 1e9"
        }),
    );
    assert_eq!(kind, "at_rest");
    let (kind, _) = reads_of_balance(&cancelling, json!({"variable": "x", "value": 1e-5}));
    assert_eq!(kind, "at_rest");
    // The model's own runs, as they were: at rest fed zero, and moving fed
    // 1e-5.
    let mut host = Host::from_test_project(&cancelling);
    let read = host.call(
        &mut Session::new("main"),
        "read_behavior",
        json!({"variables": ["balance"]}),
    );
    assert_eq!(read["series"][0]["mode"]["kind"], "at_rest", "{read}");
    let mut host = Host::from_test_project(&draining("1e-5"));
    let read = host.call(
        &mut Session::new("main"),
        "read_behavior",
        json!({"variables": ["balance"]}),
    );
    assert_ne!(read["series"][0]["mode"]["kind"], "at_rest", "{read}");
}

#[test]
fn specs_change_the_run_and_the_output_says_what_it_ran_under() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "fine", "specs": {"dt": 0.125, "method": "rk4", "stop": 30}}),
    );
    assert_eq!(
        output["specs"],
        json!({"start": 0.0, "stop": 30.0, "dt": 0.125, "method": "rk4", "timeUnits": "month"})
    );
    let summary = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"], "runs": ["fine"]}),
    );
    let samples = summary["series"][0]["samples"].as_array().unwrap();
    assert_eq!(
        samples[samples.len() - 1]["time"],
        30.0,
        "the run stops at 30"
    );
}

#[test]
fn a_name_used_again_replaces_its_run() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let first = experiment(&mut host, &mut session, json!({"name": "x"}));
    assert!(first.get("replaced").is_none());
    let second = experiment(
        &mut host,
        &mut session,
        json!({"name": "x", "set": [{"variable": "coverage", "value": 8}]}),
    );
    assert_eq!(second["replaced"], true);
}

#[test]
fn a_session_forgets_its_oldest_run_past_its_limit() {
    let project = TestProject::new("tiny")
        .with_sim_time(0.0, 2.0, 1.0)
        .stock("s", "0", &["f"], &[], None)
        .flow("f", "1", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    for i in 0..crate::tools::runs::MAX_RUNS {
        let output = experiment(&mut host, &mut session, json!({"name": format!("run {i}")}));
        assert!(output.get("forgotten").is_none());
    }
    let output = experiment(&mut host, &mut session, json!({"name": "one more"}));
    assert_eq!(output["forgotten"], "run 0");
    let refusal = host.refuse(
        &mut session,
        "read_behavior",
        json!({"variables": ["s"], "runs": ["run 0"]}),
    );
    assert!(refusal["error"].as_str().unwrap().contains("run 0"));
}

/// A refusal per rule an experiment can break, each naming what to do instead.
#[test]
fn an_experiment_that_breaks_a_rule_is_refused_saying_which() {
    for (input, says) in [
        (json!({"name": "  "}), "name is 1 to"),
        (json!({"name": "current"}), "name of its own"),
        (json!({"name": "x".repeat(65)}), "name is 1 to"),
        (
            json!({"name": "e", "set": [{"variable": "covrage", "value": 1}]}),
            "no variable 'covrage'",
        ),
        (
            json!({"name": "e", "set": [{"variable": "coverage", "value": 1, "multiply": 2}]}),
            "exactly one of",
        ),
        (
            json!({"name": "e", "set": [{"variable": "coverage"}]}),
            "exactly one of",
        ),
        (
            json!({"name": "e", "set": [{"variable": "coverage", "value": 1},
                                     {"variable": "Coverage", "value": 2}]}),
            "changed twice",
        ),
        (
            json!({"name": "e", "set": [{"variable": "orders", "value": 1}]}),
            "is computed",
        ),
        (
            json!({"name": "e", "set": [{"variable": "Inventory", "value": 1}]}),
            "is a stock",
        ),
        (
            json!({"name": "e", "set": [{"variable": "pressure_table", "value": 1}]}),
            "lookup table",
        ),
        (
            json!({"name": "e", "fromTime": 5, "set": [{"variable": "orders", "equation": "3"}]}),
            "holds from the start",
        ),
        (
            json!({"name": "e", "set": [{"variable": "orders", "equation": " "}]}),
            "not empty",
        ),
        (
            json!({"name": "e", "fromTime": 50, "set": [{"variable": "coverage", "value": 1}]}),
            "outside the run",
        ),
        (json!({"name": "e", "specs": {"dt": 0}}), "more than zero"),
        (
            json!({"name": "e", "specs": {"start": 5, "stop": 5}}),
            "come after",
        ),
        (
            json!({"name": "e", "from": "nowhere"}),
            "no run named 'nowhere'",
        ),
        (
            json!({"name": "e", "set": [{"variable": "orders", "equation": "ordrs * 2"}]}),
            "does not run",
        ),
        (json!({"name": "e", "record": ["invntory"]}), "to record"),
        (
            json!({"name": "e", "record": (0..13).map(|i| format!("v{i}")).collect::<Vec<_>>()}),
            "at most 12",
        ),
        (
            json!({"name": "e", "set": [{"variable": "coverage", "multiply": 1e308},
                                     {"variable": "adjustment_time", "value": 1}],
                "specs": {}}),
            "not a number",
        ),
    ] {
        let mut host = Host::from_test_project(&inventory());
        let mut session = Session::new("main");
        let refusal = host.refuse(&mut session, "run_experiment", input.clone());
        let message = refusal["error"].as_str().unwrap();
        assert!(message.contains(says), "{input}: {message}");
    }
}

#[test]
fn a_replacement_equation_that_does_not_compile_names_the_variable_and_changes_nothing() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let refusal = host.refuse(
        &mut session,
        "run_experiment",
        json!({"name": "broken", "set": [{"variable": "orders", "equation": "ordrs * 2"}]}),
    );
    let message = refusal["error"].as_str().unwrap();
    assert!(
        message.contains("orders") && message.contains("ordrs"),
        "{message}"
    );
    // Nothing was kept, and the model as it is still runs.
    assert!(
        host.refuse(
            &mut session,
            "read_behavior",
            json!({"variables": ["orders"], "runs": ["broken"]})
        )["error"]
            .as_str()
            .unwrap()
            .contains("no run named")
    );
    host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["orders"]}),
    );
}

#[test]
fn a_run_made_before_the_model_changed_is_not_a_starting_point() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    experiment(&mut host, &mut session, json!({"name": "early"}));
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("coverage")
            .unwrap()
            .set_scalar_equation("5")
    });
    let refusal = host.refuse(
        &mut session,
        "run_experiment",
        json!({"name": "later", "from": "early"}),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("has changed since"),
        "{refusal}"
    );
}

#[test]
fn a_knockout_changes_behavior_the_link_it_cut_drove() {
    // Production that ignores the inventory gap no longer corrects it: with
    // orders stepping up, inventory now falls where it sought its goal.
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "no correction", "set": [{"variable": "production", "equation": "10"}],
               "record": ["Inventory"]}),
    );
    let inventory = comparison(&output, "Inventory");
    assert!(
        inventory["this"]["end"].as_f64().unwrap() < inventory["base"]["end"].as_f64().unwrap(),
        "{inventory}"
    );
}

/// Every kind of variable with a table, given a replacement equation, takes
/// that equation as its value: the knockout an explanation's test rests on.
#[test]
fn a_replacement_equation_is_the_value_whatever_table_the_variable_had() {
    let gf = crate::datamodel::GraphicalFunction {
        kind: crate::datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![0.0, 1.0],
        x_scale: crate::datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    let mut project = TestProject::new("tables")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("s", "0", &["draining"], &[], None)
        .aux_with_gf("effect", "s + 5", gf.clone())
        .flow("draining", "s + 5", None)
        .build_datamodel();
    if let crate::datamodel::Variable::Flow(flow) =
        project.models[0].get_variable_mut("draining").unwrap()
    {
        flow.gf = Some(gf);
    }
    for (variable, value) in [("effect", 1.0), ("draining", 2.0)] {
        let mut host = Host::new(project.clone());
        let mut session = Session::new("main");
        let output = experiment(
            &mut host,
            &mut session,
            json!({"name": "held", "set": [{"variable": variable, "equation": value.to_string()}],
                   "record": [variable]}),
        );
        let this = &comparison(&output, variable)["this"];
        assert_eq!(this["start"], value, "{variable}: {output}");
        assert_eq!(this["end"], value, "{variable}: {output}");
        assert_eq!(output["applied"][0]["tableDropped"], true);
    }
    // A plain equation replaced drops no table and says nothing of one.
    let mut host = Host::from_test_project(&inventory());
    let output = experiment(
        &mut host,
        &mut Session::new("main"),
        json!({"name": "flat", "set": [{"variable": "orders", "equation": "10"}]}),
    );
    assert!(output["applied"][0].get("tableDropped").is_none());
}

#[test]
fn a_table_other_equations_call_has_no_value_to_replace_or_set() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let refusal = host.refuse(
        &mut session,
        "run_experiment",
        json!({"name": "x", "set": [{"variable": "pressure_table", "equation": "1"}]}),
    );
    assert!(
        refusal["error"].as_str().unwrap().contains("lookup table"),
        "{refusal}"
    );
    let refusal = host.refuse(
        &mut session,
        "run_experiment",
        json!({"name": "x", "set": [{"variable": "pressure_table", "value": 1}]}),
    );
    assert!(
        refusal["error"].as_str().unwrap().contains("lookup table"),
        "{refusal}"
    );
}

/// A flow whose value comes from a table is computed, however plain its
/// equation: its value cannot be set like a constant's.
#[test]
fn a_flow_with_a_table_is_not_set_like_a_constant() {
    let gf = crate::datamodel::GraphicalFunction {
        kind: crate::datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![0.0, 1.0],
        x_scale: crate::datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    let mut project = TestProject::new("tables")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("s", "0", &["draining"], &[], None)
        .flow("draining", "5", None)
        .build_datamodel();
    if let crate::datamodel::Variable::Flow(flow) =
        project.models[0].get_variable_mut("draining").unwrap()
    {
        flow.gf = Some(gf);
    }
    let mut host = Host::new(project);
    let refusal = host.refuse(
        &mut Session::new("main"),
        "run_experiment",
        json!({"name": "x", "set": [{"variable": "draining", "value": 1}]}),
    );
    assert!(
        refusal["error"].as_str().unwrap().contains("computed"),
        "{refusal}"
    );
}

/// A learner's broken model can be fixed in a copy: an equation that repairs
/// it runs, compared with nothing, and a value change is refused with the
/// repair.
#[test]
fn a_fix_to_a_model_that_does_not_simulate_is_tried_in_a_copy() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "fixed", "set": [{"variable": "shipments", "equation": "orders"}]}),
    );
    assert!(
        output["note"]
            .as_str()
            .unwrap()
            .contains("does not simulate"),
        "{output}"
    );
    let inventory = comparison(&output, "Inventory");
    assert!(inventory.get("base").is_none(), "{output}");
    assert!(inventory["this"]["end"].as_f64().is_some());

    let refusal = host.refuse(
        &mut session,
        "run_experiment",
        json!({"name": "valued", "set": [{"variable": "coverage", "value": 5}]}),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("replacement equation"),
        "{refusal}"
    );
}

/// A diagram edit, or an edit of a variable's notes or units, leaves the runs
/// as they were: they stay fresh, a run can still start from one, and the
/// current run is not simulated again. Units are checked, never simulated.
#[test]
fn a_layout_notes_or_units_edit_leaves_every_run_fresh() {
    let mut host = Host::new(crate::tools::test_support::with_diagram(
        inventory().build_datamodel(),
    ));
    let mut session = Session::new("main");
    experiment(
        &mut host,
        &mut session,
        json!({"name": "more coverage", "set": [{"variable": "coverage", "value": 8}]}),
    );
    let before = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"], "runs": ["current", "more coverage"]}),
    );
    host.edit(crate::tools::test_support::zoom_the_diagram);
    host.edit(|p| {
        let coverage = p.models[0].get_variable_mut("coverage").unwrap();
        coverage.set_documentation("weeks of orders the inventory aims to hold");
        coverage.set_units("week");
    });
    let after = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"], "runs": ["current", "more coverage"]}),
    );
    assert!(after.get("staleRuns").is_none(), "{after}");
    assert_eq!(after["series"], before["series"]);
    experiment(
        &mut host,
        &mut session,
        json!({"name": "on top", "from": "more coverage", "set": [{"variable": "adjustment_time", "value": 3}]}),
    );

    // An equation edit is a change: the named runs are stale.
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("orders")
            .unwrap()
            .set_scalar_equation("12")
    });
    let stale = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"], "runs": ["more coverage"]}),
    );
    assert_eq!(stale["staleRuns"][0]["run"], "more coverage");
}

/// Past the store's byte budget, the oldest runs keep only their plans: a
/// fresh one is run again from its plan when asked for, and a stale one is
/// gone.
#[test]
fn runs_past_the_store_budget_are_run_again_from_their_plans() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    session.runs.byte_budget = Some(1);
    for (name, value) in [("a", 5), ("b", 6), ("c", 7)] {
        experiment(
            &mut host,
            &mut session,
            json!({"name": name, "set": [{"variable": "coverage", "value": value}]}),
        );
    }
    let listing = session.runs(&host.workspace());
    assert_eq!(listing.len(), 3);
    let a = samples(&mut host, &mut session, "Inventory", "a");
    let mut fresh = Session::new("main");
    experiment(
        &mut host,
        &mut fresh,
        json!({"name": "a", "set": [{"variable": "coverage", "value": 5}]}),
    );
    assert_eq!(
        a,
        samples(&mut host, &mut fresh, "Inventory", "a"),
        "run again the same"
    );

    host.edit(|p| {
        p.models[0]
            .get_variable_mut("orders")
            .unwrap()
            .set_scalar_equation("12")
    });
    let listing = session.runs(&host.workspace());
    assert!(listing.iter().all(|run| run.stale), "{listing:?}");
    let gone: Vec<&str> = listing
        .iter()
        .filter(|r| r.gone)
        .map(|r| r.name.as_str())
        .collect();
    assert!(!gone.is_empty(), "{listing:?}");
    let refusal = host.refuse(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"], "runs": [gone[0]]}),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("no longer keeps"),
        "{refusal}"
    );
}

/// The schema says a change is exactly one of value, multiply and equation,
/// and the tool refuses two with the rule.
#[test]
fn a_change_is_exactly_one_of_value_multiply_and_equation() {
    let catalog: Value = serde_json::from_str(crate::tools::catalog_json()).unwrap();
    let schema = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "run_experiment")
        .unwrap()["inputSchema"]
        .clone();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let two = json!({"name": "x", "set": [{"variable": "coverage", "value": 1, "equation": "2"}]});
    assert!(!validator.is_valid(&two));
    assert!(
        validator.is_valid(&json!({"name": "x", "set": [{"variable": "coverage", "multiply": 2}]}))
    );
    let mut host = Host::from_test_project(&inventory());
    let refusal = host.refuse(&mut Session::new("main"), "run_experiment", two);
    assert!(
        refusal["error"].as_str().unwrap().contains("exactly one"),
        "{refusal}"
    );
}

fn listing(host: &mut Host, session: &mut Session) -> Value {
    serde_json::to_value(session.runs(&host.workspace())).unwrap()
}

/// A run's listing is what it changed from the model, exactly as it ran --
/// every value it set, from when, every replacement equation, its specs --
/// with the run it started from. A run made from another lists the other's
/// changes it kept, so the listing is the whole of what it did.
#[test]
fn a_listing_says_what_each_run_changed_from_when_and_from_which_run() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    experiment(
        &mut host,
        &mut session,
        json!({"name": "base", "set": [{"variable": "coverage", "multiply": 1.234567}]}),
    );
    experiment(
        &mut host,
        &mut session,
        json!({
            "name": "on top",
            "from": "base",
            "set": [{"variable": "adjustment time", "value": 3}],
            "fromTime": 5,
            "specs": {"dt": 0.125, "method": "rk4"}
        }),
    );
    experiment(
        &mut host,
        &mut session,
        json!({"name": "knockout", "set": [{"variable": "effect of pressure", "equation": "1"}]}),
    );
    assert_eq!(
        listing(&mut host, &mut session),
        json!([
            {"name": "base", "revision": 0, "stale": false, "gone": false, "from": "current",
             "changes": [{"variable": "coverage", "value": 4.0 * 1.234567}], "specs": {}},
            {"name": "on top", "revision": 0, "stale": false, "gone": false, "from": "base",
             "changes": [
                 {"variable": "coverage", "value": 4.0 * 1.234567},
                 {"variable": "adjustment_time", "value": 3.0, "fromTime": 5.0}
             ],
             "specs": {"dt": 0.125, "method": "rk4"}},
            {"name": "knockout", "revision": 0, "stale": false, "gone": false, "from": "current",
             "changes": [{"variable": "effect_of_pressure", "equation": "1", "tableDropped": true}],
             "specs": {}}
        ]),
        "the experiment's own answer rounds the value; the listing does not"
    );
    // The agent reads the same listing.
    let output = host.call(&mut session, "list_runs", json!({}));
    assert_eq!(output["runs"], listing(&mut host, &mut session));
    assert!(output.get("omitted").is_none(), "{output}");
}

/// Runs are listed in the order they were made, those kept only as their
/// plans in their places, and a run made again under its name last.
#[test]
fn runs_are_listed_oldest_first_those_kept_only_as_plans_included() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    session.runs.byte_budget = Some(1);
    for (name, value) in [("a", 5), ("b", 6), ("c", 7)] {
        experiment(
            &mut host,
            &mut session,
            json!({"name": name, "set": [{"variable": "coverage", "value": value}]}),
        );
    }
    let names = |host: &mut Host, session: &mut Session| -> Vec<String> {
        session
            .runs(&host.workspace())
            .into_iter()
            .map(|run| run.name)
            .collect()
    };
    assert_eq!(names(&mut host, &mut session), ["a", "b", "c"]);
    // Reading the oldest runs it again from its plan; it keeps its place.
    samples(&mut host, &mut session, "Inventory", "a");
    assert_eq!(names(&mut host, &mut session), ["a", "b", "c"]);
    experiment(
        &mut host,
        &mut session,
        json!({"name": "a", "set": [{"variable": "coverage", "value": 8}]}),
    );
    assert_eq!(names(&mut host, &mut session), ["b", "c", "a"]);
    assert_eq!(
        listing(&mut host, &mut session)[2]["changes"][0]["value"],
        8.0
    );
}

/// The agent's listing keeps to the answer budget: long equations are quoted
/// around their start, then the oldest runs are left out, named.
#[test]
fn the_agents_listing_keeps_to_the_budget() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    // Long, but not deep: a deeply nested equation is the parser's limit.
    let long = format!("orders{}* 1", " ".repeat(600));
    for name in ["first", "second", "third"] {
        experiment(
            &mut host,
            &mut session,
            json!({"name": name, "set": [{"variable": "shipments", "equation": long}]}),
        );
    }
    let whole = host.call(&mut session, "list_runs", json!({}));
    assert_eq!(whole["runs"][0]["changes"][0]["equation"], long.as_str());

    session.outline_budget = 2_000;
    let quoted = host.call(&mut session, "list_runs", json!({}));
    let equation = quoted["runs"][0]["changes"][0]["equation"]
        .as_str()
        .unwrap();
    assert!(
        equation.ends_with('…') && equation.chars().count() <= 241,
        "{equation}"
    );
    assert_eq!(quoted["runs"].as_array().unwrap().len(), 3, "{quoted}");

    session.outline_budget = 700;
    let fitted = host.call(&mut session, "list_runs", json!({}));
    assert!(fitted.to_string().len() <= 700, "{fitted}");
    let omitted = fitted["omitted"].as_array().unwrap();
    assert_eq!(omitted[0], "first", "the oldest go first: {fitted}");
    assert_eq!(
        omitted.len() + fitted["runs"].as_array().unwrap().len(),
        3,
        "{fitted}"
    );
}

/// An experiment whose specs would make a run hold or compute more than a
/// run may is refused before it runs, with the numbers and specs that fit.
#[test]
fn an_experiment_that_would_cost_more_than_a_run_may_is_refused_with_what_fits() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    let refusal = host.refuse(
        &mut session,
        "run_experiment",
        json!({"name": "fine", "specs": {"dt": 0.0001}}),
    );
    let reason = refusal["error"].as_str().unwrap();
    assert!(
        reason.contains("200001 rows") && reason.contains("a DT of at least"),
        "{reason}"
    );
    assert!(
        session.runs(&host.workspace()).is_empty(),
        "nothing is kept"
    );
    // The DT it names fits.
    let dt: f64 = reason
        .split("a DT of at least ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .unwrap()
        .parse()
        .unwrap();
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "fits", "specs": {"dt": dt}}),
    );
}

/// A series that is not a number, from the start or from a time, is
/// summarized without those numbers, and the answer matches its schema.
#[test]
fn numbers_that_are_not_finite_are_left_out_of_summaries() {
    let project = TestProject::new("undefined")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("population", "1", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.1", None)
        .aux("unfinished", "NaN", None)
        .aux("breaks", "IF TIME >= 5 THEN 1 / 0 ELSE TIME", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = experiment(
        &mut host,
        &mut session,
        json!({"name": "faster", "set": [{"variable": "rate", "value": 0.2}],
               "record": ["unfinished", "breaks", "population"]}),
    );
    let catalog: Value = serde_json::from_str(crate::tools::catalog_json()).unwrap();
    let schema = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "run_experiment")
        .unwrap()["outputSchema"]
        .clone();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&output)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:?}\n{output}");

    let unfinished = &comparison(&output, "unfinished")["this"];
    for field in ["start", "end", "min", "max"] {
        assert!(unfinished.get(field).is_none(), "{field}: {unfinished}");
    }
    assert_eq!(unfinished["mode"]["kind"], "undefined", "{unfinished}");
    let breaks = &comparison(&output, "breaks")["this"];
    assert_eq!(breaks["start"], 0.0, "{breaks}");
    assert!(breaks.get("end").is_none(), "{breaks}");
    assert_eq!(breaks["max"]["value"], 4.0, "the finite part: {breaks}");

    let read = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["breaks"], "runs": ["faster"]}),
    );
    let points = read["series"][0]["samples"].as_array().unwrap();
    assert!(
        points.iter().all(|p| p["value"].is_number()),
        "no sample is null: {read}"
    );
}

/// A call stops at the next run it would simulate once other work waits for
/// the project, and keeps nothing: no run is made, and a run kept only as its
/// plan stays so.
#[test]
fn a_call_stops_before_its_next_simulation_when_other_work_waits() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    // Waiting begins after the call's first check, at its start.
    let output = host.call_waiting(
        &mut session,
        "run_experiment",
        json!({"name": "slow", "set": [{"variable": "adjustment time", "value": 8}]}),
        &Host::waiting_after(1),
    );
    assert!(output.is_error, "{}", output.json);
    assert!(
        output.json.contains("\"interrupted\":true"),
        "{}",
        output.json
    );
    assert!(session.runs(&host.workspace()).is_empty(), "no run is kept");
    // With the model's own run already made, the experiment's own run is the
    // one it stops before.
    samples(&mut host, &mut session, "Inventory", "current");
    let output = host.call_waiting(
        &mut session,
        "run_experiment",
        json!({"name": "slow", "set": [{"variable": "adjustment time", "value": 8}]}),
        &Host::waiting_after(1),
    );
    assert!(
        output.json.contains("\"interrupted\":true"),
        "{}",
        output.json
    );
    assert!(session.runs(&host.workspace()).is_empty(), "no run is kept");

    session.runs.byte_budget = Some(1);
    for name in ["a", "b"] {
        experiment(
            &mut host,
            &mut session,
            json!({"name": name, "set": [{"variable": "coverage", "value": 5}]}),
        );
    }
    let output = host.call_waiting(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"], "runs": ["a"]}),
        &Host::waiting_after(1),
    );
    assert!(
        output.json.contains("\"interrupted\":true"),
        "{}",
        output.json
    );
    // With nothing waiting, the same call runs it again.
    samples(&mut host, &mut session, "Inventory", "a");
}

/// A run the person discards is forgotten: no tool reads it, the listing
/// leaves it out, and a run made from it keeps what it changed. Forgetting
/// it again, or one the session never had, is no error; the model as it is
/// is not the session's to forget.
#[test]
fn a_forgotten_run_is_gone_for_every_tool_and_what_was_made_from_it_stays() {
    let mut host = Host::from_test_project(&inventory());
    let mut session = Session::new("main");
    experiment(
        &mut host,
        &mut session,
        json!({"name": "base", "set": [{"variable": "coverage", "value": 6}]}),
    );
    experiment(
        &mut host,
        &mut session,
        json!({"name": "on top", "from": "base",
               "set": [{"variable": "adjustment time", "value": 3}]}),
    );
    assert_eq!(session.forget_run("base"), Ok(true));
    let listing = listing(&mut host, &mut session);
    assert_eq!(listing.as_array().unwrap().len(), 1, "{listing}");
    assert_eq!(listing[0]["from"], "base");
    assert_eq!(listing[0]["changes"].as_array().unwrap().len(), 2);
    let refusal = host.refuse(
        &mut session,
        "read_behavior",
        json!({"variables": ["Inventory"], "runs": ["base"]}),
    );
    assert_eq!(
        refusal["suggestions"],
        json!(["current", "on top"]),
        "{refusal}"
    );
    samples(&mut host, &mut session, "Inventory", "on top");

    assert_eq!(session.forget_run("base"), Ok(false));
    assert_eq!(session.forget_run("never made"), Ok(false));
    assert!(session.forget_run("current").is_err());
}
