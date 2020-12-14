// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use float_cmp::approx_eq;

use crate::ast::{self, AST};
use crate::common::{Ident, Result};
use crate::datamodel::{self, Dt, SimMethod};
use crate::interpreter::{BinaryOp, UnaryOp};
use crate::model::Model;
use crate::variable::Variable;
use crate::{sim_err, Error, Project};
use std::borrow::BorrowMut;

const TIME_OFF: usize = 0;
const DT_OFF: usize = 1;
const INITIAL_TIME_OFF: usize = 2;
const FINAL_TIME_OFF: usize = 3;
const IMPLICIT_VAR_COUNT: usize = 4;

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum Method {
    Euler,
}

#[derive(Clone, Debug)]
pub struct Specs {
    pub start: f64,
    pub stop: f64,
    pub dt: f64,
    pub save_step: f64,
    pub method: Method,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub data: Vec<(f64, f64)>,
}

impl Table {
    fn new(ident: &str, t: &crate::variable::Table) -> Result<Self> {
        if t.x.len() != t.y.len() {
            return sim_err!(BadTable, ident.to_string());
        }

        let data: Vec<(f64, f64)> = t.x.iter().copied().zip(t.y.iter().copied()).collect();

        Ok(Self { data })
    }
}

impl Specs {
    pub fn from(specs: &datamodel::SimSpecs) -> Self {
        let dt: f64 = match &specs.dt {
            Dt::Dt(value) => *value,
            Dt::Reciprocal(value) => 1.0 / *value,
        };

        let save_step: f64 = match &specs.save_step {
            None => dt,
            Some(save_step) => match save_step {
                Dt::Dt(value) => *value,
                Dt::Reciprocal(value) => 1.0 / *value,
            },
        };

        let method = match specs.sim_method {
            SimMethod::Euler => Method::Euler,
            SimMethod::RungeKutta4 => {
                eprintln!("warning, simulation requested 'rk4', but only support Euler");
                Method::Euler
            }
        };

        Specs {
            start: specs.start,
            stop: specs.stop,
            dt,
            save_step,
            method,
        }
    }
}

type BuiltinFn = crate::builtins::BuiltinFn<Expr>;

#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(f64),
    Var(usize),                         // offset
    Subscript(usize, Box<Expr>, usize), // offset, index expression, bounds
    Dt,
    App(BuiltinFn),
    EvalModule(Ident, Ident, Vec<Expr>),
    ModuleInput(usize),
    Op2(BinaryOp, Box<Expr>, Box<Expr>),
    Op1(UnaryOp, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Box<Expr>),
    AssignCurr(usize, Box<Expr>),
    AssignNext(usize, Box<Expr>),
}

#[derive(Clone, Debug)]
struct VariableMetadata {
    offset: usize,
    size: usize,
    // FIXME: this should be able to be borrowed
    var: Variable,
}

#[derive(Clone, Debug)]
struct Context<'a> {
    #[allow(dead_code)]
    dimensions: &'a [datamodel::Dimension],
    model_name: &'a str,
    ident: &'a str,
    active_dimension: Option<datamodel::Dimension>,
    active_subscript: Option<&'a str>,
    metadata: &'a HashMap<Ident, HashMap<Ident, VariableMetadata>>,
    module_models: &'a HashMap<Ident, HashMap<Ident, Ident>>,
    is_initial: bool,
    inputs: &'a [Ident],
}

impl<'a> Context<'a> {
    fn get_offset(&self, ident: &str) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, false)
    }

    /// get_base_offset ignores arrays and should only be used from Var::new and Expr::Subscript
    fn get_base_offset(&self, ident: &str) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, true)
    }

    #[allow(dead_code)]
    fn get_dimension(&self, name: &str) -> Result<&datamodel::Dimension> {
        for dim in self.dimensions {
            if dim.name == name {
                return Ok(dim);
            }
        }
        return sim_err!(BadDimensionName, name.to_owned());
    }

    fn get_metadata(&self, ident: &str) -> Result<&VariableMetadata> {
        self.get_submodel_metadata(self.model_name, ident)
    }

    fn get_submodel_metadata(&self, model: &str, ident: &str) -> Result<&VariableMetadata> {
        let metadata = &self.metadata[model];
        if let Some(pos) = ident.find('.') {
            let submodel_module_name = &ident[..pos];
            let submodel_name = &self.module_models[model][submodel_module_name];
            let submodel_var = &ident[pos + 1..];
            self.get_submodel_metadata(submodel_name, submodel_var)
        } else {
            Ok(&metadata[ident])
        }
    }

    fn get_submodel_offset(&self, model: &str, ident: &str, ignore_arrays: bool) -> Result<usize> {
        let metadata = &self.metadata[model];
        if let Some(pos) = ident.find('.') {
            let submodel_module_name = &ident[..pos];
            let submodel_name = &self.module_models[model][submodel_module_name];
            let submodel_var = &ident[pos + 1..];
            let submodel_off = metadata[submodel_module_name].offset;
            Ok(submodel_off
                + self.get_submodel_offset(submodel_name, submodel_var, ignore_arrays)?)
        } else if !ignore_arrays {
            if !metadata.contains_key(ident) {
                panic!("internal error: unknown var {}?", ident);
            }
            if let Some(dims) = metadata[ident].var.get_dimensions() {
                if dims.len() != 1 {
                    panic!("FIXME: only 1D arrays supported for now");
                }
                if self.active_dimension.is_none() {
                    return sim_err!(ArrayReferenceNeedsExplicitSubscripts, ident.to_owned());
                }
                let var_dim = &dims[0];
                let active_dim = self.active_dimension.as_ref().unwrap();
                if active_dim.name != var_dim.name {
                    return sim_err!(MismatchedDimensions, ident.to_owned());
                }
                if let Some(off) = var_dim.get_offset(self.active_subscript.unwrap()) {
                    Ok(metadata[ident].offset + off)
                } else {
                    return sim_err!(MismatchedDimensions, ident.to_owned());
                }
            } else {
                Ok(metadata[ident].offset)
            }
        } else {
            Ok(metadata[ident].offset)
        }
    }

    fn lower(&self, expr: &ast::Expr) -> Result<Expr> {
        let expr = match expr {
            ast::Expr::Const(_, n) => Expr::Const(*n),
            ast::Expr::Var(id) => {
                if let Some((off, _)) = self
                    .inputs
                    .iter()
                    .enumerate()
                    .find(|(_, input)| id == *input)
                {
                    Expr::ModuleInput(off)
                } else {
                    Expr::Var(self.get_offset(id)?)
                }
            }
            ast::Expr::App(id, orig_args) => {
                let args: Result<Vec<Expr>> = orig_args.iter().map(|e| self.lower(e)).collect();
                let mut args = args?;

                macro_rules! check_arity {
                    ($builtin_fn:tt, 0) => {{
                        if !args.is_empty() {
                            return sim_err!(BadBuiltinArgs, self.ident.to_string());
                        }

                        BuiltinFn::$builtin_fn
                    }};
                    ($builtin_fn:tt, 1) => {{
                        if args.len() != 1 {
                            return sim_err!(BadBuiltinArgs, self.ident.to_string());
                        }

                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a))
                    }};
                    ($builtin_fn:tt, 2) => {{
                        if args.len() != 2 {
                            return sim_err!(BadBuiltinArgs, self.ident.to_string());
                        }

                        let b = args.remove(1);
                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a), Box::new(b))
                    }};
                    ($builtin_fn:tt, 3) => {{
                        if args.len() != 3 {
                            return sim_err!(BadBuiltinArgs, self.ident.to_string());
                        }

                        let c = args.remove(2);
                        let b = args.remove(1);
                        let a = args.remove(0);
                        BuiltinFn::$builtin_fn(Box::new(a), Box::new(b), Box::new(c))
                    }};
                    ($builtin_fn:tt, 2, 3) => {{
                        if args.len() == 2 {
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), Box::new(b), None)
                        } else if args.len() == 3 {
                            let c = args.remove(2);
                            let b = args.remove(1);
                            let a = args.remove(0);
                            BuiltinFn::$builtin_fn(Box::new(a), Box::new(b), Some(Box::new(c)))
                        } else {
                            return sim_err!(BadBuiltinArgs, self.ident.to_string());
                        }
                    }};
                }

                let builtin = match id.as_str() {
                    "lookup" => {
                        if let ast::Expr::Var(ident) = &orig_args[0] {
                            BuiltinFn::Lookup(ident.clone(), Box::new(args[1].clone()))
                        } else {
                            return sim_err!(BadTable, id.clone());
                        }
                    }
                    "abs" => check_arity!(Abs, 1),
                    "arccos" => check_arity!(Arccos, 1),
                    "arcsin" => check_arity!(Arcsin, 1),
                    "arctan" => check_arity!(Arctan, 1),
                    "cos" => check_arity!(Cos, 1),
                    "exp" => check_arity!(Exp, 1),
                    "inf" => check_arity!(Inf, 0),
                    "int" => check_arity!(Int, 1),
                    "ln" => check_arity!(Ln, 1),
                    "log10" => check_arity!(Log10, 1),
                    "max" => check_arity!(Max, 2),
                    "min" => check_arity!(Min, 2),
                    "pi" => check_arity!(Pi, 0),
                    "pulse" => check_arity!(Pulse, 3),
                    "safediv" => check_arity!(SafeDiv, 2, 3),
                    "sin" => check_arity!(Sin, 1),
                    "sqrt" => check_arity!(Sqrt, 1),
                    "tan" => check_arity!(Tan, 1),
                    _ => {
                        return sim_err!(UnknownBuiltin, self.ident.to_string());
                    }
                };
                Expr::App(builtin)
            }
            ast::Expr::Subscript(id, args) => {
                if args.len() != 1 {
                    return sim_err!(MultiDimensionalArraysNotImplemented, id.clone());
                }
                let off = self.get_base_offset(id)?;
                let metadata = self.get_metadata(id)?;
                let dims = metadata.var.get_dimensions().unwrap();
                if dims.len() != 1 {
                    return sim_err!(MultiDimensionalArraysNotImplemented, id.clone());
                }
                let dim = &dims[0];
                let arg = &args[0];
                if let ast::Expr::Var(ident) = arg {
                    // we need to check to make sure that any explicit subscript names are
                    // converted to offsets here and not passed to self.lower
                    if let Some(subscript_off) = dim.get_offset(ident) {
                        Expr::Subscript(
                            off,
                            Box::new(Expr::Const((subscript_off + 1) as f64)),
                            dim.elements.len(),
                        )
                    } else {
                        Expr::Subscript(off, Box::new(self.lower(&args[0])?), dim.elements.len())
                    }
                } else {
                    Expr::Subscript(off, Box::new(self.lower(&args[0])?), dim.elements.len())
                }
            }
            ast::Expr::Op1(op, l) => {
                let l = self.lower(l)?;
                match op {
                    ast::UnaryOp::Negative => {
                        Expr::Op2(BinaryOp::Sub, Box::new(Expr::Const(0.0)), Box::new(l))
                    }
                    ast::UnaryOp::Positive => l,
                    ast::UnaryOp::Not => Expr::Op1(UnaryOp::Not, Box::new(l)),
                }
            }
            ast::Expr::Op2(op, l, r) => {
                let l = self.lower(l)?;
                let r = self.lower(r)?;
                match op {
                    ast::BinaryOp::Add => Expr::Op2(BinaryOp::Add, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Sub => Expr::Op2(BinaryOp::Sub, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Exp => Expr::Op2(BinaryOp::Exp, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Mul => Expr::Op2(BinaryOp::Mul, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Div => Expr::Op2(BinaryOp::Div, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Mod => Expr::Op2(BinaryOp::Mod, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Gt => Expr::Op2(BinaryOp::Gt, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Gte => Expr::Op2(BinaryOp::Gte, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Lt => Expr::Op2(BinaryOp::Lt, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Lte => Expr::Op2(BinaryOp::Lte, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Eq => Expr::Op2(BinaryOp::Eq, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Neq => Expr::Op2(BinaryOp::Neq, Box::new(l), Box::new(r)),
                    ast::BinaryOp::And => Expr::Op2(BinaryOp::And, Box::new(l), Box::new(r)),
                    ast::BinaryOp::Or => Expr::Op2(BinaryOp::Or, Box::new(l), Box::new(r)),
                }
            }
            ast::Expr::If(cond, t, f) => {
                let cond = self.lower(cond)?;
                let t = self.lower(t)?;
                let f = self.lower(f)?;
                Expr::If(Box::new(cond), Box::new(t), Box::new(f))
            }
        };

        Ok(expr)
    }

    fn fold_flows(&self, flows: &[String]) -> Option<Expr> {
        if flows.is_empty() {
            return None;
        }

        let mut loads = flows
            .iter()
            .map(|flow| Expr::Var(self.get_offset(flow).unwrap()));

        let first = loads.next().unwrap();
        Some(loads.fold(first, |acc, flow| {
            Expr::Op2(BinaryOp::Add, Box::new(acc), Box::new(flow))
        }))
    }

    fn build_stock_update_expr(&self, stock_off: usize, var: &Variable) -> Result<Expr> {
        if let Variable::Stock {
            inflows, outflows, ..
        } = var
        {
            // TODO: simplify the expressions we generate
            let inflows = match self.fold_flows(inflows) {
                None => Expr::Const(0.0),
                Some(flows) => flows,
            };
            let outflows = match self.fold_flows(outflows) {
                None => Expr::Const(0.0),
                Some(flows) => flows,
            };

            let dt_update = Expr::Op2(
                BinaryOp::Mul,
                Box::new(Expr::Op2(
                    BinaryOp::Sub,
                    Box::new(inflows),
                    Box::new(outflows),
                )),
                Box::new(Expr::Dt),
            );

            Ok(Expr::Op2(
                BinaryOp::Add,
                Box::new(Expr::Var(stock_off)),
                Box::new(dt_update),
            ))
        } else {
            panic!(
                "build_stock_update_expr called with non-stock {}",
                var.ident()
            );
        }
    }
}

#[test]
fn test_lower() {
    let input = {
        use ast::BinaryOp::*;
        use ast::Expr::*;
        Box::new(If(
            Box::new(Op2(
                And,
                Box::new(Var("true_input".to_string())),
                Box::new(Var("false_input".to_string())),
            )),
            Box::new(Const("1".to_string(), 1.0)),
            Box::new(Const("0".to_string(), 0.0)),
        ))
    };

    let inputs = &[];
    let module_models: HashMap<Ident, HashMap<Ident, Ident>> = HashMap::new();
    let mut metadata: HashMap<String, VariableMetadata> = HashMap::new();
    metadata.insert(
        "true_input".to_string(),
        VariableMetadata {
            offset: 7,
            size: 1,
            var: Variable::Var {
                ident: "".to_string(),
                ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                direct_deps: Default::default(),
            },
        },
    );
    metadata.insert(
        "false_input".to_string(),
        VariableMetadata {
            offset: 8,
            size: 1,
            var: Variable::Var {
                ident: "".to_string(),
                ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                direct_deps: Default::default(),
            },
        },
    );
    let mut metadata2 = HashMap::new();
    metadata2.insert("main".to_string(), metadata);
    let dimensions: Vec<datamodel::Dimension> = vec![];
    let context = Context {
        dimensions: &dimensions,
        model_name: "main",
        ident: "test",
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
    };
    let expected = Expr::If(
        Box::new(Expr::Op2(
            BinaryOp::And,
            Box::new(Expr::Var(7)),
            Box::new(Expr::Var(8)),
        )),
        Box::new(Expr::Const(1.0)),
        Box::new(Expr::Const(0.0)),
    );

    let output = context.lower(&input);
    assert!(output.is_ok());
    assert_eq!(expected, output.unwrap());

    let input = {
        use ast::BinaryOp::*;
        use ast::Expr::*;
        Box::new(If(
            Box::new(Op2(
                Or,
                Box::new(Var("true_input".to_string())),
                Box::new(Var("false_input".to_string())),
            )),
            Box::new(Const("1".to_string(), 1.0)),
            Box::new(Const("0".to_string(), 0.0)),
        ))
    };

    let inputs = &[];
    let module_models: HashMap<Ident, HashMap<Ident, Ident>> = HashMap::new();
    let mut metadata: HashMap<String, VariableMetadata> = HashMap::new();
    metadata.insert(
        "true_input".to_string(),
        VariableMetadata {
            offset: 7,
            size: 1,
            var: Variable::Var {
                ident: "".to_string(),
                ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                direct_deps: Default::default(),
            },
        },
    );
    metadata.insert(
        "false_input".to_string(),
        VariableMetadata {
            offset: 8,
            size: 1,
            var: Variable::Var {
                ident: "".to_string(),
                ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                direct_deps: Default::default(),
            },
        },
    );
    let mut metadata2 = HashMap::new();
    metadata2.insert("main".to_string(), metadata);
    let context = Context {
        dimensions: &dimensions,
        model_name: "main",
        ident: "test",
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
    };
    let expected = Expr::If(
        Box::new(Expr::Op2(
            BinaryOp::Or,
            Box::new(Expr::Var(7)),
            Box::new(Expr::Var(8)),
        )),
        Box::new(Expr::Const(1.0)),
        Box::new(Expr::Const(0.0)),
    );

    let output = context.lower(&input);
    assert!(output.is_ok());
    assert_eq!(expected, output.unwrap());
}

#[derive(Clone, Debug, PartialEq)]
pub struct Var {
    ast: Vec<Expr>,
}

#[test]
fn test_fold_flows() {
    let inputs = &[];
    let module_models: HashMap<Ident, HashMap<Ident, Ident>> = HashMap::new();
    let mut metadata: HashMap<String, VariableMetadata> = HashMap::new();
    metadata.insert(
        "a".to_string(),
        VariableMetadata {
            offset: 1,
            size: 1,
            var: Variable::Var {
                ident: "".to_string(),
                ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                direct_deps: Default::default(),
            },
        },
    );
    metadata.insert(
        "b".to_string(),
        VariableMetadata {
            offset: 2,
            size: 1,
            var: Variable::Var {
                ident: "".to_string(),
                ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                direct_deps: Default::default(),
            },
        },
    );
    metadata.insert(
        "c".to_string(),
        VariableMetadata {
            offset: 3,
            size: 1,
            var: Variable::Var {
                ident: "".to_string(),
                ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                direct_deps: Default::default(),
            },
        },
    );
    metadata.insert(
        "d".to_string(),
        VariableMetadata {
            offset: 4,
            size: 1,
            var: Variable::Var {
                ident: "".to_string(),
                ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                direct_deps: Default::default(),
            },
        },
    );
    let mut metadata2 = HashMap::new();
    metadata2.insert("main".to_string(), metadata);
    let dimensions: Vec<datamodel::Dimension> = vec![];
    let ctx = Context {
        dimensions: &dimensions,
        model_name: "main",
        ident: "test",
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
    };

    assert_eq!(None, ctx.fold_flows(&[]));
    assert_eq!(Some(Expr::Var(1)), ctx.fold_flows(&["a".to_string()]));
    assert_eq!(
        Some(Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var(1)),
            Box::new(Expr::Var(4))
        )),
        ctx.fold_flows(&["a".to_string(), "d".to_string()])
    );
}

impl Var {
    fn new(ctx: &Context, var: &Variable) -> Result<Self> {
        // if this variable is overriden by a module input, our expression is easy
        let ast: Vec<Expr> = if let Some((off, ident)) = ctx
            .inputs
            .iter()
            .enumerate()
            .find(|(_i, n)| *n == var.ident())
        {
            vec![Expr::AssignCurr(
                ctx.get_offset(ident)?,
                Box::new(Expr::ModuleInput(off)),
            )]
        } else {
            match var {
                Variable::Module {
                    ident,
                    model_name,
                    inputs,
                    ..
                } => {
                    let mut inputs = inputs.clone();
                    inputs.sort_unstable_by(|a, b| a.dst.partial_cmp(&b.dst).unwrap());
                    let inputs: Vec<Expr> = inputs
                        .into_iter()
                        .map(|mi| Expr::Var(ctx.get_offset(&mi.src).unwrap()))
                        .collect();
                    vec![Expr::EvalModule(ident.clone(), model_name.clone(), inputs)]
                }
                Variable::Stock { ast, .. } => {
                    let off = ctx.get_base_offset(var.ident())?;
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        match ast.as_ref().unwrap() {
                            AST::Scalar(ast) => {
                                vec![Expr::AssignCurr(off, Box::new(ctx.lower(ast)?))]
                            }
                            AST::ApplyToAll(dims, ast) => {
                                if dims.len() != 1 {
                                    return sim_err!(
                                        MultiDimensionalArraysNotImplemented,
                                        var.ident().to_string()
                                    );
                                }
                                let exprs: Result<Vec<Expr>> = dims[0]
                                    .elements
                                    .iter()
                                    .enumerate()
                                    .map(|(i, subscript)| {
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims[0].clone());
                                        ctx.active_subscript = Some(subscript);
                                        ctx.lower(ast)
                                            .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                    })
                                    .collect();
                                exprs?
                            }
                            AST::Arrayed(dims, elements) => {
                                if dims.len() != 1 {
                                    return sim_err!(
                                        MultiDimensionalArraysNotImplemented,
                                        var.ident().to_string()
                                    );
                                }
                                let exprs: Result<Vec<Expr>> = dims[0]
                                    .elements
                                    .iter()
                                    .enumerate()
                                    .map(|(i, subscript)| {
                                        let ast = &elements[subscript];
                                        ctx.lower(ast)
                                            .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    } else {
                        match ast.as_ref().unwrap() {
                            AST::Scalar(_) => vec![Expr::AssignNext(
                                off,
                                Box::new(ctx.build_stock_update_expr(off, var)?),
                            )],
                            AST::ApplyToAll(dims, _) | AST::Arrayed(dims, _) => {
                                let exprs: Result<Vec<Expr>> = dims[0]
                                    .elements
                                    .iter()
                                    .enumerate()
                                    .map(|(i, subscript)| {
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims[0].clone());
                                        ctx.active_subscript = Some(subscript);
                                        // when building the stock update expression, we need
                                        // the specific index of this subscript, not the base offset
                                        let update_expr = ctx.build_stock_update_expr(
                                            ctx.get_offset(var.ident())?,
                                            var,
                                        );
                                        update_expr
                                            .map(|ast| Expr::AssignNext(off + i, Box::new(ast)))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    }
                }
                Variable::Var {
                    ident, table, ast, ..
                } => {
                    let off = ctx.get_base_offset(var.ident())?;
                    if ast.is_none() {
                        return sim_err!(EmptyEquation, var.ident().to_string());
                    }
                    match ast.as_ref().unwrap() {
                        AST::Scalar(ast) => {
                            let expr = ctx.lower(ast)?;
                            let expr = if table.is_some() {
                                Expr::App(BuiltinFn::Lookup(ident.clone(), Box::new(expr)))
                            } else {
                                expr
                            };
                            vec![Expr::AssignCurr(off, Box::new(expr))]
                        }
                        AST::ApplyToAll(dims, ast) => {
                            if dims.len() != 1 {
                                return sim_err!(
                                    MultiDimensionalArraysNotImplemented,
                                    var.ident().to_string()
                                );
                            }
                            let exprs: Result<Vec<Expr>> = dims[0]
                                .elements
                                .iter()
                                .enumerate()
                                .map(|(i, subscript)| {
                                    let mut ctx = ctx.clone();
                                    ctx.active_dimension = Some(dims[0].clone());
                                    ctx.active_subscript = Some(subscript);
                                    ctx.lower(ast)
                                        .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                })
                                .collect();
                            exprs?
                        }
                        AST::Arrayed(dims, elements) => {
                            if dims.len() != 1 {
                                return sim_err!(
                                    MultiDimensionalArraysNotImplemented,
                                    var.ident().to_string()
                                );
                            }
                            let exprs: Result<Vec<Expr>> = dims[0]
                                .elements
                                .iter()
                                .enumerate()
                                .map(|(i, subscript)| {
                                    let ast = &elements[subscript];
                                    ctx.lower(ast)
                                        .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                })
                                .collect();
                            exprs?
                        }
                    }
                }
            }
        };
        Ok(Var { ast })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepPart {
    Initials,
    Flows,
    Stocks,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    ident: Ident,
    n_slots: usize, // number of f64s we need storage for
    runlist_initials: Vec<Expr>,
    runlist_flows: Vec<Expr>,
    runlist_stocks: Vec<Expr>,
    offsets: HashMap<Ident, HashMap<Ident, (usize, usize)>>,
    tables: HashMap<Ident, Table>,
}

fn topo_sort<'out>(
    vars: &'out HashMap<Ident, Variable>,
    all_deps: &'out HashMap<Ident, HashSet<Ident>>,
    runlist: Vec<&'out str>,
) -> Vec<&'out str> {
    let runlist_len = runlist.len();
    let mut result: Vec<&'out str> = Vec::with_capacity(runlist_len);
    // TODO: remove this allocation (should be &str)
    let mut used: HashSet<&str> = HashSet::new();

    // We want to do a postorder, recursive traversal of variables to ensure
    // dependencies are calculated before the variables that reference them.
    // By this point, we have already errored out if we have e.g. a cycle
    fn add<'a>(
        vars: &HashMap<Ident, Variable>,
        all_deps: &'a HashMap<Ident, HashSet<Ident>>,
        result: &mut Vec<&'a str>,
        used: &mut HashSet<&'a str>,
        ident: &'a str,
    ) {
        if used.contains(ident) {
            return;
        }
        used.insert(ident);
        for dep in all_deps[ident].iter() {
            add(vars, all_deps, result, used, dep)
        }
        result.push(ident);
    }

    for ident in runlist.into_iter() {
        add(vars, all_deps, &mut result, &mut used, ident)
    }

    assert_eq!(runlist_len, result.len());
    result
}

// calculate a mapping of module variable name -> module model name
fn calc_module_model_map(
    project: &Project,
    model_name: &str,
) -> HashMap<Ident, HashMap<Ident, Ident>> {
    let mut all_models: HashMap<Ident, HashMap<Ident, Ident>> = HashMap::new();

    let model = Rc::clone(&project.models[model_name]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        var_names.sort_unstable();
        var_names
    };

    let mut current_mapping: HashMap<Ident, Ident> = HashMap::new();

    for ident in var_names.iter() {
        if let Variable::Module { model_name, .. } = &model.variables[*ident] {
            current_mapping.insert(ident.to_string(), model_name.clone());
            let all_sub_models = calc_module_model_map(project, model_name);
            all_models.extend(all_sub_models);
        };
    }

    all_models.insert(model_name.to_string(), current_mapping);

    all_models
}

// TODO: this should memoize
fn build_metadata(
    project: &Project,
    model_name: &str,
    is_root: bool,
) -> HashMap<Ident, HashMap<Ident, VariableMetadata>> {
    let mut all_offsets: HashMap<Ident, HashMap<Ident, VariableMetadata>> = HashMap::new();

    let mut offsets: HashMap<Ident, VariableMetadata> = HashMap::new();
    let mut i = 0;
    if is_root {
        offsets.insert(
            "time".to_string(),
            VariableMetadata {
                offset: 0,
                size: 1,
                var: Variable::Var {
                    ident: "time".to_string(),
                    ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    direct_deps: Default::default(),
                },
            },
        );
        offsets.insert(
            "dt".to_string(),
            VariableMetadata {
                offset: 1,
                size: 1,
                var: Variable::Var {
                    ident: "dt".to_string(),
                    ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    direct_deps: Default::default(),
                },
            },
        );
        offsets.insert(
            "initial_time".to_string(),
            VariableMetadata {
                offset: 2,
                size: 1,
                var: Variable::Var {
                    ident: "initial_time".to_string(),
                    ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    direct_deps: Default::default(),
                },
            },
        );
        offsets.insert(
            "final_time".to_string(),
            VariableMetadata {
                offset: 3,
                size: 1,
                var: Variable::Var {
                    ident: "final_time".to_string(),
                    ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    direct_deps: Default::default(),
                },
            },
        );
        i += IMPLICIT_VAR_COUNT;
    }

    let model = Rc::clone(&project.models[model_name]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        var_names.sort_unstable();
        var_names
    };

    for ident in var_names.iter() {
        let size = if let Variable::Module { model_name, .. } = &model.variables[*ident] {
            let all_sub_offsets = build_metadata(project, model_name, false);
            let sub_offsets = &all_sub_offsets[model_name];
            let sub_size: usize = sub_offsets.values().map(|metadata| metadata.size).sum();
            all_offsets.extend(all_sub_offsets);
            sub_size
        } else if let Some(AST::ApplyToAll(dims, _)) = model.variables[*ident].ast() {
            if dims.len() != 1 {
                panic!("multi-dimensional arrays aren't supported yet");
            }
            dims[0].elements.len()
        } else if let Some(AST::Arrayed(dims, _)) = model.variables[*ident].ast() {
            if dims.len() != 1 {
                panic!("multi-dimensional arrays aren't supported yet");
            }
            dims[0].elements.len()
        } else {
            1
        };
        offsets.insert(
            (*ident).to_owned(),
            VariableMetadata {
                offset: i,
                size,
                var: model.variables[*ident].clone(),
            },
        );
        i += size;
    }

    all_offsets.insert(model_name.to_string(), offsets);

    all_offsets
}

/// calc_flattened_offsets generates a mapping from name to offset
/// for all individual variables and subscripts in a model, including
/// in submodels.  For example a variable named "offset" in a module
/// instantiated with name "sector" will produce the key "sector.offset".
fn calc_flattened_offsets(project: &Project, model_name: &str) -> HashMap<Ident, (usize, usize)> {
    let is_root = model_name == "main";

    let mut offsets: HashMap<Ident, (usize, usize)> = HashMap::new();
    let mut i = 0;
    if is_root {
        offsets.insert("time".to_string(), (0, 1));
        offsets.insert("dt".to_string(), (1, 1));
        offsets.insert("initial_time".to_string(), (2, 1));
        offsets.insert("final_time".to_string(), (3, 1));
        i += IMPLICIT_VAR_COUNT;
    }

    let model = Rc::clone(&project.models[model_name]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        var_names.sort_unstable();
        var_names
    };

    for ident in var_names.iter() {
        let size = if let Variable::Module { model_name, .. } = &model.variables[*ident] {
            let sub_offsets = calc_flattened_offsets(project, model_name);
            let mut sub_var_names: Vec<&str> = sub_offsets.keys().map(|v| v.as_str()).collect();
            sub_var_names.sort_unstable();
            for sub_name in sub_var_names {
                let (sub_off, sub_size) = sub_offsets[sub_name];
                offsets.insert(format!("{}.{}", ident, sub_name), (i + sub_off, sub_size));
            }
            let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
            sub_size
        } else if let Some(AST::ApplyToAll(dims, _)) = &model.variables[*ident].ast() {
            if dims.len() != 1 {
                panic!("multi-dimensional arrays aren't supported yet");
            }
            for (j, subscript) in dims[0].elements.iter().enumerate() {
                offsets.insert(format!("{}[{}]", ident, subscript), (i + j, 1));
            }
            dims[0].elements.len()
        } else if let Some(AST::Arrayed(dims, _)) = &model.variables[*ident].ast() {
            if dims.len() != 1 {
                panic!("multi-dimensional arrays aren't supported yet");
            }
            for (j, subscript) in dims[0].elements.iter().enumerate() {
                offsets.insert(format!("{}[{}]", ident, subscript), (i + j, 1));
            }
            dims[0].elements.len()
        } else {
            offsets.insert(ident.to_string(), (i, 1));
            1
        };
        i += size;
    }

    offsets
}

fn calc_n_slots(
    all_metadata: &HashMap<Ident, HashMap<Ident, VariableMetadata>>,
    model_name: &str,
) -> usize {
    let metadata = &all_metadata[model_name];

    metadata.values().map(|v| v.size).sum()
}

impl Module {
    fn new(project: &Project, model: Rc<Model>, inputs: &[Ident], is_root: bool) -> Result<Self> {
        if model.dt_deps.is_none() || model.initial_deps.is_none() {
            return sim_err!(NotSimulatable, model.name.clone());
        }

        let model_name: &str = &model.name;
        let metadata = build_metadata(project, model_name, is_root);

        let n_slots = calc_n_slots(&metadata, model_name);
        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
            var_names.sort_unstable();
            var_names
        };
        let module_models = calc_module_model_map(project, model_name);

        let build_runlist = |deps: &HashMap<Ident, HashSet<Ident>>,
                             part: StepPart,
                             predicate: &dyn Fn(&&str) -> bool|
         -> Result<Vec<Var>> {
            let runlist: Vec<&str> = var_names.iter().cloned().filter(predicate).collect();
            let runlist = match part {
                StepPart::Initials => {
                    let needed: HashSet<&str> = runlist
                        .iter()
                        .cloned()
                        .filter(|id| {
                            let v = &model.variables[*id];
                            v.is_stock() || v.is_module()
                        })
                        .collect();
                    let mut runlist: HashSet<&str> = needed
                        .iter()
                        .flat_map(|id| &deps[*id])
                        .map(|id| id.as_str())
                        .collect();
                    runlist.extend(needed);
                    let runlist = runlist.into_iter().collect();
                    topo_sort(&model.variables, deps, runlist)
                }
                StepPart::Flows => topo_sort(&model.variables, deps, runlist),
                StepPart::Stocks => runlist,
            };
            // eprintln!("runlist {}", model_name);
            // for (i, name) in runlist.iter().enumerate() {
            //     eprintln!("  {}: {}", i, name);
            // }
            let is_initial = matches!(part, StepPart::Initials);
            let runlist: Result<Vec<Var>> = runlist
                .into_iter()
                .map(|ident| {
                    Var::new(
                        &Context {
                            dimensions: &project.datamodel.dimensions,
                            model_name,
                            ident,
                            active_dimension: None,
                            active_subscript: None,
                            metadata: &metadata,
                            module_models: &module_models,
                            is_initial,
                            inputs,
                        },
                        &model.variables[ident],
                    )
                })
                .collect();
            // for v in runlist.clone().unwrap().iter() {
            //     eprintln!("{}", pretty(&v.ast));
            // }
            // eprintln!("");

            runlist
        };

        let initial_deps = model.initial_deps.as_ref().unwrap();
        // TODO: we can cut this down to just things needed to initialize stocks,
        //   but thats just an optimization
        let runlist_initials = build_runlist(initial_deps, StepPart::Initials, &|_| true)?;

        let inputs_set: HashSet<Ident> = inputs.iter().cloned().collect();

        let dt_deps = model.dt_deps.as_ref().unwrap();
        let runlist_flows = build_runlist(dt_deps, StepPart::Flows, &|id| {
            inputs_set.contains(*id) || !(&model.variables[*id]).is_stock()
        })?;
        let runlist_stocks = build_runlist(dt_deps, StepPart::Stocks, &|id| {
            let v = &model.variables[*id];
            !inputs_set.contains(*id) && (v.is_stock() || v.is_module())
        })?;

        // flatten out the variables so that we're just dealing with lists of expressions
        let runlist_initials = runlist_initials.into_iter().flat_map(|v| v.ast).collect();
        let runlist_flows = runlist_flows.into_iter().flat_map(|v| v.ast).collect();
        let runlist_stocks = runlist_stocks.into_iter().flat_map(|v| v.ast).collect();

        let tables: Result<HashMap<String, Table>> = var_names
            .iter()
            .map(|id| (id, &model.variables[*id]))
            .filter(|(_, v)| v.table().is_some())
            .map(|(id, v)| (id, Table::new(id, v.table().unwrap())))
            .map(|(id, t)| match t {
                Ok(table) => Ok((id.to_string(), table)),
                Err(err) => Err(err),
            })
            .collect();
        let tables = tables?;

        let offsets = metadata
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.iter()
                        .map(|(k, v)| (k.clone(), (v.offset, v.size)))
                        .collect(),
                )
            })
            .collect();

        Ok(Module {
            ident: model_name.to_string(),
            n_slots,
            runlist_initials,
            runlist_flows,
            runlist_stocks,
            offsets,
            tables,
        })
    }
}

fn is_truthy(n: f64) -> bool {
    let is_false = approx_eq!(f64, n, 0.0);
    !is_false
}

pub struct ModuleEvaluator<'a> {
    step_part: StepPart,
    off: usize,
    inputs: &'a [f64],
    curr: &'a mut [f64],
    next: &'a mut [f64],
    module: &'a Module,
    sim: &'a Simulation,
}

impl<'a> ModuleEvaluator<'a> {
    fn eval(&mut self, expr: &Expr) -> f64 {
        match expr {
            Expr::Const(n) => *n,
            Expr::Dt => self.curr[DT_OFF],
            Expr::ModuleInput(off) => self.inputs[*off],
            Expr::EvalModule(ident, model_name, args) => {
                let args: Vec<f64> = args.iter().map(|arg| self.eval(arg)).collect();
                let module_offsets = &self.module.offsets[&self.module.ident];
                let off = self.off + module_offsets[ident].0;
                let module = &self.sim.modules[model_name.as_str()];

                self.sim
                    .calc(self.step_part, module, off, &args, self.curr, self.next);

                0.0
            }
            Expr::Var(off) => self.curr[self.off + *off],
            Expr::Subscript(off, r, bounds) => {
                let rhs = self.eval(r);
                // we are 1 indexed here, because the spec sucks
                if approx_eq!(f64, rhs, 0.0) || (rhs.floor() as usize) > *bounds {
                    // 3.7.1 Arrays: If a subscript expression results in an invalid subscript index (i.e., it is out of range), a zero (0) MUST be returned[10]
                    // note 10: Note this can be NaN if so specified in the <uses_arrays> tag of the header options block
                    // 0 makes less sense than NaN, so lets do that until real models force us to do otherwise
                    f64::NAN
                } else {
                    self.curr[self.off + *off + ((rhs - 1.0).floor() as usize)]
                }
            }
            Expr::AssignCurr(off, r) => {
                let rhs = self.eval(r);
                if self.off + *off > self.curr.len() {
                    unreachable!();
                }
                self.curr[self.off + *off] = rhs;
                0.0
            }
            Expr::AssignNext(off, r) => {
                let rhs = self.eval(r);
                self.next[self.off + *off] = rhs;
                0.0
            }
            Expr::If(cond, t, f) => {
                let cond: f64 = self.eval(cond);
                if is_truthy(cond) {
                    self.eval(t)
                } else {
                    self.eval(f)
                }
            }
            Expr::Op1(op, l) => {
                let l = self.eval(l);
                match op {
                    UnaryOp::Not => (!is_truthy(l)) as i8 as f64,
                }
            }
            Expr::Op2(op, l, r) => {
                let l = self.eval(l);
                let r = self.eval(r);
                match op {
                    BinaryOp::Add => l + r,
                    BinaryOp::Sub => l - r,
                    BinaryOp::Exp => l.powf(r),
                    BinaryOp::Mul => l * r,
                    BinaryOp::Div => l / r,
                    BinaryOp::Mod => l.rem_euclid(r),
                    BinaryOp::Gt => (l > r) as i8 as f64,
                    BinaryOp::Gte => (l >= r) as i8 as f64,
                    BinaryOp::Lt => (l < r) as i8 as f64,
                    BinaryOp::Lte => (l <= r) as i8 as f64,
                    BinaryOp::Eq => approx_eq!(f64, l, r) as i8 as f64,
                    BinaryOp::Neq => !approx_eq!(f64, l, r) as i8 as f64,
                    BinaryOp::And => (is_truthy(l) && is_truthy(r)) as i8 as f64,
                    BinaryOp::Or => (is_truthy(l) || is_truthy(r)) as i8 as f64,
                }
            }
            Expr::App(builtin) => {
                match builtin {
                    BuiltinFn::Abs(a) => self.eval(a).abs(),
                    BuiltinFn::Cos(a) => self.eval(a).cos(),
                    BuiltinFn::Sin(a) => self.eval(a).sin(),
                    BuiltinFn::Tan(a) => self.eval(a).tan(),
                    BuiltinFn::Arccos(a) => self.eval(a).acos(),
                    BuiltinFn::Arcsin(a) => self.eval(a).asin(),
                    BuiltinFn::Arctan(a) => self.eval(a).atan(),
                    BuiltinFn::Exp(a) => self.eval(a).exp(),
                    BuiltinFn::Inf => std::f64::INFINITY,
                    BuiltinFn::Pi => std::f64::consts::PI,
                    BuiltinFn::Int(a) => self.eval(a).floor(),
                    BuiltinFn::Ln(a) => self.eval(a).ln(),
                    BuiltinFn::Log10(a) => self.eval(a).log10(),
                    BuiltinFn::SafeDiv(a, b, default) => {
                        let a = self.eval(a);
                        let b = self.eval(b);

                        if b != 0.0 {
                            a / b
                        } else if let Some(c) = default {
                            self.eval(c)
                        } else {
                            0.0
                        }
                    }
                    BuiltinFn::Sqrt(a) => self.eval(a).sqrt(),
                    BuiltinFn::Min(a, b) => {
                        let a = self.eval(a);
                        let b = self.eval(b);
                        // we can't use std::cmp::min here, becuase f64 is only
                        // PartialOrd
                        if a < b {
                            a
                        } else {
                            b
                        }
                    }
                    BuiltinFn::Max(a, b) => {
                        let a = self.eval(a);
                        let b = self.eval(b);
                        // we can't use std::cmp::min here, becuase f64 is only
                        // PartialOrd
                        if a > b {
                            a
                        } else {
                            b
                        }
                    }
                    BuiltinFn::Lookup(id, index) => {
                        if !self.module.tables.contains_key(id) {
                            eprintln!("bad lookup for {}", id);
                            unreachable!();
                        }
                        let table = &self.module.tables[id].data;
                        if table.is_empty() {
                            return f64::NAN;
                        }

                        let index = self.eval(index);
                        if index.is_nan() {
                            // things get wonky below if we try to binary search for NaN
                            return f64::NAN;
                        }

                        // check if index is below the start of the table
                        {
                            let (x, y) = table[0];
                            if index < x {
                                return y;
                            }
                        }

                        let size = table.len();
                        {
                            let (x, y) = table[size - 1];
                            if index > x {
                                return y;
                            }
                        }
                        // binary search seems to be the most appropriate choice here.
                        let mut low = 0;
                        let mut high = size;
                        while low < high {
                            let mid = low + (high - low) / 2;
                            if table[mid].0 < index {
                                low = mid + 1;
                            } else {
                                high = mid;
                            }
                        }

                        let i = low;
                        if approx_eq!(f64, table[i].0, index) {
                            table[i].1
                        } else {
                            // slope = deltaY/deltaX
                            let slope =
                                (table[i].1 - table[i - 1].1) / (table[i].0 - table[i - 1].0);
                            // y = m*x + b
                            (index - table[i - 1].0) * slope + table[i - 1].1
                        }
                    }
                    BuiltinFn::Pulse(a, b, c) => {
                        let time = self.curr[TIME_OFF];
                        let volume = self.eval(a);
                        let first_pulse = self.eval(b);
                        let interval = self.eval(c);

                        if time < first_pulse {
                            return 0.0;
                        }

                        let dt = self.curr[DT_OFF];
                        let mut next_pulse = first_pulse;
                        while time >= next_pulse {
                            if time < next_pulse + dt {
                                return volume / dt;
                            } else if interval <= 0.0 {
                                break;
                            } else {
                                next_pulse += interval;
                            }
                        }

                        0.0
                    }
                }
            }
        }
    }
}

#[allow(dead_code)]
pub fn pretty(expr: &Expr) -> String {
    match expr {
        Expr::Const(n) => format!("{}", n),
        Expr::Var(off) => format!("curr[{}]", off),
        Expr::Subscript(off, r, bounds) => {
            format!("curr[{} + (({}) - 1); bounds: {}]", off, pretty(r), bounds)
        }
        Expr::Dt => "dt".to_string(),
        Expr::App(builtin) => match builtin {
            BuiltinFn::Lookup(table, idx) => format!("lookup({}, {})", table, pretty(idx)),
            BuiltinFn::Abs(l) => format!("abs({})", pretty(l)),
            BuiltinFn::Arccos(l) => format!("arccos({})", pretty(l)),
            BuiltinFn::Arcsin(l) => format!("arcsin({})", pretty(l)),
            BuiltinFn::Arctan(l) => format!("arctan({})", pretty(l)),
            BuiltinFn::Cos(l) => format!("cos({})", pretty(l)),
            BuiltinFn::Exp(l) => format!("exp({})", pretty(l)),
            BuiltinFn::Inf => "∞".to_string(),
            BuiltinFn::Int(l) => format!("int({})", pretty(l)),
            BuiltinFn::Ln(l) => format!("ln({})", pretty(l)),
            BuiltinFn::Log10(l) => format!("log10({})", pretty(l)),
            BuiltinFn::Max(l, r) => format!("max({}, {})", pretty(l), pretty(r)),
            BuiltinFn::Min(l, r) => format!("min({}, {})", pretty(l), pretty(r)),
            BuiltinFn::Pi => "𝜋".to_string(),
            BuiltinFn::Pulse(a, b, c) => {
                format!("pulse({}, {}, {})", pretty(a), pretty(b), pretty(c))
            }
            BuiltinFn::SafeDiv(a, b, c) => format!(
                "safediv({}, {}, {})",
                pretty(a),
                pretty(b),
                c.as_ref()
                    .map(|expr| pretty(expr))
                    .unwrap_or_else(|| "<None>".to_string())
            ),
            BuiltinFn::Sin(l) => format!("sin({})", pretty(l)),
            BuiltinFn::Sqrt(l) => format!("sqrt({})", pretty(l)),
            BuiltinFn::Tan(l) => format!("tan({})", pretty(l)),
        },
        Expr::EvalModule(module, model_name, args) => {
            let args: Vec<_> = args.iter().map(|arg| pretty(arg)).collect();
            let string_args = args.join(", ");
            format!("eval<{}::{}>({})", module, model_name, string_args)
        }
        Expr::ModuleInput(a) => format!("mi<{}>", a),
        Expr::Op2(op, l, r) => {
            let op: &str = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Exp => "^",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "%",
                BinaryOp::Gt => ">",
                BinaryOp::Gte => ">=",
                BinaryOp::Lt => "<",
                BinaryOp::Lte => "<=",
                BinaryOp::Eq => "==",
                BinaryOp::Neq => "!=",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
            };

            format!("({}{}{})", pretty(l), op, pretty(r))
        }
        Expr::Op1(op, l) => {
            let op: &str = match op {
                UnaryOp::Not => "!",
            };
            format!("{}{}", op, pretty(l))
        }
        Expr::If(cond, l, r) => {
            format!("if {} then {} else {}", pretty(cond), pretty(l), pretty(r))
        }
        Expr::AssignCurr(off, rhs) => format!("curr[{}] := {}", off, pretty(rhs)),
        Expr::AssignNext(off, rhs) => format!("next[{}] := {}", off, pretty(rhs)),
    }
}

#[derive(Debug)]
pub struct Results {
    pub offsets: HashMap<String, usize>,
    // one large allocation
    pub data: Box<[f64]>,
    pub step_size: usize,
    pub step_count: usize,
    pub specs: Specs,
}

impl Results {
    pub fn print_tsv(&self) {
        let var_names = {
            let offset_name_map: HashMap<usize, &str> =
                self.offsets.iter().map(|(k, v)| (*v, k.as_str())).collect();
            let mut var_names: Vec<&str> = Vec::with_capacity(self.step_size);
            for i in 0..(self.step_size) {
                let name = if offset_name_map.contains_key(&i) {
                    offset_name_map[&i]
                } else {
                    "UNKNOWN"
                };
                var_names.push(name);
            }
            var_names
        };

        // print header
        for (i, id) in var_names.iter().enumerate() {
            print!("{}", id);
            if i == var_names.len() - 1 {
                println!();
            } else {
                print!("\t");
            }
        }

        for curr in self.data.chunks(self.step_size) {
            if curr[TIME_OFF] > self.specs.stop {
                break;
            }
            for (i, val) in curr.iter().enumerate() {
                print!("{}", val);
                if i == var_names.len() - 1 {
                    println!();
                } else {
                    print!("\t");
                }
            }
        }
    }

    pub fn iter(&self) -> std::slice::Chunks<f64> {
        self.data.chunks(self.step_size)
    }
}

#[derive(Debug)]
pub struct Simulation {
    modules: HashMap<Ident, Module>,
    specs: Specs,
    root: String,
    project: Rc<Project>,
}

fn enumerate_modules(
    project: &Project,
    model_name: &str,
    modules: &mut HashSet<(Ident, Vec<Ident>)>,
) -> Result<()> {
    use crate::common::{ErrorCode, ErrorKind};
    let model = project.models.get(model_name).ok_or_else(|| Error {
        kind: ErrorKind::Simulation,
        code: ErrorCode::NotSimulatable,
        details: Some(format!("model for module '{}' not found", model_name)),
    })?;
    let model = Rc::clone(model);
    for (_id, v) in model.variables.iter() {
        if let Variable::Module {
            model_name, inputs, ..
        } = v
        {
            let mut inputs: Vec<String> = inputs.iter().map(|input| input.dst.clone()).collect();
            inputs.sort_unstable();
            if modules.insert((model_name.clone(), inputs)) {
                // first time we're seeing this monomorphization; recurse
                enumerate_modules(project, model_name, modules)?;
            }
        }
    }

    Ok(())
}

impl Simulation {
    pub fn new(project_rc: &Rc<Project>, main_model_name: &str) -> Result<Self> {
        let project = project_rc.as_ref();
        if !project.models.contains_key(main_model_name) {
            return sim_err!(
                NotSimulatable,
                format!("no model named '{}' to simulate", main_model_name)
            );
        }
        let mut modules: HashSet<(Ident, Vec<Ident>)> = HashSet::new();
        modules.insert((main_model_name.to_string(), vec![]));
        enumerate_modules(project, main_model_name, &mut modules)?;

        let module_names: Vec<&str> = {
            let mut module_names: Vec<&str> = modules.iter().map(|(id, _)| id.as_str()).collect();
            module_names.sort_unstable();

            let mut sorted_names = vec![main_model_name];
            sorted_names.extend(module_names.into_iter().filter(|n| *n != main_model_name));
            sorted_names
        };

        let mut compiled_modules: HashMap<Ident, Module> = HashMap::new();
        for name in module_names {
            for (_, inputs) in modules.iter().filter(|(n, _)| n == name) {
                let model = Rc::clone(&project.models[name]);
                let is_root = name == main_model_name;
                let module = Module::new(project, model, inputs, is_root)?;
                compiled_modules.insert(name.to_string(), module);
            }
        }

        // module assign offsets

        // reset

        let specs = Specs::from(&project.datamodel.sim_specs);

        Ok(Simulation {
            modules: compiled_modules,
            specs,
            root: main_model_name.to_string(),
            project: Rc::clone(project_rc),
        })
    }

    pub fn debug_print_runlists(&self, _model_name: &str) {
        let mut model_names: Vec<_> = self.modules.keys().collect();
        model_names.sort_unstable();
        for model_name in model_names {
            eprintln!("\n\nMODEL: {}", model_name);
            let module = &self.modules[model_name];
            let offsets = &module.offsets[model_name];
            let mut idents: Vec<_> = offsets.keys().collect();
            idents.sort_unstable();

            eprintln!("offsets");
            for ident in idents {
                let (off, size) = offsets[ident];
                eprintln!("\t{}: {}, {}", ident, off, size);
            }

            eprintln!("\ninital runlist:");
            for expr in module.runlist_initials.iter() {
                eprintln!("\t{}", pretty(expr));
            }

            eprintln!("\nflows runlist:");
            for expr in module.runlist_flows.iter() {
                eprintln!("\t{}", pretty(expr));
            }

            eprintln!("\nstocks runlist:");
            for expr in module.runlist_stocks.iter() {
                eprintln!("\t{}", pretty(expr));
            }
        }
    }

    fn calc(
        &self,
        step_part: StepPart,
        module: &Module,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
    ) {
        let runlist = match step_part {
            StepPart::Initials => &module.runlist_initials,
            StepPart::Flows => &module.runlist_flows,
            StepPart::Stocks => &module.runlist_stocks,
        };

        let mut step = ModuleEvaluator {
            step_part,
            off: module_off,
            curr,
            next,
            module,
            inputs: module_inputs,
            sim: self,
        };

        for expr in runlist.iter() {
            step.eval(expr);
        }
    }

    fn n_slots(&self, module_name: &str) -> usize {
        self.modules[module_name].n_slots
    }

    pub fn run_to_end(&self) -> Result<Results> {
        let spec = &self.specs;
        if spec.stop < spec.start {
            return sim_err!(BadSimSpecs, "".to_string());
        }
        let save_step = if spec.save_step > spec.dt {
            spec.save_step
        } else {
            spec.dt
        };
        let n_chunks: usize = ((spec.stop - spec.start) / save_step + 1.0) as usize;
        let save_every = std::cmp::max(1, (spec.save_step / spec.dt + 0.5).floor() as usize);

        let dt = spec.dt;
        let stop = spec.stop;

        let n_slots = self.n_slots(&self.root);

        let module = &self.modules[&self.root];

        let slab: Vec<f64> = vec![0.0; n_slots * (n_chunks + 1)];
        let mut boxed_slab = slab.into_boxed_slice();
        {
            let mut slabs = boxed_slab.chunks_mut(n_slots);

            // let mut results: Vec<&[f64]> = Vec::with_capacity(n_chunks + 1);

            let module_inputs: &[f64] = &[];

            let mut curr = slabs.next().unwrap();
            let mut next = slabs.next().unwrap();
            curr[TIME_OFF] = self.specs.start;
            curr[DT_OFF] = dt;
            curr[INITIAL_TIME_OFF] = self.specs.start;
            curr[FINAL_TIME_OFF] = self.specs.stop;
            self.calc(StepPart::Initials, module, 0, module_inputs, curr, next);
            let mut is_initial_timestep = true;
            let mut step = 0;
            loop {
                self.calc(StepPart::Flows, module, 0, module_inputs, curr, next);
                self.calc(StepPart::Stocks, module, 0, module_inputs, curr, next);
                next[TIME_OFF] = curr[TIME_OFF] + dt;
                next[DT_OFF] = dt;
                curr[INITIAL_TIME_OFF] = self.specs.start;
                curr[FINAL_TIME_OFF] = self.specs.stop;
                step += 1;
                if step != save_every && !is_initial_timestep {
                    let curr = curr.borrow_mut();
                    curr.copy_from_slice(next);
                } else {
                    curr = next;
                    let maybe_next = slabs.next();
                    if maybe_next.is_none() {
                        break;
                    }
                    next = maybe_next.unwrap();
                    step = 0;
                    is_initial_timestep = false;
                }
            }
            // ensure we've calculated stock + flow values for the dt <= end_time
            assert!(curr[TIME_OFF] > stop);
        }

        let offsets = calc_flattened_offsets(&self.project, &module.ident);
        let offsets: HashMap<Ident, usize> =
            offsets.into_iter().map(|(k, (off, _))| (k, off)).collect();

        Ok(Results {
            offsets,
            data: boxed_slab,
            step_size: n_slots,
            step_count: n_chunks,
            specs: spec.clone(),
        })
    }
}

#[test]
fn test_arrays() {
    let project = {
        use crate::datamodel::*;
        Project {
            name: "arrays".to_owned(),
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 12.0,
                dt: Dt::Dt(0.25),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_owned()),
            },
            dimensions: vec![Dimension {
                name: "letters".to_owned(),
                elements: vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
            }],
            models: vec![Model {
                name: "main".to_owned(),
                variables: vec![
                    Variable::Aux(Aux {
                        ident: "constants".to_owned(),
                        equation: Equation::Arrayed(
                            vec!["letters".to_owned()],
                            vec![
                                ("a".to_owned(), "9".to_owned()),
                                ("b".to_owned(), "7".to_owned()),
                                ("c".to_owned(), "5".to_owned()),
                            ],
                        ),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                    }),
                    Variable::Aux(Aux {
                        ident: "picked".to_owned(),
                        equation: Equation::Scalar("aux[INT(TIME MOD 5) + 1]".to_owned()),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                    }),
                    Variable::Aux(Aux {
                        ident: "aux".to_owned(),
                        equation: Equation::ApplyToAll(
                            vec!["letters".to_owned()],
                            "constants".to_owned(),
                        ),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                    }),
                    Variable::Aux(Aux {
                        ident: "picked2".to_owned(),
                        equation: Equation::Scalar("aux[b]".to_owned()),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                    }),
                ],
                views: vec![],
            }],
        }
    };

    let parsed_project = Rc::new(Project::from(project));

    {
        let actual = calc_flattened_offsets(&parsed_project, "main");
        let expected: HashMap<_, _> = vec![
            ("time".to_owned(), (0, 1)),
            ("dt".to_owned(), (1, 1)),
            ("initial_time".to_owned(), (2, 1)),
            ("final_time".to_owned(), (3, 1)),
            ("aux[a]".to_owned(), (4, 1)),
            ("aux[b]".to_owned(), (5, 1)),
            ("aux[c]".to_owned(), (6, 1)),
            ("constants[a]".to_owned(), (7, 1)),
            ("constants[b]".to_owned(), (8, 1)),
            ("constants[c]".to_owned(), (9, 1)),
            ("picked".to_owned(), (10, 1)),
            ("picked2".to_owned(), (11, 1)),
        ]
        .into_iter()
        .collect();
        assert_eq!(actual, expected);
    }

    let metadata = build_metadata(&parsed_project, "main", true);
    let main_metadata = &metadata["main"];
    assert_eq!(main_metadata["aux"].offset, 4);
    assert_eq!(main_metadata["aux"].size, 3);
    assert_eq!(main_metadata["constants"].offset, 7);
    assert_eq!(main_metadata["constants"].size, 3);
    assert_eq!(main_metadata["picked"].offset, 10);
    assert_eq!(main_metadata["picked"].size, 1);
    assert_eq!(main_metadata["picked2"].offset, 11);
    assert_eq!(main_metadata["picked2"].size, 1);

    let module_models = calc_module_model_map(&parsed_project, "main");

    let arrayed_constants_var = &parsed_project.models["main"].variables["constants"];
    let parsed_var = Var::new(
        &Context {
            dimensions: &parsed_project.datamodel.dimensions,
            model_name: "main",
            ident: arrayed_constants_var.ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &[],
        },
        arrayed_constants_var,
    );

    assert!(parsed_var.is_ok());

    let expected = Var {
        ast: vec![
            Expr::AssignCurr(7, Box::new(Expr::Const(9.0))),
            Expr::AssignCurr(8, Box::new(Expr::Const(7.0))),
            Expr::AssignCurr(9, Box::new(Expr::Const(5.0))),
        ],
    };
    assert_eq!(expected, parsed_var.unwrap());

    let arrayed_aux_var = &parsed_project.models["main"].variables["aux"];
    let parsed_var = Var::new(
        &Context {
            dimensions: &parsed_project.datamodel.dimensions,
            model_name: "main",
            ident: arrayed_aux_var.ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &[],
        },
        arrayed_aux_var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ast: vec![
            Expr::AssignCurr(4, Box::new(Expr::Var(7))),
            Expr::AssignCurr(5, Box::new(Expr::Var(8))),
            Expr::AssignCurr(6, Box::new(Expr::Var(9))),
        ],
    };
    assert_eq!(expected, parsed_var.unwrap());

    let var = &parsed_project.models["main"].variables["picked2"];
    let parsed_var = Var::new(
        &Context {
            dimensions: &parsed_project.datamodel.dimensions,
            model_name: "main",
            ident: var.ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &[],
        },
        var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ast: vec![Expr::AssignCurr(
            11,
            Box::new(Expr::Subscript(4, Box::new(Expr::Const(2.0)), 3)),
        )],
    };
    assert_eq!(expected, parsed_var.unwrap());

    let var = &parsed_project.models["main"].variables["picked"];
    let parsed_var = Var::new(
        &Context {
            dimensions: &parsed_project.datamodel.dimensions,
            model_name: "main",
            ident: var.ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &[],
        },
        var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ast: vec![Expr::AssignCurr(
            10,
            Box::new(Expr::Subscript(
                4,
                Box::new(Expr::Op2(
                    BinaryOp::Add,
                    Box::new(Expr::App(BuiltinFn::Int(Box::new(Expr::Op2(
                        BinaryOp::Mod,
                        Box::new(Expr::Var(0)), // TIME
                        Box::new(Expr::Const(5.0)),
                    ))))),
                    Box::new(Expr::Const(1.0)),
                )),
                3,
            )),
        )],
    };
    assert_eq!(expected, parsed_var.unwrap());

    let sim = Simulation::new(&parsed_project, "main");
    assert!(sim.is_ok());
}
