// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};

use lalrpop_util::ParseError;

use crate::ast::{self, Expr, Visitor, AST};
use crate::builtins_visitor::instantiate_implicit_modules;
use crate::common::{EquationError, EquationResult, Ident};
use crate::datamodel;
use crate::model::resolve_relative;

#[derive(Clone, PartialEq, Debug)]
pub struct Table {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    x_range: datamodel::GraphicalFunctionScale,
    y_range: datamodel::GraphicalFunctionScale,
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
        ast: Option<AST>,
        eqn: Option<datamodel::Equation>,
        units: Option<String>,
        inflows: Vec<Ident>,
        outflows: Vec<Ident>,
        non_negative: bool,
        errors: Vec<EquationError>,
        direct_deps: HashSet<Ident>,
    },
    Var {
        ident: Ident,
        ast: Option<AST>,
        eqn: Option<datamodel::Equation>,
        units: Option<String>,
        table: Option<Table>,
        non_negative: bool,
        is_flow: bool,
        is_table_only: bool,
        errors: Vec<EquationError>,
        direct_deps: HashSet<Ident>,
    },
    Module {
        // the current spec has ident == model name
        ident: Ident,
        model_name: Ident,
        units: Option<String>,
        inputs: Vec<ModuleInput>,
        errors: Vec<EquationError>,
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
            Variable::Stock {
                eqn: Some(datamodel::Equation::Scalar(s)),
                ..
            } => Some(s),
            Variable::Var {
                eqn: Some(datamodel::Equation::Scalar(s)),
                ..
            } => Some(s),
            _ => None,
        }
    }

    pub fn is_stock(&self) -> bool {
        matches!(self, Variable::Stock { .. })
    }

    pub fn is_module(&self) -> bool {
        matches!(self, Variable::Module { .. })
    }

    pub fn direct_deps(&self) -> &HashSet<Ident> {
        match self {
            Variable::Stock { direct_deps, .. } => direct_deps,
            Variable::Var { direct_deps, .. } => direct_deps,
            Variable::Module { direct_deps, .. } => direct_deps,
        }
    }

    pub fn errors(&self) -> Option<&Vec<EquationError>> {
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

fn parse_table(gf: &Option<datamodel::GraphicalFunction>) -> EquationResult<Option<Table>> {
    if gf.is_none() {
        return Ok(None);
    }
    let gf = gf.as_ref().unwrap();

    let x: Vec<f64> = match &gf.x_points {
        Some(x_points) => x_points.clone(),
        None => {
            let x_min = gf.x_scale.min;
            let x_max = gf.x_scale.max;
            let size = gf.y_points.len() as f64;
            gf.y_points
                .iter()
                .enumerate()
                .map(|(i, _)| ((i as f64) / (size - 1.0)) * (x_max - x_min) + x_min)
                .collect()
        }
    };

    Ok(Some(Table {
        x,
        y: gf.y_points.clone(),
        x_range: gf.x_scale.clone(),
        y_range: gf.y_scale.clone(),
    }))
}
fn parse_equation(eqn: &datamodel::Equation) -> (Option<AST>, Vec<EquationError>) {
    match eqn {
        datamodel::Equation::Scalar(eqn) => {
            let (ast, errors) = parse_single_equation(eqn);
            (ast.map(AST::Scalar), errors)
        }
        datamodel::Equation::ApplyToAll(dimensions, eqn) => {
            let (ast, errors) = parse_single_equation(eqn);
            (
                ast.map(|ast| AST::ApplyToAll(dimensions.clone(), ast)),
                errors,
            )
        }
        datamodel::Equation::Arrayed(dimensions, elements) => {
            let mut errors: Vec<EquationError> = vec![];
            let elements: Vec<_> = elements
                .iter()
                .map(|(subscript, equation)| {
                    let (ast, single_errors) = parse_single_equation(equation);
                    errors.extend(single_errors);
                    (subscript.clone(), ast)
                })
                .filter(|(_, ast)| ast.is_some())
                .map(|(subscript, ast)| (subscript, ast.unwrap()))
                .collect();

            (Some(AST::Arrayed(dimensions.clone(), elements)), errors)
        }
    }
}

fn parse_single_equation(eqn: &str) -> (Option<ast::Expr>, Vec<EquationError>) {
    let mut errs = Vec::new();

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
    models: &HashMap<String, HashMap<Ident, &datamodel::Variable>>,
    model_name: &str,
    v: &datamodel::Variable,
    implicit_vars: &mut Vec<datamodel::Variable>,
) -> Variable {
    let mut parse_and_lower_eqn = |ident: &str,
                                   eqn: &datamodel::Equation|
     -> (Option<AST>, HashSet<Ident>, Vec<EquationError>) {
        let (ast, mut errors) = parse_equation(eqn);
        let ast = match ast {
            Some(ast) => match instantiate_implicit_modules(ident, ast) {
                Ok((ast, mut new_vars)) => {
                    implicit_vars.append(&mut new_vars);
                    Some(ast)
                }
                Err(err) => {
                    errors.push(err);
                    None
                }
            },
            None => None,
        };
        let direct_deps = match &ast {
            Some(ast) => identifier_set(ast),
            None => HashSet::new(),
        };

        (ast, direct_deps, errors)
    };
    match v {
        datamodel::Variable::Stock(v) => {
            let ident = v.ident.clone();
            let (ast, direct_deps, errors) = parse_and_lower_eqn(&ident, &v.equation);
            Variable::Stock {
                ident,
                ast,
                eqn: Some(v.equation.clone()),
                units: v.units.clone(),
                inflows: v.inflows.clone(),
                outflows: v.outflows.clone(),
                non_negative: v.non_negative,
                errors,
                direct_deps,
            }
        }
        datamodel::Variable::Flow(v) => {
            let ident = v.ident.clone();
            let (ast, direct_deps, errors) = parse_and_lower_eqn(&ident, &v.equation);
            let mut errors: Vec<EquationError> = errors;
            let table = match parse_table(&v.gf) {
                Ok(table) => table,
                Err(err) => {
                    errors.push(err);
                    None
                }
            };
            Variable::Var {
                ident,
                ast,
                eqn: Some(v.equation.clone()),
                units: v.units.clone(),
                table,
                is_flow: true,
                is_table_only: false,
                non_negative: v.non_negative,
                errors,
                direct_deps,
            }
        }
        datamodel::Variable::Aux(v) => {
            let ident = v.ident.clone();
            let (ast, direct_deps, errors) = parse_and_lower_eqn(&ident, &v.equation);
            let mut errors: Vec<EquationError> = errors;
            let table = match parse_table(&v.gf) {
                Ok(table) => table,
                Err(err) => {
                    errors.push(err);
                    None
                }
            };
            Variable::Var {
                ident,
                ast,
                eqn: Some(v.equation.clone()),
                units: v.units.clone(),
                table,
                is_flow: false,
                is_table_only: false,
                non_negative: false,
                errors,
                direct_deps,
            }
        }
        datamodel::Variable::Module(v) => {
            let ident = v.ident.clone();
            let inputs: Vec<EquationResult<ModuleInput>> = v
                .references
                .iter()
                .map(|mi| {
                    crate::model::resolve_module_input(models, model_name, &ident, &mi.src, &mi.dst)
                })
                .collect();
            let (inputs, errors): (Vec<_>, Vec<_>) =
                inputs.into_iter().partition(EquationResult::is_ok);
            let inputs: Vec<ModuleInput> = inputs.into_iter().map(|i| i.unwrap()).collect();
            let errors: Vec<EquationError> = errors.into_iter().map(|e| e.unwrap_err()).collect();

            let direct_deps = inputs
                .iter()
                .map(|r| {
                    let src = &r.src;
                    let direct_dep = match src.find('.') {
                        Some(pos) => &src[..pos],
                        None => src,
                    };

                    let src = resolve_relative(models, model_name, src);
                    // will be none if this is a temporary we created
                    if let Some(src) = src {
                        if let datamodel::Variable::Stock(_) = src {
                            // if our input is a stock, we don't have any flow dependencies to
                            // order before us this dt
                            None
                        } else {
                            Some(direct_dep.to_string())
                        }
                    } else {
                        Some(direct_dep.to_string())
                    }
                })
                .filter(|d| d.is_some())
                .map(|d| d.unwrap())
                .collect();
            Variable::Module {
                model_name: v.model_name.clone(),
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
            Expr::Subscript(id, args) => {
                self.identifiers.insert(id.clone());
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

pub fn identifier_set(ast: &AST) -> HashSet<Ident> {
    let mut id_visitor = IdentifierSetVisitor {
        identifiers: HashSet::new(),
    };
    match ast {
        AST::Scalar(ast) => id_visitor.walk(ast),
        AST::ApplyToAll(_, ast) => id_visitor.walk(ast),
        AST::Arrayed(_, elements) => {
            for (_, ast) in elements {
                id_visitor.walk(ast);
            }
        }
    };
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
        let (ast, err) = parse_equation(&datamodel::Equation::Scalar((*eqn).to_owned()));
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

    let subscript1 = Box::new(Subscript("a".to_owned(), vec![Const("1".to_owned(), 1.0)]));
    let subscript2 = Box::new(Subscript(
        "a".to_owned(),
        vec![
            Const("2".to_owned(), 2.0),
            App("int".to_owned(), vec![Var("b".to_owned())]),
        ],
    ));

    use crate::ast::print_eqn;

    let cases = [
        ("if 1 then 2 else 3", if1, "if (1) then (2) else (3)"),
        (
            "if blerg = foo then 2 else 3",
            if2,
            "if ((blerg = foo)) then (2) else (3)",
        ),
        (
            "IF quotient = quotient_target THEN 1 ELSE 0",
            if3.clone(),
            "if ((quotient = quotient_target)) then (1) else (0)",
        ),
        (
            "(IF quotient = quotient_target THEN 1 ELSE 0)",
            if3.clone(),
            "if ((quotient = quotient_target)) then (1) else (0)",
        ),
        (
            "( IF true_input and false_input THEN 1 ELSE 0 )",
            if4.clone(),
            "if ((true_input && false_input)) then (1) else (0)",
        ),
        (
            "( IF true_input && false_input THEN 1 ELSE 0 )",
            if4.clone(),
            "if ((true_input && false_input)) then (1) else (0)",
        ),
        (
            "\"oh dear\" = oh_dear",
            quoting_eq.clone(),
            "(oh_dear = oh_dear)",
        ),
        ("a[1]", subscript1.clone(), "a[1]"),
        ("a[2, INT(b)]", subscript2.clone(), "a[2, int(b)]"),
    ];

    for case in cases.iter() {
        let eqn = case.0;
        let (ast, err) = parse_single_equation(eqn);
        assert_eq!(err.len(), 0);
        assert!(ast.is_some());
        let ast = ast.unwrap();
        assert_eq!(&*case.1, &ast);
        let printed = print_eqn(&ast);
        assert_eq!(case.2, &printed);
    }

    let (ast, err) = parse_single_equation("NAN");
    assert_eq!(err.len(), 0);
    assert!(ast.is_some());
    let ast = ast.unwrap();
    assert!(matches!(&ast, Expr::Const(_, _)));
    if let Expr::Const(id, n) = &ast {
        assert_eq!("NaN", id);
        assert!(n.is_nan());
    }
    let printed = print_eqn(&ast);
    assert_eq!("NaN", &printed);
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
        let (ast, err) = parse_single_equation(case);
        assert!(ast.is_none());
        assert!(err.len() > 0);
    }
}

#[test]
fn test_tables() {
    use crate::common::canonicalize;
    let input = datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize("lookup function table"),
        equation: datamodel::Equation::Scalar("0".to_string()),
        documentation: "".to_string(),
        units: None,
        gf: Some(datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_scale: datamodel::GraphicalFunctionScale {
                min: 0.0,
                max: 45.0,
            },
            y_scale: datamodel::GraphicalFunctionScale {
                min: -1.0,
                max: 1.0,
            },
            x_points: None,
            y_points: vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, -1.0, -1.0, 0.0, 0.0],
        }),
    });

    let expected = Variable::Var {
        ident: "lookup_function_table".to_string(),
        ast: Some(AST::Scalar(Expr::Const("0".to_string(), 0.0))),
        eqn: Some(datamodel::Equation::Scalar("0".to_string())),
        units: None,
        table: Some(Table {
            x: vec![0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0],
            y: vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, -1.0, -1.0, 0.0, 0.0],
            x_range: datamodel::GraphicalFunctionScale {
                min: 0.0,
                max: 45.0,
            },
            y_range: datamodel::GraphicalFunctionScale {
                min: -1.0,
                max: 1.0,
            },
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

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let output = parse_var(&HashMap::new(), "main", &input, &mut implicit_vars);

    assert_eq!(expected, output);
}
