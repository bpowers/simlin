// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::result::Result as StdResult;

use float_cmp::approx_eq;

use crate::ast::{BinaryOp, Expr0, UnaryOp};
use crate::common::{EquationError, EquationResult, ErrorCode, UnitError};
use crate::datamodel::{SimSpecs, Unit, UnitMap};
use crate::token::LexerType;
use crate::{canonicalize, eqn_err};

/// Units is used to distinguish between explicit units (and explicit
/// dimensionless-ness) and dimensionless-ness that comes from computing
/// on constants.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Units {
    Explicit(UnitMap),
    Constant,
}

impl Units {
    pub fn equals(&self, rhs: &Units) -> bool {
        match (self, rhs) {
            (Units::Constant, Units::Constant)
            | (Units::Explicit(_), Units::Constant)
            | (Units::Constant, Units::Explicit(_)) => true,
            (Units::Explicit(lhs), Units::Explicit(rhs)) => *lhs == *rhs,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnitOp {
    Mul,
    Div,
}

pub(crate) fn combine(op: UnitOp, l: UnitMap, r: UnitMap) -> UnitMap {
    let mut l = match op {
        UnitOp::Mul => l * r,
        UnitOp::Div => l / r,
    };

    if l.map.contains_key("dmnl") {
        l.map.remove("dmnl");
    }

    l
}

#[derive(Debug, Default, PartialEq)]
pub struct Context {
    pub sim_specs: SimSpecs,
    aliases: HashMap<String, String>,
    units: HashMap<String, UnitMap>,
}

impl Context {
    pub fn new_with_builtins(
        units: &[Unit],
        sim_specs: &SimSpecs,
    ) -> StdResult<Self, Vec<(String, Vec<EquationError>)>> {
        let builtin_units: &[(&str, &[&str])] = &[
            // ("dollars", &["$", "usd"]),
            // ("year", &["years"]),
            // ("month", &["months"]),
            // ("person", &["people", "persons", "peoples"]),
        ];
        let mut builtin_units = builtin_units
            .iter()
            .map(|(name, aliases)| Unit {
                name: name.to_string(),
                equation: None,
                disabled: false,
                aliases: aliases.iter().map(|s| s.to_string()).collect(),
            })
            .collect::<Vec<_>>();

        builtin_units.append(&mut units.to_vec());

        Self::new(&builtin_units, sim_specs)
    }
    pub fn new(
        units: &[Unit],
        sim_specs: &SimSpecs,
    ) -> StdResult<Self, Vec<(String, Vec<EquationError>)>> {
        let mut unit_errors: Vec<(String, Vec<EquationError>)> = Vec::new();

        // step 1: build our base context consisting of all prime units
        let mut aliases = HashMap::new();
        let mut parsed_units = HashMap::new();
        for unit in units.iter().filter(|unit| unit.equation.is_none()) {
            let unit_name = canonicalize(&unit.name);
            for alias in unit.aliases.iter() {
                let alias = canonicalize(alias);
                if let Entry::Vacant(e) = aliases.entry(alias) {
                    e.insert(unit_name.clone());
                } else {
                    unit_errors.push((
                        unit_name.clone(),
                        vec![EquationError {
                            start: 0,
                            end: 0,
                            code: ErrorCode::DuplicateUnit,
                        }],
                    ));
                }
            }
            if aliases.contains_key(&unit_name) || parsed_units.contains_key(&unit_name) {
                unit_errors.push((
                    unit_name.clone(),
                    vec![EquationError {
                        start: 0,
                        end: 0,
                        code: ErrorCode::DuplicateUnit,
                    }],
                ));
            } else {
                parsed_units.insert(
                    unit_name.clone(),
                    [(unit_name.clone(), 1)].iter().cloned().collect(),
                );
            }
        }

        let mut ctx = Context {
            sim_specs: sim_specs.clone(),
            aliases,
            units: parsed_units,
        };

        // step 2: use this base context to parse our units with equations
        for unit in units.iter().filter(|unit| unit.equation.is_some()) {
            let unit_name = canonicalize(&unit.name);
            for alias in unit.aliases.iter() {
                let alias = canonicalize(alias);
                if let Entry::Vacant(e) = ctx.aliases.entry(alias) {
                    e.insert(unit_name.clone());
                } else {
                    unit_errors.push((
                        unit_name.clone(),
                        vec![EquationError {
                            start: 0,
                            end: 0,
                            code: ErrorCode::DuplicateUnit,
                        }],
                    ));
                }
            }

            let eqn = unit.equation.as_ref().unwrap();

            let ast = match Expr0::new(eqn, LexerType::Units) {
                Ok(ast) => ast,
                Err(errors) => {
                    unit_errors.push((unit_name.clone(), errors));
                    continue;
                }
            };

            let unit_components = match ast {
                Some(ref ast) => match build_unit_components(&ctx, ast) {
                    Ok(unit_components) => unit_components,
                    Err(err) => {
                        unit_errors.push((
                            unit_name.clone(),
                            vec![EquationError {
                                start: 0,
                                end: 0,
                                code: err.code,
                            }],
                        ));
                        continue;
                    }
                },
                None => [(unit_name.clone(), 1)].iter().cloned().collect(),
            };

            if ctx.aliases.contains_key(&unit_name) || ctx.units.contains_key(&unit_name) {
                unit_errors.push((
                    unit_name.clone(),
                    vec![EquationError {
                        start: 0,
                        end: 0,
                        code: ErrorCode::DuplicateUnit,
                    }],
                ));
            } else {
                ctx.units.insert(unit_name.clone(), unit_components);
            }
        }

        // TODO: we shouldn't discard the whole context if there are errors
        if unit_errors.is_empty() {
            Ok(ctx)
        } else {
            // for (id, errors) in unit_errors.iter() {
            //     eprintln!("unit errors for '{}'", id);
            //     for err in errors.iter() {
            //         eprintln!("    {}", err);
            //     }
            // }
            Err(unit_errors)
        }
    }

    pub(crate) fn lookup(&self, ident: &str) -> Option<&UnitMap> {
        // first, see if this identifier is an alias of a better-known unit
        let normalized = self.aliases.get(ident).map(|s| s.as_str()).unwrap_or(ident);
        self.units.get(normalized)
    }
}

#[allow(dead_code)]
fn const_int_eval(ast: &Expr0) -> EquationResult<i32> {
    match ast {
        Expr0::Const(_, n, loc) => {
            if approx_eq!(f64, *n, n.round()) {
                Ok(n.round() as i32)
            } else {
                eqn_err!(ExpectedInteger, loc.start, loc.end)
            }
        }
        Expr0::Var(_, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr0::App(_, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr0::Subscript(_, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr0::Op1(op, expr, _) => {
            let expr = const_int_eval(expr)?;
            let result = match op {
                UnaryOp::Positive => expr,
                UnaryOp::Negative => -expr,
                UnaryOp::Not => i32::from(expr == 0),
            };
            Ok(result)
        }
        Expr0::Op2(op, l, r, _) => {
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
        Expr0::If(_, _, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
    }
}

fn build_unit_components(ctx: &Context, ast: &Expr0) -> EquationResult<UnitMap> {
    let unit_map: UnitMap = match ast {
        Expr0::Const(_, _, loc) => {
            // dimensionless is special
            if let Ok(1) = const_int_eval(ast) {
                UnitMap::new()
            } else {
                // nothing to do here (handled below in Op2)
                return eqn_err!(NoConstInUnits, loc.start, loc.end);
            }
        }
        Expr0::Var(id, _) => {
            let id = ctx.aliases.get(id).unwrap_or(id);
            if id == "dmnl" || id == "nil" || id == "dimensionless" || id == "fraction" {
                // dimensionless is special
                UnitMap::new()
            } else {
                ctx.lookup(id)
                    .cloned()
                    .unwrap_or_else(|| [(id.to_owned(), 1)].iter().cloned().collect())
            }
        }
        Expr0::App(_, loc) => {
            return eqn_err!(NoAppInUnits, loc.start, loc.end);
        }
        Expr0::Subscript(_, _, loc) => {
            return eqn_err!(NoSubscriptInUnits, loc.start, loc.end);
        }
        Expr0::Op1(_, _, loc) => {
            return eqn_err!(NoUnaryOpInUnits, loc.start, loc.end);
        }
        Expr0::Op2(op, l, r, loc) => match op {
            BinaryOp::Exp => {
                let exp = const_int_eval(r)?;
                build_unit_components(ctx, l)?.exp(exp)
            }
            BinaryOp::Mul => build_unit_components(ctx, l)? * build_unit_components(ctx, r)?,
            BinaryOp::Div => {
                // check first for the reciprocal case -- 1/blah
                if let Ok(i) = const_int_eval(l) {
                    if i != 1 {
                        let loc = l.get_loc();
                        return eqn_err!(ExpectedIntegerOne, loc.start, loc.end);
                    }
                    build_unit_components(ctx, r)?.reciprocal()
                } else {
                    build_unit_components(ctx, l)? / build_unit_components(ctx, r)?
                }
            }
            _ => {
                return eqn_err!(BadBinaryOpInUnits, loc.start, loc.end);
            }
        },
        Expr0::If(_, _, _, loc) => {
            return eqn_err!(NoIfInUnits, loc.start, loc.end);
        }
    };

    Ok(unit_map)
}

pub fn parse_units(
    ctx: &Context,
    unit_eqn: Option<&str>,
) -> StdResult<Option<UnitMap>, Vec<UnitError>> {
    if let Some(unit_eqn) = unit_eqn {
        if let Some(expr) = Expr0::new(unit_eqn, LexerType::Units).map_err(|errors| {
            errors
                .into_iter()
                .map(|err| UnitError::DefinitionError(err, None))
                .collect::<Vec<UnitError>>()
        })? {
            let result = build_unit_components(ctx, &expr)
                .map_err(|err| vec![UnitError::DefinitionError(err, None)])?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

#[test]
fn test_pretty_print_unit() {
    let context = Context::new(
        &[
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
        ],
        &Default::default(),
    )
    .unwrap();

    let positive_cases: &[(&str, &str); 9] = &[
        ("m^2/s", "meter^2/second"),
        ("person * people * persons", "people^3"),
        ("m^2/meters", "meter"),
        ("m*people/time", "meter*people/time"),
        ("time * people / time", "people"),
        ("1", "dmnl"),
        ("1/dmnl", "dmnl"),
        ("1/s", "1/second"),
        ("1/s/m", "1/meter/second"),
    ];

    for (input, output) in positive_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        let result = build_unit_components(&context, &expr).unwrap();
        let pretty = result.pretty_print();
        assert_eq!(*output, pretty);
    }
}

// we have 3 problems here: the first (and simpler) is evaluating unit equations and turning them in to UnitMaps (done)
// the second is: given a context of unitmaps, can we _check_ the types of variables.  This won't work if there are builtins in use.
// the third is: if we only have _some_ units filled in, can we _infer_ the rest? This will also enable units for builtins

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
        sim_specs: Default::default(),
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

    assert_eq!(
        expected,
        Context::new(simple_units, &Default::default()).unwrap()
    );

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
        sim_specs: Default::default(),
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

    assert_eq!(
        expected2,
        Context::new(more_units, &Default::default()).unwrap()
    );
}

#[test]
fn test_basic_unit_parsing() {
    let context = Context::new(
        &[
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
        ],
        &Default::default(),
    )
    .unwrap();

    let positive_cases: &[(&str, UnitMap); 6] = &[
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
        ("1", UnitMap::new()),
        ("dmnl", UnitMap::new()),
    ];

    for (input, output) in positive_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
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
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        let result = build_unit_components(&context, &expr).unwrap_err();
        assert_eq!(*output, result.code);
    }
}

#[test]
fn test_basic_unit_checks() {
    let _context = Context::new(
        &[
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
        ],
        &Default::default(),
    )
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
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        assert_eq!(*output, const_int_eval(&expr).unwrap());
    }

    use crate::common::ErrorCode;

    let negative_cases = &["3.5", "foo", "if 1 then 2 else 3", "bar[2]", "foo(1, 2)"];

    for input in negative_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        assert_eq!(
            ErrorCode::ExpectedInteger,
            const_int_eval(&expr).unwrap_err().code
        );
    }
}
