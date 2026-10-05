// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde_json::{Value, json};

use super::*;
use crate::test_common::TestProject;

fn view(project: TestProject, ident: &str) -> Value {
    let datamodel = project.build_datamodel();
    serde_json::to_value(variable_equation(&datamodel, "main", ident).expect("the variable"))
        .unwrap()
}

/// The tree of `every` alone.
fn tree(project: TestProject, ident: &str) -> Value {
    view(project, ident)["every"]["tree"].clone()
}

fn ages() -> TestProject {
    TestProject::new("view")
        .named_dimension("Age", &["Young", "Adult", "Old"])
        .array_with_ranges(
            "c[Age]",
            vec![("Young", "10"), ("Adult", "20"), ("Old", "30")],
        )
        .aux("rate", "0.1", None)
}

#[test]
fn a_scalar_is_its_equation_with_no_dimensions_or_cells() {
    let project = TestProject::new("view")
        .aux("Birth Rate", "0.1", None)
        .aux("births", "population * birth_rate", None)
        .stock("population", "100", &["births"], &[], None);
    assert_eq!(
        view(project, "births"),
        json!({
            "kind": "aux",
            "dimensions": [],
            "every": {
                "text": "population * birth_rate",
                "tree": {
                    "kind": "op", "op": "*", "span": [0, 23],
                    "left": {"kind": "ref", "ident": "population", "name": "population",
                             "known": true, "span": [0, 10]},
                    "right": {"kind": "ref", "ident": "birth_rate", "name": "Birth Rate",
                              "known": true, "span": [13, 23]}
                }
            },
            "cells": []
        })
    );
}

/// A stock's equation is its initial value.
#[test]
fn a_stocks_equation_is_its_initial_value() {
    let project = TestProject::new("view").flow("births", "1", None).stock(
        "population",
        "100",
        &["births"],
        &[],
        None,
    );
    let view = view(project, "population");
    assert_eq!(view["kind"], "stock");
    assert_eq!(view["every"]["text"], "100");
    assert_eq!(
        view["every"]["tree"],
        json!({"kind": "number", "text": "100", "span": [0, 3]})
    );
}

/// The engine puts the parentheses the grouping rule needs, so a host needs
/// no precedence table: `(a + b) * c` groups its left operand, `a + b * c`
/// groups nothing, and `a - (b - c)` groups its right.
#[test]
fn groups_are_where_the_grouping_rule_puts_parentheses() {
    let project = || {
        TestProject::new("view")
            .aux("a", "1", None)
            .aux("b", "1", None)
            .aux("c", "1", None)
    };
    let grouped = tree(project().aux("x", "(a + b) * c", None), "x");
    assert_eq!(grouped["left"]["kind"], "group");
    assert_eq!(grouped["left"]["inner"]["op"], "+");
    assert_eq!(grouped["right"]["kind"], "ref");

    let plain = tree(project().aux("x", "a + b * c", None), "x");
    assert_eq!(plain["op"], "+");
    assert_eq!(plain["right"]["kind"], "op");

    let right = tree(project().aux("x", "a - (b - c)", None), "x");
    assert_eq!(right["right"]["kind"], "group");
    assert_eq!(right["left"]["kind"], "ref");
}

/// `a // b` is the operator written, with the grouping it binds by: a call
/// of `safediv` is a call only where it is written as one.
#[test]
fn safe_division_is_the_operator_written() {
    let project = || {
        TestProject::new("view")
            .aux("a", "1", None)
            .aux("b", "1", None)
            .aux("c", "1", None)
    };
    let op = tree(project().aux("x", "a // b", None), "x");
    assert_eq!(op["kind"], "op");
    assert_eq!(op["op"], "//");
    assert_eq!(op["left"]["ident"], "a");
    let grouped = tree(project().aux("x", "a * (b // c)", None), "x");
    assert_eq!(grouped["right"]["kind"], "group");
    assert_eq!(grouped["right"]["inner"]["op"], "//");
    let call = tree(project().aux("x", "SAFEDIV(a, b)", None), "x");
    assert_eq!(call["kind"], "call");
}

/// An element qualified by its dimension, and a module's output, are names
/// that read something.
#[test]
fn a_qualified_element_and_a_modules_output_are_known() {
    let value = tree(
        ages().array_aux("d[Age]", "IF Age = Age.Young THEN 1 ELSE 0"),
        "d",
    );
    assert_eq!(value["cond"]["right"]["known"], true, "{value}");
    let index = tree(ages().aux("e", "c[Age.Old]", None), "e");
    assert_eq!(index["index"][0]["node"]["known"], true, "{index}");

    let mut project = TestProject::new("view")
        .aux("input", "1", None)
        .build_datamodel();
    let mut sub = project.models[0].clone();
    sub.name = "sub".to_string();
    sub.variables =
        crate::shared_vec::SharedVec::from(vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "output".to_string(),
            equation: Equation::Scalar("2".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })]);
    project.models.push(sub);
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
            ident: "m".to_string(),
            model_name: "sub".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    for (text, known) in [("m.output * 2", true), ("m.nothing * 2", false)] {
        let mut project = project.clone();
        project.models[0]
            .variables
            .push(datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: Equation::Scalar(text.to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }));
        let view = serde_json::to_value(variable_equation(&project, "main", "x").unwrap()).unwrap();
        assert_eq!(
            view["every"]["tree"]["left"]["known"], known,
            "{text}: {view}"
        );
    }
}

/// A builtin is a call as written, never the module the compiler makes of
/// it; an `IF` is its three parts; an unknown name is a reference the model
/// does not have.
#[test]
fn calls_conditionals_and_unknown_names() {
    let project = TestProject::new("view").aux("x", "1", None).aux(
        "y",
        "IF x > 0 THEN SMTH1(x, 3) ELSE missing",
        None,
    );
    let tree = tree(project, "y");
    assert_eq!(tree["kind"], "if");
    assert_eq!(tree["cond"]["op"], ">");
    assert_eq!(tree["then"]["kind"], "call");
    assert_eq!(tree["then"]["args"].as_array().unwrap().len(), 2);
    assert_eq!(
        tree["else"],
        json!({"kind": "ref", "ident": "missing", "name": "missing", "known": false,
               "span": [31, 38]})
    );
}

/// Inside brackets an element of the axis is an element, a dimension's
/// name is "this element", and arithmetic over it is an expression -- `Age
/// - 1`, the element before.
#[test]
fn indices_name_elements_dimensions_and_offsets() {
    let project = ages().array_aux("d[Age]", "c[Age - 1] + c[young] + c[Age] + SUM(c[*])");
    let view = view(project, "d");
    let tree = &view["every"]["tree"];
    // ((c[Age - 1] + c[young]) + c[Age]) + SUM(c[*])
    let offset = &tree["left"]["left"]["left"]["index"][0];
    assert_eq!(offset["kind"], "expr");
    assert_eq!(offset["node"]["op"], "-");
    assert_eq!(
        offset["node"]["left"],
        json!({"kind": "dimension", "name": "Age", "span": [2, 5]})
    );
    assert_eq!(
        tree["left"]["left"]["right"]["index"][0],
        json!({"kind": "element", "name": "Young", "dimension": "Age", "span": [15, 20]})
    );
    assert_eq!(
        tree["left"]["right"]["index"][0],
        json!({"kind": "dimension", "name": "Age", "span": [26, 29]})
    );
    assert_eq!(tree["right"]["args"][0]["index"][0]["kind"], "all");
}

/// An apply-to-all variable is its equation for every element, with one
/// cell per element following it, in the dimension's order.
#[test]
fn an_apply_to_all_variable_has_every_cell_following_it() {
    let view = view(ages().array_aux("d[Age]", "c * rate"), "d");
    assert_eq!(
        view["dimensions"],
        json!([{"name": "Age", "elements": ["Young", "Adult", "Old"]}])
    );
    assert_eq!(view["every"]["text"], "c * rate");
    assert_eq!(
        view["cells"],
        json!([
            {"element": ["Young"], "key": "young", "state": "follows"},
            {"element": ["Adult"], "key": "adult", "state": "follows"},
            {"element": ["Old"], "key": "old", "state": "follows"}
        ])
    );
}

/// A default with exceptions: the default is the top row, an element with
/// its own equation is `own`, the rest follow -- in the dimension's order,
/// whatever order the elements were stored in.
#[test]
fn a_default_with_exceptions_is_a_top_row_and_its_own_cells() {
    let project = ages().array_with_default_and_overrides(
        "d[Age]",
        "c[Age - 1] * rate",
        vec![("Old", "c[Adult] + 1"), ("Young", "5")],
    );
    let view = view(project, "d");
    assert_eq!(view["every"]["text"], "c[Age - 1] * rate");
    let cells = view["cells"].as_array().unwrap();
    let states: Vec<(&str, &str)> = cells
        .iter()
        .map(|c| (c["key"].as_str().unwrap(), c["state"].as_str().unwrap()))
        .collect();
    assert_eq!(
        states,
        vec![("young", "own"), ("adult", "follows"), ("old", "own")]
    );
    assert_eq!(cells[0]["own"]["text"], "5");
    assert_eq!(
        cells[2]["own"]["tree"]["left"]["index"][0]["kind"],
        "element"
    );
}

/// An element with no equation and no top row to follow is `missing`; an
/// element equation naming no element is shown, not dropped.
#[test]
fn missing_cells_and_arms_naming_no_element() {
    let project = ages().array_with_ranges("d[Age]", vec![("Young", "1"), ("Elder", "2")]);
    let view = view(project, "d");
    assert_eq!(view["every"], Value::Null);
    let states: Vec<&str> = view["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, vec!["own", "missing", "missing"]);
    assert_eq!(view["unmatched"][0]["subscript"], "Elder");
    assert_eq!(view["unmatched"][0]["equation"]["text"], "2");
}

/// Cells of two dimensions are row-major, each naming its element on both.
#[test]
fn two_dimensions_are_row_major() {
    let project = ages()
        .named_dimension("Region", &["North", "South"])
        .array_with_ranges("p[Region, Age]", vec![("South,Old", "9")]);
    let view = view(project, "p");
    let keys: Vec<&str> = view["cells"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        vec![
            "north,young",
            "north,adult",
            "north,old",
            "south,young",
            "south,adult",
            "south,old"
        ]
    );
    assert_eq!(view["cells"][5]["element"], json!(["South", "Old"]));
    assert_eq!(view["cells"][5]["state"], "own");
}

/// A text that does not parse has no tree and says where it fails; spans
/// are UTF-8 byte offsets, so a name of non-ASCII letters spans its bytes.
#[test]
fn a_text_that_does_not_parse_and_byte_offsets() {
    let broken = view(TestProject::new("view").aux("x", "1 +", None), "x");
    assert_eq!(broken["every"]["tree"], Value::Null);
    assert_eq!(broken["every"]["error"]["span"], json!([3, 4]));
    assert!(
        !broken["every"]["error"]["reason"]
            .as_str()
            .unwrap()
            .is_empty()
    );

    let wide = tree(
        TestProject::new("view")
            .aux("café", "1", None)
            .aux("x", "café + 1", None),
        "x",
    );
    // `café` is five bytes, four characters.
    assert_eq!(wide["left"]["span"], json!([0, 5]));
    assert_eq!(wide["right"]["span"], json!([8, 9]));
}

/// A variable the model lacks, or a model the project lacks, has no view.
#[test]
fn a_missing_variable_or_model_has_no_view() {
    let datamodel = TestProject::new("view")
        .aux("x", "1", None)
        .build_datamodel();
    assert!(variable_equation(&datamodel, "main", "nope").is_none());
    assert!(variable_equation(&datamodel, "elsewhere", "x").is_none());
}

/// A dimension's name used as a value is a dimension node.
#[test]
fn a_dimension_as_a_value() {
    let tree = tree(ages().array_aux("d[Age]", "Age * 2"), "d");
    assert_eq!(
        tree["left"],
        json!({"kind": "dimension", "name": "Age", "span": [0, 3]})
    );
}
