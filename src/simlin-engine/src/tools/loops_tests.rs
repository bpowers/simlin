// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::datamodel::{self, GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale};
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

/// Call `analyze_loops`, which must answer, and check the answer against the
/// output schema the catalog publishes and, for loops from a run, that its
/// timelines keep their rules.
fn analyze(host: &mut Host, session: &mut Session, input: Value) -> Value {
    let output = host.call(session, "analyze_loops", input);
    let catalog: Value = serde_json::from_str(crate::tools::catalog_json()).unwrap();
    let schema = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "analyze_loops")
        .unwrap()["outputSchema"]
        .clone();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&output)
        .map(|e| format!("{e} at {}", e.instance_path))
        .collect();
    assert!(errors.is_empty(), "{errors:?}\n{output}");
    for partition in output["partitions"].as_array().unwrap() {
        check_timeline(partition);
    }
    output
}

/// A partition's timeline tiles the run in at most [`WINDOWS`] spans, each
/// naming at most [`MAX_LEADERS`] leaders strongest first, each active and
/// holding at least [`RIVAL_SHARE`] of the strongest's share; and every loop
/// it names is listed.
fn check_timeline(partition: &Value) {
    let Some(spans) = partition["dominance"].as_array() else {
        return;
    };
    assert!(!spans.is_empty() && spans.len() <= WINDOWS, "{partition}");
    for pair in spans.windows(2) {
        assert_eq!(pair[0]["to"], pair[1]["from"], "spans tile: {partition}");
        assert_ne!(
            pair[0]["leaders"][0]["loop"], pair[1]["leaders"][0]["loop"],
            "adjacent spans have different leaders: {partition}"
        );
    }
    let listed: Vec<&Value> = partition["loops"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| &l["id"])
        .collect();
    for span in spans {
        let leaders = span["leaders"].as_array().unwrap();
        assert!(leaders.len() <= MAX_LEADERS, "{span}");
        let shares: Vec<f64> = leaders
            .iter()
            .map(|l| l["share"].as_f64().unwrap())
            .collect();
        assert!(shares.windows(2).all(|w| w[0] >= w[1]), "{span}");
        if let Some(&strongest) = shares.first() {
            // Shares are rounded to hundredths, so compare within that.
            assert!(
                shares.iter().all(|&s| s + 0.01 >= RIVAL_SHARE * strongest),
                "{span}"
            );
        }
        for leader in leaders {
            assert!(listed.contains(&&leader["loop"]), "{leader} is listed");
        }
    }
}

/// The chain of a loop report as `(variable, sign)` pairs.
fn chain(report: &Value) -> Vec<(String, String)> {
    report["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            (
                step["variable"].as_str().unwrap().to_string(),
                step["polarity"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn steps(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(v, s)| (v.to_string(), s.to_string()))
        .collect()
}

/// The loop in `output`'s partitions of `polarity`, which must be the only one.
fn only_loop<'a>(output: &'a Value, polarity: &str) -> &'a Value {
    let found: Vec<&Value> = output["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["loops"].as_array().unwrap())
        .filter(|l| l["polarity"] == polarity)
        .collect();
    assert_eq!(found.len(), 1, "one {polarity} loop: {output}");
    found[0]
}

/// Logistic growth with its two loops apart: births compound the population
/// (reinforcing), and crowding lowers the fractional birth rate as the
/// population nears its capacity (balancing). The reinforcing loop drives the
/// early growth and the balancing loop the approach to capacity; the switch is
/// at the inflection, when the population is half its capacity (t ~ 9.2).
fn logistic() -> TestProject {
    TestProject::new("logistic")
        .with_sim_time(0.0, 40.0, 0.125)
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

/// A goal seeker at its goal: at rest, so no loop is active.
fn at_rest() -> TestProject {
    TestProject::new("at rest")
        .with_sim_time(0.0, 20.0, 0.25)
        .stock("level", "100", &["adjustment"], &[], None)
        .flow("adjustment", "(goal - level) / adjustment_time", None)
        .aux("goal", "100", None)
        .aux("adjustment_time", "4", None)
}

/// A ring of `n` auxiliaries between a stock and its outflow: one balancing
/// loop of `n + 2` variables.
fn ring(n: usize, initial: &str) -> TestProject {
    let mut project = TestProject::new("ring")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", initial, &[], &["drain"], None)
        .aux("a1", "level", None);
    for i in 2..=n {
        project = project.aux(&format!("a{i}"), &format!("a{}", i - 1), None);
    }
    project.flow("drain", &format!("a{n} / 5"), None)
}

/// A ring the structure alone cannot enumerate: its one cycle is past the
/// size at which loops are found only from a run's scores.
fn large_ring(initial: &str) -> TestProject {
    ring(crate::ltm::MAX_LTM_SCC_NODES, initial)
}

/// Growth through an effect whose table rises to a peak at 1 and falls after:
/// the engine cannot sign the link into the effect from its equation, and a
/// run signs it by what it did. `rate` sets whether the level passes the peak.
fn hump(rate: &str) -> TestProject {
    let table = GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0, 2.0]),
        y_points: vec![0.0, 1.0, 0.0],
        x_scale: GraphicalFunctionScale { min: 0.0, max: 2.0 },
        y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    TestProject::new("hump")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("level", "0.1", &["growth"], &[], None)
        .flow("growth", &format!("effect * {rate}"), None)
        .aux_with_gf("effect", "level", table)
}

#[test]
fn logistic_growth_is_led_by_its_reinforcing_loop_then_its_balancing_loop() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["run"], "current");
    assert_eq!(output["basis"], "run");
    assert_eq!(output["complete"], true);
    assert_eq!(output["found"], 2);
    assert!(output.get("omitted").is_none() && output.get("note").is_none());
    let partitions = output["partitions"].as_array().unwrap();
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0]["stocks"], json!(["population"]));
    assert_eq!(partitions[0]["loopCount"], 2);

    let growth = only_loop(&output, "reinforcing");
    let crowding = only_loop(&output, "balancing");
    assert_eq!(
        chain(growth),
        steps(&[("population", "+"), ("births", "+")]),
        "from the stock, each link signed, returning to it"
    );
    assert_eq!(
        chain(crowding),
        steps(&[
            ("population", "-"),
            ("fractional_birth_rate", "+"),
            ("births", "+")
        ])
    );
    let share = |l: &Value| l["share"].as_f64().unwrap();
    assert!(share(crowding) > share(growth), "crowding leads for longer");
    assert!(share(growth) > 0.0 && share(crowding) < 1.0);

    let spans = partitions[0]["dominance"].as_array().unwrap();
    assert_eq!(spans[0]["from"], 0.0);
    assert_eq!(spans[0]["leaders"][0]["loop"], growth["id"]);
    let last = spans.last().unwrap();
    assert_eq!(last["to"], 40.0);
    assert_eq!(last["leaders"][0]["loop"], crowding["id"]);
    let switch = spans[1]["from"].as_f64().unwrap();
    assert!(
        (5.0..=15.0).contains(&switch),
        "the lead changes near the inflection, at {switch}"
    );
}

#[test]
fn a_loop_keeps_its_id_across_runs_and_edits() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let ids = |output: &Value| -> Vec<(Vec<String>, String)> {
        let mut ids: Vec<(Vec<String>, String)> = output["partitions"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|p| p["loops"].as_array().unwrap())
            .map(|l| {
                let variables = chain(l).into_iter().map(|(v, _)| v).collect();
                (variables, l["id"].as_str().unwrap().to_string())
            })
            .collect();
        ids.sort();
        ids
    };
    let first = ids(&analyze(&mut host, &mut session, json!({})));
    assert_eq!(first.len(), 2);

    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "fast", "set": [{"variable": "max_rate", "multiply": 2}]}),
    );
    let fast = ids(&analyze(&mut host, &mut session, json!({"run": "fast"})));
    assert_eq!(fast, first, "the same loops in another run");

    host.edit(|p| {
        p.models[0]
            .get_variable_mut("capacity")
            .unwrap()
            .set_scalar_equation("200")
    });
    let edited = analyze(&mut host, &mut session, json!({}));
    assert_eq!(edited["revision"], 1);
    assert_eq!(ids(&edited), first, "the same loops after an edit");
}

#[test]
fn a_model_at_rest_reports_its_loops_from_structure_and_how_to_disturb_it() {
    let mut host = Host::from_test_project(&at_rest());
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "structure");
    assert_eq!(output["complete"], true);
    assert_eq!(output["found"], 1);
    let note = output["note"].as_str().unwrap();
    assert!(
        note.contains("No loop was active") && note.contains("run_experiment"),
        "{note}"
    );
    let partition = &output["partitions"][0];
    assert!(partition.get("dominance").is_none());
    let adjustment = &partition["loops"][0];
    assert_eq!(adjustment["id"], "L1");
    assert_eq!(adjustment["polarity"], "balancing");
    assert!(adjustment.get("share").is_none());
    assert_eq!(
        chain(adjustment),
        steps(&[("level", "-"), ("adjustment", "+")])
    );

    // Disturbed, the same loop is active from the step on, under its id.
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "step", "set": [{"variable": "goal", "value": 120}], "fromTime": 5}),
    );
    let stepped = analyze(&mut host, &mut session, json!({"run": "step"}));
    assert_eq!(stepped["basis"], "run");
    assert!(stepped.get("note").is_none());
    let partition = &stepped["partitions"][0];
    assert_eq!(partition["loops"][0]["id"], "L1");
    assert_eq!(partition["loops"][0]["polarity"], "balancing");
    let spans = partition["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(
        (&spans[0]["to"], &spans[0]["leaders"]),
        (&json!(5.0), &json!([])),
        "no loop leads before the step"
    );
    assert_eq!(spans[1]["leaders"][0]["loop"], "L1");
}

#[test]
fn a_model_without_feedback_says_so() {
    let project = TestProject::new("fill")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("tank", "0", &["filling"], &[], None)
        .flow("filling", "2", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "structure");
    assert_eq!(output["found"], 0);
    assert_eq!(output["partitions"], json!([]));
    assert_eq!(output["note"], "The model has no feedback loops.");
}

#[test]
fn a_model_too_large_for_its_structure_alone_says_so_at_rest() {
    let mut host = Host::from_test_project(&large_ring("0"));
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "structure");
    assert_eq!(output["complete"], false);
    assert_eq!(output["partitions"], json!([]));
    let note = output["note"].as_str().unwrap();
    assert!(
        note.contains("too many loops to list from its structure alone")
            && note.contains("run_experiment"),
        "{note}"
    );

    // The same ring moving has its loop from the run: too long for an
    // overview to chain, so given by its stocks, and whole by its id.
    let mut host = Host::from_test_project(&large_ring("100"));
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "run");
    let ring = only_loop(&output, "balancing");
    let length = crate::ltm::MAX_LTM_SCC_NODES + 2;
    assert_eq!(
        ring["length"], length,
        "the stock, every auxiliary, the flow"
    );
    assert!(length > MAX_CHAIN && ring.get("chain").is_none());
    assert_eq!(ring["stocks"], json!(["level"]));

    let whole = analyze(&mut host, &mut session, json!({"loops": [ring["id"]]}));
    let ring = only_loop(&whole, "balancing");
    assert!(ring.get("stocks").is_none());
    assert_eq!(chain(ring).len(), length);
    assert_eq!(chain(ring)[0], ("level".to_string(), "+".to_string()));
    assert_eq!(
        chain(ring)[length - 1],
        ("drain".to_string(), "-".to_string())
    );
    assert!(whole["partitions"][0].get("dominance").is_none());
}

#[test]
fn a_replaced_equation_names_the_links_and_loops_it_cut() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let current = analyze(&mut host, &mut session, json!({}));
    let growth = only_loop(&current, "reinforcing")["id"].clone();
    let crowding = only_loop(&current, "balancing")["id"].clone();

    host.call(
        &mut session,
        "run_experiment",
        json!({
            "name": "no crowding",
            "set": [{"variable": "fractional_birth_rate", "equation": "max_rate"}]
        }),
    );
    let output = analyze(&mut host, &mut session, json!({"run": "no crowding"}));
    assert_eq!(output["basis"], "run");
    assert_eq!(output["found"], 1);
    assert_eq!(only_loop(&output, "reinforcing")["id"], growth);
    let cut = &output["cut"];
    assert_eq!(
        cut["links"],
        json!([
            {"from": "capacity", "to": "fractional_birth_rate"},
            {"from": "population", "to": "fractional_birth_rate"}
        ]),
        "what the replaced equation read and its replacement does not"
    );
    let cut_loops = cut["loops"].as_array().unwrap();
    assert_eq!(cut_loops.len(), 1);
    assert_eq!(cut_loops[0]["id"], crowding);
    assert_eq!(cut_loops[0]["polarity"], "balancing");
    assert!(
        cut_loops[0].get("share").is_none(),
        "a cut loop has no share in this run"
    );

    // A run that replaces no equation cuts nothing.
    let current = analyze(&mut host, &mut session, json!({}));
    assert!(current.get("cut").is_none());

    // The model itself is as it was: the replacement held in that run only.
    let record = host.call(
        &mut session,
        "read_variables",
        json!({"names": ["fractional_birth_rate"]}),
    );
    let inputs: Vec<&Value> = record["variables"][0]["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| &i["name"])
        .collect();
    assert_eq!(inputs, ["capacity", "max_rate", "population"]);
}

#[test]
fn an_analysis_leaves_the_projects_loop_mode_as_it_found_it() {
    for prior in [false, true] {
        let mut host = Host::from_test_project(&logistic());
        let source_project = host.db.current_source_project().unwrap();
        crate::db::set_project_ltm_discovery_mode(&mut host.db, source_project, prior);
        let mut session = Session::new("main");
        host.call(
            &mut session,
            "run_experiment",
            json!({
                "name": "no crowding",
                "set": [{"variable": "fractional_birth_rate", "equation": "max_rate"}]
            }),
        );
        // A staged run, with the model's loops from structure for its cut.
        analyze(&mut host, &mut session, json!({"run": "no crowding"}));
        assert_eq!(source_project.ltm_discovery_mode(&host.db), prior);
        // A run at rest, whose loops come from structure.
        let mut host = Host::from_test_project(&at_rest());
        let source_project = host.db.current_source_project().unwrap();
        crate::db::set_project_ltm_discovery_mode(&mut host.db, source_project, prior);
        let mut session = Session::new("main");
        analyze(&mut host, &mut session, json!({}));
        assert_eq!(source_project.ltm_discovery_mode(&host.db), prior);
    }
}

#[test]
fn a_cut_in_a_model_too_large_for_its_structure_alone_names_the_loops_of_its_current_run() {
    let mut host = Host::from_test_project(&large_ring("100"));
    let mut session = Session::new("main");
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "cut", "set": [{"variable": "a10", "equation": "5"}]}),
    );
    let output = analyze(&mut host, &mut session, json!({"run": "cut"}));
    assert_eq!(output["cut"]["links"], json!([{"from": "a9", "to": "a10"}]));
    let cut_loops = output["cut"]["loops"].as_array().unwrap();
    assert_eq!(cut_loops.len(), 1);
    assert_eq!(cut_loops[0]["polarity"], "balancing");
    assert_eq!(cut_loops[0]["length"], crate::ltm::MAX_LTM_SCC_NODES + 2);
    assert_eq!(output["basis"], "structure");
    assert_eq!(
        output["note"],
        "With its equations replaced, the model has no feedback loops."
    );

    // The loop the cut names is the current run's, under its id.
    let current = analyze(&mut host, &mut session, json!({}));
    assert_eq!(only_loop(&current, "balancing")["id"], cut_loops[0]["id"]);
}

#[test]
fn a_link_its_equation_cannot_sign_takes_the_runs_sign_unless_the_run_shows_both() {
    let effect_input = |host: &mut Host, session: &mut Session| {
        let record = host.call(session, "read_variables", json!({"names": ["effect"]}));
        record["variables"][0]["inputs"][0]["polarity"].clone()
    };

    // Below the peak throughout: the run signs the link.
    let mut host = Host::from_test_project(&hump("0.2"));
    let mut session = Session::new("main");
    assert_eq!(effect_input(&mut host, &mut session), "?", "the premise");
    let output = analyze(&mut host, &mut session, json!({}));
    let growth = only_loop(&output, "reinforcing");
    assert_eq!(
        chain(growth),
        steps(&[("level", "+"), ("effect", "+"), ("growth", "+")])
    );

    // Past the peak the link turns negative, and the loop with it: a link
    // the run shows both ways reads "?".
    let mut host = Host::from_test_project(&hump("2"));
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let l = &output["partitions"][0]["loops"][0];
    assert_ne!(l["polarity"], "reinforcing", "{l}");
    assert_eq!(chain(l)[0], ("level".to_string(), "?".to_string()), "{l}");
}

/// Logistic growth beside a damped oscillator: two partitions, the
/// oscillator's two stocks and the population.
fn two_subsystems() -> TestProject {
    logistic()
        .stock("x", "0", &["dx"], &[], None)
        .stock("v", "0", &["dv"], &[], None)
        .flow("dx", "v", None)
        .flow("dv", "0.25 * (100 - x) - 0.1 * v", None)
}

#[test]
fn partitions_are_listed_largest_first_and_through_finds_a_variables_loops() {
    let mut host = Host::from_test_project(&two_subsystems());
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let stocks: Vec<&Value> = output["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["stocks"])
        .collect();
    assert_eq!(stocks, [&json!(["v", "x"]), &json!(["population"])]);
    assert_eq!(output["found"], 4);

    let output = analyze(
        &mut host,
        &mut session,
        json!({"through": "fractional birth rate"}),
    );
    assert_eq!(output["through"], "fractional_birth_rate");
    assert_eq!(output["found"], 1);
    let partitions = output["partitions"].as_array().unwrap();
    assert_eq!(partitions.len(), 1, "only the population's partition");
    assert_eq!(partitions[0]["loopCount"], 1);
    // The timeline is the partition's, so the loop that leads early is listed
    // with the one through the variable.
    let crowding = only_loop(&output, "balancing");
    assert!(
        chain(crowding)
            .iter()
            .any(|(v, _)| v == "fractional_birth_rate")
    );

    let refusal = host.refuse(
        &mut session,
        "analyze_loops",
        json!({"through": "zebra crossing"}),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("no variable 'zebra crossing'"),
        "{refusal}"
    );

    let output = analyze(&mut host, &mut session, json!({"through": "max_rate"}));
    assert_eq!(output["found"], 0);
    assert_eq!(output["partitions"], json!([]));
    assert_eq!(
        output["note"],
        "No loop active in this run goes through max_rate."
    );
}

#[test]
fn activity_spread_across_many_loops_is_said_to_have_no_dominant_loop() {
    // Twelve equal loops through one stock: each holds a twelfth.
    let mut project = TestProject::new("spread").with_sim_time(0.0, 10.0, 0.5);
    let outflows: Vec<String> = (1..=12).map(|i| format!("loss_{i}")).collect();
    let refs: Vec<&str> = outflows.iter().map(String::as_str).collect();
    project = project.stock("level", "100", &[], &refs, None);
    for i in 1..=12 {
        project = project.aux(&format!("reading_{i}"), "level", None).flow(
            &format!("loss_{i}"),
            &format!("reading_{i} * 0.01"),
            None,
        );
    }
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["found"], 12);
    let partition = &output["partitions"][0];
    let leaders = &partition["dominance"].as_array().unwrap().last().unwrap()["leaders"];
    assert_eq!(leaders.as_array().unwrap().len(), MAX_LEADERS);
    assert_eq!(leaders[0]["share"], 0.08);
    let note = output["note"].as_str().unwrap();
    assert!(
        note.contains("partition with level") && note.contains("no one loop dominates"),
        "{note}"
    );

    // One loop holding the activity is not spread.
    let mut host = Host::from_test_project(&logistic());
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    assert!(output.get("note").is_none());
}

#[test]
fn a_builtins_internals_are_left_out_of_a_chain_and_the_links_across_them_composed() {
    // Adjustment toward a goal through a perceived level: SMTH1 is a module
    // whose internal nodes sit between the level and its perception. The
    // model starts at its goal, at rest.
    let project = TestProject::new("perceived")
        .with_sim_time(0.0, 20.0, 0.25)
        .stock("level", "10", &["adjustment"], &[], None)
        .flow("adjustment", "(goal - perceived) / 4", None)
        .aux("perceived", "SMTH1(level, 2)", None)
        .aux("goal", "10", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let at_rest = analyze(&mut host, &mut session, json!({}));
    assert_eq!(at_rest["basis"], "structure");
    let expected = steps(&[("level", "+"), ("perceived", "-"), ("adjustment", "+")]);
    let from_structure = only_loop(&at_rest, "balancing");
    assert_eq!(chain(from_structure), expected);
    assert_eq!(from_structure["length"], 3);

    // Disturbed, the run finds the same loop, under the same id.
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "step", "set": [{"variable": "goal", "value": 20}], "fromTime": 2}),
    );
    let moving = analyze(&mut host, &mut session, json!({"run": "step"}));
    assert_eq!(moving["basis"], "run");
    let from_run = only_loop(&moving, "balancing");
    assert_eq!(chain(from_run), expected);
    assert_eq!(from_run["id"], from_structure["id"]);
}

#[test]
fn an_arrayed_model_reports_each_elements_loop_under_its_subscript() {
    let project = TestProject::new("arrayed")
        .with_sim_time(0.0, 10.0, 0.5)
        .named_dimension("region", &["north", "south"])
        .array_stock("Population[region]", "10", &["births"], &[], None)
        .array_flow("births[region]", "Population * 0.1", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({"through": "population"}));
    let partitions = output["partitions"].as_array().unwrap();
    assert_eq!(partitions.len(), 2);
    for (partition, element) in partitions.iter().zip(["north", "south"]) {
        assert_eq!(
            partition["stocks"],
            json!([format!("Population[{element}]")])
        );
        assert_eq!(
            chain(&partition["loops"][0]),
            steps(&[
                (&format!("Population[{element}]"), "+"),
                (&format!("births[{element}]"), "+")
            ])
        );
    }
}

#[test]
fn a_loop_the_model_names_is_reported_under_its_name() {
    let mut project = logistic().build_datamodel();
    let model = &mut project.models[0];
    model.variables.rewrite(|variables| {
        for (uid, var) in variables.iter_mut().enumerate() {
            match var {
                datamodel::Variable::Stock(s) => s.uid = Some(uid as i32),
                datamodel::Variable::Flow(f) => f.uid = Some(uid as i32),
                datamodel::Variable::Aux(a) => a.uid = Some(uid as i32),
                datamodel::Variable::Module(m) => m.uid = Some(uid as i32),
            }
        }
    });
    let uids: Vec<i32> = ["population", "births", "fractional_birth_rate"]
        .iter()
        .map(|name| {
            model
                .variables
                .iter()
                .position(|v| v.get_ident() == *name)
                .unwrap() as i32
        })
        .collect();
    model.loop_metadata.push(datamodel::LoopMetadata {
        uids,
        deleted: false,
        name: "crowding".to_string(),
        description: String::new(),
    });
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(only_loop(&output, "balancing")["name"], "crowding");
    assert!(only_loop(&output, "reinforcing").get("name").is_none());
}

#[test]
fn loops_asked_for_by_id_come_whole_and_a_run_without_one_says_so() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let current = analyze(&mut host, &mut session, json!({}));
    let growth = only_loop(&current, "reinforcing").clone();
    let crowding = only_loop(&current, "balancing").clone();

    let output = analyze(
        &mut host,
        &mut session,
        json!({"loops": [crowding["id"], growth["id"]]}),
    );
    assert_eq!(output["found"], 2);
    let listed = &output["partitions"][0]["loops"];
    assert_eq!(listed[0], crowding, "in the order asked");
    assert_eq!(listed[1], growth);
    assert!(output["partitions"][0].get("dominance").is_none());

    // A run the crowding loop is cut from does not have it.
    host.call(
        &mut session,
        "run_experiment",
        json!({
            "name": "no crowding",
            "set": [{"variable": "fractional_birth_rate", "equation": "max_rate"}]
        }),
    );
    let output = analyze(
        &mut host,
        &mut session,
        json!({"run": "no crowding", "loops": [crowding["id"]]}),
    );
    assert_eq!(output["found"], 0);
    assert_eq!(output["absent"], json!([crowding["id"]]));
    let note = output["note"].as_str().unwrap();
    assert!(note.contains("not a loop of this run"), "{note}");

    for (input, says) in [
        (json!({"loops": ["L9"]}), "no loop has the id 'L9'"),
        (json!({"loops": ["loop one"]}), "no loop has the id"),
        (
            json!({"loops": ["L1", "L1", "L1", "L1", "L1"]}),
            "at most 4",
        ),
        (json!({"loops": ["L1"], "through": "births"}), "not both"),
    ] {
        let refusal = host.refuse(&mut session, "analyze_loops", input.clone());
        assert!(
            refusal["error"].as_str().unwrap().contains(says),
            "{input}: {refusal}"
        );
    }
}

#[test]
fn the_analysis_is_kept_with_the_run() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let first = analyze(&mut host, &mut session, json!({}));
    let model = host.project.models[0].clone();
    let run = session
        .runs
        .get(&mut host.workspace(), &model, CURRENT)
        .unwrap();
    assert!(run.loops.get().is_some());
    assert_eq!(analyze(&mut host, &mut session, json!({})), first);
}

#[test]
fn a_stale_run_and_a_run_that_does_not_exist_are_refused() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "fast", "set": [{"variable": "max_rate", "multiply": 2}]}),
    );
    let refusal = host.refuse(&mut session, "analyze_loops", json!({"run": "nowhere"}));
    assert_eq!(refusal["suggestions"], json!(["current", "fast"]));

    host.edit(|p| {
        p.models[0]
            .get_variable_mut("capacity")
            .unwrap()
            .set_scalar_equation("200")
    });
    let refusal = host.refuse(&mut session, "analyze_loops", json!({"run": "fast"}));
    let message = refusal["error"].as_str().unwrap();
    assert!(
        message.contains("revision 0") && message.contains("run it again"),
        "{message}"
    );
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
    let refusal = host.refuse(&mut session, "analyze_loops", json!({}));
    let message = refusal["error"].as_str().unwrap();
    assert!(message.contains("does not simulate"), "{message}");
}

#[test]
fn an_answer_over_its_budget_leaves_out_others_before_partitions_and_leaders_last() {
    // Two partitions of ten loops each; in each, the first loop leads the
    // whole run with the second as its rival.
    let steps = 24;
    let make = |partition: usize, i: usize| {
        let rel = (0..steps)
            .map(|_| match i {
                0 => 0.5,
                1 => 0.4,
                _ => 0.01,
            })
            .collect();
        AnalyzedLoop {
            key: vec![format!("p{partition}l{i}")],
            chain: vec![format!("p{partition}l{i}")],
            partition: Some(partition),
            ..scored(rel)
        }
    };
    let analysis = LoopAnalysis {
        basis: LoopBasis::Run,
        times: (0..steps).map(|t| t as f64).collect(),
        loops: (0..2)
            .flat_map(|p| (0..10).map(move |i| (p, i)))
            .map(|(p, i)| make(p, i))
            .collect(),
        partitions: vec![vec!["a".into(), "b".into()], vec!["c".into()]],
        complete: true,
        capped_from: None,
        conveyors: false,
        at_rest: false,
        cut: None,
    };
    let mut selection = Selection::new(&analysis, None);
    let state = |s: &Selection<'_>| -> Vec<(usize, usize)> {
        s.groups
            .iter()
            .map(|g| (g.leaders, g.listed.len() - g.leaders))
            .collect()
    };
    assert_eq!(state(&selection), [(2, MAX_LOOPS), (2, MAX_LOOPS)]);
    let mut states = vec![state(&selection)];
    while selection.shed() {
        states.push(state(&selection));
    }
    let expected: Vec<Vec<(usize, usize)>> = [
        // The smaller partition's others down to the few, then the larger's.
        (MIN_LOOPS..MAX_LOOPS)
            .rev()
            .map(|n| vec![(2, MAX_LOOPS), (2, n)])
            .collect::<Vec<_>>(),
        (MIN_LOOPS..MAX_LOOPS)
            .rev()
            .map(|n| vec![(2, n), (2, MIN_LOOPS)])
            .collect(),
        // Then the smaller partition, then the first's last others.
        vec![vec![(2, MIN_LOOPS)]],
        (0..MIN_LOOPS).rev().map(|n| vec![(2, n)]).collect(),
        // Last, the rival, whose loop no span names any more.
        vec![vec![(1, 0)]],
    ]
    .concat();
    assert_eq!(states[1..], expected[..]);
    assert_eq!(selection.omitted_partitions, 1);
}

/// A loop for [`dominance`] with the partition-relative series `rel`.
fn scored(rel: Vec<f64>) -> AnalyzedLoop {
    AnalyzedLoop {
        key: vec![],
        chain: vec![],
        signs: vec![],
        polarity: LoopPolarityName::Reinforcing,
        share: Some(mean_abs(&rel)),
        rel,
        partition: Some(0),
        name: None,
    }
}

#[test]
fn a_timeline_merges_windows_while_the_same_loop_leads() {
    // 24 steps, 12 windows of 2: a leads the first 8 steps with b its rival,
    // then b alone, a holding under half b's share.
    let a = scored((0..24).map(|t| if t < 8 { 0.6 } else { 0.4 }).collect());
    let b = scored((0..24).map(|t| if t < 8 { -0.4 } else { -0.95 }).collect());
    let spans = dominance(&[&a, &b], 24);
    let summary: Vec<(usize, usize, Vec<usize>)> = spans
        .iter()
        .map(|s| (s.start, s.end, s.leaders.iter().map(|&(i, _)| i).collect()))
        .collect();
    assert_eq!(summary, [(0, 8, vec![0, 1]), (8, 24, vec![1])]);
    assert!(
        (spans[1].leaders[0].1 - 0.95).abs() < 1e-12,
        "shares are |rel|"
    );

    // Many loops sharing the activity: the strongest leads however small
    // its share.
    let many: Vec<AnalyzedLoop> = (0..40)
        .map(|i| scored(vec![0.02 + 0.0001 * i as f64; 24]))
        .collect();
    let refs: Vec<&AnalyzedLoop> = many.iter().collect();
    let spans = dominance(&refs, 24);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].leaders.len(), MAX_LEADERS);
    assert_eq!(spans[0].leaders[0].0, 39);

    // A loop alternating with another every step never merges past the
    // window grid: at most WINDOWS spans.
    let n = 10 * WINDOWS;
    let a = scored((0..n).map(|t| if t % 2 == 0 { 1.0 } else { 0.0 }).collect());
    let b = scored((0..n).map(|t| if t % 2 == 0 { 0.0 } else { 1.0 }).collect());
    assert!(dominance(&[&a, &b], n).len() <= WINDOWS);

    // Fewer steps than windows: one window a step.
    let a = scored(vec![0.0, 1.0, 0.0]);
    let b = scored(vec![0.0, 0.0, 1.0]);
    let summary: Vec<(usize, usize)> = dominance(&[&a, &b], 3)
        .iter()
        .map(|s| (s.start, s.end))
        .collect();
    assert_eq!(summary, [(0, 1), (1, 2), (2, 3)]);

    // No loop active: one span with no leaders.
    let quiet = scored(vec![0.0; 40]);
    let spans = dominance(&[&quiet], 40);
    assert_eq!(spans.len(), 1);
    assert!(spans[0].leaders.is_empty());
}

#[test]
fn a_loops_polarity_reads_the_same_from_a_run_and_from_structure() {
    let pairs = [
        (LoopPolarity::Reinforcing, DetectedLoopPolarity::Reinforcing),
        (LoopPolarity::Balancing, DetectedLoopPolarity::Balancing),
        (
            LoopPolarity::MostlyReinforcing,
            DetectedLoopPolarity::MostlyReinforcing,
        ),
        (
            LoopPolarity::MostlyBalancing,
            DetectedLoopPolarity::MostlyBalancing,
        ),
        (
            LoopPolarity::Undetermined,
            DetectedLoopPolarity::Undetermined,
        ),
    ];
    let names: Vec<LoopPolarityName> = pairs
        .into_iter()
        .map(|(run, structure)| {
            let name = LoopPolarityName::from(run);
            assert_eq!(name, LoopPolarityName::from(structure));
            name
        })
        .collect();
    assert_eq!(names, LoopPolarityName::ALL);
}

/// The corpus's two largest models' loops are analyzed within a budget of
/// time and size, each call timed.
///
/// Run with: scripts/gates.sh --nocapture the_largest_corpus_models_loops_are_analyzed_within_bounds
#[test]
#[ignore = "compiles World3 and C-LEARN under the LTM overlay; run under the gates profile"]
fn the_largest_corpus_models_loops_are_analyzed_within_bounds() {
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
        let output = host.call_raw(&mut session, "analyze_loops", "{}");
        let first = started.elapsed();
        assert!(!output.is_error, "{}", output.json);
        let started = std::time::Instant::now();
        let again = host.call_raw(&mut session, "analyze_loops", "{}");
        let kept = started.elapsed();
        assert_eq!(again, output);
        let analysis: Value = serde_json::from_str(&output.json).unwrap();
        let partitions = analysis["partitions"].as_array().unwrap();
        let listed: usize = partitions
            .iter()
            .map(|p| p["loops"].as_array().unwrap().len())
            .sum();
        let longest = partitions
            .iter()
            .flat_map(|p| p["loops"].as_array().unwrap())
            .map(|l| l["length"].as_u64().unwrap())
            .max()
            .unwrap_or(0);
        eprintln!(
            "{}: {} bytes, basis {}, complete {}, found {}, {} partitions, {} loops listed \
             (longest chain {longest}), omitted {}, note {}; first call {first:?}, kept {kept:?}",
            path.display(),
            output.json.len(),
            analysis["basis"],
            analysis["complete"],
            analysis["found"],
            partitions.len(),
            listed,
            analysis["omitted"],
            analysis["note"],
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
fn a_model_with_a_conveyor_says_its_loops_through_it_are_not_analyzed() {
    // Alumni recruit students, who graduate through a conveyor into alumni:
    // a loop through the conveyor, which the engine neither scores in a run
    // nor sees in the structure.
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>pipeline</name><vendor>test</vendor><product version="1.0">test</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>12</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="Students"><eqn>100</eqn><inflow>matriculating</inflow><outflow>graduating</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="matriculating"><eqn>Alumni * 0.1</eqn><non_negative/></flow>
    <flow name="graduating"></flow>
    <stock name="Alumni"><eqn>10</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#;
    let project = crate::open_xmile(&mut std::io::BufReader::new(xml.as_bytes())).unwrap();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "structure");
    assert_eq!(output["complete"], false);
    let note = output["note"].as_str().unwrap();
    assert!(
        note.contains("does not analyze loops through a conveyor or a queue"),
        "{note}"
    );
    assert!(!note.contains("no feedback loops"), "{note}");
}

/// Logistic growth's lead changes at the step where the balancing loop
/// overtakes the reinforcing one, not at the edge of the window it falls in.
#[test]
fn a_change_of_lead_is_placed_at_the_step_it_happens() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let spans = output["partitions"][0]["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 2, "{output}");
    let switch = spans[1]["from"].as_f64().unwrap();
    // The crossing, from the run's own relative scores.
    let model = host.project.models[0].clone();
    let run = session
        .runs
        .get(&mut host.workspace(), &model, "current")
        .unwrap();
    let analysis = run.loops.get().unwrap();
    let (a, b) = (&analysis.loops[0], &analysis.loops[1]);
    let crossing = (1..analysis.times.len())
        .find(|&k| (a.rel[k].abs() > b.rel[k].abs()) != (a.rel[k - 1].abs() > b.rel[k - 1].abs()))
        .map(|k| analysis.times[k])
        .unwrap();
    assert!(
        (switch - crossing).abs() <= 0.125 + 1e-9,
        "switch {switch}, crossing {crossing}"
    );
}

/// A run whose stocks do not move, to the precision a summary reports, is at
/// rest whatever its loop scores round to: its loops come from structure.
#[test]
fn a_run_at_rest_to_the_reported_precision_has_no_dominance() {
    let project = TestProject::new("barely")
        .with_sim_time(0.0, 20.0, 1.0)
        .stock("level", "100", &["adjustment"], &[], None)
        .flow("adjustment", "(100.0001 - level) / 4", None);
    let mut host = Host::from_test_project(&project);
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    assert_eq!(output["basis"], "structure", "{output}");
    assert!(
        output["partitions"][0]["dominance"]
            .as_array()
            .is_none_or(Vec::is_empty)
    );
    assert!(
        output["note"].as_str().unwrap().contains("do not move"),
        "{output}"
    );
}

/// A link through an aggregate (a SUM over an array's elements) is signed by
/// composing the links into and out of it, as a builtin's internals are.
#[test]
fn a_link_through_an_aggregate_is_signed() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test/cross_agg_ltm/cross_agg.stmx");
    let file = std::fs::File::open(path).expect("the aggregate model is in the corpus");
    let project = crate::xmile::project_from_reader(&mut std::io::BufReader::new(file)).unwrap();
    // The same loops from structure, in a copy at rest.
    let at_rest = TestProject::new("still")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("region", &["a", "b", "c"])
        .array_stock("pop[region]", "0", &["growth"], &[], None)
        .array_flow("growth[region]", "SUM(pop[*]) * 0.05", None);
    for (mut host, basis) in [
        (Host::new(project), "run"),
        (Host::from_test_project(&at_rest), "structure"),
    ] {
        let output = analyze(&mut host, &mut Session::new("main"), json!({}));
        assert_eq!(output["basis"], basis);
        let mut links = 0;
        for partition in output["partitions"].as_array().unwrap() {
            for l in partition["loops"].as_array().unwrap() {
                assert_eq!(l["polarity"], "reinforcing", "{l}");
                for step in l["chain"].as_array().into_iter().flatten() {
                    links += 1;
                    assert_eq!(step["polarity"], "+", "{l}");
                }
            }
        }
        assert!(links > 0, "{output}");
    }
}

#[test]
fn through_finds_the_loops_of_one_element() {
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 10.0, 0.25)
        .named_dimension("region", &["north", "south"])
        .array_stock("population[region]", "100", &["births"], &[], None)
        .array_flow("births[region]", "population * rate", None)
        .aux("rate", "0.1", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let all = analyze(&mut host, &mut session, json!({"through": "population"}));
    let north = analyze(
        &mut host,
        &mut session,
        json!({"through": "population[North]"}),
    );
    assert_eq!(all["found"], 2, "{all}");
    assert_eq!(north["found"], 1, "{north}");
    let chain = &north["partitions"][0]["loops"][0]["chain"];
    assert!(chain.to_string().contains("population[north]"), "{north}");
    let refusal = host.refuse(
        &mut session,
        "analyze_loops",
        json!({"through": "population[east]"}),
    );
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("not an element"),
        "{refusal}"
    );
}

/// A diagram edit leaves a run's loop analysis as it was: the run is fresh,
/// its loops are analyzed from it, and the analysis kept with it answers.
#[test]
fn a_layout_edit_leaves_a_runs_loops_to_analyze() {
    let mut host = Host::new(crate::tools::test_support::with_diagram(
        logistic().build_datamodel(),
    ));
    let mut session = Session::new("main");
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "faster", "set": [{"variable": "max_rate", "value": 0.8}]}),
    );
    let before = analyze(&mut host, &mut session, json!({"run": "faster"}));
    let model = host.project.models[0].clone();
    let current = session
        .runs
        .get(&mut host.workspace(), &model, "current")
        .unwrap();
    host.edit(crate::tools::test_support::zoom_the_diagram);
    let after = analyze(&mut host, &mut session, json!({"run": "faster"}));
    assert_eq!(after["partitions"], before["partitions"]);
    let model = host.project.models[0].clone();
    let again = session
        .runs
        .get(&mut host.workspace(), &model, "current")
        .unwrap();
    assert!(
        Arc::ptr_eq(&current, &again),
        "the current run is not simulated again"
    );
}

/// An answer keeps to the session's budget by shedding loops, and loops asked
/// for by id past the budget are left for another call.
#[test]
fn loop_answers_keep_to_the_budget() {
    let mut project = TestProject::new("many")
        .with_sim_time(0.0, 10.0, 0.25)
        .stock("level", "1", &["growth"], &[], None);
    let mut terms = Vec::new();
    for i in 0..12 {
        project = project.aux(&format!("path_{i:02}"), "level * 0.01", None);
        terms.push(format!("path_{i:02}"));
    }
    let project = project.flow("growth", &terms.join(" + "), None);
    let mut host = Host::from_test_project(&project);
    let mut roomy = Session::new("main");
    roomy.outline_budget = usize::MAX;
    let whole = analyze(&mut host, &mut roomy, json!({}));
    let whole_len = whole.to_string().len();
    let listed = |output: &Value| output["partitions"][0]["loops"].as_array().unwrap().len();
    assert!(listed(&whole) > MIN_LOOPS, "{whole}");

    let mut tight = Session::new("main");
    tight.outline_budget = whole_len / 2;
    let fitted = analyze(&mut host, &mut tight, json!({}));
    assert!(fitted.to_string().len() <= whole_len / 2, "{fitted}");
    assert!(listed(&fitted) < listed(&whole));
    assert!(fitted["omitted"]["loops"].as_u64().unwrap() > 0);

    let ids: Vec<Value> = whole["partitions"][0]["loops"]
        .as_array()
        .unwrap()
        .iter()
        .take(4)
        .map(|l| l["id"].clone())
        .collect();
    // Room for two of the four asked for, less what naming the others takes.
    let two = analyze(&mut host, &mut roomy, json!({"loops": ids[..2]}));
    let mut small = Session::new("main");
    small.outline_budget = two.to_string().len();
    analyze(&mut host, &mut small, json!({}));
    let fitted = analyze(&mut host, &mut small, json!({"loops": ids}));
    assert!(fitted.to_string().len() <= small.outline_budget, "{fitted}");
    let returned = listed(&fitted);
    let left: Vec<Value> = fitted["leftOut"].as_array().cloned().unwrap_or_default();
    assert!((1..=2).contains(&returned), "{fitted}");
    assert_eq!(
        left,
        ids[returned..],
        "the last asked for are left out, in order"
    );
}

/// The project's discovery mode is set back however an analysis ends, a
/// panic included.
#[test]
fn the_discovery_mode_is_set_back_after_a_panic() {
    let mut host = Host::from_test_project(&logistic());
    let source_project = host.db.current_source_project().unwrap();
    let before = source_project.ltm_discovery_mode(&host.db);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_discovery_mode(&mut host.db, source_project, !before, |_| {
            panic!("an analysis that fails partway")
        })
    }));
    assert!(result.is_err());
    assert_eq!(source_project.ltm_discovery_mode(&host.db), before);
}

/// A link whose sign changes over the run reads "?", as the loop it makes
/// undetermined does, rather than the sign its equation would suggest.
#[test]
fn a_link_whose_sign_changes_reads_unknown_in_its_undetermined_loop() {
    let project = TestProject::new("flipping")
        .with_sim_time(0.0, 20.0, 0.125)
        .stock("level", "1", &["change"], &[], None)
        .flow("change", "(desired - level) / 4", None)
        .aux("desired", "2 + level * impact", None)
        .aux("impact", "COS(TIME)", None);
    let mut host = Host::from_test_project(&project);
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    let loops = output["partitions"][0]["loops"].as_array().unwrap();
    let through_desired = loops
        .iter()
        .find(|l| l["chain"].to_string().contains("\"desired\""))
        .unwrap_or_else(|| panic!("{output}"));
    assert_eq!(through_desired["polarity"], "undetermined", "{output}");
    let signs: Vec<(&str, &str)> = through_desired["chain"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            (
                step["variable"].as_str().unwrap(),
                step["polarity"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(signs, [("level", "?"), ("desired", "+"), ("change", "+")]);
}

/// A run whose stocks move while none of its loops is active says that its
/// loops' scores are zero, not that it is at rest.
#[test]
fn a_run_with_no_active_loop_says_why() {
    let still_loop_moving_stock = TestProject::new("apart")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "100", &["adjustment"], &[], None)
        .flow("adjustment", "(100 - level) / 4", None)
        .stock("tally", "0", &["arrivals"], &[], None)
        .flow("arrivals", "1", None);
    let mut host = Host::from_test_project(&still_loop_moving_stock);
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    assert_eq!(output["basis"], "structure", "{output}");
    let note = output["note"].as_str().unwrap();
    assert!(note.contains("every loop score is zero"), "{note}");

    let mut host = Host::from_test_project(&at_rest());
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    let note = output["note"].as_str().unwrap();
    assert!(note.contains("its stocks do not move"), "{note}");
}

/// An analysis stops before each of its stages once other work waits for the
/// project -- the run it reads, its replay under the overlay, the discovery
/// over the replay, the loops from structure of a run at rest -- and between
/// the slices of each run, and keeps nothing: the next analysis numbers its
/// loops as if none had begun.
#[test]
fn an_analysis_stops_before_each_stage_when_other_work_waits_and_keeps_nothing() {
    for (project, stages) in [(logistic(), 3), (at_rest(), 4)] {
        let mut stopped = 0;
        for after in 1.. {
            let mut host = Host::from_test_project(&project);
            let mut session = Session::new("main");
            let output = host.call_waiting(
                &mut session,
                "analyze_loops",
                json!({}),
                &Host::waiting_after(after),
            );
            if !output.is_error {
                break;
            }
            assert!(
                output.json.contains("\"interrupted\":true"),
                "{}",
                output.json
            );
            stopped += 1;
            let answer = analyze(&mut host, &mut session, json!({}));
            let first = &answer["partitions"][0]["loops"][0]["id"];
            assert_eq!(first, "L1", "an interrupted analysis gave no ids: {answer}");
        }
        assert!(
            stopped > stages,
            "every stage, and every slice of a run, is a place to stop: {stopped}"
        );
    }
}
