// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::Host;

/// Verify `findings`, which must answer, checking the answer against the
/// output schema the catalog publishes.
fn verify(host: &mut Host, session: &mut Session, findings: Value) -> Value {
    let output = host.call(session, "verify_findings", json!({ "findings": findings }));
    let catalog: Value = serde_json::from_str(crate::tools::catalog_json()).unwrap();
    let schema = catalog["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "verify_findings")
        .unwrap()["outputSchema"]
        .clone();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let errors: Vec<String> = validator
        .iter_errors(&output)
        .map(|e| format!("{e} at {}", e.instance_path))
        .collect();
    assert!(errors.is_empty(), "{errors:?}\n{output}");
    output
}

/// Whether one citation holds, and what it says when it does not.
fn cite(host: &mut Host, session: &mut Session, citation: Value) -> Result<(), String> {
    let output = verify(
        host,
        session,
        json!([{"kind": "observation", "claim": "a claim", "citations": [citation]}]),
    );
    let verdict = &output["findings"][0];
    if verdict["holds"] == true {
        Ok(())
    } else {
        Err(verdict["failures"][0]["reason"]
            .as_str()
            .unwrap()
            .to_string())
    }
}

/// Logistic growth: a reinforcing loop through births, a balancing one
/// through crowding, the reinforcing leading until the inflection (t ~ 9).
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
        .aux("unused", "3", None)
}

/// The ids of logistic growth's reinforcing and balancing loops.
fn loop_ids(host: &mut Host, session: &mut Session) -> (String, String) {
    let loops = host.call(session, "analyze_loops", json!({}));
    let id = |polarity: &str| {
        loops["partitions"][0]["loops"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["polarity"] == polarity)
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    (id("reinforcing"), id("balancing"))
}

#[test]
fn a_finding_whose_citations_hold_gets_an_id_and_one_that_does_not_says_what_is_true() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let (growth, crowding) = loop_ids(&mut host, &mut session);
    let output = verify(
        &mut host,
        &mut session,
        json!([
            {"kind": "observation",
             "claim": "The population grows in an S: births compound it until crowding takes over.",
             "citations": [
                {"cites": "behavior_mode", "variable": "population", "mode": "s_shaped"},
                {"cites": "leads", "loop": growth, "from": 0, "to": 8},
                {"cites": "leads", "loop": crowding, "from": 12, "to": 40},
                {"cites": "ends_near", "variable": "population", "value": 100}
             ]},
            {"kind": "flaw",
             "claim": "The population overshoots its capacity.",
             "citations": [
                {"cites": "behavior_mode", "variable": "population", "mode": "overshoot"},
                {"cites": "ends_near", "variable": "population", "value": 150}
             ]}
        ]),
    );
    assert_eq!(output["findings"][0], json!({"id": "F1", "holds": true}));
    let refuted = &output["findings"][1];
    assert_eq!(refuted["holds"], false);
    assert!(refuted.get("id").is_none());
    assert_eq!(
        refuted["failures"],
        json!([
            {"citation": 1, "reason": "population's behavior in run 'current' is s shaped"},
            {"citation": 2, "reason": "population ends at 100 in run 'current'"}
        ])
    );
}

#[test]
fn every_kind_of_citation_holds_when_true_and_says_what_is_true_when_not() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let (growth, crowding) = loop_ids(&mut host, &mut session);
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "crowded", "set": [{"variable": "capacity", "value": 50}]}),
    );
    let rows: Vec<(Value, Option<&str>)> = vec![
        (json!({"cites": "variable", "variable": "capacity"}), None),
        (
            json!({"cites": "variable", "variable": "capacty"}),
            Some("no variable 'capacty'; the closest are capacity"),
        ),
        (
            json!({"cites": "reads", "variable": "births", "reads": "population", "polarity": "+"}),
            None,
        ),
        (
            json!({"cites": "reads", "variable": "fractional_birth_rate", "reads": "population",
                   "polarity": "+"}),
            Some("is negative by its equation"),
        ),
        (
            json!({"cites": "reads", "variable": "births", "reads": "capacity"}),
            Some("births does not read capacity; it reads"),
        ),
        (
            json!({"cites": "equation", "variable": "fractional_birth_rate",
                   "equation": "Max_Rate*(1-Population/capacity)"}),
            None,
        ),
        (
            json!({"cites": "equation", "variable": "capacity", "equation": "120"}),
            Some("capacity's equation is `100`"),
        ),
        (
            json!({"cites": "equation", "variable": "capacity", "equation": "1 +"}),
            Some("not an equation the engine can read"),
        ),
        (
            json!({"cites": "value", "variable": "capacity", "value": 100}),
            None,
        ),
        (
            json!({"cites": "value", "variable": "population", "value": 1}),
            None,
        ),
        (
            json!({"cites": "value", "variable": "population", "value": 50, "time": 40}),
            Some("population is 100 at 40 in run 'current'"),
        ),
        (
            json!({"cites": "value", "variable": "capacity", "value": 50, "run": "crowded"}),
            None,
        ),
        (
            json!({"cites": "readers", "variable": "unused", "readers": []}),
            None,
        ),
        (
            json!({"cites": "readers", "variable": "capacity", "readers": ["fractional_birth_rate"]}),
            None,
        ),
        (
            json!({"cites": "readers", "variable": "capacity", "readers": []}),
            Some("capacity is read by fractional_birth_rate"),
        ),
        (
            json!({"cites": "readers", "variable": "unused", "readers": ["births"]}),
            Some("nothing reads unused"),
        ),
        (
            json!({"cites": "readers", "variable": "births", "readers": ["population"]}),
            None,
        ),
        (json!({"cites": "no_diagnostics"}), None),
        (
            json!({"cites": "diagnostic", "id": "D4"}),
            Some("the model has no diagnostic D4 now; its diagnostics are none"),
        ),
        (
            json!({"cites": "loop", "id": growth, "polarity": "reinforcing"}),
            None,
        ),
        (
            json!({"cites": "loop", "id": crowding, "polarity": "reinforcing"}),
            Some("is balancing in run 'current'"),
        ),
        (
            json!({"cites": "loop", "id": "L99"}),
            Some("no loop has the id 'L99'"),
        ),
        (
            json!({"cites": "leads", "loop": growth, "from": 20, "to": 40}),
            Some("led the most"),
        ),
        // Growth leads the first quarter of the run, which is not the run.
        (
            json!({"cites": "leads", "loop": growth, "from": 0, "to": 40}),
            Some("led the most"),
        ),
        (
            json!({"cites": "leads", "loop": crowding, "from": 0, "to": 40}),
            None,
        ),
        (
            json!({"cites": "leads", "loop": growth, "from": 8, "to": 0}),
            Some("a span's from comes before its to"),
        ),
        // A span no loop was active in is led by none: here one between two
        // saved steps, which holds no step at all.
        (
            json!({"cites": "leads", "loop": growth, "from": 4.03, "to": 4.06}),
            Some("no loop was active between 4.03 and 4.06"),
        ),
        (
            json!({"cites": "no_loop_through", "variable": "unused"}),
            None,
        ),
        (
            json!({"cites": "no_loop_through", "variable": "capacity"}),
            None,
        ),
        (
            json!({"cites": "no_loop_through", "variable": "fractional_birth_rate"}),
            Some("goes through fractional_birth_rate"),
        ),
        (
            json!({"cites": "goes_negative", "variable": "population"}),
            Some("never goes below zero"),
        ),
        (
            json!({"cites": "peaks_at", "variable": "population", "time": 40}),
            Some("population is at its largest (100) where run 'current' ends"),
        ),
        (
            json!({"cites": "peaks_at", "variable": "births", "time": 9}),
            None,
        ),
        (
            json!({"cites": "peaks_at", "variable": "births", "time": 30}),
            Some("births peaks at"),
        ),
        (
            json!({"cites": "ends_near", "variable": "population", "value": 50, "run": "crowded"}),
            None,
        ),
        (
            json!({"cites": "behavior_mode", "variable": "births", "mode": "rise_and_fall"}),
            None,
        ),
        (
            json!({"cites": "compares", "variable": "population", "relation": "lower",
                   "run": "crowded", "than": "current"}),
            None,
        ),
        (
            json!({"cites": "compares", "variable": "population", "relation": "higher",
                   "run": "crowded", "than": "current"}),
            Some("population ends at 50 in run 'crowded' and 100 in run 'current'"),
        ),
        (
            json!({"cites": "compares", "variable": "population", "relation": "higher",
                   "run": "current", "than": "crowded"}),
            None,
        ),
        (
            json!({"cites": "compares", "variable": "population", "relation": "lower",
                   "run": "current", "than": "crowded"}),
            Some("population ends at 100 in run 'current' and 50 in run 'crowded'"),
        ),
        (
            json!({"cites": "test", "id": "T1", "outcome": "failed"}),
            Some("no battery check has the id 'T1'"),
        ),
    ];
    for (citation, says) in rows {
        let verdict = cite(&mut host, &mut session, citation.clone());
        match says {
            None => assert_eq!(verdict, Ok(()), "{citation}"),
            Some(says) => {
                let reason = verdict.expect_err(&citation.to_string());
                assert!(reason.contains(says), "{citation}: {reason}");
            }
        }
    }
}

#[test]
fn a_diagnostic_is_cited_by_the_id_read_model_gave_it() {
    let project = TestProject::new("units")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_time_units("month")
        .stock("widgets", "0", &["making"], &[], Some("widget"))
        .flow("making", "rate", Some("widget/month"))
        .aux("rate", "3", Some("widget"));
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let outline = host.call(&mut session, "read_model", json!({}));
    let id = outline["diagnostics"][0]["id"].clone();
    assert_eq!(
        cite(
            &mut host,
            &mut session,
            json!({"cites": "diagnostic", "id": id})
        ),
        Ok(())
    );
    let absent = cite(
        &mut host,
        &mut session,
        json!({"cites": "no_diagnostics", "category": "unit_consistency"}),
    );
    assert!(absent.unwrap_err().contains("the model has D"));
    assert_eq!(
        cite(
            &mut host,
            &mut session,
            json!({"cites": "no_diagnostics", "category": "equation"})
        ),
        Ok(())
    );
}

#[test]
fn a_finding_keeps_its_id_however_its_claim_is_spaced() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let finding = |claim: &str| {
        json!([{"kind": "observation", "claim": claim,
                "citations": [{"cites": "variable", "variable": "population"}]}])
    };
    let first = verify(&mut host, &mut session, finding("Population is a stock."));
    let again = verify(
        &mut host,
        &mut session,
        finding("  Population is   a stock. "),
    );
    let other = verify(&mut host, &mut session, finding("Births are a flow."));
    assert_eq!(first["findings"][0]["id"], "F1");
    assert_eq!(again["findings"][0]["id"], "F1");
    assert_eq!(other["findings"][0]["id"], "F2");
}

#[test]
fn a_run_made_before_the_model_changed_is_not_evidence() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "crowded", "set": [{"variable": "capacity", "value": 50}]}),
    );
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("max_rate")
            .unwrap()
            .set_scalar_equation("0.4")
    });
    let reason = cite(
        &mut host,
        &mut session,
        json!({"cites": "ends_near", "variable": "population", "value": 50, "run": "crowded"}),
    )
    .unwrap_err();
    assert!(
        reason.contains("before the model changed") && reason.contains("run it again"),
        "{reason}"
    );
}

#[test]
fn a_battery_check_is_cited_by_its_id_and_checked_again_after_an_edit() {
    let project = TestProject::new("workforce")
        .with_sim_time(0.0, 10.0, 0.25)
        .with_time_units("Months")
        .stock("people", "100", &[], &["leaving"], None)
        .flow("leaving", "people / tenure", None)
        .aux("tenure", "5", None)
        .aux("per_head", "budget / head_count", None)
        .aux("budget", "10", None)
        .aux("head_count", "people / 2", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    host.call(&mut session, "read_model", json!({}));
    // A tenure of zero, the call's own extreme, divides by it.
    let tests = host.call(
        &mut session,
        "run_tests",
        json!({
            "tests": ["extreme_conditions"],
            "extremes": [{"variable": "tenure", "low": 0}]
        }),
    );
    let failed = tests["results"][0]["id"].clone();
    assert_eq!(tests["results"][0]["outcome"], "failed");
    let citation = json!({"cites": "test", "id": failed, "outcome": "failed"});
    assert_eq!(cite(&mut host, &mut session, citation.clone()), Ok(()));

    // Guard the division, and the check passes when run again, at the
    // extreme the call gave.
    host.edit(|p| {
        p.models[0]
            .get_variable_mut("leaving")
            .unwrap()
            .set_scalar_equation("people / MAX(tenure, 1)")
    });
    let reason = cite(&mut host, &mut session, citation).unwrap_err();
    assert!(reason.contains("comes out passed"), "{reason}");
    assert_eq!(
        cite(
            &mut host,
            &mut session,
            json!({"cites": "test", "id": failed, "outcome": "passed"})
        ),
        Ok(())
    );
}

#[test]
fn an_element_of_an_arrayed_variable_is_cited_by_its_subscript() {
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "10", &["births"], &[], None)
        .array_flow("births[region]", "pop * 0.1", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    assert_eq!(
        cite(
            &mut host,
            &mut session,
            json!({"cites": "behavior_mode", "variable": "pop[South]", "mode": "exponential"})
        ),
        Ok(())
    );
    let reason = cite(
        &mut host,
        &mut session,
        json!({"cites": "behavior_mode", "variable": "pop", "mode": "exponential"}),
    )
    .unwrap_err();
    assert!(reason.contains("name one of its elements"), "{reason}");
    let reason = cite(
        &mut host,
        &mut session,
        json!({"cites": "ends_near", "variable": "pop[east]", "value": 1}),
    )
    .unwrap_err();
    assert!(reason.contains("has no element [east]"), "{reason}");
    assert_eq!(
        cite(
            &mut host,
            &mut session,
            json!({"cites": "equation", "variable": "births[South]", "equation": "pop*0.1"})
        ),
        Ok(())
    );
    let reason = cite(
        &mut host,
        &mut session,
        json!({"cites": "no_loop_through", "variable": "pop[south]"}),
    )
    .unwrap_err();
    assert!(reason.contains("goes through pop[south]"), "{reason}");
}

/// A diagram edit changes nothing a run simulated: a run made before one is
/// still evidence.
#[test]
fn a_run_made_before_a_diagram_edit_is_still_evidence() {
    let mut host = Host::new(crate::tools::test_support::with_diagram(
        logistic().build_datamodel(),
    ));
    let mut session = Session::new("main");
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "crowded", "set": [{"variable": "capacity", "value": 50}]}),
    );
    host.edit(crate::tools::test_support::zoom_the_diagram);
    assert_eq!(
        cite(
            &mut host,
            &mut session,
            json!({"cites": "ends_near", "variable": "population", "value": 50, "run": "crowded"})
        ),
        Ok(())
    );
}

#[test]
fn malformed_findings_are_refused() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let variable = json!({"cites": "variable", "variable": "population"});
    for (findings, says) in [
        (
            json!([{"kind": "flaw", "claim": " ", "citations": [variable]}]),
            "has no claim",
        ),
        (
            json!([{"kind": "flaw", "claim": "x", "citations": []}]),
            "between 1 and 8",
        ),
        (
            json!(
                (0..=MAX_FINDINGS)
                    .map(|_| json!({"kind": "flaw", "claim": "x", "citations": [variable]}))
                    .collect::<Vec<_>>()
            ),
            "between 1 and 12",
        ),
        (
            json!([{"kind": "flaw", "claim": "x", "citations": [{"cites": "rumor"}]}]),
            "rumor",
        ),
    ] {
        let refusal = host.refuse(
            &mut session,
            "verify_findings",
            json!({ "findings": findings }),
        );
        let message = refusal["error"].as_str().unwrap();
        assert!(message.contains(says), "{message}");
    }
}

/// A value is judged against the value itself, not the series' range: 40
/// is not where a series that starts at 1 starts, nor 30 its value at a time
/// it is 10. A value that reads as zero beside the series is zero, a time
/// the run does not reach is refused, a series that holds still has no
/// peak, and two runs that end a hair apart end alike.
#[test]
fn a_value_is_judged_against_the_value_and_a_time_against_the_run() {
    let project = TestProject::new("growth")
        .with_sim_time(0.0, 60.0, 0.25)
        .stock("population", "1", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.115", None)
        .stock("residue", "100", &[], &["decay"], None)
        .flow("decay", "residue * 0.5", None)
        .aux("idle", "5", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    host.call(
        &mut session,
        "run_experiment",
        json!({"name": "hair", "set": [{"variable": "rate", "value": 0.11500001}]}),
    );
    let rows: Vec<(Value, Option<&str>)> = vec![
        (
            json!({"cites": "value", "variable": "population", "value": 40}),
            Some("population is 1 at 0"),
        ),
        (
            json!({"cites": "value", "variable": "population", "value": 1.02}),
            None,
        ),
        (
            json!({"cites": "value", "variable": "population", "value": 30, "time": 20}),
            Some("population is 9.6558 at 20"),
        ),
        (
            json!({"cites": "value", "variable": "population", "value": 500, "time": 500}),
            Some("run 'current' goes from 0 to 60, so it has no time 500"),
        ),
        (
            json!({"cites": "ends_near", "variable": "residue", "value": 0}),
            None,
        ),
        (
            json!({"cites": "ends_near", "variable": "residue", "value": 5}),
            Some("residue ends at"),
        ),
        (
            json!({"cites": "peaks_at", "variable": "idle", "time": 0}),
            Some("idle holds at 5 throughout run 'current', so it has no peak"),
        ),
        (
            json!({"cites": "peaks_at", "variable": "population", "time": 90}),
            Some("so it has no time 90"),
        ),
        (
            json!({"cites": "compares", "variable": "population", "relation": "higher",
                   "run": "hair", "than": "current"}),
            Some("population ends at"),
        ),
    ];
    for (citation, says) in rows {
        let verdict = cite(&mut host, &mut session, citation.clone());
        match says {
            None => assert_eq!(verdict, Ok(()), "{citation}"),
            Some(says) => {
                let reason = verdict.expect_err(&citation.to_string());
                assert!(reason.contains(says), "{citation}: {reason}");
            }
        }
    }
}

/// The floor is for values near zero. On a series that spans orders of
/// magnitude -- exponential growth from 1 to 136,420, an epidemic whose
/// infected rise from 1 to 235,350 -- a small value is judged by 5% of
/// itself, as any other is, so the book's own falsehoods fail, and a true
/// small value holds.
#[test]
fn the_value_floor_applies_only_near_zero() {
    let growth = TestProject::new("growth")
        .with_sim_time(0.0, 100.0, 0.25)
        .stock("population", "1", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "0.12", None);
    let epidemic = TestProject::new("sir")
        .with_sim_time(0.0, 100.0, 0.125)
        .stock("susceptible", "999999", &[], &["infection"], None)
        .stock("infected", "1", &["infection"], &["recovery"], None)
        .stock("recovered", "0", &["recovery"], &[], None)
        .flow(
            "infection",
            "contact_rate * susceptible * infected / 1000000",
            None,
        )
        .flow("recovery", "infected / duration", None)
        .aux("contact_rate", "0.5", None)
        .aux("duration", "5", None);
    for (project, rows) in [
        (
            &growth,
            vec![
                (
                    json!({"cites": "value", "variable": "population", "value": 40}),
                    Some("population is 1 at 0"),
                ),
                (
                    json!({"cites": "value", "variable": "population", "value": 30, "time": 20}),
                    Some("at 20 in run 'current'"),
                ),
                (
                    json!({"cites": "value", "variable": "population", "value": 100, "time": 30}),
                    Some("at 30 in run 'current'"),
                ),
                (
                    json!({"cites": "value", "variable": "population", "value": 1}),
                    None,
                ),
            ],
        ),
        (
            &epidemic,
            vec![
                (
                    json!({"cites": "value", "variable": "infected", "value": 100}),
                    Some("infected is 1 at 0"),
                ),
                (
                    json!({"cites": "value", "variable": "infected", "value": 200, "time": 10}),
                    Some("at 10 in run 'current'"),
                ),
                (
                    json!({"cites": "value", "variable": "infected", "value": 1}),
                    None,
                ),
            ],
        ),
    ] {
        let mut host = Host::from_test_project(project);
        let mut session = Session::new("main");
        for (citation, says) in rows {
            let verdict = cite(&mut host, &mut session, citation.clone());
            match says {
                None => assert_eq!(verdict, Ok(()), "{citation}"),
                Some(says) => {
                    let reason = verdict.expect_err(&citation.to_string());
                    assert!(reason.contains(says), "{citation}: {reason}");
                }
            }
        }
    }
}

/// A variable with a table is its table at its equation's value, and its
/// equation is cited that way: the input alone is not what it computes.
#[test]
fn a_table_variables_equation_is_its_table_at_its_input() {
    let mut host = Host::from_test_project(&crate::tools::test_support::inventory());
    let mut session = Session::new("main");
    let bare = cite(
        &mut host,
        &mut session,
        json!({"cites": "equation", "variable": "effect_of_pressure",
               "equation": "Inventory / desired_inventory"}),
    );
    let reason = bare.expect_err("the input is not the value");
    assert!(
        reason.contains("`LOOKUP(effect_of_pressure, Inventory / desired_inventory)`"),
        "{reason}"
    );
    assert_eq!(
        cite(
            &mut host,
            &mut session,
            json!({"cites": "equation", "variable": "effect of pressure",
                   "equation": "lookup(Effect_of_Pressure, inventory/desired_inventory)"}),
        ),
        Ok(())
    );
}

/// A verification stops once other work waits for the project, at the
/// citation whose run or analysis it would make next, and gives out no id:
/// the next finding the session verifies is its first.
#[test]
fn a_verification_stops_when_other_work_waits_and_gives_no_id() {
    let findings = json!([
        {"kind": "observation", "claim": "The capacity is 100.",
         "citations": [{"cites": "value", "variable": "capacity", "value": 100}]},
        {"kind": "observation", "claim": "Crowding leads a crowded run late.",
         "citations": [{"cites": "loop", "id": "L2", "run": "crowded"}]}
    ]);
    let mut stopped = 0;
    for after in 1.. {
        let mut host = Host::from_test_project(&logistic());
        let mut session = Session::new("main");
        loop_ids(&mut host, &mut session);
        host.call(
            &mut session,
            "run_experiment",
            json!({"name": "crowded", "set": [{"variable": "capacity", "value": 50}]}),
        );
        let output = host.call_waiting(
            &mut session,
            "verify_findings",
            json!({"findings": findings}),
            &Host::waiting_after(after),
        );
        if !output.is_error {
            break;
        }
        assert!(output.interrupted, "{}", output.json);
        stopped += 1;
        let other = verify(
            &mut host,
            &mut session,
            json!([{"kind": "observation", "claim": "Another claim.",
                    "citations": [{"cites": "variable", "variable": "capacity"}]}]),
        );
        assert_eq!(other["findings"][0]["id"], "F1", "no id was kept: {other}");
    }
    assert!(
        stopped >= 3,
        "before each citation and each stage: {stopped}"
    );
}

/// Check each row's citation: `None` holds, `Some` fails saying so.
fn check_rows(host: &mut Host, session: &mut Session, rows: Vec<(Value, Option<&str>)>) {
    for (citation, says) in rows {
        let verdict = cite(host, session, citation.clone());
        match says {
            None => assert_eq!(verdict, Ok(()), "{citation}"),
            Some(says) => {
                let reason = verdict.expect_err(&citation.to_string());
                assert!(reason.contains(says), "{citation}: {reason}");
            }
        }
    }
}

/// A cited value is judged against the value, however small either is
/// beside the series: on growth from 2 to tens of billions, 50,000 is not
/// where it starts, and neither is a value of the other sign. Only a cited
/// zero is judged against the series' size.
#[test]
fn a_small_value_does_not_stand_for_another_on_a_series_that_grows_large() {
    let project = TestProject::new("growth")
        .with_sim_time(0.0, 25.0, 0.125)
        .stock("population", "2", &["births"], &[], None)
        .flow("births", "population * rate", None)
        .aux("rate", "1", None)
        .stock("residue", "100", &[], &["decay"], None)
        .flow("decay", "residue", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let behavior = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["population"]}),
    );
    assert!(
        behavior["series"][0]["end"].as_f64().unwrap() > 1e10,
        "the premise: the series ends ten orders of magnitude above its start"
    );
    check_rows(
        &mut host,
        &mut session,
        vec![
            (
                json!({"cites": "value", "variable": "population", "value": 2}),
                None,
            ),
            (
                json!({"cites": "value", "variable": "population", "value": 50000}),
                Some("population is 2 at 0"),
            ),
            (
                json!({"cites": "value", "variable": "population", "value": 300000, "time": 2}),
                Some("at 2 in run 'current'"),
            ),
            (
                json!({"cites": "value", "variable": "population", "value": -2}),
                Some("population is 2 at 0"),
            ),
            // A stock that drains to nothing ends at zero, and at nothing
            // else.
            (
                json!({"cites": "ends_near", "variable": "residue", "value": 0}),
                None,
            ),
            (
                json!({"cites": "ends_near", "variable": "residue", "value": 0.0005}),
                Some("residue ends at"),
            ),
        ],
    );
}

/// The value tolerance at its edge: 5% of the larger of the actual value
/// and the cited one, on either side of a value of 100.
#[test]
fn a_value_is_within_five_percent_of_the_larger_of_the_two() {
    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    for (cited, holds) in [
        (100.0, true),
        (105.0, true),
        // 5.3 is more than 5% of 105.3.
        (105.3, false),
        // 4.8 is within 5% of 100, the larger, though not of 95.2.
        (95.2, true),
        (94.9, false),
        (-100.0, false),
        (0.0, false),
    ] {
        let verdict = cite(
            &mut host,
            &mut session,
            json!({"cites": "value", "variable": "capacity", "value": cited}),
        );
        assert_eq!(verdict.is_ok(), holds, "{cited}: {verdict:?}");
    }
    // The rule itself, at zero: a cited zero is as near as the series is
    // large, and a value is never near one of the other sign.
    let series = [0.0, 1000.0];
    for (actual, cited, holds) in [
        (0.009, 0.0, true),
        (0.011, 0.0, false),
        (-0.009, 0.0, true),
        (0.0, 0.009, false),
        (0.009, 0.0091, true),
        (0.009, -0.009, false),
        (f64::NAN, 0.0, false),
    ] {
        assert_eq!(
            near(actual, cited, &series),
            holds,
            "{actual} cited as {cited}"
        );
    }
    // Two runs end apart by more than both tolerances.
    for (a, b, holds) in [
        (100.0, 94.0, true),
        (100.0, 96.0, false),
        (0.009, 0.0, false),
        (0.02, 0.0, true),
        (0.02, 0.0199, false),
        (f64::NAN, 1.0, false),
    ] {
        assert_eq!(apart(a, b, &series), holds, "{a} and {b}");
        assert_eq!(apart(b, a, &series), holds, "{b} and {a}");
    }
}

/// A peak is a largest value the series comes down from on both sides, and
/// a cited time is within 5% of the run of it.
#[test]
fn a_peak_is_a_turn_inside_the_run_within_five_percent_of_the_cited_time() {
    let times: Vec<f64> = (0..=10).map(f64::from).collect();
    let series = |f: fn(f64) -> f64| times.iter().map(|&t| f(t)).collect::<Vec<f64>>();
    let at = |values: &[f64]| peak(&times, values, 0.0).ok();
    assert_eq!(
        at(&series(|t| 25.0 - (t - 4.0) * (t - 4.0))),
        Some((4.0, 25.0))
    );
    // Growth is largest where the run ends, decay where it starts, and a
    // series that rose to a plateau holds its largest to the end.
    assert!(matches!(
        peak(&times, &series(|t| t * t), 0.0),
        Err(NoPeak::LargestAtTheEnd(_))
    ));
    assert!(matches!(
        peak(&times, &series(|t| 100.0 - t), 0.0),
        Err(NoPeak::LargestAtTheStart(_))
    ));
    assert!(matches!(
        peak(&times, &series(|t| t.min(5.0)), 0.0),
        Err(NoPeak::LargestAtTheEnd(_))
    ));
    assert!(matches!(
        peak(&times, &series(|_| 3.0), 0.0),
        Err(NoPeak::Still(_))
    ));
    // A fall of less than the noise a behavior mode ignores is no turn, at
    // either end: the largest value is a step inside the run, and the end
    // is half a unit under it on a range of ninety.
    assert!(matches!(
        peak(
            &times,
            &series(|t| if t == 10.0 { 89.5 } else { t * 10.0 }),
            0.0
        ),
        Err(NoPeak::LargestAtTheEnd(_))
    ));
    assert!(matches!(
        peak(
            &times,
            &series(|t| if t == 0.0 { 99.5 } else { 110.0 - t * 10.0 }),
            0.0
        ),
        Err(NoPeak::LargestAtTheStart(_))
    ));
    // A top the series holds for a while peaks where it is first reached.
    assert_eq!(
        at(&[0.0, 5.0, 10.0, 10.0, 10.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
        Some((2.0, 10.0))
    );

    let mut host = Host::from_test_project(&logistic());
    let mut session = Session::new("main");
    let behavior = host.call(
        &mut session,
        "read_behavior",
        json!({"variables": ["births"]}),
    );
    let peaks = behavior["series"][0]["max"]["time"].as_f64().unwrap();
    // The run is 40 long, so a cited time is within 2 of the peak.
    for (cited, holds) in [
        (peaks, true),
        (peaks + 1.9, true),
        (peaks - 1.9, true),
        (peaks + 2.1, false),
        (peaks - 2.1, false),
    ] {
        let verdict = cite(
            &mut host,
            &mut session,
            json!({"cites": "peaks_at", "variable": "births", "time": cited}),
        );
        assert_eq!(verdict.is_ok(), holds, "{cited}: {verdict:?}");
    }
}

/// A loop leads a span it led at least half the active steps of, and a loop
/// is of a cited polarity when the run shows that polarity, or mostly that
/// polarity: a row per pair of polarities.
#[test]
fn a_lead_is_half_the_active_steps_and_a_polarity_is_its_own_or_mostly_it() {
    for (led, active, holds) in [
        (1, 2, true),
        (2, 3, true),
        (1, 3, false),
        (0, 1, false),
        (0, 0, false),
        (3, 3, true),
    ] {
        assert_eq!(leads(led, active), holds, "{led} of {active}");
    }
    use LoopPolarityName::{
        Balancing, MostlyBalancing, MostlyReinforcing, Reinforcing, Undetermined,
    };
    for cited in LoopPolarityName::ALL {
        for actual in LoopPolarityName::ALL {
            let expected = match (cited, actual) {
                (Reinforcing, Reinforcing | MostlyReinforcing) => true,
                (Balancing, Balancing | MostlyBalancing) => true,
                (MostlyReinforcing, MostlyReinforcing) => true,
                (MostlyBalancing, MostlyBalancing) => true,
                (Undetermined, Undetermined) => true,
                (
                    Reinforcing | Balancing | MostlyReinforcing | MostlyBalancing | Undetermined,
                    _,
                ) => false,
            };
            assert_eq!(
                polarity_matches(cited, actual),
                expected,
                "{} cited of a loop that is {}",
                polarity_name(cited),
                polarity_name(actual)
            );
        }
    }
}

/// An equation is cited any way that parses the same, and no other way:
/// spacing, case, the spelling of names and of numbers aside; an operator,
/// an order or a grouping is the equation.
#[test]
fn an_equation_is_the_same_only_as_it_parses() {
    let project = TestProject::new("forms")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("a", "100", None)
        .aux("b", "4", None)
        .aux("c", "2", None)
        .aux("left", "a - b - c", None)
        .aux("less", "a - b", None)
        .aux("opposite", "-a", None)
        .aux("larger", "MAX(a, b)", None)
        .aux("chosen", "IF a > b THEN a ELSE b", None)
        .named_dimension("region", &["north", "south"])
        .array_aux("share[region]", "0.5")
        .aux("picked", "share[north]", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let equation = |variable: &str, equation: &str| json!({"cites": "equation", "variable": variable, "equation": equation});
    check_rows(
        &mut host,
        &mut session,
        vec![
            (equation("a", "100"), None),
            (equation("a", "1e2"), None),
            (equation("a", "100.0"), None),
            (equation("a", "1000"), Some("a's equation is `100`")),
            (equation("left", "A-B-C"), None),
            (equation("left", "(a - b) - c"), None),
            (equation("left", "a - (b - c)"), Some("left's equation is")),
            (equation("left", "a + b - c"), Some("left's equation is")),
            (equation("left", "a - c - b"), Some("left's equation is")),
            (equation("less", "b - a"), Some("less's equation is")),
            (equation("opposite", "- a"), None),
            (equation("opposite", "+a"), Some("opposite's equation is")),
            (equation("opposite", "a"), Some("opposite's equation is")),
            (equation("larger", "max( A , B )"), None),
            (
                equation("larger", "MIN(a, b)"),
                Some("larger's equation is"),
            ),
            (equation("chosen", "if A > B then A else B"), None),
            (
                equation("chosen", "IF a > b THEN b ELSE a"),
                Some("chosen's equation is"),
            ),
            (equation("picked", "Share[North]"), None),
            (
                equation("picked", "share[south]"),
                Some("picked's equation is"),
            ),
            (
                equation("chosen", "IF a >= b THEN a ELSE b"),
                Some("chosen's equation is"),
            ),
        ],
    );
}

/// An equation citation of an arrayed variable names an element it has:
/// that element's own equation, with its own table when it has one, or the
/// default that covers it. A table other equations look up has no equation
/// of its own, and is cited with none.
#[test]
fn an_equation_citation_names_an_element_the_variable_has() {
    let table = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0]),
        y_points: vec![0.0, 2.0],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
    };
    let mut project = TestProject::new("regions")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("region", &["north", "south", "east"])
        .named_dimension("age", &["young", "old"])
        .array_aux("all[region]", "0.3")
        .array_aux("square[region, age]", "0.4")
        .array_with_default_and_overrides(
            "partial[region]",
            "0.1",
            vec![("north", "0.5"), ("south", "0.2")],
        )
        .aux_with_gf("standalone", "", table.clone())
        .build_datamodel();
    if let Some(datamodel::Variable::Aux(aux)) = project.models[0].get_variable_mut("partial")
        && let datamodel::Equation::Arrayed(_, elements, _, _) = &mut aux.equation
    {
        elements[0].3 = Some(table);
    }
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let equation = |variable: &str, equation: &str| json!({"cites": "equation", "variable": variable, "equation": equation});
    check_rows(
        &mut host,
        &mut session,
        vec![
            (equation("all", "0.3"), None),
            (equation("all[North]", "0.3"), None),
            (
                equation("all[narnia]", "0.3"),
                Some("all has no element [narnia]: `narnia` is not an element of region"),
            ),
            (
                equation("all[north][x", "0.3"),
                Some("the model has no variable"),
            ),
            (equation("square[north, Old]", "0.4"), None),
            (
                equation("square[north]", "0.4"),
                Some("square has no element [north]: it is arrayed over 2"),
            ),
            (
                equation("a_scalar[north]", "1"),
                Some("the model has no variable"),
            ),
            // Its own arm, with its own table.
            (
                equation("partial[north]", "LOOKUP(partial[north], 0.5)"),
                None,
            ),
            (
                equation("partial[north]", "0.5"),
                Some("partial[north]'s equation is `LOOKUP(partial[north], 0.5)`"),
            ),
            (equation("partial[south]", "0.2"), None),
            // No arm of its own: the default's.
            (equation("partial[east]", "0.1"), None),
            (
                equation("partial[east]", "0.2"),
                Some("partial[east]'s equation is `0.1`"),
            ),
            (
                equation("partial", "0.1"),
                Some("partial has an equation per element: name one"),
            ),
            (equation("standalone", ""), None),
            (
                equation("standalone", "0"),
                Some("standalone is a table other equations look up"),
            ),
        ],
    );
}

/// A fact of a run about an arrayed variable names an element it has; a
/// variable that goes negative in any element goes negative.
#[test]
fn a_series_citation_names_an_element_the_variable_has() {
    let project = TestProject::new("regions")
        .with_sim_time(0.0, 4.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_with_default_and_overrides("level[region]", "1", vec![("south", "1 - TIME")])
        .array_aux("steady[region]", "3")
        .build_datamodel();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    check_rows(
        &mut host,
        &mut session,
        vec![
            (json!({"cites": "goes_negative", "variable": "level"}), None),
            (
                json!({"cites": "goes_negative", "variable": "level[South]"}),
                None,
            ),
            (
                json!({"cites": "goes_negative", "variable": "level[north]"}),
                Some("never goes below zero"),
            ),
            (
                json!({"cites": "goes_negative", "variable": "steady"}),
                Some("its least value is 3"),
            ),
            (
                json!({"cites": "ends_near", "variable": "level[ south ]", "value": -3}),
                None,
            ),
            (
                json!({"cites": "ends_near", "variable": "level[west]", "value": -3}),
                Some("level has no element [west]: `west` is not an element of region"),
            ),
            (
                json!({"cites": "ends_near", "variable": "level", "value": 1}),
                Some("level is arrayed: name one of its elements, as level[north]"),
            ),
        ],
    );
}

/// A table is read by the variable that looks it up: a readers citation
/// counts it, though the table orders nothing and is no causal link.
#[test]
fn a_tables_readers_are_the_variables_that_look_it_up() {
    let gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0]),
        y_points: vec![0.0, 2.0],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
    };
    let project = TestProject::new("table")
        .aux_with_gf("effect_table", "", gf)
        .aux("effect", "LOOKUP(effect_table, pressure)", None)
        .aux("pressure", "TIME / 10", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let verdict = |host: &mut Host, session: &mut Session, readers: Value| {
        let citation = json!({"cites": "readers", "variable": "effect_table", "readers": readers});
        host.call(
            session,
            "verify_findings",
            json!({"findings": [{"kind": "observation", "claim": "c", "citations": [citation]}]}),
        )["findings"][0]
            .clone()
    };
    assert_eq!(
        verdict(&mut host, &mut session, json!(["effect"]))["holds"],
        true
    );
    let failed = verdict(&mut host, &mut session, json!([]));
    assert_eq!(failed["holds"], false);
    assert!(
        failed["failures"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("read by effect"),
        "{failed}"
    );
}

/// Verdicts that do not fit the budget are left out, counted, and given no
/// id: the next finding verified takes the next id after the ones shown.
#[test]
fn findings_left_out_of_an_answer_get_no_id() {
    let mut host = Host::from_test_project(&TestProject::new("m").aux("x", "1", None));
    let mut session = Session::new("main");
    session.outline_budget = 200;
    let findings: Vec<Value> = (0..12)
        .map(|i| {
            json!({"kind": "observation", "claim": format!("claim {i}"),
                   "citations": [{"cites": "no_diagnostics"}]})
        })
        .collect();
    let answer = host.call(
        &mut session,
        "verify_findings",
        json!({ "findings": findings }),
    );
    let shown = answer["findings"].as_array().unwrap().len();
    assert!(shown < 12, "{answer}");
    assert_eq!(answer["omitted"], 12 - shown);
    assert!(answer.to_string().len() <= 200);
    let next = host.call(
        &mut session,
        "verify_findings",
        json!({"findings": [{"kind": "observation", "claim": "another",
                             "citations": [{"cites": "no_diagnostics"}]}]}),
    );
    assert_eq!(
        next["findings"][0]["id"],
        format!("F{}", shown + 1),
        "{next}"
    );
}

/// A citation's failures are cut before whole findings are: the finding
/// with the most failures loses its last first, counted.
#[test]
fn a_findings_failures_are_cut_before_findings_are() {
    let mut host = Host::from_test_project(&TestProject::new("m").aux("x", "1", None));
    let mut session = Session::new("main");
    session.outline_budget = 250;
    let failing = json!({"cites": "value", "variable": "x", "value": 50});
    let answer = host.call(
        &mut session,
        "verify_findings",
        json!({"findings": [{"kind": "observation", "claim": "c",
                             "citations": vec![failing; 8]}]}),
    );
    let verdict = &answer["findings"][0];
    let listed = verdict["failures"].as_array().unwrap().len();
    assert!((1..8).contains(&listed), "{answer}");
    assert_eq!(verdict["omittedFailures"], 8 - listed);
}

/// A `readers` citation is exhaustive only over a model whose equations
/// all parse: one that does not may read the variable, so the citation
/// says what is true (no reader found, and which equations could not be
/// read) rather than holding.
#[test]
fn readers_are_not_certified_while_an_equation_cannot_be_read() {
    let project = TestProject::new("unread")
        .aux("cited", "1", None)
        .aux("garbled", "cited +", None);
    let mut host = Host::from_test_project(&project);
    let mut session = Session::new("main");
    let answer = host.call(
        &mut session,
        "verify_findings",
        json!({"findings": [{"kind": "observation", "claim": "nothing reads cited",
                             "citations": [{"cites": "readers", "variable": "cited", "readers": []}]}]}),
    );
    let verdict = &answer["findings"][0];
    assert_eq!(verdict["holds"], false, "{answer}");
    let reason = verdict["failures"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("could not be read") && reason.contains("garbled"),
        "{reason}"
    );
}
