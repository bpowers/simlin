// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A per-element subscript names the same element however it is spelled, and
//! a subscript naming nothing is reported: the one key owner
//! (`CanonicalElementName::from_subscript`) applied at the JSON boundary and
//! in every consumer.  The fixture is the arrays investigation's `sliced_sum`
//! model, whose spaced spelling ("nyc, young") once simulated every element
//! as 0 with no diagnostic.
//!
//! Two advisories, both Warnings: an arm whose subscript names nothing, or
//! names an element outside the variable's dimensions, is an unused arm
//! (`UnknownElementSubscript`); a declared element left with no arm and no
//! applicable default evaluates to the compiler's fabricated zero, and
//! `MissingElementEquation` names it. The zero stays a Warning because
//! Vensim defines subscripted variables on part of their range as a matter
//! of course (the elements do not exist there; see the sdeverywhere `except`
//! corpus), so an Error would refuse real models.

use std::collections::HashMap;

use simlin_engine::common::ErrorCode;
use simlin_engine::db::{
    DiagnosticError, LtmOverlay, SimlinDb, collect_all_diagnostics, compile_project_incremental,
    sync_from_datamodel_incremental,
};
use simlin_engine::{Results, Vm, json};

/// The `sliced_sum` model as Simlin JSON, with the stock's six per-element
/// subscripts spelled by `subscript`.
fn sliced_sum_json(subscript: impl Fn(&str, &str) -> String) -> String {
    let elements: Vec<String> = [
        ("nyc", "young", "100"),
        ("nyc", "old", "50"),
        ("boston", "young", "200"),
        ("boston", "old", "80"),
        ("la", "young", "60"),
        ("la", "old", "40"),
    ]
    .iter()
    .map(|(region, age, init)| {
        format!(
            r#"{{"subscript": "{}", "equation": "{init}"}}"#,
            subscript(region, age)
        )
    })
    .collect();
    format!(
        r#"{{
  "name": "sliced_sum",
  "simSpecs": {{"startTime": 0.0, "endTime": 20.0, "dt": "1", "method": "euler"}},
  "models": [{{
    "name": "main",
    "stocks": [{{"name": "pop", "inflows": ["growth"], "outflows": [],
      "arrayedEquation": {{"dimensions": ["Region", "Age"], "elements": [{}]}}}}],
    "flows": [{{"name": "growth", "arrayedEquation": {{"dimensions": ["Region", "Age"],
      "equation": "pop * rate * (1 - region_total / 2000)"}}}}],
    "auxiliaries": [
      {{"name": "region_total", "arrayedEquation": {{"dimensions": ["Region"], "equation": "SUM(pop[Region, *])"}}}},
      {{"name": "rate", "arrayedEquation": {{"dimensions": ["Age"], "elements": [
        {{"subscript": "young", "equation": "0.08"}}, {{"subscript": "old", "equation": "0.02"}}]}}}}
    ],
    "views": []
  }}],
  "dimensions": [
    {{"name": "Region", "elements": ["nyc", "boston", "la"]}},
    {{"name": "Age", "elements": ["young", "old"]}}
  ],
  "units": []
}}"#,
        elements.join(", ")
    )
}

fn load(json_text: &str) -> simlin_engine::datamodel::Project {
    let project: json::Project = serde_json::from_str(json_text).expect("valid Simlin JSON");
    project.into()
}

/// Simulate `project` (no LTM) and return every saved series by name.
fn simulate(project: &simlin_engine::datamodel::Project) -> (Results, HashMap<String, Vec<f64>>) {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off)
        .expect("the model compiles");
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().expect("the model simulates");
    let results = vm.into_results();
    let series: HashMap<String, Vec<f64>> = results
        .offsets
        .iter()
        .map(|(name, &off)| {
            (
                name.as_str().to_string(),
                results.iter().map(|row| row[off]).collect(),
            )
        })
        .collect();
    (results, series)
}

/// `"nyc, young"` and `"NYC,Young"` are the element `nyc,young`: the spaced
/// and capitalized spellings simulate bit-for-bit like the canonical one, and
/// the datamodel stores the canonical spelling for all three.
#[test]
fn spaced_and_capitalized_subscripts_simulate_like_the_canonical_spelling() {
    let canonical = load(&sliced_sum_json(|r, a| format!("{r},{a}")));
    let spaced = load(&sliced_sum_json(|r, a| format!("{r}, {a}")));
    let capitalized = load(&sliced_sum_json(|r, a| {
        let cap = |s: &str| {
            let mut c = s.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        };
        format!(" {} , {} ", cap(r), cap(a))
    }));

    let stored_subscripts = |p: &simlin_engine::datamodel::Project| -> Vec<String> {
        let stock = p.models[0]
            .variables
            .iter()
            .find(|v| v.get_ident() == "pop")
            .expect("pop");
        match stock {
            simlin_engine::datamodel::Variable::Stock(s) => match &s.equation {
                simlin_engine::datamodel::Equation::Arrayed(_, elements, _, _) => {
                    elements.iter().map(|(sub, _, _, _)| sub.clone()).collect()
                }
                other => panic!("pop is arrayed: {other:?}"),
            },
            other => panic!("pop is a stock: {other:?}"),
        }
    };
    let expected = [
        "nyc,young",
        "nyc,old",
        "boston,young",
        "boston,old",
        "la,young",
        "la,old",
    ];
    for project in [&canonical, &spaced, &capitalized] {
        assert_eq!(stored_subscripts(project), expected);
    }

    let (results, canonical_series) = simulate(&canonical);
    assert!(results.step_count > 1);
    let final_pop: f64 = canonical_series["pop[nyc,young]"][results.step_count - 1];
    assert!(final_pop > 100.0, "the fixture grows: {final_pop}");
    for (label, project) in [("spaced", &spaced), ("capitalized", &capitalized)] {
        let (_, series) = simulate(project);
        assert_eq!(series.len(), canonical_series.len(), "{label}");
        for (name, values) in &canonical_series {
            assert_eq!(&series[name], values, "{label}: series {name} differs");
        }
    }
}

/// A subscript naming no element combination of the variable's dimensions is
/// reported on that variable, naming the subscript, and is never a silent
/// default: the diagnostic is the `UnknownElementSubscript` advisory, which
/// matches by the same key the compiler expands with.
#[test]
fn a_subscript_naming_no_element_is_reported_on_the_variable() {
    let project = load(&sliced_sum_json(|r, a| {
        if r == "nyc" && a == "young" {
            "nyc, yung".to_string()
        } else {
            format!("{r},{a}")
        }
    }));
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
    let unknown: Vec<(String, String)> = diagnostics
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Model(e) if e.code == ErrorCode::UnknownElementSubscript => Some((
                d.variable.clone().unwrap_or_default(),
                e.get_details().unwrap_or_default().to_string(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        unknown.len(),
        1,
        "one unknown-subscript diagnostic: {diagnostics:?}"
    );
    let (variable, message) = &unknown[0];
    assert_eq!(variable, "pop");
    assert!(
        message.contains("nyc,yung"),
        "the diagnostic names the subscript: {message}"
    );
    // The typo also leaves the declared element `nyc,young` with no arm:
    // that is the zero consequence, and the sibling advisory names it.
    let missing = missing_element_findings(&diagnostics);
    assert_eq!(
        missing,
        vec![("pop".to_string(), "'nyc,young'".to_string())],
        "{diagnostics:?}"
    );
}

/// Every `MissingElementEquation` advisory, as `(variable, the quoted
/// element list the message names)`.
fn missing_element_findings(
    diagnostics: &[simlin_engine::db::Diagnostic],
) -> Vec<(String, String)> {
    diagnostics
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Model(e) if e.code == ErrorCode::MissingElementEquation => {
                assert_eq!(d.severity, simlin_engine::db::DiagnosticSeverity::Warning);
                let details = e.get_details().unwrap_or_default().to_string();
                let elements = details
                    .split(" has no equation for ")
                    .nth(1)
                    .and_then(|rest| rest.split(": no element entry").next())
                    .unwrap_or_else(|| panic!("the warning names the elements: {details}"))
                    .to_string();
                Some((d.variable.clone().unwrap_or_default(), elements))
            }
            _ => None,
        })
        .collect()
}

/// `sliced_sum` with `pop`'s `la,old` arm dropped and no default: the
/// declared element has no equation, so `pop` carries a Warning naming it,
/// and the element simulates as the fabricated 0 the warning announces --
/// a loud zero, never a silent one.
#[test]
fn a_declared_element_with_no_arm_is_a_warning_naming_it() {
    let text = sliced_sum_json(|r, a| format!("{r},{a}"))
        .replace(r#", {"subscript": "la,old", "equation": "40"}"#, "");
    assert!(!text.contains("la,old"), "the arm is dropped");
    let project = load(&text);
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
    assert_eq!(
        missing_element_findings(&diagnostics),
        vec![("pop".to_string(), "'la,old'".to_string())],
        "{diagnostics:?}"
    );
    let (_, series) = simulate(&project);
    assert!(
        series["pop[la,old]"].iter().all(|v| *v == 0.0),
        "the armless element is the fabricated 0 the warning names"
    );
    assert!(
        series["pop[la,young]"][0] == 60.0,
        "its siblings are untouched"
    );
}

/// The same dropped arm under an EXCEPT default: the default applies to
/// the missing element, so there is nothing to report and it simulates.
#[test]
fn an_except_default_covers_a_missing_element() {
    let text = sliced_sum_json(|r, a| format!("{r},{a}"))
        .replace(r#", {"subscript": "la,old", "equation": "40"}"#, "")
        .replace(
            r#""arrayedEquation": {"dimensions": ["Region", "Age"], "elements": ["#,
            r#""arrayedEquation": {"dimensions": ["Region", "Age"], "equation": "40", "hasExceptDefault": true, "elements": ["#,
        );
    assert!(
        text.contains("hasExceptDefault"),
        "the default is spliced in"
    );
    let project = load(&text);
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
    assert!(
        missing_element_findings(&diagnostics).is_empty(),
        "{diagnostics:?}"
    );
    let (results, series) = simulate(&project);
    assert_eq!(series["pop[la,old]"][0], 40.0);
    assert!(series["pop[la,old]"][results.step_count - 1] > 40.0);
}

/// An entry whose equation is empty is not an arm: the compiler drops it and
/// the element takes the fabricated 0, so the warning names it exactly as it
/// names an absent entry.
#[test]
fn an_entry_with_an_empty_equation_is_not_an_arm() {
    let text = sliced_sum_json(|r, a| format!("{r},{a}")).replace(
        r#"{"subscript": "young", "equation": "0.08"}"#,
        r#"{"subscript": "young", "equation": ""}"#,
    );
    assert!(text.contains(r#""equation": """#), "the arm is emptied");
    let project = load(&text);
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
    assert_eq!(
        missing_element_findings(&diagnostics),
        vec![("rate".to_string(), "'young'".to_string())],
        "{diagnostics:?}"
    );
    let (_, series) = simulate(&project);
    assert!(series["rate[young]"].iter().all(|v| *v == 0.0));
    assert!(series["rate[old]"].iter().all(|v| *v == 0.02));
}

/// An EXCEPT default that is the empty string parses to nothing and covers
/// nothing: the dropped `la,old` arm is reported as if there were no default.
#[test]
fn an_empty_except_default_covers_nothing() {
    let text = sliced_sum_json(|r, a| format!("{r},{a}"))
        .replace(r#", {"subscript": "la,old", "equation": "40"}"#, "")
        .replace(
            r#""arrayedEquation": {"dimensions": ["Region", "Age"], "elements": ["#,
            r#""arrayedEquation": {"dimensions": ["Region", "Age"], "equation": "", "hasExceptDefault": true, "elements": ["#,
        );
    assert!(text.contains(r#""equation": "", "hasExceptDefault": true"#));
    let project = load(&text);
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
    assert_eq!(
        missing_element_findings(&diagnostics),
        vec![("pop".to_string(), "'la,old'".to_string())],
        "{diagnostics:?}"
    );
    let (_, series) = simulate(&project);
    assert!(series["pop[la,old]"].iter().all(|v| *v == 0.0));
}

/// The XMILE spelling of the same shape: `<element subscript="b"/>` with no
/// `<eqn>` and no `<gf>` is an entry that names `b` and gives it nothing, so
/// `v[b]` is the fabricated 0, its reader reads 0, and the warning names `b`.
#[test]
fn an_xmile_element_with_neither_equation_nor_gf_is_reported() {
    let xmile = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
  <dimensions><dim name="D"><elem name="a"/><elem name="b"/></dim></dimensions>
  <model><variables>
    <aux name="v">
      <element subscript="a"><eqn>1</eqn></element>
      <element subscript="b"/>
      <dimensions><dim name="D"/></dimensions>
    </aux>
    <aux name="reads_v"><eqn>v[b]</eqn></aux>
  </variables></model>
</xmile>"#;
    let project = simlin_engine::compat::open_xmile(&mut xmile.as_bytes()).expect("XMILE parses");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
    assert_eq!(
        missing_element_findings(&diagnostics),
        vec![("v".to_string(), "'b'".to_string())],
        "{diagnostics:?}"
    );
    let (_, series) = simulate(&project);
    assert!(series["v[a]"].iter().all(|v| *v == 1.0));
    assert!(series["v[b]"].iter().all(|v| *v == 0.0));
    assert!(series["reads_v"].iter().all(|v| *v == 0.0));
}

/// An arm naming an element that exists in the project but not in the
/// variable's own dimensions (`rate` is over `Age`; the arm names the
/// `Region` element `nyc`) is an unused arm: a Warning naming it, no
/// missing-element finding, and the model simulates -- every element of
/// `Age` has its equation.
#[test]
fn an_arm_for_an_element_of_another_dimension_is_only_a_warning() {
    let text = sliced_sum_json(|r, a| format!("{r},{a}")).replace(
        r#"{"subscript": "old", "equation": "0.02"}"#,
        r#"{"subscript": "old", "equation": "0.02"}, {"subscript": "nyc", "equation": "0.5"}"#,
    );
    assert!(
        text.contains(r#""subscript": "nyc""#),
        "the extra arm is spliced in"
    );
    let project = load(&text);
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, sync.project, LtmOverlay::Off);
    assert!(
        missing_element_findings(&diagnostics).is_empty(),
        "{diagnostics:?}"
    );
    let unused: Vec<(String, String)> = diagnostics
        .iter()
        .filter_map(|d| match &d.error {
            DiagnosticError::Model(e) if e.code == ErrorCode::UnknownElementSubscript => {
                assert_eq!(d.severity, simlin_engine::db::DiagnosticSeverity::Warning);
                Some((
                    d.variable.clone().unwrap_or_default(),
                    e.get_details().unwrap_or_default().to_string(),
                ))
            }
            _ => None,
        })
        .collect();
    assert_eq!(unused.len(), 1, "{diagnostics:?}");
    assert_eq!(unused[0].0, "rate");
    assert!(
        unused[0].1.contains("'nyc'"),
        "names the arm: {}",
        unused[0].1
    );
    let (_, series) = simulate(&project);
    assert!(series["rate[young]"][0] == 0.08 && series["rate[old]"][0] == 0.02);
}
