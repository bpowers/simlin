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
