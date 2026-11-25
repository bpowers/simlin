// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The datamodel's `==` compares floats bit for bit, and a variable's
//! expression texts and names are each listed once for readers and writers
//! alike.

use super::view_element::{FlowPoint, LinkShape};
use super::*;

/// `make(x)` equals itself for every float, a NaN included; differs for two
/// floats that compare equal but are not the same bits; and differs for two
/// different numbers.
fn assert_compares_bits<T: Eq>(what: &str, make: impl Fn(f64) -> T) {
    for x in [0.0, -0.0, 1.5, f64::NAN, f64::INFINITY] {
        assert!(make(x) == make(x), "{what}: {x} equals itself");
    }
    assert!(make(0.0) != make(-0.0), "{what}: 0.0 is not -0.0");
    assert!(make(1.0) != make(2.0), "{what}: 1 is not 2");
    assert!(make(f64::NAN) != make(1.0), "{what}: a NaN is not 1");
}

fn table(x_points: Option<Vec<f64>>, y_points: Vec<f64>, max: f64) -> GraphicalFunction {
    GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points,
        y_points,
        x_scale: GraphicalFunctionScale { min: 0.0, max },
        y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
    }
}

#[test]
fn a_float_in_a_list_an_option_or_a_field_is_compared_by_its_bits() {
    assert_compares_bits("a float field", |x| table(None, vec![1.0], x));
    assert_compares_bits("a list of floats", |x| table(None, vec![0.5, x], 1.0));
    assert_compares_bits("an optional list of floats", |x| {
        table(Some(vec![x, 2.0]), vec![1.0, 2.0], 1.0)
    });
    assert!(
        table(None, vec![1.0], 1.0) != table(Some(vec![1.0]), vec![1.0], 1.0),
        "a list that is there is not one that is not"
    );
    assert!(
        table(None, vec![1.0], 1.0) != table(None, vec![1.0, 1.0], 1.0),
        "lists of different lengths differ"
    );
}

/// One of each `LinkShape` variant holding `x`. The match has no wildcard,
/// so a variant added to the enum fails to compile until it has a row.
fn link_shapes(x: f64) -> Vec<LinkShape> {
    let all = vec![
        LinkShape::Straight,
        LinkShape::Arc(x),
        LinkShape::MultiPoint(vec![FlowPoint {
            x,
            y: 0.0,
            attached_to_uid: None,
        }]),
    ];
    for shape in &all {
        match shape {
            LinkShape::Straight | LinkShape::Arc(_) | LinkShape::MultiPoint(_) => {}
        }
    }
    all
}

#[test]
fn an_enum_holding_a_float_is_compared_by_its_bits_variant_by_variant() {
    for (i, a) in link_shapes(1.0).iter().enumerate() {
        for (j, b) in link_shapes(1.0).iter().enumerate() {
            assert_eq!(a == b, i == j, "link shapes {i} and {j}");
        }
    }
    assert_compares_bits("an arc's angle", LinkShape::Arc);
    assert_compares_bits("a multi-point link's point", |x| {
        link_shapes(x)
            .pop()
            .unwrap_or_else(|| unreachable!("the list is not empty"))
    });

    assert_compares_bits("a dt", Dt::Dt);
    assert_compares_bits("a reciprocal dt", Dt::Reciprocal);
    assert!(Dt::Dt(4.0) != Dt::Reciprocal(4.0));
}

#[test]
fn a_variable_holding_a_nan_equals_itself() {
    let variable = Variable::Aux(Aux {
        ident: "lookup".to_string(),
        equation: Equation::Scalar("TIME".to_string()),
        documentation: String::new(),
        units: None,
        gf: Some(table(None, vec![0.0, f64::NAN, 1.0], 2.0)),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    assert!(variable == variable.clone());
}

#[test]
fn a_variables_expression_texts_are_the_same_to_read_and_to_write() {
    let variable = Variable::Flow(Flow {
        ident: "flow".to_string(),
        equation: Equation::Arrayed(
            vec!["d".to_string()],
            vec![
                (
                    "a".to_string(),
                    "e1".to_string(),
                    Some("i1".to_string()),
                    None,
                ),
                ("b".to_string(), "e2".to_string(), None, None),
            ],
            Some("default".to_string()),
            true,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat {
            active_initial: Some("active".to_string()),
            leakage: Some(Leakage {
                fraction: Some("fraction".to_string()),
                integers: false,
                zone_start: None,
                zone_end: Some("end".to_string()),
            }),
            spreadflow: Some(SpreadFlow::Dist("dist".to_string())),
            ..Compat::default()
        },
    });
    use ExpressionRole::{Equation as Eqn, Initial, Option as Opt};
    use StockOption::{LeakFraction, LeakZoneEnd};
    let read = variable.expression_texts();
    assert_eq!(
        read,
        [
            (Eqn, "e1"),
            (Initial, "i1"),
            (Eqn, "e2"),
            (Eqn, "default"),
            (Initial, "active"),
            (Opt(LeakFraction), "fraction"),
            (Opt(LeakZoneEnd), "end"),
        ],
        "a spread flow's distribution is a name, not an expression"
    );
    let mut copy = variable.clone();
    let written: Vec<(ExpressionRole, String)> = copy
        .expression_texts_mut()
        .into_iter()
        .map(|(role, text)| (role, text.clone()))
        .collect();
    assert_eq!(
        written,
        read.iter()
            .map(|(role, text)| (*role, text.to_string()))
            .collect::<Vec<_>>()
    );

    assert!(
        variable.map_expression_texts(|_, _| None).is_none(),
        "a map that replaces nothing gives no copy"
    );
    let mapped = variable
        .map_expression_texts(|role, text| (role == Initial).then(|| text.to_uppercase()))
        .expect("the map replaces the initials");
    let texts: Vec<&str> = mapped
        .expression_texts()
        .into_iter()
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        texts,
        ["e1", "I1", "e2", "default", "ACTIVE", "fraction", "end"]
    );
}

#[test]
fn a_variables_names_are_the_same_to_read_and_to_write() {
    use NameRole::{Distribution, Ident, Inflow, ModuleDestination, ModuleSource, Outflow};
    let variables = [
        (
            Variable::Stock(Stock {
                ident: "level".to_string(),
                equation: Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["in a".to_string(), "in b".to_string()],
                outflows: vec!["out".to_string()],
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            }),
            vec![
                (Ident, "level"),
                (Inflow, "in a"),
                (Inflow, "in b"),
                (Outflow, "out"),
            ],
        ),
        (
            Variable::Flow(Flow {
                ident: "entering".to_string(),
                equation: Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: Compat {
                    spreadflow: Some(SpreadFlow::Dist("profile".to_string())),
                    ..Compat::default()
                },
            }),
            vec![(Ident, "entering"), (Distribution, "profile")],
        ),
        (
            Variable::Aux(Aux {
                ident: "plain".to_string(),
                equation: Equation::Scalar("other * 2".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            }),
            vec![(Ident, "plain")],
        ),
        (
            Variable::Module(Module {
                ident: "instance".to_string(),
                model_name: "sub".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![
                    ModuleReference {
                        src: "a".to_string(),
                        dst: "instance.x".to_string(),
                    },
                    ModuleReference {
                        src: "b".to_string(),
                        dst: "instance.y".to_string(),
                    },
                ],
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            }),
            vec![
                (Ident, "instance"),
                (ModuleSource, "a"),
                (ModuleDestination, "instance.x"),
                (ModuleSource, "b"),
                (ModuleDestination, "instance.y"),
            ],
        ),
    ];
    for (variable, names) in variables {
        assert_eq!(variable.names(), names);
        let mut copy = variable.clone();
        let written: Vec<(NameRole, String)> = copy
            .names_mut()
            .into_iter()
            .map(|(role, name)| (role, name.clone()))
            .collect();
        assert_eq!(
            written,
            names
                .iter()
                .map(|(role, name)| (*role, name.to_string()))
                .collect::<Vec<_>>()
        );
        assert!(
            variable.map_names(|_, _| None).is_none(),
            "a map that replaces nothing gives no copy"
        );
        let mapped = variable
            .map_names(|role, name| (role != Ident).then(|| name.to_uppercase()))
            .unwrap_or_else(|| variable.clone());
        let after: Vec<String> = mapped
            .names()
            .into_iter()
            .map(|(_, name)| name.to_string())
            .collect();
        let want: Vec<String> = names
            .iter()
            .map(|(role, name)| {
                if *role == Ident {
                    name.to_string()
                } else {
                    name.to_uppercase()
                }
            })
            .collect();
        assert_eq!(after, want);
    }
}

/// Every stock and flow option is an expression text of its variable, under
/// its own role: the rows are `StockOption::ALL`, so a new option fails here
/// until a conveyor or a leak holds it.
#[test]
fn every_stock_and_flow_option_is_an_expression_text_with_its_role() {
    let conveyor = Compat {
        conveyor: Some(Conveyor {
            transit_time: "len".to_string(),
            capacity: Some("capacity".to_string()),
            inflow_limit: Some("in limit".to_string()),
            sample: Some("sample".to_string()),
            arrest: Some("arrest".to_string()),
            discrete: false,
            batch_integrity: false,
            one_at_a_time: true,
            exponential_leak: false,
            ignore_earlier_zone_losses: false,
        }),
        ..Compat::default()
    };
    let leak = Compat {
        leakage: Some(Leakage {
            fraction: Some("fraction".to_string()),
            integers: false,
            zone_start: Some("start".to_string()),
            zone_end: Some("end".to_string()),
        }),
        ..Compat::default()
    };
    let mut found: Vec<StockOption> = [conveyor, leak]
        .iter()
        .flat_map(|compat| compat.expression_texts())
        .map(|(role, text)| match role {
            ExpressionRole::Option(option) => option,
            ExpressionRole::Equation | ExpressionRole::Initial => {
                panic!("{text} is an option's text")
            }
        })
        .collect();
    found.sort();
    let mut all = StockOption::ALL.to_vec();
    all.sort();
    assert_eq!(found, all);
    for option in StockOption::ALL {
        assert!(!option.describe().is_empty());
    }
}
