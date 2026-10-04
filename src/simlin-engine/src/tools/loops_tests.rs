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

/// A partition's timeline is the run's steps in at most [`MAX_SPANS`] spans,
/// each from its first step's time to its last's and the next starting after
/// it, no two neighbours led by one loop, each naming at most [`MAX_LEADERS`]
/// loops: its leader, then its rivals strongest first, each holding at least
/// [`RIVAL_SHARE`] of the leader's share; and every loop it names is listed.
fn check_timeline(partition: &Value) {
    let Some(spans) = partition["dominance"].as_array() else {
        return;
    };
    assert!(!spans.is_empty() && spans.len() <= MAX_SPANS, "{partition}");
    let time = |span: &Value, end: &str| span[end].as_f64().unwrap();
    for span in spans {
        assert!(time(span, "from") <= time(span, "to"), "{partition}");
    }
    for pair in spans.windows(2) {
        assert!(
            time(&pair[0], "to") < time(&pair[1], "from"),
            "spans follow one another: {partition}"
        );
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
        if let Some((&leader, rivals)) = shares.split_first() {
            assert!(rivals.windows(2).all(|w| w[0] >= w[1]), "{span}");
            // Shares are rounded to hundredths, so compare within that.
            assert!(
                rivals.iter().all(|&s| s + 0.01 >= RIVAL_SHARE * leader),
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

/// Every loop an answer's partitions list.
fn listed(output: &Value) -> Vec<&Value> {
    output["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| p["loops"].as_array().unwrap())
        .collect()
}

/// The loop in `output`'s partitions of `polarity`, which must be the only one.
fn only_loop<'a>(output: &'a Value, polarity: &str) -> &'a Value {
    let found: Vec<&Value> = listed(output)
        .into_iter()
        .filter(|l| l["polarity"] == polarity)
        .collect();
    assert_eq!(found.len(), 1, "one {polarity} loop: {output}");
    found[0]
}

/// The loop analysis the session keeps of `run`, as a call made it.
fn kept_analysis(host: &mut Host, session: &mut Session, run: &str) -> Arc<LoopAnalysis> {
    let model = host
        .project
        .models
        .iter()
        .find(|m| m.name == session.model_name)
        .unwrap()
        .clone();
    let run = session
        .runs
        .get(&mut host.workspace(), &model, run)
        .unwrap();
    run.loops
        .get()
        .expect("the run's loops were analyzed")
        .clone()
}

/// The model's loops from structure, as the tool reads them, out of discovery
/// mode: `None` when the structure alone does not enumerate them.
fn structure_of(host: &mut Host, model_name: &str) -> Option<Structure> {
    let source_project = host.db.current_source_project().unwrap();
    let model = host
        .project
        .models
        .iter()
        .find(|m| m.name == model_name)
        .unwrap()
        .clone();
    let canonical = crate::canonicalize(&model.name).into_owned();
    let source_model = *source_project.models(&host.db).get(&canonical)?;
    let project = host.project.clone();
    with_discovery_mode(&mut host.db, source_project, false, |db| {
        structural_loops(db, source_project, source_model, &model, &project)
    })
}

/// What a loop analysis holds whatever the model, checked against what the
/// engine says by other routes; the breaches, for `label`.
///
/// - No two of its loops have one key, and it has as many loops as discovery
///   found for the same run.
/// - A run's loop is one of the model's structural loops under the same key,
///   where the structure enumerates them; the loops it lists as inactive are
///   the rest of them.
/// - Where the run has every loop, a partition's shares sum to one.
/// - Every boundary of a partition's timeline is a step at which the loops
///   that hold the lead change, and its spans tile the run.
fn breaches(
    label: &str,
    host: &mut Host,
    model_name: &str,
    analysis: &LoopAnalysis,
) -> Vec<String> {
    let mut breaches = Vec::new();
    let mut keys: Vec<&Vec<String>> = analysis.loops.iter().map(|l| &l.key).collect();
    keys.sort();
    if keys.windows(2).any(|pair| pair[0] == pair[1]) {
        breaches.push(format!("{label}: two loops have one key"));
    }
    if analysis.basis != LoopBasis::Run {
        return breaches;
    }

    let structure = structure_of(host, model_name);
    let source_project = host.db.current_source_project().unwrap();
    let discovered = crate::analysis::analyze_model(
        &host.project,
        &mut host.db,
        source_project,
        model_name,
        Some(DISCOVERY_BUDGET),
    )
    .unwrap()
    .loop_dominance
    .len();
    if discovered != analysis.loops.len() {
        breaches.push(format!(
            "{label}: discovery found {discovered} loops, the analysis has {}",
            analysis.loops.len()
        ));
    }

    if let Some(structure) = structure {
        let structural: HashSet<&Vec<String>> = structure.loops.iter().map(|l| &l.key).collect();
        for l in &analysis.loops {
            if !structural.contains(&l.key) {
                breaches.push(format!("{label}: no structural loop is {:?}", l.key));
            }
        }
        let inactive = analysis.inactive.as_ref().map_or(0, Vec::len);
        if analysis.loops.len() + inactive != structure.loops.len() {
            breaches.push(format!(
                "{label}: {} active and {inactive} inactive loops of {} structural",
                analysis.loops.len(),
                structure.loops.len()
            ));
        }
    }

    let steps = analysis.times.len();
    for (partition, members) in by_partition(analysis.loops.iter().map(|l| (l.partition, l))) {
        if partition.is_none() {
            continue;
        }
        if analysis.complete() {
            let total: f64 = members.iter().filter_map(|l| l.share).sum();
            if (total - 1.0).abs() > 1e-6 {
                breaches.push(format!(
                    "{label}: partition {partition:?}'s shares sum to {total}"
                ));
            }
        }
        // The loops that hold the lead at a step, read off the scores alone:
        // those within a tie of the largest share, none where no loop is
        // active.
        let top = |k: usize| -> Vec<usize> {
            let largest = members
                .iter()
                .map(|l| l.rel[k].abs())
                .fold(0.0_f64, f64::max);
            if largest < ACTIVE_SHARE {
                return vec![];
            }
            (0..members.len())
                .filter(|&i| {
                    members[i].rel[k].abs() >= largest * (1.0 - crate::ltm_dominance::LEAD_TIE)
                })
                .collect()
        };
        let spans = timeline(&members, steps, |_| true);
        if spans.first().map(|s| s.start) != Some(0) || spans.last().map(|s| s.end) != Some(steps) {
            breaches.push(format!(
                "{label}: partition {partition:?}'s spans do not tile"
            ));
        }
        for pair in spans.windows(2) {
            let at = pair[1].start;
            if pair[0].end != at {
                breaches.push(format!(
                    "{label}: partition {partition:?}'s spans do not tile"
                ));
            }
            // A lead passes only where the loops that hold it change: where
            // they are the same as the step before, the loop that led still
            // does.
            if at != 1 && top(at) == top(at - 1) {
                breaches.push(format!(
                    "{label}: partition {partition:?} has a boundary at step {at} of {steps}, \
                     where the lead does not change"
                ));
            }
        }
    }
    breaches
}

/// The span leaders `answer` (an `analyze_loops` answer) names that the
/// verifier does not confirm, for `label`, and how many it names: each, cited
/// back as `leads` over its span's `from` and `to` as the answer prints them,
/// must hold, since the timeline and the verifier read a lead by one rule.
fn unconfirmed_leaders(
    label: &str,
    host: &mut Host,
    session: &mut Session,
    answer: &Value,
) -> (Vec<String>, usize) {
    let mut unconfirmed = Vec::new();
    let mut named = 0;
    for partition in answer["partitions"].as_array().into_iter().flatten() {
        for span in partition["dominance"].as_array().into_iter().flatten() {
            let Some(leader) = span["leaders"][0]["loop"].as_str() else {
                continue;
            };
            named += 1;
            let citation = json!({
                "cites": "leads", "loop": leader, "from": span["from"], "to": span["to"]
            });
            let verdict = host.call(
                session,
                "verify_findings",
                json!({"findings": [{
                    "kind": "observation", "claim": "a span's leader", "citations": [citation]
                }]}),
            );
            if verdict["findings"][0]["holds"] != true {
                unconfirmed.push(format!("{label}: {citation} does not hold: {verdict}"));
            }
        }
    }
    (unconfirmed, named)
}

/// Analyze `project`'s current run and check it against [`breaches`] and
/// [`unconfirmed_leaders`].
fn analyzed(label: &str, project: datamodel::Project) -> (Host, Session, Arc<LoopAnalysis>) {
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let answer = analyze(&mut host, &mut session, json!({}));
    let analysis = kept_analysis(&mut host, &mut session, "current");
    let mut found = breaches(label, &mut host, "main", &analysis);
    found.extend(unconfirmed_leaders(label, &mut host, &mut session, &answer).0);
    assert!(found.is_empty(), "{found:#?}");
    (host, session, analysis)
}

fn corpus_model(path: &str) -> datamodel::Project {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    let text = std::fs::read_to_string(&path).expect("the corpus model is present");
    if path.extension().is_some_and(|e| e == "mdl") {
        crate::compat::open_vensim(&text).expect("the corpus model parses")
    } else {
        crate::open_xmile(&mut std::io::BufReader::new(text.as_bytes()))
            .expect("the corpus model parses")
    }
}

/// Logistic growth with its two loops apart: births compound the population
/// (reinforcing), and crowding lowers the fractional birth rate as the
/// population nears its capacity (balancing). The reinforcing loop drives the
/// early growth and the balancing loop the approach to capacity; the switch is
/// at the inflection, when the population is half its capacity (t ~ 9.2).
fn logistic() -> TestProject {
    logistic_until(40.0, 0.125)
}

fn logistic_until(stop: f64, dt: f64) -> TestProject {
    TestProject::new("logistic")
        .with_sim_time(0.0, stop, dt)
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

/// An epidemic: contagion (reinforcing) leads until the susceptible run low
/// (balancing), then recovery (balancing) to the end.
fn sir() -> TestProject {
    TestProject::new("sir")
        .with_sim_time(0.0, 100.0, 0.25)
        .stock("susceptible", "999", &[], &["infection"], None)
        .stock("infected", "1", &["infection"], &["recovery"], None)
        .stock("recovered", "0", &["recovery"], &[], None)
        .flow(
            "infection",
            "contact_rate * infectivity * susceptible * infected / total_population",
            None,
        )
        .flow("recovery", "infected / duration", None)
        .aux(
            "total_population",
            "susceptible + infected + recovered",
            None,
        )
        .aux("contact_rate", "6", None)
        .aux("infectivity", "0.1", None)
        .aux("duration", "5", None)
}

/// An inventory a workforce is hired to keep, stepped at 10: an oscillation
/// in which the hiring loop and the inventory loop trade the lead every half
/// cycle, ten times.
fn inventory_workforce() -> TestProject {
    TestProject::new("inventory workforce")
        .with_sim_time(0.0, 100.0, 0.25)
        .stock("inventory", "400", &["production"], &["shipments"], None)
        .stock("workforce", "100", &["net_hiring"], &[], None)
        .flow("production", "workforce * productivity", None)
        .flow("shipments", "demand", None)
        .flow(
            "net_hiring",
            "(desired_workforce - workforce) / hiring_time",
            None,
        )
        .aux("demand", "100 + STEP(20, 10)", None)
        .aux("productivity", "1", None)
        .aux("desired_inventory", "demand * coverage", None)
        .aux("coverage", "4", None)
        .aux(
            "desired_production",
            "demand + (desired_inventory - inventory) / inventory_adjustment_time",
            None,
        )
        .aux(
            "desired_workforce",
            "desired_production / productivity",
            None,
        )
        .aux("inventory_adjustment_time", "4", None)
        .aux("hiring_time", "8", None)
}

/// Predators and prey: five loops, four and a half cycles, and so more
/// changes of lead than a timeline has spans.
fn predator_prey() -> TestProject {
    TestProject::new("predator prey")
        .with_sim_time(0.0, 60.0, 0.0625)
        .stock("hares", "50", &["hare_births"], &["hare_deaths"], None)
        .stock("lynx", "10", &["lynx_births"], &["lynx_deaths"], None)
        .flow("hare_births", "hares * 0.8", None)
        .flow("hare_deaths", "hares * lynx * 0.02", None)
        .flow("lynx_births", "hares * lynx * 0.004", None)
        .flow("lynx_deaths", "lynx * 0.4", None)
}

/// A level read directly and through a smooth in one equation, as a trend or
/// an anchor is: two loops between the same two variables.
fn trend() -> TestProject {
    TestProject::new("trend")
        .with_sim_time(0.0, 40.0, 0.25)
        .stock("level", "10", &["change"], &[], None)
        .flow(
            "change",
            "(20 - level) / 4 + 0.5 * (level - SMTH1(level, 3))",
            None,
        )
}

/// Each region's population grows on its own, from a seed: at rest until
/// seeded.
fn regions() -> TestProject {
    TestProject::new("regions")
        .with_sim_time(0.0, 10.0, 0.25)
        .named_dimension("region", &["north", "south"])
        .array_stock("population[region]", "0", &["births"], &[], None)
        .array_flow("births[region]", "population * 0.1 + seed", None)
        .aux("seed", "0", None)
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
    for absent in ["omitted", "note", "inactive"] {
        assert!(output.get(absent).is_none(), "{absent}: {output}");
    }
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
    assert_eq!(
        share(growth) + share(crowding),
        1.0,
        "the partition's shares sum to one"
    );

    let spans = partitions[0]["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 2, "{spans:?}");
    assert_eq!(spans[0]["from"], 0.0);
    assert_eq!(spans[0]["leaders"][0]["loop"], growth["id"]);
    assert_eq!(spans[1]["to"], 40.0);
    assert_eq!(spans[1]["leaders"][0]["loop"], crowding["id"]);
    // A span's shares are over its own steps: each phase's leader holds most
    // of its span, which the run's shares, summing to one, could not both
    // show.
    for span in spans {
        let leader = span["leaders"][0]["share"].as_f64().unwrap();
        assert!(leader > 0.5, "{span}");
    }
    // The lead changes at the step the loops' scores cross.
    let analysis = kept_analysis(&mut host, &mut session, "current");
    let (a, b) = (&analysis.loops[0], &analysis.loops[1]);
    let crossing = (2..analysis.times.len())
        .find(|&k| (a.rel[k].abs() > b.rel[k].abs()) != (a.rel[k - 1].abs() > b.rel[k - 1].abs()))
        .map(|k| analysis.times[k])
        .unwrap();
    assert_eq!(spans[1]["from"], crossing);
    assert!((9.0..=10.0).contains(&crossing), "near the inflection");
}

/// A timeline's spans are cut where the lead changes, so a growth phase a
/// small part of a long run still has its span: the run's length does not
/// decide what a timeline shows.
#[test]
fn a_long_run_keeps_the_span_of_its_growth_phase() {
    let (mut host, mut session, _) = analyzed(
        "long logistic",
        logistic_until(400.0, 0.5).build_datamodel(),
    );
    let output = analyze(&mut host, &mut session, json!({}));
    let growth = only_loop(&output, "reinforcing");
    let crowding = only_loop(&output, "balancing");
    let spans = output["partitions"][0]["dominance"].as_array().unwrap();
    let leaders: Vec<&Value> = spans.iter().map(|s| &s["leaders"][0]["loop"]).collect();
    assert_eq!(
        leaders,
        [&growth["id"], &crowding["id"], &Value::Null],
        "growth, crowding, then at rest: {spans:?}"
    );
    let switch = spans[1]["from"].as_f64().unwrap();
    assert!((8.0..=11.0).contains(&switch), "{spans:?}");
    let settled = spans[2]["from"].as_f64().unwrap();
    assert!((50.0..=150.0).contains(&settled), "{spans:?}");
    // Shares are of the steps the partition is active at, however long the
    // run goes on at rest.
    let share = |l: &Value| l["share"].as_f64().unwrap();
    assert_eq!(share(growth) + share(crowding), 1.0);
}

/// The analyses of an epidemic, an oscillator and a predator-prey cycle keep
/// the rules [`breaches`] states: boundaries at changes of lead, shares
/// summing to one, the loops discovery found, each under its structural key.
#[test]
fn an_analysis_agrees_with_the_scores_and_the_structure_it_is_read_from() {
    let (.., epidemic) = analyzed("sir", sir().build_datamodel());
    let lead_changes = |analysis: &LoopAnalysis| {
        let members: Vec<&AnalyzedLoop> = analysis.loops.iter().collect();
        timeline(&members, analysis.times.len(), |_| true).len() - 1
    };
    assert_eq!(lead_changes(&epidemic), 2, "contagion, depletion, recovery");

    // Ten changes of lead, each at its step: none before the step in demand
    // at 10 wakes the loops, then the two loops in turn.
    let (mut host, mut session, _) = analyzed(
        "inventory workforce",
        inventory_workforce().build_datamodel(),
    );
    let output = analyze(&mut host, &mut session, json!({}));
    let spans = output["partitions"][0]["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 11, "{spans:?}");
    assert_eq!(spans[0]["leaders"], json!([]));
    assert_eq!(
        (&spans[0]["to"], &spans[1]["from"]),
        (&json!(10.25), &json!(10.5)),
        "led from the first step after the step in demand"
    );
    for pair in spans[1..].windows(3) {
        assert_eq!(
            pair[0]["leaders"][0]["loop"], pair[2]["leaders"][0]["loop"],
            "the two loops alternate: {spans:?}"
        );
    }

    // More changes of lead than spans: the shortest are joined, and what is
    // left is still cut at changes of lead.
    let (.., cycle) = analyzed("predator prey", predator_prey().build_datamodel());
    assert_eq!(lead_changes(&cycle), MAX_SPANS - 1);
    assert_eq!(cycle.loops.len(), 5);
}

#[test]
fn a_loop_keeps_its_id_across_runs_and_edits() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let ids = |output: &Value| -> Vec<(Vec<String>, String)> {
        let mut ids: Vec<(Vec<String>, String)> = listed(output)
            .into_iter()
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
    assert_eq!(partition["loops"][0]["share"], 1.0);
    let spans = partition["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(
        (&spans[0]["to"], &spans[0]["leaders"], &spans[1]["from"]),
        (&json!(5.0), &json!([]), &json!(5.25)),
        "no loop leads until the level has moved, the step after the goal's"
    );
    assert_eq!(spans[1]["leaders"][0]["loop"], "L1");
}

/// A run whose stocks barely move is a run all the same: its loop's scores
/// are ratios of the changes, as exact a tenth of a thousandth from the goal
/// as a tenth of it away, so the run is analyzed from them.
#[test]
fn a_run_that_moves_a_little_is_analyzed_from_its_scores() {
    let project = TestProject::new("barely")
        .with_sim_time(0.0, 20.0, 1.0)
        .stock("level", "100", &["adjustment"], &[], None)
        .flow("adjustment", "(100.0001 - level) / 4", None);
    let mut host = Host::from_test_project(&project);
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    assert_eq!(output["basis"], "run", "{output}");
    assert!(output.get("note").is_none(), "{output}");
    let adjustment = only_loop(&output, "balancing");
    assert_eq!(adjustment["share"], 1.0);
    let spans = output["partitions"][0]["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0]["leaders"][0]["loop"], adjustment["id"]);
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
    assert!(
        output.get("inactive").is_none(),
        "no structure to say what else there is"
    );
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
    assert!(
        output.get("inactive").is_none(),
        "the cut model has no other loop: {output}"
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

/// A link's sign is weighted by time, as its loop's polarity is: mixed signs
/// netting to at least 99% count as the one sign, and the tally is of the
/// link's share of its target's change at each step, never of its raw score.
#[test]
fn a_links_sign_is_the_one_it_held_for_all_but_a_hundredth_of_the_run() {
    // The level creeps up the hump and passes its peak at the run's last
    // step: positive for all but that one. The run is cut where a longer one
    // shows the link turn.
    let (dt, long) = (0.03125, hump("0.2").with_sim_time(0.0, 16.0, 0.03125));
    let mut host = Host::from_test_project(&long);
    let mut session = Session::new("main");
    analyze(&mut host, &mut session, json!({}));
    let analysis = kept_analysis(&mut host, &mut session, "current");
    let turn = analysis.loops[0].rel.iter().position(|s| *s < 0.0).unwrap();
    let mut host =
        Host::from_test_project(&hump("0.2").with_sim_time(0.0, analysis.times[turn], dt));
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let analysis = kept_analysis(&mut host, &mut session, "current");
    let negative = analysis.loops[0].rel.iter().filter(|s| **s < 0.0).count();
    let active = analysis.loops[0].rel.iter().filter(|s| **s != 0.0).count();
    assert!(
        negative == 1 && active >= 200,
        "the premise: {negative} of {active} steps past the peak"
    );
    let l = only_loop(&output, "mostly_reinforcing");
    assert_eq!(
        chain(l),
        steps(&[("level", "+"), ("effect", "+"), ("growth", "+")]),
        "{l}"
    );

    // A link positive through the whole of a growth phase and negative
    // after, whose raw score is hundreds of times larger after (its target's
    // inputs nearly cancel there): its sign changed, whatever the raw scores
    // sum to, and its undetermined loop shows the link that changed.
    let project = TestProject::new("hump with drift")
        .with_sim_time(0.0, 10.0, 0.0625)
        .stock("level", "0.1", &["growth"], &[], None)
        .flow("growth", "level * (2 - level) + drift", None)
        .aux("drift", "-0.02 * TIME", None);
    let mut host = Host::from_test_project(&project);
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    let l = only_loop(&output, "undetermined");
    assert_eq!(chain(l), steps(&[("level", "?"), ("growth", "+")]), "{l}");
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
    // Only the loops through the variable are listed, and the timeline is of
    // them: no loop through it leads while growth does.
    let crowding = only_loop(&output, "balancing");
    assert_eq!(listed(&output).len(), 1, "{output}");
    assert!(
        chain(crowding)
            .iter()
            .any(|(v, _)| v == "fractional_birth_rate")
    );
    let spans = partitions[0]["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 2, "{spans:?}");
    assert_eq!(spans[0]["leaders"], json!([]));
    assert_eq!(spans[1]["leaders"][0]["loop"], crowding["id"]);

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

/// Twelve loops through one stock, each a little stronger than the last.
fn twelve_paths() -> TestProject {
    let mut project = TestProject::new("paths")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", "1", &["growth"], &[], None);
    let mut terms = Vec::new();
    for i in 0..12 {
        project = project.aux(
            &format!("path_{i:02}"),
            &format!("level * {}", 0.01 * (20 + i) as f64),
            None,
        );
        terms.push(format!("path_{i:02}"));
    }
    project.flow("growth", &terms.join(" + "), None)
}

#[test]
fn activity_spread_across_many_loops_is_said_to_have_no_dominant_loop() {
    // Twelve equal loops through one stock: each holds a twelfth, and the
    // lead stays with one of them rather than passing among equals.
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
    let spans = partition["dominance"].as_array().unwrap();
    assert_eq!(
        spans.len(),
        1,
        "equal loops do not trade the lead: {spans:?}"
    );
    let leaders = &spans[0]["leaders"];
    assert_eq!(leaders.as_array().unwrap().len(), MAX_LEADERS);
    assert_eq!(leaders[0]["share"], 0.08);
    let note = output["note"].as_str().unwrap();
    assert!(
        note.contains("partition with level") && note.contains("no one loop dominates"),
        "{note}"
    );

    // Equal loops lead a span together, however small the share each
    // holds. A span is its steps from `from` to `to`, both included.
    let analysis = kept_analysis(&mut host, &mut session, "current");
    let scored = analysis.times.len() - 1;
    for l in &analysis.loops {
        let lead = leadership(&analysis, &l.key, 0.0, 10.0).unwrap();
        assert!(lead.leads(), "every loop of the tie leads");
        assert_eq!(lead.active, scored, "the last step is the span's");
        let before_the_end = leadership(&analysis, &l.key, 0.0, 9.5).unwrap();
        assert_eq!(
            before_the_end.active,
            scored - 1,
            "a span ends at `to`: 10 is not its step"
        );
    }

    // One loop holding the activity is not spread.
    let mut host = Host::from_test_project(&logistic());
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    assert!(output.get("note").is_none());
}

/// A span names its leader and, as rivals, the loops holding at least half
/// its share: of three loops holding a half, three tenths and a fifth, the
/// first two.
#[test]
fn a_span_names_its_leader_and_the_rivals_with_half_its_share() {
    let project = TestProject::new("three paths")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", "1", &["growth"], &[], None)
        .aux("half", "level * 0.5", None)
        .aux("three_tenths", "level * 0.3", None)
        .aux("fifth", "level * 0.2", None)
        .flow("growth", "half + three_tenths + fifth", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let id_through = |variable: &str| -> Value {
        listed(&output)
            .into_iter()
            .find(|l| l["chain"][1]["variable"] == variable)
            .unwrap_or_else(|| panic!("{output}"))["id"]
            .clone()
    };
    let spans = output["partitions"][0]["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 1);
    assert_eq!(
        spans[0]["leaders"],
        json!([
            {"loop": id_through("half"), "share": 0.5},
            {"loop": id_through("three_tenths"), "share": 0.3}
        ])
    );
    assert_eq!(listed(&output).len(), 3, "the fifth is listed, as another");

    // Through the leader's variable, the rival, which does not go through
    // it, is neither named nor listed.
    let through = analyze(&mut host, &mut session, json!({"through": "half"}));
    assert_eq!(through["found"], 1);
    assert_eq!(
        through["partitions"][0]["dominance"][0]["leaders"],
        json!([{"loop": id_through("half"), "share": 0.5}])
    );
    assert_eq!(listed(&through).len(), 1);
}

/// Loops that pass through no stock of the model (their state is inside a
/// builtin) are in no partition: each stands alone, so they are listed
/// without a timeline or shares, and each leads wherever it is active.
#[test]
fn loops_in_no_partition_stand_alone() {
    let project = TestProject::new("smoothed")
        .with_sim_time(0.0, 10.0, 0.25)
        .aux("first", "SMTH1(first_goal, 2, 1)", None)
        .aux("first_goal", "first * 0.5 + 4", None)
        .aux("second", "SMTH1(second_goal, 3, 2)", None)
        .aux("second_goal", "second * 0.25 + 9", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "run", "{output}");
    assert_eq!(output["found"], 2);
    let partitions = output["partitions"].as_array().unwrap();
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0]["stocks"], json!([]));
    assert!(partitions[0].get("dominance").is_none(), "{output}");
    for l in listed(&output) {
        assert!(l.get("share").is_none(), "{l}");
        assert_eq!(l["chain"][1]["via"], "smth1", "{l}");
    }
    let analysis = kept_analysis(&mut host, &mut session, "current");
    let scored = analysis.times.len() - 1;
    for l in &analysis.loops {
        let lead = leadership(&analysis, &l.key, 0.0, 10.0).unwrap();
        assert!(lead.leads());
        assert_eq!((lead.share, lead.active), (1.0, scored));
        assert_eq!(lead.strongest.as_ref(), Some(&l.key));
    }
}

/// The loops that lead after a time are those of the spans that end after it.
#[test]
fn the_leaders_after_a_time_are_those_of_the_spans_past_it() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let growth = only_loop(&output, "reinforcing")["id"].as_str().unwrap();
    let crowding = only_loop(&output, "balancing")["id"].as_str().unwrap();
    let analysis = kept_analysis(&mut host, &mut session, "current");
    let mut after = |time: f64, n: usize| leaders_after(&mut session.evidence, &analysis, time, n);
    assert_eq!(after(0.0, 4), [crowding, growth], "strongest first");
    assert_eq!(after(20.0, 4), [crowding], "growth's span ended by then");
    assert_eq!(after(0.0, 1), [crowding]);
    assert_eq!(after(40.0, 4), Vec::<String>::new());
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
    assert_eq!(
        from_structure["chain"][0]["via"], "smth1",
        "the link into the perception is the one through the smooth"
    );
    assert!(from_structure["chain"][1].get("via").is_none());

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
    assert_eq!(from_run["chain"], from_structure["chain"]);
    assert_eq!(from_run["id"], from_structure["id"]);

    // An argument the engine hoists into a helper of its own is part of its
    // call's link, and names nothing.
    let project = TestProject::new("scaled")
        .with_sim_time(0.0, 20.0, 0.25)
        .stock("level", "10", &["adjustment"], &[], None)
        .flow("adjustment", "(10 - perceived) / 4", None)
        .aux("perceived", "SMTH1(level * 1, 2)", None);
    let output = analyze(
        &mut Host::from_test_project(&project),
        &mut Session::new("main"),
        json!({}),
    );
    let l = only_loop(&output, "balancing");
    assert_eq!(chain(l), expected);
    assert_eq!(l["chain"][0]["via"], "smth1");
}

/// Two loops between the same variables, one direct and one through a
/// builtin, are two loops: each with its id, its polarity and its share.
#[test]
fn a_loop_through_a_builtin_is_another_loop_than_the_direct_one() {
    let (mut host, mut session, analysis) = analyzed("trend", trend().build_datamodel());
    assert_eq!(analysis.loops.len(), 2);
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["found"], 2);
    let direct = only_loop(&output, "reinforcing");
    let smoothed = only_loop(&output, "balancing");
    assert_ne!(direct["id"], smoothed["id"]);
    assert_eq!(chain(direct), steps(&[("level", "+"), ("change", "+")]));
    assert_eq!(chain(smoothed), steps(&[("level", "-"), ("change", "+")]));
    assert!(direct["chain"][0].get("via").is_none());
    assert_eq!(smoothed["chain"][0]["via"], "smth1");
    let share = |l: &Value| l["share"].as_f64().unwrap();
    assert_eq!(share(direct) + share(smoothed), 1.0);
    let spans = output["partitions"][0]["dominance"].as_array().unwrap();
    assert!(spans.len() > 2, "they trade the lead: {spans:?}");

    // Naming one of them names its variables, the builtin's instance aside.
    host.call(&mut session, "read_model", json!({}));
    let edit = host.call(
        &mut session,
        "edit_model",
        json!({
            "summary": "name the smoothed loop",
            "operations": [{"op": "name_loop", "loop": smoothed["id"], "name": "anchoring"}]
        }),
    );
    assert_eq!(
        edit["changes"][0]["detail"], "names the loop through change, level",
        "{edit}"
    );
}

/// An initial-value argument sets where a smooth starts and nothing after:
/// it is no link of the run, so the model's one loop holds all the activity.
#[test]
fn a_builtins_initial_value_argument_makes_no_loop() {
    let project = TestProject::new("perceived")
        .with_sim_time(0.0, 40.0, 0.25)
        .stock("level", "10", &["adjustment"], &[], None)
        .flow("adjustment", "(goal - perceived) / 4", None)
        .aux("perceived", "SMTH1(level, 2, level * 0.5)", None)
        .aux("goal", "20", None);
    let (mut host, mut session, analysis) = analyzed("perceived", project.build_datamodel());
    assert_eq!(analysis.loops.len(), 1);
    let output = analyze(&mut host, &mut session, json!({}));
    let l = only_loop(&output, "balancing");
    assert_eq!(l["share"], 1.0);
    assert_eq!(
        chain(l),
        steps(&[("level", "+"), ("perceived", "-"), ("adjustment", "+")])
    );
    assert!(output.get("inactive").is_none(), "{output}");
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

/// A loop is one cycle of the model's elements from structure as from a run:
/// an arrayed model at rest lists each element's loop, in its own partition,
/// under the id the run gives it.
#[test]
fn an_arrayed_loop_has_one_id_from_structure_and_from_a_run() {
    let mut host = Host::from_test_project(&regions());
    let mut session = Session::new("main");
    let at_rest = analyze(&mut host, &mut session, json!({}));
    assert_eq!(at_rest["basis"], "structure");
    assert_eq!(at_rest["found"], 2);
    let by_stock = |output: &Value| -> Vec<(Value, Value)> {
        output["partitions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["stocks"].clone(), p["loops"][0]["id"].clone()))
            .collect()
    };
    assert_eq!(
        by_stock(&at_rest),
        [
            (json!(["population[north]"]), json!("L1")),
            (json!(["population[south]"]), json!("L2"))
        ]
    );
    let north = analyze(
        &mut host,
        &mut session,
        json!({"through": "population[north]"}),
    );
    assert_eq!(north["found"], 1);
    assert_eq!(
        chain(only_loop(&north, "reinforcing")),
        steps(&[("population[north]", "+"), ("births[north]", "+")])
    );

    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "seeded", "set": [{"variable": "seed", "value": 1}]}),
    );
    let seeded = analyze(&mut host, &mut session, json!({"run": "seeded"}));
    assert_eq!(seeded["basis"], "run");
    assert_eq!(by_stock(&seeded), by_stock(&at_rest));
    assert!(seeded.get("inactive").is_none(), "{seeded}");
}

/// A finding of one citation, verified: its verdict.
fn cited(host: &mut Host, session: &mut Session, citation: Value) -> Value {
    let verdict = host.call(
        session,
        "verify_findings",
        json!({"findings": [{"kind": "observation", "claim": "c", "citations": [citation]}]}),
    );
    verdict["findings"][0].clone()
}

/// Logistic growth beside a ring of auxiliaries through a stock at rest: the
/// ring is a loop of the model, inactive in the run.
fn logistic_beside_a_ring_at_rest() -> TestProject {
    let mut project = logistic()
        .stock("level", "0", &[], &["drain"], None)
        .aux("a1", "level", None);
    for i in 2..=14 {
        project = project.aux(&format!("a{i}"), &format!("a{}", i - 1), None);
    }
    project.flow("drain", "a14 / 5", None)
}

/// Where the structure lists the model's loops, an absence is shown from the
/// active loops and the inactive ones together: a variable on a loop the run
/// left inactive has a loop through it, named as inactive; one on no loop has
/// none.
#[test]
fn no_loop_through_counts_the_loops_a_run_left_inactive() {
    let mut host = Host::from_test_project(&logistic_beside_a_ring_at_rest());
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let ring = output["inactive"]["loops"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let on_the_ring = cited(
        &mut host,
        &mut session,
        json!({"cites": "no_loop_through", "variable": "a7"}),
    );
    assert_eq!(on_the_ring["holds"], false, "{on_the_ring}");
    assert!(
        on_the_ring
            .to_string()
            .contains(&format!("{ring} (inactive in run 'current')")),
        "{on_the_ring}"
    );
    let off_every_loop = cited(
        &mut host,
        &mut session,
        json!({"cites": "no_loop_through", "variable": "max_rate"}),
    );
    assert_eq!(off_every_loop["holds"], true, "{off_every_loop}");
}

/// Loops a run left out as negligible are among the model's loops from
/// structure, so the analysis still shows an absence, though it is not
/// complete: a variable on a negligible loop has it, one on no loop has none.
#[test]
fn no_loop_through_is_shown_beside_loops_left_out_as_negligible() {
    let mut project = TestProject::new("faint")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", "1", &["growth"], &[], None)
        .aux("rate", "0.1", None);
    let mut terms = vec!["level * rate".to_string()];
    for i in 1..=3 {
        project = project.aux(&format!("faint_{i}"), "level * 1e-7", None);
        terms.push(format!("faint_{i}"));
    }
    let project = project.flow("growth", &terms.join(" + "), None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["complete"], false, "{output}");
    let faint = cited(
        &mut host,
        &mut session,
        json!({"cites": "no_loop_through", "variable": "faint_2"}),
    );
    assert_eq!(faint["holds"], false, "{faint}");
    assert!(
        faint
            .to_string()
            .contains("not among the loops this analysis reports for run 'current'"),
        "{faint}"
    );
    let rate = cited(
        &mut host,
        &mut session,
        json!({"cites": "no_loop_through", "variable": "rate"}),
    );
    assert_eq!(rate["holds"], true, "{rate}");
}

/// A loop the run left inactive is a loop of the model: asked for by id it
/// comes whole, its chain however long, under `inactive`, and the verifier
/// says it is inactive rather than no loop at all.
#[test]
fn an_inactive_loop_comes_whole_by_id() {
    let mut host = Host::from_test_project(&logistic_beside_a_ring_at_rest());
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let ring = &output["inactive"]["loops"][0];
    assert!(ring.get("chain").is_none(), "too long to chain: {ring}");
    let id = ring["id"].as_str().unwrap().to_string();
    let by_id = analyze(&mut host, &mut session, json!({"loops": [id]}));
    assert!(by_id.get("absent").is_none(), "{by_id}");
    let whole = &by_id["inactive"]["loops"][0];
    assert_eq!(whole["id"], json!(id), "{by_id}");
    assert_eq!(whole["chain"].as_array().unwrap().len(), 16, "{whole}");
    let verdict = cited(&mut host, &mut session, json!({"cites": "loop", "id": id}));
    assert_eq!(verdict["holds"], false, "{verdict}");
    assert!(
        verdict.to_string().contains("inactive in run 'current'"),
        "{verdict}"
    );
}

/// Two loops tied for the lead are named in one order wherever a lead is
/// read: the first of them by key, however an answer lists them. Here the
/// shorter loop, which an answer lists first, is the later by key (`z` after
/// `b1`), and its twin through `b1` and `b2` carries exactly the same score.
#[test]
fn a_tie_for_the_lead_goes_to_the_first_loop_by_key() {
    let project = TestProject::new("tied")
        .with_sim_time(0.0, 10.0, 0.5)
        .stock("level", "1", &["growth"], &[], None)
        .aux("z", "level * 0.1", None)
        .aux("b1", "level * 0.1", None)
        .aux("b2", "b1", None)
        .flow("growth", "z + b2", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    let through = |variable: &str| -> Value {
        listed(&output)
            .into_iter()
            .find(|l| l["chain"].to_string().contains(&format!("\"{variable}\"")))
            .unwrap_or_else(|| panic!("{output}"))["id"]
            .clone()
    };
    assert_eq!(
        listed(&output)[0]["id"],
        through("z"),
        "shortest listed first"
    );
    let spans = output["partitions"][0]["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 1, "{output}");
    assert_eq!(spans[0]["leaders"][0]["loop"], through("b1"), "{output}");
    let analysis = kept_analysis(&mut host, &mut session, "current");
    assert_eq!(
        leaders_after(&mut session.evidence, &analysis, 0.0, 1),
        [through("b1").as_str().unwrap()]
    );
}

/// A lead that changes at a run's last step makes a span of that one step,
/// printed from its time to itself; cited back, it holds, as every span the
/// answer prints does ([`unconfirmed_leaders`], which `analyzed` runs). The
/// second loop's gain grows with the clock (read at the step before, as a
/// partial reads it) and passes the first's only at the end.
#[test]
fn a_lead_that_changes_at_the_last_step_is_a_span_of_one_step() {
    let project = TestProject::new("late")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "1", &["growth"], &[], None)
        .aux("early", "level * 0.2", None)
        .aux("late", "level * 0.0026 * TIME * TIME", None)
        .flow("growth", "early + late", None);
    let (mut host, mut session, _) = analyzed("late", project.build_datamodel());
    let output = analyze(&mut host, &mut session, json!({}));
    let spans = output["partitions"][0]["dominance"].as_array().unwrap();
    assert_eq!(spans.len(), 2, "{output}");
    assert_eq!(
        (&spans[0]["from"], &spans[0]["to"]),
        (&json!(0.0), &json!(9.0))
    );
    assert_eq!(
        (&spans[1]["from"], &spans[1]["to"]),
        (&json!(10.0), &json!(10.0))
    );
}

/// A cycle of elements the structure reports twice -- the loop through a
/// reducer, its aggregate node left out, beside the direct loop through the
/// same elements -- is one loop with one id, whose sign is undetermined
/// where the two disagree: `pop * 0.02` closes each element's loop
/// reinforcing, `- SUM(pop[*]) * 0.001` the same cycle balancing.
#[test]
fn a_cycle_the_structure_reports_twice_is_one_loop() {
    let project = TestProject::new("twins")
        .with_sim_time(0.0, 20.0, 0.25)
        .named_dimension("region", &["a", "b"])
        .array_stock("pop[region]", "0", &["growth"], &[], None)
        .array_flow("growth[region]", "pop * 0.02 - SUM(pop[*]) * 0.001", None);
    let mut host = Host::from_test_project(&project);
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    assert_eq!(output["basis"], "structure", "{output}");
    let ids: Vec<&Value> = listed(&output).iter().map(|l| &l["id"]).collect();
    let mut distinct = ids.clone();
    distinct.sort_by_key(|id| id.to_string());
    distinct.dedup();
    assert_eq!(ids.len(), distinct.len(), "each loop listed once: {output}");
    assert_eq!(output["found"], ids.len(), "{output}");
    let own: Vec<&Value> = listed(&output)
        .into_iter()
        .filter(|l| l["length"] == 2)
        .collect();
    assert_eq!(own.len(), 2, "each element's own loop: {output}");
    assert!(
        own.iter().all(|l| l["polarity"] == "undetermined"),
        "{output}"
    );
}

/// The corpus's small arrayed and module-bearing models keep the rules
/// [`breaches`] states: same-element loops, loops across elements, loops
/// through a reducer, a loop through a module, and loops that tie for the
/// lead before one pulls ahead.
#[test]
fn the_corpus_loop_fixtures_agree_with_their_scores_and_structure() {
    for path in [
        "../../test/arrayed_population_ltm/arrayed_population.stmx",
        "../../test/cross_element_ltm/cross_element.stmx",
        "../../test/cross_agg_ltm/cross_agg.stmx",
        "../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
        "../../test/decoupled_stocks/decoupled.stmx",
    ] {
        analyzed(path, corpus_model(path));
    }
}

/// The model's loops that a run did not have active are listed, from its
/// structure, so the answer is of the model's loops and not only of the
/// moving part of it.
#[test]
fn the_loops_a_run_left_inactive_are_listed_from_structure() {
    // Logistic growth beside an oscillator that starts at its rest point.
    let project = logistic()
        .stock("x", "100", &["dx"], &[], None)
        .stock("v", "0", &["dv"], &[], None)
        .flow("dx", "v", None)
        .flow("dv", "0.25 * (100 - x) - 0.1 * v", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "run");
    assert_eq!(output["found"], 2);
    assert_eq!(output["complete"], true);
    assert_eq!(output["partitions"].as_array().unwrap().len(), 1);
    let inactive = output["inactive"]["loops"].as_array().unwrap();
    assert!(output["inactive"].get("otherLoops").is_none());
    let chains: Vec<Vec<(String, String)>> = inactive.iter().map(chain).collect();
    assert_eq!(
        chains,
        [
            steps(&[("v", "-"), ("dv", "+")]),
            steps(&[("v", "+"), ("dx", "+"), ("x", "-"), ("dv", "+")]),
        ],
        "shortest first, with their equations' signs"
    );
    for l in inactive {
        assert_eq!(l["polarity"], "balancing");
        assert!(l.get("share").is_none(), "{l}");
    }
    let ids: HashSet<&str> = listed(&output)
        .into_iter()
        .chain(inactive)
        .map(|l| l["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 4, "each loop has its id");

    // Through a variable of the inactive part: none active, those listed.
    let output = analyze(&mut host, &mut session, json!({"through": "x"}));
    assert_eq!(output["found"], 0);
    assert_eq!(output["inactive"]["loops"].as_array().unwrap().len(), 1);
    assert_eq!(output["note"], "No loop active in this run goes through x.");
}

/// An absence is shown only from every loop of the model. Where the
/// structure lists the model's loops, the loops a capped analysis left out
/// are among those listed as inactive, so an absence is shown from both; where
/// it does not, an analysis that does not have every loop of the run shows
/// none, since a variable only the others pass through is on a loop all the
/// same.
#[test]
fn an_absence_is_shown_only_from_every_loop_of_the_model() {
    let mut host = Host::from_test_project(&twelve_paths());
    let mut session = Session::new("main");
    let whole = analyze(&mut host, &mut session, json!({}));
    assert_eq!(
        (&whole["found"], &whole["complete"]),
        (&json!(12), &json!(true))
    );
    let analysis = kept_analysis(&mut host, &mut session, "current");
    let path_00 = analysis
        .loops
        .iter()
        .find(|l| l.goes_through("path_00"))
        .unwrap()
        .key
        .clone();
    assert_eq!(
        loops_through(&analysis, "path_00"),
        Some(vec![(path_00.clone(), false)])
    );
    assert_eq!(loops_through(&analysis, "level").map(|l| l.len()), Some(12));

    // Discovery keeps the loops that lead at some step whatever its cap, so
    // of twelve loops of which the strongest always leads, a cap of four
    // keeps the four strongest; the weakest path's is listed as inactive.
    let _cap = crate::ltm_finding::MaxLoopsGuard::new(4);
    let mut host = Host::from_test_project(&twelve_paths());
    let mut session = Session::new("main");
    let capped = analyze(&mut host, &mut session, json!({}));
    assert_eq!(capped["found"], 4);
    assert_eq!(capped["complete"], false);
    assert!(
        capped["note"]
            .as_str()
            .unwrap()
            .contains("keeps the 4 most important of the run's 12 loops"),
        "{capped}"
    );
    let analysis = kept_analysis(&mut host, &mut session, "current");
    assert_eq!(
        loops_through(&analysis, "path_00"),
        Some(vec![(path_00, true)])
    );
    let through = analyze(&mut host, &mut session, json!({"through": "path_00"}));
    assert_eq!(through["found"], 0);
    assert!(
        through["note"].as_str().unwrap().ends_with(
            "No loop this analysis reports for the run goes through path_00; the model's \
                 other loops are listed as inactive."
        ),
        "{through}"
    );
    assert_eq!(through["inactive"]["loops"].as_array().unwrap().len(), 1);
    drop(_cap);

    // A ring the structure cannot enumerate, and a trickle out of its stock
    // that never holds a thousandth of the activity: the analysis leaves the
    // trickle's loop out, and nothing lists it.
    let project = large_ring("100")
        .aux("trickle", "level * 0.0000001", None)
        .flow("seep", "trickle", None);
    let mut project = project.build_datamodel();
    if let Some(datamodel::Variable::Stock(level)) = project.models[0]
        .variables
        .find_mut(|v| v.get_ident() == "level")
    {
        level.outflows.push("seep".to_string());
    }
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({"through": "trickle"}));
    assert_eq!(output["found"], 0, "{output}");
    assert_eq!(output["complete"], false, "{output}");
    assert!(output.get("inactive").is_none(), "{output}");
    let note = output["note"].as_str().unwrap();
    assert!(
        note.contains("leaves out 1 loop that never held a thousandth of its partition")
            && note.contains("one it left out may"),
        "{note}"
    );
    let analysis = kept_analysis(&mut host, &mut session, "current");
    assert_eq!(loops_through(&analysis, "trickle"), None);
}

/// Discovery that runs out of room to enumerate searches the run for the
/// strongest loops instead: a sample, which is not every loop of the run, and
/// the answer says so. (A small memory budget makes the twelve paths' search a
/// sample.)
#[test]
fn a_sampled_analysis_is_not_complete_and_says_so() {
    let _budget = crate::ltm_finding::MemoryBudgetGuard::new(4096);
    let mut host = Host::from_test_project(&twelve_paths());
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "run", "{output}");
    assert_eq!(output["complete"], false, "{output}");
    assert!(
        output["note"]
            .as_str()
            .unwrap()
            .contains("a sample, not every loop"),
        "{output}"
    );
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

/// A loop as a report lists one a run does not have: without its share.
fn crowding_elsewhere(report: &Value) -> Value {
    let mut report = report.clone();
    report.as_object_mut().unwrap().remove("share");
    report
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

    // A run the crowding loop is cut from has it as the cut's, whole.
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
    assert!(output.get("absent").is_none(), "{output}");
    assert_eq!(output["cut"]["loops"][0], crowding_elsewhere(&crowding));

    // A model edited to be without it has no such loop at all.
    host.call(&mut session, "read_model", json!({}));
    host.call(
        &mut session,
        "edit_model",
        json!({
            "summary": "no crowding",
            "operations": [{"op": "set_equation", "variable": "fractional_birth_rate",
                            "equation": "max_rate"}]
        }),
    );
    let output = analyze(&mut host, &mut session, json!({"loops": [crowding["id"]]}));
    assert_eq!(output["absent"], json!([crowding["id"]]), "{output}");
    let note = output["note"].as_str().unwrap();
    assert!(note.contains("no loop of this run's analysis"), "{note}");

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

/// A loop's polarity is one type from a run and from structure, so the name
/// an answer gives it is one mapping: each of the engine's polarities to its
/// own name, none to another's.
#[test]
fn a_loops_polarity_reads_the_same_from_a_run_and_from_structure() {
    // The structural surface's polarity is the run's type: this line is the
    // assertion, and it stops compiling if the two part.
    let structural: crate::db::DetectedLoopPolarity = LoopPolarity::Undetermined;
    assert_eq!(
        LoopPolarityName::from(structural),
        LoopPolarityName::Undetermined
    );

    // The match is exhaustive, so a polarity the engine adds fails to
    // compile here until it has a row.
    let expected = |polarity: LoopPolarity| match polarity {
        LoopPolarity::Reinforcing => "reinforcing",
        LoopPolarity::Balancing => "balancing",
        LoopPolarity::MostlyReinforcing => "mostly_reinforcing",
        LoopPolarity::MostlyBalancing => "mostly_balancing",
        LoopPolarity::Undetermined => "undetermined",
    };
    let polarities = [
        LoopPolarity::Reinforcing,
        LoopPolarity::Balancing,
        LoopPolarity::MostlyReinforcing,
        LoopPolarity::MostlyBalancing,
        LoopPolarity::Undetermined,
    ];
    let names: Vec<LoopPolarityName> = polarities.map(LoopPolarityName::from).to_vec();
    assert_eq!(names, LoopPolarityName::ALL);
    for (polarity, name) in polarities.into_iter().zip(names) {
        assert_eq!(json!(name), json!(expected(polarity)));
    }
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
        let mut host = Host::new(corpus_model(path));
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
        eprintln!(
            "{path}: {} bytes, basis {}, complete {}, found {}, {} partitions, {listed} loops \
             listed, omitted {}, note {}; first call {first:?}, kept {kept:?}",
            output.json.len(),
            analysis["basis"],
            analysis["complete"],
            analysis["found"],
            partitions.len(),
            analysis["omitted"],
            analysis["note"],
        );
        assert!(
            output.json.len() <= crate::tools::OUTLINE_BUDGET,
            "{path}: {} bytes",
            output.json.len()
        );
        assert_eq!(
            analysis["complete"], false,
            "{path}: the run has more loops than the analysis keeps"
        );
        let (unconfirmed, named) = unconfirmed_leaders(path, &mut host, &mut session, &analysis);
        eprintln!(
            "{path}: {} of {named} span leaders the verifier does not confirm",
            unconfirmed.len()
        );
        assert!(unconfirmed.is_empty(), "{unconfirmed:#?}");
    }

    // World3's analysis keeps 200 of the run's loops, and `deaths_0_to_14`
    // is on some of the others (it drains a stock it reads): no absence of a
    // loop through it can be shown, to the agent or to the verifier.
    let mut host = Host::new(corpus_model("../../test/metasd/WRLD3-03/wrld3-03.mdl"));
    let mut session = Session::new("main");
    let through = host.call(
        &mut session,
        "analyze_loops",
        json!({"through": "deaths 0 to 14"}),
    );
    if through["found"] == 0 {
        assert!(
            through["note"]
                .as_str()
                .unwrap()
                .contains("one it left out may"),
            "{through}"
        );
    }
    let verdict = host.call(
        &mut session,
        "verify_findings",
        json!({"findings": [{
            "kind": "observation",
            "claim": "No feedback loop goes through deaths 0 to 14.",
            "citations": [{"cites": "no_loop_through", "variable": "deaths 0 to 14"}]
        }]}),
    );
    assert_eq!(verdict["findings"][0]["holds"], false, "{verdict}");
}

/// Every corpus model's loop analysis keeps the rules [`breaches`] states.
#[test]
#[ignore = "analyzes every corpus model's loops twice; run under the gates profile"]
fn every_corpus_models_loop_analysis_agrees_with_its_scores_and_structure() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    let mut files = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for path in std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()) {
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase);
            if path.is_dir() {
                dirs.push(path);
            } else if matches!(
                extension.as_deref(),
                Some("xmile" | "stmx" | "mdl" | "itmx")
            ) {
                files.push(path);
            }
        }
    }
    files.sort();
    let (mut analyzed, mut from_runs) = (0, 0);
    let mut found = Vec::new();
    for path in &files {
        let label = path.strip_prefix(&root).unwrap().display().to_string();
        let bytes = std::fs::read(path).unwrap();
        let is_mdl = label.to_lowercase().ends_with(".mdl");
        // The sweep is of the analysis, not of the importers.
        let project = std::panic::catch_unwind(|| {
            if is_mdl {
                crate::compat::open_vensim(&String::from_utf8_lossy(&bytes)).ok()
            } else {
                crate::compat::open_xmile(&mut std::io::BufReader::new(&bytes[..])).ok()
            }
        })
        .ok()
        .flatten();
        let Some(project) = project else { continue };
        let Some(model_name) = project
            .models
            .iter()
            .find(|m| m.name == "main")
            .or(project.models.first())
            .map(|m| m.name.clone())
        else {
            continue;
        };
        let mut host = Host::new(project);
        let mut session = Session::new(&model_name);
        let output = host.call_raw(&mut session, "analyze_loops", "{}");
        if output.is_error {
            continue;
        }
        analyzed += 1;
        let analysis = kept_analysis(&mut host, &mut session, "current");
        if analysis.basis == LoopBasis::Run {
            from_runs += 1;
        }
        found.extend(breaches(&label, &mut host, &model_name, &analysis));
        let answer: Value = serde_json::from_str(&output.json).unwrap();
        found.extend(unconfirmed_leaders(&label, &mut host, &mut session, &answer).0);
    }
    eprintln!(
        "{} files, {analyzed} analyzed, {from_runs} from their runs",
        files.len()
    );
    assert!(analyzed > 300 && from_runs > 50, "the corpus is swept");
    assert!(found.is_empty(), "{found:#?}");
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

/// A link through an aggregate (a SUM over an array's elements) is signed by
/// composing the links into and out of it, as a builtin's internals are.
#[test]
fn a_link_through_an_aggregate_is_signed() {
    let project = corpus_model("../../test/cross_agg_ltm/cross_agg.stmx");
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
        for l in listed(&output) {
            assert_eq!(l["polarity"], "reinforcing", "{l}");
            for step in l["chain"].as_array().into_iter().flatten() {
                links += 1;
                assert_eq!(step["polarity"], "+", "{l}");
            }
        }
        assert!(links > 0, "{output}");
    }
}

/// A link discovery stitched across aggregates has the sign of the paths
/// through them only where they agree: one element reaching another through
/// a SUM that raises its growth and through a MAX that lowers it has none.
#[test]
fn a_link_through_aggregates_that_disagree_is_unsigned() {
    let project = TestProject::new("two reducers")
        .with_sim_time(0.0, 10.0, 0.5)
        .named_dimension("region", &["a", "b"])
        .array_stock("pop[region]", "10", &["growth"], &[], None)
        .array_flow(
            "growth[region]",
            "SUM(pop[*]) * 0.05 - MAX(pop[*]) * 0.02",
            None,
        );
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let output = analyze(&mut host, &mut session, json!({}));
    assert_eq!(output["basis"], "run", "{output}");
    let across: Vec<&Value> = listed(&output)
        .into_iter()
        .filter(|l| l["length"] == 4)
        .collect();
    assert!(!across.is_empty(), "{output}");
    for l in across {
        let signs: Vec<String> = chain(l).into_iter().map(|(_, sign)| sign).collect();
        assert_eq!(signs, ["?", "+", "?", "+"], "{l}");
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

/// The ids of the loops an answer lists, in the order it lists them: its
/// partitions', then its cut's, then its inactive loops'.
fn ids_in_order(output: &Value) -> Vec<String> {
    let of = |loops: &Value| -> Vec<String> {
        loops
            .as_array()
            .into_iter()
            .flatten()
            .map(|l| l["id"].as_str().unwrap().to_string())
            .collect()
    };
    let mut ids: Vec<String> = output["partitions"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| of(&p["loops"]))
        .collect();
    ids.extend(of(&output["cut"]["loops"]));
    ids.extend(of(&output["inactive"]["loops"]));
    ids
}

/// An answer over its budget lists a prefix of what it would list: its
/// partitions with their timelines and the loops those name, largest first;
/// then the loops a cut names; then the other loops, the most important
/// first; then the inactive loops. So a smaller budget's answer lists a
/// subset of a larger one's, the first partition and its leaders always, and
/// only the loops an answer lists are given ids.
#[test]
fn an_answer_over_its_budget_lists_a_prefix_of_what_it_would_list() {
    // Two partitions: twelve paths of growth, and logistic growth cut by a
    // replaced equation, beside an oscillator at rest (inactive loops).
    let project = twelve_paths()
        .stock("population", "1", &["births"], &[], None)
        .flow("births", "population * fractional_birth_rate", None)
        .aux(
            "fractional_birth_rate",
            "max_rate * (1 - population / capacity)",
            None,
        )
        .aux("max_rate", "0.5", None)
        .aux("capacity", "100", None)
        .stock("x", "100", &["dx"], &[], None)
        .stock("v", "0", &["dv"], &[], None)
        .flow("dx", "v", None)
        .flow("dv", "0.25 * (100 - x) - 0.1 * v", None);
    let experiment = json!({
        "name": "no crowding",
        "set": [{"variable": "fractional_birth_rate", "equation": "max_rate"}]
    });
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    host.call(&mut session, "run_experiment", experiment);
    let whole = analyze(&mut host, &mut session, json!({"run": "no crowding"}));
    let whole_len = whole.to_string().len();
    // The run's one analysis, answered within each budget as a session that
    // has named no loop answers it.
    let analysis = kept_analysis(&mut host, &mut session, "no crowding");
    let model = host.project.models[0].clone();
    let answer_within = |budget: usize| -> (Value, Evidence) {
        let mut evidence = Evidence::default();
        let output = report(
            &mut evidence,
            &analysis,
            &model,
            "no crowding",
            0,
            None,
            budget,
        );
        let output = serde_json::to_value(output).unwrap();
        for partition in output["partitions"].as_array().unwrap() {
            check_timeline(partition);
        }
        (output, evidence)
    };
    let parts = |output: &Value| -> (usize, usize, usize, usize) {
        (
            output["partitions"].as_array().unwrap().len(),
            listed(output).len(),
            output["cut"]["loops"].as_array().unwrap().len(),
            output["inactive"]["loops"].as_array().unwrap().len(),
        )
    };
    // The paths' leader and its two rivals, eight of the other nine paths,
    // and growth; the crowding loop the replaced equation cut; the
    // oscillator's two.
    assert_eq!(parts(&whole), (2, 12, 1, 2), "{whole}");
    assert_eq!(whole["omitted"], json!({"partitions": 0, "loops": 1}));
    assert_eq!(
        ids_in_order(&whole),
        (1..=15).map(|n| format!("L{n}")).collect::<Vec<_>>()
    );

    let mut previous = whole.clone();
    let mut seen = vec![parts(&whole)];
    for budget in (200..whole_len).rev().step_by(40) {
        let (fitted, evidence) = answer_within(budget);
        let size = fitted.to_string().len();
        let shape = parts(&fitted);
        // The least an answer lists is its first partition with its leaders.
        let least = shape == (1, 1, 0, 0);
        assert!(size <= budget || least, "{size} over {budget}: {fitted}");
        assert!(shape.0 >= 1 && shape.1 >= 1, "{fitted}");

        // What it lists, a larger budget's answer listed.
        let chains = |output: &Value| -> HashSet<String> {
            listed(output)
                .into_iter()
                .chain(output["cut"]["loops"].as_array().into_iter().flatten())
                .chain(output["inactive"]["loops"].as_array().into_iter().flatten())
                .map(|l| l["chain"].to_string())
                .collect()
        };
        assert!(chains(&fitted).is_subset(&chains(&previous)), "{fitted}");

        // In the order of the units: no inactive loop without every other
        // loop, no other loop without the cut's, no cut loop without every
        // partition.
        if shape.3 > 0 {
            assert_eq!(shape.1, 12, "{fitted}");
        }
        if shape.1 > 4 {
            assert_eq!(shape.2, 1, "{fitted}");
        }
        if shape.2 > 0 {
            assert_eq!(shape.0, 2, "{fitted}");
        }

        // Only what is listed has an id: the session's ids are those of the
        // answer, in its order, and none beyond.
        let ids = ids_in_order(&fitted);
        assert_eq!(
            ids,
            (1..=ids.len()).map(|n| format!("L{n}")).collect::<Vec<_>>(),
            "{fitted}"
        );
        assert!(
            evidence.loop_key(&format!("L{}", ids.len() + 1)).is_none(),
            "a loop left out has no id"
        );

        if seen.last() != Some(&shape) {
            seen.push(shape);
        }
        previous = fitted;
    }
    assert_eq!(seen.last(), Some(&(1, 1, 0, 0)), "{seen:?}");
    for shape in [(2, 12, 1, 0), (2, 4, 1, 0), (2, 4, 0, 0), (1, 3, 0, 0)] {
        assert!(
            seen.contains(&shape),
            "each kind of unit is left out in its turn, the rivals last: {seen:?}"
        );
    }
}

/// Loops asked for by id past the budget are left for another call.
#[test]
fn loops_by_id_over_the_budget_are_left_for_another_call() {
    let mut host = Host::from_test_project(&twelve_paths());
    let mut session = Session::new("main");
    session.outline_budget = usize::MAX;
    let whole = analyze(&mut host, &mut session, json!({}));
    let ids: Vec<Value> = listed(&whole)
        .into_iter()
        .take(4)
        .map(|l| l["id"].clone())
        .collect();
    // Room for two of the four asked for, less what naming the others takes.
    let two = analyze(&mut host, &mut session, json!({"loops": ids[..2]}));
    session.outline_budget = two.to_string().len();
    let fitted = analyze(&mut host, &mut session, json!({"loops": ids}));
    assert!(
        fitted.to_string().len() <= session.outline_budget,
        "{fitted}"
    );
    let returned = listed(&fitted).len();
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
    assert_eq!(
        chain(through_desired),
        steps(&[("level", "?"), ("desired", "+"), ("change", "+")])
    );
}

/// A run whose stocks move while none of its loops is active is answered
/// from structure, as a model at rest is.
#[test]
fn a_run_with_no_active_loop_is_answered_from_structure() {
    let still_loop_moving_stock = TestProject::new("apart")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "100", &["adjustment"], &[], None)
        .flow("adjustment", "(100 - level) / 4", None)
        .stock("tally", "0", &["arrivals"], &[], None)
        .flow("arrivals", "1", None);
    let mut host = Host::from_test_project(&still_loop_moving_stock);
    let output = analyze(&mut host, &mut Session::new("main"), json!({}));
    assert_eq!(output["basis"], "structure", "{output}");
    assert_eq!(output["found"], 1);
    let note = output["note"].as_str().unwrap();
    assert!(note.contains("every loop's score is zero"), "{note}");
}

/// An analysis stops before each of its stages once other work waits for the
/// project -- the run it reads, its replay under the overlay, the discovery
/// over the replay, the loops from structure -- and between the slices of
/// each run, and keeps nothing: the next analysis numbers its loops as if
/// none had begun.
#[test]
fn an_analysis_stops_before_each_stage_when_other_work_waits_and_keeps_nothing() {
    for (project, stages) in [(logistic(), 4), (at_rest(), 4)] {
        let mut stopped = 0;
        // One host throughout: a stopped analysis leaves the project and its
        // database as they were, so the next call finds them so.
        let mut host = Host::from_test_project(&project);
        for after in 1.. {
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

/// A fit names nothing: the ids it previews are the ids the session then
/// gives the same loops asked in the same order, a loop it has named keeping
/// its id and the others numbering on from the last id given out.
#[test]
fn a_fit_previews_the_ids_the_session_then_gives() {
    let key = |name: &str| vec![name.to_string()];
    let mut evidence = Evidence::default();
    assert_eq!(evidence.loop_id(&key("a")), "L1");
    assert_eq!(evidence.loop_id(&key("b")), "L2");
    let asked = ["c", "a", "d", "c", "b"];
    let previewed: Vec<String> = {
        let mut preview = evidence.preview_loop_ids();
        asked
            .iter()
            .map(|name| preview.loop_id(&key(name)))
            .collect()
    };
    assert_eq!(previewed, ["L3", "L1", "L4", "L3", "L2"]);
    assert!(evidence.loop_key("L3").is_none(), "a preview names nothing");
    let given: Vec<String> = asked
        .iter()
        .map(|name| evidence.loop_id(&key(name)))
        .collect();
    assert_eq!(given, previewed);
}
