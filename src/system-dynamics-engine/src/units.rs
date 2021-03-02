// Copyright 2021 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, HashMap};
use std::result::Result as StdResult;

use float_cmp::approx_eq;

use crate::ast::{parse_equation, BinaryOp, Expr, UnaryOp};
use crate::common::{EquationError, EquationResult, ErrorCode};
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
        let mut unit_errors: Vec<(String, Vec<EquationError>)> = Vec::new();

        // step 1: build our base context consisting of all prime units
        let mut aliases = HashMap::new();
        let mut parsed_units = HashMap::new();
        for unit in units.iter().filter(|unit| unit.equation.is_none()) {
            for alias in unit.aliases.iter() {
                if aliases.contains_key(alias) {
                    unit_errors.push((
                        unit.name.clone(),
                        vec![EquationError {
                            start: 0,
                            end: 0,
                            code: ErrorCode::DuplicateUnit,
                        }],
                    ));
                } else {
                    aliases.insert(alias.clone(), unit.name.clone());
                }
            }
            if aliases.contains_key(&unit.name) || parsed_units.contains_key(&unit.name) {
                unit_errors.push((
                    unit.name.clone(),
                    vec![EquationError {
                        start: 0,
                        end: 0,
                        code: ErrorCode::DuplicateUnit,
                    }],
                ));
            } else {
                parsed_units.insert(
                    unit.name.clone(),
                    [(unit.name.clone(), 1)].iter().cloned().collect(),
                );
            }
        }

        let mut ctx = Context {
            aliases,
            units: parsed_units,
        };

        // step 2: use this base context to parse our units with equations
        for unit in units.iter().filter(|unit| unit.equation.is_some()) {
            for alias in unit.aliases.iter() {
                if ctx.aliases.contains_key(alias) {
                    unit_errors.push((
                        unit.name.clone(),
                        vec![EquationError {
                            start: 0,
                            end: 0,
                            code: ErrorCode::DuplicateUnit,
                        }],
                    ));
                } else {
                    ctx.aliases.insert(alias.clone(), unit.name.clone());
                }
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

            if ctx.aliases.contains_key(&unit.name) || ctx.units.contains_key(&unit.name) {
                unit_errors.push((
                    unit.name.clone(),
                    vec![EquationError {
                        start: 0,
                        end: 0,
                        code: ErrorCode::DuplicateUnit,
                    }],
                ));
            } else {
                ctx.units.insert(unit.name.clone(), unit_components);
            }
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

fn build_unit_components(ctx: &Context, ast: &Expr) -> EquationResult<UnitMap> {
    let unit_map: UnitMap = match ast {
        Expr::Const(_, _, loc) => {
            // nothing to do here (handled below in Op2)
            return eqn_err!(NoConstInUnits, loc.start, loc.end);
        }
        Expr::Var(id, _) => {
            let id = ctx.aliases.get(id).unwrap_or(id);
            [(id.to_owned(), 1)].iter().cloned().collect()
        }
        Expr::App(_, _, loc) => {
            return eqn_err!(NoAppInUnits, loc.start, loc.end);
        }
        Expr::Subscript(_, _, loc) => {
            return eqn_err!(NoSubscriptInUnits, loc.start, loc.end);
        }
        Expr::Op1(_, _, loc) => {
            return eqn_err!(NoUnaryOpInUnits, loc.start, loc.end);
        }
        Expr::Op2(op, l, r, loc) => match op {
            BinaryOp::Exp => {
                let exp = const_int_eval(r)?;
                let mut unit_map = build_unit_components(ctx, l)?;
                unit_map.iter_mut().for_each(|(_name, unit)| {
                    *unit *= exp;
                });
                unit_map
            }
            BinaryOp::Mul => {
                let mut unit_map = build_unit_components(ctx, l)?;
                let r = build_unit_components(ctx, r)?;
                for (unit, n) in r.into_iter() {
                    let new_value = match unit_map.get(&unit) {
                        None => n,
                        Some(m) => n + *m,
                    };
                    unit_map.insert(unit, new_value);
                }
                unit_map
            }
            BinaryOp::Div => {
                // check first for the reciprocal case -- 1/blah
                if let Ok(i) = const_int_eval(l) {
                    if i != 1 {
                        let loc = l.get_loc();
                        return eqn_err!(ExpectedIntegerOne, loc.start, loc.end);
                    }
                    let mut unit_map = build_unit_components(ctx, r)?;
                    unit_map.iter_mut().for_each(|(_name, unit)| {
                        *unit *= -1;
                    });
                    unit_map
                } else {
                    let mut unit_map = build_unit_components(ctx, l)?;
                    let r = build_unit_components(ctx, r)?;
                    for (unit, n) in r.into_iter() {
                        let new_value = match unit_map.get(&unit) {
                            None => -n,
                            Some(m) => *m - n,
                        };
                        if new_value == 0 {
                            unit_map.remove(&unit);
                        } else {
                            unit_map.insert(unit, new_value);
                        }
                    }
                    unit_map
                }
            }
            _ => {
                return eqn_err!(BadBinaryOpInUnits, loc.start, loc.end);
            }
        },
        Expr::If(_, _, _, loc) => {
            return eqn_err!(NoIfInUnits, loc.start, loc.end);
        }
    };

    Ok(unit_map)
}

// we have 3 problems here: the first (and simpler) is evaluating unit equations and turning them in to UnitMaps (done)
// the second is: given a context of unitmaps, can we _check_ the types of variables
// the third is: if we only have _some_ units filled in, can we _infer_ the rest?

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

    let more_units = &[
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
            aliases: vec!["itime".to_owned()],
        },
    ];

    let expected2 = Context {
        aliases: [("itime".to_owned(), "invtime".to_owned())]
            .iter()
            .cloned()
            .collect(),
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

    assert_eq!(expected2, Context::new(more_units).unwrap());
}

#[test]
fn test_basic_unit_parsing() {
    let context = Context::new(&[
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
        Unit {
            name: "meter".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["m".to_owned(), "meters".to_owned()],
        },
        Unit {
            name: "second".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["s".to_owned(), "seconds".to_owned()],
        },
    ])
    .unwrap();

    let positive_cases: &[(&str, UnitMap); 4] = &[
        (
            "m^2/s",
            [("meter".to_owned(), 2), ("second".to_owned(), -1)]
                .iter()
                .cloned()
                .collect(),
        ),
        (
            "person * people * persons",
            [("people".to_owned(), 3)].iter().cloned().collect(),
        ),
        (
            "m^2/meters",
            [("meter".to_owned(), 1)].iter().cloned().collect(),
        ),
        (
            "time * people / time",
            [("people".to_owned(), 1)].iter().cloned().collect(),
        ),
    ];

    for (input, output) in positive_cases {
        let expr = parse_equation(input).unwrap().unwrap();
        let result = build_unit_components(&context, &expr).unwrap();
        assert_eq!(*output, result);
    }

    use crate::common::ErrorCode;

    let negative_cases = &[
        ("2 / time", ErrorCode::ExpectedIntegerOne),
        ("2 * time", ErrorCode::NoConstInUnits),
        ("foo(time)", ErrorCode::NoAppInUnits),
        ("bar[time]", ErrorCode::NoSubscriptInUnits),
        ("-time", ErrorCode::NoUnaryOpInUnits),
        ("if 1 then time else people", ErrorCode::NoIfInUnits),
        ("time + people", ErrorCode::BadBinaryOpInUnits),
    ];

    for (input, output) in negative_cases {
        let expr = parse_equation(input).unwrap().unwrap();
        let result = build_unit_components(&context, &expr).unwrap_err();
        assert_eq!(*output, result.code);
    }
}

#[test]
fn test_basic_unit_checks() {
    let _context = Context::new(&[
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
        Unit {
            name: "USD".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["dollar".to_owned(), "dollars".to_owned(), "$".to_owned()],
        },
    ])
    .unwrap();
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
        ("7 / 0", 0),
        ("4 - 1", 3),
        ("15 mod 7", 1),
        ("3^(1+2)", 27),
        ("4 > 2", 1),
        ("4 < 2", 0),
        ("5 >= 5", 1),
        ("7 <= 6", 0),
        ("3 and 2", 1),
        ("0 or 3", 1),
        ("3 = 3", 1),
        ("3 <> 3", 0),
        ("not 7", 0),
        ("not 0", 1),
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
