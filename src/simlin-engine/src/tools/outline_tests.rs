// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

fn outline_of(project: datamodel::Project) -> Value {
    let mut host = Host::new(project);
    host.call(&mut Session::new("main"), "read_model", json!({}))
}

fn names(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn every_variable_is_listed_once_under_its_kind_with_its_equation_and_units() {
    let outline = outline_of(inventory().build_datamodel());
    assert_eq!(names(&outline["stocks"]), ["Inventory"]);
    assert_eq!(names(&outline["flows"]), ["production", "shipments"]);
    assert_eq!(
        names(&outline["variables"]),
        ["orders", "desired_inventory", "effect_of_pressure"]
    );
    assert_eq!(
        names(&outline["constants"]),
        ["coverage", "adjustment_time"]
    );
    assert_eq!(names(&outline["lookups"]), ["pressure_table"]);
    assert!(outline.get("modules").is_none(), "{outline}");
    assert_eq!(
        outline["counts"],
        json!({"stocks": 1, "flows": 2, "variables": 3, "constants": 2, "lookups": 1,
               "modules": 0, "errors": 0, "warnings": 0})
    );

    assert_eq!(
        outline["stocks"][0],
        json!({"name": "Inventory", "units": "widget", "initial": "desired_inventory",
               "inflows": ["production"], "outflows": ["shipments"], "nonNegative": true})
    );
    assert_eq!(
        outline["flows"][1],
        json!({"name": "shipments", "units": "widget/month", "equation": "orders"})
    );
    assert_eq!(
        outline["constants"][0],
        json!({"name": "coverage", "units": "month", "value": "4"})
    );
    assert_eq!(
        outline["variables"][2]["lookup"],
        json!({"points": 3, "xMin": 0.0, "xMax": 2.0, "yMin": 0.0, "yMax": 1.0})
    );
    assert_eq!(
        outline["specs"],
        json!({"start": 0.0, "stop": 20.0, "dt": 0.25, "method": "euler", "timeUnits": "month"})
    );
}

#[test]
fn each_entry_carries_the_ids_of_its_own_diagnostics() {
    let mut project = inventory().build_datamodel();
    project.models[0]
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    let outline = outline_of(project);
    let id = outline["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["variable"] == "shipments")
        .unwrap()["id"]
        .clone();
    assert_eq!(outline["flows"][1]["diagnostics"], json!([id]));
    assert!(outline["flows"][0].get("diagnostics").is_none());
    assert!(outline["counts"]["errors"].as_u64().unwrap() >= 1);
}

#[test]
fn a_stock_that_lists_a_flow_twice_is_outlined_with_it_once() {
    let project = TestProject::new("p")
        .stock("s", "1", &["in", "In"], &[], None)
        .flow("in", "1", None)
        .build_datamodel();
    let outline = outline_of(project);
    assert_eq!(outline["stocks"][0]["inflows"], json!(["in"]));
}

#[test]
fn arrayed_equations_are_outlined_with_their_dimensions() {
    let project = TestProject::new("p")
        .named_dimension("region", &["north", "south"])
        .array_const("capacity[region]", 5.0)
        .array_aux("demand[region]", "capacity * 2")
        .array_with_default_and_overrides("price[region]", "10", vec![("south", "12")])
        .build_datamodel();
    let outline = outline_of(project);
    let constants = outline["constants"].as_array().unwrap();
    let capacity = constants.iter().find(|c| c["name"] == "capacity").unwrap();
    assert_eq!(capacity["dimensions"], json!(["region"]));
    let demand = &outline["variables"][0];
    assert_eq!(demand["name"], "demand");
    assert_eq!(demand["equation"], "capacity * 2");
    assert_eq!(demand["dimensions"], json!(["region"]));
    // Per-element numbers are a constant, spelled element by element.
    let price = constants.iter().find(|c| c["name"] == "price").unwrap();
    assert_eq!(price["dimensions"], json!(["region"]));
    assert!(
        price["value"].as_str().unwrap().contains("south: 12"),
        "{price}"
    );
}

#[test]
fn a_long_equation_is_cut_and_says_so() {
    let long = (0..100)
        .map(|i| format!("x{i}"))
        .collect::<Vec<_>>()
        .join(" + ");
    let mut project = TestProject::new("p").aux("total", &long, None);
    for i in 0..100 {
        project = project.aux(&format!("x{i}"), "1", None);
    }
    let outline = outline_of(project.build_datamodel());
    let total = &outline["variables"][0];
    assert_eq!(total["name"], "total");
    assert_eq!(total["truncated"], true);
    let equation = total["equation"].as_str().unwrap();
    assert_eq!(equation.chars().count(), OUTLINE_EQUATION_CHARS + 1);
    assert!(equation.ends_with('…'));
    assert!(
        outline["variables"].as_array().unwrap()[1..]
            .iter()
            .all(|v| v.get("truncated").is_none())
    );
}

#[test]
fn the_specs_name_their_method_and_a_save_step_only_when_it_differs_from_dt() {
    for (method, name) in [
        (datamodel::SimMethod::Euler, "euler"),
        (datamodel::SimMethod::RungeKutta2, "rk2"),
        (datamodel::SimMethod::RungeKutta4, "rk4"),
    ] {
        let project = TestProject::new("p")
            .with_sim_time(0.0, 10.0, 0.5)
            .with_save_step(1.0)
            .with_sim_method(method)
            .aux("x", "1", None)
            .build_datamodel();
        let specs = &outline_of(project)["specs"];
        assert_eq!(specs["method"], name);
        assert_eq!(specs["saveStep"], 1.0);
    }
    let mut project = TestProject::new("p").aux("x", "1", None).build_datamodel();
    project.sim_specs.dt = datamodel::Dt::Reciprocal(4.0);
    project.sim_specs.save_step = Some(datamodel::Dt::Dt(0.25));
    let specs = &outline_of(project)["specs"];
    assert_eq!(specs["dt"], 0.25);
    assert!(specs.get("saveStep").is_none(), "{specs}");
}

#[test]
fn a_lookup_without_x_points_spans_its_x_scale() {
    let gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Discrete,
        x_points: None,
        y_points: vec![3.0, 1.0, 2.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 10.0,
            max: 30.0,
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 5.0 },
    };
    let summary = lookup_summary(&gf);
    assert_eq!(
        (
            summary.points,
            summary.x_min,
            summary.x_max,
            summary.y_min,
            summary.y_max
        ),
        (3, 10.0, 30.0, 1.0, 3.0)
    );
}

#[test]
fn modules_are_outlined_with_their_model_and_wiring() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx"
    );
    let file = std::fs::File::open(path).expect("the hares and foxes model is in the corpus");
    let project = crate::xmile::project_from_reader(&mut std::io::BufReader::new(file)).unwrap();
    let outline = outline_of(project);
    assert_eq!(names(&outline["modules"]), ["hares", "lynxes"]);
    assert_eq!(outline["modules"][0]["model"], "hares");
    assert!(
        outline["modules"][0]["inputs"]
            .as_array()
            .unwrap()
            .contains(&json!({"from": "·area", "to": "hares·area"})),
        "{outline}"
    );
}

/// A model with `n` stocks, each with a flow, in two sectors: evens and odds.
fn sectored(n: usize) -> datamodel::Project {
    let mut project = TestProject::new("big");
    for i in 0..n {
        project = project
            .stock(
                &format!("stock_{i}"),
                "1",
                &[&format!("flow_{i}")],
                &[],
                None,
            )
            .flow(
                &format!("flow_{i}"),
                &format!("stock_{i} * 0.1 + MAX(0, 5 - stock_{i}) / 2"),
                None,
            );
    }
    let mut project = project.build_datamodel();
    let (first, second): (Vec<usize>, Vec<usize>) = (0..n).partition(|i| i % 2 == 0);
    let members = |indices: &[usize]| {
        indices
            .iter()
            .flat_map(|i| [format!("stock_{i}"), format!("flow_{i}")])
            .collect()
    };
    project.models[0].groups = vec![
        datamodel::ModelGroup {
            name: "evens".to_string(),
            members: members(&first),
            ..Default::default()
        },
        datamodel::ModelGroup {
            name: "odds".to_string(),
            members: members(&second),
            ..Default::default()
        },
    ];
    project
}

fn outline_with_budget(project: datamodel::Project, budget: usize) -> (Value, usize) {
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    session.outline_budget = budget;
    let output = host.call_raw(&mut session, "read_model", "{}");
    assert!(!output.is_error, "{}", output.json);
    (
        serde_json::from_str(&output.json).unwrap(),
        output.json.len(),
    )
}

#[test]
fn a_model_over_the_budget_is_outlined_by_sector_with_every_count_whole() {
    // Twenty stocks and their flows: enough that listing the stocks alone,
    // with the sectors and the note, is shorter than listing everything.
    let n = 20;
    let (whole, whole_len) = outline_with_budget(sectored(n), usize::MAX);
    assert!(whole.get("sectors").is_none() && whole.get("note").is_none());

    let (outline, len) = outline_with_budget(sectored(n), whole_len - 1);
    assert!(len < whole_len);
    assert_eq!(outline["counts"], whole["counts"], "counts stay whole");
    assert!(outline.get("flows").is_none(), "{outline}");
    assert_eq!(names(&outline["stocks"]).len(), n);
    assert!(
        outline["stocks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s.get("initial").is_none() && s["inflows"].as_array().unwrap().len() == 1)
    );
    let stocks = |parity: usize| -> Vec<String> {
        (0..n)
            .filter(|i| i % 2 == parity)
            .map(|i| format!("stock_{i}"))
            .collect()
    };
    assert_eq!(
        outline["sectors"],
        json!([
            {"name": "evens", "variables": n, "stocks": stocks(0)},
            {"name": "odds", "variables": n, "stocks": stocks(1)}
        ])
    );
    let flows: Vec<String> = (0..n).map(|i| format!("flow_{i}")).collect();
    assert_eq!(
        outline["otherNames"],
        json!(flows),
        "every other variable is named"
    );
    assert!(outline["note"].as_str().unwrap().contains("read_variables"));
    assert!(outline.get("omitted").is_none());
}

#[test]
fn a_sector_outline_over_the_budget_lists_the_stocks_that_fit_and_counts_the_rest() {
    let (full_sectors, full_len) = outline_with_budget(sectored(40), 1);
    // A budget below every outline still answers, with nothing listed.
    assert_eq!(
        full_sectors["omitted"],
        json!({"stocks": 40, "diagnostics": 0, "otherNames": 40})
    );
    assert!(full_sectors.get("stocks").is_none() && full_sectors.get("otherNames").is_none());
    assert!(full_len > 1);

    let (roomy, _) = outline_with_budget(sectored(40), usize::MAX);
    let roomy_len = serde_json::to_string(&roomy).unwrap().len();
    for budget in [full_len + 200, full_len + 800, roomy_len / 2] {
        let (outline, len) = outline_with_budget(sectored(40), budget);
        let listed = names(&outline["stocks"]).len();
        let omitted = outline["omitted"]["stocks"].as_u64().unwrap_or(0) as usize;
        assert_eq!(listed + omitted, 40, "budget {budget}");
        assert!(len <= budget, "budget {budget}: {len} bytes");
        assert!(listed > 0, "budget {budget} holds a stock");
        // The stocks listed are the model's first ones, in order, and names
        // are listed only once every stock is.
        let expected: Vec<String> = (0..listed).map(|i| format!("stock_{i}")).collect();
        assert_eq!(names(&outline["stocks"]), expected);
        if omitted > 0 {
            assert!(outline.get("otherNames").is_none(), "budget {budget}");
        }
    }
}

/// The corpus's two largest models are outlined within the budget, and a read
/// of their first stocks answers, each call timed.
///
/// Run with: scripts/gates.sh --nocapture the_largest_corpus_models_are_outlined_within_the_budget
#[test]
#[ignore = "compiles World3 and C-LEARN for their diagnostics; run under the gates profile"]
fn the_largest_corpus_models_are_outlined_within_the_budget() {
    for path in [
        "../../test/metasd/WRLD3-03/wrld3-03.mdl",
        "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
        // A small model whose diagnostics quote long equations.
        "../../test/metasd/FREE/FREE6/FREE6-original/energy_pos_loop.mdl",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
        let mdl = std::fs::read_to_string(&path).expect("the corpus model is present");
        let project = crate::compat::open_vensim(&mdl).expect("the corpus model parses");
        let mut host = Host::new(project.clone());
        let mut session = Session::new("main");

        let started = std::time::Instant::now();
        let output = host.call_raw(&mut session, "read_model", "{}");
        let outline_time = started.elapsed();
        assert!(!output.is_error, "{}", output.json);
        let outline: Value = serde_json::from_str(&output.json).unwrap();
        assert!(
            output.json.len() <= crate::tools::OUTLINE_BUDGET,
            "{}: {} bytes",
            path.display(),
            output.json.len()
        );

        // The twelve records with the most text: the largest answer an agent
        // can ask for.
        let mut largest: Vec<(usize, String)> = project.models[0]
            .variables
            .iter()
            .map(|v| {
                let text = v.get_equation().map(|e| e.source_text().len()).unwrap_or(0);
                (text, v.get_ident().to_string())
            })
            .collect();
        largest.sort_by_key(|b| std::cmp::Reverse(b.0));
        let names: Vec<String> = largest.into_iter().take(12).map(|(_, n)| n).collect();
        let started = std::time::Instant::now();
        let output = host.call_raw(
            &mut session,
            "read_variables",
            &json!({ "names": names }).to_string(),
        );
        let read_time = started.elapsed();
        let record: Value = serde_json::from_str(&output.json).unwrap();
        let listed = record["variables"].as_array().unwrap().len();

        let omitted = record["omitted"].as_array().map_or(0, Vec::len);
        assert_eq!(listed + omitted, names.len(), "{record}");
        assert!(
            listed == 1 || output.json.len() <= crate::tools::OUTLINE_BUDGET,
            "{}: read_variables {} bytes",
            path.display(),
            output.json.len()
        );
        eprintln!(
            "{}: outline {} bytes by sector: {}, {} stocks listed of {}, diagnostics omitted: {}, \
             other names omitted: {}, in {outline_time:?}; read_variables of the {} largest: \
             {} bytes, {listed} listed, in {read_time:?}",
            path.display(),
            outline_len(&outline),
            outline.get("sectors").is_some(),
            outline["stocks"].as_array().map_or(0, Vec::len),
            outline["counts"]["stocks"],
            outline["omitted"]["diagnostics"],
            outline["omitted"]["otherNames"],
            names.len(),
            output.json.len(),
        );
    }
}

fn outline_len(outline: &Value) -> usize {
    outline.to_string().len()
}

/// `NaN`, what an importer writes for an equation never filled in, is not a
/// constant: the outline lists it as a variable, with its diagnostic.
#[test]
fn an_unfilled_equation_is_no_constant() {
    let project = TestProject::new("p")
        .aux("unfilled", "NaN", None)
        .aux("infinite", "inf", None)
        .aux("x", "1", None)
        .build_datamodel();
    let outline = outline_of(project);
    assert_eq!(names(&outline["constants"]), ["x"]);
    assert_eq!(names(&outline["variables"]), ["unfilled", "infinite"]);
}

/// A model whose entries fit but whose diagnostics do not keeps every entry
/// and lists as many diagnostics as fit, errors first; each entry still names
/// its own diagnostics' ids.
#[test]
fn an_outline_over_its_budget_for_its_diagnostics_keeps_its_entries() {
    let mut project = inventory().build_datamodel();
    let model = &mut project.models[0];
    // Warnings first in the engine's report, then errors.
    for name in ["production", "shipments"] {
        model.get_variable_mut(name).unwrap().set_units("widget");
    }
    for name in ["adjustment_time", "coverage"] {
        model
            .get_variable_mut(name)
            .unwrap()
            .set_scalar_equation("no_such_variable");
    }
    let (whole, whole_len) = outline_with_budget(project.clone(), usize::MAX);
    let total = whole["diagnostics"].as_array().unwrap().len();
    assert!(total >= 3, "{whole}");
    let severities: Vec<&str> = whole["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["severity"].as_str().unwrap())
        .collect();
    let first_warning = severities.iter().position(|s| *s == "warning");
    assert!(
        first_warning.is_none_or(|i| severities[i..].iter().all(|s| *s == "warning")),
        "errors first: {severities:?}"
    );

    let one_less = whole["diagnostics"][total - 1].to_string().len();
    let (outline, len) = outline_with_budget(project, whole_len - one_less);
    assert!(len <= whole_len - one_less);
    assert!(
        outline.get("sectors").is_none() && outline["flows"].is_array(),
        "{outline}"
    );
    let listed = outline["diagnostics"].as_array().unwrap().len();
    assert!(listed < total);
    assert_eq!(outline["omitted"]["diagnostics"], total - listed);
    assert_eq!(outline["diagnostics"][0], whole["diagnostics"][0]);
    assert_eq!(
        outline["flows"][1]["diagnostics"],
        whole["flows"][1]["diagnostics"]
    );
    assert!(outline["note"].as_str().unwrap().contains("read_variables"));
}
