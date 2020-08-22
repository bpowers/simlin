// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashSet;
use std::rc::Rc;

use lalrpop_util::ParseError;

use crate::ast::{self, Visitor, Expr};
use crate::common::{canonicalize, Ident, VariableError};
use crate::xmile;

#[derive(Debug)]
pub struct Table {
    x: Vec<f64>,
    y: Vec<f64>,
    x_range: Option<(f64, f64)>,
    y_range: Option<(f64, f64)>,
}

#[derive(Debug)]
pub enum Variable {
    Stock {
        name: String,
        ast: Option<Rc<ast::Expr>>,
        eqn: Option<String>,
        units: Option<String>,
        inflows: Vec<String>,
        outflows: Vec<String>,
        non_negative: bool,
        errors: Vec<VariableError>,
        dependencies: Vec<String>,
    },
    Var {
        name: String,
        ast: Option<Rc<ast::Expr>>,
        eqn: Option<String>,
        units: Option<String>,
        table: Option<Table>,
        non_negative: bool,
        is_flow: bool,
        is_table_only: bool,
        errors: Vec<VariableError>,
        dependencies: Vec<String>,
    },
    Module {
        name: String,
        units: Option<String>,
        refs: Vec<xmile::Ref>,
        errors: Vec<VariableError>,
        dependencies: Vec<String>,
    },
}

impl Variable {
    pub fn name(&self) -> &String {
        match self {
            Variable::Stock { name, .. } => name,
            Variable::Var { name, .. } => name,
            Variable::Module { name, .. } => name,
        }
    }

    pub fn eqn(&self) -> Option<&String> {
        match self {
            Variable::Stock { eqn: Some(s), .. } => Some(s),
            Variable::Var { eqn: Some(s), .. } => Some(s),
            _ => None,
        }
    }

    pub fn deps(&self) -> &Vec<String> {
        match self {
            Variable::Stock {
                dependencies: deps, ..
            } => deps,
            Variable::Var {
                dependencies: deps, ..
            } => deps,
            Variable::Module {
                dependencies: deps, ..
            } => deps,
        }
    }

    pub fn errors(&self) -> Option<&Vec<VariableError>> {
        let errors = match self {
            Variable::Stock { errors: e, .. } => e,
            Variable::Var { errors: e, .. } => e,
            Variable::Module { errors: e, .. } => e,
        };

        if errors.is_empty() {
            return None;
        }

        Some(errors)
    }

    fn add_error(&mut self, err: VariableError) {
        match self {
            Variable::Stock { errors: e, .. } => e.push(err),
            Variable::Var { errors: e, .. } => e.push(err),
            Variable::Module { errors: e, .. } => e.push(err),
        };
    }
}

fn parse_eqn(eqn: &Option<String>) -> (Option<Rc<ast::Expr>>, Vec<VariableError>) {
    let mut errs = Vec::new();

    if eqn.is_none() {
        return (None, errs);
    }

    let eqn_string = eqn.as_ref().unwrap();
    let eqn = eqn_string.as_str();
    let lexer = crate::token::Lexer::new(eqn);
    match crate::equation::EquationParser::new().parse(eqn, lexer) {
        Ok(ast) => (Some(ast), errs),
        Err(err) => {
            use crate::common::ErrorCode::*;
            let err = match err {
                ParseError::InvalidToken { location: l } => VariableError {
                    location: l,
                    code: InvalidToken,
                },
                ParseError::UnrecognizedEOF {
                    location: l,
                    expected: e,
                } => {
                    // if we get an EOF at position 0, that simply means
                    // we have an empty (or comment-only) equation
                    if l == 0 {
                        return (None, errs);
                    }
                    eprintln!("unrecognized eof, expected: {:?}", e);
                    VariableError {
                        location: l,
                        code: UnrecognizedEOF,
                    }
                }
                ParseError::UnrecognizedToken {
                    token: (l, t, r), ..
                } => {
                    eprintln!("unrecognized tok: {:?} {} {}", t, l, r);
                    VariableError {
                        location: l,
                        code: UnrecognizedToken,
                    }
                }
                ParseError::ExtraToken { .. } => VariableError {
                    location: eqn.len(),
                    code: ExtraToken,
                },
                ParseError::User { error: e } => e,
            };

            errs.push(err);

            (None, errs)
        }
    }
}

pub fn parse_var(v: &xmile::Var) -> Variable {
    match v {
        xmile::Var::Stock(v) => {
            let (ast, errors) = parse_eqn(&v.eqn);
            Variable::Stock {
                name: canonicalize(v.name.as_ref()),
                ast,
                eqn: v.eqn.clone(),
                units: v.units.clone(),
                inflows: v.inflows.clone().unwrap_or_default(),
                outflows: v.outflows.clone().unwrap_or_default(),
                non_negative: v.non_negative.is_some(),
                errors,
                dependencies: Vec::new(),
            }
        }
        xmile::Var::Flow(v) => {
            let (ast, errors) = parse_eqn(&v.eqn);
            Variable::Var {
                name: canonicalize(v.name.as_ref()),
                ast,
                eqn: v.eqn.clone(),
                units: v.units.clone(),
                table: None,
                is_flow: true,
                is_table_only: false,
                non_negative: v.non_negative.is_some(),
                errors,
                dependencies: Vec::new(),
            }
        }
        xmile::Var::Aux(v) => {
            let (ast, errors) = parse_eqn(&v.eqn);
            Variable::Var {
                name: canonicalize(v.name.as_ref()),
                ast,
                eqn: v.eqn.clone(),
                units: v.units.clone(),
                table: None,
                is_flow: false,
                is_table_only: false,
                non_negative: false,
                errors,
                dependencies: Vec::new(),
            }
        }
        xmile::Var::Module(v) => Variable::Module {
            name: canonicalize(v.name.as_ref()),
            units: v.units.clone(),
            refs: v.refs.clone().unwrap_or_default(),
            errors: Vec::new(),
            dependencies: Vec::new(),
        },
    }
}

struct IdentifierSetVisitor {
    identifiers: HashSet<Ident>,
}

impl Visitor<()> for IdentifierSetVisitor {
    fn visit_const(&mut self, _: &Expr) -> () {}
    fn visit_var(&mut self, e: &Expr) -> () {
        if let Expr::Var(id) = e {
            self.identifiers.insert(id.clone());
        }
    }
    fn visit_app(&mut self, e: &Expr) -> () {
        if let Expr::App(func, args) = e {
            self.identifiers.insert(func.clone());
            for arg in args.iter() {
                arg.walk(self);
            }
        }
    }
    fn visit_op2(&mut self, e: &Expr) -> () {
        if let Expr::Op2(_, l, r) = e {
            l.walk(self);
            r.walk(self);
        }
    }
    fn visit_op1(&mut self, e: &Expr) -> () {
        if let Expr::Op1(_, l) = e {
            l.walk(self);
        }
    }
    fn visit_if(&mut self, e: &Expr) -> () {
        if let Expr::If(cond, t, f) = e {
            cond.walk(self);
            t.walk(self);
            f.walk(self);
        }
    }
}

pub fn identifier_set(e: &Expr) -> HashSet<Ident> {
    let mut id_visitor = IdentifierSetVisitor{
        identifiers: HashSet::new(),
    };
    e.walk(&mut id_visitor);

    id_visitor.identifiers
}

#[test]
fn test_identifier_sets() {
    let cases: &[(&str, &[&str])] = &[
        ("if a then b else c", &["a", "b", "c"]),
        ("a(1, b, c)", &["a", "b", "c"]),
        ("-(a)", &["a"]),
        ("if a = 1 then -c else c(1, d, b)", &["a", "b", "c", "d"]),
    ];

    for (eqn, id_list) in cases.iter() {
        let (ast, err) = parse_eqn(&Some(eqn.to_string()));
        assert_eq!(err.len(), 0);
        assert!(ast.is_some());
        let ast = ast.unwrap();
        let id_set_expected: HashSet<Ident> = id_list.into_iter().map(|s| s.to_string()).collect();
        let id_set_test = identifier_set(&ast);
        assert_eq!(id_set_expected, id_set_test);
    }
}

#[test]
fn test_parse() {
    use crate::ast::BinaryOp::*;
    use crate::ast::Expr::*;

    let if1 = Rc::new(If(
        Rc::new(Const("1".to_string(), 1.0)),
        Rc::new(Const("2".to_string(), 2.0)),
        Rc::new(Const("3".to_string(), 3.0)),
    ));

    let if2 = Rc::new(If(
        Rc::new(Op2(
            Eq,
            Rc::new(Var("blerg".to_string())),
            Rc::new(Var("foo".to_string())),
        )),
        Rc::new(Const("2".to_string(), 2.0)),
        Rc::new(Const("3".to_string(), 3.0)),
    ));

    let if3 = Rc::new(If(
        Rc::new(Op2(
            Eq,
            Rc::new(Var("quotient".to_string())),
            Rc::new(Var("quotient_target".to_string())),
        )),
        Rc::new(Const("1".to_string(), 1.0)),
        Rc::new(Const("0".to_string(), 0.0)),
    ));

    let cases = [
        ("if 1 then 2 else 3", if1),
        ("if blerg = foo then 2 else 3", if2),
        ("IF quotient = quotient_target THEN 1 ELSE 0", if3.clone()),
        ("(IF quotient = quotient_target THEN 1 ELSE 0)", if3.clone()),
    ];

    for case in cases.iter() {
        let eqn = case.0;
        let (ast, err) = parse_eqn(&Some(eqn.to_string()));
        assert_eq!(err.len(), 0);
        assert!(ast.is_some());
        assert_eq!(case.1, ast.unwrap());
    }
}
