// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, HashMap};
use std::result::Result as StdResult;

use float_cmp::approx_eq;

use crate::ast::{parse_equation, BinaryOp, Expr, UnaryOp};
use crate::common::{EquationError, EquationResult};
use crate::datamodel::Unit;
use crate::eqn_err;

type UnitMap = BTreeMap<String, i32>;

#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub struct Context {
    aliases: HashMap<String, String>,
    units: HashMap<String, UnitMap>,
}

impl Context {
    #[allow(dead_code)]
    fn new(units: &[Unit]) -> StdResult<Self, Vec<(String, Vec<EquationError>)>> {
        // step 1: build our base context consisting of all prime units
        let mut aliases = HashMap::new();
        let mut parsed_units = HashMap::new();
        for unit in units.iter().filter(|unit| unit.equation.is_none()) {
            for alias in unit.aliases.iter() {
                aliases.insert(alias.clone(), unit.name.clone());
            }
            parsed_units.insert(
                unit.name.clone(),
                [(unit.name.clone(), 1)].iter().cloned().collect(),
            );
        }

        let mut ctx = Context {
            aliases,
            units: parsed_units,
        };

        let mut unit_errors: Vec<(String, Vec<EquationError>)> = Vec::new();

        // step 2: use this base context to parse our units with equations
        for unit in units.iter().filter(|unit| unit.equation.is_some()) {
            for alias in unit.aliases.iter() {
                ctx.aliases.insert(alias.clone(), unit.name.clone());
            }

            let eqn = unit.equation.as_ref().unwrap();

            let ast = match parse_equation(eqn) {
                Ok(ast) => ast,
                Err(errors) => {
                    unit_errors.push((unit.name.clone(), errors));
                    continue;
                }
            };

            let unit_components = match ast {
                Some(ref ast) => match build_unit_components(&ctx, ast) {
                    Ok(unit_components) => unit_components,
                    Err(err) => {
                        unit_errors.push((
                            unit.name.clone(),
                            vec![EquationError {
                                start: 0,
                                end: 0,
                                code: err.code,
                            }],
                        ));
                        continue;
                    }
                },
                None => [(unit.name.clone(), 1)].iter().cloned().collect(),
            };

            ctx.units.insert(unit.name.clone(), unit_components);
        }

        if unit_errors.is_empty() {
            Ok(ctx)
        } else {
            Err(unit_errors)
        }
    }
}

#[allow(dead_code)]
fn const_int_eval(ast: &Expr) -> EquationResult<i32> {
    match ast {
        Expr::Const(_, n, loc) => {
            if approx_eq!(f64, *n, n.round()) {
                Ok(n.round() as i32)
            } else {
                eqn_err!(ExpectedInteger, loc.start, loc.end)
            }
        }
        Expr::Var(_, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr::App(_, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr::Subscript(_, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr::Op1(op, expr, _) => {
            let expr = const_int_eval(expr)?;
            let result = match op {
                UnaryOp::Positive => expr,
                UnaryOp::Negative => -expr,
                UnaryOp::Not => {
                    if expr == 0 {
                        1
                    } else {
                        0
                    }
                }
            };
            Ok(result)
        }
        Expr::Op2(op, l, r, _) => {
            let l = const_int_eval(l)?;
            let r = const_int_eval(r)?;
            let result = match op {
                BinaryOp::Add => l + r,
                BinaryOp::Sub => l - r,
                BinaryOp::Exp => l.pow(r as u32),
                BinaryOp::Mul => l * r,
                BinaryOp::Div => {
                    if r == 0 {
                        0
                    } else {
                        l / r
                    }
                }
                BinaryOp::Mod => l % r,
                BinaryOp::Gt => (l > r) as i32,
                BinaryOp::Lt => (l < r) as i32,
                BinaryOp::Gte => (l >= r) as i32,
                BinaryOp::Lte => (l <= r) as i32,
                BinaryOp::Eq => (l == r) as i32,
                BinaryOp::Neq => (l != r) as i32,
                BinaryOp::And => ((l != 0) && (r != 0)) as i32,
                BinaryOp::Or => ((l != 0) || (r != 0)) as i32,
            };
            Ok(result)
        }
        Expr::If(_, _, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
    }
}

fn build_unit_components(_ctx: &Context, ast: &Expr) -> EquationResult<UnitMap> {
    let mut unit_map = UnitMap::new();

    match ast {
        Expr::Const(_, _, _) => {
            // nothing to do here (handled below in Op2)
        }
        Expr::Var(id, _) => {
            unit_map.insert(id.to_owned(), 1);
        }
        Expr::App(_, _, loc) => {
            return eqn_err!(NoAppInUnits, loc.start, loc.end);
        }
        Expr::Subscript(_, _, loc) => {
            return eqn_err!(NoSubscriptInUnits, loc.start, loc.end);
        }
        Expr::Op1(_op, _expr, _) => {}
        Expr::Op2(_op, _l, _r, _) => {}
        Expr::If(_, _, _, loc) => {
            return eqn_err!(NoSubscriptInUnits, loc.start, loc.end);
        }
    }

    Ok(unit_map)
}

// we have 2 problems here: the first (and simpler) is evaluating unit equations and turning them in to UnitMaps
// the second is: given a context of unitmaps, can we check and _infer_ the types of variables

#[test]
fn test_context_creation() {
    let simple_units = &[
        Unit {
            name: "time".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec![],
        },
        Unit {
            name: "people".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["person".to_owned(), "persons".to_owned()],
        },
    ];

    let expected = Context {
        aliases: [
            ("person".to_owned(), "people".to_owned()),
            ("persons".to_owned(), "people".to_owned()),
        ]
        .iter()
        .cloned()
        .collect(),
        units: [
            (
                "time".to_owned(),
                [("time".to_owned(), 1)].iter().cloned().collect(),
            ),
            (
                "people".to_owned(),
                [("people".to_owned(), 1)].iter().cloned().collect(),
            ),
        ]
        .iter()
        .cloned()
        .collect(),
    };

    assert_eq!(expected, Context::new(simple_units).unwrap());

    let _more_units = &[
        Unit {
            name: "time".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec![],
        },
        Unit {
            name: "invtime".to_owned(),
            equation: Some("1/time".to_owned()),
            disabled: false,
            aliases: vec![],
        },
    ];

    let _expected2 = Context {
        aliases: HashMap::new(),
        units: [
            (
                "time".to_owned(),
                [("time".to_owned(), 1)].iter().cloned().collect(),
            ),
            (
                "invtime".to_owned(),
                [("time".to_owned(), -1)].iter().cloned().collect(),
            ),
        ]
        .iter()
        .cloned()
        .collect(),
    };

    // assert_eq!(expected2, Context::new(more_units).unwrap());
}

#[test]
fn test_basic_unit_checks() {
    // from a set of datamodel::Units build a Context

    // with a context, check if a set of variables unit checks
}

#[test]
fn test_const_int_eval() {
    let positive_cases = &[
        ("0", 0),
        ("1", 1),
        ("-1", -1),
        ("1 * 1", 1),
        ("2 / 3", 0),
        ("4 - 1", 3),
        ("15 mod 7", 1),
        ("3^(1+2)", 27),
        ("4 > 2", 1),
        ("4 < 2", 0),
        ("5 >= 5", 1),
        ("7 <= 6", 0),
        ("3 and 2", 1),
        ("0 or 3", 1),
    ];

    for (input, output) in positive_cases {
        let expr = parse_equation(input).unwrap().unwrap();
        assert_eq!(*output, const_int_eval(&expr).unwrap());
    }

    use crate::common::ErrorCode;

    let negative_cases = &["3.5", "foo", "if 1 then 2 else 3", "bar[2]", "foo(1, 2)"];

    for input in negative_cases {
        let expr = parse_equation(input).unwrap().unwrap();
        assert_eq!(
            ErrorCode::ExpectedInteger,
            const_int_eval(&expr).unwrap_err().code
        );
    }
}
