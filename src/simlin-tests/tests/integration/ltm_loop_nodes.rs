// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The node sequence a detected loop reports is its element circuit, each
//! node once. The fixtures are the arrays investigation's `bare_reducer`,
//! `variable_backed` and `scalar_cofactor` models: a per-element growth loop
//! closed through a variable-backed reducer (`total = SUM(pop)`), whose
//! mixed loops were reported as `growth -> pop[boston] -> total ->
//! growth[boston]` -- four nodes for a three-link loop, `growth` twice, once
//! bare -- because the same-element hop's `from` is stripped for link-score
//! name resolution.

use std::collections::BTreeSet;

use simlin_engine::db::{
    SimlinDb, model_detected_loops, set_project_ltm_discovery_mode, sync_from_datamodel_incremental,
};
use simlin_engine::json;

/// The investigation's corpus shape as Simlin JSON: `pop[Region]` (100, 200,
/// 300) with inflow `growth[Region] = pop * 0.02 * (1 - <reducer var> /
/// <cap>)` and the reducer auxiliaries `auxes` (JSON objects).
fn reducer_feedback(
    name: &str,
    reducer_var: &str,
    cap: &str,
    auxes: &str,
) -> simlin_engine::datamodel::Project {
    reducer_feedback_with_pins(name, reducer_var, cap, auxes, "")
}

/// [`reducer_feedback`] with `loopMetadata` entries (JSON objects). The
/// variables carry uids -- 1 for `pop`, 2 for `growth`, and whatever the
/// `auxes` objects declare -- so a pin can name them.
fn reducer_feedback_with_pins(
    name: &str,
    reducer_var: &str,
    cap: &str,
    auxes: &str,
    loop_metadata: &str,
) -> simlin_engine::datamodel::Project {
    let text = format!(
        r#"{{
  "name": "{name}",
  "simSpecs": {{"startTime": 0.0, "endTime": 20.0, "dt": "1", "method": "euler"}},
  "models": [{{
    "name": "main",
    "stocks": [{{"name": "pop", "uid": 1, "inflows": ["growth"], "outflows": [],
      "arrayedEquation": {{"dimensions": ["Region"], "elements": [
        {{"subscript": "nyc", "equation": "100"}}, {{"subscript": "boston", "equation": "200"}},
        {{"subscript": "la", "equation": "300"}}]}}}}],
    "flows": [{{"name": "growth", "uid": 2, "arrayedEquation": {{"dimensions": ["Region"],
      "equation": "pop * 0.02 * (1 - {reducer_var} / {cap})"}}}}],
    "auxiliaries": [{auxes}],
    "loopMetadata": [{loop_metadata}],
    "views": []
  }}],
  "dimensions": [{{"name": "Region", "elements": ["nyc", "boston", "la"]}}],
  "units": []
}}"#
    );
    let project: json::Project = serde_json::from_str(&text).expect("valid Simlin JSON");
    project.into()
}

/// Rotate a node sequence to start at its smallest node, so two spellings
/// of one cycle compare equal.
fn rotation(nodes: &[String]) -> Vec<String> {
    let Some(start) = (0..nodes.len()).min_by_key(|&i| &nodes[i]) else {
        return Vec::new();
    };
    nodes[start..]
        .iter()
        .chain(&nodes[..start])
        .cloned()
        .collect()
}

/// Every reported loop's node sequence, rotation-normalized, for `project`.
fn reported_cycles(project: &simlin_engine::datamodel::Project) -> BTreeSet<Vec<String>> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let source_model = sync.models["main"].source_model;
    let detected = model_detected_loops(&db, source_model, sync.project);
    detected
        .loops
        .iter()
        .map(|l| {
            let distinct: BTreeSet<&String> = l.variables.iter().collect();
            assert_eq!(
                distinct.len(),
                l.variables.len(),
                "{}: a loop visits each node once: {:?}",
                l.id,
                l.variables
            );
            rotation(&l.variables)
        })
        .collect()
}

fn cycle(nodes: &[&str]) -> Vec<String> {
    rotation(&nodes.iter().map(|n| n.to_string()).collect::<Vec<_>>())
}

/// The three per-element loops through the reducer are `growth[e] ->
/// pop[e] -> total`, three nodes each once, beside the A2A loop `pop ->
/// growth`, for a bare argument (`SUM(pop)`) and a starred one
/// (`SUM(pop[*])`).
#[test]
fn mixed_loops_through_a_variable_backed_reducer_report_the_element_circuit() {
    let expected: BTreeSet<Vec<String>> = [
        cycle(&["pop", "growth"]),
        cycle(&["growth[nyc]", "pop[nyc]", "total"]),
        cycle(&["growth[boston]", "pop[boston]", "total"]),
        cycle(&["growth[la]", "pop[la]", "total"]),
    ]
    .into_iter()
    .collect();
    for (name, reducer) in [
        ("bare_reducer", "SUM(pop)"),
        ("variable_backed", "SUM(pop[*])"),
    ] {
        let auxes = format!(r#"{{"name": "total", "uid": 3, "equation": "{reducer}"}}"#);
        let cycles = reported_cycles(&reducer_feedback(name, "total", "1000", &auxes));
        assert_eq!(cycles, expected, "{name}");
    }
}

/// The scalar-cofactor model reads `pop[nyc]` inside the reducer's scale
/// (`weighted = SUM(pop[*] * scale)`, `scale = 1 + pop[nyc] / 10000`), so
/// `pop[nyc]` also closes loops through `scale`; every reported sequence
/// still visits each node once and the reducer loops are the three-node
/// circuits.
#[test]
fn scalar_cofactor_reducer_loops_report_each_node_once() {
    let auxes = r#"{"name": "weighted", "uid": 3, "equation": "SUM(pop[*] * scale)"},
      {"name": "scale", "uid": 4, "equation": "1 + pop[nyc] / 10000"}"#;
    let cycles = reported_cycles(&reducer_feedback(
        "scalar_cofactor",
        "weighted",
        "5000",
        auxes,
    ));
    for region in ["nyc", "boston", "la"] {
        let through_reducer = cycle(&[
            &format!("growth[{region}]"),
            &format!("pop[{region}]"),
            "weighted",
        ]);
        assert!(
            cycles.contains(&through_reducer),
            "{region}: the reducer loop is its element circuit; got {cycles:?}"
        );
    }
    assert!(cycles.contains(&cycle(&["pop", "growth"])), "{cycles:?}");
}

/// The pin path, in discovery mode: a pin naming `{growth, pop, total}` is
/// expanded on the element graph into the three mixed circuits, each
/// reported through `detected_loop_from_loop` -- the same owner -- as
/// `growth[e] -> pop[e] -> total`, and in discovery mode those are the only
/// loops the structural surface reports.
#[test]
fn a_pin_through_a_reducer_reports_the_element_circuits_in_discovery_mode() {
    let project = reducer_feedback_with_pins(
        "bare_reducer_pinned",
        "total",
        "1000",
        r#"{"name": "total", "uid": 3, "equation": "SUM(pop)"}"#,
        r#"{"uids": [1, 2, 3], "name": "Reducer feedback"}"#,
    );
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let detected = model_detected_loops(&db, source_model, sync.project);
    let mut ids: Vec<&str> = detected.loops.iter().map(|l| l.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        ["pin1\u{205A}1", "pin1\u{205A}2", "pin1\u{205A}3"],
        "the pin's three element-level instances are the reported loops"
    );
    let cycles: BTreeSet<Vec<String>> = detected
        .loops
        .iter()
        .map(|l| {
            let distinct: BTreeSet<&String> = l.variables.iter().collect();
            assert_eq!(
                distinct.len(),
                l.variables.len(),
                "{}: {:?}",
                l.id,
                l.variables
            );
            rotation(&l.variables)
        })
        .collect();
    let expected: BTreeSet<Vec<String>> = [
        cycle(&["growth[nyc]", "pop[nyc]", "total"]),
        cycle(&["growth[boston]", "pop[boston]", "total"]),
        cycle(&["growth[la]", "pop[la]", "total"]),
    ]
    .into_iter()
    .collect();
    assert_eq!(cycles, expected);
}
