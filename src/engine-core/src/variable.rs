// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};

use lalrpop_util::ParseError;

use crate::ast::{self, Expr, Visitor};
use crate::common::{canonicalize, EquationError, Error, Ident, Result};
use crate::xmile;

#[derive(Clone, PartialEq, Debug)]
pub struct Table {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    x_range: Option<(f64, f64)>,
    y_range: Option<(f64, f64)>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ModuleInput {
    // the Variable identifier in the current model we will use for input
    pub src: Ident,
    // the Variable identifier in the module's model we will override
    pub dst: Ident,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Variable {
    Stock {
        ident: Ident,
        ast: Option<Box<ast::Expr>>,
        eqn: Option<String>,
        units: Option<String>,
        inflows: Vec<Ident>,
        outflows: Vec<Ident>,
        non_negative: bool,
        errors: Vec<Error>,
        direct_deps: HashSet<Ident>,
    },
    Var {
        ident: Ident,
        ast: Option<Box<ast::Expr>>,
        eqn: Option<String>,
        units: Option<String>,
        table: Option<Table>,
        non_negative: bool,
        is_flow: bool,
        is_table_only: bool,
        errors: Vec<Error>,
        direct_deps: HashSet<Ident>,
    },
    Module {
        // the current spec has ident == model name
        ident: Ident,
        units: Option<String>,
        inputs: Vec<ModuleInput>,
        errors: Vec<Error>,
        direct_deps: HashSet<Ident>,
    },
}

impl Variable {
    pub fn ident(&self) -> &str {
        match self {
            Variable::Stock { ident: name, .. } => name.as_str(),
            Variable::Var { ident: name, .. } => name.as_str(),
            Variable::Module { ident: name, .. } => name.as_str(),
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

    pub fn is_module(&self) -> bool {
        match self {
            Variable::Module { .. } => true,
            _ => false,
        }
    }

    pub fn direct_deps(&self) -> &HashSet<Ident> {
        match self {
            Variable::Stock { direct_deps, .. } => direct_deps,
            Variable::Var { direct_deps, .. } => direct_deps,
            Variable::Module { direct_deps, .. } => direct_deps,
        }
    }

    pub fn errors(&self) -> Option<&Vec<Error>> {
        let errors = match self {
            Variable::Stock { errors, .. } => errors,
            Variable::Var { errors, .. } => errors,
            Variable::Module { errors, .. } => errors,
        };

        if errors.is_empty() {
            return None;
        }

        Some(errors)
    }

    pub fn table(&self) -> Option<&Table> {
        match self {
            Variable::Stock { .. } => None,
            Variable::Var { table, .. } => table.as_ref(),
            Variable::Module { .. } => None,
        }
    }
}

fn parse_table(ident: &str, gf: &Option<xmile::GF>) -> Result<Option<Table>> {
    use std::str::FromStr;

    if gf.is_none() {
        return Ok(None);
    }
    let gf = gf.as_ref().unwrap();

    let x_range: Option<(f64, f64)> = gf.x_scale.as_ref().map(|scale| (scale.min, scale.max));
    let y_range: Option<(f64, f64)> = gf.y_scale.as_ref().map(|scale| (scale.min, scale.max));

    let y: std::result::Result<Vec<f64>, _> = match &gf.y_pts {
        None => Ok(vec![]),
        Some(y_pts) => y_pts.split(',').map(|n| f64::from_str(n.trim())).collect(),
    };
    if y.is_err() {
        return var_err!(BadTable, ident.to_string());
    }
    let y = y.unwrap();

    let x: std::result::Result<Vec<f64>, _> = match &gf.x_pts {
        None => {
            if let Some((x_min, x_max)) = x_range {
                let size = y.len() as f64;
                Ok(y.iter()
                    .enumerate()
                    .map(|(i, _)| ((i as f64) / (size - 1.0)) * (x_max - x_min) + x_min)
                    .collect())
            } else {
                Ok(vec![])
            }
        }
        Some(x_pts) => x_pts.split(',').map(|n| f64::from_str(n.trim())).collect(),
    };
    if x.is_err() {
        return var_err!(BadTable, ident.to_string());
    }
    let x = x.unwrap();

    Ok(Some(Table {
        x,
        y,
        x_range,
        y_range,
    }))
}

fn parse_eqn(eqn: &Option<String>) -> (Option<Box<ast::Expr>>, Vec<EquationError>) {
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
                ParseError::InvalidToken { location: l } => EquationError {
                    location: l,
                    code: InvalidToken,
                },
                ParseError::UnrecognizedEOF {
                    location: l,
                    expected: _e,
                } => {
                    // if we get an EOF at position 0, that simply means
                    // we have an empty (or comment-only) equation
                    if l == 0 {
                        return (None, errs);
                    }
                    // TODO: we can give a more precise error message here, including what
                    //   types of tokens would be ok
                    EquationError {
                        location: l,
                        code: UnrecognizedEOF,
                    }
                }
                ParseError::UnrecognizedToken {
                    token: (l, _t, _r), ..
                } => EquationError {
                    location: l,
                    code: UnrecognizedToken,
                },
                ParseError::ExtraToken { .. } => EquationError {
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

pub fn parse_var(
    models: &HashMap<String, HashMap<Ident, &xmile::Var>>,
    model_name: &str,
    v: &xmile::Var,
) -> Variable {
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
            let ident = canonicalize(v.name.as_ref());
            let errors = errors
                .into_iter()
                .map(|e| Error::VariableError(e.code, ident.clone(), Some(e.location)))
                .collect();
            Variable::Stock {
                ident,
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
            let ident = canonicalize(v.name.as_ref());
            let mut errors: Vec<Error> = errors
                .into_iter()
                .map(|e| Error::VariableError(e.code, ident.clone(), Some(e.location)))
                .collect();
            let table = match parse_table(ident.as_str(), &v.gf) {
                Ok(table) => table,
                Err(err) => {
                    errors.push(err);
                    None
                }
            };
            Variable::Var {
                ident,
                ast,
                eqn: v.eqn.clone(),
                units: v.units.clone(),
                table,
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
            let ident = canonicalize(v.name.as_ref());
            let mut errors: Vec<Error> = errors
                .into_iter()
                .map(|e| Error::VariableError(e.code, ident.clone(), Some(e.location)))
                .collect();
            let table = match parse_table(ident.as_str(), &v.gf) {
                Ok(table) => table,
                Err(err) => {
                    errors.push(err);
                    None
                }
            };
            Variable::Var {
                ident,
                ast,
                eqn: v.eqn.clone(),
                units: v.units.clone(),
                table,
                is_flow: false,
                is_table_only: false,
                non_negative: false,
                errors,
                direct_deps,
            }
        }
        xmile::Var::Module(v) => {
            let ident = canonicalize(v.name.as_ref());
            let input_prefix = format!("{}.", ident);
            let inputs: Vec<Result<ModuleInput>> = v
                .refs
                .iter()
                .filter(|r| {
                    if let xmile::Reference::Connect(_) = r {
                        true
                    } else {
                        false
                    }
                })
                .map(|r| {
                    if let xmile::Reference::Connect(r) = r {
                        (r.src.as_str(), r.dst.as_str())
                    } else {
                        unreachable!();
                    }
                })
                .filter(|(_src, dst)| (*dst).starts_with(&input_prefix))
                .map(|(src, dst)| {
                    crate::model::resolve_module_input(models, model_name, &ident, src, dst)
                })
                .collect();
            let (inputs, errors): (Vec<_>, Vec<_>) = inputs.into_iter().partition(Result::is_ok);
            let inputs: Vec<ModuleInput> = inputs.into_iter().map(|i| i.unwrap()).collect();
            let errors: Vec<Error> = errors.into_iter().map(|e| e.unwrap_err()).collect();

            let direct_deps = inputs
                .iter()
                .map(|r| {
                    let src = &r.src;
                    let direct_dep = match src.find('.') {
                        Some(pos) => &src[..pos],
                        None => src,
                    };

                    if let xmile::Var::Stock(_) =
                        crate::model::resolve_relative(models, model_name, src).unwrap()
                    {
                        // if our input is a stock, we don't have any flow dependencies to
                        // order before us this dt
                        None
                    } else {
                        Some(direct_dep.to_string())
                    }
                })
                .filter(|d| d.is_some())
                .map(|d| d.unwrap())
                .collect();
            Variable::Module {
                ident,
                units: v.units.clone(),
                inputs,
                errors,
                direct_deps,
            }
        }
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

    let if1 = Box::new(If(
        Box::new(Const("1".to_string(), 1.0)),
        Box::new(Const("2".to_string(), 2.0)),
        Box::new(Const("3".to_string(), 3.0)),
    ));

    let if2 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var("blerg".to_string())),
            Box::new(Var("foo".to_string())),
        )),
        Box::new(Const("2".to_string(), 2.0)),
        Box::new(Const("3".to_string(), 3.0)),
    ));

    let if3 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var("quotient".to_string())),
            Box::new(Var("quotient_target".to_string())),
        )),
        Box::new(Const("1".to_string(), 1.0)),
        Box::new(Const("0".to_string(), 0.0)),
    ));

    let if4 = Box::new(If(
        Box::new(Op2(
            And,
            Box::new(Var("true_input".to_string())),
            Box::new(Var("false_input".to_string())),
        )),
        Box::new(Const("1".to_string(), 1.0)),
        Box::new(Const("0".to_string(), 0.0)),
    ));

    let quoting_eq = Box::new(Op2(
        Eq,
        Box::new(Var("oh_dear".to_string())),
        Box::new(Var("oh_dear".to_string())),
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
fn test_parse_failures() {
    let failures = &[
        "(",
        "(3",
        "3 +",
        "3 *",
        "(3 +)",
        "call(a,",
        "call(a,1+",
        "if if",
        "if 1 then",
        "if then",
        "if 1 then 2 else",
    ];

    for case in failures {
        let (ast, err) = parse_eqn(&Some(case.to_string()));
        assert!(ast.is_none());
        assert!(err.len() > 0);
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
        ast: Some(Box::new(Expr::Var("total_population".to_string()))),
        eqn: Some("total_population".to_string()),
        units: Some("people".to_string()),
        inflows: vec!["solar_radiation".to_string()],
        outflows: vec!["succumbing".to_string(), "succumbing_2".to_string()],
        non_negative: false,
        errors: vec![],
        direct_deps: HashSet::from_iter(["total_population".to_string()].iter().cloned()),
    };

    let output = parse_var(&HashMap::new(), "main", &input);

    assert_eq!(expected, output);
}

#[test]
fn test_tables() {
    let input = xmile::Var::Aux(xmile::Aux {
        name: "lookup function table".to_string(),
        eqn: Some("0".to_string()),
        doc: None,
        units: None,
        gf: Some(xmile::GF {
            name: None,
            kind: None,
            x_scale: None,
            y_scale: Some(xmile::Scale {
                min: -1.0,
                max: 1.0,
            }),
            x_pts: Some("0,5,10,15,20,25,30,35,40,45".to_string()),
            y_pts: Some("0,0,1,1,0,0,-1,-1,0,0".to_string()),
        }),
        dimensions: None,
    });

    let expected = Variable::Var {
        ident: "lookup_function_table".to_string(),
        ast: Some(Box::new(Expr::Const("0".to_string(), 0.0))),
        eqn: Some("0".to_string()),
        units: None,
        table: Some(Table {
            x: vec![0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0],
            y: vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, -1.0, -1.0, 0.0, 0.0],
            x_range: None,
            y_range: Some((-1.0, 1.0)),
        }),
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        direct_deps: HashSet::new(),
    };

    if let Variable::Var {
        table: Some(table), ..
    } = &expected
    {
        assert_eq!(table.x.len(), table.y.len());
    } else {
        assert!(false);
    }

    let output = parse_var(&HashMap::new(), "main", &input);

    assert_eq!(expected, output);
}
