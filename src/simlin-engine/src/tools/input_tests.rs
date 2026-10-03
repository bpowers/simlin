// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use serde::Deserialize;
use serde_json::json;

use super::*;

/// An input with every shape a tool's input has: required and optional
/// fields, a list of structs, a list of internally tagged enums, a fixed
/// pair, and an enum named by a string.
#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Input {
    name: String,
    #[serde(default)]
    from_time: Option<f64>,
    #[serde(default)]
    changes: Vec<Change>,
    #[serde(default)]
    operations: Vec<Operation>,
    #[serde(default)]
    specs: Option<Specs>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    variable: String,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    count: Option<usize>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    SetEquation {
        variable: String,
        equation: String,
    },
    SetLookup {
        variable: String,
        points: Vec<[f64; 2]>,
    },
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Specs {
    method: Method,
    #[serde(default)]
    stop: Option<f64>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Method {
    Euler,
    Rk4,
}

fn parsed(input: serde_json::Value) -> Result<Input, Mismatch> {
    parse(&input.to_string())
}

/// What a text parser reads from the same input: the tracked read must agree
/// with it on every input that is the type's.
fn by_text(input: &serde_json::Value) -> Option<Input> {
    serde_json::from_str(&input.to_string()).ok()
}

#[test]
fn input_that_is_the_type_reads_as_a_text_parser_reads_it() {
    for input in [
        json!({"name": "plain"}),
        json!({"name": "all", "fromTime": 2.5, "changes": [
            {"variable": "a", "value": 1}, {"variable": "b", "value": 0.5, "count": 3}
        ], "operations": [
            {"op": "set_equation", "variable": "a", "equation": "b * 2"},
            {"op": "set_lookup", "variable": "t", "points": [[0, 1], [1.5, 2e3], [-3, 4]]}
        ], "specs": {"method": "rk4", "stop": 100}}),
        json!({"name": "nulls", "fromTime": null, "specs": null}),
        json!({"name": "a {tricky}, \"name\" [1]\n", "changes": []}),
    ] {
        let expected = by_text(&input).unwrap_or_else(|| panic!("{input} is the type"));
        assert_eq!(parsed(input.clone()), Ok(expected), "{input}");
    }
}

/// One row per way a value fails to be the type, each at a first, a middle
/// and a last position where a list is involved: the path is exact wherever
/// the mismatch is, and a text parser refuses the same input.
#[test]
fn a_mismatch_is_reported_at_its_place_in_the_input() {
    let change = |i: usize| json!({"variable": format!("v{i}"), "value": 1});
    let operation =
        |i: usize| json!({"op": "set_equation", "variable": format!("v{i}"), "equation": "1"});
    let mut rows: Vec<(serde_json::Value, String, &str)> = vec![
        (json!({}), String::new(), "missing field `name`"),
        (
            json!("text"),
            String::new(),
            "invalid type: string \"text\"",
        ),
        (
            json!({"name": 3}),
            "name".to_string(),
            "invalid type: integer `3`, expected a string",
        ),
        (
            json!({"name": "x", "nmae": 1}),
            "nmae".to_string(),
            "unknown field `nmae`",
        ),
        (
            json!({"name": "x", "fromTime": "soon"}),
            "fromTime".to_string(),
            "invalid type: string \"soon\", expected a number",
        ),
        (
            json!({"name": "x", "specs": {"stop": 5}}),
            "specs".to_string(),
            "missing field `method`",
        ),
        (
            json!({"name": "x", "specs": {"method": "rk9"}}),
            "specs.method".to_string(),
            "unknown variant `rk9`, expected `euler` or `rk4`",
        ),
        (
            json!({"name": "x", "specs": {"method": "euler", "stop": []}}),
            "specs.stop".to_string(),
            "invalid type: sequence, expected a number",
        ),
        (
            json!({"name": "x", "operations": [
                {"op": "set_lookup", "variable": "t", "points": [[0, 1], [1, "two"]]}
            ]}),
            // An internally tagged enum's variant is read from a copy, so
            // the field at fault is found by elimination: the field is
            // named, not the place inside it.
            "operations[0].points".to_string(),
            "invalid type: string \"two\", expected a number",
        ),
        (
            json!({"name": "x", "operations": [
                {"op": "set_equation", "variable": 5, "equation": "1"}
            ]}),
            "operations[0].variable".to_string(),
            "invalid type: integer `5`, expected a string",
        ),
        (
            json!({"name": "x", "operations": [{"op": "set_equation", "variable": "a"}]}),
            "operations[0]".to_string(),
            "missing field `equation`",
        ),
    ];
    for at in 0..3 {
        let mut changes: Vec<_> = (0..3).map(change).collect();
        changes[at]["value"] = json!("five");
        rows.push((
            json!({"name": "x", "changes": changes}),
            format!("changes[{at}].value"),
            "invalid type: string \"five\", expected a number",
        ));
        let mut changes: Vec<_> = (0..3).map(change).collect();
        changes[at]["count"] = json!(-1);
        rows.push((
            json!({"name": "x", "changes": changes}),
            format!("changes[{at}].count"),
            "invalid value: integer `-1`, expected a whole number",
        ));
        let mut operations: Vec<_> = (0..3).map(operation).collect();
        operations[at]["eqn"] = json!("1");
        rows.push((
            json!({"name": "x", "operations": operations}),
            format!("operations[{at}].eqn"),
            "unknown field `eqn`, expected `variable` or `equation`",
        ));
        let mut operations: Vec<_> = (0..3).map(operation).collect();
        operations[at]["op"] = json!("set_eqn");
        rows.push((
            json!({"name": "x", "operations": operations}),
            format!("operations[{at}].op"),
            "unknown variant `set_eqn`, expected `set_equation` or `set_lookup`",
        ));
    }
    for (input, path, reason) in rows {
        assert!(by_text(&input).is_none(), "{input} is not the type");
        match parsed(input.clone()) {
            Err(Mismatch::NotInput {
                path: at,
                reason: why,
            }) => {
                assert_eq!(at, path, "{input}: {why}");
                assert!(why.starts_with(reason), "{input}: {why}");
                assert!(!why.contains(" line "), "{why}");
            }
            other => panic!("{input}: {other:?}"),
        }
    }
}

/// Input serde reads as the type and no agent means: a struct written as a
/// list of its fields in order (which a text parser reads too), and a tag
/// written as a variant's index (which serde reads from a value, as the
/// tracked read is made). Each is refused where it is.
#[test]
fn input_read_as_serde_would_and_not_as_json_is_written_is_refused() {
    for (input, path, reason) in [
        (
            json!(["x", 2.5]),
            "",
            "invalid type: sequence, expected an object",
        ),
        (
            json!({"name": "x", "changes": [["a", 1, 2]]}),
            "changes[0]",
            "invalid type: sequence, expected an object",
        ),
        (
            json!({"name": "x", "specs": ["euler"]}),
            "specs",
            "invalid type: sequence, expected an object",
        ),
        (
            json!({"name": "x", "operations": [["set_equation", "a", "1"]]}),
            "operations[0]",
            "invalid type: sequence, expected an object",
        ),
        (
            json!({"name": "x", "operations": [{"op": 0, "variable": "a", "equation": "1"}]}),
            "operations[0].op",
            "invalid type: integer `0`, expected a name",
        ),
    ] {
        match parsed(input.clone()) {
            Err(Mismatch::NotInput {
                path: at,
                reason: why,
            }) => {
                assert_eq!(at, path, "{input}: {why}");
                assert!(why.starts_with(reason), "{input}: {why}");
            }
            other => panic!("{input}: {other:?}"),
        }
    }
}

#[test]
fn text_that_is_not_json_is_refused_with_the_parsers_position() {
    for text in [
        "{\"name\": ",
        "name",
        "{\"name\": \"x\"} trailing",
        // A key twice, which a text parser reads as the last.
        "{\"name\": \"x\", \"name\": \"y\"}",
        "{\"name\": \"x\", \"specs\": {\"method\": \"rk4\", \"method\": \"euler\"}}",
    ] {
        match parse::<Input>(text) {
            Err(Mismatch::NotJson(reason)) => {
                assert!(reason.contains("line 1 column"), "{text}: {reason}")
            }
            other => panic!("{text}: {other:?}"),
        }
    }
}

/// The shapes an equation's depth is made of, each nesting one way.
#[derive(Clone, Copy, Debug)]
enum Shape {
    /// A chain of operators: `1 + 1 + ... + 1`.
    Chain,
    /// Nested parentheses: `((...1...))`.
    Parentheses,
    /// Nested conditionals: `IF 1 THEN 1 ELSE (...)`.
    Conditionals,
    /// Nested calls: `ABS(ABS(...1...))`.
    Calls,
}

impl Shape {
    const ALL: [Shape; 4] = [
        Shape::Chain,
        Shape::Parentheses,
        Shape::Conditionals,
        Shape::Calls,
    ];

    /// An equation of this shape that measures exactly `depth`, as deep in
    /// its own way as it can be, the rest made up with a chain of operators.
    fn of_depth(self, depth: usize) -> String {
        // (the units one level of the shape is, how a level opens and closes)
        let (per_level, open, close) = match self {
            Shape::Chain => (1, "1 + ", ""),
            Shape::Parentheses => (2, "(", ")"),
            Shape::Conditionals => (3, "IF 1 THEN 1 ELSE (", ")"),
            Shape::Calls => (8, "ABS(", ")"),
        };
        let levels = depth / per_level;
        let rest = depth - levels * per_level;
        format!(
            "{}{}1{}",
            open.repeat(levels),
            "1 + ".repeat(rest),
            close.repeat(levels)
        )
    }
}

#[test]
fn an_equations_depth_counts_operators_and_open_brackets() {
    for shape in Shape::ALL {
        for depth in [0, 1, 7, 100, 101] {
            assert_eq!(
                equation_depth(&shape.of_depth(depth)),
                depth,
                "{shape:?}: {}",
                shape.of_depth(depth)
            );
        }
    }
    // A balanced tree is counted by its operators all the same: the measure
    // is an upper bound.
    assert_eq!(equation_depth("(a + b) * (c + d)"), 5);
    assert_eq!(equation_depth("x[i + 1]"), 3);
}

/// An equation an agent sends is bounded before anything parses it: at
/// [`MAX_EQUATION_DEPTH`], in every shape, `edit_model`, `run_experiment`
/// and `verify_findings` answer on a thread with the smallest stack a host
/// gives a call (2 MiB, a spawned thread's default), where the parser and the
/// compile would otherwise abort the process; one unit over, each refuses,
/// saying why. The default suite runs it in a debug build, whose frames are
/// the largest; it holds in a release build too.
#[test]
fn an_equation_at_the_bound_is_read_and_one_over_it_is_refused() {
    use crate::tools::Session;
    use crate::tools::test_support::{Host, inventory};
    let answer = |depth: usize, shape: Shape| {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let equation = shape.of_depth(depth);
                let mut host = Host::from_test_project(&inventory());
                let mut session = Session::new("main");
                host.call(&mut session, "read_model", json!({}));
                let calls = [
                    (
                        "edit_model",
                        json!({"summary": "deep", "operations": [
                            {"op": "set_equation", "variable": "coverage", "equation": equation}]}),
                    ),
                    (
                        "run_experiment",
                        json!({"name": "deep", "set": [
                            {"variable": "coverage", "equation": equation}]}),
                    ),
                    (
                        "verify_findings",
                        json!({"findings": [{"kind": "observation", "claim": "c", "citations": [
                            {"cites": "equation", "variable": "coverage", "equation": equation}]}]}),
                    ),
                ];
                calls
                    .into_iter()
                    .map(|(tool, input)| {
                        let output = host.call_raw(&mut session, tool, &input.to_string());
                        (tool, output.json)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap()
            .join()
            .unwrap_or_else(|_| panic!("{shape:?} at {depth} answers"))
    };
    for shape in Shape::ALL {
        for (tool, json) in answer(MAX_EQUATION_DEPTH, shape) {
            assert!(
                !json.contains("nested too deeply"),
                "{shape:?} {tool}: {json}"
            );
        }
        for (tool, json) in answer(MAX_EQUATION_DEPTH + 1, shape) {
            assert!(
                json.contains("nested too deeply"),
                "{shape:?} {tool}: {json}"
            );
        }
    }
}
