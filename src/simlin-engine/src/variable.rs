// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};

#[cfg(test)]
use crate::ast::Loc;
use crate::ast::{Ast, Expr0, Expr2, IndexExpr2};
use crate::builtins::{BuiltinContents, BuiltinFn, walk_builtin_expr};
use crate::builtins_visitor::instantiate_implicit_modules;
use crate::common::{
    Canonical, CanonicalElementName, DimensionName, EquationError, EquationResult, Ident,
    UnitError, canonicalize,
};
use crate::datamodel;
use crate::dimensions::Dimension;
#[cfg(test)]
use crate::model::ScopeStage0;
use crate::token::LexerType;
use crate::units::parse_units;
use crate::{ErrorCode, eqn_err, units};

#[derive(Clone, PartialEq, Debug)]
pub struct Table {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    x_range: datamodel::GraphicalFunctionScale,
    y_range: datamodel::GraphicalFunctionScale,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModuleInput {
    // the Variable identifier in the current model we will use for input
    pub src: Ident<Canonical>,
    // the Variable identifier in the module's model we will override
    pub dst: Ident<Canonical>,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Variable<MI = ModuleInput, E = Expr2> {
    Stock {
        ident: Ident<Canonical>,
        init_ast: Option<Ast<E>>,
        eqn: Option<datamodel::Equation>,
        units: Option<datamodel::UnitMap>,
        inflows: Vec<Ident<Canonical>>,
        outflows: Vec<Ident<Canonical>>,
        non_negative: bool,
        errors: Vec<EquationError>,
        unit_errors: Vec<UnitError>,
    },
    Var {
        ident: Ident<Canonical>,
        ast: Option<Ast<E>>,
        init_ast: Option<Ast<E>>,
        eqn: Option<datamodel::Equation>,
        units: Option<datamodel::UnitMap>,
        table: Option<Table>,
        non_negative: bool,
        is_flow: bool,
        is_table_only: bool,
        errors: Vec<EquationError>,
        unit_errors: Vec<UnitError>,
    },
    Module {
        // the current spec has ident == model name
        ident: Ident<Canonical>,
        model_name: Ident<Canonical>,
        units: Option<datamodel::UnitMap>,
        inputs: Vec<MI>,
        errors: Vec<EquationError>,
        unit_errors: Vec<UnitError>,
    },
}

impl<MI, E> Variable<MI, E> {
    pub fn ident(&self) -> &str {
        match self {
            Variable::Stock { ident: name, .. }
            | Variable::Var { ident: name, .. }
            | Variable::Module { ident: name, .. } => name.as_str(),
        }
    }

    pub fn canonical_ident(&self) -> &Ident<Canonical> {
        match self {
            Variable::Stock { ident: name, .. }
            | Variable::Var { ident: name, .. }
            | Variable::Module { ident: name, .. } => name,
        }
    }

    pub fn ast(&self) -> Option<&Ast<E>> {
        match self {
            Variable::Stock {
                init_ast: Some(ast),
                ..
            }
            | Variable::Var { ast: Some(ast), .. } => Some(ast),
            _ => None,
        }
    }

    // returns the init_ast if one exists, otherwise ast()
    pub fn init_ast(&self) -> Option<&Ast<E>> {
        if let Variable::Var {
            init_ast: Some(ast),
            ..
        } = self
        {
            return Some(ast);
        }
        self.ast()
    }

    pub fn scalar_equation(&self) -> Option<&String> {
        match self {
            Variable::Stock {
                eqn: Some(datamodel::Equation::Scalar(s, ..)),
                ..
            }
            | Variable::Var {
                eqn: Some(datamodel::Equation::Scalar(s, ..)),
                ..
            } => Some(s),
            _ => None,
        }
    }

    pub fn get_dimensions(&self) -> Option<&[Dimension]> {
        match self {
            Variable::Stock {
                init_ast: Some(Ast::Arrayed(dims, _)),
                ..
            }
            | Variable::Var {
                ast: Some(Ast::Arrayed(dims, _)),
                ..
            } => Some(dims),
            Variable::Stock {
                init_ast: Some(Ast::ApplyToAll(dims, _)),
                ..
            }
            | Variable::Var {
                ast: Some(Ast::ApplyToAll(dims, _)),
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

    pub fn equation_errors(&self) -> Option<Vec<EquationError>> {
        let errors = match self {
            Variable::Stock { errors, .. }
            | Variable::Var { errors, .. }
            | Variable::Module { errors, .. } => errors,
        };
        if errors.is_empty() {
            None
        } else {
            Some(errors.clone())
        }
    }

    pub fn unit_errors(&self) -> Option<Vec<UnitError>> {
        let errors = match self {
            Variable::Stock { unit_errors, .. }
            | Variable::Var { unit_errors, .. }
            | Variable::Module { unit_errors, .. } => unit_errors,
        };
        if errors.is_empty() {
            None
        } else {
            Some(errors.clone())
        }
    }

    pub fn push_error(&mut self, err: EquationError) {
        match self {
            Variable::Stock { errors, .. }
            | Variable::Var { errors, .. }
            | Variable::Module { errors, .. } => errors.push(err),
        }
    }

    pub fn push_unit_error(&mut self, err: UnitError) {
        match self {
            Variable::Stock { unit_errors, .. }
            | Variable::Var { unit_errors, .. }
            | Variable::Module { unit_errors, .. } => unit_errors.push(err),
        }
    }

    pub fn table(&self) -> Option<&Table> {
        match self {
            Variable::Stock { .. } => None,
            Variable::Var { table, .. } => table.as_ref(),
            Variable::Module { .. } => None,
        }
    }

    pub fn units(&self) -> Option<&datamodel::UnitMap> {
        match self {
            Variable::Stock { units, .. } => units.as_ref(),
            Variable::Var { units, .. } => units.as_ref(),
            Variable::Module { units, .. } => units.as_ref(),
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
    dimensions: &[datamodel::Dimension],
    names: &[DimensionName],
) -> Result<Vec<Dimension>, EquationError> {
    names
        .iter()
        .map(|name| -> Result<Dimension, EquationError> {
            for dim in dimensions {
                if dim.name() == name {
                    return Ok(Dimension::from(dim.clone()));
                }
            }
            eqn_err!(BadDimensionName, 0, 0)
        })
        .collect()
}

fn parse_equation(
    eqn: &datamodel::Equation,
    dimensions: &[datamodel::Dimension],
    is_initial: bool,
) -> (Option<Ast<Expr0>>, Vec<EquationError>) {
    fn parse_inner(eqn: &str) -> (Option<Expr0>, Vec<EquationError>) {
        match Expr0::new(eqn, LexerType::Equation) {
            Ok(expr) => (expr, vec![]),
            Err(errors) => (None, errors),
        }
    }
    match eqn {
        datamodel::Equation::Scalar(eqn, init_eqn) => {
            let (ast, errors) = if !is_initial {
                parse_inner(eqn)
            } else if let Some(init_eqn) = init_eqn {
                parse_inner(init_eqn)
            } else {
                (None, vec![])
            };
            (ast.map(Ast::Scalar), errors)
        }
        datamodel::Equation::ApplyToAll(dimension_names, eqn, init_eqn) => {
            let (ast, mut errors) = if !is_initial {
                parse_inner(eqn)
            } else if let Some(init_eqn) = init_eqn {
                parse_inner(init_eqn)
            } else {
                (None, vec![])
            };

            match get_dimensions(dimensions, dimension_names) {
                Ok(dims) => (ast.map(|ast| Ast::ApplyToAll(dims, ast)), errors),
                Err(err) => {
                    errors.push(err);
                    (None, errors)
                }
            }
        }
        datamodel::Equation::Arrayed(dimension_names, elements) => {
            let mut errors: Vec<EquationError> = vec![];
            let elements: HashMap<_, _> = elements
                .iter()
                .map(|(subscript, eqn, init_eqn)| {
                    let (ast, single_errors) = if is_initial && init_eqn.is_some() {
                        parse_inner(init_eqn.as_ref().unwrap())
                    } else {
                        parse_inner(eqn)
                    };
                    errors.extend(single_errors);
                    (CanonicalElementName::from_raw(subscript), ast)
                })
                .filter(|(_, ast)| ast.is_some())
                .map(|(subscript, ast)| (subscript, ast.unwrap()))
                .collect();

            match get_dimensions(dimensions, dimension_names) {
                Ok(dims) => (Some(Ast::Arrayed(dims, elements)), errors),
                Err(err) => {
                    errors.push(err);
                    (None, errors)
                }
            }
        }
    }
}

pub fn parse_var<MI, F>(
    dimensions: &[datamodel::Dimension],
    v: &datamodel::Variable,
    implicit_vars: &mut Vec<datamodel::Variable>,
    units_ctx: &units::Context,
    module_input_mapper: F,
) -> Variable<MI, Expr0>
where
    MI: std::fmt::Debug, // TODO: not sure why unwrap_err needs this
    F: Fn(&datamodel::ModuleReference) -> EquationResult<Option<MI>>,
{
    let mut parse_and_lower_eqn = |ident: &str,
                                   eqn: &datamodel::Equation,
                                   is_initial: bool|
     -> (Option<Ast<Expr0>>, Vec<EquationError>) {
        let (ast, mut errors) = parse_equation(eqn, dimensions, is_initial);
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
                if errors.is_empty() && !is_initial && !v.can_be_module_input() {
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

            // TODO: should is_intial be true here?
            let (ast, errors) = parse_and_lower_eqn(&ident, &v.equation, false);

            let mut unit_errors: Vec<UnitError> = vec![];
            let units = match parse_units(units_ctx, v.units.as_deref()) {
                Ok(units) => units,
                Err(errors) => {
                    for err in errors.into_iter() {
                        unit_errors.push(err);
                    }
                    None
                }
            };
            Variable::Stock {
                ident: canonicalize(&ident),
                init_ast: ast,
                eqn: Some(v.equation.clone()),
                units,
                inflows: v.inflows.iter().map(|i| canonicalize(i)).collect(),
                outflows: v.outflows.iter().map(|o| canonicalize(o)).collect(),
                non_negative: v.non_negative,
                errors,
                unit_errors,
            }
        }
        datamodel::Variable::Flow(v) => {
            let ident = canonicalize(&v.ident);

            let (ast, mut errors) = parse_and_lower_eqn(ident.as_str(), &v.equation, false);
            let (init_ast, init_errors) = parse_and_lower_eqn(ident.as_str(), &v.equation, true);
            errors.extend(init_errors);

            let mut unit_errors: Vec<UnitError> = vec![];
            let units = match parse_units(units_ctx, v.units.as_deref()) {
                Ok(units) => units,
                Err(errors) => {
                    for err in errors.into_iter() {
                        unit_errors.push(err);
                    }
                    None
                }
            };
            let table = match parse_table(&v.gf) {
                Ok(table) => table,
                Err(err) => {
                    // TODO: should have a TableError variant
                    errors.push(err);
                    None
                }
            };
            Variable::Var {
                ident,
                ast,
                init_ast,
                eqn: Some(v.equation.clone()),
                units,
                table,
                is_flow: true,
                is_table_only: false,
                non_negative: v.non_negative,
                errors,
                unit_errors,
            }
        }
        datamodel::Variable::Aux(v) => {
            let ident = canonicalize(&v.ident);

            let (ast, mut errors) = parse_and_lower_eqn(ident.as_str(), &v.equation, false);
            let (init_ast, init_errors) = parse_and_lower_eqn(ident.as_str(), &v.equation, true);
            errors.extend(init_errors);

            let mut unit_errors: Vec<UnitError> = vec![];
            let units = match parse_units(units_ctx, v.units.as_deref()) {
                Ok(units) => units,
                Err(errors) => {
                    for err in errors.into_iter() {
                        unit_errors.push(err);
                    }
                    None
                }
            };
            let table = match parse_table(&v.gf) {
                Ok(table) => table,
                Err(err) => {
                    // TODO: should have TableError variant
                    errors.push(err);
                    None
                }
            };
            Variable::Var {
                ident,
                ast,
                init_ast,
                eqn: Some(v.equation.clone()),
                units,
                table,
                is_flow: false,
                is_table_only: false,
                non_negative: false,
                errors,
                unit_errors,
            }
        }
        datamodel::Variable::Module(v) => {
            let ident = canonicalize(&v.ident);
            let inputs = v.references.iter().map(module_input_mapper);
            let (inputs, errors): (Vec<_>, Vec<_>) = inputs.partition(EquationResult::is_ok);
            let inputs: Vec<MI> = inputs.into_iter().flat_map(|i| i.unwrap()).collect();
            let errors: Vec<EquationError> = errors.into_iter().map(|e| e.unwrap_err()).collect();
            let mut unit_errors: Vec<UnitError> = vec![];
            let units = match parse_units(units_ctx, v.units.as_deref()) {
                Ok(units) => units,
                Err(errors) => {
                    for err in errors.into_iter() {
                        unit_errors.push(err);
                    }
                    None
                }
            };

            Variable::Module {
                model_name: canonicalize(&v.model_name),
                ident,
                units,
                inputs,
                errors,
                unit_errors,
            }
        }
    }
}

struct IdentifierSetVisitor<'a> {
    identifiers: HashSet<Ident<Canonical>>,
    dimensions: &'a [Dimension],
    module_inputs: Option<&'a BTreeSet<Ident<Canonical>>>,
}

impl IdentifierSetVisitor<'_> {
    fn walk_index(&mut self, e: &IndexExpr2) {
        match e {
            IndexExpr2::Wildcard(_) => {}
            IndexExpr2::StarRange(_, _) => {}
            IndexExpr2::Range(_, _, _) => {}
            IndexExpr2::DimPosition(_, _) => {}
            IndexExpr2::Expr(expr) => {
                if let Expr2::Var(arg_ident, _, _) = expr {
                    let mut is_subscript_or_dimension = false;
                    // TODO: this should be optimized
                    for dim in self.dimensions.iter() {
                        if arg_ident.as_str() == canonicalize(dim.name()).as_str() {
                            is_subscript_or_dimension = true;
                        } else if let Dimension::Named(_, named_dim) = dim {
                            // Check if arg_ident matches any element (case-insensitive)
                            // We need to canonicalize both for comparison since identifiers are canonicalized
                            let canonicalized_arg = arg_ident.as_str();
                            let is_element = named_dim
                                .elements
                                .iter()
                                .any(|elem| elem.as_str() == canonicalized_arg);
                            if is_element {
                                is_subscript_or_dimension = true;
                            }
                        }
                        if is_subscript_or_dimension {
                            break;
                        }
                    }
                    if !is_subscript_or_dimension {
                        self.walk(expr);
                    }
                } else {
                    self.walk(expr)
                }
            }
        }
    }

    fn walk(&mut self, e: &Expr2) {
        match e {
            Expr2::Const(_, _, _) => (),
            Expr2::Var(id, _, _) => {
                // Check if this identifier is a dimension name
                // If so, don't add it as a dependency since it will be resolved during compilation
                let is_dimension = self.dimensions.iter().any(|dim| {
                    let canonicalized_dim = canonicalize(dim.name());
                    id.as_str() == canonicalized_dim.as_str()
                });

                if !is_dimension {
                    self.identifiers.insert(id.clone());
                }
            }
            Expr2::App(builtin, _, _) => {
                walk_builtin_expr(builtin, |contents| match contents {
                    BuiltinContents::Ident(id, _loc) => {
                        self.identifiers.insert(canonicalize(id));
                    }
                    BuiltinContents::Expr(expr) => self.walk(expr),
                });
            }
            Expr2::Subscript(id, args, _, _) => {
                self.identifiers.insert(id.clone());
                args.iter().for_each(|arg| self.walk_index(arg));
            }
            Expr2::Op2(_, l, r, _, _) => {
                self.walk(l);
                self.walk(r);
            }
            Expr2::Op1(_, l, _, _) => {
                self.walk(l);
            }
            Expr2::If(cond, t, f, _, _) => {
                if let Some(module_inputs) = self.module_inputs
                    && let Expr2::App(BuiltinFn::IsModuleInput(ident, _), _, _) = cond.as_ref()
                {
                    if module_inputs.contains(&canonicalize(ident)) {
                        self.walk(t);
                    } else {
                        self.walk(f);
                    }
                    return;
                }

                self.walk(cond);
                self.walk(t);
                self.walk(f);
            }
        }
    }
}

pub fn identifier_set(
    ast: &Ast<Expr2>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> HashSet<Ident<Canonical>> {
    let mut id_visitor = IdentifierSetVisitor {
        identifiers: HashSet::new(),
        dimensions,
        module_inputs,
    };
    match ast {
        Ast::Scalar(ast) => id_visitor.walk(ast),
        Ast::ApplyToAll(_, ast) => id_visitor.walk(ast),
        Ast::Arrayed(_, elements) => {
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
        ("if isModuleInput(input) then b else c", &["b"]),
        ("if a then b else c", &["a", "b", "c"]),
        ("lookup(b, c)", &["b", "c"]),
        ("-(a)", &["a"]),
        ("if a = 1 then -c else lookup(c,b)", &["a", "b", "c"]),
        ("if a.d then b else c", &["a·d", "b", "c"]),
        ("if \"a.d\" then b else c", &["a.d", "b", "c"]),
        ("g[foo]", &["g"]),
    ];

    let dimensions: Vec<Dimension> = vec![Dimension::from(datamodel::Dimension::Named(
        "dim1".to_string(),
        vec!["foo".to_owned()],
    ))];

    let module_inputs: &[ModuleInput] = &[ModuleInput {
        src: canonicalize("whatever"),
        dst: canonicalize("input"),
    }];

    use crate::ast::lower_ast;

    for (eqn, id_list) in cases.iter() {
        let (ast, err) = parse_equation(
            &datamodel::Equation::Scalar((*eqn).to_owned(), None),
            &[],
            false,
        );
        assert_eq!(err.len(), 0);
        assert!(ast.is_some());
        let scope = ScopeStage0 {
            models: &Default::default(),
            dimensions: &Default::default(),
            model_name: "test_model",
        };
        let ast = lower_ast(&scope, ast.unwrap()).unwrap();
        let id_set_expected: HashSet<Ident<Canonical>> = id_list
            .iter()
            .map(|s| {
                // If the test expectation already contains a middle dot, use it directly
                // Otherwise canonicalize it
                if s.contains('·') {
                    Ident::<Canonical>::from_unchecked(s.to_string())
                } else {
                    // For test expectations like "a.d", we treat them as already canonical
                    // (as they would be after parsing a quoted identifier)
                    Ident::<Canonical>::from_unchecked(s.to_string())
                }
            })
            .collect();
        let module_input_names = module_inputs.iter().map(|mi| mi.dst.clone()).collect();
        let id_set_test = identifier_set(&ast, &dimensions, Some(&module_input_names));
        if id_set_expected != id_set_test {
            eprintln!("Test case failed: {eqn}");
            eprintln!("Expected: {id_set_expected:?}");
            eprintln!("Got: {id_set_test:?}");
        }
        assert_eq!(id_set_expected, id_set_test);
    }
}

#[test]
fn test_tables() {
    use crate::common::canonicalize;
    let input = datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize("lookup function table").as_str().to_string(),
        equation: datamodel::Equation::Scalar("0".to_string(), None),
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
        can_be_module_input: false,
        visibility: datamodel::Visibility::Private,
        ai_state: None,
    });

    let expected = Variable::Var {
        ident: canonicalize("lookup_function_table"),
        ast: Some(Ast::Scalar(Expr0::Const(
            "0".to_string(),
            0.0,
            Loc::new(0, 1),
        ))),
        init_ast: None,
        eqn: Some(datamodel::Equation::Scalar("0".to_string(), None)),
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
        unit_errors: vec![],
    };

    if let Variable::Var {
        table: Some(table), ..
    } = &expected
    {
        assert_eq!(table.x.len(), table.y.len());
    } else {
        panic!("not eq");
    }

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let unit_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
    let output = parse_var(&[], &input, &mut implicit_vars, &unit_ctx, |mi| {
        Ok(Some(mi.clone()))
    });

    assert_eq!(expected, output);
}
