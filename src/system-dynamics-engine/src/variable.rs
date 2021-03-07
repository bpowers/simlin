// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};

#[cfg(test)]
use crate::ast::Loc;
use crate::ast::{parse_equation as parse_single_equation, Expr, Visitor, AST};
use crate::builtins::{is_builtin_fn, is_builtin_fn_or_time};
use crate::builtins_visitor::instantiate_implicit_modules;
use crate::common::{DimensionName, EquationError, EquationResult, Ident};
use crate::datamodel::Dimension;
use crate::{datamodel, eqn_err, ErrorCode};

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
    },
    Module {
        // the current spec has ident == model name
        ident: Ident,
        model_name: Ident,
        units: Option<String>,
        inputs: Vec<ModuleInput>,
        errors: Vec<EquationError>,
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

    pub fn ast(&self) -> Option<&AST> {
        match self {
            Variable::Stock { ast: Some(ast), .. } => Some(ast),
            Variable::Var { ast: Some(ast), .. } => Some(ast),
            _ => None,
        }
    }

    pub fn scalar_equation(&self) -> Option<&String> {
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

    pub fn get_dimensions(&self) -> Option<&[Dimension]> {
        match self {
            Variable::Stock {
                ast: Some(AST::Arrayed(dims, _)),
                ..
            } => Some(dims),
            Variable::Stock {
                ast: Some(AST::ApplyToAll(dims, _)),
                ..
            } => Some(dims),
            Variable::Var {
                ast: Some(AST::Arrayed(dims, _)),
                ..
            } => Some(dims),
            Variable::Var {
                ast: Some(AST::ApplyToAll(dims, _)),
                ..
            } => Some(dims),
            _ => None,
        }
    }

    pub fn is_stock(&self) -> bool {
        matches!(self, Variable::Stock { .. })
    }

    pub fn is_module(&self) -> bool {
        matches!(self, Variable::Module { .. })
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

    pub fn push_error(&mut self, err: EquationError) {
        match self {
            Variable::Stock { errors, .. } => errors.push(err),
            Variable::Var { errors, .. } => errors.push(err),
            Variable::Module { errors, .. } => errors.push(err),
        }
    }

    pub fn table(&self) -> Option<&Table> {
        match self {
            Variable::Stock { .. } => None,
            Variable::Var { table, .. } => table.as_ref(),
            Variable::Module { .. } => None,
        }
    }
}

#[allow(clippy::unnecessary_wraps)]
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

fn get_dimensions(
    dimensions: &[Dimension],
    names: &[DimensionName],
) -> Result<Vec<datamodel::Dimension>, EquationError> {
    names
        .iter()
        .map(|name| -> Result<datamodel::Dimension, EquationError> {
            for dim in dimensions {
                if dim.name == *name {
                    return Ok(dim.clone());
                }
            }
            eqn_err!(BadDimensionName, 0, 0)
        })
        .collect()
}

fn parse_equation(
    eqn: &datamodel::Equation,
    dimensions: &[Dimension],
) -> (Option<AST>, Vec<EquationError>) {
    match eqn {
        datamodel::Equation::Scalar(eqn) => {
            match parse_single_equation(eqn).map(|eqn| eqn.map(AST::Scalar)) {
                Ok(expr) => (expr, vec![]),
                Err(errors) => (None, errors),
            }
        }
        datamodel::Equation::ApplyToAll(dimension_names, eqn) => {
            let (ast, mut errors) = match parse_single_equation(eqn) {
                Ok(expr) => (expr, vec![]),
                Err(errors) => (None, errors),
            };
            match get_dimensions(dimensions, dimension_names) {
                Ok(dims) => (ast.map(|ast| AST::ApplyToAll(dims, ast)), errors),
                Err(err) => {
                    errors.push(err);
                    (None, errors)
                }
            }
        }
        datamodel::Equation::Arrayed(dimension_names, elements) => {
            let mut errors: Vec<EquationError> = vec![];
            let elements: Vec<_> = elements
                .iter()
                .map(|(subscript, equation)| {
                    let (ast, single_errors) = match parse_single_equation(equation) {
                        Ok(expr) => (expr, vec![]),
                        Err(errors) => (None, errors),
                    };
                    errors.extend(single_errors);
                    (subscript.clone(), ast)
                })
                .filter(|(_, ast)| ast.is_some())
                .map(|(subscript, ast)| (subscript, ast.unwrap()))
                .collect();

            match get_dimensions(dimensions, dimension_names) {
                Ok(dims) => (
                    Some(AST::Arrayed(dims, elements.iter().cloned().collect())),
                    errors,
                ),
                Err(err) => {
                    errors.push(err);
                    (None, errors)
                }
            }
        }
    }
}

pub fn parse_var(
    models: &HashMap<String, HashMap<Ident, &datamodel::Variable>>,
    model_name: &str,
    dimensions: &[Dimension],
    v: &datamodel::Variable,
    implicit_vars: &mut Vec<datamodel::Variable>,
) -> Variable {
    let mut parse_and_lower_eqn =
        |ident: &str, eqn: &datamodel::Equation| -> (Option<AST>, Vec<EquationError>) {
            let (ast, mut errors) = parse_equation(eqn, dimensions);
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
                None => {
                    if errors.is_empty() {
                        errors.push(EquationError {
                            start: 0,
                            end: 0,
                            code: ErrorCode::EmptyEquation,
                        })
                    }
                    None
                }
            };

            (ast, errors)
        };
    match v {
        datamodel::Variable::Stock(v) => {
            let ident = v.ident.clone();
            let (ast, errors) = parse_and_lower_eqn(&ident, &v.equation);
            Variable::Stock {
                ident,
                ast,
                eqn: Some(v.equation.clone()),
                units: v.units.clone(),
                inflows: v.inflows.clone(),
                outflows: v.outflows.clone(),
                non_negative: v.non_negative,
                errors,
            }
        }
        datamodel::Variable::Flow(v) => {
            let ident = v.ident.clone();
            let (ast, errors) = parse_and_lower_eqn(&ident, &v.equation);
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
            }
        }
        datamodel::Variable::Aux(v) => {
            let ident = v.ident.clone();
            let (ast, errors) = parse_and_lower_eqn(&ident, &v.equation);
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
            }
        }
        datamodel::Variable::Module(v) => {
            let ident = v.ident.clone();
            let inputs: Vec<EquationResult<Option<ModuleInput>>> = v
                .references
                .iter()
                .map(|mi| {
                    crate::model::resolve_module_input(models, model_name, &ident, &mi.src, &mi.dst)
                })
                .collect();
            let (inputs, errors): (Vec<_>, Vec<_>) =
                inputs.into_iter().partition(EquationResult::is_ok);
            let inputs: Vec<ModuleInput> = inputs
                .into_iter()
                .map(|i| i.unwrap())
                .filter(|i| i.is_some())
                .map(|i| i.unwrap())
                .collect();
            let errors: Vec<EquationError> = errors.into_iter().map(|e| e.unwrap_err()).collect();

            Variable::Module {
                model_name: v.model_name.clone(),
                ident,
                units: v.units.clone(),
                inputs,
                errors,
            }
        }
    }
}

struct IdentifierSetVisitor<'a> {
    identifiers: HashSet<Ident>,
    dimensions: &'a [Dimension],
}

impl<'a> Visitor<()> for IdentifierSetVisitor<'a> {
    fn walk(&mut self, e: &Expr) {
        match e {
            Expr::Const(_, _, _) => (),
            Expr::Var(id, _) => {
                if !is_builtin_fn_or_time(id) {
                    self.identifiers.insert(id.clone());
                }
            }
            Expr::App(func, args, _) => {
                if !is_builtin_fn(func) {
                    self.identifiers.insert(func.clone());
                }
                for arg in args.iter() {
                    self.walk(arg);
                }
            }
            Expr::Subscript(id, args, _) => {
                if !is_builtin_fn_or_time(id) {
                    self.identifiers.insert(id.clone());
                }
                for arg in args.iter() {
                    if let Expr::Var(arg_ident, _) = arg {
                        let mut is_subscript_or_dimension = false;
                        // TODO: this should be optimized
                        for dim in self.dimensions.iter() {
                            if arg_ident == &dim.name {
                                is_subscript_or_dimension = true;
                            }
                            for element_name in dim.elements.iter() {
                                // subscript names aren't dependencies
                                if arg_ident == element_name {
                                    is_subscript_or_dimension = true;
                                }
                            }
                        }
                        if !is_subscript_or_dimension {
                            self.walk(arg);
                        }
                    } else {
                        self.walk(arg)
                    }
                }
            }
            Expr::Op2(_, l, r, _) => {
                self.walk(l);
                self.walk(r);
            }
            Expr::Op1(_, l, _) => {
                self.walk(l);
            }
            Expr::If(cond, t, f, _) => {
                self.walk(cond);
                self.walk(t);
                self.walk(f);
            }
        }
    }
}

pub fn identifier_set(ast: &AST, dimensions: &[Dimension]) -> HashSet<Ident> {
    let mut id_visitor = IdentifierSetVisitor {
        identifiers: HashSet::new(),
        dimensions,
    };
    match ast {
        AST::Scalar(ast) => id_visitor.walk(ast),
        AST::ApplyToAll(_, ast) => id_visitor.walk(ast),
        AST::Arrayed(_, elements) => {
            for ast in elements.values() {
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
        ("g[foo]", &["g"]),
    ];

    let dimensions: &[Dimension] = &[Dimension {
        name: "dim1".to_string(),
        elements: vec!["foo".to_owned()],
    }];

    for (eqn, id_list) in cases.iter() {
        let (ast, err) = parse_equation(&datamodel::Equation::Scalar((*eqn).to_owned()), &[]);
        assert_eq!(err.len(), 0);
        assert!(ast.is_some());
        let ast = ast.unwrap();
        let id_set_expected: HashSet<Ident> = id_list.into_iter().map(|s| s.to_string()).collect();
        let id_set_test = identifier_set(&ast, &dimensions);
        assert_eq!(id_set_expected, id_set_test);
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
        ast: Some(AST::Scalar(Expr::Const(
            "0".to_string(),
            0.0,
            Loc::new(0, 1),
        ))),
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
    let output = parse_var(&HashMap::new(), "main", &[], &input, &mut implicit_vars);

    assert_eq!(expected, output);
}
