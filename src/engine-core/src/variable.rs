// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashSet;
use std::rc::Rc;

use lalrpop_util::ParseError;

use crate::ast::{self, Expr, Visitor};
use crate::common::{canonicalize, Ident, VariableError};
use crate::xmile;

#[derive(Clone, PartialEq, Debug)]
pub struct Table {
    x: Vec<f64>,
    y: Vec<f64>,
    x_range: Option<(f64, f64)>,
    y_range: Option<(f64, f64)>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Variable {
    Stock {
        ident: Ident,
        ast: Option<Rc<ast::Expr>>,
        eqn: Option<String>,
        units: Option<String>,
        inflows: Vec<Ident>,
        outflows: Vec<Ident>,
        non_negative: bool,
        errors: Vec<VariableError>,
        direct_deps: HashSet<Ident>,
    },
    Var {
        ident: Ident,
        ast: Option<Rc<ast::Expr>>,
        eqn: Option<String>,
        units: Option<String>,
        table: Option<Table>,
        non_negative: bool,
        is_flow: bool,
        is_table_only: bool,
        errors: Vec<VariableError>,
        direct_deps: HashSet<Ident>,
    },
    Module {
        ident: Ident,
        units: Option<String>,
        refs: Vec<xmile::Ref>,
        errors: Vec<VariableError>,
        direct_deps: HashSet<Ident>,
    },
}

impl Variable {
    pub fn ident(&self) -> &Ident {
        match self {
            Variable::Stock { ident: name, .. } => name,
            Variable::Var { ident: name, .. } => name,
            Variable::Module { ident: name, .. } => name,
        }
    }

    pub fn eqn(&self) -> Option<&String> {
        match self {
            Variable::Stock { eqn: Some(s), .. } => Some(s),
            Variable::Var { eqn: Some(s), .. } => Some(s),
            _ => None,
        }
    }

    pub fn is_stock(&self) -> bool {
        match self {
            Variable::Stock { .. } => true,
            _ => false,
        }
    }

    pub fn direct_deps(&self) -> &HashSet<Ident> {
        match self {
            Variable::Stock {
                direct_deps: deps, ..
            } => deps,
            Variable::Var {
                direct_deps: deps, ..
            } => deps,
            Variable::Module {
                direct_deps: deps, ..
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
            let direct_deps = match &ast {
                Some(ast) => identifier_set(ast),
                None => HashSet::new(),
            };
            let inflows = match &v.inflows {
                None => Vec::new(),
                Some(inflows) => inflows.iter().map(|id| canonicalize(id)).collect(),
            };
            let outflows = match &v.outflows {
                None => Vec::new(),
                Some(outflows) => outflows.iter().map(|id| canonicalize(id)).collect(),
            };
            Variable::Stock {
                ident: canonicalize(v.name.as_ref()),
                ast,
                eqn: v.eqn.clone(),
                units: v.units.clone(),
                inflows,
                outflows,
                non_negative: v.non_negative.is_some(),
                errors,
                direct_deps,
            }
        }
        xmile::Var::Flow(v) => {
            let (ast, errors) = parse_eqn(&v.eqn);
            let direct_deps = match &ast {
                Some(ast) => identifier_set(ast),
                None => HashSet::new(),
            };
            Variable::Var {
                ident: canonicalize(v.name.as_ref()),
                ast,
                eqn: v.eqn.clone(),
                units: v.units.clone(),
                table: None,
                is_flow: true,
                is_table_only: false,
                non_negative: v.non_negative.is_some(),
                errors,
                direct_deps,
            }
        }
        xmile::Var::Aux(v) => {
            let (ast, errors) = parse_eqn(&v.eqn);
            let direct_deps = match &ast {
                Some(ast) => identifier_set(ast),
                None => HashSet::new(),
            };
            Variable::Var {
                ident: canonicalize(v.name.as_ref()),
                ast,
                eqn: v.eqn.clone(),
                units: v.units.clone(),
                table: None,
                is_flow: false,
                is_table_only: false,
                non_negative: false,
                errors,
                direct_deps,
            }
        }
        xmile::Var::Module(v) => Variable::Module {
            ident: canonicalize(v.name.as_ref()),
            units: v.units.clone(),
            refs: v.refs.clone().unwrap_or_default(),
            errors: Vec::new(),
            direct_deps: match &v.refs {
                Some(refs) => refs.iter().map(|r| r.src.clone()).collect(),
                None => HashSet::new(),
            },
        },
    }
}

struct IdentifierSetVisitor {
    identifiers: HashSet<Ident>,
}

impl Visitor<()> for IdentifierSetVisitor {
    fn walk(&mut self, e: &Expr) {
        match e {
            Expr::Const(_, _) => (),
            Expr::Var(id) => {
                self.identifiers.insert(id.clone());
            }
            Expr::App(func, args) => {
                self.identifiers.insert(func.clone());
                for arg in args.iter() {
                    self.walk(arg);
                }
            }
            Expr::Op2(_, l, r) => {
                self.walk(l);
                self.walk(r);
            }
            Expr::Op1(_, l) => {
                self.walk(l);
            }
            Expr::If(cond, t, f) => {
                self.walk(cond);
                self.walk(t);
                self.walk(f);
            }
        }
    }
}

pub fn identifier_set(e: &Expr) -> HashSet<Ident> {
    let mut id_visitor = IdentifierSetVisitor {
        identifiers: HashSet::new(),
    };
    id_visitor.walk(e);
    id_visitor.identifiers
}

#[test]
fn test_identifier_sets() {
    let cases: &[(&str, &[&str])] = &[
        ("if a then b else c", &["a", "b", "c"]),
        ("a(1, b, c)", &["a", "b", "c"]),
        ("-(a)", &["a"]),
        ("if a = 1 then -c else c(1, d, b)", &["a", "b", "c", "d"]),
        ("if a.d then b else c", &["a.d", "b", "c"]),
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

    let if4 = Rc::new(If(
        Rc::new(Op2(
            And,
            Rc::new(Var("true_input".to_string())),
            Rc::new(Var("false_input".to_string())),
        )),
        Rc::new(Const("1".to_string(), 1.0)),
        Rc::new(Const("0".to_string(), 0.0)),
    ));

    let quoting_eq = Rc::new(Op2(
        Eq,
        Rc::new(Var("oh_dear".to_string())),
        Rc::new(Var("oh_dear".to_string())),
    ));

    let cases = [
        ("if 1 then 2 else 3", if1),
        ("if blerg = foo then 2 else 3", if2),
        ("IF quotient = quotient_target THEN 1 ELSE 0", if3.clone()),
        ("(IF quotient = quotient_target THEN 1 ELSE 0)", if3.clone()),
        (
            "( IF true_input and false_input THEN 1 ELSE 0 )",
            if4.clone(),
        ),
        ("\"oh dear\" = oh_dear", quoting_eq.clone()),
    ];

    for case in cases.iter() {
        let eqn = case.0;
        let (ast, err) = parse_eqn(&Some(eqn.to_string()));
        assert_eq!(err.len(), 0);
        assert!(ast.is_some());
        assert_eq!(case.1, ast.unwrap());
    }
}

#[test]
fn test_canonicalize_stock_inflows() {
    use std::iter::FromIterator;

    let input = xmile::Var::Stock(xmile::Stock {
        name: "Heat Loss To Room".to_string(),
        eqn: Some("total_population".to_string()),
        doc: Some("People who can contract the disease.".to_string()),
        units: Some("people".to_string()),
        inflows: Some(vec!["\"Solar Radiation\"".to_string()]),
        outflows: Some(vec![
            "\"succumbing\"".to_string(),
            "\"succumbing 2\"".to_string(),
        ]),
        non_negative: None,
        dimensions: None,
    });

    let expected = Variable::Stock {
        ident: "heat_loss_to_room".to_string(),
        ast: Some(Rc::new(Expr::Var("total_population".to_string()))),
        eqn: Some("total_population".to_string()),
        units: Some("people".to_string()),
        inflows: vec!["solar_radiation".to_string()],
        outflows: vec!["succumbing".to_string(), "succumbing_2".to_string()],
        non_negative: false,
        errors: vec![],
        direct_deps: HashSet::from_iter(["total_population".to_string()].iter().cloned()),
    };

    let output = parse_var(&input);

    assert_eq!(expected, output);
}
