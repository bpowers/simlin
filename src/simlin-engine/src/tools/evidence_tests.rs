// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::datamodel::Project;
use crate::tools::Session;
use crate::tools::test_support::{Host, inventory};

fn set_equation(project: &mut Project, name: &str, equation: &str) {
    project.models[0]
        .get_variable_mut(name)
        .unwrap_or_else(|| panic!("{name} exists"))
        .set_scalar_equation(equation);
}

fn diagnostics(host: &mut Host, session: &mut Session) -> Vec<Value> {
    let outline = host.call(session, "read_model", json!({}));
    outline["diagnostics"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn on<'a>(diagnostics: &'a [Value], variable: &str) -> &'a Value {
    diagnostics
        .iter()
        .find(|d| d["variable"] == variable)
        .unwrap_or_else(|| panic!("a diagnostic on {variable}: {diagnostics:?}"))
}

#[test]
fn a_parse_error_is_reported_with_the_text_it_points_at() {
    let mut project = inventory().build_datamodel();
    set_equation(&mut project, "shipments", "orders * * 2");
    let mut host = Host::new(project);
    let reported = diagnostics(&mut host, &mut Session::new("main"));
    let d = on(&reported, "shipments");
    assert_eq!(d["severity"], "error");
    assert_eq!(d["category"], "equation");
    assert_eq!(d["reason"], "at `*` in `orders * * 2`", "{d}");

    // An error at the end of the text has no span to quote, so the reason
    // quotes the text it is at the end of.
    let mut project = inventory().build_datamodel();
    set_equation(&mut project, "shipments", "orders +");
    let mut host = Host::new(project);
    let reported = diagnostics(&mut host, &mut Session::new("main"));
    let d = on(&reported, "shipments");
    assert_eq!(d["code"], "unrecognized_eof");
    assert_eq!(d["reason"], "in `orders +`");
}

#[test]
fn a_reason_the_engine_wrote_is_reported_with_where_it_points() {
    let mut project = inventory().build_datamodel();
    set_equation(&mut project, "shipments", "ordrs");
    let mut host = Host::new(project);
    let reported = diagnostics(&mut host, &mut Session::new("main"));
    let d = on(&reported, "shipments");
    assert_eq!(d["code"], "unknown_dependency");
    assert_eq!(
        d["reason"],
        "'ordrs' is not a variable of model 'main', at `ordrs` in `ordrs`"
    );
}

#[test]
fn a_model_level_problem_gives_its_reason_and_quotes_no_equation() {
    let mut project = inventory().build_datamodel();
    // orders -> desired_inventory -> orders, with no stock between.
    set_equation(&mut project, "orders", "desired_inventory");
    let mut host = Host::new(project);
    let reported = diagnostics(&mut host, &mut Session::new("main"));
    let cycle = reported
        .iter()
        .find(|d| d["code"] == "circular_dependency")
        .expect("the cycle is reported");
    assert_eq!(cycle["category"], "model");
    let reason = cycle["reason"].as_str().unwrap();
    assert!(reason.contains("depends on itself"), "{cycle}");
    assert!(!reason.contains('`'), "no equation is quoted: {cycle}");
}

#[test]
fn a_diagnostic_keeps_its_id_while_its_problem_exists_and_no_other_problem_takes_it() {
    let mut project = inventory().build_datamodel();
    set_equation(&mut project, "shipments", "ordrs");
    set_equation(&mut project, "production", "orders +");
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    let first = diagnostics(&mut host, &mut session);
    let shipments_id = on(&first, "shipments")["id"].clone();
    let production_id = on(&first, "production")["id"].clone();
    assert_ne!(shipments_id, production_id);
    let ids: Vec<&Value> = first.iter().map(|d| &d["id"]).collect();
    assert_eq!(
        ids,
        [&json!("D1"), &json!("D2")],
        "numbered in report order"
    );

    // Fix shipments, break adjustment_time, and edit production's equation
    // without fixing it: its problem, and so its id, stays.
    host.edit(|p| {
        set_equation(p, "shipments", "orders");
        set_equation(p, "adjustment_time", "2 +");
        set_equation(p, "production", "orders  +");
    });
    let second = diagnostics(&mut host, &mut session);
    assert!(second.iter().all(|d| d["variable"] != "shipments"));
    assert_eq!(on(&second, "production")["id"], production_id);
    assert_eq!(on(&second, "adjustment_time")["id"], "D3");

    // A problem that goes away and comes back is the same problem, under its
    // old id; D3 above shows a different problem never takes a freed one.
    host.edit(|p| set_equation(p, "shipments", "ordrs"));
    let third = diagnostics(&mut host, &mut session);
    assert_eq!(on(&third, "shipments")["id"], shipments_id);
}

#[test]
fn a_new_session_numbers_from_one() {
    let mut project = inventory().build_datamodel();
    set_equation(&mut project, "production", "orders +");
    let mut host = Host::new(project);
    for _ in 0..2 {
        let reported = diagnostics(&mut host, &mut Session::new("main"));
        assert_eq!(on(&reported, "production")["id"], "D1");
    }
}

/// Rows alike in variable, severity, code and reason are told apart by their
/// order. No model this suite builds produces two such rows (the engine
/// reports one row per failing variable and code), so the arm is pinned on the
/// numbering itself.
#[test]
fn rows_alike_but_for_their_order_get_ids_of_their_own() {
    let mut evidence = Evidence::default();
    let key = |occurrence| DiagnosticKey {
        variable: Some("x".to_string()),
        severity: Severity::Error,
        code: ErrorCode::UnknownDependency,
        reason: None,
        occurrence,
    };
    assert_eq!(evidence.diagnostic_id(key(0)), "D1");
    assert_eq!(evidence.diagnostic_id(key(1)), "D2");
    assert_eq!(evidence.diagnostic_id(key(0)), "D1");
}

#[test]
fn a_project_level_problem_is_every_models_and_another_models_is_not() {
    let mut project = inventory().build_datamodel();
    project.units.push(datamodel::Unit {
        name: "gizmo".to_string(),
        equation: Some("widget/".to_string()),
        disabled: false,
        aliases: vec![],
    });
    let mut other = project.models[0].clone();
    other.name = "other".to_string();
    other
        .get_variable_mut("shipments")
        .unwrap()
        .set_scalar_equation("ordrs");
    project.models.push(other);
    let mut host = Host::new(project);

    let main = diagnostics(&mut host, &mut Session::new("main"));
    assert_eq!(on(&main, "gizmo")["category"], "unit_definition");
    assert!(
        main.iter().all(|d| d["code"] != "unknown_dependency"),
        "another model's problem is not main's: {main:?}"
    );

    let other = diagnostics(&mut host, &mut Session::new("other"));
    assert_eq!(on(&other, "shipments")["code"], "unknown_dependency");
    assert_eq!(on(&other, "gizmo")["category"], "unit_definition");
}

#[test]
fn unit_problems_are_warnings_filed_under_the_variable_or_under_none() {
    let mut project = inventory().build_datamodel();
    set_equation(&mut project, "shipments", "coverage");
    let mut host = Host::new(project);
    let reported = diagnostics(&mut host, &mut Session::new("main"));
    let consistency = on(&reported, "shipments");
    assert_eq!(consistency["severity"], "warning");
    assert_eq!(consistency["category"], "unit_consistency");
    let inference = reported
        .iter()
        .find(|d| d["category"] == "unit_inference")
        .expect("the model-wide contradiction is reported");
    assert!(inference.get("variable").is_none(), "{inference}");
}

#[test]
fn every_engine_category_has_a_name_of_its_own() {
    // The rows are `DiagnosticCategory`'s variants; the exhaustive match in
    // `From` is what makes a new variant a compile error here.
    let names: std::collections::HashSet<String> = [
        DiagnosticCategory::Equation,
        DiagnosticCategory::Model,
        DiagnosticCategory::UnitDefinition,
        DiagnosticCategory::UnitConsistency,
        DiagnosticCategory::UnitInference,
        DiagnosticCategory::Assembly,
    ]
    .into_iter()
    .map(|c| serde_json::to_string(&DiagnosticCategoryName::from(c)).unwrap())
    .collect();
    assert_eq!(names.len(), 6);
}

#[test]
fn a_name_the_engine_reports_is_shown_as_the_model_spells_it() {
    let model = inventory().build_datamodel().models.remove(0);
    assert_eq!(display_name(&model, "inventory"), "Inventory");
    assert_eq!(display_name(&model, "not_here"), "not_here");
}

/// A long equation is quoted around the text a parse error points at, not
/// whole: a quote says where.
#[test]
fn a_parse_error_in_a_long_equation_quotes_a_window_around_it() {
    let long = format!(
        "{} + orders * * 2 + {}",
        "orders".repeat(80),
        "orders".repeat(80)
    );
    let mut project = inventory().build_datamodel();
    set_equation(&mut project, "shipments", &long);
    let mut host = Host::new(project);
    let reported = diagnostics(&mut host, &mut Session::new("main"));
    let reason = on(&reported, "shipments")["reason"]
        .as_str()
        .unwrap()
        .to_string();
    let quote = reason.split("in `").nth(1).unwrap().trim_end_matches('`');
    assert!(quote.chars().count() <= QUOTE_CHARS + 2, "{reason}");
    assert!(quote.starts_with('…') && quote.ends_with('…'), "{reason}");
    assert!(
        quote.contains("* * 2"),
        "the window holds what it points at: {reason}"
    );
}

#[test]
fn a_window_is_the_text_when_it_fits_and_marks_both_cuts_otherwise() {
    assert_eq!(window("short", 0, 0, 10), "short");
    let text: String = ('a'..='z').collect();
    assert_eq!(window(&text, 0, 0, 5), "abcde…");
    assert_eq!(window(&text, 25, 26, 5), "…vwxyz");
    assert_eq!(window(&text, 12, 13, 5), "…klmno…");
    let multibyte = "é".repeat(20);
    assert_eq!(window(&multibyte, 0, 2, 3).chars().count(), 4);
}

/// With neither a reason of the raising site's own nor a span to quote, a
/// report says what the code means.
#[test]
fn a_diagnostic_with_neither_reason_nor_span_says_what_its_code_means() {
    let model = inventory().build_datamodel().models[0].clone();
    let diagnostic = crate::db::Diagnostic {
        model: "main".to_string(),
        variable: None,
        owner: None,
        severity: crate::db::DiagnosticSeverity::Error,
        error: crate::db::DiagnosticError::Model(crate::common::Error {
            kind: crate::common::ErrorKind::Model,
            code: ErrorCode::CircularDependency,
            details: None,
        }),
    };
    let formatted = format_diagnostic_with_datamodel(&diagnostic, &inventory().build_datamodel());
    assert_eq!(
        reason_given(&diagnostic, &formatted, &model).as_deref(),
        Some(ErrorCode::CircularDependency.description())
    );
}
